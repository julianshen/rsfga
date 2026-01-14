//! Core type definitions for the authorization model.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A user identifier (e.g., "user:alice" or "group:eng#member").
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct User(String);

impl User {
    /// Creates a new User from a string, validating the format.
    ///
    /// Valid formats:
    /// - `type:id` (e.g., "user:alice", "group:engineers")
    /// - `type:id#relation` (e.g., "group:eng#member" - userset reference)
    /// - `*` (wildcard, all users)
    /// - `type:*` (type wildcard, e.g., "user:*")
    pub fn new(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        if value.is_empty() {
            return Err("user identifier cannot be empty");
        }

        // Wildcard is valid
        if value == "*" {
            return Ok(Self(value));
        }

        // Validate type:id or type:id#relation format using split_once
        let Some((type_part, rest)) = value.split_once(':') else {
            return Err("user identifier must be in 'type:id' format (e.g., 'user:alice')");
        };

        if type_part.is_empty() {
            return Err("user type cannot be empty");
        }

        // Rest can be: id, *, or id#relation
        if rest.is_empty() {
            return Err("user id cannot be empty");
        }

        // If it contains #, validate the relation part using split_once
        if let Some((id_part, relation_part)) = rest.split_once('#') {
            if id_part.is_empty() {
                return Err("user id cannot be empty");
            }
            if relation_part.is_empty() {
                return Err("relation in userset reference cannot be empty");
            }
        }

        Ok(Self(value))
    }

    /// Returns the user identifier as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns true if this is a wildcard user.
    pub fn is_wildcard(&self) -> bool {
        self.0 == "*" || self.0.ends_with(":*")
    }

    /// Returns true if this is a userset reference (type:id#relation).
    pub fn is_userset_reference(&self) -> bool {
        self.0.contains('#')
    }
}

/// An object identifier (e.g., "document:readme").
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Object {
    /// The type portion (e.g., "document").
    pub object_type: String,
    /// The ID portion (e.g., "readme").
    pub object_id: String,
}

impl Object {
    /// Creates a new Object from type and ID.
    pub fn new(object_type: impl Into<String>, object_id: impl Into<String>) -> Self {
        Self {
            object_type: object_type.into(),
            object_id: object_id.into(),
        }
    }

    /// Parses an object from "type:id" format.
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        let parts: Vec<&str> = value.splitn(2, ':').collect();
        if parts.len() != 2 {
            return Err("object must be in 'type:id' format");
        }
        if parts[0].is_empty() || parts[1].is_empty() {
            return Err("object type and id cannot be empty");
        }
        Ok(Self::new(parts[0], parts[1]))
    }
}

impl fmt::Display for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.object_type, self.object_id)
    }
}

/// A relation name (e.g., "viewer", "editor", "owner").
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Relation(String);

impl Relation {
    /// Creates a new Relation from a string.
    pub fn new(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        if value.is_empty() {
            return Err("relation cannot be empty");
        }
        Ok(Self(value))
    }

    /// Returns the relation as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A tuple representing a relationship (user, relation, object).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Tuple {
    /// The user (subject) of the relationship.
    pub user: String,
    /// The relation between user and object.
    pub relation: String,
    /// The object of the relationship.
    pub object: String,
}

impl Tuple {
    /// Creates a new Tuple, validating all fields are non-empty.
    pub fn new(
        user: impl Into<String>,
        relation: impl Into<String>,
        object: impl Into<String>,
    ) -> Result<Self, &'static str> {
        let user = user.into();
        let relation = relation.into();
        let object = object.into();

        if user.is_empty() {
            return Err("tuple user cannot be empty");
        }
        if relation.is_empty() {
            return Err("tuple relation cannot be empty");
        }
        if object.is_empty() {
            return Err("tuple object cannot be empty");
        }

        Ok(Self {
            user,
            relation,
            object,
        })
    }

    /// Creates a new Tuple without validation (for internal use or when already validated).
    pub fn new_unchecked(
        user: impl Into<String>,
        relation: impl Into<String>,
        object: impl Into<String>,
    ) -> Self {
        Self {
            user: user.into(),
            relation: relation.into(),
            object: object.into(),
        }
    }
}

