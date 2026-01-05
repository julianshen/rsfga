//! Core type definitions for the authorization model.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A user identifier (e.g., "user:alice" or "group:eng#member").
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct User(String);

impl User {
    /// Creates a new User from a string, validating the format.
    pub fn new(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        if value.is_empty() {
            return Err("user identifier cannot be empty");
        }
        Ok(Self(value))
    }

    /// Returns the user identifier as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
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
    /// Creates a new Tuple.
    pub fn new(
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

/// An authorization model defining types and their relations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizationModel {
    /// Schema version (e.g., "1.1").
    pub schema_version: String,
    /// Type definitions in the model.
    pub type_definitions: Vec<TypeDefinition>,
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

    #[test]
    fn test_user_creation() {
        let user = User::new("user:alice").unwrap();
        assert_eq!(user.as_str(), "user:alice");
    }

    #[test]
    fn test_user_empty_fails() {
        assert!(User::new("").is_err());
    }

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
    fn test_relation_creation() {
        let rel = Relation::new("viewer").unwrap();
        assert_eq!(rel.as_str(), "viewer");
    }

    #[test]
    fn test_tuple_creation() {
        let tuple = Tuple::new("user:alice", "viewer", "document:readme");
        assert_eq!(tuple.user, "user:alice");
        assert_eq!(tuple.relation, "viewer");
        assert_eq!(tuple.object, "document:readme");
    }
}
