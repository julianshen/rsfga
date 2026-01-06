//! Batch check handler with two-stage deduplication.
//!
//! This handler processes multiple permission checks in a single request,
//! optimizing throughput through:
//!
//! 1. **Intra-batch deduplication**: Identical checks execute only once
//! 2. **Singleflight**: Concurrent requests for same check share results
//!
//! Note: Cache integration (stage 3) is handled at the API layer where
//! the CheckCache is consulted before invoking the batch handler.
//!
//! # API Format Adaptation
//!
//! This module uses a simplified internal format for the domain layer.
//! The HTTP API layer (`rsfga-api`) adapts to OpenFGA's wire format:
//!
//! - **Request**: OpenFGA uses `correlation_id` and nested `tuple_key` objects.
//!   The API layer extracts these and maps to our flat `BatchCheckItem`.
//! - **Response**: OpenFGA returns `{ "result": { "<correlation_id>": {...} } }`.
//!   The API layer maps our `Vec<BatchCheckItemResult>` back using correlation IDs.
//!
//! # Performance Target (UNVALIDATED - M1.8)
//!
//! - Batch throughput: >500 checks/s
//! - Deduplication effectiveness: >90% on typical workloads

use std::collections::HashMap;
use std::sync::Arc;

use dashmap::DashMap;
use futures::future::join_all;
use rsfga_domain::cache::CheckCache;
use rsfga_domain::resolver::{CheckRequest, GraphResolver, ModelReader, TupleReader};
use tokio::sync::broadcast;

/// Maximum batch size per OpenFGA specification.
/// OpenFGA enforces a limit of 50 items per batch-check request.
const MAX_BATCH_SIZE: usize = 50;

/// Key for identifying unique checks (used for deduplication).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CheckKey {
    store_id: String,
    user: String,
    relation: String,
    object: String,
}

impl CheckKey {
    fn new(store_id: &str, item: &BatchCheckItem) -> Self {
        Self {
            store_id: store_id.to_string(),
            user: item.user.clone(),
            relation: item.relation.clone(),
            object: item.object.clone(),
        }
    }
}

/// Result type for singleflight operations.
#[derive(Debug, Clone)]
struct SingleflightResult {
    allowed: bool,
    error: Option<String>,
}

/// Result of trying to acquire a singleflight slot.
enum SingleflightSlot {
    /// We won the race and should execute the check.
    /// Contains the sender to broadcast results.
    Leader(broadcast::Sender<SingleflightResult>),
    /// Another task is executing; wait for its result.
    Follower(broadcast::Receiver<SingleflightResult>),
}

/// Singleflight implementation for deduplicating concurrent check requests.
///
/// When multiple requests for the same check arrive concurrently,
/// only one actual check is executed and all requesters share the result.
///
/// Uses atomic operations to prevent race conditions between checking
/// for existing requests and registering new ones.
struct Singleflight {
    /// Map of in-flight requests to their broadcast senders.
    in_flight: DashMap<CheckKey, broadcast::Sender<SingleflightResult>>,
}

impl Singleflight {
    fn new() -> Self {
        Self {
            in_flight: DashMap::new(),
        }
    }

    /// Atomically try to acquire a slot for this check.
    ///
    /// Returns `Leader` if this caller should execute the check,
    /// or `Follower` if another caller is already executing it.
    ///
    /// This uses DashMap's entry API for atomic check-and-insert,
    /// preventing race conditions between try_join and register.
    fn acquire(&self, key: CheckKey) -> SingleflightSlot {
        use dashmap::mapref::entry::Entry;

        match self.in_flight.entry(key) {
            Entry::Occupied(entry) => {
                // Another task is already executing this check
                SingleflightSlot::Follower(entry.get().subscribe())
            }
            Entry::Vacant(entry) => {
                // We're the first - register and become the leader
                let (tx, _rx) = broadcast::channel(1);
                entry.insert(tx.clone());
                SingleflightSlot::Leader(tx)
            }
        }
    }

    /// Remove a completed in-flight request.
    fn complete(&self, key: &CheckKey) {
        self.in_flight.remove(key);
    }
}

/// RAII guard that ensures singleflight cleanup on drop.
///
/// This prevents resource leaks if the check execution panics.
struct SingleflightGuard<'a> {
    singleflight: &'a Singleflight,
    key: CheckKey,
    completed: bool,
}

impl<'a> SingleflightGuard<'a> {
    fn new(singleflight: &'a Singleflight, key: CheckKey) -> Self {
        Self {
            singleflight,
            key,
            completed: false,
        }
    }

    /// Mark as completed (normal path, not panic).
    fn complete(mut self) {
        self.singleflight.complete(&self.key);
        self.completed = true;
    }
}

impl<'a> Drop for SingleflightGuard<'a> {
    fn drop(&mut self) {
        // If not already completed (e.g., due to panic), clean up
        if !self.completed {
            self.singleflight.complete(&self.key);
        }
    }
}

/// A single check within a batch request.
#[derive(Debug, Clone)]
pub struct BatchCheckItem {
    /// The user performing the access (e.g., "user:alice").
    pub user: String,
    /// The relation to check (e.g., "viewer").
    pub relation: String,
    /// The object identifier (e.g., "document:readme").
    pub object: String,
}

/// Request for batch permission checks.
#[derive(Debug, Clone)]
pub struct BatchCheckRequest {
    /// The store ID to check against.
    pub store_id: String,
    /// The list of checks to perform.
    pub checks: Vec<BatchCheckItem>,
}

impl BatchCheckRequest {
    /// Creates a new batch check request.
    pub fn new(store_id: impl Into<String>, checks: Vec<BatchCheckItem>) -> Self {
        Self {
            store_id: store_id.into(),
            checks,
        }
    }
}

/// Result of a single check within a batch.
#[derive(Debug, Clone)]
pub struct BatchCheckItemResult {
    /// Whether the check is allowed.
    pub allowed: bool,
    /// Error message if the check failed (optional).
    pub error: Option<String>,
}

/// Response from a batch check operation.
#[derive(Debug, Clone)]
pub struct BatchCheckResponse {
    /// Results for each check, in the same order as the request.
    pub results: Vec<BatchCheckItemResult>,
}

/// Errors that can occur during batch check operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum BatchCheckError {
    /// The batch request is empty.
    #[error("batch request cannot be empty")]
    EmptyBatch,

    /// The batch request exceeds the maximum allowed size.
    #[error("batch size {size} exceeds maximum allowed {max}")]
    BatchTooLarge { size: usize, max: usize },

    /// A check item has invalid format.
    #[error("invalid check at index {index}: {message}")]
    InvalidCheck { index: usize, message: String },

    /// Domain error during check execution.
    #[error("check error: {0}")]
    DomainError(String),
}