/// A store that contains authorization models and tuples.
///
/// This type uses `chrono::DateTime<Utc>` for timestamps to ensure consistency
/// with the storage layer (rsfga-storage). Serde serializes timestamps as ISO 8601
/// strings for API compatibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Store {
    /// Unique store identifier.
    pub id: String,
    /// Human-readable store name.
    pub name: String,
    /// When the store was created.
    pub created_at: DateTime<Utc>,
    /// When the store was last updated.
    pub updated_at: DateTime<Utc>,
}

impl Store {
    /// Creates a new Store with the given ID and name.
    /// Sets created_at and updated_at to the current time.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Result<Self, &'static str> {
        let id = id.into();
        let name = name.into();

        if id.is_empty() {
            return Err("store id cannot be empty");
        }
        if name.is_empty() {
            return Err("store name cannot be empty");
        }

        let now = Utc::now();
        Ok(Self {
            id,
            name,
            created_at: now,
            updated_at: now,
        })
    }

    /// Creates a Store with explicit timestamps.
    pub fn with_timestamps(
        id: impl Into<String>,
        name: impl Into<String>,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    ) -> Result<Self, &'static str> {
        let id = id.into();
        let name = name.into();

        if id.is_empty() {
            return Err("store id cannot be empty");
        }
        if name.is_empty() {
            return Err("store name cannot be empty");
        }

        Ok(Self {
            id,
            name,
            created_at,
            updated_at,
        })
    }
}

/// An authorization model defining types and their relations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizationModel {
    /// Unique model identifier.
    pub id: Option<String>,
    /// Schema version (e.g., "1.1").
    pub schema_version: String,
    /// Type definitions in the model.
    pub type_definitions: Vec<TypeDefinition>,
    /// Condition definitions that can be used in relations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
}

impl AuthorizationModel {
    /// Creates a new AuthorizationModel with the given schema version.
    pub fn new(schema_version: impl Into<String>) -> Self {
        Self {
            id: None,
            schema_version: schema_version.into(),
            type_definitions: Vec::new(),
            conditions: Vec::new(),
        }
    }

    /// Creates a new AuthorizationModel with type definitions.
    pub fn with_types(
        schema_version: impl Into<String>,
        type_definitions: Vec<TypeDefinition>,
    ) -> Self {
        Self {
            id: None,
            schema_version: schema_version.into(),
            type_definitions,
            conditions: Vec::new(),
        }
    }

    /// Creates a new AuthorizationModel with type definitions and conditions.
    pub fn with_types_and_conditions(
        schema_version: impl Into<String>,
        type_definitions: Vec<TypeDefinition>,
        conditions: Vec<Condition>,
    ) -> Self {
        Self {
            id: None,
            schema_version: schema_version.into(),
            type_definitions,
            conditions,
        }
    }

    /// Adds a condition to the model.
    pub fn add_condition(&mut self, condition: Condition) {
        self.conditions.push(condition);
    }

    /// Finds a condition by name.
    pub fn find_condition(&self, name: &str) -> Option<&Condition> {
        self.conditions.iter().find(|c| c.name == name)
    }
}

/// A type definition within the authorization model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeDefinition {
    /// The type name (e.g., "document", "folder").
    pub type_name: String,
    /// Relations defined on this type.
    pub relations: Vec<RelationDefinition>,
}

/// A relation definition on a type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationDefinition {
    /// The relation name.
    pub name: String,
    /// Type constraints for direct assignment (e.g., [user], [group#member]).
    pub type_constraints: Vec<TypeConstraint>,
    /// The userset rewrite for this relation.
    pub rewrite: Userset,
}

