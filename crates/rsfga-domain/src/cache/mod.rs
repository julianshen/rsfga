//! Check result caching with TTL and async invalidation.
//!
//! This module provides a high-performance check cache using Moka for
//! concurrent access with built-in TTL-based eviction.
//!
//! # Architecture
//!
//! The cache uses Moka's async Cache which provides:
//! - Lock-free concurrent reads
//! - Automatic TTL-based eviction
//! - Memory-bounded storage
//!
//! # Performance Characteristics (Issue #60)
//!
//! - **Insert**: O(1) - hash-based insertion
//! - **Get**: O(1) - hash-based lookup
//! - **Invalidate single**: O(1) - direct key removal
//! - **Invalidate by store**: O(K) where K is keys for that store (not O(N))
//!
//! Secondary indices enable O(1) store-based invalidation instead of
//! scanning all N entries.
//!
//! # Key Design (ADR-002)
//!
//! Cache keys include: `(store_id, object, relation, user)` to ensure
//! complete isolation between stores and precise invalidation.
//!
//! # Cache Safety
//!
//! By default, caching is **disabled** (`enabled: false`). This is a deliberate
//! safety measure because cached positive authorization decisions (`allowed=true`)
//! can serve stale results after tuple writes/deletes until TTL expires.
//!
//! Enable caching only when:
//! 1. You understand the staleness implications for your use case
//! 2. Write handlers properly invalidate affected cache entries
//! 3. The TTL is acceptable for your authorization freshness requirements
//!
//! # Example
//!
//! ```rust,ignore
//! use rsfga_domain::cache::{CheckCache, CheckCacheConfig, CacheKey};
//! use std::time::Duration;
//!
//! // Explicitly enable caching (opt-in for safety)
//! let config = CheckCacheConfig::default().with_enabled(true);
//! let cache = CheckCache::new(config);
//!
//! let key = CacheKey::new("store-1", "document:doc1", "viewer", "user:alice");
//! cache.insert(key.clone(), true).await;
//!
//! assert_eq!(cache.get(&key).await, Some(true));
//! ```

use dashmap::DashMap;
use moka::future::Cache;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::time::Duration;

/// Configuration for the check cache.
///
/// # Cache Safety
///
/// By default, caching is **disabled** (`enabled: false`). This is a deliberate
/// safety measure because cached positive authorization decisions can serve
/// stale results after tuple writes/deletes until TTL expires.
///
/// Enable caching only when:
/// 1. You understand the staleness implications for your use case
/// 2. Write handlers properly invalidate affected cache entries
/// 3. The TTL is acceptable for your authorization freshness requirements
#[derive(Debug, Clone)]
pub struct CheckCacheConfig {
    /// Whether caching is enabled.
    ///
    /// Defaults to `false` for safety - cached positive auth decisions can
    /// serve stale results. Enable explicitly when staleness is acceptable.
    pub enabled: bool,
    /// Maximum number of entries in the cache.
    pub max_capacity: u64,
    /// Default TTL for cache entries.
    pub default_ttl: Duration,
}

impl Default for CheckCacheConfig {
    fn default() -> Self {
        Self {
            enabled: false, // Disabled by default for safety (#131)
            max_capacity: 100_000,
            default_ttl: Duration::from_secs(10),
        }
    }
}

impl CheckCacheConfig {
    /// Enables or disables caching.
    ///
    /// # Safety Note
    ///
    /// Enabling caching means cached positive authorization decisions may be
    /// served after tuple writes/deletes until TTL expires or invalidation occurs.
    /// Only enable when this staleness window is acceptable for your use case.
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Sets the maximum capacity.
    pub fn with_max_capacity(mut self, max_capacity: u64) -> Self {
        self.max_capacity = max_capacity;
        self
    }

    /// Sets the default TTL.
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.default_ttl = ttl;
        self
    }
}

/// Cache key that uniquely identifies a check operation.
///
/// The key includes store_id to ensure complete isolation between stores.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheKey {
    /// The store identifier.
    pub store_id: String,
    /// The object being checked (e.g., "document:doc1").
    pub object: String,
    /// The relation being checked (e.g., "viewer").
    pub relation: String,
    /// The user performing the access (e.g., "user:alice").
    pub user: String,
}

impl CacheKey {
    /// Creates a new cache key.
    pub fn new(
        store_id: impl Into<String>,
        object: impl Into<String>,
        relation: impl Into<String>,
        user: impl Into<String>,
    ) -> Self {
        Self {
            store_id: store_id.into(),
            object: object.into(),
            relation: relation.into(),
            user: user.into(),
        }
    }
}

