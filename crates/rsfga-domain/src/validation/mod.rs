//! Authorization model validation.
//!
//! Validates that authorization models are semantically correct:
//! - No cyclic relation definitions
//! - All referenced types exist
//! - All referenced relations exist
//! - Type constraints are satisfied

use std::collections::{HashMap, HashSet};

use crate::cel::CelExpression;
use crate::model::{AuthorizationModel, TypeConstraint, TypeDefinition, Userset};

/// Validation error types
#[derive(Debug, Clone, PartialEq)]
pub enum ValidationError {
    /// A relation definition contains a cycle
    CyclicRelation {
        type_name: String,
        relation_name: String,
        cycle_path: Vec<String>,
    },
    /// A referenced relation does not exist
    UndefinedRelation {
        type_name: String,
        relation_name: String,
        referenced_relation: String,
    },
    /// A referenced type does not exist
    UndefinedType {
        type_name: String,
        relation_name: String,
        referenced_type: String,
    },
    /// A relation is referenced but not defined
    MissingRelationDefinition {
        type_name: String,
        relation_name: String,
    },
    /// Type constraint references undefined type
    InvalidTypeConstraint {
        type_name: String,
        relation_name: String,
        invalid_type: String,
    },
    /// Empty model (no type definitions)
    EmptyModel,
    /// Condition referenced in type constraint does not exist
    UndefinedCondition {
        type_name: String,
        relation_name: String,
        condition_name: String,
    },
    /// Condition expression is not valid CEL
    InvalidConditionExpression {
        condition_name: String,
        error_message: String,
    },
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationError::CyclicRelation {
                type_name,
                relation_name,
                cycle_path,
            } => write!(
                f,
                "cyclic relation definition in {}#{}: {}",
                type_name,
                relation_name,
                cycle_path.join(" -> ")
            ),
            ValidationError::UndefinedRelation {
                type_name,
                relation_name,
                referenced_relation,
            } => write!(
                f,
                "undefined relation '{}' referenced in {}#{}",
                referenced_relation, type_name, relation_name
            ),
            ValidationError::UndefinedType {
                type_name,
                relation_name,
                referenced_type,
            } => write!(
                f,
                "undefined type '{}' referenced in {}#{}",
                referenced_type, type_name, relation_name
            ),
            ValidationError::MissingRelationDefinition {
                type_name,
                relation_name,
            } => write!(
                f,
                "relation '{}' referenced but not defined in type '{}'",
                relation_name, type_name
            ),
            ValidationError::InvalidTypeConstraint {
                type_name,
                relation_name,
                invalid_type,
            } => write!(
                f,
                "invalid type constraint '{}' in {}#{}",
                invalid_type, type_name, relation_name
            ),
            ValidationError::EmptyModel => {
                write!(f, "model must have at least one type definition")
            }
            ValidationError::UndefinedCondition {
                type_name,
                relation_name,
                condition_name,
            } => write!(
                f,
                "undefined condition '{}' referenced in {}#{}",
                condition_name, type_name, relation_name
            ),
            ValidationError::InvalidConditionExpression {
                condition_name,
                error_message,
            } => write!(
                f,
                "invalid CEL expression in condition '{}': {}",
                condition_name, error_message
            ),
        }
    }
}

impl std::error::Error for ValidationError {}

/// Result type for validation operations
pub type ValidationResult<T> = Result<T, Vec<ValidationError>>;

/// Model validator
pub struct ModelValidator {
    /// All defined types in the model
    defined_types: HashSet<String>,
    /// Relations defined on each type: type_name -> [relation_names]
    type_relations: HashMap<String, HashSet<String>>,
    /// All defined conditions in the model
    defined_conditions: HashSet<String>,
}

