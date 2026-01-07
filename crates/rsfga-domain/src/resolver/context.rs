//! Internal traversal context for the graph resolver.

use std::collections::HashSet;
use std::sync::Arc;

/// Internal context for graph traversal.
#[derive(Debug, Clone)]
pub(crate) struct TraversalContext {
    /// Current traversal depth.
    pub(crate) depth: u32,
    /// Visited nodes for cycle detection (object:relation pairs).
    /// Wrapped in Arc for cheap cloning when not mutating.
    pub(crate) visited: Arc<HashSet<String>>,
}

impl TraversalContext {
    pub(crate) fn new() -> Self {
        Self {
            depth: 0,
            visited: Arc::new(HashSet::new()),
        }
    }

    pub(crate) fn increment_depth(&self) -> Self {
        Self {
            depth: self.depth + 1,
            visited: Arc::clone(&self.visited),
        }
    }

    pub(crate) fn with_visited(&self, key: &str) -> Self {
        // Clone the inner HashSet only when adding new entries (copy-on-write)
        let mut new_visited = (*self.visited).clone();
        new_visited.insert(key.to_string());
        Self {
            depth: self.depth,
            visited: Arc::new(new_visited),
        }
    }
}
