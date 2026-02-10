//! Bootstrap sync for edge nodes.
//!
//! Provides initial full synchronization of tuples from a central (source)
//! storage to a local (target) storage. After bootstrap completes, the
//! edge consumer takes over with incremental NATS-based sync.
//!
//! # Architecture
//!
//! ```text
//! Central DB ──(paginated reads)──▶ BootstrapSyncer ──(bulk writes)──▶ Edge DB
//!                                        │
//!                                        └── metrics (tuples synced, duration)
//! ```
//!
//! # Large Dataset Handling
//!
//! Bootstrap uses cursor-based pagination to handle arbitrarily large stores
//! without loading all tuples into memory. Each page is written to the target
//! before reading the next page.

use std::sync::Arc;
use std::time::Instant;

use tracing::{debug, info, warn};

use rsfga_storage::{DataStore, PaginationOptions, TupleFilter};

use crate::error::{EdgeError, Result};
use crate::metrics::EdgeMetrics;

/// Default page size for paginated bootstrap reads.
#[allow(dead_code)] // Public API, used by CLI wiring and tests
pub const DEFAULT_BOOTSTRAP_PAGE_SIZE: u32 = 1000;

/// Configuration for bootstrap sync.
#[derive(Debug, Clone)]
pub struct BootstrapConfig {
    /// Number of tuples to read per page from the source store.
    pub page_size: u32,

    /// Store IDs to bootstrap. Must contain at least one store.
    pub store_ids: Vec<String>,
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            page_size: DEFAULT_BOOTSTRAP_PAGE_SIZE,
            store_ids: Vec::new(),
        }
    }
}

/// Result of bootstrapping a single store.
#[derive(Debug, Clone)]
pub struct BootstrapStoreResult {
    /// The store ID that was bootstrapped.
    pub store_id: String,

    /// Total number of tuples synced.
    pub tuples_synced: usize,

    /// Number of pages read from source.
    pub pages_read: usize,

    /// Total duration of the bootstrap for this store.
    pub duration: std::time::Duration,
}

/// Result of a full bootstrap operation across all configured stores.
#[derive(Debug, Clone)]
pub struct BootstrapResult {
    /// Results for each bootstrapped store.
    pub stores: Vec<BootstrapStoreResult>,

    /// Total duration of the entire bootstrap operation.
    pub duration: std::time::Duration,
}

impl BootstrapResult {
    /// Total tuples synced across all stores.
    pub fn total_tuples(&self) -> usize {
        self.stores.iter().map(|s| s.tuples_synced).sum()
    }
}

/// Bootstrap syncer that copies tuples from a source (central) to a target (edge) storage.
pub struct BootstrapSyncer {
    source: Arc<dyn DataStore>,
    target: Arc<dyn DataStore>,
    metrics: Arc<EdgeMetrics>,
    config: BootstrapConfig,
}

impl BootstrapSyncer {
    /// Create a new bootstrap syncer.
    pub fn new(
        source: Arc<dyn DataStore>,
        target: Arc<dyn DataStore>,
        metrics: Arc<EdgeMetrics>,
        config: BootstrapConfig,
    ) -> Self {
        Self {
            source,
            target,
            metrics,
            config,
        }
    }

