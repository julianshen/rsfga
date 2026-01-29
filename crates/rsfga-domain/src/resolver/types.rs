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
    /// Optional authorization model ID to use for the check.
    /// If not provided, the latest model for the store is used.
    pub authorization_model_id: Option<String>,
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
            authorization_model_id: None,
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
            authorization_model_id: None,
        }
    }

    /// Creates a new CheckRequest with all options including authorization model ID.
    pub fn with_model_id(
        store_id: String,
        user: String,
        relation: String,
        object: String,
        contextual_tuples: Vec<ContextualTuple>,
        context: HashMap<String, serde_json::Value>,
        authorization_model_id: Option<String>,
    ) -> Self {
        Self {
            store_id,
            user,
            relation,
            object,
            contextual_tuples: Arc::new(contextual_tuples),
            context: Arc::new(context),
            authorization_model_id,
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

/// Information about an object-relation tuple for reverse lookup.
/// Used by `get_objects_for_user` to return object IDs with condition info.
#[derive(Debug, Clone)]
pub struct ObjectTupleInfo {
    /// The object ID (e.g., "doc1" not "document:doc1").
    pub object_id: String,
    /// The relation name.
    pub relation: String,
    /// Optional condition name that must be satisfied for this tuple.
    pub condition_name: Option<String>,
    /// Optional condition context (parameters) as JSON key-value pairs.
    pub condition_context: Option<HashMap<String, serde_json::Value>>,
}

impl ObjectTupleInfo {
    /// Creates a new ObjectTupleInfo without a condition.
    pub fn new(object_id: impl Into<String>, relation: impl Into<String>) -> Self {
        Self {
            object_id: object_id.into(),
            relation: relation.into(),
            condition_name: None,
            condition_context: None,
        }
    }

    /// Creates a new ObjectTupleInfo with a condition.
    pub fn with_condition(
        object_id: impl Into<String>,
        relation: impl Into<String>,
        condition_name: impl Into<String>,
        condition_context: Option<HashMap<String, serde_json::Value>>,
    ) -> Self {
        Self {
            object_id: object_id.into(),
            relation: relation.into(),
            condition_name: Some(condition_name.into()),
            condition_context,
        }
    }
}

// ============================================================
// Expand API Types
// ============================================================

/// Request for expanding a relation tree.
#[derive(Debug, Clone)]
pub struct ExpandRequest {
    /// The store ID to expand against.
    pub store_id: String,
    /// The relation to expand (e.g., "viewer").
    pub relation: String,
    /// The object to expand (e.g., "document:readme").
    pub object: String,
}

impl ExpandRequest {
    /// Creates a new ExpandRequest.
    pub fn new(
        store_id: impl Into<String>,
        relation: impl Into<String>,
        object: impl Into<String>,
    ) -> Self {
        Self {
            store_id: store_id.into(),
            relation: relation.into(),
            object: object.into(),
        }
    }
}

// ============================================================
// ListObjects API Types
// ============================================================

/// Request for listing objects accessible to a user.
#[derive(Debug, Clone)]
pub struct ListObjectsRequest {
    /// The store ID to query.
    pub store_id: String,
    /// The user to check permissions for.
    pub user: String,
    /// The relation to check (e.g., "viewer").
    pub relation: String,
    /// The object type to list (e.g., "document").
    pub object_type: String,
    /// Contextual tuples to consider during the check.
    pub contextual_tuples: Arc<Vec<ContextualTuple>>,
    /// CEL evaluation context variables.
    pub context: Arc<HashMap<String, serde_json::Value>>,
}

impl ListObjectsRequest {
    /// Creates a new ListObjectsRequest without contextual tuples or context.
    pub fn new(
        store_id: impl Into<String>,
        user: impl Into<String>,
        relation: impl Into<String>,
        object_type: impl Into<String>,
    ) -> Self {
        Self {
            store_id: store_id.into(),
            user: user.into(),
            relation: relation.into(),
            object_type: object_type.into(),
            contextual_tuples: Arc::new(Vec::new()),
            context: Arc::new(HashMap::new()),
        }
    }

    /// Creates a new ListObjectsRequest with contextual tuples and context.
    pub fn with_context(
        store_id: impl Into<String>,
        user: impl Into<String>,
        relation: impl Into<String>,
        object_type: impl Into<String>,
        contextual_tuples: Vec<ContextualTuple>,
        context: HashMap<String, serde_json::Value>,
    ) -> Self {
        Self {
            store_id: store_id.into(),
            user: user.into(),
            relation: relation.into(),
            object_type: object_type.into(),
            contextual_tuples: Arc::new(contextual_tuples),
            context: Arc::new(context),
        }
    }
}

/// Result of expanding a relation tree.
#[derive(Debug, Clone)]
pub struct ExpandResult {
    /// The expansion tree showing how users relate to the object.
    pub tree: UsersetTree,
}

/// A tree structure representing the expansion of a relation.
#[derive(Debug, Clone)]
pub struct UsersetTree {
    /// The root node of the expansion tree.
    pub root: ExpandNode,
}

/// A node in the expansion tree.
#[derive(Debug, Clone)]
pub enum ExpandNode {
    /// A leaf node containing direct users.
    Leaf(ExpandLeaf),
    /// A union of child nodes (any child grants access).
    Union {
        /// Name of this union node.
        name: String,
        /// Child nodes in the union.
        nodes: Vec<ExpandNode>,
    },
    /// An intersection of child nodes (all children must grant access).
    Intersection {
        /// Name of this intersection node.
        name: String,
        /// Child nodes in the intersection.
        nodes: Vec<ExpandNode>,
    },
    /// A difference (exclusion) of nodes (base minus subtract).
    Difference {
        /// Name of this difference node.
        name: String,
        /// The base node.
        base: Box<ExpandNode>,
        /// The node to subtract from base.
        subtract: Box<ExpandNode>,
    },
}

impl ExpandNode {
    /// Returns the name of this node.
    pub fn name(&self) -> &str {
        match self {
            ExpandNode::Leaf(leaf) => &leaf.name,
            ExpandNode::Union { name, .. } => name,
            ExpandNode::Intersection { name, .. } => name,
            ExpandNode::Difference { name, .. } => name,
        }
    }
}

/// A leaf node in the expansion tree.
#[derive(Debug, Clone)]
pub struct ExpandLeaf {
    /// Name of this leaf node.
    pub name: String,
    /// The type of leaf content.
    pub value: ExpandLeafValue,
}

/// The value of a leaf node.
#[derive(Debug, Clone)]
pub enum ExpandLeafValue {
    /// Direct users who have the relation.
    Users(Vec<String>),
    /// A computed userset reference.
    Computed { userset: String },
    /// A tuple-to-userset reference.
    TupleToUserset {
        tupleset: String,
        computed_userset: String,
    },
}

/// Result of listing objects accessible to a user.
#[derive(Debug, Clone)]
pub struct ListObjectsResult {
    /// Object IDs that the user has the specified relation to.
    /// Format: "type:id" (e.g., "document:readme")
    pub objects: Vec<String>,
    /// Whether the results were truncated due to limits.
    pub truncated: bool,
}

// ============================================================
// ListUsers API Types
// ============================================================

/// Request for listing users with a specific relation to an object.
/// This is the inverse of ListObjects.
#[derive(Debug, Clone)]
pub struct ListUsersRequest {
    /// The store ID to query.
    pub store_id: String,
    /// The object to check permissions for (type:id format).
    pub object: String,
    /// The relation to check (e.g., "viewer").
    pub relation: String,
    /// Filter for user types to return.
    pub user_filters: Vec<UserFilter>,
    /// Contextual tuples to consider during the check.
    pub contextual_tuples: Arc<Vec<ContextualTuple>>,
    /// CEL evaluation context variables.
    pub context: Arc<HashMap<String, serde_json::Value>>,
}

impl ListUsersRequest {
    /// Creates a new ListUsersRequest without contextual tuples or context.
    pub fn new(
        store_id: impl Into<String>,
        object: impl Into<String>,
        relation: impl Into<String>,
        user_filters: Vec<UserFilter>,
    ) -> Self {
        Self {
            store_id: store_id.into(),
            object: object.into(),
            relation: relation.into(),
            user_filters,
            contextual_tuples: Arc::new(Vec::new()),
            context: Arc::new(HashMap::new()),
        }
    }

    /// Creates a new ListUsersRequest with contextual tuples and context.
    pub fn with_context(
        store_id: impl Into<String>,
        object: impl Into<String>,
        relation: impl Into<String>,
        user_filters: Vec<UserFilter>,
        contextual_tuples: Vec<ContextualTuple>,
        context: HashMap<String, serde_json::Value>,
    ) -> Self {
        Self {
            store_id: store_id.into(),
            object: object.into(),
            relation: relation.into(),
            user_filters,
            contextual_tuples: Arc::new(contextual_tuples),
            context: Arc::new(context),
        }
    }
}

/// Filter for user types in ListUsers requests.
#[derive(Debug, Clone)]
pub struct UserFilter {
    /// The type to filter for (e.g., "user", "group").
    pub type_name: String,
    /// Optional relation for userset filters (e.g., "member" for "group#member").
    pub relation: Option<String>,
}

impl UserFilter {
    /// Creates a new UserFilter for a direct type.
    pub fn new(type_name: impl Into<String>) -> Self {
        Self {
            type_name: type_name.into(),
            relation: None,
        }
    }

    /// Creates a new UserFilter for a userset type (e.g., group#member).
    pub fn with_relation(type_name: impl Into<String>, relation: impl Into<String>) -> Self {
        Self {
            type_name: type_name.into(),
            relation: Some(relation.into()),
        }
    }
}

/// A user result in ListUsers response.
/// Matches OpenFGA's response format with object/userset/wildcard variants.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum UserResult {
    /// A direct user object (e.g., user:alice).
    Object {
        /// The type of the user (e.g., "user").
        user_type: String,
        /// The ID of the user (e.g., "alice").
        user_id: String,
    },
    /// A userset reference (e.g., group:engineering#member).
    Userset {
        /// The type of the userset (e.g., "group").
        userset_type: String,
        /// The ID of the userset (e.g., "engineering").
        userset_id: String,
        /// The relation on the userset (e.g., "member").
        relation: String,
    },
    /// A wildcard match (e.g., user:*).
    Wildcard {
        /// The type of the wildcard (e.g., "user").
        wildcard_type: String,
    },
}

impl UserResult {
    /// Creates a new Object variant.
    pub fn object(user_type: impl Into<String>, user_id: impl Into<String>) -> Self {
        Self::Object {
            user_type: user_type.into(),
            user_id: user_id.into(),
        }
    }

    /// Creates a new Userset variant.
    pub fn userset(
        userset_type: impl Into<String>,
        userset_id: impl Into<String>,
        relation: impl Into<String>,
    ) -> Self {
        Self::Userset {
            userset_type: userset_type.into(),
            userset_id: userset_id.into(),
            relation: relation.into(),
        }
    }

    /// Creates a new Wildcard variant.
    pub fn wildcard(wildcard_type: impl Into<String>) -> Self {
        Self::Wildcard {
            wildcard_type: wildcard_type.into(),
        }
    }
}

/// Result of listing users with relation to an object.
#[derive(Debug, Clone)]
pub struct ListUsersResult {
    /// Users that have the specified relation to the object.
    pub users: Vec<UserResult>,
    /// Users excluded from access (for exclusion relations).
    pub excluded_users: Vec<UserResult>,
    /// Whether the results were truncated due to max_results limit.
    pub truncated: bool,
}