impl Hash for CacheKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.store_id.hash(state);
        self.object.hash(state);
        self.relation.hash(state);
        self.user.hash(state);
    }
}

/// High-performance check result cache with TTL support.
///
/// Uses Moka's async Cache for lock-free concurrent access with
/// automatic TTL-based eviction.
///
/// # Performance (Issue #60)
///
/// Uses secondary indices for O(1) store-based invalidation:
/// - `by_store`: Maps store_id to all cache keys for that store
/// - `by_object`: Maps (store_id, object) to all cache keys for that object
///
/// This avoids O(N) scans when invalidating entries for a specific store or object.
///
/// # Thread Safety
///
/// This cache is fully thread-safe and can be shared across multiple
/// async tasks without external synchronization.
pub struct CheckCache {
    /// The underlying Moka cache storing check results.
    cache: Cache<CacheKey, bool>,
    /// Configuration for this cache instance.
    config: CheckCacheConfig,
    /// Secondary index: store_id -> set of cache keys for O(1) store invalidation.
    by_store: DashMap<String, HashSet<CacheKey>>,
    /// Secondary index: (store_id, object) -> set of cache keys for O(1) object invalidation.
    by_object: DashMap<(String, String), HashSet<CacheKey>>,
}

impl std::fmt::Debug for CheckCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CheckCache")
            .field("config", &self.config)
            .field("entry_count", &self.cache.entry_count())
            .field("store_index_size", &self.by_store.len())
            .field("object_index_size", &self.by_object.len())
            .finish()
    }
}

impl CheckCache {
    /// Creates a new check cache with the given configuration.
    pub fn new(config: CheckCacheConfig) -> Self {
        let cache = Cache::builder()
            .max_capacity(config.max_capacity)
            .time_to_live(config.default_ttl)
            .build();

        Self {
            cache,
            config,
            by_store: DashMap::new(),
            by_object: DashMap::new(),
        }
    }

    /// Returns the configuration for this cache.
    pub fn config(&self) -> &CheckCacheConfig {
        &self.config
    }

    /// Returns whether caching is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Inserts a check result into the cache.
    ///
    /// The entry will expire after the configured TTL.
    /// Also updates secondary indices for O(1) invalidation.
    pub async fn insert(&self, key: CacheKey, allowed: bool) {
        // Clone store_id once to avoid double cloning
        let store_id = key.store_id.clone();
        let object_key = (store_id.clone(), key.object.clone());

        // Add to secondary indices for O(1) invalidation later
        self.by_store
            .entry(store_id)
            .or_default()
            .insert(key.clone());
        self.by_object
            .entry(object_key)
            .or_default()
            .insert(key.clone());

        self.cache.insert(key, allowed).await;
    }

    /// Retrieves a cached check result.
    ///
    /// Returns `None` if the key is not in the cache or has expired.
    ///
    /// # Metrics
    ///
    /// Records cache hit/miss to:
    /// - `rsfga_cache_hits_total` - Incremented on cache hit
    /// - `rsfga_cache_misses_total` - Incremented on cache miss
    pub async fn get(&self, key: &CacheKey) -> Option<bool> {
        let result = self.cache.get(key).await;
        if result.is_some() {
            metrics::counter!("rsfga_cache_hits_total").increment(1);
        } else {
            metrics::counter!("rsfga_cache_misses_total").increment(1);
        }
        result
    }

    /// Manually invalidates a single cache entry.
    /// Also removes from secondary indices.
    pub async fn invalidate(&self, key: &CacheKey) {
        // Remove from secondary indices
        if let Some(mut keys) = self.by_store.get_mut(&key.store_id) {
            keys.remove(key);
        }
        if let Some(mut keys) = self
            .by_object
            .get_mut(&(key.store_id.clone(), key.object.clone()))
        {
            keys.remove(key);
        }

        self.cache.invalidate(key).await;
    }

    /// Invalidates all entries for a specific store.
    ///
    /// Uses secondary index for O(K) where K is number of keys for this store,
    /// instead of O(N) where N is total cache entries.
    pub async fn invalidate_store(&self, store_id: &str) {
        // Use atomic remove() to avoid TOCTOU race condition.
        // This ensures no concurrent insert can add keys between reading and removing.
        if let Some((_, keys_to_remove)) = self.by_store.remove(store_id) {
            for key in &keys_to_remove {
                self.cache.invalidate(key).await;
                // Also remove from by_object index
                if let Some(mut object_keys) = self
                    .by_object
                    .get_mut(&(key.store_id.clone(), key.object.clone()))
                {
                    object_keys.remove(key);
                }
            }
        }
    }

