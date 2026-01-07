//! Configuration for the graph resolver.

use std::time::Duration;

/// Configuration for the graph resolver.
#[derive(Debug, Clone)]
pub struct ResolverConfig {
    /// Maximum depth for graph traversal (matches OpenFGA default of 25).
    pub max_depth: u32,
    /// Timeout for check operations.
    pub timeout: Duration,
}

impl Default for ResolverConfig {
    fn default() -> Self {
        Self {
            max_depth: 25,
            timeout: Duration::from_secs(30),
        }
    }
}
