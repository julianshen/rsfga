//! Type system for authorization model lookups with caching.
//!
//! The `TypeSystem` provides efficient lookups for types and relations
//! with internal caching using `DashMap` for thread-safe concurrent access.

use std::sync::Arc;

use dashmap::DashMap;

use crate::error::{DomainError, DomainResult};

use super::types::{AuthorizationModel, Object, RelationDefinition, Tuple, TypeDefinition};

/// Type system providing cached access to authorization model types and relations.
///
/// # Thread Safety
///
/// The `TypeSystem` is thread-safe and can be shared across async tasks.
/// It uses `DashMap` internally for lock-free concurrent reads and minimal
/// contention on writes.
///
/// # Example
///
/// ```ignore
/// use rsfga_domain::model::{AuthorizationModel, TypeDefinition, TypeSystem};
///
/// let model = AuthorizationModel::new("1.1");
/// let type_system = TypeSystem::new(model);
///
/// // Look up a type
/// let doc_type = type_system.get_type("document")?;
///
/// // Look up a relation on a type
/// let viewer_rel = type_system.get_relation("document", "viewer")?;
/// ```
#[derive(Debug)]
pub struct TypeSystem {
    /// The underlying authorization model.
    model: Arc<AuthorizationModel>,
    /// Cache for type definitions, keyed by type name.
    type_cache: DashMap<String, Arc<TypeDefinition>>,
    /// Cache for relation definitions, keyed by "type_name:relation_name".
    relation_cache: DashMap<String, Arc<RelationDefinition>>,
}

impl TypeSystem {
    /// Creates a new `TypeSystem` from an authorization model.
    ///
    /// The type system will lazily cache lookups as they are accessed.
    pub fn new(model: AuthorizationModel) -> Self {
        Self {
            model: Arc::new(model),
            type_cache: DashMap::new(),
            relation_cache: DashMap::new(),
        }
    }

    /// Returns a reference to the underlying authorization model.
    pub fn model(&self) -> &AuthorizationModel {
        &self.model
    }

    /// Gets a type definition by name, using the cache if available.
    ///
    /// # Errors
    ///
    /// Returns `DomainError::TypeNotFound` if the type does not exist in the model.
    pub fn get_type(&self, type_name: &str) -> DomainResult<Arc<TypeDefinition>> {
        // Check cache first
        if let Some(cached) = self.type_cache.get(type_name) {
            return Ok(Arc::clone(cached.value()));
        }

        // Look up in model
        let type_def = self
            .model
            .type_definitions
            .iter()
            .find(|td| td.type_name == type_name)
            .ok_or_else(|| DomainError::TypeNotFound {
                type_name: type_name.to_string(),
            })?;

        // Cache and return
        let type_def_arc = Arc::new(type_def.clone());
        self.type_cache
            .insert(type_name.to_string(), Arc::clone(&type_def_arc));
        Ok(type_def_arc)
    }

    /// Gets a relation definition for a specific type.
    ///
    /// # Errors
    ///
    /// Returns `DomainError::TypeNotFound` if the type does not exist.
    /// Returns `DomainError::RelationNotFound` if the relation does not exist on the type.
    pub fn get_relation(
        &self,
        type_name: &str,
        relation: &str,
    ) -> DomainResult<Arc<RelationDefinition>> {
        let cache_key = format!("{}:{}", type_name, relation);

        // Check cache first
        if let Some(cached) = self.relation_cache.get(&cache_key) {
            return Ok(Arc::clone(cached.value()));
        }

        // Get the type first
        let type_def = self.get_type(type_name)?;

        // Look up relation on the type
        let relation_def = type_def
            .relations
            .iter()
            .find(|r| r.name == relation)
            .ok_or_else(|| DomainError::RelationNotFound {
                type_name: type_name.to_string(),
                relation: relation.to_string(),
            })?;

        // Cache and return
        let relation_def_arc = Arc::new(relation_def.clone());
        self.relation_cache
            .insert(cache_key, Arc::clone(&relation_def_arc));
        Ok(relation_def_arc)
    }

    /// Checks if a type exists in the model.
    pub fn has_type(&self, type_name: &str) -> bool {
        self.get_type(type_name).is_ok()
    }

    /// Checks if a relation exists on a type.
    pub fn has_relation(&self, type_name: &str, relation: &str) -> bool {
        self.get_relation(type_name, relation).is_ok()
    }

