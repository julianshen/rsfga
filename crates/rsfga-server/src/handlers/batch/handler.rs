//! Batch check handler implementation.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use futures::stream::{self, StreamExt};
use rsfga_domain::cache::CheckCache;
use rsfga_domain::resolver::{CheckRequest, GraphResolver, ModelReader, TupleReader};

use super::singleflight::{Singleflight, SingleflightGuard, SingleflightResult, SingleflightSlot};
use super::types::{
    BatchCheckError, BatchCheckItem, BatchCheckItemResult, BatchCheckRequest, BatchCheckResponse,
    BatchCheckResult, MAX_BATCH_SIZE,
};

/// Maximum number of singleflight retries when a leader is dropped.
/// This prevents unbounded recursion in edge cases.
const MAX_SINGLEFLIGHT_RETRIES: u32 = 3;

/// Default maximum concurrent checks within a batch.
/// This prevents resource exhaustion when processing large batches.
pub const DEFAULT_BATCH_CONCURRENCY: usize = 10;

/// Key for identifying unique checks (used for deduplication).
///
/// CRITICAL: This includes context because different contexts can produce
/// different authorization results even for the same user/relation/object tuple.
/// For example, a time-based condition like `current_time < expiry` would return
/// different results depending on the context's current_time value.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CheckKey {
    store_id: String,
    user: String,
    relation: String,
    object: String,
    /// Serialized context for hash equality.
    /// Using a deterministic JSON string representation of the context HashMap.
    context_hash: String,
}

impl CheckKey {
    pub fn new(store_id: &str, item: &BatchCheckItem) -> Self {
        // Serialize context to a deterministic string for hashing.
        // BTreeMap ensures consistent key ordering for deterministic comparison.
        let context_hash = if item.context.is_empty() {
            String::new()
        } else {
            // Sort keys for deterministic serialization
            let sorted: std::collections::BTreeMap<_, _> = item.context.iter().collect();
            serde_json::to_string(&sorted).unwrap_or_default()
        };

        Self {
            store_id: store_id.to_string(),
            user: item.user.clone(),
            relation: item.relation.clone(),
            object: item.object.clone(),
            context_hash,
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
///
/// Concurrency is bounded by `max_concurrency` to prevent resource exhaustion
/// when processing large batches.
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
    /// Maximum concurrent checks to execute in parallel.
    max_concurrency: usize,
}

impl<T, M> BatchCheckHandler<T, M>
where
    T: TupleReader + 'static,
    M: ModelReader + 'static,
{
    /// Creates a new batch check handler with default concurrency limit.
    pub fn new(resolver: Arc<GraphResolver<T, M>>, cache: Arc<CheckCache>) -> Self {
        Self::with_concurrency(resolver, cache, DEFAULT_BATCH_CONCURRENCY)
    }

    /// Creates a new batch check handler with a custom concurrency limit.
    ///
    /// # Arguments
    ///
    /// * `resolver` - The graph resolver for executing checks
    /// * `cache` - Cache for storing check results
    /// * `max_concurrency` - Maximum concurrent checks to execute in parallel
    pub fn with_concurrency(
        resolver: Arc<GraphResolver<T, M>>,
        cache: Arc<CheckCache>,
        max_concurrency: usize,
    ) -> Self {
        Self {
            resolver,
            cache,
            singleflight: Arc::new(Singleflight::new()),
            max_concurrency: max_concurrency.max(1), // Ensure at least 1
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
        // Build a map of unique checks with their keys, avoiding key recreation
        let mut unique_entries: Vec<(&BatchCheckItem, CheckKey)> = Vec::new();
        let mut key_to_index: HashMap<CheckKey, usize> = HashMap::new();
        let mut position_to_unique: Vec<usize> = Vec::with_capacity(request.checks.len());

        for check in &request.checks {
            let key = CheckKey::new(&request.store_id, check);
            let unique_index = match key_to_index.get(&key) {
                Some(&idx) => idx,
                None => {
                    let idx = unique_entries.len();
                    key_to_index.insert(key.clone(), idx);
                    unique_entries.push((check, key));
                    idx
                }
            };
            position_to_unique.push(unique_index);
        }

        // Execute unique checks in parallel with singleflight (Stage 2: Cross-request dedup)
        // Use buffer_unordered to limit concurrent executions and prevent resource exhaustion
        // Clone data to avoid lifetime issues with async closures
        let check_data: Vec<_> = unique_entries
            .into_iter()
            .enumerate()
            .map(|(idx, (check, key))| (idx, check.clone(), key))
            .collect();

        let store_id = request.store_id.clone();
        let unique_results: Vec<BatchCheckItemResult> = {
            let mut unordered_results: Vec<_> = stream::iter(check_data)
                .map(|(idx, check, key)| {
                    let store_id = store_id.clone();
                    async move {
                        let result = self
                            .execute_check_with_singleflight_owned(&store_id, check, key, 0)
                            .await;
                        (idx, result)
                    }
                })
                .buffer_unordered(self.max_concurrency)
                .collect()
                .await;

            // Sort results back into their original order
            unordered_results.sort_by_key(|(idx, _)| *idx);

            unordered_results
                .into_iter()
                .map(|(_, result)| result)
                .collect()
        };

        // Map results back to original positions
        let results: Vec<BatchCheckItemResult> = position_to_unique
            .iter()
            .map(|&idx| unique_results[idx].clone())
            .collect();

        Ok(BatchCheckResponse { results })
    }

    /// Execute a single check with singleflight deduplication (owned version).
    ///
    /// This version takes owned `BatchCheckItem` to work with async closures in streams.
    async fn execute_check_with_singleflight_owned(
        &self,
        store_id: &str,
        check: BatchCheckItem,
        key: CheckKey,
        retry_count: u32,
    ) -> BatchCheckItemResult {
        self.execute_check_with_singleflight(store_id, &check, key, retry_count)
            .await
    }

    /// Execute a single check with singleflight deduplication.
    ///
    /// If there's already an in-flight request for this check, wait for its result.
    /// Otherwise, execute the check and broadcast the result to any waiters.
    ///
    /// Uses atomic acquire() to prevent race conditions and SingleflightGuard
    /// for cleanup on panic. Retries are limited by MAX_SINGLEFLIGHT_RETRIES.
    async fn execute_check_with_singleflight(
        &self,
        store_id: &str,
        check: &BatchCheckItem,
        key: CheckKey,
        retry_count: u32,
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
                        // Limit retries to prevent unbounded recursion
                        if retry_count >= MAX_SINGLEFLIGHT_RETRIES {
                            return BatchCheckItemResult {
                                allowed: false,
                                error: Some(format!(
                                    "singleflight retry limit exceeded ({MAX_SINGLEFLIGHT_RETRIES} retries)"
                                )),
                            };
                        }
                        // This is safe because SingleflightGuard cleaned up
                        Box::pin(self.execute_check_with_singleflight(
                            store_id,
                            check,
                            key,
                            retry_count + 1,
                        ))
                        .await
                    }
                }
            }
            SingleflightSlot::Leader(sender) => {
                // We're the leader - create guard for cleanup on panic
                let guard = SingleflightGuard::new(&self.singleflight, key);

                // Execute the actual check with context
                let check_request = CheckRequest::with_context(
                    store_id.to_string(),
                    check.user.clone(),
                    check.relation.clone(),
                    check.object.clone(),
                    vec![], // No contextual tuples in batch checks
                    check.context.clone(),
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
