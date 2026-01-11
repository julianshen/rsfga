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
//! The cache uses DashMap for lock-free concurrent access.

use dashmap::DashMap;
use std::sync::Arc;
use std::time::Duration;

use super::expression::CelExpression;
use super::CelResult;

/// Configuration for the CEL expression cache.
///
/// # ⚠️ Implementation Status
///
/// **Note**: `max_capacity` and `ttl` are reserved for future use with bounded
/// caching (e.g., Moka LRU). The current DashMap implementation does **not**
/// enforce these limits. For cache invalidation, use [`CelExpressionCache::invalidate_all`]
/// when authorization models are updated.
#[derive(Debug, Clone)]
pub struct CelCacheConfig {
    /// Maximum number of expressions to cache.
    ///
    /// **⚠️ Not currently enforced.** Reserved for future LRU implementation.
    /// Default: 10,000 (expressions are small, typically <1KB each)
    pub max_capacity: u64,
    /// Time-to-live for cached expressions.
    ///
    /// **⚠️ Not currently enforced.** Reserved for future TTL-based eviction.
    /// Use [`CelExpressionCache::invalidate_all`] to clear cache on model updates.
    /// Default: 1 hour
    pub ttl: Duration,
}

impl Default for CelCacheConfig {
    fn default() -> Self {
        Self {
            // Not enforced - reserved for future LRU implementation
            max_capacity: 10_000,
            // Not enforced - reserved for future TTL eviction
            ttl: Duration::from_secs(3600),
        }
    }
}

/// Thread-safe cache for parsed CEL expressions.
///
/// This cache stores compiled CEL programs keyed by their source expression string.
/// Since `CelExpression` contains an `Arc<Program>`, cloning cached entries is cheap.
///
/// # Implementation Note
///
/// Uses DashMap for simple, lock-free concurrent access. For production workloads
/// with millions of unique expressions, consider using Moka with LRU eviction.
/// Current implementation doesn't enforce max_capacity or TTL - suitable for
/// typical authorization models with <10K unique conditions.
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
    cache: DashMap<String, Arc<CelExpression>>,
    #[allow(dead_code)]
    config: CelCacheConfig,
}

impl CelExpressionCache {
    /// Creates a new CEL expression cache with the given configuration.
    pub fn new(config: CelCacheConfig) -> Self {
        Self {
            cache: DashMap::new(),
            config,
        }
    }

    /// Gets a cached expression or parses and caches it.
    ///
    /// This is the main entry point for cached expression access.
    /// If the expression is already cached, returns the cached version.
    /// Otherwise, parses the expression, caches it, and returns it.
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
        // Try to get from cache first (fast path - no lock contention)
        if let Some(cached) = self.cache.get(expression) {
            return Ok(Arc::clone(cached.value()));
        }

        // Slow path: Use the entry API to ensure that parsing and insertion are
        // atomic, preventing race conditions where multiple threads could parse
        // the same expression and return different Arc instances.
        use dashmap::mapref::entry::Entry;
        match self.cache.entry(expression.to_string()) {
            Entry::Occupied(entry) => {
                // Another thread inserted the expression while we were checking.
                // Return the existing, cached Arc.
                Ok(Arc::clone(entry.get()))
            }
            Entry::Vacant(entry) => {
                // The entry is vacant, and we hold the lock.
                // Parse the expression and insert it into the cache.
                let parsed = CelExpression::parse(expression)?;
                let arc_expr = Arc::new(parsed);
                entry.insert(Arc::clone(&arc_expr));
                Ok(arc_expr)
            }
        }
    }

    /// Returns the number of cached expressions.
    pub fn entry_count(&self) -> usize {
        self.cache.len()
    }

    /// Invalidates all cached expressions.
    ///
    /// This should be called when authorization models are updated,
    /// as condition expressions may have changed.
    pub fn invalidate_all(&self) {
        self.cache.clear();
    }

    /// Invalidates a specific expression from the cache.
    pub fn invalidate(&self, expression: &str) {
        self.cache.remove(expression);
    }
}

impl Default for CelExpressionCache {
    fn default() -> Self {
        Self::new(CelCacheConfig::default())
    }
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
    }

    #[test]
    fn test_get_or_parse_caches_expression() {
        let cache = CelExpressionCache::new(CelCacheConfig::default());

        // First parse
        let expr1 = cache.get_or_parse("x > 5").unwrap();
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
        assert_eq!(cache.entry_count(), 2);

        cache.invalidate("x > 5");
        assert_eq!(cache.entry_count(), 1);
    }

    #[test]
    fn test_invalidate_all() {
        let cache = CelExpressionCache::new(CelCacheConfig::default());

        cache.get_or_parse("x > 5").unwrap();
        cache.get_or_parse("y < 10").unwrap();
        assert_eq!(cache.entry_count(), 2);

        cache.invalidate_all();
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

        // All 1000 unique expressions should be cached
        assert_eq!(cache.entry_count(), 1000);
    }

    /// Test that concurrent access to the SAME expression returns identical Arc pointers.
    /// This verifies the race condition fix - all threads must get the same cached instance.
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

        // Only one expression should be cached
        assert_eq!(cache.entry_count(), 1);
    }
}