impl From<rsfga_domain::error::DomainError> for BatchCheckError {
    fn from(err: rsfga_domain::error::DomainError) -> Self {
        BatchCheckError::DomainError(err.to_string())
    }
}

/// Result type for batch check operations.
pub type BatchCheckResult<T> = Result<T, BatchCheckError>;

/// Handler for batch permission checks.
///
/// Processes multiple checks in parallel with two-stage deduplication:
/// 1. Intra-batch: Identical checks within a batch execute once
/// 2. Singleflight: Concurrent requests across batches share results
///
/// Cache integration is handled at the API layer.
pub struct BatchCheckHandler<T, M>
where
    T: TupleReader,
    M: ModelReader,
{
    /// The graph resolver for executing checks.
    resolver: Arc<GraphResolver<T, M>>,
    /// Cache for storing check results (used by API layer).
    #[allow(dead_code)]
    cache: Arc<CheckCache>,
    /// Singleflight for deduplicating concurrent requests.
    singleflight: Arc<Singleflight>,
}

impl<T, M> BatchCheckHandler<T, M>
where
    T: TupleReader + 'static,
    M: ModelReader + 'static,
{
    /// Creates a new batch check handler.
    pub fn new(resolver: Arc<GraphResolver<T, M>>, cache: Arc<CheckCache>) -> Self {
        Self {
            resolver,
            cache,
            singleflight: Arc::new(Singleflight::new()),
        }
    }

    /// Validates a batch check request.
    pub fn validate(&self, request: &BatchCheckRequest) -> BatchCheckResult<()> {
        // Check for empty batch
        if request.checks.is_empty() {
            return Err(BatchCheckError::EmptyBatch);
        }

        // Check batch size limit
        if request.checks.len() > MAX_BATCH_SIZE {
            return Err(BatchCheckError::BatchTooLarge {
                size: request.checks.len(),
                max: MAX_BATCH_SIZE,
            });
        }

        // Validate each check item
        for (index, check) in request.checks.iter().enumerate() {
            if check.user.is_empty() {
                return Err(BatchCheckError::InvalidCheck {
                    index,
                    message: "user cannot be empty".to_string(),
                });
            }
            if check.relation.is_empty() {
                return Err(BatchCheckError::InvalidCheck {
                    index,
                    message: "relation cannot be empty".to_string(),
                });
            }
            if check.object.is_empty() {
                return Err(BatchCheckError::InvalidCheck {
                    index,
                    message: "object cannot be empty".to_string(),
                });
            }
        }

        Ok(())
    }

    /// Executes a batch check request.
    ///
    /// The results are returned in the same order as the input checks.
    pub async fn check(&self, request: BatchCheckRequest) -> BatchCheckResult<BatchCheckResponse> {
        // Validate the request
        self.validate(&request)?;

        // Stage 1: Intra-batch deduplication
        // Build a map of unique checks and track which positions map to each unique check
        let mut unique_checks: Vec<&BatchCheckItem> = Vec::new();
        let mut key_to_index: HashMap<CheckKey, usize> = HashMap::new();
        let mut position_to_unique: Vec<usize> = Vec::with_capacity(request.checks.len());

        for check in &request.checks {
            let key = CheckKey::new(&request.store_id, check);
            let unique_index = *key_to_index.entry(key).or_insert_with(|| {
                let idx = unique_checks.len();
                unique_checks.push(check);
                idx
            });
            position_to_unique.push(unique_index);
        }

        // Execute unique checks in parallel with singleflight (Stage 2: Cross-request dedup)
        let check_futures: Vec<_> = unique_checks
            .iter()
            .map(|check| {
                let key = CheckKey::new(&request.store_id, check);
                self.execute_check_with_singleflight(&request.store_id, check, key)
            })
            .collect();

        let unique_results: Vec<BatchCheckItemResult> = join_all(check_futures).await;

        // Map results back to original positions
        let results: Vec<BatchCheckItemResult> = position_to_unique
            .iter()
            .map(|&idx| unique_results[idx].clone())
            .collect();

        Ok(BatchCheckResponse { results })
    }

    /// Execute a single check with singleflight deduplication.
    ///
    /// If there's already an in-flight request for this check, wait for its result.
    /// Otherwise, execute the check and broadcast the result to any waiters.
    ///
    /// Uses atomic acquire() to prevent race conditions and SingleflightGuard
    /// for cleanup on panic.
    async fn execute_check_with_singleflight(
        &self,
        store_id: &str,
        check: &BatchCheckItem,
        key: CheckKey,
    ) -> BatchCheckItemResult {
        // Atomically acquire a slot - either become leader or follower
        match self.singleflight.acquire(key.clone()) {
            SingleflightSlot::Follower(mut receiver) => {
                // Wait for the leader's result
                match receiver.recv().await {
                    Ok(result) => BatchCheckItemResult {
                        allowed: result.allowed,
                        error: result.error,
                    },
                    Err(_) => {
                        // Leader was dropped (likely panicked), retry as new leader
                        // This is safe because SingleflightGuard cleaned up
                        Box::pin(self.execute_check_with_singleflight(store_id, check, key)).await
                    }
                }
            }
            SingleflightSlot::Leader(sender) => {
                // We're the leader - create guard for cleanup on panic
                let guard = SingleflightGuard::new(&self.singleflight, key);

                // Execute the actual check
                let check_request = CheckRequest::new(
                    store_id.to_string(),
                    check.user.clone(),
                    check.relation.clone(),
                    check.object.clone(),
                    vec![], // No contextual tuples in batch checks
                );

                let result = match self.resolver.check(&check_request).await {
                    Ok(result) => SingleflightResult {
                        allowed: result.allowed,
                        error: None,
                    },
                    Err(e) => SingleflightResult {
                        allowed: false,
                        error: Some(e.to_string()),
                    },
                };

                // Broadcast the result to any waiters (ignore send errors - no receivers)
                let _ = sender.send(result.clone());

                // Complete and clean up (guard handles this)
                guard.complete();

                BatchCheckItemResult {
                    allowed: result.allowed,
                    error: result.error,
                }
            }
        }
    }

    /// Returns statistics about deduplication for a batch request.
    /// Returns (total_checks, unique_checks).
    pub fn dedup_stats(&self, request: &BatchCheckRequest) -> (usize, usize) {
        let mut seen: HashMap<CheckKey, ()> = HashMap::new();
        for check in &request.checks {
            let key = CheckKey::new(&request.store_id, check);
            seen.entry(key).or_insert(());
        }
        (request.checks.len(), seen.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use rsfga_domain::cache::CheckCacheConfig;
    use rsfga_domain::error::{DomainError, DomainResult};
    use rsfga_domain::model::{AuthorizationModel, RelationDefinition, TypeDefinition, Userset};
    use rsfga_domain::resolver::StoredTupleRef;

    // Mock tuple reader for testing
    #[derive(Clone)]
    struct MockTupleReader {
        tuples: Vec<StoredTupleRef>,
    }

    impl MockTupleReader {
        fn new() -> Self {
            Self { tuples: vec![] }
        }

        #[allow(dead_code)]
        fn with_tuples(tuples: Vec<StoredTupleRef>) -> Self {
            Self { tuples }
        }
    }

    #[async_trait]
    impl TupleReader for MockTupleReader {
        async fn read_tuples(
            &self,
            _store_id: &str,
            _object_type: &str,
            _object_id: &str,
            _relation: &str,
        ) -> DomainResult<Vec<StoredTupleRef>> {
            Ok(self.tuples.clone())
        }

        async fn store_exists(&self, _store_id: &str) -> DomainResult<bool> {
            Ok(true)
        }
    }

    // Mock model reader for testing
    #[derive(Clone)]
    struct MockModelReader {
        model: AuthorizationModel,
    }

    impl MockModelReader {
        fn new() -> Self {
            Self {
                model: AuthorizationModel::new("1.1"),
            }
        }

        fn with_type(mut self, type_def: TypeDefinition) -> Self {
            self.model.type_definitions.push(type_def);
            self
        }
    }

    #[async_trait]
    impl ModelReader for MockModelReader {
        async fn get_model(&self, _store_id: &str) -> DomainResult<AuthorizationModel> {
            Ok(self.model.clone())
        }

        async fn get_type_definition(
            &self,
            _store_id: &str,
            type_name: &str,
        ) -> DomainResult<TypeDefinition> {
            self.model
                .type_definitions
                .iter()
                .find(|t| t.type_name == type_name)
                .cloned()
                .ok_or_else(|| DomainError::TypeNotFound {
                    type_name: type_name.to_string(),
                })
        }

        async fn get_relation_definition(
            &self,
            _store_id: &str,
            type_name: &str,
            relation: &str,
        ) -> DomainResult<RelationDefinition> {
            let type_def = self
                .model
                .type_definitions
                .iter()
                .find(|t| t.type_name == type_name)
                .ok_or_else(|| DomainError::TypeNotFound {
                    type_name: type_name.to_string(),
                })?;

            type_def
                .relations
                .iter()
                .find(|r| r.name == relation)
                .cloned()
                .ok_or_else(|| DomainError::RelationNotFound {
                    type_name: type_name.to_string(),
                    relation: relation.to_string(),
                })
        }
    }

    // Helper to create a test handler with mocks
    fn create_test_handler() -> BatchCheckHandler<MockTupleReader, MockModelReader> {
        let tuple_reader = MockTupleReader::new();
        let model_reader = MockModelReader::new().with_type(TypeDefinition {
            type_name: "document".to_string(),
            relations: vec![RelationDefinition {
                name: "viewer".to_string(),
                type_constraints: vec!["user".to_string()],
                rewrite: Userset::This,
            }],
        });

        let resolver = Arc::new(GraphResolver::new(
            Arc::new(tuple_reader),
            Arc::new(model_reader),
        ));
        let cache = Arc::new(CheckCache::new(CheckCacheConfig::default()));

        BatchCheckHandler::new(resolver, cache)
    }

    // ============================================================
    // Section 1: Batch Request Parsing
    // ============================================================

    #[test]
    fn test_can_parse_batch_check_request() {
        // Arrange
        let checks = vec![
            BatchCheckItem {
                user: "user:alice".to_string(),
                relation: "viewer".to_string(),
                object: "document:doc1".to_string(),
            },
            BatchCheckItem {
                user: "user:bob".to_string(),
                relation: "editor".to_string(),
                object: "document:doc2".to_string(),
            },
        ];

        // Act
        let request = BatchCheckRequest::new("store1", checks);

        // Assert
        assert_eq!(request.store_id, "store1");
        assert_eq!(request.checks.len(), 2);
        assert_eq!(request.checks[0].user, "user:alice");
        assert_eq!(request.checks[1].user, "user:bob");
    }

    #[test]
    fn test_validates_each_check_in_batch() {
        // Arrange
        let handler = create_test_handler();
        let request = BatchCheckRequest::new(
            "store1",
            vec![
                BatchCheckItem {
                    user: "user:alice".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:doc1".to_string(),
                },
                BatchCheckItem {
                    user: "".to_string(), // Invalid: empty user
                    relation: "viewer".to_string(),
                    object: "document:doc2".to_string(),
                },
            ],
        );

        // Act
        let result = handler.validate(&request);

        // Assert
        assert!(result.is_err());
        match result.unwrap_err() {
            BatchCheckError::InvalidCheck { index, message } => {
                assert_eq!(index, 1);
                assert!(message.contains("user"));
            }
            _ => panic!("Expected InvalidCheck error"),
        }
    }

    #[test]
    fn test_rejects_empty_batch() {
        // Arrange
        let handler = create_test_handler();
        let request = BatchCheckRequest::new("store1", vec![]);

        // Act
        let result = handler.validate(&request);

        // Assert
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), BatchCheckError::EmptyBatch));
    }

    #[test]
    fn test_accepts_batch_with_single_check() {
        // Arrange
        let handler = create_test_handler();
        let request = BatchCheckRequest::new(
            "store1",
            vec![BatchCheckItem {
                user: "user:alice".to_string(),
                relation: "viewer".to_string(),
                object: "document:doc1".to_string(),
            }],
        );

        // Act
        let result = handler.validate(&request);

        // Assert
        assert!(result.is_ok());
    }

    #[test]
    fn test_accepts_batch_near_max_size() {
        // Arrange - test with batch size just under the limit
        let handler = create_test_handler();
        let checks: Vec<BatchCheckItem> = (0..45) // Below MAX_BATCH_SIZE (50)
            .map(|i| BatchCheckItem {
                user: format!("user:user{}", i),
                relation: "viewer".to_string(),
                object: format!("document:doc{}", i),
            })
            .collect();
        let request = BatchCheckRequest::new("store1", checks);

        // Act
        let result = handler.validate(&request);

        // Assert
        assert!(result.is_ok());
        assert_eq!(request.checks.len(), 45);
    }

    #[test]
    fn test_rejects_batch_exceeding_max_size() {
        // Arrange - OpenFGA enforces max 50 items per batch
        let handler = create_test_handler();
        let checks: Vec<BatchCheckItem> = (0..51) // MAX_BATCH_SIZE + 1
            .map(|i| BatchCheckItem {
                user: format!("user:user{}", i),
                relation: "viewer".to_string(),
                object: format!("document:doc{}", i),
            })
            .collect();
        let request = BatchCheckRequest::new("store1", checks);

        // Act
        let result = handler.validate(&request);

        // Assert
        assert!(result.is_err());
        match result.unwrap_err() {
            BatchCheckError::BatchTooLarge { size, max } => {
                assert_eq!(size, 51);
                assert_eq!(max, MAX_BATCH_SIZE);
            }
            _ => panic!("Expected BatchTooLarge error"),
        }
    }

    #[test]
    fn test_accepts_batch_at_max_size() {
        // Arrange
        let handler = create_test_handler();
        let checks: Vec<BatchCheckItem> = (0..MAX_BATCH_SIZE)
            .map(|i| BatchCheckItem {
                user: format!("user:user{}", i),
                relation: "viewer".to_string(),
                object: format!("document:doc{}", i),
            })
            .collect();
        let request = BatchCheckRequest::new("store1", checks);

        // Act
        let result = handler.validate(&request);

        // Assert
        assert!(result.is_ok());
        assert_eq!(request.checks.len(), MAX_BATCH_SIZE);
    }

    // ============================================================
    // Section 2: Intra-Batch Deduplication
    // ============================================================

    #[test]
    fn test_identifies_duplicate_checks_in_batch() {
        // Arrange
        let handler = create_test_handler();
        let request = BatchCheckRequest::new(
            "store1",
            vec![
                BatchCheckItem {
                    user: "user:alice".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:doc1".to_string(),
                },
                BatchCheckItem {
                    user: "user:alice".to_string(), // Duplicate
                    relation: "viewer".to_string(),
                    object: "document:doc1".to_string(),
                },
                BatchCheckItem {
                    user: "user:bob".to_string(), // Different user
                    relation: "viewer".to_string(),
                    object: "document:doc1".to_string(),
                },
            ],
        );

        // Act
        let (total, unique) = handler.dedup_stats(&request);

        // Assert
        assert_eq!(total, 3);
        assert_eq!(unique, 2); // Only 2 unique checks
    }

    #[tokio::test]
    async fn test_executes_unique_checks_only_once() {
        // Arrange - create handler with tracking
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CountingTupleReader {
            call_count: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl TupleReader for CountingTupleReader {
            async fn read_tuples(
                &self,
                _store_id: &str,
                _object_type: &str,
                _object_id: &str,
                _relation: &str,
            ) -> DomainResult<Vec<StoredTupleRef>> {
                self.call_count.fetch_add(1, Ordering::SeqCst);
                // Return a tuple so check returns true
                Ok(vec![StoredTupleRef {
                    user_type: "user".to_string(),
                    user_id: "alice".to_string(),
                    user_relation: None,
                }])
            }

            async fn store_exists(&self, _store_id: &str) -> DomainResult<bool> {
                Ok(true)
            }
        }

        let call_count = Arc::new(AtomicUsize::new(0));
        let tuple_reader = CountingTupleReader {
            call_count: call_count.clone(),
        };
        let model_reader = MockModelReader::new().with_type(TypeDefinition {
            type_name: "document".to_string(),
            relations: vec![RelationDefinition {
                name: "viewer".to_string(),
                type_constraints: vec!["user".to_string()],
                rewrite: Userset::This,
            }],
        });

        let resolver = Arc::new(GraphResolver::new(
            Arc::new(tuple_reader),
            Arc::new(model_reader),
        ));
        let cache = Arc::new(CheckCache::new(CheckCacheConfig::default()));
        let handler = BatchCheckHandler::new(resolver, cache);

        // Create batch with 5 identical checks
        let request = BatchCheckRequest::new(
            "store1",
            vec![
                BatchCheckItem {
                    user: "user:alice".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:doc1".to_string(),
                };
                5
            ],
        );

        // Act
        let response = handler.check(request).await.unwrap();

        // Assert - should only execute once despite 5 identical checks
        assert_eq!(response.results.len(), 5);
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_maps_results_back_to_original_positions() {
        // Arrange - create a handler where different checks return different results
        struct PositionAwareTupleReader;

        #[async_trait]
        impl TupleReader for PositionAwareTupleReader {
            async fn read_tuples(
                &self,
                _store_id: &str,
                _object_type: &str,
                object_id: &str,
                _relation: &str,
            ) -> DomainResult<Vec<StoredTupleRef>> {
                // doc1 returns true (has tuple), doc2 returns false (no tuple)
                if object_id == "doc1" {
                    Ok(vec![StoredTupleRef {
                        user_type: "user".to_string(),
                        user_id: "alice".to_string(),
                        user_relation: None,
                    }])
                } else {
                    Ok(vec![])
                }
            }

            async fn store_exists(&self, _store_id: &str) -> DomainResult<bool> {
                Ok(true)
            }
        }

        let tuple_reader = PositionAwareTupleReader;
        let model_reader = MockModelReader::new().with_type(TypeDefinition {
            type_name: "document".to_string(),
            relations: vec![RelationDefinition {
                name: "viewer".to_string(),
                type_constraints: vec!["user".to_string()],
                rewrite: Userset::This,
            }],
        });

        let resolver = Arc::new(GraphResolver::new(
            Arc::new(tuple_reader),
            Arc::new(model_reader),
        ));
        let cache = Arc::new(CheckCache::new(CheckCacheConfig::default()));
        let handler = BatchCheckHandler::new(resolver, cache);

        // Create batch: doc1 (true), doc2 (false), doc1 (true - duplicate)
        let request = BatchCheckRequest::new(
            "store1",
            vec![
                BatchCheckItem {
                    user: "user:alice".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:doc1".to_string(),
                },
                BatchCheckItem {
                    user: "user:alice".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:doc2".to_string(),
                },
                BatchCheckItem {
                    user: "user:alice".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:doc1".to_string(), // Duplicate of first
                },
            ],
        );

        // Act
        let response = handler.check(request).await.unwrap();

        // Assert - results should be in original order
        assert_eq!(response.results.len(), 3);
        assert!(response.results[0].allowed); // doc1 = true
        assert!(!response.results[1].allowed); // doc2 = false
        assert!(response.results[2].allowed); // doc1 = true (same as first)
    }

    #[tokio::test]
    async fn test_preserves_request_order_in_response() {
        // Arrange
        let handler = create_test_handler();

        // Create batch with specific order
        let request = BatchCheckRequest::new(
            "store1",
            vec![
                BatchCheckItem {
                    user: "user:alice".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:doc1".to_string(),
                },
                BatchCheckItem {
                    user: "user:bob".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:doc2".to_string(),
                },
                BatchCheckItem {
                    user: "user:charlie".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:doc3".to_string(),
                },
            ],
        );

        // Act
        let response = handler.check(request).await.unwrap();

        // Assert - response should have same number of results in order
        assert_eq!(response.results.len(), 3);
        // Each result corresponds to its position (no reordering)
    }

    // ============================================================
    // Section 3: Singleflight (Cross-Request Deduplication)
    // ============================================================

    #[tokio::test]
    async fn test_concurrent_requests_for_same_check_share_result() {
        // Arrange - create handler with slow tuple reader to ensure overlap
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        struct SlowTupleReader {
            call_count: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl TupleReader for SlowTupleReader {
            async fn read_tuples(
                &self,
                _store_id: &str,
                _object_type: &str,
                _object_id: &str,
                _relation: &str,
            ) -> DomainResult<Vec<StoredTupleRef>> {
                self.call_count.fetch_add(1, Ordering::SeqCst);
                // Add delay to ensure concurrent requests overlap
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(vec![StoredTupleRef {
                    user_type: "user".to_string(),
                    user_id: "alice".to_string(),
                    user_relation: None,
                }])
            }

            async fn store_exists(&self, _store_id: &str) -> DomainResult<bool> {
                Ok(true)
            }
        }

        let call_count = Arc::new(AtomicUsize::new(0));
        let tuple_reader = SlowTupleReader {
            call_count: call_count.clone(),
        };
        let model_reader = MockModelReader::new().with_type(TypeDefinition {
            type_name: "document".to_string(),
            relations: vec![RelationDefinition {
                name: "viewer".to_string(),
                type_constraints: vec!["user".to_string()],
                rewrite: Userset::This,
            }],
        });

        let resolver = Arc::new(GraphResolver::new(
            Arc::new(tuple_reader),
            Arc::new(model_reader),
        ));
        let cache = Arc::new(CheckCache::new(CheckCacheConfig::default()));
        let handler = Arc::new(BatchCheckHandler::new(resolver, cache));

        // Create identical requests
        let make_request = || {
            BatchCheckRequest::new(
                "store1",
                vec![BatchCheckItem {
                    user: "user:alice".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:doc1".to_string(),
                }],
            )
        };

        // Act - launch 5 concurrent requests for the same check
        let handler_clone = handler.clone();
        let handles: Vec<_> = (0..5)
            .map(|_| {
                let h = handler_clone.clone();
                let req = make_request();
                tokio::spawn(async move { h.check(req).await })
            })
            .collect();

        // Wait for all to complete
        let results: Vec<Result<BatchCheckResult<BatchCheckResponse>, _>> =
            futures::future::join_all(handles).await;

        // Assert - all requests should succeed with same result
        for result in &results {
            let response = result.as_ref().unwrap().as_ref().unwrap();
            assert_eq!(response.results.len(), 1);
            assert!(response.results[0].allowed);
        }

        // The key assertion: with singleflight, only 1 actual check should execute
        // Without singleflight, 5 checks would execute
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "Singleflight should deduplicate concurrent requests"
        );
    }

    #[tokio::test]
    async fn test_singleflight_groups_expire_after_completion() {
        // Arrange
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CountingTupleReader {
            call_count: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl TupleReader for CountingTupleReader {
            async fn read_tuples(
                &self,
                _store_id: &str,
                _object_type: &str,
                _object_id: &str,
                _relation: &str,
            ) -> DomainResult<Vec<StoredTupleRef>> {
                self.call_count.fetch_add(1, Ordering::SeqCst);
                Ok(vec![StoredTupleRef {
                    user_type: "user".to_string(),
                    user_id: "alice".to_string(),
                    user_relation: None,
                }])
            }

            async fn store_exists(&self, _store_id: &str) -> DomainResult<bool> {
                Ok(true)
            }
        }

        let call_count = Arc::new(AtomicUsize::new(0));
        let tuple_reader = CountingTupleReader {
            call_count: call_count.clone(),
        };
        let model_reader = MockModelReader::new().with_type(TypeDefinition {
            type_name: "document".to_string(),
            relations: vec![RelationDefinition {
                name: "viewer".to_string(),
                type_constraints: vec!["user".to_string()],
                rewrite: Userset::This,
            }],
        });

        let resolver = Arc::new(GraphResolver::new(
            Arc::new(tuple_reader),
            Arc::new(model_reader),
        ));
        let cache = Arc::new(CheckCache::new(CheckCacheConfig {
            max_capacity: 0, // Disable cache to test singleflight isolation
            ..Default::default()
        }));
        let handler = BatchCheckHandler::new(resolver, cache);

        let make_request = || {
            BatchCheckRequest::new(
                "store1",
                vec![BatchCheckItem {
                    user: "user:alice".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:doc1".to_string(),
                }],
            )
        };

        // Act - make two sequential requests (not concurrent)
        let _ = handler.check(make_request()).await.unwrap();
        let _ = handler.check(make_request()).await.unwrap();

        // Assert - each sequential request should execute separately
        // (singleflight only deduplicates concurrent requests, not sequential)
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            2,
            "Sequential requests should not share singleflight group"
        );
    }

    #[tokio::test]
    async fn test_errors_dont_poison_singleflight_group() {
        // Arrange - tuple reader that fails first, succeeds second
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct FailOnceTupleReader {
            call_count: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl TupleReader for FailOnceTupleReader {
            async fn read_tuples(
                &self,
                _store_id: &str,
                _object_type: &str,
                _object_id: &str,
                _relation: &str,
            ) -> DomainResult<Vec<StoredTupleRef>> {
                let count = self.call_count.fetch_add(1, Ordering::SeqCst);
                if count == 0 {
                    Err(DomainError::ResolverError {
                        message: "transient error".to_string(),
                    })
                } else {
                    Ok(vec![StoredTupleRef {
                        user_type: "user".to_string(),
                        user_id: "alice".to_string(),
                        user_relation: None,
                    }])
                }
            }

            async fn store_exists(&self, _store_id: &str) -> DomainResult<bool> {
                Ok(true)
            }
        }

        let call_count = Arc::new(AtomicUsize::new(0));
        let tuple_reader = FailOnceTupleReader {
            call_count: call_count.clone(),
        };
        let model_reader = MockModelReader::new().with_type(TypeDefinition {
            type_name: "document".to_string(),
            relations: vec![RelationDefinition {
                name: "viewer".to_string(),
                type_constraints: vec!["user".to_string()],
                rewrite: Userset::This,
            }],
        });

        let resolver = Arc::new(GraphResolver::new(
            Arc::new(tuple_reader),
            Arc::new(model_reader),
        ));
        let cache = Arc::new(CheckCache::new(CheckCacheConfig {
            max_capacity: 0, // Disable cache
            ..Default::default()
        }));
        let handler = BatchCheckHandler::new(resolver, cache);

        let make_request = || {
            BatchCheckRequest::new(
                "store1",
                vec![BatchCheckItem {
                    user: "user:alice".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:doc1".to_string(),
                }],
            )
        };

        // Act - first request fails, second should succeed
        let result1 = handler.check(make_request()).await.unwrap();
        let result2 = handler.check(make_request()).await.unwrap();

        // Assert - first has error, second succeeds
        assert!(result1.results[0].error.is_some());
        assert!(result2.results[0].allowed);
        assert!(result2.results[0].error.is_none());
    }

    #[tokio::test]
    async fn test_singleflight_handles_timeouts_correctly() {
        // Arrange - very slow tuple reader
        use std::time::Duration;

        struct VerySlowTupleReader;

        #[async_trait]
        impl TupleReader for VerySlowTupleReader {
            async fn read_tuples(
                &self,
                _store_id: &str,
                _object_type: &str,
                _object_id: &str,
                _relation: &str,
            ) -> DomainResult<Vec<StoredTupleRef>> {
                // Sleep longer than the timeout
                tokio::time::sleep(Duration::from_secs(10)).await;
                Ok(vec![])
            }

            async fn store_exists(&self, _store_id: &str) -> DomainResult<bool> {
                Ok(true)
            }
        }

        let tuple_reader = VerySlowTupleReader;
        let model_reader = MockModelReader::new().with_type(TypeDefinition {
            type_name: "document".to_string(),
            relations: vec![RelationDefinition {
                name: "viewer".to_string(),
                type_constraints: vec!["user".to_string()],
                rewrite: Userset::This,
            }],
        });

        let resolver = Arc::new(GraphResolver::new(
            Arc::new(tuple_reader),
            Arc::new(model_reader),
        ));
        let cache = Arc::new(CheckCache::new(CheckCacheConfig::default()));
        let handler = BatchCheckHandler::new(resolver, cache);

        let request = BatchCheckRequest::new(
            "store1",
            vec![BatchCheckItem {
                user: "user:alice".to_string(),
                relation: "viewer".to_string(),
                object: "document:doc1".to_string(),
            }],
        );

        // Act - request with timeout
        let result = tokio::time::timeout(Duration::from_millis(100), handler.check(request)).await;

        // Assert - should timeout (not hang forever)
        assert!(result.is_err(), "Request should timeout");
    }

    // ============================================================
    // Section 4: Parallel Execution
    // ============================================================

    #[tokio::test]
    async fn test_unique_checks_execute_in_parallel() {
        // Arrange - track start times to verify parallelism
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::{Duration, Instant};

        struct TimingTupleReader {
            concurrent_count: Arc<AtomicUsize>,
            max_concurrent: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl TupleReader for TimingTupleReader {
            async fn read_tuples(
                &self,
                _store_id: &str,
                _object_type: &str,
                _object_id: &str,
                _relation: &str,
            ) -> DomainResult<Vec<StoredTupleRef>> {
                // Track concurrency
                let current = self.concurrent_count.fetch_add(1, Ordering::SeqCst) + 1;
                self.max_concurrent.fetch_max(current, Ordering::SeqCst);

                // Simulate I/O delay
                tokio::time::sleep(Duration::from_millis(50)).await;

                self.concurrent_count.fetch_sub(1, Ordering::SeqCst);

                Ok(vec![StoredTupleRef {
                    user_type: "user".to_string(),
                    user_id: "alice".to_string(),
                    user_relation: None,
                }])
            }

            async fn store_exists(&self, _store_id: &str) -> DomainResult<bool> {
                Ok(true)
            }
        }

        let concurrent_count = Arc::new(AtomicUsize::new(0));
        let max_concurrent = Arc::new(AtomicUsize::new(0));
        let tuple_reader = TimingTupleReader {
            concurrent_count: concurrent_count.clone(),
            max_concurrent: max_concurrent.clone(),
        };
        let model_reader = MockModelReader::new().with_type(TypeDefinition {
            type_name: "document".to_string(),
            relations: vec![RelationDefinition {
                name: "viewer".to_string(),
                type_constraints: vec!["user".to_string()],
                rewrite: Userset::This,
            }],
        });

        let resolver = Arc::new(GraphResolver::new(
            Arc::new(tuple_reader),
            Arc::new(model_reader),
        ));
        let cache = Arc::new(CheckCache::new(CheckCacheConfig::default()));
        let handler = BatchCheckHandler::new(resolver, cache);

        // Create batch with 5 unique checks
        let request = BatchCheckRequest::new(
            "store1",
            vec![
                BatchCheckItem {
                    user: "user:alice".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:doc1".to_string(),
                },
                BatchCheckItem {
                    user: "user:bob".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:doc2".to_string(),
                },
                BatchCheckItem {
                    user: "user:charlie".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:doc3".to_string(),
                },
                BatchCheckItem {
                    user: "user:dave".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:doc4".to_string(),
                },
                BatchCheckItem {
                    user: "user:eve".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:doc5".to_string(),
                },
            ],
        );

        // Act
        let start = Instant::now();
        let response = handler.check(request).await.unwrap();
        let elapsed = start.elapsed();

        // Assert
        assert_eq!(response.results.len(), 5);

        // If parallel, max concurrent should be > 1
        // If sequential, max concurrent would be exactly 1
        assert!(
            max_concurrent.load(Ordering::SeqCst) > 1,
            "Checks should execute in parallel, max concurrent was {}",
            max_concurrent.load(Ordering::SeqCst)
        );

        // If parallel, total time should be ~50ms (not 5*50=250ms)
        assert!(
            elapsed < Duration::from_millis(200),
            "Parallel execution should be faster than sequential, took {:?}",
            elapsed
        );
    }

    #[tokio::test]
    async fn test_batch_processes_faster_than_sequential() {
        // Arrange
        use std::time::{Duration, Instant};

        struct DelayedTupleReader;

        #[async_trait]
        impl TupleReader for DelayedTupleReader {
            async fn read_tuples(
                &self,
                _store_id: &str,
                _object_type: &str,
                _object_id: &str,
                _relation: &str,
            ) -> DomainResult<Vec<StoredTupleRef>> {
                tokio::time::sleep(Duration::from_millis(20)).await;
                Ok(vec![StoredTupleRef {
                    user_type: "user".to_string(),
                    user_id: "alice".to_string(),
                    user_relation: None,
                }])
            }

            async fn store_exists(&self, _store_id: &str) -> DomainResult<bool> {
                Ok(true)
            }
        }

        let tuple_reader = DelayedTupleReader;
        let model_reader = MockModelReader::new().with_type(TypeDefinition {
            type_name: "document".to_string(),
            relations: vec![RelationDefinition {
                name: "viewer".to_string(),
                type_constraints: vec!["user".to_string()],
                rewrite: Userset::This,
            }],
        });

        let resolver = Arc::new(GraphResolver::new(
            Arc::new(tuple_reader),
            Arc::new(model_reader),
        ));
        let cache = Arc::new(CheckCache::new(CheckCacheConfig::default()));
        let handler = BatchCheckHandler::new(resolver, cache);

        // Create batch with 10 unique checks
        let checks: Vec<BatchCheckItem> = (0..10)
            .map(|i| BatchCheckItem {
                user: format!("user:user{}", i),
                relation: "viewer".to_string(),
                object: format!("document:doc{}", i),
            })
            .collect();
        let request = BatchCheckRequest::new("store1", checks);

        // Act
        let start = Instant::now();
        let response = handler.check(request).await.unwrap();
        let elapsed = start.elapsed();

        // Assert
        assert_eq!(response.results.len(), 10);

        // Sequential would take 10 * 20ms = 200ms
        // Parallel should take ~20ms (plus overhead)
        // Use 150ms as threshold to account for some overhead
        assert!(
            elapsed < Duration::from_millis(150),
            "Batch should be faster than sequential (10*20ms=200ms), took {:?}",
            elapsed
        );
    }

    #[tokio::test]
    async fn test_parallel_execution_uses_all_available_concurrency() {
        // This test verifies that the handler executes checks in parallel.
        // Note: Explicit concurrency limits (e.g., buffer_unordered) are not yet
        // implemented. When added, this test should verify the actual limit.
        // For now, we verify parallelism is happening (max_concurrent > 1).
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        struct ConcurrencyTrackingReader {
            concurrent_count: Arc<AtomicUsize>,
            max_concurrent: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl TupleReader for ConcurrencyTrackingReader {
            async fn read_tuples(
                &self,
                _store_id: &str,
                _object_type: &str,
                _object_id: &str,
                _relation: &str,
            ) -> DomainResult<Vec<StoredTupleRef>> {
                let current = self.concurrent_count.fetch_add(1, Ordering::SeqCst) + 1;
                self.max_concurrent.fetch_max(current, Ordering::SeqCst);

                tokio::time::sleep(Duration::from_millis(10)).await;

                self.concurrent_count.fetch_sub(1, Ordering::SeqCst);
                Ok(vec![])
            }

            async fn store_exists(&self, _store_id: &str) -> DomainResult<bool> {
                Ok(true)
            }
        }

        let concurrent_count = Arc::new(AtomicUsize::new(0));
        let max_concurrent = Arc::new(AtomicUsize::new(0));
        let tuple_reader = ConcurrencyTrackingReader {
            concurrent_count,
            max_concurrent: max_concurrent.clone(),
        };
        let model_reader = MockModelReader::new().with_type(TypeDefinition {
            type_name: "document".to_string(),
            relations: vec![RelationDefinition {
                name: "viewer".to_string(),
                type_constraints: vec!["user".to_string()],
                rewrite: Userset::This,
            }],
        });

        let resolver = Arc::new(GraphResolver::new(
            Arc::new(tuple_reader),
            Arc::new(model_reader),
        ));
        let cache = Arc::new(CheckCache::new(CheckCacheConfig::default()));
        let handler = BatchCheckHandler::new(resolver, cache);

        // Create batch with MAX_BATCH_SIZE unique checks
        let checks: Vec<BatchCheckItem> = (0..MAX_BATCH_SIZE)
            .map(|i| BatchCheckItem {
                user: format!("user:user{}", i),
                relation: "viewer".to_string(),
                object: format!("document:doc{}", i),
            })
            .collect();
        let request = BatchCheckRequest::new("store1", checks);

        // Act
        let _ = handler.check(request).await.unwrap();

        // Assert - verify parallel execution is happening
        let max = max_concurrent.load(Ordering::SeqCst);
        assert!(
            max > 1,
            "Checks should execute in parallel, max concurrent was {}",
            max
        );
        // TODO: When explicit concurrency limits are added (e.g., MAX_CONCURRENT = 32),
        // add assertion: assert!(max <= MAX_CONCURRENT, "Should respect limit");
    }

    #[tokio::test]
    async fn test_handles_partial_failures_gracefully() {
        // Arrange - some checks succeed, some fail
        struct PartialFailureTupleReader;

        #[async_trait]
        impl TupleReader for PartialFailureTupleReader {
            async fn read_tuples(
                &self,
                _store_id: &str,
                _object_type: &str,
                object_id: &str,
                _relation: &str,
            ) -> DomainResult<Vec<StoredTupleRef>> {
                // Fail for odd document numbers
                if object_id.ends_with('1') || object_id.ends_with('3') {
                    Err(DomainError::ResolverError {
                        message: format!("failed for {}", object_id),
                    })
                } else {
                    Ok(vec![StoredTupleRef {
                        user_type: "user".to_string(),
                        user_id: "alice".to_string(),
                        user_relation: None,
                    }])
                }
            }

            async fn store_exists(&self, _store_id: &str) -> DomainResult<bool> {
                Ok(true)
            }
        }

        let tuple_reader = PartialFailureTupleReader;
        let model_reader = MockModelReader::new().with_type(TypeDefinition {
            type_name: "document".to_string(),
            relations: vec![RelationDefinition {
                name: "viewer".to_string(),
                type_constraints: vec!["user".to_string()],
                rewrite: Userset::This,
            }],
        });

        let resolver = Arc::new(GraphResolver::new(
            Arc::new(tuple_reader),
            Arc::new(model_reader),
        ));
        let cache = Arc::new(CheckCache::new(CheckCacheConfig::default()));
        let handler = BatchCheckHandler::new(resolver, cache);

        // Create batch with mix of succeeding and failing checks
        let request = BatchCheckRequest::new(
            "store1",
            vec![
                BatchCheckItem {
                    user: "user:alice".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:doc0".to_string(), // Success
                },
                BatchCheckItem {
                    user: "user:alice".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:doc1".to_string(), // Fail
                },
                BatchCheckItem {
                    user: "user:alice".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:doc2".to_string(), // Success
                },
                BatchCheckItem {
                    user: "user:alice".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:doc3".to_string(), // Fail
                },
            ],
        );

        // Act
        let response = handler.check(request).await;

        // Assert - batch should complete (not fail entirely)
        assert!(
            response.is_ok(),
            "Batch should complete despite partial failures"
        );
        let response = response.unwrap();
        assert_eq!(response.results.len(), 4);

        // Check individual results
        assert!(response.results[0].allowed);
        assert!(response.results[0].error.is_none());

        assert!(!response.results[1].allowed);
        assert!(response.results[1].error.is_some());

        assert!(response.results[2].allowed);
        assert!(response.results[2].error.is_none());

        assert!(!response.results[3].allowed);
        assert!(response.results[3].error.is_some());
    }

    // ============================================================
    // Section 5: Performance
    // ============================================================

    #[tokio::test]
    async fn test_batch_of_max_identical_checks_executes_only_once() {
        // Arrange
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CountingTupleReader {
            call_count: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl TupleReader for CountingTupleReader {
            async fn read_tuples(
                &self,
                _store_id: &str,
                _object_type: &str,
                _object_id: &str,
                _relation: &str,
            ) -> DomainResult<Vec<StoredTupleRef>> {
                self.call_count.fetch_add(1, Ordering::SeqCst);
                Ok(vec![StoredTupleRef {
                    user_type: "user".to_string(),
                    user_id: "alice".to_string(),
                    user_relation: None,
                }])
            }

            async fn store_exists(&self, _store_id: &str) -> DomainResult<bool> {
                Ok(true)
            }
        }

        let call_count = Arc::new(AtomicUsize::new(0));
        let tuple_reader = CountingTupleReader {
            call_count: call_count.clone(),
        };
        let model_reader = MockModelReader::new().with_type(TypeDefinition {
            type_name: "document".to_string(),
            relations: vec![RelationDefinition {
                name: "viewer".to_string(),
                type_constraints: vec!["user".to_string()],
                rewrite: Userset::This,
            }],
        });

        let resolver = Arc::new(GraphResolver::new(
            Arc::new(tuple_reader),
            Arc::new(model_reader),
        ));
        let cache = Arc::new(CheckCache::new(CheckCacheConfig::default()));
        let handler = BatchCheckHandler::new(resolver, cache);

        // Create batch with MAX_BATCH_SIZE identical checks
        let checks: Vec<BatchCheckItem> = (0..MAX_BATCH_SIZE)
            .map(|_| BatchCheckItem {
                user: "user:alice".to_string(),
                relation: "viewer".to_string(),
                object: "document:doc1".to_string(),
            })
            .collect();
        let request = BatchCheckRequest::new("store1", checks);

        // Act
        let response = handler.check(request).await.unwrap();

        // Assert
        assert_eq!(response.results.len(), MAX_BATCH_SIZE);
        assert!(response.results.iter().all(|r| r.allowed));

        // With intra-batch deduplication, only 1 unique check should execute
        let actual_calls = call_count.load(Ordering::SeqCst);
        assert_eq!(
            actual_calls, 1,
            "{} identical checks should execute only 1 check, got {}",
            MAX_BATCH_SIZE, actual_calls
        );
    }

    #[tokio::test]
    async fn test_batch_throughput_target() {
        // Arrange - fast mock to measure overhead
        use std::time::Instant;

        struct FastTupleReader;

        #[async_trait]
        impl TupleReader for FastTupleReader {
            async fn read_tuples(
                &self,
                _store_id: &str,
                _object_type: &str,
                _object_id: &str,
                _relation: &str,
            ) -> DomainResult<Vec<StoredTupleRef>> {
                // No artificial delay - test framework overhead only
                Ok(vec![StoredTupleRef {
                    user_type: "user".to_string(),
                    user_id: "alice".to_string(),
                    user_relation: None,
                }])
            }

            async fn store_exists(&self, _store_id: &str) -> DomainResult<bool> {
                Ok(true)
            }
        }

        let tuple_reader = FastTupleReader;
        let model_reader = MockModelReader::new().with_type(TypeDefinition {
            type_name: "document".to_string(),
            relations: vec![RelationDefinition {
                name: "viewer".to_string(),
                type_constraints: vec!["user".to_string()],
                rewrite: Userset::This,
            }],
        });

        let resolver = Arc::new(GraphResolver::new(
            Arc::new(tuple_reader),
            Arc::new(model_reader),
        ));
        let cache = Arc::new(CheckCache::new(CheckCacheConfig::default()));
        let handler = BatchCheckHandler::new(resolver, cache);

        // Create batch with MAX_BATCH_SIZE checks (mix of duplicates)
        // OpenFGA limits batches to 50 items, so we test at that limit
        let checks: Vec<BatchCheckItem> = (0..MAX_BATCH_SIZE)
            .map(|i| BatchCheckItem {
                user: format!("user:user{}", i % 10), // 10 unique users
                relation: "viewer".to_string(),
                object: format!("document:doc{}", i % 25), // 25 unique docs
            })
            .collect();
        let request = BatchCheckRequest::new("store1", checks);

        // Act - measure throughput
        let start = Instant::now();
        let response = handler.check(request).await.unwrap();
        let elapsed = start.elapsed();

        // Assert
        assert_eq!(response.results.len(), MAX_BATCH_SIZE);

        let checks_per_second = MAX_BATCH_SIZE as f64 / elapsed.as_secs_f64();

        // Target: >500 checks/s
        // Note: This is a rough test; actual performance depends on hardware
        // For CI, we use a lower threshold to avoid flaky tests
        // In production benchmarks (M1.8), we'll validate the actual target
        assert!(
            checks_per_second > 100.0, // Conservative threshold for CI
            "Batch throughput should be reasonable, got {:.0} checks/s",
            checks_per_second
        );

        // Log actual throughput for visibility (won't cause test failure)
        println!(
            "Batch throughput: {:.0} checks/s ({} checks in {:?})",
            checks_per_second, MAX_BATCH_SIZE, elapsed
        );
    }

    #[test]
    fn test_memory_usage_scales_with_unique_checks() {
        // This test verifies that the deduplication data structures
        // scale with unique checks, not total checks

        let handler = create_test_handler();

        // Create request with many duplicates
        // Use different modulos to get cross-product of (user, object) pairs
        let checks_with_duplicates: Vec<BatchCheckItem> = (0..1000)
            .map(|i| BatchCheckItem {
                user: format!("user:user{}", i % 10), // 10 unique users
                relation: "viewer".to_string(),
                object: format!("document:doc{}", (i / 10) % 10), // 10 unique docs
            })
            .collect();

        let request = BatchCheckRequest::new("store1", checks_with_duplicates);

        // Check dedup stats
        let (total, unique) = handler.dedup_stats(&request);

        // Assert
        assert_eq!(total, 1000);
        assert_eq!(unique, 100); // 10 users * 10 docs = 100 unique combinations

        // The key insight: internal data structures should store ~100 entries,
        // not 1000, because of deduplication. This is verified by the dedup_stats
        // method returning 100 unique checks.

        // Memory usage is proportional to unique checks (100), not total (1000)
        // This is a design validation test, not a runtime memory measurement
    }
}