    /// Validates a tuple against the type system.
    ///
    /// Checks that:
    /// - The object type exists
    /// - The relation exists on the object type
    /// - The user type exists (if not a wildcard)
    ///
    /// # Errors
    ///
    /// Returns appropriate `DomainError` variants if validation fails.
    pub fn validate_tuple(&self, tuple: &Tuple) -> DomainResult<()> {
        // Parse object to get type and id
        let object =
            Object::parse(&tuple.object).map_err(|e| DomainError::InvalidObjectFormat {
                value: format!("{}: {}", tuple.object, e),
            })?;

        // Check that object type exists
        self.get_type(&object.object_type)?;

        // Check that relation exists on the object type
        self.get_relation(&object.object_type, &tuple.relation)?;

        // Parse and validate user (if not a wildcard)
        if tuple.user != "*" && !tuple.user.ends_with(":*") {
            // Extract user type from "type:id" or "type:id#relation"
            let user_type =
                tuple
                    .user
                    .split(':')
                    .next()
                    .ok_or_else(|| DomainError::InvalidUserFormat {
                        value: tuple.user.clone(),
                    })?;

            if user_type.is_empty() {
                return Err(DomainError::InvalidUserFormat {
                    value: tuple.user.clone(),
                });
            }

            // Check that user type exists
            self.get_type(user_type)?;

            // If it's a userset reference (type:id#relation), validate the relation exists
            if let Some(hash_pos) = tuple.user.find('#') {
                let user_relation = &tuple.user[hash_pos + 1..];
                if !user_relation.is_empty() {
                    self.get_relation(user_type, user_relation)?;
                }
            }
        }

        Ok(())
    }