/// A type constraint for a relation, optionally with a condition.
///
/// # Examples
///
/// - Simple type: `user` → `TypeConstraint { type_name: "user", condition: None }`
/// - With condition: `user with time_bound` → `TypeConstraint { type_name: "user", condition: Some("time_bound") }`
/// - Userset: `group#member` → `TypeConstraint { type_name: "group#member", condition: None }`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeConstraint {
    /// The type reference (e.g., "user", "group#member").
    pub type_name: String,
    /// Optional condition name that must be satisfied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
}

impl TypeConstraint {
    /// Creates a simple type constraint without a condition.
    pub fn new(type_name: impl Into<String>) -> Self {
        Self {
            type_name: type_name.into(),
            condition: None,
        }
    }

    /// Creates a type constraint with a condition.
    pub fn with_condition(type_name: impl Into<String>, condition: impl Into<String>) -> Self {
        Self {
            type_name: type_name.into(),
            condition: Some(condition.into()),
        }
    }

    /// Returns true if this constraint has a condition.
    pub fn has_condition(&self) -> bool {
        self.condition.is_some()
    }
}

impl From<&str> for TypeConstraint {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for TypeConstraint {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

/// Errors that can occur when creating or validating a condition.
///
/// This enum provides structured, type-safe errors for condition validation,
/// enabling programmatic error handling by callers.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ConditionError {
    /// The condition name was empty.
    #[error("condition name cannot be empty")]
    EmptyName,

    /// The condition expression was empty.
    #[error("condition expression cannot be empty")]
    EmptyExpression,

    /// A condition parameter was invalid.
    #[error("invalid parameter: {0}")]
    InvalidParameter(String),
}

/// A condition definition that can be referenced by relation type constraints.
///
/// Conditions enable ABAC (Attribute-Based Access Control) by allowing
/// CEL expressions to be evaluated at check time.
///
/// # Example DSL
///
/// ```text
/// condition time_bound_access(current_time: timestamp, expires_at: timestamp) {
///     current_time < expires_at
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Condition {
    /// The condition name (e.g., "time_bound_access").
    pub name: String,
    /// Parameter definitions for the condition.
    pub parameters: Vec<ConditionParameter>,
    /// The CEL expression to evaluate.
    pub expression: String,
}

impl Condition {
    /// Creates a new Condition with the given name and expression.
    pub fn new(
        name: impl Into<String>,
        expression: impl Into<String>,
    ) -> Result<Self, ConditionError> {
        let name = name.into();
        let expression = expression.into();

        if name.is_empty() {
            return Err(ConditionError::EmptyName);
        }
        if expression.is_empty() {
            return Err(ConditionError::EmptyExpression);
        }

        Ok(Self {
            name,
            parameters: Vec::new(),
            expression,
        })
    }

    /// Creates a new Condition with parameters.
    pub fn with_parameters(
        name: impl Into<String>,
        parameters: Vec<ConditionParameter>,
        expression: impl Into<String>,
    ) -> Result<Self, ConditionError> {
        let name = name.into();
        let expression = expression.into();

        if name.is_empty() {
            return Err(ConditionError::EmptyName);
        }
        if expression.is_empty() {
            return Err(ConditionError::EmptyExpression);
        }

        Ok(Self {
            name,
            parameters,
            expression,
        })
    }
}

/// A parameter definition for a condition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConditionParameter {
    /// The parameter name.
    pub name: String,
    /// The parameter type (e.g., "string", "int", "timestamp", "bool").
    pub type_name: String,
}

impl ConditionParameter {
    /// Creates a new ConditionParameter.
    pub fn new(name: impl Into<String>, type_name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            type_name: type_name.into(),
        }
    }
}

