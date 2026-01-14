//! Configuration for the graph resolver.

use std::sync::Arc;
use std::time::Duration;

use crate::cache::CheckCache;

/// Configuration for the graph resolver.
#[derive(Debug, Clone)]
pub struct ResolverConfig {
    /// Maximum depth for graph traversal (matches OpenFGA default of 25).
    pub max_depth: u32,
    /// Timeout for check operations.
    pub timeout: Duration,
    /// Optional check result cache for performance optimization.
    ///
    /// When enabled, the resolver will:
    /// 1. Check the cache before performing graph traversal
    /// 2. Store successful results in the cache after traversal
    ///
    /// Caching is skipped when contextual tuples are provided, since
    /// these are temporary and specific to a single request.
    ///
    /// # Performance Impact
    ///
    /// Expected 10-100x improvement for repeated checks (ADR-007).
    /// Cache staleness window target: <100ms p99.
    pub cache: Option<Arc<CheckCache>>,
}

impl Default for ResolverConfig {
    fn default() -> Self {
        Self {
            max_depth: 25,
            timeout: Duration::from_secs(30),
            cache: None,
        }
    }
}

impl ResolverConfig {
    /// Creates a new configuration with caching enabled.
    pub fn with_cache(mut self, cache: Arc<CheckCache>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Creates a new configuration with the specified max depth.
    pub fn with_max_depth(mut self, max_depth: u32) -> Self {
        self.max_depth = max_depth;
        self
    }

    /// Creates a new configuration with the specified timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}
