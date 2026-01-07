//! Types for the graph resolver.

use std::sync::Arc;

/// Request for a permission check.
#[derive(Debug, Clone)]
pub struct CheckRequest {
    /// The store ID to check against.
    pub store_id: String,
    /// The user identifier (e.g., "user:alice").
    pub user: String,
    /// The relation to check (e.g., "viewer").
    pub relation: String,
    /// The object identifier (e.g., "document:readme").
    pub object: String,
    /// Contextual tuples to consider during the check.
    /// Wrapped in Arc for cheap cloning during graph traversal.
    pub contextual_tuples: Arc<Vec<ContextualTuple>>,
}

impl CheckRequest {
    /// Creates a new CheckRequest with contextual tuples.
    pub fn new(
        store_id: String,
        user: String,
        relation: String,
        object: String,
        contextual_tuples: Vec<ContextualTuple>,
    ) -> Self {
        Self {
            store_id,
            user,
            relation,
            object,
            contextual_tuples: Arc::new(contextual_tuples),
        }
    }
}

/// A contextual tuple for temporary authorization during a check.
#[derive(Debug, Clone)]
pub struct ContextualTuple {
    pub user: String,
    pub relation: String,
    pub object: String,
}

/// Result of a permission check.
#[derive(Debug, Clone)]
pub struct CheckResult {
    /// Whether the check is allowed.
    pub allowed: bool,
}

/// Reference to a stored tuple for resolver use.
#[derive(Debug, Clone)]
pub struct StoredTupleRef {
    pub user_type: String,
    pub user_id: String,
    pub user_relation: Option<String>,
}