/// A userset defines how a relation is computed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Userset {
    /// Direct assignment (this).
    This,
    /// Computed userset from another relation.
    ComputedUserset { relation: String },
    /// Tuple to userset (relation from parent).
    TupleToUserset {
        tupleset: String,
        computed_userset: String,
    },
    /// Union of multiple usersets.
    Union { children: Vec<Userset> },
    /// Intersection of multiple usersets.
    Intersection { children: Vec<Userset> },
    /// Exclusion (base but not subtract).
    Exclusion {
        base: Box<Userset>,
        subtract: Box<Userset>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== User Tests ==========

    #[test]
    fn test_user_creation() {
        let user = User::new("user:alice").unwrap();
        assert_eq!(user.as_str(), "user:alice");
    }

    #[test]
    fn test_user_empty_fails() {
        let result = User::new("");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "user identifier cannot be empty");
    }

    #[test]
    fn test_user_rejects_invalid_format_missing_type_prefix() {
        // Missing type:id format
        let result = User::new("alice");
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "user identifier must be in 'type:id' format (e.g., 'user:alice')"
        );

        // Missing type prefix
        let result = User::new("justanid");
        assert!(result.is_err());
    }

    #[test]
    fn test_user_rejects_empty_type() {
        let result = User::new(":alice");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "user type cannot be empty");
    }

    #[test]
    fn test_user_rejects_empty_id() {
        let result = User::new("user:");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "user id cannot be empty");
    }

    #[test]
    fn test_user_accepts_wildcard() {
        let user = User::new("*").unwrap();
        assert!(user.is_wildcard());
        assert_eq!(user.as_str(), "*");
    }

    #[test]
    fn test_user_accepts_type_wildcard() {
        let user = User::new("user:*").unwrap();
        assert!(user.is_wildcard());
        assert_eq!(user.as_str(), "user:*");
    }

    #[test]
    fn test_user_accepts_userset_reference() {
        let user = User::new("group:eng#member").unwrap();
        assert!(user.is_userset_reference());
        assert!(!user.is_wildcard());
        assert_eq!(user.as_str(), "group:eng#member");
    }

    #[test]
    fn test_user_rejects_empty_relation_in_userset() {
        let result = User::new("group:eng#");
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "relation in userset reference cannot be empty"
        );
    }

    #[test]
    fn test_user_rejects_empty_id_in_userset() {
        let result = User::new("group:#member");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "user id cannot be empty");
    }

    // ========== Object Tests ==========

    #[test]
    fn test_object_parse() {
        let obj = Object::parse("document:readme").unwrap();
        assert_eq!(obj.object_type, "document");
        assert_eq!(obj.object_id, "readme");
    }

    #[test]
    fn test_object_invalid_format() {
        assert!(Object::parse("invalid").is_err());
        assert!(Object::parse(":id").is_err());
        assert!(Object::parse("type:").is_err());
    }

    #[test]
    fn test_object_display() {
        let obj = Object::new("document", "readme");
        assert_eq!(obj.to_string(), "document:readme");
    }

    // ========== Relation Tests ==========

    #[test]
    fn test_relation_creation() {
        let rel = Relation::new("viewer").unwrap();
        assert_eq!(rel.as_str(), "viewer");
    }

    #[test]
    fn test_relation_empty_fails() {
        let result = Relation::new("");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "relation cannot be empty");
    }

    // ========== Tuple Tests ==========

    #[test]
    fn test_tuple_creation() {
        let tuple = Tuple::new("user:alice", "viewer", "document:readme").unwrap();
        assert_eq!(tuple.user, "user:alice");
        assert_eq!(tuple.relation, "viewer");
        assert_eq!(tuple.object, "document:readme");
    }

    #[test]
    fn test_tuple_validates_all_fields_non_empty() {
        // Empty user
        let result = Tuple::new("", "viewer", "document:readme");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "tuple user cannot be empty");

        // Empty relation
        let result = Tuple::new("user:alice", "", "document:readme");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "tuple relation cannot be empty");

        // Empty object
        let result = Tuple::new("user:alice", "viewer", "");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "tuple object cannot be empty");
    }

    #[test]
    fn test_tuple_unchecked_creation() {
        // unchecked doesn't validate
        let tuple = Tuple::new_unchecked("", "", "");
        assert_eq!(tuple.user, "");
    }

    // ========== Store Tests ==========

    #[test]
    fn test_store_creation_with_unique_id() {
        let before = Utc::now();
        let store = Store::new("store-123", "My Store").unwrap();
        let after = Utc::now();

        assert_eq!(store.id, "store-123");
        assert_eq!(store.name, "My Store");
        // Timestamps should be set to around the current time
        assert!(store.created_at >= before && store.created_at <= after);
        assert!(store.updated_at >= before && store.updated_at <= after);
        // created_at and updated_at should be the same for a new store
        assert_eq!(store.created_at, store.updated_at);
    }

    #[test]
    fn test_store_rejects_empty_id() {
        let result = Store::new("", "My Store");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "store id cannot be empty");
    }

    #[test]
    fn test_store_rejects_empty_name() {
        let result = Store::new("store-123", "");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "store name cannot be empty");
    }

    #[test]
    fn test_store_with_timestamps() {
        let created = "2024-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let updated = "2024-01-02T00:00:00Z".parse::<DateTime<Utc>>().unwrap();

        let store = Store::with_timestamps("store-123", "My Store", created, updated).unwrap();
        assert_eq!(store.id, "store-123");
        assert_eq!(store.created_at, created);
        assert_eq!(store.updated_at, updated);
    }

    // ========== AuthorizationModel Tests ==========

    #[test]
    fn test_authorization_model_creation_with_schema_version() {
        let model = AuthorizationModel::new("1.1");
        assert_eq!(model.schema_version, "1.1");
        assert!(model.type_definitions.is_empty());
        assert!(model.id.is_none());
    }

    #[test]
    fn test_authorization_model_with_type_definitions() {
        let type_def = TypeDefinition {
            type_name: "user".to_string(),
            relations: vec![],
        };
        let model = AuthorizationModel::with_types("1.1", vec![type_def]);
        assert_eq!(model.schema_version, "1.1");
        assert_eq!(model.type_definitions.len(), 1);
        assert_eq!(model.type_definitions[0].type_name, "user");
    }

    // ========== Condition Tests (Section 3: Condition Model Integration) ==========

    #[test]
    fn test_authorization_model_can_have_conditions() {
        // Test: AuthorizationModel can have conditions
        // Create a model with a time-based condition for ABAC
        let condition = Condition::with_parameters(
            "time_bound_access",
            vec![
                ConditionParameter::new("current_time", "timestamp"),
                ConditionParameter::new("expires_at", "timestamp"),
            ],
            "current_time < expires_at",
        )
        .unwrap();

        let model = AuthorizationModel::with_types_and_conditions(
            "1.1",
            vec![TypeDefinition {
                type_name: "user".to_string(),
                relations: vec![],
            }],
            vec![condition],
        );

        assert_eq!(model.conditions.len(), 1);
        assert_eq!(model.conditions[0].name, "time_bound_access");
        assert_eq!(model.conditions[0].parameters.len(), 2);
        assert_eq!(model.conditions[0].expression, "current_time < expires_at");

        // Test find_condition helper
        let found = model.find_condition("time_bound_access");
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "time_bound_access");

        // Test condition not found
        let not_found = model.find_condition("nonexistent");
        assert!(not_found.is_none());
    }

    #[test]
    fn test_condition_can_be_attached_to_relation_definition() {
        // Test: Condition can be attached to relation definition via TypeConstraint
        // OpenFGA allows conditions on type constraints, e.g., `[user with time_bound_access]`
        let type_constraint = TypeConstraint::with_condition("user", "time_bound_access");

        assert_eq!(type_constraint.type_name, "user");
        assert_eq!(
            type_constraint.condition,
            Some("time_bound_access".to_string())
        );
        assert!(type_constraint.has_condition());

        // Create a relation definition with a conditional type constraint
        let relation_def = RelationDefinition {
            name: "viewer".to_string(),
            type_constraints: vec![
                TypeConstraint::new("user"), // Direct user without condition
                type_constraint,             // User with time-bound condition
            ],
            rewrite: Userset::This,
        };

        assert_eq!(relation_def.type_constraints.len(), 2);
        assert!(!relation_def.type_constraints[0].has_condition());
        assert!(relation_def.type_constraints[1].has_condition());
    }

    #[test]
    fn test_condition_rejects_empty_name() {
        let result = Condition::new("", "expression");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), ConditionError::EmptyName);
    }

    #[test]
    fn test_condition_rejects_empty_expression() {
        let result = Condition::new("condition_name", "");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), ConditionError::EmptyExpression);
    }

    #[test]
    fn test_add_condition_to_model() {
        let mut model = AuthorizationModel::new("1.1");
        assert!(model.conditions.is_empty());

        let condition = Condition::new("simple_check", "true").unwrap();
        model.add_condition(condition);

        assert_eq!(model.conditions.len(), 1);
        assert_eq!(model.conditions[0].name, "simple_check");
    }

    #[test]
    fn test_can_serialize_deserialize_model_with_conditions() {
        // Test: Can serialize/deserialize model with conditions
        // Verify JSON roundtrip preserves all condition data
        let condition = Condition::with_parameters(
            "department_check",
            vec![
                ConditionParameter::new("user_dept", "string"),
                ConditionParameter::new("required_dept", "string"),
            ],
            "user_dept == required_dept",
        )
        .unwrap();

        let model = AuthorizationModel::with_types_and_conditions(
            "1.1",
            vec![TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec![TypeConstraint::with_condition(
                        "user",
                        "department_check",
                    )],
                    rewrite: Userset::This,
                }],
            }],
            vec![condition],
        );

        // Serialize to JSON
        let json = serde_json::to_string(&model).expect("serialize should succeed");

        // Verify condition is in the JSON
        assert!(json.contains("department_check"));
        assert!(json.contains("user_dept == required_dept"));

        // Deserialize back
        let deserialized: AuthorizationModel =
            serde_json::from_str(&json).expect("deserialize should succeed");

        // Verify roundtrip
        assert_eq!(deserialized.conditions.len(), 1);
        assert_eq!(deserialized.conditions[0].name, "department_check");
        assert_eq!(deserialized.conditions[0].parameters.len(), 2);
        assert_eq!(
            deserialized.conditions[0].expression,
            "user_dept == required_dept"
        );

        // Verify type constraint condition is preserved
        let type_def = &deserialized.type_definitions[0];
        let rel_def = &type_def.relations[0];
        assert!(rel_def.type_constraints[0].has_condition());
        assert_eq!(
            rel_def.type_constraints[0].condition,
            Some("department_check".to_string())
        );
    }

    #[test]
    fn test_type_constraint_from_str() {
        // Test From<&str> and From<String> impls
        let tc1: TypeConstraint = "user".into();
        assert_eq!(tc1.type_name, "user");
        assert!(!tc1.has_condition());

        let tc2: TypeConstraint = String::from("group#member").into();
        assert_eq!(tc2.type_name, "group#member");
        assert!(!tc2.has_condition());
    }

    #[test]
    fn test_condition_parameter_creation() {
        let param = ConditionParameter::new("expires_at", "timestamp");
        assert_eq!(param.name, "expires_at");
        assert_eq!(param.type_name, "timestamp");
    }

    #[test]
    fn test_model_without_conditions_omits_field_in_json() {
        // Verify serde skip_serializing_if works for empty conditions
        let model = AuthorizationModel::new("1.1");
        let json = serde_json::to_string(&model).expect("serialize should succeed");

        // The "conditions" field should not appear when empty
        assert!(!json.contains("conditions"));
    }

    #[test]
    fn test_condition_error_display() {
        // Test that ConditionError Display works correctly
        assert_eq!(
            ConditionError::EmptyName.to_string(),
            "condition name cannot be empty"
        );
        assert_eq!(
            ConditionError::EmptyExpression.to_string(),
            "condition expression cannot be empty"
        );
        assert_eq!(
            ConditionError::InvalidParameter("foo".to_string()).to_string(),
            "invalid parameter: foo"
        );
    }

    #[test]
    fn test_condition_error_is_eq() {
        // Test that ConditionError implements PartialEq correctly
        assert_eq!(ConditionError::EmptyName, ConditionError::EmptyName);
        assert_ne!(ConditionError::EmptyName, ConditionError::EmptyExpression);
    }
}