    /// Invalidates entries matching a predicate.
    ///
    /// The predicate receives each cache key and returns true if the
    /// entry should be invalidated.
    pub async fn invalidate_matching<F>(&self, predicate: F)
    where
        F: Fn(&CacheKey) -> bool,
    {
        self.cache.run_pending_tasks().await;

        // Note: Moka's iter() returns (Arc<K>, V), so we need to deref
        let keys_to_remove: Vec<CacheKey> = self
            .cache
            .iter()
            .filter(|(k, _)| predicate(k.as_ref()))
            .map(|(k, _)| (*k).clone())
            .collect();

        for key in keys_to_remove {
            self.cache.invalidate(&key).await;
        }
    }

    /// Returns the approximate number of entries in the cache.
    pub fn entry_count(&self) -> u64 {
        self.cache.entry_count()
    }

    /// Runs pending maintenance tasks.
    ///
    /// This triggers any pending evictions. Useful for testing TTL behavior.
    pub async fn run_pending_tasks(&self) {
        self.cache.run_pending_tasks().await;
    }

    /// Invalidates cache entries affected by a tuple write or delete.
    ///
    /// When a tuple like `user:alice#viewer@document:doc1` is written or deleted,
    /// we need to invalidate any cached check results that might be affected.
    ///
    /// Uses secondary index for O(K) where K is keys for this (store_id, object),
    /// instead of O(N) where N is total cache entries.
    ///
    /// For now, we use a conservative approach and invalidate all entries
    /// for the affected object across all relations.
    pub async fn invalidate_for_tuple(&self, store_id: &str, object: &str, _relation: &str) {
        // Use atomic remove() to avoid TOCTOU race condition.
        // This ensures no concurrent insert can add keys between reading and removing.
        let index_key = (store_id.to_string(), object.to_string());
        if let Some((_, keys_to_remove)) = self.by_object.remove(&index_key) {
            for key in &keys_to_remove {
                self.cache.invalidate(key).await;
                // Also remove from by_store index
                if let Some(mut store_keys) = self.by_store.get_mut(&key.store_id) {
                    store_keys.remove(key);
                }
            }
        }
    }

    /// Spawns an async invalidation task that runs in the background.
    ///
    /// This is the fire-and-forget pattern for write handlers - the write
    /// completes immediately while invalidation happens asynchronously.
    ///
    /// Returns a handle that can be used to wait for completion if needed.
    pub fn spawn_invalidation_for_tuple(
        self: &std::sync::Arc<Self>,
        store_id: String,
        object: String,
        relation: String,
    ) -> tokio::task::JoinHandle<()> {
        let cache = std::sync::Arc::clone(self);
        tokio::spawn(async move {
            cache
                .invalidate_for_tuple(&store_id, &object, &relation)
                .await;
        })
    }

    /// Measures the time it takes for an invalidation to complete.
    ///
    /// This is useful for monitoring staleness windows.
    pub async fn timed_invalidate_for_tuple(
        &self,
        store_id: &str,
        object: &str,
        relation: &str,
    ) -> Duration {
        let start = std::time::Instant::now();
        self.invalidate_for_tuple(store_id, object, relation).await;
        start.elapsed()
    }
}

impl Clone for CheckCache {
    fn clone(&self) -> Self {
        Self {
            cache: self.cache.clone(),
            config: self.config.clone(),
            by_store: self.by_store.clone(),
            by_object: self.by_object.clone(),
        }
    }
}