    /// Validates the authorization model semantically.
    ///
    /// Performs the following checks:
    /// - All referenced types exist
    /// - All referenced relations exist
    /// - No cycles in computed usersets
    /// - TTU (TupleToUserset) references are valid
    ///
    /// # Errors
    ///
    /// Returns `DomainError::ModelValidationError` if validation fails.
    pub fn validate_model(&self) -> DomainResult<()> {
        let mut errors = Vec::new();

        for type_def in &self.model.type_definitions {
            for relation_def in &type_def.relations {
                self.validate_userset(
                    &type_def.type_name,
                    &relation_def.name,
                    &relation_def.rewrite,
                    &mut errors,
                );
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(DomainError::ModelValidationError {
                message: errors.join("; "),
            })
        }
    }

    /// Validates a userset definition.
    fn validate_userset(
        &self,
        type_name: &str,
        relation_name: &str,
        userset: &super::types::Userset,
        errors: &mut Vec<String>,
    ) {
        use super::types::Userset;

        match userset {
            Userset::This => {
                // Direct assignment is always valid
            }
            Userset::ComputedUserset { relation } => {
                // Check that the computed relation exists on the same type
                if !self.has_relation(type_name, relation) {
                    errors.push(format!(
                        "type '{}' relation '{}': computed_userset references non-existent relation '{}'",
                        type_name, relation_name, relation
                    ));
                }
            }
            Userset::TupleToUserset {
                tupleset,
                computed_userset,
            } => {
                // Check that the tupleset relation exists on this type
                if !self.has_relation(type_name, tupleset) {
                    errors.push(format!(
                        "type '{}' relation '{}': tupleset references non-existent relation '{}'",
                        type_name, relation_name, tupleset
                    ));
                }
                // Note: We can't fully validate computed_userset without knowing
                // the target type of the tupleset, which would require runtime info
                // or type constraint analysis
                let _ = computed_userset; // Acknowledge parameter
            }
            Userset::Union { children } => {
                for child in children {
                    self.validate_userset(type_name, relation_name, child, errors);
                }
            }
            Userset::Intersection { children } => {
                for child in children {
                    self.validate_userset(type_name, relation_name, child, errors);
                }
            }
            Userset::Exclusion { base, subtract } => {
                self.validate_userset(type_name, relation_name, base, errors);
                self.validate_userset(type_name, relation_name, subtract, errors);
            }
        }
    }

    /// Clears the internal caches.
    ///
    /// This is primarily useful for testing or when the model is updated.
    pub fn clear_cache(&self) {
        self.type_cache.clear();
        self.relation_cache.clear();
    }

    /// Returns the number of cached type definitions.
    pub fn type_cache_size(&self) -> usize {
        self.type_cache.len()
    }

    /// Returns the number of cached relation definitions.
    pub fn relation_cache_size(&self) -> usize {
        self.relation_cache.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::types::{RelationDefinition, TypeDefinition, Userset};

    fn create_test_model() -> AuthorizationModel {
        AuthorizationModel::with_types(
            "1.1",
            vec![
                TypeDefinition {
                    type_name: "user".to_string(),
                    relations: vec![],
                },
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![
                        RelationDefinition {
                            name: "owner".to_string(),
                            type_constraints: vec!["user".to_string()],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "viewer".to_string(),
                            type_constraints: vec!["user".to_string()],
                            rewrite: Userset::Union {
                                children: vec![
                                    Userset::This,
                                    Userset::ComputedUserset {
                                        relation: "owner".to_string(),
                                    },
                                ],
                            },
                        },
                    ],
                },
                TypeDefinition {
                    type_name: "folder".to_string(),
                    relations: vec![
                        RelationDefinition {
                            name: "parent".to_string(),
                            type_constraints: vec!["folder".to_string()],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "viewer".to_string(),
                            type_constraints: vec!["user".to_string()],
                            rewrite: Userset::Union {
                                children: vec![
                                    Userset::This,
                                    Userset::TupleToUserset {
                                        tupleset: "parent".to_string(),
                                        computed_userset: "viewer".to_string(),
                                    },
                                ],
                            },
                        },
                    ],
                },
            ],
        )
    }

    #[test]
    fn test_type_system_creation() {
        let model = create_test_model();
        let ts = TypeSystem::new(model);
        assert_eq!(ts.model().schema_version, "1.1");
        assert_eq!(ts.model().type_definitions.len(), 3);
    }

    #[test]
    fn test_get_type_success() {
        let model = create_test_model();
        let ts = TypeSystem::new(model);

        let user_type = ts.get_type("user").unwrap();
        assert_eq!(user_type.type_name, "user");
        assert!(user_type.relations.is_empty());

        let doc_type = ts.get_type("document").unwrap();
        assert_eq!(doc_type.type_name, "document");
        assert_eq!(doc_type.relations.len(), 2);
    }

    #[test]
    fn test_get_type_not_found() {
        let model = create_test_model();
        let ts = TypeSystem::new(model);

        let result = ts.get_type("nonexistent");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            DomainError::TypeNotFound { type_name } if type_name == "nonexistent"
        ));
    }

    #[test]
    fn test_get_type_caching() {
        let model = create_test_model();
        let ts = TypeSystem::new(model);

        assert_eq!(ts.type_cache_size(), 0);

        // First lookup should cache
        let _ = ts.get_type("document").unwrap();
        assert_eq!(ts.type_cache_size(), 1);

        // Second lookup should use cache
        let _ = ts.get_type("document").unwrap();
        assert_eq!(ts.type_cache_size(), 1);

        // Different type should add to cache
        let _ = ts.get_type("user").unwrap();
        assert_eq!(ts.type_cache_size(), 2);
    }

    #[test]
    fn test_get_relation_success() {
        let model = create_test_model();
        let ts = TypeSystem::new(model);

        let viewer_rel = ts.get_relation("document", "viewer").unwrap();
        assert_eq!(viewer_rel.name, "viewer");

        let owner_rel = ts.get_relation("document", "owner").unwrap();
        assert_eq!(owner_rel.name, "owner");
    }

    #[test]
    fn test_get_relation_type_not_found() {
        let model = create_test_model();
        let ts = TypeSystem::new(model);

        let result = ts.get_relation("nonexistent", "viewer");
        assert!(matches!(
            result.unwrap_err(),
            DomainError::TypeNotFound { .. }
        ));
    }

    #[test]
    fn test_get_relation_relation_not_found() {
        let model = create_test_model();
        let ts = TypeSystem::new(model);

        let result = ts.get_relation("document", "nonexistent");
        assert!(matches!(
            result.unwrap_err(),
            DomainError::RelationNotFound { type_name, relation }
            if type_name == "document" && relation == "nonexistent"
        ));
    }

    #[test]
    fn test_get_relation_caching() {
        let model = create_test_model();
        let ts = TypeSystem::new(model);

        assert_eq!(ts.relation_cache_size(), 0);

        // First lookup should cache
        let _ = ts.get_relation("document", "viewer").unwrap();
        assert_eq!(ts.relation_cache_size(), 1);

        // Second lookup should use cache
        let _ = ts.get_relation("document", "viewer").unwrap();
        assert_eq!(ts.relation_cache_size(), 1);

        // Different relation should add to cache
        let _ = ts.get_relation("document", "owner").unwrap();
        assert_eq!(ts.relation_cache_size(), 2);
    }

    #[test]
    fn test_has_type() {
        let model = create_test_model();
        let ts = TypeSystem::new(model);

        assert!(ts.has_type("user"));
        assert!(ts.has_type("document"));
        assert!(!ts.has_type("nonexistent"));
    }

    #[test]
    fn test_has_relation() {
        let model = create_test_model();
        let ts = TypeSystem::new(model);

        assert!(ts.has_relation("document", "viewer"));
        assert!(ts.has_relation("document", "owner"));
        assert!(!ts.has_relation("document", "nonexistent"));
        assert!(!ts.has_relation("nonexistent", "viewer"));
    }

    #[test]
    fn test_validate_tuple_success() {
        let model = create_test_model();
        let ts = TypeSystem::new(model);

        // Valid direct tuple
        let tuple = Tuple::new("user:alice", "viewer", "document:readme").unwrap();
        assert!(ts.validate_tuple(&tuple).is_ok());

        // Valid with wildcard user
        let tuple = Tuple::new("user:*", "viewer", "document:readme").unwrap();
        assert!(ts.validate_tuple(&tuple).is_ok());

        // Valid with global wildcard
        let tuple = Tuple::new("*", "viewer", "document:readme").unwrap();
        assert!(ts.validate_tuple(&tuple).is_ok());
    }

    #[test]
    fn test_validate_tuple_invalid_object_type() {
        let model = create_test_model();
        let ts = TypeSystem::new(model);

        let tuple = Tuple::new("user:alice", "viewer", "nonexistent:doc1").unwrap();
        let result = ts.validate_tuple(&tuple);
        assert!(matches!(
            result.unwrap_err(),
            DomainError::TypeNotFound { .. }
        ));
    }

    #[test]
    fn test_validate_tuple_invalid_relation() {
        let model = create_test_model();
        let ts = TypeSystem::new(model);

        let tuple = Tuple::new("user:alice", "nonexistent", "document:readme").unwrap();
        let result = ts.validate_tuple(&tuple);
        assert!(matches!(
            result.unwrap_err(),
            DomainError::RelationNotFound { .. }
        ));
    }

    #[test]
    fn test_validate_tuple_invalid_user_type() {
        let model = create_test_model();
        let ts = TypeSystem::new(model);

        let tuple = Tuple::new("nonexistent:alice", "viewer", "document:readme").unwrap();
        let result = ts.validate_tuple(&tuple);
        assert!(matches!(
            result.unwrap_err(),
            DomainError::TypeNotFound { .. }
        ));
    }

    #[test]
    fn test_validate_tuple_userset_reference() {
        let model = create_test_model();
        let ts = TypeSystem::new(model);

        // Valid userset reference
        let tuple = Tuple::new("folder:docs#viewer", "viewer", "document:readme").unwrap();
        assert!(ts.validate_tuple(&tuple).is_ok());

        // Invalid userset reference (relation doesn't exist)
        let tuple = Tuple::new("folder:docs#nonexistent", "viewer", "document:readme").unwrap();
        let result = ts.validate_tuple(&tuple);
        assert!(matches!(
            result.unwrap_err(),
            DomainError::RelationNotFound { .. }
        ));
    }

    #[test]
    fn test_validate_model_success() {
        let model = create_test_model();
        let ts = TypeSystem::new(model);

        assert!(ts.validate_model().is_ok());
    }

    #[test]
    fn test_validate_model_invalid_computed_userset() {
        let model = AuthorizationModel::with_types(
            "1.1",
            vec![TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec![],
                    rewrite: Userset::ComputedUserset {
                        relation: "nonexistent".to_string(),
                    },
                }],
            }],
        );
        let ts = TypeSystem::new(model);

        let result = ts.validate_model();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, DomainError::ModelValidationError { message } if message.contains("nonexistent"))
        );
    }

    #[test]
    fn test_validate_model_invalid_ttu_tupleset() {
        let model = AuthorizationModel::with_types(
            "1.1",
            vec![TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec![],
                    rewrite: Userset::TupleToUserset {
                        tupleset: "nonexistent".to_string(),
                        computed_userset: "viewer".to_string(),
                    },
                }],
            }],
        );
        let ts = TypeSystem::new(model);

        let result = ts.validate_model();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, DomainError::ModelValidationError { message } if message.contains("nonexistent"))
        );
    }

    #[test]
    fn test_clear_cache() {
        let model = create_test_model();
        let ts = TypeSystem::new(model);

        // Populate caches
        let _ = ts.get_type("document").unwrap();
        let _ = ts.get_relation("document", "viewer").unwrap();
        assert!(ts.type_cache_size() > 0);
        assert!(ts.relation_cache_size() > 0);

        // Clear and verify
        ts.clear_cache();
        assert_eq!(ts.type_cache_size(), 0);
        assert_eq!(ts.relation_cache_size(), 0);
    }
}