    /// Bootstrap a single store by copying all tuples from source to target.
    ///
    /// Uses cursor-based pagination to handle large datasets without loading
    /// all tuples into memory. Each page is written to the target before
    /// reading the next page. Writes use upsert semantics for idempotency.
    pub async fn sync_store(&self, store_id: &str) -> Result<BootstrapStoreResult> {
        let start = Instant::now();
        let filter = TupleFilter::default();
        let mut continuation_token = None;
        let mut total_synced: usize = 0;
        let mut pages_read: usize = 0;

        info!(
            store_id,
            page_size = self.config.page_size,
            "Starting bootstrap sync"
        );

        loop {
            let pagination = PaginationOptions {
                page_size: Some(self.config.page_size),
                continuation_token: continuation_token.clone(),
            };

            let page = self
                .source
                .read_tuples_paginated(store_id, &filter, &pagination)
                .await
                .map_err(EdgeError::Storage)?;

            pages_read += 1;
            let page_count = page.items.len();

            if !page.items.is_empty() {
                self.target
                    .write_tuples(store_id, page.items, vec![])
                    .await
                    .map_err(EdgeError::Storage)?;

                total_synced += page_count;
                self.metrics.record_tuples_written(page_count);
            }

            debug!(
                store_id,
                page = pages_read,
                tuples = page_count,
                total = total_synced,
                "Bootstrap page synced"
            );

            match page.continuation_token {
                Some(token) => continuation_token = Some(token),
                None => break,
            }
        }

        let duration = start.elapsed();
        info!(
            store_id,
            tuples_synced = total_synced,
            pages_read,
            duration_ms = duration.as_millis() as u64,
            "Bootstrap sync complete for store"
        );

        Ok(BootstrapStoreResult {
            store_id: store_id.to_string(),
            tuples_synced: total_synced,
            pages_read,
            duration,
        })
    }

