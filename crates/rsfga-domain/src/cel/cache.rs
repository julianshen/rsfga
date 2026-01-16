//! CEL expression cache for avoiding repeated parsing.
//!
//! Parsing CEL expressions is expensive (lexing, parsing, AST construction).
//! This module provides a thread-safe cache that stores parsed expressions
//! keyed by their source string.
//!
//! # Performance Impact
//!
//! Without caching, each condition evaluation requires:
//! - Lexing the expression string
//! - Parsing tokens into AST
//! - Building the Program structure
//!
//! With caching, subsequent evaluations of the same expression only require
//! a hash lookup, providing 10-100x speedup for hot paths.
//!
//! # Thread Safety
//!
//! The cache uses moka for lock-free concurrent access with LRU eviction.
//!
//! # Metrics
//!
//! The cache emits the following Prometheus metrics:
//! - `rsfga_cel_cache_hits_total` - Number of cache hits
//! - `rsfga_cel_cache_misses_total` - Number of cache misses
//! - `rsfga_cel_cache_size` - Current number of cached expressions
//! - `rsfga_cel_cache_parse_duration_seconds` - Time to parse new expressions

use moka::sync::Cache;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::error::CelError;
use super::expression::CelExpression;
use super::CelResult;

/// Configuration for the CEL expression cache.
///
/// # Bounded Cache Behavior
///
/// The cache enforces both capacity and TTL limits:
/// - When `max_capacity` is reached, least-recently-used entries are evicted
/// - Entries expire after `ttl` and are removed on next access or background sweep
#[derive(Debug, Clone)]
pub struct CelCacheConfig {
    /// Maximum number of expressions to cache.
    ///
    /// When this limit is reached, least-recently-used entries are evicted.
    /// Default: 10,000 (expressions are small, typically <1KB each)
    pub max_capacity: u64,
    /// Time-to-live for cached expressions.
    ///
    /// Entries are evicted after this duration, ensuring stale expressions
    /// don't persist indefinitely. For immediate invalidation on model updates,
    /// use [`CelExpressionCache::invalidate_all`].
    /// Default: 1 hour
    pub ttl: Duration,
}

impl Default for CelCacheConfig {
    fn default() -> Self {
        Self {
            max_capacity: 10_000,
            ttl: Duration::from_secs(3600),
        }
    }
}

/// Thread-safe cache for parsed CEL expressions with LRU eviction.
///
/// This cache stores compiled CEL programs keyed by their source expression string.
/// Since `CelExpression` contains an `Arc<Program>`, cloning cached entries is cheap.
///
/// # Bounded Memory
///
/// Unlike simple HashMap-based caches, this implementation:
/// - Enforces `max_capacity` with LRU eviction
/// - Supports TTL-based expiration
/// - Provides O(1) concurrent access
///
/// # ⚠️ Clone Not Implemented
///
/// This type intentionally does **not** implement `Clone`. Cloning would require
/// deep-copying all entries (O(n) complexity), which is expensive. Instead:
/// - Use `Arc<CelExpressionCache>` for shared ownership across threads
/// - Use [`global_cache()`] singleton for most use cases
///
/// # Example
///
/// ```ignore
/// use rsfga_domain::cel::{CelExpressionCache, CelCacheConfig};
///
/// let cache = CelExpressionCache::new(CelCacheConfig::default());
///
/// // First call parses the expression
/// let expr1 = cache.get_or_parse("x > 5")?;
///
/// // Second call returns cached version (no parsing)
/// let expr2 = cache.get_or_parse("x > 5")?;
/// ```
pub struct CelExpressionCache {
    cache: Cache<String, Arc<CelExpression>>,
    #[allow(dead_code)]
    config: CelCacheConfig,
}

impl CelExpressionCache {
    /// Creates a new CEL expression cache with the given configuration.
    pub fn new(config: CelCacheConfig) -> Self {
        let cache = Cache::builder()
            .max_capacity(config.max_capacity)
            .time_to_live(config.ttl)
            .build();

        Self { cache, config }
    }

