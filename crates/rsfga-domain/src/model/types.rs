//! Core type definitions for the authorization model.

use std::fmt;

use serde::{Deserialize, Serialize};

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

        // Must contain colon for type:id format
        if !value.contains(':') {
            return Err("user identifier must be in 'type:id' format (e.g., 'user:alice')");
        }

        // Validate type:id or type:id#relation format
        let colon_pos = value.find(':').unwrap();
        let type_part = &value[..colon_pos];
        let rest = &value[colon_pos + 1..];

        if type_part.is_empty() {
            return Err("user type cannot be empty");
        }

        // Rest can be: id, *, or id#relation
        if rest.is_empty() {
            return Err("user id cannot be empty");
        }

        // If it contains #, validate the relation part
        if let Some(hash_pos) = rest.find('#') {
            let id_part = &rest[..hash_pos];
            let relation_part = &rest[hash_pos + 1..];
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Store {
    /// Unique store identifier.
    pub id: String,
    /// Human-readable store name.
    pub name: String,
    /// When the store was created (ISO 8601 timestamp).
    pub created_at: Option<String>,
    /// When the store was last updated (ISO 8601 timestamp).
    pub updated_at: Option<String>,
}

impl Store {
    /// Creates a new Store with the given ID and name.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Result<Self, &'static str> {
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
            created_at: None,
            updated_at: None,
        })
    }

    /// Creates a Store with timestamps.
    pub fn with_timestamps(
        id: impl Into<String>,
        name: impl Into<String>,
        created_at: impl Into<String>,
        updated_at: impl Into<String>,
    ) -> Result<Self, &'static str> {
        let mut store = Self::new(id, name)?;
        store.created_at = Some(created_at.into());
        store.updated_at = Some(updated_at.into());
        Ok(store)
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
}

impl AuthorizationModel {
    /// Creates a new AuthorizationModel with the given schema version.
    pub fn new(schema_version: impl Into<String>) -> Self {
        Self {
            id: None,
            schema_version: schema_version.into(),
            type_definitions: Vec::new(),
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
        }
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
    /// The userset rewrite for this relation.
    pub rewrite: Userset,
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
        let store = Store::new("store-123", "My Store").unwrap();
        assert_eq!(store.id, "store-123");
        assert_eq!(store.name, "My Store");
        assert!(store.created_at.is_none());
        assert!(store.updated_at.is_none());
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
        let store = Store::with_timestamps(
            "store-123",
            "My Store",
            "2024-01-01T00:00:00Z",
            "2024-01-02T00:00:00Z",
        )
        .unwrap();
        assert_eq!(store.id, "store-123");
        assert_eq!(store.created_at, Some("2024-01-01T00:00:00Z".to_string()));
        assert_eq!(store.updated_at, Some("2024-01-02T00:00:00Z".to_string()));
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
}
