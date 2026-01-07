//! Batch check handler implementation.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use futures::future::join_all;
use rsfga_domain::cache::CheckCache;
use rsfga_domain::resolver::{CheckRequest, GraphResolver, ModelReader, TupleReader};

use super::singleflight::{Singleflight, SingleflightGuard, SingleflightResult, SingleflightSlot};
use super::types::{
    BatchCheckError, BatchCheckItem, BatchCheckItemResult, BatchCheckRequest, BatchCheckResponse,
    BatchCheckResult, MAX_BATCH_SIZE,
};

/// Key for identifying unique checks (used for deduplication).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CheckKey {
    pub store_id: String,
    pub user: String,
    pub relation: String,
    pub object: String,
}

impl CheckKey {
    pub fn new(store_id: &str, item: &BatchCheckItem) -> Self {
        Self {
            store_id: store_id.to_string(),
            user: item.user.clone(),
            relation: item.relation.clone(),
            object: item.object.clone(),
        }
    }
}

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
    singleflight: Arc<Singleflight<CheckKey>>,
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
        let mut seen: HashSet<CheckKey> = HashSet::with_capacity(request.checks.len());
        for check in &request.checks {
            let key = CheckKey::new(&request.store_id, check);
            seen.insert(key);
        }
        (request.checks.len(), seen.len())
    }
}