    /// Gets a cached expression or parses and caches it.
    ///
    /// This is the main entry point for cached expression access.
    /// If the expression is already cached, returns the cached version.
    /// Otherwise, parses the expression, caches it, and returns it.
    ///
    /// # Metrics
    ///
    /// This method records the following metrics:
    /// - `rsfga_cel_cache_hits_total` - Incremented on cache hit
    /// - `rsfga_cel_cache_misses_total` - Incremented on cache miss
    /// - `rsfga_cel_cache_parse_duration_seconds` - Parse time for cache misses
    /// - `rsfga_cel_cache_size` - Updated after cache modifications
    ///
    /// # Arguments
    ///
    /// * `expression` - The CEL expression string to parse
    ///
    /// # Returns
    ///
    /// * `Ok(Arc<CelExpression>)` - The parsed expression (cached or fresh)
    /// * `Err(CelError)` - If parsing fails
    pub fn get_or_parse(&self, expression: &str) -> CelResult<Arc<CelExpression>> {
        // Fast path: check cache first without allocation
        if let Some(expr) = self.cache.get(expression) {
            metrics::counter!("rsfga_cel_cache_hits_total").increment(1);
            return Ok(expr);
        }

        // Record cache miss
        metrics::counter!("rsfga_cel_cache_misses_total").increment(1);

        // Slow path: use try_get_with for atomic get-or-insert with fallible initialization.
        // This ensures that when multiple threads request the same expression
        // concurrently, only ONE thread parses it and all others get the same Arc.
        let start = Instant::now();
        let result = self
            .cache
            .try_get_with(expression.to_string(), || {
                let parsed = CelExpression::parse(expression)?;
                Ok(Arc::new(parsed))
            })
            .map_err(|e: Arc<CelError>| (*e).clone());

        // Record parse duration (only meaningful for actual parses, but we record
        // it for all cache misses to track the overall slow path cost)
        metrics::histogram!("rsfga_cel_cache_parse_duration_seconds")
            .record(start.elapsed().as_secs_f64());

        // Update cache size gauge
        metrics::gauge!("rsfga_cel_cache_size").set(self.cache.entry_count() as f64);

        result
    }

    /// Returns the number of cached expressions.
    ///
    /// Note: This count may be approximate due to concurrent modifications
    /// and pending evictions.
    pub fn entry_count(&self) -> u64 {
        self.cache.entry_count()
    }

    /// Returns the maximum capacity of the cache.
    pub fn max_capacity(&self) -> u64 {
        self.cache.policy().max_capacity().unwrap_or(0)
    }

    /// Invalidates all cached expressions.
    ///
    /// This should be called when authorization models are updated,
    /// as condition expressions may have changed.
    ///
    /// # Metrics
    ///
    /// Updates `rsfga_cel_cache_size` gauge to 0 after invalidation.
    pub fn invalidate_all(&self) {
        self.cache.invalidate_all();
        metrics::gauge!("rsfga_cel_cache_size").set(0.0);
    }

    /// Invalidates a specific expression from the cache.
    ///
    /// # Metrics
    ///
    /// Updates `rsfga_cel_cache_size` gauge after invalidation.
    pub fn invalidate(&self, expression: &str) {
        self.cache.invalidate(expression);
        // Run pending tasks to ensure the entry is removed before updating gauge
        self.cache.run_pending_tasks();
        metrics::gauge!("rsfga_cel_cache_size").set(self.cache.entry_count() as f64);
    }

    /// Runs pending maintenance tasks (eviction, expiration).
    ///
    /// Moka performs maintenance lazily, but this can be called to
    /// force immediate cleanup. Useful in tests.
    pub fn run_pending_tasks(&self) {
        self.cache.run_pending_tasks();
    }
}

impl Default for CelExpressionCache {
    fn default() -> Self {
        Self::new(CelCacheConfig::default())
    }
}

/// Registers CEL cache metrics descriptions.
///
/// Call this function once during application startup to register metric
/// descriptions with the metrics recorder. This is optional but provides
/// better documentation in Prometheus/Grafana.
///
/// # Example
///
/// ```ignore
/// use rsfga_domain::cel::cache::register_cel_cache_metrics;
///
/// // During application initialization
/// register_cel_cache_metrics();
/// ```
pub fn register_cel_cache_metrics() {
    metrics::describe_counter!(
        "rsfga_cel_cache_hits_total",
        "Total number of CEL expression cache hits"
    );
    metrics::describe_counter!(
        "rsfga_cel_cache_misses_total",
        "Total number of CEL expression cache misses"
    );
    metrics::describe_gauge!(
        "rsfga_cel_cache_size",
        "Current number of cached CEL expressions"
    );
    metrics::describe_histogram!(
        "rsfga_cel_cache_parse_duration_seconds",
        "Time to parse CEL expressions (on cache miss)"
    );
}