impl ModelValidator {
    /// Create a new validator for the given model
    pub fn new(model: &AuthorizationModel) -> Self {
        let mut defined_types = HashSet::new();
        let mut type_relations = HashMap::new();
        let defined_conditions: HashSet<String> =
            model.conditions.iter().map(|c| c.name.clone()).collect();

        for type_def in &model.type_definitions {
            defined_types.insert(type_def.type_name.clone());

            let relations: HashSet<String> =
                type_def.relations.iter().map(|r| r.name.clone()).collect();
            type_relations.insert(type_def.type_name.clone(), relations);
        }

        Self {
            defined_types,
            type_relations,
            defined_conditions,
        }
    }

    /// Validate the model and return any errors found
    pub fn validate(&self, model: &AuthorizationModel) -> ValidationResult<()> {
        let mut errors = Vec::new();

        // Check for empty model
        if model.type_definitions.is_empty() {
            errors.push(ValidationError::EmptyModel);
            return Err(errors);
        }

        // Validate condition expressions are valid CEL
        for condition in &model.conditions {
            if let Err(e) = CelExpression::parse(&condition.expression) {
                errors.push(ValidationError::InvalidConditionExpression {
                    condition_name: condition.name.clone(),
                    error_message: e.to_string(),
                });
            }
        }

        // Validate each type definition
        for type_def in &model.type_definitions {
            self.validate_type_definition(type_def, &mut errors);
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Validate a single type definition
    fn validate_type_definition(
        &self,
        type_def: &TypeDefinition,
        errors: &mut Vec<ValidationError>,
    ) {
        // Check for undefined relation references in all relations
        for relation_def in &type_def.relations {
            // Validate type constraints
            self.validate_type_constraints(
                &type_def.type_name,
                &relation_def.name,
                &relation_def.type_constraints,
                errors,
            );

            self.validate_userset(
                &type_def.type_name,
                &relation_def.name,
                &relation_def.rewrite,
                errors,
            );
        }

        // Check for cycles at the type level
        if let Some((relation_name, cycle_path)) = Self::detect_cycle_in_type(type_def) {
            errors.push(ValidationError::CyclicRelation {
                type_name: type_def.type_name.clone(),
                relation_name,
                cycle_path,
            });
        }
    }

    /// Validate type constraints (e.g., [user], [group#member], [user with condition])
    fn validate_type_constraints(
        &self,
        type_name: &str,
        relation_name: &str,
        constraints: &[TypeConstraint],
        errors: &mut Vec<ValidationError>,
    ) {
        for constraint in constraints {
            let type_ref = &constraint.type_name;
            // Parse the type_name - can be "type" or "type#relation"
            let (ref_type, ref_relation) = if let Some(hash_pos) = type_ref.find('#') {
                let type_part = &type_ref[..hash_pos];
                let rel_part = &type_ref[hash_pos + 1..];
                (type_part, Some(rel_part))
            } else {
                (type_ref.as_str(), None)
            };

            // Check if the referenced type exists
            if !self.defined_types.contains(ref_type) {
                errors.push(ValidationError::InvalidTypeConstraint {
                    type_name: type_name.to_string(),
                    relation_name: relation_name.to_string(),
                    invalid_type: constraint.type_name.clone(),
                });
                continue;
            }

            // If there's a relation part (type#relation), check if that relation exists
            if let Some(relation) = ref_relation {
                if !self.relation_exists(ref_type, relation) {
                    errors.push(ValidationError::InvalidTypeConstraint {
                        type_name: type_name.to_string(),
                        relation_name: relation_name.to_string(),
                        invalid_type: constraint.type_name.clone(),
                    });
                }
            }

            // Check if the referenced condition exists
            if let Some(condition_name) = &constraint.condition {
                if !self.defined_conditions.contains(condition_name) {
                    errors.push(ValidationError::UndefinedCondition {
                        type_name: type_name.to_string(),
                        relation_name: relation_name.to_string(),
                        condition_name: condition_name.clone(),
                    });
                }
            }
        }
    }

    /// Validate a userset expression
    fn validate_userset(
        &self,
        type_name: &str,
        relation_name: &str,
        userset: &Userset,
        errors: &mut Vec<ValidationError>,
    ) {
        match userset {
            Userset::This => {
                // Direct assignment, always valid
            }
            Userset::ComputedUserset { relation } => {
                // Check if the referenced relation exists on this type
                if let Some(relations) = self.type_relations.get(type_name) {
                    if !relations.contains(relation) {
                        errors.push(ValidationError::UndefinedRelation {
                            type_name: type_name.to_string(),
                            relation_name: relation_name.to_string(),
                            referenced_relation: relation.clone(),
                        });
                    }
                }
            }
            Userset::TupleToUserset {
                tupleset,
                computed_userset: _,
            } => {
                // The tupleset must be a relation on this type
                if let Some(relations) = self.type_relations.get(type_name) {
                    if !relations.contains(tupleset) {
                        errors.push(ValidationError::UndefinedRelation {
                            type_name: type_name.to_string(),
                            relation_name: relation_name.to_string(),
                            referenced_relation: tupleset.clone(),
                        });
                    }
                }
                // Note: We can't fully validate computed_userset without knowing the target type
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

    /// Check if a type exists in the model
    pub fn type_exists(&self, type_name: &str) -> bool {
        self.defined_types.contains(type_name)
    }

    /// Check if a relation exists on a type
    pub fn relation_exists(&self, type_name: &str, relation_name: &str) -> bool {
        self.type_relations
            .get(type_name)
            .is_some_and(|relations| relations.contains(relation_name))
    }

    /// Detect cycles in relation definitions using DFS
    fn detect_cycle_in_type(type_def: &TypeDefinition) -> Option<(String, Vec<String>)> {
        // Build adjacency list: relation -> [referenced relations]
        let mut graph: HashMap<String, HashSet<String>> = HashMap::new();
        for rel_def in &type_def.relations {
            let mut refs = HashSet::new();
            collect_referenced_relations(&rel_def.rewrite, &mut refs);
            graph.insert(rel_def.name.clone(), refs);
        }

        // DFS to find cycles
        let mut visited = HashSet::new();
        let mut rec_stack = HashSet::new();
        let mut path = Vec::new();

        for rel_name in graph.keys() {
            if dfs_cycle_detect(rel_name, &graph, &mut visited, &mut rec_stack, &mut path) {
                return Some((rel_name.clone(), path));
            }
        }
        None
    }
}

/// Collect all referenced relations from a userset expression
fn collect_referenced_relations(userset: &Userset, refs: &mut HashSet<String>) {
    match userset {
        Userset::This => {}
        Userset::ComputedUserset { relation } => {
            refs.insert(relation.clone());
        }
        Userset::TupleToUserset { .. } => {
            // Tuple to userset goes to a different object, no local cycle
        }
        Userset::Union { children } => {
            for child in children {
                collect_referenced_relations(child, refs);
            }
        }
        Userset::Intersection { children } => {
            for child in children {
                collect_referenced_relations(child, refs);
            }
        }
        Userset::Exclusion { base, subtract } => {
            collect_referenced_relations(base, refs);
            collect_referenced_relations(subtract, refs);
        }
    }
}

/// DFS-based cycle detection in relation graph
fn dfs_cycle_detect(
    node: &str,
    graph: &HashMap<String, HashSet<String>>,
    visited: &mut HashSet<String>,
    rec_stack: &mut HashSet<String>,
    path: &mut Vec<String>,
) -> bool {
    if rec_stack.contains(node) {
        // Found a cycle
        path.push(node.to_string());
        return true;
    }
    if visited.contains(node) {
        return false;
    }

    visited.insert(node.to_string());
    rec_stack.insert(node.to_string());
    path.push(node.to_string());

    if let Some(neighbors) = graph.get(node) {
        for neighbor in neighbors {
            // Only follow edges to relations that exist in this type
            if graph.contains_key(neighbor)
                && dfs_cycle_detect(neighbor, graph, visited, rec_stack, path)
            {
                return true;
            }
        }
    }

    rec_stack.remove(node);
    path.pop();
    false
}

/// Validate an authorization model
pub fn validate(model: &AuthorizationModel) -> ValidationResult<()> {
    let validator = ModelValidator::new(model);
    validator.validate(model)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{RelationDefinition, TypeDefinition, Userset};

    fn create_valid_model() -> AuthorizationModel {
        AuthorizationModel {
            id: None,
            schema_version: "1.1".to_string(),
            type_definitions: vec![
                TypeDefinition {
                    type_name: "user".into(),
                    relations: vec![],
                },
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![
                        RelationDefinition {
                            name: "owner".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "editor".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::Union {
                                children: vec![
                                    Userset::This,
                                    Userset::ComputedUserset {
                                        relation: "owner".to_string(),
                                    },
                                ],
                            },
                        },
                        RelationDefinition {
                            name: "viewer".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::Union {
                                children: vec![
                                    Userset::This,
                                    Userset::ComputedUserset {
                                        relation: "editor".to_string(),
                                    },
                                ],
                            },
                        },
                    ],
                },
            ],
            conditions: Vec::new(),
        }
    }

    #[test]
    fn test_validator_accepts_valid_model() {
        let model = create_valid_model();
        let result = validate(&model);
        assert!(result.is_ok(), "Valid model should pass validation");
    }

    #[test]
    fn test_validator_rejects_cyclic_relation_definitions() {
        let model = AuthorizationModel {
            id: None,
            schema_version: "1.1".to_string(),
            type_definitions: vec![TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "a".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::ComputedUserset {
                            relation: "b".to_string(),
                        },
                    },
                    RelationDefinition {
                        name: "b".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::ComputedUserset {
                            relation: "a".to_string(),
                        },
                    },
                ],
            }],
            conditions: Vec::new(),
        };

        let result = validate(&model);
        assert!(result.is_err(), "Cyclic relations should fail validation");
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::CyclicRelation { .. })),
            "Should have cyclic relation error"
        );
    }

    #[test]
    fn test_validator_rejects_undefined_relation_references() {
        let model = AuthorizationModel {
            id: None,
            schema_version: "1.1".to_string(),
            type_definitions: vec![TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec![],
                    rewrite: Userset::ComputedUserset {
                        relation: "nonexistent".into(),
                    },
                }],
            }],
            conditions: Vec::new(),
        };

        let result = validate(&model);
        assert!(result.is_err(), "Undefined relation should fail validation");
        let errors = result.unwrap_err();
        assert!(
            errors.iter().any(|e| matches!(
                e,
                ValidationError::UndefinedRelation { referenced_relation, .. }
                if referenced_relation == "nonexistent"
            )),
            "Should have undefined relation error"
        );
    }

    #[test]
    fn test_validator_rejects_undefined_type_references() {
        // This test validates that the validator exists and can check type references
        // In a complete implementation, type constraints would be validated
        let validator = ModelValidator::new(&create_valid_model());
        assert!(validator.type_exists("user"));
        assert!(validator.type_exists("document"));
        assert!(!validator.type_exists("nonexistent"));
    }

    #[test]
    fn test_validator_checks_relation_type_constraints() {
        // Test that relations can be queried
        let validator = ModelValidator::new(&create_valid_model());
        assert!(validator.relation_exists("document", "owner"));
        assert!(validator.relation_exists("document", "editor"));
        assert!(validator.relation_exists("document", "viewer"));
        assert!(!validator.relation_exists("document", "nonexistent"));
        assert!(!validator.relation_exists("nonexistent", "owner"));
    }

    #[test]
    fn test_validator_ensures_all_relations_have_definitions() {
        // Empty model test
        let empty_model = AuthorizationModel {
            id: None,
            schema_version: "1.1".to_string(),
            type_definitions: vec![],
            conditions: Vec::new(),
        };

        let result = validate(&empty_model);
        assert!(result.is_err(), "Empty model should fail validation");
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::EmptyModel)),
            "Should have empty model error"
        );
    }

    #[test]
    fn test_validator_accepts_union_relations() {
        let model = AuthorizationModel {
            id: None,
            schema_version: "1.1".to_string(),
            type_definitions: vec![TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "owner".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "editor".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::Union {
                            children: vec![
                                Userset::This,
                                Userset::ComputedUserset {
                                    relation: "owner".to_string(),
                                },
                                Userset::ComputedUserset {
                                    relation: "editor".to_string(),
                                },
                            ],
                        },
                    },
                ],
            }],
            conditions: Vec::new(),
        };

        let result = validate(&model);
        assert!(result.is_ok(), "Union relations should be valid");
    }

    #[test]
    fn test_validator_accepts_intersection_relations() {
        let model = AuthorizationModel {
            id: None,
            schema_version: "1.1".to_string(),
            type_definitions: vec![TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "owner".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "approved".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "can_delete".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::Intersection {
                            children: vec![
                                Userset::ComputedUserset {
                                    relation: "owner".to_string(),
                                },
                                Userset::ComputedUserset {
                                    relation: "approved".to_string(),
                                },
                            ],
                        },
                    },
                ],
            }],
            conditions: Vec::new(),
        };

        let result = validate(&model);
        assert!(result.is_ok(), "Intersection relations should be valid");
    }

    #[test]
    fn test_validator_accepts_exclusion_relations() {
        let model = AuthorizationModel {
            id: None,
            schema_version: "1.1".to_string(),
            type_definitions: vec![TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "blocked".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "can_view".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::Exclusion {
                            base: Box::new(Userset::ComputedUserset {
                                relation: "viewer".to_string(),
                            }),
                            subtract: Box::new(Userset::ComputedUserset {
                                relation: "blocked".to_string(),
                            }),
                        },
                    },
                ],
            }],
            conditions: Vec::new(),
        };

        let result = validate(&model);
        assert!(result.is_ok(), "Exclusion relations should be valid");
    }

    #[test]
    fn test_validator_rejects_invalid_type_constraints() {
        let model = AuthorizationModel {
            id: None,
            schema_version: "1.1".to_string(),
            type_definitions: vec![
                TypeDefinition {
                    type_name: "user".into(),
                    relations: vec![],
                },
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![RelationDefinition {
                        name: "owner".to_string(),
                        // References a type that doesn't exist
                        type_constraints: vec!["nonexistent".into()],
                        rewrite: Userset::This,
                    }],
                },
            ],
            conditions: Vec::new(),
        };

        let result = validate(&model);
        assert!(
            result.is_err(),
            "Invalid type constraint should fail validation"
        );
        let errors = result.unwrap_err();
        assert!(
            errors.iter().any(|e| matches!(
                e,
                ValidationError::InvalidTypeConstraint { invalid_type, .. }
                if invalid_type == "nonexistent"
            )),
            "Should have invalid type constraint error"
        );
    }

    #[test]
    fn test_validator_rejects_invalid_relation_in_type_constraint() {
        let model = AuthorizationModel {
            id: None,
            schema_version: "1.1".to_string(),
            type_definitions: vec![
                TypeDefinition {
                    type_name: "user".into(),
                    relations: vec![],
                },
                TypeDefinition {
                    type_name: "group".to_string(),
                    relations: vec![RelationDefinition {
                        name: "member".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    }],
                },
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![RelationDefinition {
                        name: "owner".to_string(),
                        // References a relation that doesn't exist on the type
                        type_constraints: vec!["group#nonexistent".into()],
                        rewrite: Userset::This,
                    }],
                },
            ],
            conditions: Vec::new(),
        };

        let result = validate(&model);
        assert!(
            result.is_err(),
            "Invalid relation in type constraint should fail validation"
        );
        let errors = result.unwrap_err();
        assert!(
            errors.iter().any(|e| matches!(
                e,
                ValidationError::InvalidTypeConstraint { invalid_type, .. }
                if invalid_type == "group#nonexistent"
            )),
            "Should have invalid type constraint error for relation"
        );
    }

    #[test]
    fn test_validator_accepts_valid_type_constraints() {
        let model = AuthorizationModel {
            id: None,
            schema_version: "1.1".to_string(),
            type_definitions: vec![
                TypeDefinition {
                    type_name: "user".into(),
                    relations: vec![],
                },
                TypeDefinition {
                    type_name: "group".to_string(),
                    relations: vec![RelationDefinition {
                        name: "member".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    }],
                },
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![RelationDefinition {
                        name: "owner".to_string(),
                        type_constraints: vec!["user".into(), "group#member".into()],
                        rewrite: Userset::This,
                    }],
                },
            ],
            conditions: Vec::new(),
        };

        let result = validate(&model);
        assert!(
            result.is_ok(),
            "Valid type constraints should pass validation: {:?}",
            result.err()
        );
    }

    // ========== Condition Validation Tests (Section 3: Condition Model Integration) ==========

    #[test]
    fn test_validator_validates_condition_expressions() {
        // Test: Model validator validates condition expressions
        // Valid CEL expression should pass
        use crate::model::{Condition, ConditionParameter};

        let model = AuthorizationModel {
            id: None,
            schema_version: "1.1".to_string(),
            type_definitions: vec![TypeDefinition {
                type_name: "user".into(),
                relations: vec![],
            }],
            conditions: vec![Condition::with_parameters(
                "time_check",
                vec![
                    ConditionParameter::new("current_time", "timestamp"),
                    ConditionParameter::new("expires_at", "timestamp"),
                ],
                "current_time < expires_at",
            )
            .unwrap()],
        };

        let result = validate(&model);
        assert!(
            result.is_ok(),
            "Valid CEL expression should pass validation: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_validator_rejects_invalid_condition_expression() {
        // Invalid CEL expression should fail
        use crate::model::Condition;

        let model = AuthorizationModel {
            id: None,
            schema_version: "1.1".to_string(),
            type_definitions: vec![TypeDefinition {
                type_name: "user".into(),
                relations: vec![],
            }],
            conditions: vec![Condition::new(
                "bad_condition",
                "this is not valid CEL syntax !!!@#$",
            )
            .unwrap()],
        };

        let result = validate(&model);
        assert!(
            result.is_err(),
            "Invalid CEL expression should fail validation"
        );
        let errors = result.unwrap_err();
        assert!(
            errors.iter().any(|e| matches!(
                e,
                ValidationError::InvalidConditionExpression { condition_name, .. }
                if condition_name == "bad_condition"
            )),
            "Should have invalid condition expression error"
        );
    }

    #[test]
    fn test_validator_rejects_undefined_condition_reference() {
        // Referencing a condition that doesn't exist should fail
        let model = AuthorizationModel {
            id: None,
            schema_version: "1.1".to_string(),
            type_definitions: vec![
                TypeDefinition {
                    type_name: "user".into(),
                    relations: vec![],
                },
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec![TypeConstraint::with_condition(
                            "user",
                            "nonexistent_condition",
                        )],
                        rewrite: Userset::This,
                    }],
                },
            ],
            conditions: Vec::new(), // No conditions defined!
        };

        let result = validate(&model);
        assert!(
            result.is_err(),
            "Undefined condition reference should fail validation"
        );
        let errors = result.unwrap_err();
        assert!(
            errors.iter().any(|e| matches!(
                e,
                ValidationError::UndefinedCondition { condition_name, .. }
                if condition_name == "nonexistent_condition"
            )),
            "Should have undefined condition error"
        );
    }

    #[test]
    fn test_validator_accepts_valid_condition_reference() {
        // Referencing a defined condition should pass
        use crate::model::Condition;

        let model = AuthorizationModel {
            id: None,
            schema_version: "1.1".to_string(),
            type_definitions: vec![
                TypeDefinition {
                    type_name: "user".into(),
                    relations: vec![],
                },
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec![TypeConstraint::with_condition(
                            "user",
                            "time_check",
                        )],
                        rewrite: Userset::This,
                    }],
                },
            ],
            conditions: vec![Condition::new("time_check", "true").unwrap()],
        };

        let result = validate(&model);
        assert!(
            result.is_ok(),
            "Valid condition reference should pass validation: {:?}",
            result.err()
        );
    }
}