/// Registers check cache metrics descriptions.
///
/// Call this function once during application startup to register metric
/// descriptions with the metrics recorder. This is optional but provides
/// better documentation in Prometheus/Grafana.
///
/// # Metrics Registered
///
/// - `rsfga_cache_hits_total` - Total number of check cache hits
/// - `rsfga_cache_misses_total` - Total number of check cache misses
/// - `rsfga_cache_size` - Current number of cached check results (gauge)
///
/// # Example
///
/// ```ignore
/// use rsfga_domain::cache::register_check_cache_metrics;
///
/// // During application initialization
/// register_check_cache_metrics();
/// ```
pub fn register_check_cache_metrics() {
    metrics::describe_counter!(
        "rsfga_cache_hits_total",
        "Total number of check cache hits"
    );
    metrics::describe_counter!(
        "rsfga_cache_misses_total",
        "Total number of check cache misses"
    );
    metrics::describe_gauge!(
        "rsfga_cache_size",
        "Current number of cached check results"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Helper to create an enabled cache config for tests.
    fn enabled_cache_config() -> CheckCacheConfig {
        CheckCacheConfig::default().with_enabled(true)
    }

    // ============================================================
    // Section 1: Cache Structure
    // ============================================================

    #[tokio::test]
    async fn test_cache_creation_and_initial_state() {
        // Arrange
        let config = enabled_cache_config();

        // Act
        let cache = CheckCache::new(config);

        // Assert
        // Cache should be created successfully and be empty
        let test_key = CacheKey::new("store-1", "object:1", "relation", "user:1");
        assert!(cache.get(&test_key).await.is_none());
    }

    #[tokio::test]
    async fn test_can_insert_check_result_into_cache() {
        // Arrange
        let cache = CheckCache::new(enabled_cache_config());
        let key = CacheKey::new("store-1", "document:doc1", "viewer", "user:alice");

        // Act
        cache.insert(key.clone(), true).await;

        // Assert
        // Should be able to insert without error (implicit - no panic)
        assert!(cache.get(&key).await.is_some());
    }

    #[tokio::test]
    async fn test_can_retrieve_cached_check_result() {
        // Arrange
        let cache = CheckCache::new(enabled_cache_config());
        let key = CacheKey::new("store-1", "document:doc1", "viewer", "user:alice");
        cache.insert(key.clone(), true).await;

        // Act
        let result = cache.get(&key).await;

        // Assert
        assert_eq!(result, Some(true));
    }

    #[tokio::test]
    async fn test_cache_miss_returns_none() {
        // Arrange
        let cache = CheckCache::new(enabled_cache_config());
        let key = CacheKey::new("store-1", "document:doc1", "viewer", "user:alice");
        // Intentionally NOT inserting the key

        // Act
        let result = cache.get(&key).await;

        // Assert
        assert_eq!(result, None);
    }

    #[test]
    fn test_cache_key_includes_store_user_relation_object() {
        // Arrange
        let key = CacheKey::new("store-1", "document:doc1", "viewer", "user:alice");

        // Assert - all components should be stored correctly
        assert_eq!(key.store_id, "store-1");
        assert_eq!(key.object, "document:doc1");
        assert_eq!(key.relation, "viewer");
        assert_eq!(key.user, "user:alice");
    }

    #[tokio::test]
    async fn test_different_stores_have_separate_cache_entries() {
        // Arrange
        let cache = CheckCache::new(enabled_cache_config());

        // Same object/relation/user but different stores
        let key_store1 = CacheKey::new("store-1", "document:doc1", "viewer", "user:alice");
        let key_store2 = CacheKey::new("store-2", "document:doc1", "viewer", "user:alice");

        // Act
        cache.insert(key_store1.clone(), true).await;
        cache.insert(key_store2.clone(), false).await;

        // Assert - should be separate entries
        assert_eq!(cache.get(&key_store1).await, Some(true));
        assert_eq!(cache.get(&key_store2).await, Some(false));
    }

    #[test]
    fn test_cache_disabled_by_default() {
        // Arrange & Act
        let config = CheckCacheConfig::default();

        // Assert - caching should be disabled by default for safety
        assert!(!config.enabled);
    }

    #[test]
    fn test_cache_can_be_explicitly_enabled() {
        // Arrange & Act
        let config = CheckCacheConfig::default().with_enabled(true);

        // Assert
        assert!(config.enabled);
    }

    // ============================================================
    // Section 2: TTL and Eviction
    // ============================================================

    #[tokio::test]
    async fn test_cached_entry_expires_after_ttl() {
        // Arrange - use a very short TTL for testing
        let config = CheckCacheConfig {
            enabled: true,
            max_capacity: 100,
            default_ttl: Duration::from_millis(50),
        };
        let cache = CheckCache::new(config);
        let key = CacheKey::new("store-1", "document:doc1", "viewer", "user:alice");

        // Act
        cache.insert(key.clone(), true).await;

        // Verify it's initially present
        assert_eq!(cache.get(&key).await, Some(true));

        // Wait for TTL to expire
        tokio::time::sleep(Duration::from_millis(100)).await;
        cache.run_pending_tasks().await;

        // Assert - entry should be expired
        assert_eq!(cache.get(&key).await, None);
    }

    #[tokio::test]
    async fn test_ttl_is_configurable() {
        // Arrange - create two caches with different TTLs
        let short_ttl_config = CheckCacheConfig {
            enabled: true,
            max_capacity: 100,
            default_ttl: Duration::from_millis(50),
        };
        let long_ttl_config = CheckCacheConfig {
            enabled: true,
            max_capacity: 100,
            default_ttl: Duration::from_secs(60),
        };

        let short_cache = CheckCache::new(short_ttl_config.clone());
        let long_cache = CheckCache::new(long_ttl_config.clone());

        // Assert - configurations are different
        assert_eq!(short_cache.config().default_ttl, Duration::from_millis(50));
        assert_eq!(long_cache.config().default_ttl, Duration::from_secs(60));

        // Verify behavior differs
        let key = CacheKey::new("store-1", "document:doc1", "viewer", "user:alice");

        short_cache.insert(key.clone(), true).await;
        long_cache.insert(key.clone(), true).await;

        // Wait for short TTL to expire
        tokio::time::sleep(Duration::from_millis(100)).await;
        short_cache.run_pending_tasks().await;
        long_cache.run_pending_tasks().await;

        // Short cache should be expired, long cache should still have value
        assert_eq!(short_cache.get(&key).await, None);
        assert_eq!(long_cache.get(&key).await, Some(true));
    }

    #[tokio::test]
    async fn test_can_manually_invalidate_cache_entry() {
        // Arrange
        let cache = CheckCache::new(enabled_cache_config());
        let key = CacheKey::new("store-1", "document:doc1", "viewer", "user:alice");
        cache.insert(key.clone(), true).await;

        // Verify it's present
        assert_eq!(cache.get(&key).await, Some(true));

        // Act
        cache.invalidate(&key).await;

        // Assert
        assert_eq!(cache.get(&key).await, None);
    }

    #[tokio::test]
    async fn test_can_invalidate_all_entries_for_store() {
        // Arrange
        let cache = CheckCache::new(enabled_cache_config());

        // Insert entries for two different stores
        let key_store1_a = CacheKey::new("store-1", "doc:a", "viewer", "user:alice");
        let key_store1_b = CacheKey::new("store-1", "doc:b", "editor", "user:bob");
        let key_store2 = CacheKey::new("store-2", "doc:a", "viewer", "user:alice");

        cache.insert(key_store1_a.clone(), true).await;
        cache.insert(key_store1_b.clone(), true).await;
        cache.insert(key_store2.clone(), true).await;

        // Act - invalidate all entries for store-1
        cache.invalidate_store("store-1").await;

        // Assert - store-1 entries should be gone, store-2 should remain
        assert_eq!(cache.get(&key_store1_a).await, None);
        assert_eq!(cache.get(&key_store1_b).await, None);
        assert_eq!(cache.get(&key_store2).await, Some(true));
    }

    #[tokio::test]
    async fn test_can_invalidate_entries_matching_pattern() {
        // Arrange
        let cache = CheckCache::new(enabled_cache_config());

        let key_viewer = CacheKey::new("store-1", "doc:a", "viewer", "user:alice");
        let key_editor = CacheKey::new("store-1", "doc:b", "editor", "user:alice");
        let key_owner = CacheKey::new("store-1", "doc:c", "owner", "user:alice");

        cache.insert(key_viewer.clone(), true).await;
        cache.insert(key_editor.clone(), true).await;
        cache.insert(key_owner.clone(), true).await;

        // Act - invalidate all entries with "viewer" relation
        cache.invalidate_matching(|k| k.relation == "viewer").await;

        // Assert - only viewer should be invalidated
        assert_eq!(cache.get(&key_viewer).await, None);
        assert_eq!(cache.get(&key_editor).await, Some(true));
        assert_eq!(cache.get(&key_owner).await, Some(true));
    }

    #[tokio::test]
    async fn test_eviction_doesnt_block_reads() {
        // Arrange - this test verifies that eviction runs in background
        // and doesn't block read operations
        let config = CheckCacheConfig {
            enabled: true,
            max_capacity: 100,
            default_ttl: Duration::from_millis(10),
        };
        let cache = Arc::new(CheckCache::new(config));

        // Insert many entries that will expire
        for i in 0..50 {
            let key = CacheKey::new("store-1", format!("doc:{i}"), "viewer", "user:alice");
            cache.insert(key, true).await;
        }

        // Wait for entries to expire
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Act - perform reads while eviction might be happening
        // This should complete without blocking
        let cache_clone = cache.clone();
        let read_task = tokio::spawn(async move {
            let start = std::time::Instant::now();

            // Perform multiple reads
            for i in 0..100 {
                let key = CacheKey::new("store-1", format!("doc:{i}"), "viewer", "user:alice");
                let _ = cache_clone.get(&key).await;
            }

            start.elapsed()
        });

        // Assert - reads should complete quickly (under 100ms)
        let read_duration = read_task.await.unwrap();
        assert!(
            read_duration < Duration::from_millis(100),
            "Reads took too long: {read_duration:?}"
        );
    }

    // ============================================================
    // Section 3: Cache Invalidation
    // ============================================================

    #[tokio::test]
    async fn test_writing_tuple_invalidates_affected_cache_entries() {
        // Arrange
        let cache = CheckCache::new(enabled_cache_config());

        // Cache some check results for document:doc1
        let key_viewer = CacheKey::new("store-1", "document:doc1", "viewer", "user:alice");
        let key_editor = CacheKey::new("store-1", "document:doc1", "editor", "user:alice");
        let key_other_doc = CacheKey::new("store-1", "document:doc2", "viewer", "user:alice");

        cache.insert(key_viewer.clone(), true).await;
        cache.insert(key_editor.clone(), true).await;
        cache.insert(key_other_doc.clone(), true).await;

        // Simulate writing a tuple for document:doc1
        // Act
        cache
            .invalidate_for_tuple("store-1", "document:doc1", "viewer")
            .await;

        // Assert - entries for document:doc1 should be invalidated
        // but document:doc2 should remain
        assert_eq!(cache.get(&key_viewer).await, None);
        assert_eq!(cache.get(&key_editor).await, None);
        assert_eq!(cache.get(&key_other_doc).await, Some(true));
    }

    #[tokio::test]
    async fn test_deleting_tuple_invalidates_affected_cache_entries() {
        // Arrange - same setup as write test
        let cache = CheckCache::new(enabled_cache_config());

        let key_viewer = CacheKey::new("store-1", "document:doc1", "viewer", "user:alice");
        let key_other_user = CacheKey::new("store-1", "document:doc1", "viewer", "user:bob");

        cache.insert(key_viewer.clone(), true).await;
        cache.insert(key_other_user.clone(), true).await;

        // Simulate deleting a tuple for document:doc1
        // Act
        cache
            .invalidate_for_tuple("store-1", "document:doc1", "viewer")
            .await;

        // Assert - all entries for document:doc1 should be invalidated
        assert_eq!(cache.get(&key_viewer).await, None);
        assert_eq!(cache.get(&key_other_user).await, None);
    }

    #[tokio::test]
    async fn test_invalidation_is_async_doesnt_block_write_response() {
        // Arrange
        let cache = Arc::new(CheckCache::new(enabled_cache_config()));

        // Insert many entries to make invalidation take some time
        for i in 0..100 {
            let key = CacheKey::new(
                "store-1",
                format!("document:doc{i}"),
                "viewer",
                "user:alice",
            );
            cache.insert(key, true).await;
        }

        // Act - spawn async invalidation and measure how quickly we return
        let start = std::time::Instant::now();

        // Spawn invalidation in background (fire-and-forget)
        let handle = cache.spawn_invalidation_for_tuple(
            "store-1".to_string(),
            "document:doc50".to_string(),
            "viewer".to_string(),
        );

        let spawn_duration = start.elapsed();

        // Assert - spawning should be nearly instant (under 1ms)
        assert!(
            spawn_duration < Duration::from_millis(1),
            "Spawning invalidation took too long: {spawn_duration:?}"
        );

        // Wait for invalidation to complete to verify it works
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_invalidation_completes_eventually() {
        // Arrange
        let cache = Arc::new(CheckCache::new(enabled_cache_config()));

        let key = CacheKey::new("store-1", "document:doc1", "viewer", "user:alice");
        cache.insert(key.clone(), true).await;

        // Verify entry exists
        assert_eq!(cache.get(&key).await, Some(true));

        // Act - spawn async invalidation
        let handle = cache.spawn_invalidation_for_tuple(
            "store-1".to_string(),
            "document:doc1".to_string(),
            "viewer".to_string(),
        );

        // Wait for completion
        handle.await.unwrap();

        // Assert - invalidation should have completed
        assert_eq!(cache.get(&key).await, None);
    }

    #[tokio::test]
    async fn test_invalidation_handles_errors_gracefully() {
        // Arrange - test that invalidation doesn't panic on edge cases
        let cache = CheckCache::new(enabled_cache_config());

        // Act - invalidate a tuple for non-existent store/object
        // This should not panic
        cache
            .invalidate_for_tuple("nonexistent-store", "nonexistent:object", "relation")
            .await;

        // Act - invalidate on empty cache
        let empty_cache = CheckCache::new(enabled_cache_config());
        empty_cache
            .invalidate_for_tuple("store-1", "document:doc1", "viewer")
            .await;

        // Assert - no panic means success
        // The test passes if we get here
    }

    #[tokio::test]
    async fn test_can_measure_staleness_window() {
        // Arrange
        let cache = CheckCache::new(enabled_cache_config());

        // Insert entries
        for i in 0..10 {
            let key = CacheKey::new(
                "store-1",
                format!("document:doc{i}"),
                "viewer",
                "user:alice",
            );
            cache.insert(key, true).await;
        }

        // Act - measure multiple invalidation times
        let mut durations = Vec::new();
        for i in 0..10 {
            let duration = cache
                .timed_invalidate_for_tuple("store-1", &format!("document:doc{i}"), "viewer")
                .await;
            durations.push(duration);
        }

        // Calculate p99 (for 10 samples, just take max)
        let p99 = durations.iter().max().unwrap();

        // Assert - p99 should be under 100ms target (ADR-007)
        // Note: In tests with small data, this should be well under 1ms
        assert!(
            *p99 < Duration::from_millis(100),
            "Staleness window p99 too high: {p99:?}"
        );

        // Log the measurement for informational purposes
        println!(
            "Staleness measurements: min={:?}, max(p99)={:?}",
            durations.iter().min().unwrap(),
            p99
        );
    }

    // ============================================================
    // Section 4: Concurrent Access
    // ============================================================

    #[tokio::test]
    async fn test_concurrent_reads_dont_block_each_other() {
        // Arrange
        let cache = Arc::new(CheckCache::new(enabled_cache_config()));

        // Pre-populate cache with entries
        for i in 0..100 {
            let key = CacheKey::new(
                "store-1",
                format!("document:doc{i}"),
                "viewer",
                "user:alice",
            );
            cache.insert(key, i % 2 == 0).await;
        }

        // Act - spawn many concurrent read tasks
        let mut handles = Vec::new();
        for task_id in 0..10 {
            let cache_clone = cache.clone();
            let handle = tokio::spawn(async move {
                let start = std::time::Instant::now();

                // Each task reads 100 entries
                for i in 0..100 {
                    let key = CacheKey::new(
                        "store-1",
                        format!("document:doc{i}"),
                        "viewer",
                        "user:alice",
                    );
                    let result = cache_clone.get(&key).await;

                    // Verify we get correct values
                    assert_eq!(result, Some(i % 2 == 0));
                }

                (task_id, start.elapsed())
            });
            handles.push(handle);
        }

        // Wait for all to complete
        let results: Vec<_> = futures::future::join_all(handles)
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        // Assert - all should complete quickly (within 100ms)
        for (task_id, duration) in &results {
            assert!(
                *duration < Duration::from_millis(100),
                "Task {task_id} took too long: {duration:?}"
            );
        }
    }

    #[tokio::test]
    async fn test_concurrent_writes_dont_lose_data() {
        // Arrange
        let cache = Arc::new(CheckCache::new(enabled_cache_config()));
        let completed = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        // Act - spawn many concurrent write tasks, each writing to different keys
        let mut handles = Vec::new();
        for task_id in 0..10 {
            let cache_clone = cache.clone();
            let completed_clone = completed.clone();
            let handle = tokio::spawn(async move {
                // Each task writes 100 unique entries
                for i in 0..100 {
                    let key = CacheKey::new(
                        "store-1",
                        format!("document:task{task_id}_doc{i}"),
                        "viewer",
                        "user:alice",
                    );
                    cache_clone.insert(key, true).await;
                }
                completed_clone.fetch_add(100, std::sync::atomic::Ordering::SeqCst);
            });
            handles.push(handle);
        }

        // Wait for all writes to complete
        futures::future::join_all(handles).await;

        // Assert - all writes should have completed
        assert_eq!(
            completed.load(std::sync::atomic::Ordering::SeqCst),
            1000 // 10 tasks * 100 writes each
        );

        // Verify all entries are in cache
        for task_id in 0..10 {
            for i in 0..100 {
                let key = CacheKey::new(
                    "store-1",
                    format!("document:task{task_id}_doc{i}"),
                    "viewer",
                    "user:alice",
                );
                assert_eq!(
                    cache.get(&key).await,
                    Some(true),
                    "Missing entry for task {task_id} doc {i}"
                );
            }
        }
    }

    #[tokio::test]
    async fn test_read_during_write_returns_valid_result() {
        // Arrange
        let cache = Arc::new(CheckCache::new(enabled_cache_config()));
        let key = CacheKey::new("store-1", "document:doc1", "viewer", "user:alice");

        // Insert initial value
        cache.insert(key.clone(), false).await;

        // Act - concurrent read and write
        let cache_for_write = cache.clone();
        let key_for_write = key.clone();
        let write_handle = tokio::spawn(async move {
            // Write new value
            cache_for_write.insert(key_for_write, true).await;
        });

        let cache_for_read = cache.clone();
        let key_for_read = key.clone();
        let read_handle = tokio::spawn(async move {
            // Read value - should get either old or new value, never garbage
            cache_for_read.get(&key_for_read).await
        });

        // Wait for both
        write_handle.await.unwrap();
        let read_result = read_handle.await.unwrap();

        // Assert - read should return either Some(false) or Some(true), never None or garbage
        assert!(
            read_result == Some(false) || read_result == Some(true),
            "Got unexpected result: {read_result:?}"
        );
    }

    #[tokio::test]
    async fn test_cache_scales_to_1000_plus_concurrent_operations() {
        // Arrange
        let cache = Arc::new(CheckCache::new(CheckCacheConfig {
            enabled: true,
            max_capacity: 10_000,
            default_ttl: Duration::from_secs(60),
        }));

        // Act - spawn 1000 concurrent operations (mix of reads and writes)
        let mut handles = Vec::new();
        for op_id in 0..1000 {
            let cache_clone = cache.clone();
            let handle = tokio::spawn(async move {
                let key = CacheKey::new(
                    "store-1",
                    format!("document:doc{}", op_id % 100), // 100 unique keys
                    "viewer",
                    "user:alice",
                );

                if op_id % 2 == 0 {
                    // Write operation
                    cache_clone.insert(key, true).await;
                } else {
                    // Read operation
                    let _ = cache_clone.get(&key).await;
                }
            });
            handles.push(handle);
        }

        // Wait for all with timeout
        let start = std::time::Instant::now();
        futures::future::join_all(handles).await;
        let duration = start.elapsed();

        // Assert - should complete within reasonable time (1 second for 1000 ops)
        assert!(
            duration < Duration::from_secs(1),
            "1000 concurrent operations took too long: {duration:?}"
        );
    }

    #[tokio::test]
    async fn test_no_deadlocks_under_high_contention() {
        // Arrange
        let cache = Arc::new(CheckCache::new(enabled_cache_config()));

        // Create a single "hot" key that all tasks will compete for
        let hot_key = CacheKey::new("store-1", "document:hot", "viewer", "user:alice");
        cache.insert(hot_key.clone(), true).await;

        // Act - spawn many tasks all accessing the same key
        let mut handles = Vec::new();
        for task_id in 0..100 {
            let cache_clone = cache.clone();
            let key_clone = hot_key.clone();
            let handle = tokio::spawn(async move {
                // Each task does multiple operations on the same key
                for _ in 0..100 {
                    // Alternate between read, write, and invalidate
                    match task_id % 3 {
                        0 => {
                            let _ = cache_clone.get(&key_clone).await;
                        }
                        1 => {
                            cache_clone
                                .insert(key_clone.clone(), task_id % 2 == 0)
                                .await;
                        }
                        _ => {
                            cache_clone.invalidate(&key_clone).await;
                        }
                    }
                }
            });
            handles.push(handle);
        }

        // Use timeout to detect deadlocks
        let result = tokio::time::timeout(Duration::from_secs(5), async {
            futures::future::join_all(handles).await
        })
        .await;

        // Assert - should complete without timeout (deadlock would cause timeout)
        assert!(
            result.is_ok(),
            "Deadlock detected: operations did not complete within timeout"
        );
    }
}