// Global singleton for convenience (most use cases need just one cache)
use std::sync::OnceLock;

static GLOBAL_CACHE: OnceLock<CelExpressionCache> = OnceLock::new();

/// Gets the global CEL expression cache.
///
/// This provides a convenient singleton for cases where explicit cache
/// management isn't needed. The global cache uses default configuration.
pub fn global_cache() -> &'static CelExpressionCache {
    GLOBAL_CACHE.get_or_init(CelExpressionCache::default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_creation() {
        let cache = CelExpressionCache::new(CelCacheConfig::default());
        assert_eq!(cache.entry_count(), 0);
        assert_eq!(cache.max_capacity(), 10_000);
    }

    #[test]
    fn test_get_or_parse_caches_expression() {
        let cache = CelExpressionCache::new(CelCacheConfig::default());

        // First parse
        let expr1 = cache.get_or_parse("x > 5").unwrap();
        cache.run_pending_tasks(); // Ensure insertion is visible
        assert_eq!(cache.entry_count(), 1);

        // Second access should return cached version
        let expr2 = cache.get_or_parse("x > 5").unwrap();
        assert_eq!(cache.entry_count(), 1);

        // Both should be the same Arc (same pointer)
        assert!(Arc::ptr_eq(&expr1, &expr2));
    }

    #[test]
    fn test_different_expressions_cached_separately() {
        let cache = CelExpressionCache::new(CelCacheConfig::default());

        let expr1 = cache.get_or_parse("x > 5").unwrap();
        let expr2 = cache.get_or_parse("y < 10").unwrap();
        cache.run_pending_tasks();

        assert_eq!(cache.entry_count(), 2);
        assert!(!Arc::ptr_eq(&expr1, &expr2));
    }

    #[test]
    fn test_invalid_expression_not_cached() {
        let cache = CelExpressionCache::new(CelCacheConfig::default());

        // Invalid expression should fail
        let result = cache.get_or_parse("invalid ==");
        assert!(result.is_err());

        // Should not be cached
        assert_eq!(cache.entry_count(), 0);
    }

    #[test]
    fn test_invalidate_specific_expression() {
        let cache = CelExpressionCache::new(CelCacheConfig::default());

        cache.get_or_parse("x > 5").unwrap();
        cache.get_or_parse("y < 10").unwrap();
        cache.run_pending_tasks();
        assert_eq!(cache.entry_count(), 2);

        cache.invalidate("x > 5");
        cache.run_pending_tasks();
        assert_eq!(cache.entry_count(), 1);
    }

    #[test]
    fn test_invalidate_all() {
        let cache = CelExpressionCache::new(CelCacheConfig::default());

        cache.get_or_parse("x > 5").unwrap();
        cache.get_or_parse("y < 10").unwrap();
        cache.run_pending_tasks();
        assert_eq!(cache.entry_count(), 2);

        cache.invalidate_all();
        cache.run_pending_tasks();
        assert_eq!(cache.entry_count(), 0);
    }

    #[test]
    fn test_global_cache() {
        let cache = global_cache();

        // Parse through global cache
        let result = cache.get_or_parse("global_test == true");
        assert!(result.is_ok());
    }

    #[test]
    fn test_cache_is_thread_safe() {
        use std::thread;

        let cache = Arc::new(CelExpressionCache::new(CelCacheConfig::default()));

        let handles: Vec<_> = (0..10)
            .map(|i| {
                let cache_clone = Arc::clone(&cache);
                thread::spawn(move || {
                    for j in 0..100 {
                        let expr = format!("x{} > {}", i, j);
                        cache_clone.get_or_parse(&expr).unwrap();
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        cache.run_pending_tasks();
        // All 1000 unique expressions should be cached
        assert_eq!(cache.entry_count(), 1000);
    }

    /// Test that concurrent access to the SAME expression returns identical Arc pointers.
    /// This verifies the race condition handling - all threads should get the same result.
    #[test]
    fn test_concurrent_identical_expression_returns_same_arc() {
        use std::thread;

        let cache = Arc::new(CelExpressionCache::new(CelCacheConfig::default()));
        let expression = "shared_expr > 42";

        // Spawn many threads that all request the same expression simultaneously
        let handles: Vec<_> = (0..100)
            .map(|_| {
                let cache_clone = Arc::clone(&cache);
                thread::spawn(move || cache_clone.get_or_parse(expression).unwrap())
            })
            .collect();

        // Collect all returned Arcs
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // All results must be pointer-equal (same Arc instance)
        let first = &results[0];
        for (i, result) in results.iter().enumerate().skip(1) {
            assert!(
                Arc::ptr_eq(first, result),
                "Thread {} returned different Arc than thread 0 (race condition detected)",
                i
            );
        }

        cache.run_pending_tasks();
        // Only one expression should be cached
        assert_eq!(cache.entry_count(), 1);
    }

    /// Test that the cache enforces max_capacity with LRU eviction.
    #[test]
    fn test_lru_eviction_at_max_capacity() {
        let config = CelCacheConfig {
            max_capacity: 5,
            ttl: Duration::from_secs(3600),
        };
        let cache = CelExpressionCache::new(config);

        // Add 5 expressions (at capacity)
        for i in 0..5 {
            cache.get_or_parse(&format!("x > {}", i)).unwrap();
        }
        cache.run_pending_tasks();
        assert_eq!(cache.entry_count(), 5);

        // Add one more - should trigger eviction
        cache.get_or_parse("x > 100").unwrap();
        cache.run_pending_tasks();

        // Should still be at or below max capacity
        assert!(
            cache.entry_count() <= 5,
            "Cache exceeded max_capacity: {} > 5",
            cache.entry_count()
        );
    }

    /// Test that TTL expiration works.
    #[test]
    fn test_ttl_expiration() {
        let config = CelCacheConfig {
            max_capacity: 100,
            ttl: Duration::from_millis(50), // Very short TTL for testing
        };
        let cache = CelExpressionCache::new(config);

        cache.get_or_parse("x > 5").unwrap();
        cache.run_pending_tasks();
        assert_eq!(cache.entry_count(), 1);

        // Wait for TTL to expire
        std::thread::sleep(Duration::from_millis(100));
        cache.run_pending_tasks();

        // Entry should be expired
        assert_eq!(
            cache.entry_count(),
            0,
            "Entry should have expired after TTL"
        );
    }

    /// Test that max_capacity is correctly reported.
    #[test]
    fn test_max_capacity_getter() {
        let config = CelCacheConfig {
            max_capacity: 500,
            ttl: Duration::from_secs(60),
        };
        let cache = CelExpressionCache::new(config);
        assert_eq!(cache.max_capacity(), 500);
    }

    /// Test that metrics registration doesn't panic.
    ///
    /// Note: This test verifies the metrics registration function works without
    /// requiring a full metrics recorder setup. In production, metrics are
    /// collected by the Prometheus exporter initialized in rsfga-api.
    #[test]
    fn test_metrics_registration_doesnt_panic() {
        // Should not panic even without a recorder installed
        super::register_cel_cache_metrics();
    }

    /// Test that cache operations emit metrics without panicking.
    ///
    /// This test verifies that the metrics macros work correctly even without
    /// a recorder installed. The metrics crate handles this gracefully by
    /// using a no-op recorder.
    #[test]
    fn test_cache_operations_emit_metrics() {
        let cache = CelExpressionCache::new(CelCacheConfig::default());

        // Cache miss (parses expression)
        let _ = cache.get_or_parse("x > 5");
        cache.run_pending_tasks();

        // Cache hit
        let _ = cache.get_or_parse("x > 5");

        // Invalidate specific
        cache.invalidate("x > 5");

        // Invalidate all
        let _ = cache.get_or_parse("y < 10");
        cache.invalidate_all();

        // All operations should complete without panicking
        // Metrics are recorded but discarded without a recorder
    }
}