    /// Bootstrap all configured stores.
    ///
    /// Syncs each store sequentially. Returns results for all stores.
    /// If a store fails, the error is returned immediately (fail-fast).
    pub async fn sync_all(&self) -> Result<BootstrapResult> {
        let start = Instant::now();

        if self.config.store_ids.is_empty() {
            warn!("No store IDs configured for bootstrap sync");
            return Ok(BootstrapResult {
                stores: Vec::new(),
                duration: start.elapsed(),
            });
        }

        let mut store_results = Vec::with_capacity(self.config.store_ids.len());

        for store_id in &self.config.store_ids {
            let result = self.sync_store(store_id).await?;
            store_results.push(result);
        }

        let duration = start.elapsed();
        let total_tuples: usize = store_results.iter().map(|r| r.tuples_synced).sum();

        info!(
            stores = store_results.len(),
            total_tuples,
            duration_ms = duration.as_millis() as u64,
            "Bootstrap sync complete for all stores"
        );

        Ok(BootstrapResult {
            stores: store_results,
            duration,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsfga_storage::MemoryDataStore;

    /// Helper to create an in-memory source store with test tuples.
    async fn setup_source_with_tuples(store_id: &str, count: usize) -> Arc<MemoryDataStore> {
        use rsfga_storage::StoredTuple;

        let store = Arc::new(MemoryDataStore::new());
        store.create_store(store_id, store_id).await.unwrap();

        let tuples: Vec<StoredTuple> = (0..count)
            .map(|i| StoredTuple {
                user_type: "user".to_string(),
                user_id: format!("user{i}"),
                user_relation: None,
                relation: "viewer".to_string(),
                object_type: "document".to_string(),
                object_id: format!("doc{i}"),
                condition_name: None,
                condition_context: None,
                created_at: None,
            })
            .collect();

        if !tuples.is_empty() {
            store.write_tuples(store_id, tuples, vec![]).await.unwrap();
        }

        store
    }

    /// Helper to create an empty in-memory target store.
    async fn setup_target(store_id: &str) -> Arc<MemoryDataStore> {
        let store = Arc::new(MemoryDataStore::new());
        store.create_store(store_id, store_id).await.unwrap();
        store
    }

    /// Helper to count tuples in a store.
    async fn count_tuples(store: &dyn DataStore, store_id: &str) -> usize {
        let tuples = store
            .read_tuples(store_id, &TupleFilter::default())
            .await
            .unwrap();
        tuples.len()
    }

    // ===========================================
    // Section 1: Basic Bootstrap Sync
    // ===========================================

    #[tokio::test]
    async fn test_bootstrap_sync_copies_all_tuples() {
        let source = setup_source_with_tuples("store-1", 10).await;
        let target = setup_target("store-1").await;
        let metrics = Arc::new(EdgeMetrics::new());

        let syncer = BootstrapSyncer::new(
            source.clone(),
            target.clone(),
            metrics,
            BootstrapConfig {
                page_size: 100,
                store_ids: vec!["store-1".to_string()],
            },
        );

        let result = syncer.sync_store("store-1").await.unwrap();

        assert_eq!(result.tuples_synced, 10);
        assert_eq!(result.store_id, "store-1");
        assert_eq!(count_tuples(target.as_ref(), "store-1").await, 10);
    }

    #[tokio::test]
    async fn test_bootstrap_sync_handles_pagination() {
        // Use a small page size to force multiple pages
        let source = setup_source_with_tuples("store-1", 25).await;
        let target = setup_target("store-1").await;
        let metrics = Arc::new(EdgeMetrics::new());

        let syncer = BootstrapSyncer::new(
            source.clone(),
            target.clone(),
            metrics,
            BootstrapConfig {
                page_size: 10, // Forces 3 pages (10 + 10 + 5)
                store_ids: vec!["store-1".to_string()],
            },
        );

        let result = syncer.sync_store("store-1").await.unwrap();

        assert_eq!(result.tuples_synced, 25);
        assert!(
            result.pages_read >= 3,
            "Should need at least 3 pages for 25 tuples with page_size=10"
        );
        assert_eq!(count_tuples(target.as_ref(), "store-1").await, 25);
    }

    #[tokio::test]
    async fn test_bootstrap_sync_empty_store() {
        let source = setup_source_with_tuples("store-1", 0).await;
        let target = setup_target("store-1").await;
        let metrics = Arc::new(EdgeMetrics::new());

        let syncer = BootstrapSyncer::new(
            source.clone(),
            target.clone(),
            metrics,
            BootstrapConfig {
                page_size: 100,
                store_ids: vec!["store-1".to_string()],
            },
        );

        let result = syncer.sync_store("store-1").await.unwrap();

        assert_eq!(result.tuples_synced, 0);
        assert_eq!(result.pages_read, 1); // One empty page read
        assert_eq!(count_tuples(target.as_ref(), "store-1").await, 0);
    }

    // ===========================================
    // Section 2: Multi-Store and sync_all
    // ===========================================

    #[tokio::test]
    async fn test_bootstrap_sync_all_multiple_stores() {
        let source = Arc::new(MemoryDataStore::new());
        let target = Arc::new(MemoryDataStore::new());

        // Setup source with two stores
        source.create_store("store-a", "Store A").await.unwrap();
        source.create_store("store-b", "Store B").await.unwrap();
        target.create_store("store-a", "Store A").await.unwrap();
        target.create_store("store-b", "Store B").await.unwrap();

        // Add tuples to source stores
        let tuples_a: Vec<_> = (0..5)
            .map(|i| rsfga_storage::StoredTuple {
                user_type: "user".to_string(),
                user_id: format!("user{i}"),
                user_relation: None,
                relation: "viewer".to_string(),
                object_type: "document".to_string(),
                object_id: format!("doc{i}"),
                condition_name: None,
                condition_context: None,
                created_at: None,
            })
            .collect();
        let tuples_b: Vec<_> = (0..8)
            .map(|i| rsfga_storage::StoredTuple {
                user_type: "user".to_string(),
                user_id: format!("user{i}"),
                user_relation: None,
                relation: "editor".to_string(),
                object_type: "folder".to_string(),
                object_id: format!("folder{i}"),
                condition_name: None,
                condition_context: None,
                created_at: None,
            })
            .collect();

        source
            .write_tuples("store-a", tuples_a, vec![])
            .await
            .unwrap();
        source
            .write_tuples("store-b", tuples_b, vec![])
            .await
            .unwrap();

        let metrics = Arc::new(EdgeMetrics::new());
        let syncer = BootstrapSyncer::new(
            source,
            target.clone(),
            metrics,
            BootstrapConfig {
                page_size: 100,
                store_ids: vec!["store-a".to_string(), "store-b".to_string()],
            },
        );

        let result = syncer.sync_all().await.unwrap();

        assert_eq!(result.stores.len(), 2);
        assert_eq!(result.total_tuples(), 13);
        assert_eq!(count_tuples(target.as_ref(), "store-a").await, 5);
        assert_eq!(count_tuples(target.as_ref(), "store-b").await, 8);
    }

    #[tokio::test]
    async fn test_bootstrap_sync_all_empty_config() {
        let source = Arc::new(MemoryDataStore::new());
        let target = Arc::new(MemoryDataStore::new());
        let metrics = Arc::new(EdgeMetrics::new());

        let syncer = BootstrapSyncer::new(
            source,
            target,
            metrics,
            BootstrapConfig {
                page_size: 100,
                store_ids: vec![], // No stores configured
            },
        );

        let result = syncer.sync_all().await.unwrap();

        assert!(result.stores.is_empty());
        assert_eq!(result.total_tuples(), 0);
    }

    // ===========================================
    // Section 3: Metrics and Large Datasets
    // ===========================================

    #[tokio::test]
    async fn test_bootstrap_sync_records_metrics() {
        use std::sync::atomic::Ordering;

        let source = setup_source_with_tuples("store-1", 15).await;
        let target = setup_target("store-1").await;
        let metrics = Arc::new(EdgeMetrics::new());

        let syncer = BootstrapSyncer::new(
            source.clone(),
            target.clone(),
            metrics.clone(),
            BootstrapConfig {
                page_size: 10,
                store_ids: vec!["store-1".to_string()],
            },
        );

        syncer.sync_store("store-1").await.unwrap();

        assert_eq!(
            metrics.tuples_written.load(Ordering::Relaxed),
            15,
            "Should record all synced tuples in metrics"
        );
    }

    #[tokio::test]
    async fn test_bootstrap_sync_large_dataset() {
        // Simulate a large dataset (1000 tuples) with small page size
        let source = setup_source_with_tuples("store-1", 1000).await;
        let target = setup_target("store-1").await;
        let metrics = Arc::new(EdgeMetrics::new());

        let syncer = BootstrapSyncer::new(
            source.clone(),
            target.clone(),
            metrics,
            BootstrapConfig {
                page_size: 100, // 10 pages
                store_ids: vec!["store-1".to_string()],
            },
        );

        let result = syncer.sync_store("store-1").await.unwrap();

        assert_eq!(result.tuples_synced, 1000);
        assert_eq!(count_tuples(target.as_ref(), "store-1").await, 1000);
    }

    // ===========================================
    // Section 4: Idempotent Re-bootstrap
    // ===========================================

    #[tokio::test]
    async fn test_bootstrap_sync_is_idempotent() {
        // Running bootstrap twice should produce the same result (upsert semantics)
        let source = setup_source_with_tuples("store-1", 10).await;
        let target = setup_target("store-1").await;
        let metrics = Arc::new(EdgeMetrics::new());

        let syncer = BootstrapSyncer::new(
            source.clone(),
            target.clone(),
            metrics,
            BootstrapConfig {
                page_size: 100,
                store_ids: vec!["store-1".to_string()],
            },
        );

        // First bootstrap
        let result1 = syncer.sync_store("store-1").await.unwrap();
        assert_eq!(result1.tuples_synced, 10);

        // Second bootstrap (should succeed with same result)
        let result2 = syncer.sync_store("store-1").await.unwrap();
        assert_eq!(result2.tuples_synced, 10);

        // Target should still have exactly 10 tuples (not 20)
        assert_eq!(count_tuples(target.as_ref(), "store-1").await, 10);
    }

    // ===========================================
    // Section 5: BootstrapConfig Tests
    // ===========================================

    #[test]
    fn test_bootstrap_config_defaults() {
        let config = BootstrapConfig::default();
        assert_eq!(config.page_size, DEFAULT_BOOTSTRAP_PAGE_SIZE);
        assert!(config.store_ids.is_empty());
    }

    #[test]
    fn test_bootstrap_result_total_tuples() {
        let result = BootstrapResult {
            stores: vec![
                BootstrapStoreResult {
                    store_id: "store-a".to_string(),
                    tuples_synced: 100,
                    pages_read: 1,
                    duration: std::time::Duration::from_millis(50),
                },
                BootstrapStoreResult {
                    store_id: "store-b".to_string(),
                    tuples_synced: 200,
                    pages_read: 2,
                    duration: std::time::Duration::from_millis(100),
                },
            ],
            duration: std::time::Duration::from_millis(150),
        };

        assert_eq!(result.total_tuples(), 300);
    }
}
