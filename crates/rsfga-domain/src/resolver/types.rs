//! Types for the graph resolver.

use std::collections::HashMap;
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
    /// CEL evaluation context variables.
    /// These are passed to condition expressions during evaluation.
    /// Keys are variable names, values are JSON-compatible values.
    pub context: Arc<HashMap<String, serde_json::Value>>,
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
            context: Arc::new(HashMap::new()),
        }
    }

    /// Creates a new CheckRequest with contextual tuples and CEL context.
    pub fn with_context(
        store_id: String,
        user: String,
        relation: String,
        object: String,
        contextual_tuples: Vec<ContextualTuple>,
        context: HashMap<String, serde_json::Value>,
    ) -> Self {
        Self {
            store_id,
            user,
            relation,
            object,
            contextual_tuples: Arc::new(contextual_tuples),
            context: Arc::new(context),
        }
    }
}

/// A contextual tuple for temporary authorization during a check.
#[derive(Debug, Clone)]
pub struct ContextualTuple {
    pub user: String,
    pub relation: String,
    pub object: String,
    /// Optional condition name that must be satisfied for this tuple.
    pub condition_name: Option<String>,
    /// Optional condition context (parameters) as JSON key-value pairs.
    pub condition_context: Option<HashMap<String, serde_json::Value>>,
}

impl ContextualTuple {
    /// Creates a new ContextualTuple without a condition.
    pub fn new(
        user: impl Into<String>,
        relation: impl Into<String>,
        object: impl Into<String>,
    ) -> Self {
        Self {
            user: user.into(),
            relation: relation.into(),
            object: object.into(),
            condition_name: None,
            condition_context: None,
        }
    }

    /// Creates a new ContextualTuple with a condition.
    pub fn with_condition(
        user: impl Into<String>,
        relation: impl Into<String>,
        object: impl Into<String>,
        condition_name: impl Into<String>,
        condition_context: Option<HashMap<String, serde_json::Value>>,
    ) -> Self {
        Self {
            user: user.into(),
            relation: relation.into(),
            object: object.into(),
            condition_name: Some(condition_name.into()),
            condition_context,
        }
    }
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
    /// Optional condition name that must be satisfied for this tuple.
    pub condition_name: Option<String>,
    /// Optional condition context (parameters) as JSON key-value pairs.
    pub condition_context: Option<HashMap<String, serde_json::Value>>,
}

impl StoredTupleRef {
    /// Creates a new StoredTupleRef without a condition.
    pub fn new(
        user_type: impl Into<String>,
        user_id: impl Into<String>,
        user_relation: Option<String>,
    ) -> Self {
        Self {
            user_type: user_type.into(),
            user_id: user_id.into(),
            user_relation,
            condition_name: None,
            condition_context: None,
        }
    }

    /// Creates a new StoredTupleRef with a condition.
    pub fn with_condition(
        user_type: impl Into<String>,
        user_id: impl Into<String>,
        user_relation: Option<String>,
        condition_name: impl Into<String>,
        condition_context: Option<HashMap<String, serde_json::Value>>,
    ) -> Self {
        Self {
            user_type: user_type.into(),
            user_id: user_id.into(),
            user_relation,
            condition_name: Some(condition_name.into()),
            condition_context,
        }
    }
}
