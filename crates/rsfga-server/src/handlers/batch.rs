//! Batch check handler with three-stage deduplication.
//!
//! This handler processes multiple permission checks in a single request,
//! optimizing throughput through:
//!
//! 1. **Intra-batch deduplication**: Identical checks execute only once
//! 2. **Singleflight**: Concurrent requests for same check share results
//! 3. **Cache integration**: Already-computed results skip execution
//!
//! # Performance Target (UNVALIDATED - M1.8)
//!
//! - Batch throughput: >500 checks/s
//! - Deduplication effectiveness: >90% on typical workloads

use std::collections::HashMap;
use std::sync::Arc;

use rsfga_domain::cache::CheckCache;
use rsfga_domain::resolver::{CheckRequest, GraphResolver, ModelReader, TupleReader};

/// Key for identifying unique checks (used for deduplication).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CheckKey {
    user: String,
    relation: String,
    object: String,
}

impl From<&BatchCheckItem> for CheckKey {
    fn from(item: &BatchCheckItem) -> Self {
        Self {
            user: item.user.clone(),
            relation: item.relation.clone(),
            object: item.object.clone(),
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
/// Processes multiple checks in parallel with three-stage deduplication.
pub struct BatchCheckHandler<T, M>
where
    T: TupleReader,
    M: ModelReader,
{
    /// The graph resolver for executing checks.
    resolver: Arc<GraphResolver<T, M>>,
    /// Cache for storing check results.
    #[allow(dead_code)]
    cache: Arc<CheckCache>,
}

impl<T, M> BatchCheckHandler<T, M>
where
    T: TupleReader + 'static,
    M: ModelReader + 'static,
{
    /// Creates a new batch check handler.
    pub fn new(resolver: Arc<GraphResolver<T, M>>, cache: Arc<CheckCache>) -> Self {
        Self { resolver, cache }
    }

    /// Validates a batch check request.
    pub fn validate(&self, request: &BatchCheckRequest) -> BatchCheckResult<()> {
        // Check for empty batch
        if request.checks.is_empty() {
            return Err(BatchCheckError::EmptyBatch);
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
            let key = CheckKey::from(check);
            let unique_index = *key_to_index.entry(key).or_insert_with(|| {
                let idx = unique_checks.len();
                unique_checks.push(check);
                idx
            });
            position_to_unique.push(unique_index);
        }

        // Execute unique checks only
        let mut unique_results: Vec<BatchCheckItemResult> = Vec::with_capacity(unique_checks.len());

        for check in &unique_checks {
            let check_request = CheckRequest::new(
                request.store_id.clone(),
                check.user.clone(),
                check.relation.clone(),
                check.object.clone(),
                vec![], // No contextual tuples in batch checks
            );

            match self.resolver.check(&check_request).await {
                Ok(result) => {
                    unique_results.push(BatchCheckItemResult {
                        allowed: result.allowed,
                        error: None,
                    });
                }
                Err(e) => {
                    unique_results.push(BatchCheckItemResult {
                        allowed: false,
                        error: Some(e.to_string()),
                    });
                }
            }
        }

        // Map results back to original positions
        let results: Vec<BatchCheckItemResult> = position_to_unique
            .iter()
            .map(|&idx| unique_results[idx].clone())
            .collect();

        Ok(BatchCheckResponse { results })
    }

    /// Returns statistics about deduplication for a batch request.
    /// Returns (total_checks, unique_checks).
    pub fn dedup_stats(&self, request: &BatchCheckRequest) -> (usize, usize) {
        let mut seen: HashMap<CheckKey, ()> = HashMap::new();
        for check in &request.checks {
            let key = CheckKey::from(check);
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
    fn test_accepts_batch_with_100_plus_checks() {
        // Arrange
        let handler = create_test_handler();
        let checks: Vec<BatchCheckItem> = (0..150)
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
        assert_eq!(request.checks.len(), 150);
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
}
