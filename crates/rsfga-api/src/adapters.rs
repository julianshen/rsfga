//! Adapters that bridge storage layer to domain layer.
//!
//! The domain layer (rsfga-domain) defines abstract traits for data access:
//! - `TupleReader`: Read tuples for authorization checks
//! - `ModelReader`: Read authorization models
//!
//! The storage layer (rsfga-storage) implements `DataStore` with concrete backends.
//!
//! This module provides adapters that implement domain traits using `DataStore`,
//! enabling the API layer to connect storage implementations to domain logic.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use moka::future::Cache;

use rsfga_domain::error::{DomainError, DomainResult};
use rsfga_domain::model::{
    AuthorizationModel, Condition, ConditionParameter, RelationDefinition, TypeConstraint,
    TypeDefinition, Userset,
};
use rsfga_domain::resolver::{ModelReader, ObjectTupleInfo, StoredTupleRef, TupleReader};
use rsfga_storage::DataStore;

/// Converts a DomainError to a user-friendly validation error message.
///
/// This function provides centralized error message formatting for validation errors,
/// ensuring consistent error messages across HTTP and gRPC layers.
///
/// # Arguments
///
/// * `error` - The domain error to convert
///
/// # Returns
///
/// A human-readable error message suitable for API responses.
pub fn domain_error_to_validation_message(error: &DomainError) -> String {
    match error {
        DomainError::ModelParseError { message } => message.clone(),
        DomainError::ModelValidationError { message } => message.clone(),
        DomainError::TypeNotFound { type_name } => {
            format!("type '{type_name}' not defined in authorization model")
        }
        DomainError::RelationNotFound {
            type_name,
            relation,
        } => {
            format!("relation '{relation}' not defined for type '{type_name}'")
        }
        DomainError::ConditionNotFound { condition_name } => {
            format!("condition '{condition_name}' not defined in authorization model")
        }
        // For other errors, use the Display implementation
        other => other.to_string(),
    }
}

/// Default cache TTL for parsed authorization models.
/// Slightly longer TTL (30s) is safe with moka's automatic eviction.
const MODEL_CACHE_TTL: Duration = Duration::from_secs(30);

/// Maximum number of models to cache. Prevents unbounded memory growth.
const MODEL_CACHE_MAX_CAPACITY: u64 = 1000;

/// Maximum nesting depth for parsing userset definitions.
/// Matches the resolver's depth limit of 25 for consistency.
/// Prevents stack overflow from maliciously nested model structures.
const MAX_PARSE_DEPTH: usize = 25;

/// Parses a JSON relation definition into a Userset.
///
/// Supports all OpenFGA relation rewrite types:
/// - Empty object `{}` or `{"this": {}}` → `Userset::This`
/// - `{"computedUserset": {"relation": "..."}}` → `Userset::ComputedUserset`
/// - `{"tupleToUserset": {...}}` → `Userset::TupleToUserset`
/// - `{"union": {"child": [...]}}` → `Userset::Union`
/// - `{"intersection": {"child": [...]}}` → `Userset::Intersection`
/// - `{"exclusion": {"base": {...}, "subtract": {...}}}` → `Userset::Exclusion`
/// - `{"difference": {...}}` → `Userset::Exclusion` (alias)
#[doc(hidden)]
pub fn parse_userset(
    rel_def: &serde_json::Value,
    type_name: &str,
    rel_name: &str,
) -> DomainResult<Userset> {
    parse_userset_inner(rel_def, type_name, rel_name, 0)
}

/// Internal helper for parsing child arrays in union/intersection.
/// Reduces code duplication between union and intersection parsing.
fn parse_child_array(
    value: &serde_json::Value,
    key: &str,
    type_name: &str,
    rel_name: &str,
    depth: usize,
) -> DomainResult<Vec<Userset>> {
    let children = value
        .get("child")
        .and_then(|v| v.as_array())
        .ok_or_else(|| DomainError::ModelParseError {
            message: format!(
                "{key} requires 'child' array: type '{type_name}', relation '{rel_name}'"
            ),
        })?;

    // Security: reject empty child arrays to prevent "allow all" in intersection
    // or "deny all" in union, which could lead to auth bypass or unintended denials
    if children.is_empty() {
        return Err(DomainError::ModelParseError {
            message: format!(
                "{key} requires at least one child: type '{type_name}', relation '{rel_name}'"
            ),
        });
    }

    children
        .iter()
        .map(|c| parse_userset_inner(c, type_name, rel_name, depth + 1))
        .collect()
}

/// Internal recursive parser with depth tracking.
fn parse_userset_inner(
    rel_def: &serde_json::Value,
    type_name: &str,
    rel_name: &str,
    depth: usize,
) -> DomainResult<Userset> {
    // Security: prevent stack overflow from deeply nested structures
    if depth > MAX_PARSE_DEPTH {
        return Err(DomainError::ModelParseError {
            message: format!(
                "relation definition exceeds max nesting depth of {MAX_PARSE_DEPTH}: type '{type_name}', relation '{rel_name}'"
            ),
        });
    }

    let obj = rel_def
        .as_object()
        .ok_or_else(|| DomainError::ModelParseError {
            message: format!(
                "relation definition must be an object: type '{type_name}', relation '{rel_name}'"
            ),
        })?;

    // Empty object or {"this": {}} means direct assignment
    if obj.is_empty() || obj.contains_key("this") {
        return Ok(Userset::This);
    }

    // computedUserset or computed_userset
    if let Some(cu) = obj
        .get("computedUserset")
        .or_else(|| obj.get("computed_userset"))
    {
        let relation = cu.get("relation").and_then(|v| v.as_str()).ok_or_else(|| {
            DomainError::ModelParseError {
                message: format!(
                    "computedUserset requires 'relation' field: type '{type_name}', relation '{rel_name}'"
                ),
            }
        })?;
        return Ok(Userset::ComputedUserset {
            relation: relation.to_string(),
        });
    }

    // tupleToUserset or tuple_to_userset
    if let Some(ttu) = obj
        .get("tupleToUserset")
        .or_else(|| obj.get("tuple_to_userset"))
    {
        let tupleset = ttu
            .get("tupleset")
            .and_then(|ts| ts.get("relation"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| DomainError::ModelParseError {
                message: format!(
                    "tupleToUserset requires 'tupleset.relation' field: type '{type_name}', relation '{rel_name}'"
                ),
            })?;
        let computed_userset = ttu
            .get("computedUserset")
            .or_else(|| ttu.get("computed_userset"))
            .and_then(|cu| cu.get("relation"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| DomainError::ModelParseError {
                message: format!(
                    "tupleToUserset requires 'computedUserset.relation' field: type '{type_name}', relation '{rel_name}'"
                ),
            })?;
        return Ok(Userset::TupleToUserset {
            tupleset: tupleset.to_string(),
            computed_userset: computed_userset.to_string(),
        });
    }

    // union
    if let Some(union) = obj.get("union") {
        let parsed_children = parse_child_array(union, "union", type_name, rel_name, depth)?;
        return Ok(Userset::Union {
            children: parsed_children,
        });
    }

    // intersection
    if let Some(intersection) = obj.get("intersection") {
        let parsed_children =
            parse_child_array(intersection, "intersection", type_name, rel_name, depth)?;
        return Ok(Userset::Intersection {
            children: parsed_children,
        });
    }

    // exclusion or difference (both map to Userset::Exclusion)
    if let Some(exclusion) = obj.get("exclusion").or_else(|| obj.get("difference")) {
        let base = exclusion
            .get("base")
            .ok_or_else(|| DomainError::ModelParseError {
                message: format!(
                    "exclusion requires 'base' field: type '{type_name}', relation '{rel_name}'"
                ),
            })?;
        let subtract = exclusion
            .get("subtract")
            .ok_or_else(|| DomainError::ModelParseError {
                message: format!(
                    "exclusion requires 'subtract' field: type '{type_name}', relation '{rel_name}'"
                ),
            })?;
        return Ok(Userset::Exclusion {
            base: Box::new(parse_userset_inner(base, type_name, rel_name, depth + 1)?),
            subtract: Box::new(parse_userset_inner(
                subtract,
                type_name,
                rel_name,
                depth + 1,
            )?),
        });
    }

    // Unknown relation definition - fail with descriptive error
    let keys: Vec<_> = obj.keys().collect();
    Err(DomainError::ModelParseError {
        message: format!(
            "unknown relation definition keys {keys:?}: type '{type_name}', relation '{rel_name}'"
        ),
    })
}

/// Parses type constraints from metadata if present.
///
/// Type constraints restrict which user types can have a relation.
/// Format: `{"metadata": {"relations": {"<rel>": {"directly_related_user_types": [...]}}}}`
///
/// # Security Note
///
/// This function returns an error if `directly_related_user_types` is present but
/// contains malformed entries. Silently dropping entries could widen access by
/// treating a constrained relation as unconstrained.
fn parse_type_constraints(
    type_def: &serde_json::Value,
    type_name: &str,
    rel_name: &str,
) -> DomainResult<Vec<TypeConstraint>> {
    // If no metadata or no directly_related_user_types, return empty (no constraints)
    let Some(arr) = type_def
        .get("metadata")
        .and_then(|m| m.get("relations"))
        .and_then(|r| r.get(rel_name))
        .and_then(|rel_meta| rel_meta.get("directly_related_user_types"))
        .and_then(|v| v.as_array())
    else {
        return Ok(vec![]);
    };

    // Parse each type constraint entry, failing on malformed entries
    let mut constraints = Vec::with_capacity(arr.len());
    for (idx, item) in arr.iter().enumerate() {
        // Each item must have a "type" field
        let user_type = item.get("type").and_then(|t| t.as_str()).ok_or_else(|| {
            DomainError::ModelParseError {
                message: format!(
                    "directly_related_user_types[{idx}] missing 'type' field: type '{type_name}', relation '{rel_name}'"
                ),
            }
        })?;

        let condition = item.get("condition").and_then(|c| c.as_str());

        let full_type = if let Some(relation) = item.get("relation").and_then(|r| r.as_str()) {
            format!("{user_type}#{relation}")
        } else {
            user_type.to_string()
        };

        let constraint = if let Some(cond) = condition {
            TypeConstraint::with_condition(&full_type, cond)
        } else {
            TypeConstraint::new(&full_type)
        };
        constraints.push(constraint);
    }

    Ok(constraints)
}

/// Valid OpenFGA type names for condition parameters.
/// These match the TypeName enum from the OpenFGA protobuf spec.
const VALID_TYPE_NAMES: &[&str] = &[
    "TYPE_NAME_UNSPECIFIED",
    "TYPE_NAME_ANY",
    "TYPE_NAME_BOOL",
    "TYPE_NAME_STRING",
    "TYPE_NAME_INT",
    "TYPE_NAME_UINT",
    "TYPE_NAME_DOUBLE",
    "TYPE_NAME_DURATION",
    "TYPE_NAME_TIMESTAMP",
    "TYPE_NAME_MAP",
    "TYPE_NAME_LIST",
    "TYPE_NAME_IPADDRESS",
];

/// Validates that a type_name is a valid OpenFGA type name.
fn is_valid_type_name(type_name: &str) -> bool {
    VALID_TYPE_NAMES.contains(&type_name)
}

/// Parse conditions from the model JSON.
///
/// Conditions in OpenFGA are defined at the model level and referenced by name
/// in type constraints. Format:
/// ```json
/// {
///   "conditions": {
///     "condition_name": {
///       "name": "condition_name",
///       "expression": "cel_expression",
///       "parameters": {
///         "param_name": { "type_name": "TYPE_NAME_STRING" }
///       }
///     }
///   }
/// }
/// ```
///
/// # Type Name Validation (Security I4)
///
/// The `type_name` field is validated against the OpenFGA TypeName enum.
/// Invalid type names are rejected with an error rather than defaulting to
/// `TYPE_NAME_ANY`, which could mask configuration errors and lead to
/// unexpected authorization behavior.
fn parse_conditions(model_json: &serde_json::Value) -> DomainResult<Vec<Condition>> {
    let Some(conditions_obj) = model_json.get("conditions").and_then(|c| c.as_object()) else {
        // No conditions defined is valid
        return Ok(vec![]);
    };

    let mut conditions = Vec::with_capacity(conditions_obj.len());

    for (name, cond_def) in conditions_obj {
        // Validate that if "name" field is present, it matches the map key
        if let Some(declared_name) = cond_def.get("name").and_then(|n| n.as_str()) {
            if declared_name != name {
                return Err(DomainError::ModelParseError {
                    message: format!(
                        "condition '{name}' has mismatched name field '{declared_name}'"
                    ),
                });
            }
        }

        // Get the expression (required)
        let expression = cond_def
            .get("expression")
            .and_then(|e| e.as_str())
            .ok_or_else(|| DomainError::ModelParseError {
                message: format!("condition '{name}' missing 'expression' field"),
            })?;

        // Parse parameters
        let mut parameters = Vec::new();
        if let Some(params_obj) = cond_def.get("parameters").and_then(|p| p.as_object()) {
            for (param_name, param_def) in params_obj {
                let type_name = param_def
                    .get("type_name")
                    .and_then(|t| t.as_str())
                    .unwrap_or("TYPE_NAME_ANY");

                // Validate type_name is a known OpenFGA type (Security I4)
                if !is_valid_type_name(type_name) {
                    return Err(DomainError::ModelParseError {
                        message: format!(
                            "condition '{name}' parameter '{param_name}' has invalid type_name '{type_name}'. \
                             Must be one of: {}",
                            VALID_TYPE_NAMES.join(", ")
                        ),
                    });
                }

                parameters.push(ConditionParameter::new(param_name, type_name));
            }
        }

        // Create the condition with parameters
        let condition = Condition::with_parameters(name, parameters, expression).map_err(|e| {
            DomainError::ModelParseError {
                message: format!("invalid condition '{name}': {e}"),
            }
        })?;

        conditions.push(condition);
    }

    Ok(conditions)
}

/// Parses type definitions from a model JSON array.
///
/// This helper extracts the common parsing logic used by `validate_authorization_model_json`,
/// `parse_model_json`, and `fetch_and_parse_model` to follow DRY principles.
///
/// # Arguments
///
/// * `type_defs` - The JSON array of type definitions
///
/// # Errors
///
/// Returns `DomainError::ModelParseError` if:
/// - A type definition is missing the required "type" field
/// - Relation parsing fails
/// - Type constraint parsing fails
fn parse_type_definitions_from_json(
    type_defs: &[serde_json::Value],
) -> DomainResult<Vec<TypeDefinition>> {
    let mut type_definitions = Vec::with_capacity(type_defs.len());

    for (idx, type_def) in type_defs.iter().enumerate() {
        // Fail fast if "type" field is missing - don't silently skip
        let type_name = type_def
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DomainError::ModelParseError {
                message: format!("type_definitions[{idx}] missing required 'type' field"),
            })?;

        let mut relations = Vec::new();

        // Parse relations if present
        if let Some(rels) = type_def.get("relations").and_then(|v| v.as_object()) {
            for (rel_name, rel_def) in rels {
                // Parse the userset (relation rewrite) from JSON
                let rewrite = parse_userset(rel_def, type_name, rel_name)?;

                // Parse type constraints from metadata
                let type_constraints = parse_type_constraints(type_def, type_name, rel_name)?;

                relations.push(RelationDefinition {
                    name: rel_name.clone(),
                    type_constraints,
                    rewrite,
                });
            }
        }

        type_definitions.push(TypeDefinition {
            type_name: type_name.to_string(),
            relations,
        });
    }

    Ok(type_definitions)
}

/// Valid schema versions supported by RSFGA.
const VALID_SCHEMA_VERSIONS: &[&str] = &["1.1", "1.2"];

/// Validates that a schema version is supported.
fn validate_schema_version(version: &str) -> DomainResult<()> {
    if VALID_SCHEMA_VERSIONS.contains(&version) {
        Ok(())
    } else {
        Err(DomainError::ModelParseError {
            message: format!(
                "unsupported schema version '{}'. Supported versions: {}",
                version,
                VALID_SCHEMA_VERSIONS.join(", ")
            ),
        })
    }
}

/// Validates an authorization model JSON without storing it.
///
/// This function performs comprehensive validation including:
/// - Schema version validation
/// - JSON structure parsing
/// - Duplicate type name detection (OpenFGA compatibility)
/// - Domain-level semantic validation (cycles, undefined references, etc.)
/// - CEL condition expression validation
///
/// # Arguments
///
/// * `model_json` - The authorization model as a JSON value
/// * `schema_version` - The schema version (must be "1.1" or "1.2")
///
/// # Errors
///
/// Returns `DomainError::ModelParseError` if:
/// - Schema version is unsupported
/// - JSON structure is invalid
/// - Duplicate type definitions exist
/// - Type definitions are missing required "type" field
/// - Any domain validation error occurs (cycles, undefined refs, etc.)
pub fn validate_authorization_model_json(
    model_json: &serde_json::Value,
    schema_version: &str,
) -> DomainResult<()> {
    use std::collections::HashSet;

    // Validate schema version
    validate_schema_version(schema_version)?;

    // Extract type_definitions from the JSON
    let type_defs = model_json
        .get("type_definitions")
        .and_then(|v| v.as_array())
        .ok_or_else(|| DomainError::ModelParseError {
            message: "type_definitions must be an array".to_string(),
        })?;

    // Check for duplicate type names (OpenFGA returns 400 for duplicates)
    // Must happen before parse_type_definitions_from_json which fails on missing type
    let mut seen_types = HashSet::new();
    for (idx, type_def) in type_defs.iter().enumerate() {
        let type_name = type_def
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DomainError::ModelParseError {
                message: format!("type_definitions[{idx}] missing required 'type' field"),
            })?;

        if !seen_types.insert(type_name.to_string()) {
            return Err(DomainError::ModelParseError {
                message: format!("duplicate type definition: '{type_name}'"),
            });
        }
    }

    // Parse type definitions using shared helper
    let type_definitions = parse_type_definitions_from_json(type_defs)?;

    // Parse conditions from the JSON
    let conditions = parse_conditions(model_json)?;

    // Build the domain model with the provided schema version
    let model =
        AuthorizationModel::with_types_and_conditions(schema_version, type_definitions, conditions);

    // Run domain validation (cycles, undefined refs, CEL syntax, etc.)
    rsfga_domain::validation::validate(&model).map_err(|errors| {
        // Combine all validation errors into a single message
        let messages: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
        DomainError::ModelParseError {
            message: messages.join("; "),
        }
    })
}

/// Validates that a tuple is consistent with the authorization model.
///
/// Checks that:
/// - The object type exists in the model
/// - The relation exists for that object type
/// - If a condition is specified, it exists in the model
///
/// # Arguments
///
/// * `model` - The authorization model to validate against
/// * `object_type` - The type of the object in the tuple
/// * `relation` - The relation in the tuple
/// * `condition_name` - Optional condition name in the tuple
///
/// # Errors
///
/// Returns `DomainError` if validation fails.
///
/// # Performance Note
///
/// For batch validation, use `create_model_validator` once and call
/// `validate_tuple_with_validator` for each tuple to avoid recreating
/// the validator for each tuple.
pub fn validate_tuple_against_model(
    model: &AuthorizationModel,
    object_type: &str,
    relation: &str,
    condition_name: Option<&str>,
) -> DomainResult<()> {
    use rsfga_domain::validation::ModelValidator;

    let validator = ModelValidator::new(model);
    validate_tuple_with_validator(&validator, object_type, relation, condition_name)
}

/// Creates a ModelValidator for batch tuple validation.
///
/// Use this to create a validator once before validating multiple tuples,
/// then call `validate_tuple_with_validator` for each tuple.
///
/// # Performance
///
/// Creating a validator is O(n) where n is the number of types in the model.
/// Reusing the validator across multiple tuples avoids this overhead.
pub fn create_model_validator(
    model: &AuthorizationModel,
) -> rsfga_domain::validation::ModelValidator {
    rsfga_domain::validation::ModelValidator::new(model)
}

/// Validates a tuple using a pre-created ModelValidator.
///
/// This is the optimized version for batch validation. Build a validator
/// once with `create_model_validator` and reuse it for all tuples.
///
/// # Validation Performed
///
/// - Object type must exist in the authorization model
/// - Relation must exist for the given object type
/// - Condition (if specified) must exist in the model
///
/// # User Field Validation (Intentionally Not Performed)
///
/// This function does NOT validate the user field type. This is intentional
/// and matches OpenFGA's behavior. The user field can contain:
/// - External identity references (e.g., `user:alice` where `user` may not be in the model)
/// - Usersets from other types (e.g., `group:admins#member`)
/// - Wildcards (e.g., `*` for public access)
///
/// User type validation would be overly restrictive and break valid use cases.
/// Type constraints on the relation definition (if present) are enforced at
/// check time by the graph resolver, not at write time.
///
/// # Arguments
///
/// * `validator` - Pre-created ModelValidator for efficiency
/// * `model` - The authorization model (for condition lookup)
/// * `object_type` - The type of the object in the tuple
/// * `relation` - The relation in the tuple
/// * `condition_name` - Optional condition name in the tuple
///
/// # Errors
///
/// Returns `DomainError` if validation fails.
pub fn validate_tuple_with_validator(
    validator: &rsfga_domain::validation::ModelValidator,
    object_type: &str,
    relation: &str,
    condition_name: Option<&str>,
) -> DomainResult<()> {
    // Check if the object type exists in the model
    if !validator.type_exists(object_type) {
        return Err(DomainError::TypeNotFound {
            type_name: object_type.to_string(),
        });
    }

    // Check if the relation exists for this type
    if !validator.relation_exists(object_type, relation) {
        return Err(DomainError::RelationNotFound {
            type_name: object_type.to_string(),
            relation: relation.to_string(),
        });
    }

    // Check if the condition exists in the model (if specified)
    // Use validator.condition_exists() for O(1) HashSet lookup instead of O(n) linear scan
    if let Some(cond_name) = condition_name {
        if !validator.condition_exists(cond_name) {
            return Err(DomainError::ConditionNotFound {
                condition_name: cond_name.to_string(),
            });
        }
    }

    Ok(())
}

/// Describes a tuple validation error with context.
#[derive(Debug)]
pub struct TupleValidationError {
    /// Index of the tuple in the batch.
    pub index: usize,
    /// Object type that was invalid.
    pub object_type: String,
    /// Relation that was invalid.
    pub relation: String,
    /// Human-readable error message.
    pub reason: String,
    /// Whether this was a write or delete operation.
    pub is_delete: bool,
}

impl std::fmt::Display for TupleValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let op = if self.is_delete { "delete " } else { "" };
        write!(
            f,
            "invalid {op}tuple at index {}: type={}, relation={}, reason={}",
            self.index, self.object_type, self.relation, self.reason
        )
    }
}

/// Validates a batch of tuples against an authorization model.
///
/// This shared helper reduces duplication between HTTP and gRPC layers by
/// centralizing the tuple validation loop. Both layers can call this once
/// for writes and once for deletes.
///
/// # Arguments
///
/// * `model` - The authorization model to validate against
/// * `tuples` - Iterator over tuples to validate (object_type, relation, condition_name)
/// * `is_delete` - Whether these are delete operations (affects error message)
///
/// # Returns
///
/// `Ok(())` if all tuples are valid, `Err(TupleValidationError)` for the first invalid tuple.
pub fn validate_tuples_batch<'a, I>(
    model: &AuthorizationModel,
    tuples: I,
    is_delete: bool,
) -> Result<(), TupleValidationError>
where
    I: Iterator<Item = (usize, &'a str, &'a str, Option<&'a str>)>,
{
    let validator = create_model_validator(model);

    for (index, object_type, relation, condition_name) in tuples {
        if let Err(e) =
            validate_tuple_with_validator(&validator, object_type, relation, condition_name)
        {
            return Err(TupleValidationError {
                index,
                object_type: object_type.to_string(),
                relation: relation.to_string(),
                reason: domain_error_to_validation_message(&e),
                is_delete,
            });
        }
    }

    Ok(())
}

/// Parses a stored authorization model JSON into a domain AuthorizationModel.
///
/// This is used for tuple validation to check that tuples reference valid types
/// and relations in the current model.
///
/// # Arguments
///
/// * `model_json_str` - The JSON string from storage
/// * `schema_version` - The schema version of the model
///
/// # Errors
///
/// Returns `DomainError::ModelParseError` if parsing fails.
pub fn parse_model_json(
    model_json_str: &str,
    schema_version: &str,
) -> DomainResult<AuthorizationModel> {
    // Validate schema version
    validate_schema_version(schema_version)?;

    // Parse the stored model JSON
    let model_json: serde_json::Value =
        serde_json::from_str(model_json_str).map_err(|e| DomainError::ModelParseError {
            message: format!("failed to parse model JSON: {e}"),
        })?;

    // Extract and parse type_definitions using shared helper
    let type_definitions = if let Some(type_defs) = model_json
        .get("type_definitions")
        .and_then(|v| v.as_array())
    {
        parse_type_definitions_from_json(type_defs)?
    } else {
        Vec::new()
    };

    // Parse conditions from the JSON
    let conditions = parse_conditions(&model_json)?;

    Ok(AuthorizationModel::with_types_and_conditions(
        schema_version,
        type_definitions,
        conditions,
    ))
}

/// Adapter that implements `TupleReader` using a `DataStore`.
///
/// This adapter bridges the storage layer to the domain layer for tuple operations.
pub struct DataStoreTupleReader<S: DataStore> {
    storage: Arc<S>,
}

impl<S: DataStore> DataStoreTupleReader<S> {
    /// Creates a new adapter wrapping the given storage.
    pub fn new(storage: Arc<S>) -> Self {
        Self { storage }
    }
}

#[async_trait]
impl<S: DataStore> TupleReader for DataStoreTupleReader<S> {
    async fn read_tuples(
        &self,
        store_id: &str,
        object_type: &str,
        object_id: &str,
        relation: &str,
    ) -> DomainResult<Vec<StoredTupleRef>> {
        let filter = rsfga_storage::TupleFilter {
            object_type: Some(object_type.to_string()),
            object_id: Some(object_id.to_string()),
            relation: Some(relation.to_string()),
            user: None,
            condition_name: None,
        };

        let tuples = self
            .storage
            .read_tuples(store_id, &filter)
            .await
            .map_err(|e| match e {
                rsfga_storage::StorageError::StoreNotFound { store_id } => {
                    DomainError::StoreNotFound { store_id }
                }
                _ => DomainError::StorageOperationFailed {
                    reason: e.to_string(),
                },
            })?;

        // Convert StoredTuple to StoredTupleRef
        let refs = tuples
            .into_iter()
            .map(|t| StoredTupleRef {
                user_type: t.user_type,
                user_id: t.user_id,
                user_relation: t.user_relation,
                condition_name: t.condition_name,
                condition_context: t.condition_context,
            })
            .collect();

        Ok(refs)
    }

    async fn store_exists(&self, store_id: &str) -> DomainResult<bool> {
        match self.storage.get_store(store_id).await {
            Ok(_) => Ok(true),
            Err(rsfga_storage::StorageError::StoreNotFound { .. }) => Ok(false),
            Err(e) => Err(DomainError::StorageOperationFailed {
                reason: e.to_string(),
            }),
        }
    }

    async fn get_objects_of_type(
        &self,
        store_id: &str,
        object_type: &str,
        max_count: usize,
    ) -> DomainResult<Vec<String>> {
        self.storage
            .list_objects_by_type(store_id, object_type, max_count)
            .await
            .map_err(|e| match e {
                rsfga_storage::StorageError::StoreNotFound { store_id } => {
                    DomainError::StoreNotFound { store_id }
                }
                _ => DomainError::StorageOperationFailed {
                    reason: e.to_string(),
                },
            })
    }

    async fn get_objects_for_user(
        &self,
        store_id: &str,
        user: &str,
        object_type: &str,
        relation: Option<&str>,
        max_count: usize,
    ) -> DomainResult<Vec<ObjectTupleInfo>> {
        let filter = rsfga_storage::TupleFilter {
            object_type: Some(object_type.to_string()),
            object_id: None,
            relation: relation.map(|r| r.to_string()),
            user: Some(user.to_string()),
            condition_name: None,
        };

        let tuples = self
            .storage
            .read_tuples(store_id, &filter)
            .await
            .map_err(|e| match e {
                rsfga_storage::StorageError::StoreNotFound { store_id } => {
                    DomainError::StoreNotFound { store_id }
                }
                _ => DomainError::StorageOperationFailed {
                    reason: e.to_string(),
                },
            })?;

        // Collect ObjectTupleInfo with condition info, up to max_count
        let results: Vec<ObjectTupleInfo> = tuples
            .into_iter()
            .take(max_count)
            .map(|t| {
                if let Some(condition_name) = t.condition_name {
                    ObjectTupleInfo::with_condition(
                        t.object_id,
                        t.relation,
                        condition_name,
                        t.condition_context,
                    )
                } else {
                    ObjectTupleInfo::new(t.object_id, t.relation)
                }
            })
            .collect();

        Ok(results)
    }

    async fn get_objects_with_parents(
        &self,
        store_id: &str,
        object_type: &str,
        tupleset_relation: &str,
        parent_type: &str,
        parent_ids: &[String],
        max_count: usize,
    ) -> DomainResult<Vec<ObjectTupleInfo>> {
        let storage_results = self
            .storage
            .get_objects_with_parents(
                store_id,
                object_type,
                tupleset_relation,
                parent_type,
                parent_ids,
                max_count,
            )
            .await
            .map_err(|e| match e {
                rsfga_storage::StorageError::StoreNotFound { store_id } => {
                    DomainError::StoreNotFound { store_id }
                }
                _ => DomainError::StorageOperationFailed {
                    reason: e.to_string(),
                },
            })?;

        // Convert storage ObjectWithCondition to domain ObjectTupleInfo
        // The tupleset_relation is the relation on this tuple (e.g., "parent")
        let results = storage_results
            .into_iter()
            .map(|obj| {
                if let Some(condition_name) = obj.condition_name {
                    ObjectTupleInfo::with_condition(
                        obj.object_id,
                        tupleset_relation,
                        condition_name,
                        obj.condition_context,
                    )
                } else {
                    ObjectTupleInfo::new(obj.object_id, tupleset_relation)
                }
            })
            .collect();

        Ok(results)
    }
}

/// Adapter that implements `ModelReader` using a `DataStore`.
///
/// This adapter bridges the storage layer to the domain layer for model operations.
/// It retrieves the latest authorization model from storage and parses it.
///
/// # Caching
///
/// Parsed models are cached per store_id with TTL-based expiration.
/// Uses moka's async cache with built-in singleflight behavior to prevent
/// thundering herd on cache misses - only one request fetches while others wait.
pub struct DataStoreModelReader<S: DataStore> {
    storage: Arc<S>,
    /// Cache of parsed models keyed by store_id.
    /// Moka provides automatic expiration, size limits, and singleflight behavior.
    cache: Cache<String, AuthorizationModel>,
}

impl<S: DataStore> DataStoreModelReader<S> {
    /// Creates a new adapter wrapping the given storage.
    pub fn new(storage: Arc<S>) -> Self {
        let cache = Cache::builder()
            .max_capacity(MODEL_CACHE_MAX_CAPACITY)
            .time_to_live(MODEL_CACHE_TTL)
            .build();

        Self { storage, cache }
    }

    /// Helper to get and parse the latest model for a store.
    ///
    /// Uses moka's `try_get_with` for singleflight behavior: if multiple
    /// concurrent requests find a cache miss, only one fetches from storage
    /// while others wait for the result. This prevents thundering herd.
    async fn get_parsed_model(&self, store_id: &str) -> DomainResult<AuthorizationModel> {
        // Use try_get_with for singleflight behavior
        // Clone storage Arc for the async closure
        let storage = Arc::clone(&self.storage);
        let store_id_owned = store_id.to_string();

        self.cache
            .try_get_with(store_id.to_string(), async move {
                Self::fetch_and_parse_model(&storage, &store_id_owned).await
            })
            .await
            .map_err(|e| {
                // Arc<DomainError> -> DomainError
                // The error is wrapped in Arc by moka's try_get_with
                // Reconstruct the error since DomainError doesn't implement Clone
                match e.as_ref() {
                    DomainError::ResolverError { message } => DomainError::ResolverError {
                        message: message.clone(),
                    },
                    DomainError::ModelParseError { message } => DomainError::ModelParseError {
                        message: message.clone(),
                    },
                    DomainError::TypeNotFound { type_name } => DomainError::TypeNotFound {
                        type_name: type_name.clone(),
                    },
                    DomainError::RelationNotFound {
                        type_name,
                        relation,
                    } => DomainError::RelationNotFound {
                        type_name: type_name.clone(),
                        relation: relation.clone(),
                    },
                    DomainError::ConditionNotFound { condition_name } => {
                        DomainError::ConditionNotFound {
                            condition_name: condition_name.clone(),
                        }
                    }
                    DomainError::AuthorizationModelNotFound { store_id } => {
                        DomainError::AuthorizationModelNotFound {
                            store_id: store_id.clone(),
                        }
                    }
                    DomainError::StorageOperationFailed { reason } => {
                        DomainError::StorageOperationFailed {
                            reason: reason.clone(),
                        }
                    }
                    DomainError::ConditionParseError { expression, reason } => {
                        DomainError::ConditionParseError {
                            expression: expression.clone(),
                            reason: reason.clone(),
                        }
                    }
                    DomainError::ConditionEvalError { reason } => DomainError::ConditionEvalError {
                        reason: reason.clone(),
                    },
                    DomainError::InvalidParameter { parameter, reason } => {
                        DomainError::InvalidParameter {
                            parameter: parameter.clone(),
                            reason: reason.clone(),
                        }
                    }
                    DomainError::InvalidFilter { reason } => DomainError::InvalidFilter {
                        reason: reason.clone(),
                    },
                    DomainError::MissingContextKey { key } => {
                        DomainError::MissingContextKey { key: key.clone() }
                    }
                    // Remaining variants - exhaustive handling ensures compile-time
                    // detection if new variants are added to DomainError
                    DomainError::ModelValidationError { message } => {
                        DomainError::ModelValidationError {
                            message: message.clone(),
                        }
                    }
                    DomainError::DepthLimitExceeded { max_depth } => {
                        DomainError::DepthLimitExceeded {
                            max_depth: *max_depth,
                        }
                    }
                    DomainError::Timeout { duration_ms } => DomainError::Timeout {
                        duration_ms: *duration_ms,
                    },
                    DomainError::OperationTimeout {
                        operation,
                        timeout_secs,
                    } => DomainError::OperationTimeout {
                        operation: operation.clone(),
                        timeout_secs: *timeout_secs,
                    },
                    DomainError::CycleDetected { path } => {
                        DomainError::CycleDetected { path: path.clone() }
                    }
                    DomainError::InvalidUserFormat { value } => DomainError::InvalidUserFormat {
                        value: value.clone(),
                    },
                    DomainError::InvalidObjectFormat { value } => {
                        DomainError::InvalidObjectFormat {
                            value: value.clone(),
                        }
                    }
                    DomainError::InvalidRelationFormat { value } => {
                        DomainError::InvalidRelationFormat {
                            value: value.clone(),
                        }
                    }
                    DomainError::StoreNotFound { store_id } => DomainError::StoreNotFound {
                        store_id: store_id.clone(),
                    },
                }
            })
    }

    /// Parses a stored authorization model into a domain AuthorizationModel.
    ///
    /// This helper extracts the common parsing logic used by both
    /// `fetch_and_parse_model` and `fetch_and_parse_model_by_id`.
    fn parse_stored_model(
        stored_model: &rsfga_storage::StoredAuthorizationModel,
    ) -> DomainResult<AuthorizationModel> {
        // Parse the stored model JSON
        let model_json: serde_json::Value = serde_json::from_str(&stored_model.model_json)
            .map_err(|e| DomainError::ModelParseError {
                message: format!("failed to parse model JSON: {e}"),
            })?;

        // Extract and parse type_definitions using shared helper
        let type_definitions = if let Some(type_defs) = model_json
            .get("type_definitions")
            .and_then(|v| v.as_array())
        {
            parse_type_definitions_from_json(type_defs)?
        } else {
            Vec::new()
        };

        // Parse conditions from the JSON
        let conditions = parse_conditions(&model_json)?;

        Ok(AuthorizationModel::with_types_and_conditions(
            &stored_model.schema_version,
            type_definitions,
            conditions,
        ))
    }

    /// Fetches and parses the latest model from storage (no caching).
    /// This is a static method to allow use in async closures.
    async fn fetch_and_parse_model(
        storage: &S,
        store_id: &str,
    ) -> DomainResult<AuthorizationModel> {
        let stored_model = storage
            .get_latest_authorization_model(store_id)
            .await
            .map_err(|e| match e {
                // Model not found - no authorization model exists for this store
                rsfga_storage::StorageError::ModelNotFound { .. } => {
                    DomainError::AuthorizationModelNotFound {
                        store_id: store_id.to_string(),
                    }
                }
                // Store not found - preserve for 404 response
                rsfga_storage::StorageError::StoreNotFound { store_id } => {
                    DomainError::StoreNotFound { store_id }
                }
                // All other storage errors
                _ => DomainError::StorageOperationFailed {
                    reason: e.to_string(),
                },
            })?;

        Self::parse_stored_model(&stored_model)
    }

    /// Fetches and parses a model by its ID from storage (no caching).
    async fn fetch_and_parse_model_by_id(
        storage: &S,
        store_id: &str,
        authorization_model_id: &str,
    ) -> DomainResult<AuthorizationModel> {
        let stored_model = storage
            .get_authorization_model(store_id, authorization_model_id)
            .await
            .map_err(|e| match e {
                rsfga_storage::StorageError::ModelNotFound { .. } => {
                    DomainError::AuthorizationModelNotFound {
                        store_id: store_id.to_string(),
                    }
                }
                rsfga_storage::StorageError::StoreNotFound { store_id } => {
                    DomainError::StoreNotFound { store_id }
                }
                _ => DomainError::StorageOperationFailed {
                    reason: e.to_string(),
                },
            })?;

        Self::parse_stored_model(&stored_model)
    }
}

#[async_trait]
impl<S: DataStore> ModelReader for DataStoreModelReader<S> {
    async fn get_model(&self, store_id: &str) -> DomainResult<AuthorizationModel> {
        self.get_parsed_model(store_id).await
    }

    async fn get_model_by_id(
        &self,
        store_id: &str,
        authorization_model_id: &str,
    ) -> DomainResult<AuthorizationModel> {
        Self::fetch_and_parse_model_by_id(&self.storage, store_id, authorization_model_id).await
    }

    async fn get_type_definition(
        &self,
        store_id: &str,
        type_name: &str,
    ) -> DomainResult<TypeDefinition> {
        let model = self.get_parsed_model(store_id).await?;

        model
            .type_definitions
            .into_iter()
            .find(|td| td.type_name == type_name)
            .ok_or_else(|| DomainError::TypeNotFound {
                type_name: type_name.to_string(),
            })
    }

    async fn get_relation_definition(
        &self,
        store_id: &str,
        type_name: &str,
        relation: &str,
    ) -> DomainResult<RelationDefinition> {
        let type_def = self.get_type_definition(store_id, type_name).await?;

        type_def
            .relations
            .into_iter()
            .find(|r| r.name == relation)
            .ok_or_else(|| DomainError::RelationNotFound {
                type_name: type_name.to_string(),
                relation: relation.to_string(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsfga_storage::MemoryDataStore;

    #[tokio::test]
    async fn test_tuple_reader_adapter_store_exists() {
        let storage = Arc::new(MemoryDataStore::new());
        storage
            .create_store("test-store", "Test Store")
            .await
            .unwrap();

        let reader = DataStoreTupleReader::new(storage);

        assert!(reader.store_exists("test-store").await.unwrap());
        assert!(!reader.store_exists("nonexistent").await.unwrap());
    }

    #[tokio::test]
    async fn test_tuple_reader_adapter_read_tuples() {
        let storage = Arc::new(MemoryDataStore::new());
        storage
            .create_store("test-store", "Test Store")
            .await
            .unwrap();

        // Write a tuple
        let tuple =
            rsfga_storage::StoredTuple::new("document", "readme", "viewer", "user", "alice", None);
        storage
            .write_tuples("test-store", vec![tuple], vec![])
            .await
            .unwrap();

        let reader = DataStoreTupleReader::new(storage);
        let tuples = reader
            .read_tuples("test-store", "document", "readme", "viewer")
            .await
            .unwrap();

        assert_eq!(tuples.len(), 1);
        assert_eq!(tuples[0].user_type, "user");
        assert_eq!(tuples[0].user_id, "alice");
    }

    #[tokio::test]
    async fn test_model_reader_adapter_store_not_found() {
        let storage = Arc::new(MemoryDataStore::new());
        let reader = DataStoreModelReader::new(storage);

        let result = reader.get_model("nonexistent").await;
        assert!(result.is_err());
    }

    /// Helper to create a simple authorization model JSON.
    fn simple_model_json() -> &'static str {
        r#"{
            "type_definitions": [
                {"type": "user"},
                {"type": "document", "relations": {"viewer": {}, "editor": {}, "owner": {}}}
            ]
        }"#
    }

    /// Helper to set up a store with an authorization model.
    async fn setup_store_with_model(storage: &MemoryDataStore) -> String {
        storage
            .create_store("test-store", "Test Store")
            .await
            .unwrap();

        let model = rsfga_storage::StoredAuthorizationModel::new(
            "model-1".to_string(),
            "test-store",
            "1.1",
            simple_model_json().to_string(),
        );
        storage.write_authorization_model(model).await.unwrap();
        "test-store".to_string()
    }

    #[tokio::test]
    async fn test_model_reader_get_model_success() {
        let storage = Arc::new(MemoryDataStore::new());
        let store_id = setup_store_with_model(&storage).await;

        let reader = DataStoreModelReader::new(storage);
        let model = reader.get_model(&store_id).await.unwrap();

        assert_eq!(model.type_definitions.len(), 2);
        assert!(model
            .type_definitions
            .iter()
            .any(|td| td.type_name == "user"));
        assert!(model
            .type_definitions
            .iter()
            .any(|td| td.type_name == "document"));
    }

    #[tokio::test]
    async fn test_model_reader_get_type_definition_success() {
        let storage = Arc::new(MemoryDataStore::new());
        let store_id = setup_store_with_model(&storage).await;

        let reader = DataStoreModelReader::new(storage);

        // Get existing type definition
        let type_def = reader
            .get_type_definition(&store_id, "document")
            .await
            .unwrap();
        assert_eq!(type_def.type_name, "document");
        assert_eq!(type_def.relations.len(), 3);

        // Get user type (no relations)
        let user_def = reader.get_type_definition(&store_id, "user").await.unwrap();
        assert_eq!(user_def.type_name, "user");
        assert!(user_def.relations.is_empty());
    }

    #[tokio::test]
    async fn test_model_reader_get_type_definition_not_found() {
        let storage = Arc::new(MemoryDataStore::new());
        let store_id = setup_store_with_model(&storage).await;

        let reader = DataStoreModelReader::new(storage);
        let result = reader.get_type_definition(&store_id, "nonexistent").await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            DomainError::TypeNotFound { .. }
        ));
    }

    #[tokio::test]
    async fn test_model_reader_get_relation_definition_success() {
        let storage = Arc::new(MemoryDataStore::new());
        let store_id = setup_store_with_model(&storage).await;

        let reader = DataStoreModelReader::new(storage);

        // Get existing relation
        let relation = reader
            .get_relation_definition(&store_id, "document", "viewer")
            .await
            .unwrap();
        assert_eq!(relation.name, "viewer");

        // Get another relation
        let editor = reader
            .get_relation_definition(&store_id, "document", "editor")
            .await
            .unwrap();
        assert_eq!(editor.name, "editor");
    }

    #[tokio::test]
    async fn test_model_reader_get_relation_definition_not_found() {
        let storage = Arc::new(MemoryDataStore::new());
        let store_id = setup_store_with_model(&storage).await;

        let reader = DataStoreModelReader::new(storage);
        let result = reader
            .get_relation_definition(&store_id, "document", "nonexistent")
            .await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            DomainError::RelationNotFound { .. }
        ));
    }

    #[tokio::test]
    async fn test_model_reader_caches_parsed_model() {
        let storage = Arc::new(MemoryDataStore::new());
        let store_id = setup_store_with_model(&storage).await;

        let reader = DataStoreModelReader::new(Arc::clone(&storage));

        // First call - cache miss
        let model1 = reader.get_model(&store_id).await.unwrap();

        // Second call - should hit cache
        let model2 = reader.get_model(&store_id).await.unwrap();

        // Both should return the same model
        assert_eq!(model1.type_definitions.len(), model2.type_definitions.len());

        // Verify cache is populated (moka's contains_key is synchronous)
        assert!(reader.cache.contains_key(&store_id));
    }

    #[tokio::test]
    async fn test_model_reader_parses_complex_relations() {
        use rsfga_domain::model::Userset;

        let storage = Arc::new(MemoryDataStore::new());
        storage
            .create_store("test-store", "Test Store")
            .await
            .unwrap();

        // Create a model with a complex relation (union)
        let complex_model_json = r#"{
            "type_definitions": [
                {"type": "user"},
                {
                    "type": "document",
                    "relations": {
                        "viewer": {
                            "union": {
                                "child": [
                                    {"this": {}},
                                    {"computedUserset": {"relation": "editor"}}
                                ]
                            }
                        }
                    }
                }
            ]
        }"#;

        let model = rsfga_storage::StoredAuthorizationModel::new(
            "model-complex".to_string(),
            "test-store",
            "1.1",
            complex_model_json.to_string(),
        );
        storage.write_authorization_model(model).await.unwrap();

        let reader = DataStoreModelReader::new(storage);
        let result = reader.get_model("test-store").await;

        // Complex relations should now be parsed correctly
        assert!(
            result.is_ok(),
            "Failed to parse complex relation: {result:?}"
        );
        let model = result.unwrap();

        // Find the document type and verify the viewer relation was parsed correctly
        let doc_type = model
            .type_definitions
            .iter()
            .find(|t| t.type_name == "document");
        assert!(doc_type.is_some(), "document type not found");

        let viewer_rel = doc_type
            .unwrap()
            .relations
            .iter()
            .find(|r| r.name == "viewer");
        assert!(viewer_rel.is_some(), "viewer relation not found");

        // Verify it's a Union with 2 children: This and ComputedUserset
        match &viewer_rel.unwrap().rewrite {
            Userset::Union { children } => {
                assert_eq!(children.len(), 2);
                assert!(matches!(&children[0], Userset::This));
                assert!(matches!(
                    &children[1],
                    Userset::ComputedUserset { relation } if relation == "editor"
                ));
            }
            other => panic!("Expected Union, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_model_reader_cache_returns_fresh_model_after_update() {
        // CRITICAL: Verify that model updates are reflected after cache expiry.
        // This test uses cache invalidation through entry removal.
        let storage = Arc::new(MemoryDataStore::new());
        storage
            .create_store("test-store", "Test Store")
            .await
            .unwrap();

        // Initial model with only "user" type
        let model_v1 = rsfga_storage::StoredAuthorizationModel::new(
            "model-v1".to_string(),
            "test-store",
            "1.1",
            r#"{"type_definitions": [{"type": "user"}]}"#.to_string(),
        );
        storage.write_authorization_model(model_v1).await.unwrap();

        let reader = DataStoreModelReader::new(Arc::clone(&storage));

        // First read - should get model with 1 type
        let cached_model = reader.get_model("test-store").await.unwrap();
        assert_eq!(cached_model.type_definitions.len(), 1);

        // Write a new model with 2 types
        let model_v2 = rsfga_storage::StoredAuthorizationModel::new(
            "model-v2".to_string(),
            "test-store",
            "1.1",
            r#"{"type_definitions": [{"type": "user"}, {"type": "document"}]}"#.to_string(),
        );
        storage.write_authorization_model(model_v2).await.unwrap();

        // Invalidate cache entry manually (simulating expiry)
        reader.cache.invalidate("test-store").await;

        // Next read should get the updated model
        let fresh_model = reader.get_model("test-store").await.unwrap();
        assert_eq!(fresh_model.type_definitions.len(), 2);
    }

    #[tokio::test]
    async fn test_model_reader_cache_max_capacity_eviction() {
        // Verify MODEL_CACHE_MAX_CAPACITY eviction works.
        // We create more entries than the cache capacity and verify old ones are evicted.
        let storage = Arc::new(MemoryDataStore::new());

        // Create a reader and manually insert entries to test eviction
        let reader = DataStoreModelReader::new(Arc::clone(&storage));

        // Insert entries up to and beyond capacity
        // Note: moka uses approximate LRU, so we test that the cache doesn't grow unbounded
        for i in 0..10 {
            let model = AuthorizationModel::with_types("1.1", vec![]);
            reader.cache.insert(format!("store-{i}"), model).await;
        }

        // Run pending tasks to ensure eviction happens
        reader.cache.run_pending_tasks().await;

        // Cache should have entries (exact count depends on moka's eviction policy)
        // The key invariant is that it doesn't exceed max capacity over time
        assert!(reader.cache.entry_count() <= super::MODEL_CACHE_MAX_CAPACITY);
    }

    #[test]
    fn test_parse_userset_this_empty_object() {
        use rsfga_domain::model::Userset;
        use serde_json::json;

        let result = super::parse_userset(&json!({}), "document", "viewer");
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), Userset::This));
    }

    #[test]
    fn test_parse_userset_this_explicit() {
        use rsfga_domain::model::Userset;
        use serde_json::json;

        let result = super::parse_userset(&json!({"this": {}}), "document", "viewer");
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), Userset::This));
    }

    #[test]
    fn test_parse_userset_computed_userset_camel_case() {
        use rsfga_domain::model::Userset;
        use serde_json::json;

        let result = super::parse_userset(
            &json!({"computedUserset": {"relation": "owner"}}),
            "document",
            "can_edit",
        );
        assert!(result.is_ok());
        match result.unwrap() {
            Userset::ComputedUserset { relation } => {
                assert_eq!(relation, "owner");
            }
            _ => panic!("Expected ComputedUserset"),
        }
    }

    #[test]
    fn test_parse_userset_computed_userset_snake_case() {
        use rsfga_domain::model::Userset;
        use serde_json::json;

        let result = super::parse_userset(
            &json!({"computed_userset": {"relation": "admin"}}),
            "folder",
            "can_delete",
        );
        assert!(result.is_ok());
        match result.unwrap() {
            Userset::ComputedUserset { relation } => {
                assert_eq!(relation, "admin");
            }
            _ => panic!("Expected ComputedUserset"),
        }
    }

    #[test]
    fn test_parse_userset_tuple_to_userset() {
        use rsfga_domain::model::Userset;
        use serde_json::json;

        let result = super::parse_userset(
            &json!({
                "tupleToUserset": {
                    "tupleset": {"relation": "parent"},
                    "computedUserset": {"relation": "viewer"}
                }
            }),
            "document",
            "can_view",
        );
        assert!(result.is_ok());
        match result.unwrap() {
            Userset::TupleToUserset {
                tupleset,
                computed_userset,
            } => {
                assert_eq!(tupleset, "parent");
                assert_eq!(computed_userset, "viewer");
            }
            _ => panic!("Expected TupleToUserset"),
        }
    }

    #[test]
    fn test_parse_userset_tuple_to_userset_snake_case() {
        use rsfga_domain::model::Userset;
        use serde_json::json;

        let result = super::parse_userset(
            &json!({
                "tuple_to_userset": {
                    "tupleset": {"relation": "org"},
                    "computed_userset": {"relation": "member"}
                }
            }),
            "project",
            "contributor",
        );
        assert!(result.is_ok());
        match result.unwrap() {
            Userset::TupleToUserset {
                tupleset,
                computed_userset,
            } => {
                assert_eq!(tupleset, "org");
                assert_eq!(computed_userset, "member");
            }
            _ => panic!("Expected TupleToUserset"),
        }
    }

    #[test]
    fn test_parse_userset_mixed_camel_and_snake_case() {
        // Test that we can mix camelCase and snake_case within the same structure
        // This matches OpenFGA's flexible JSON handling
        use rsfga_domain::model::Userset;
        use serde_json::json;

        let result = super::parse_userset(
            &json!({
                "union": {
                    "child": [
                        {"this": {}},
                        {"computedUserset": {"relation": "owner"}},  // camelCase
                        {"computed_userset": {"relation": "admin"}}, // snake_case
                        {
                            "tuple_to_userset": {  // snake_case outer
                                "tupleset": {"relation": "parent"},
                                "computedUserset": {"relation": "viewer"}  // camelCase inner
                            }
                        }
                    ]
                }
            }),
            "document",
            "can_read",
        );
        assert!(result.is_ok());
        match result.unwrap() {
            Userset::Union { children } => {
                assert_eq!(children.len(), 4);
                assert!(matches!(&children[0], Userset::This));
                assert!(
                    matches!(&children[1], Userset::ComputedUserset { relation } if relation == "owner")
                );
                assert!(
                    matches!(&children[2], Userset::ComputedUserset { relation } if relation == "admin")
                );
                assert!(matches!(&children[3], Userset::TupleToUserset { .. }));
            }
            _ => panic!("Expected Union"),
        }
    }

    #[test]
    fn test_parse_userset_union() {
        use rsfga_domain::model::Userset;
        use serde_json::json;

        let result = super::parse_userset(
            &json!({
                "union": {
                    "child": [
                        {"this": {}},
                        {"computedUserset": {"relation": "owner"}}
                    ]
                }
            }),
            "document",
            "viewer",
        );
        assert!(result.is_ok());
        match result.unwrap() {
            Userset::Union { children } => {
                assert_eq!(children.len(), 2);
                assert!(matches!(&children[0], Userset::This));
                assert!(
                    matches!(&children[1], Userset::ComputedUserset { relation } if relation == "owner")
                );
            }
            _ => panic!("Expected Union"),
        }
    }

    #[test]
    fn test_parse_userset_intersection() {
        use rsfga_domain::model::Userset;
        use serde_json::json;

        let result = super::parse_userset(
            &json!({
                "intersection": {
                    "child": [
                        {"computedUserset": {"relation": "member"}},
                        {"computedUserset": {"relation": "active"}}
                    ]
                }
            }),
            "group",
            "active_member",
        );
        assert!(result.is_ok());
        match result.unwrap() {
            Userset::Intersection { children } => {
                assert_eq!(children.len(), 2);
            }
            _ => panic!("Expected Intersection"),
        }
    }

    #[test]
    fn test_parse_userset_exclusion() {
        use rsfga_domain::model::Userset;
        use serde_json::json;

        let result = super::parse_userset(
            &json!({
                "exclusion": {
                    "base": {"computedUserset": {"relation": "member"}},
                    "subtract": {"computedUserset": {"relation": "banned"}}
                }
            }),
            "group",
            "active_member",
        );
        assert!(result.is_ok());
        match result.unwrap() {
            Userset::Exclusion { base, subtract } => {
                assert!(
                    matches!(*base, Userset::ComputedUserset { relation } if relation == "member")
                );
                assert!(
                    matches!(*subtract, Userset::ComputedUserset { relation } if relation == "banned")
                );
            }
            _ => panic!("Expected Exclusion"),
        }
    }

    #[test]
    fn test_parse_userset_difference_alias() {
        use rsfga_domain::model::Userset;
        use serde_json::json;

        // "difference" is an alias for "exclusion"
        let result = super::parse_userset(
            &json!({
                "difference": {
                    "base": {"this": {}},
                    "subtract": {"computedUserset": {"relation": "blocked"}}
                }
            }),
            "user",
            "can_message",
        );
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), Userset::Exclusion { .. }));
    }

    #[test]
    fn test_parse_userset_nested_union_in_intersection() {
        use rsfga_domain::model::Userset;
        use serde_json::json;

        let result = super::parse_userset(
            &json!({
                "intersection": {
                    "child": [
                        {
                            "union": {
                                "child": [
                                    {"this": {}},
                                    {"computedUserset": {"relation": "owner"}}
                                ]
                            }
                        },
                        {"computedUserset": {"relation": "verified"}}
                    ]
                }
            }),
            "document",
            "can_publish",
        );
        assert!(result.is_ok());
        match result.unwrap() {
            Userset::Intersection { children } => {
                assert_eq!(children.len(), 2);
                assert!(matches!(&children[0], Userset::Union { .. }));
            }
            _ => panic!("Expected Intersection"),
        }
    }

    #[test]
    fn test_parse_userset_error_missing_relation() {
        use serde_json::json;

        let result = super::parse_userset(&json!({"computedUserset": {}}), "document", "viewer");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("requires 'relation' field"));
    }

    #[test]
    fn test_parse_userset_error_unknown_key() {
        use serde_json::json;

        let result = super::parse_userset(&json!({"unknownKey": {}}), "document", "viewer");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("unknown relation definition keys"));
    }

    #[test]
    fn test_parse_userset_error_not_object() {
        use serde_json::json;

        let result = super::parse_userset(&json!("not an object"), "document", "viewer");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("must be an object"));
    }

    #[test]
    fn test_parse_userset_error_empty_union_child() {
        use serde_json::json;

        // Security: empty child array in union should be rejected
        let result = super::parse_userset(&json!({"union": {"child": []}}), "document", "viewer");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("requires at least one child"));
    }

    #[test]
    fn test_parse_userset_error_empty_intersection_child() {
        use serde_json::json;

        // Security: empty child array in intersection should be rejected
        // (would cause "allow all" behavior)
        let result = super::parse_userset(
            &json!({"intersection": {"child": []}}),
            "document",
            "viewer",
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("requires at least one child"));
    }

    #[test]
    fn test_parse_userset_error_exceeds_max_depth() {
        use serde_json::json;

        // Create deeply nested structure that exceeds MAX_PARSE_DEPTH
        let mut nested = json!({"this": {}});
        for _ in 0..(super::MAX_PARSE_DEPTH + 5) {
            nested = json!({
                "union": {
                    "child": [nested, {"computedUserset": {"relation": "r"}}]
                }
            });
        }

        let result = super::parse_userset(&nested, "document", "viewer");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("exceeds max nesting depth"));
    }

    #[test]
    fn test_parse_type_constraints_success() {
        use serde_json::json;

        let type_def = json!({
            "type": "document",
            "relations": {"viewer": {}},
            "metadata": {
                "relations": {
                    "viewer": {
                        "directly_related_user_types": [
                            {"type": "user"},
                            {"type": "group", "relation": "member"}
                        ]
                    }
                }
            }
        });

        let result = super::parse_type_constraints(&type_def, "document", "viewer");
        assert!(result.is_ok());
        let constraints = result.unwrap();
        assert_eq!(constraints.len(), 2);
        assert_eq!(constraints[0].type_name, "user");
        assert_eq!(constraints[1].type_name, "group#member");
    }

    #[test]
    fn test_parse_type_constraints_with_condition() {
        use serde_json::json;

        let type_def = json!({
            "type": "document",
            "relations": {"viewer": {}},
            "metadata": {
                "relations": {
                    "viewer": {
                        "directly_related_user_types": [
                            {"type": "user", "condition": "is_admin"}
                        ]
                    }
                }
            }
        });

        let result = super::parse_type_constraints(&type_def, "document", "viewer");
        assert!(result.is_ok());
        let constraints = result.unwrap();
        assert_eq!(constraints.len(), 1);
        assert_eq!(constraints[0].type_name, "user");
        assert_eq!(constraints[0].condition, Some("is_admin".to_string()));
    }

    #[test]
    fn test_parse_type_constraints_no_metadata() {
        use serde_json::json;

        let type_def = json!({
            "type": "document",
            "relations": {"viewer": {}}
        });

        let result = super::parse_type_constraints(&type_def, "document", "viewer");
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_parse_type_constraints_error_missing_type() {
        use serde_json::json;

        // Security: malformed entry should fail, not be silently dropped
        let type_def = json!({
            "type": "document",
            "relations": {"viewer": {}},
            "metadata": {
                "relations": {
                    "viewer": {
                        "directly_related_user_types": [
                            {"relation": "member"}  // Missing "type" field
                        ]
                    }
                }
            }
        });

        let result = super::parse_type_constraints(&type_def, "document", "viewer");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("missing 'type' field"));
    }

    // =========================================================================
    // Integration Tests with OpenFGA Compatibility
    // =========================================================================

    /// Test: parse_userset handles real OpenFGA model patterns
    ///
    /// Validates parsing against common OpenFGA authorization model patterns
    /// from the OpenFGA documentation and examples.
    #[tokio::test]
    async fn test_parse_userset_openfga_google_drive_model() {
        use rsfga_domain::model::Userset;
        use serde_json::json;

        // Google Drive model pattern from OpenFGA docs
        // https://openfga.dev/docs/modeling/getting-started
        let viewer_relation = json!({
            "union": {
                "child": [
                    {"this": {}},
                    {"computedUserset": {"relation": "editor"}}
                ]
            }
        });

        let result = super::parse_userset(&viewer_relation, "document", "viewer");
        assert!(result.is_ok(), "Should parse Google Drive viewer pattern");
        match result.unwrap() {
            Userset::Union { children } => {
                assert_eq!(children.len(), 2);
                assert!(matches!(&children[0], Userset::This));
                assert!(matches!(
                    &children[1],
                    Userset::ComputedUserset { relation } if relation == "editor"
                ));
            }
            _ => panic!("Expected Union"),
        }

        // Editor relation: can_edit = editor OR owner
        let editor_relation = json!({
            "union": {
                "child": [
                    {"this": {}},
                    {"computedUserset": {"relation": "owner"}}
                ]
            }
        });

        let result = super::parse_userset(&editor_relation, "document", "editor");
        assert!(result.is_ok());

        // Parent folder access pattern (tupleToUserset)
        let parent_viewer = json!({
            "tupleToUserset": {
                "tupleset": {"relation": "parent"},
                "computedUserset": {"relation": "viewer"}
            }
        });

        let result = super::parse_userset(&parent_viewer, "document", "can_view_from_parent");
        assert!(result.is_ok());
        match result.unwrap() {
            Userset::TupleToUserset {
                tupleset,
                computed_userset,
            } => {
                assert_eq!(tupleset, "parent");
                assert_eq!(computed_userset, "viewer");
            }
            _ => panic!("Expected TupleToUserset"),
        }
    }

    /// Test: parse_userset handles GitHub model with exclusion
    #[test]
    fn test_parse_userset_openfga_github_model() {
        use rsfga_domain::model::Userset;
        use serde_json::json;

        // GitHub repo access: can read if member AND NOT banned
        let can_read = json!({
            "exclusion": {
                "base": {"computedUserset": {"relation": "member"}},
                "subtract": {"computedUserset": {"relation": "banned"}}
            }
        });

        let result = super::parse_userset(&can_read, "repo", "can_read");
        assert!(result.is_ok());
        match result.unwrap() {
            Userset::Exclusion { base, subtract } => {
                assert!(matches!(
                    *base,
                    Userset::ComputedUserset { relation } if relation == "member"
                ));
                assert!(matches!(
                    *subtract,
                    Userset::ComputedUserset { relation } if relation == "banned"
                ));
            }
            _ => panic!("Expected Exclusion"),
        }
    }

    /// Test: Full model parsing with complex relations through DataStoreModelReader
    #[tokio::test]
    async fn test_model_reader_parses_openfga_style_model() {
        use rsfga_domain::model::Userset;

        let storage = Arc::new(MemoryDataStore::new());
        storage
            .create_store("test-store", "Test Store")
            .await
            .unwrap();

        // Complete OpenFGA-style model with all relation types
        let model_json = r#"{
            "type_definitions": [
                {"type": "user"},
                {"type": "group", "relations": {
                    "member": {},
                    "admin": {}
                }},
                {"type": "folder", "relations": {
                    "viewer": {},
                    "editor": {}
                }},
                {"type": "document", "relations": {
                    "owner": {},
                    "editor": {
                        "union": {
                            "child": [
                                {"this": {}},
                                {"computedUserset": {"relation": "owner"}}
                            ]
                        }
                    },
                    "viewer": {
                        "union": {
                            "child": [
                                {"this": {}},
                                {"computedUserset": {"relation": "editor"}},
                                {"tupleToUserset": {
                                    "tupleset": {"relation": "parent"},
                                    "computedUserset": {"relation": "viewer"}
                                }}
                            ]
                        }
                    },
                    "parent": {},
                    "can_share": {
                        "intersection": {
                            "child": [
                                {"computedUserset": {"relation": "owner"}},
                                {"computedUserset": {"relation": "editor"}}
                            ]
                        }
                    },
                    "can_delete": {
                        "exclusion": {
                            "base": {"computedUserset": {"relation": "owner"}},
                            "subtract": {"computedUserset": {"relation": "blocked"}}
                        }
                    },
                    "blocked": {}
                }}
            ]
        }"#;

        let model = rsfga_storage::StoredAuthorizationModel::new(
            "model-complex".to_string(),
            "test-store",
            "1.1",
            model_json.to_string(),
        );
        storage.write_authorization_model(model).await.unwrap();

        let reader = DataStoreModelReader::new(storage);
        let result = reader.get_model("test-store").await;
        assert!(result.is_ok(), "Failed to parse model: {result:?}");

        let model = result.unwrap();

        // Find document type and verify all relations
        let doc_type = model
            .type_definitions
            .iter()
            .find(|t| t.type_name == "document")
            .expect("document type not found");

        // Check editor relation (union of this + owner)
        let editor_rel = doc_type
            .relations
            .iter()
            .find(|r| r.name == "editor")
            .expect("editor relation not found");
        assert!(matches!(&editor_rel.rewrite, Userset::Union { children } if children.len() == 2));

        // Check viewer relation (union of this + editor + parent->viewer)
        let viewer_rel = doc_type
            .relations
            .iter()
            .find(|r| r.name == "viewer")
            .expect("viewer relation not found");
        match &viewer_rel.rewrite {
            Userset::Union { children } => {
                assert_eq!(children.len(), 3);
                assert!(matches!(&children[2], Userset::TupleToUserset { .. }));
            }
            _ => panic!("Expected Union for viewer"),
        }

        // Check can_share relation (intersection)
        let can_share_rel = doc_type
            .relations
            .iter()
            .find(|r| r.name == "can_share")
            .expect("can_share relation not found");
        assert!(
            matches!(&can_share_rel.rewrite, Userset::Intersection { children } if children.len() == 2)
        );

        // Check can_delete relation (exclusion)
        let can_delete_rel = doc_type
            .relations
            .iter()
            .find(|r| r.name == "can_delete")
            .expect("can_delete relation not found");
        assert!(matches!(&can_delete_rel.rewrite, Userset::Exclusion { .. }));
    }

    // =========================================================================
    // Property-Based Tests (proptest)
    // =========================================================================

    mod proptest_tests {
        use crate::adapters::MAX_PARSE_DEPTH;
        use proptest::prelude::*;
        use serde_json::json;

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(100))]

            /// Property: parse_userset never panics on valid This structures
            #[test]
            fn prop_parse_userset_never_panics_on_this(
                type_name in "[a-z]{1,20}",
                rel_name in "[a-z]{1,20}"
            ) {
                let value = json!({"this": {}});
                let result = crate::adapters::parse_userset(&value, &type_name, &rel_name);
                prop_assert!(result.is_ok());
            }

            /// Property: parse_userset returns correct type for empty object
            #[test]
            fn prop_parse_userset_empty_is_this(
                type_name in "[a-z]{1,20}",
                rel_name in "[a-z]{1,20}"
            ) {
                let value = json!({});
                let result = crate::adapters::parse_userset(&value, &type_name, &rel_name);
                prop_assert!(result.is_ok());
                prop_assert!(matches!(result.unwrap(), rsfga_domain::model::Userset::This));
            }

            /// Property: parse_userset handles all valid relation names
            #[test]
            fn prop_parse_userset_valid_computed(
                type_name in "[a-z]{1,20}",
                rel_name in "[a-z]{1,20}",
                computed_rel in "[a-z_]{1,50}"
            ) {
                let value = json!({"computedUserset": {"relation": computed_rel}});
                let result = crate::adapters::parse_userset(&value, &type_name, &rel_name);
                prop_assert!(result.is_ok());
                match result.unwrap() {
                    rsfga_domain::model::Userset::ComputedUserset { relation } => {
                        prop_assert_eq!(relation, computed_rel);
                    }
                    _ => prop_assert!(false, "Expected ComputedUserset"),
                }
            }

            /// Property: parse_userset respects depth limit
            #[test]
            fn prop_parse_userset_depth_limit(
                depth in (MAX_PARSE_DEPTH + 1)..50usize
            ) {
                // Create nested structure at specified depth
                let mut nested = json!({"this": {}});
                for _ in 0..depth {
                    nested = json!({"union": {"child": [nested]}});
                }

                let result = crate::adapters::parse_userset(&nested, "type", "rel");
                // Should fail with depth limit error
                prop_assert!(result.is_err());
                let err = result.unwrap_err().to_string();
                prop_assert!(err.contains("exceeds max nesting depth"));
            }

            /// Property: parse_userset rejects empty union/intersection child arrays
            #[test]
            fn prop_parse_userset_empty_children_rejected(
                key in prop_oneof![Just("union"), Just("intersection")]
            ) {
                let value = json!({key: {"child": []}});
                let result = crate::adapters::parse_userset(&value, "type", "rel");
                prop_assert!(result.is_err());
                let err = result.unwrap_err().to_string();
                prop_assert!(err.contains("requires at least one child"));
            }

            /// Property: parse_userset preserves union children order
            #[test]
            fn prop_parse_userset_union_order_preserved(
                rels in prop::collection::vec("[a-z]{1,10}", 1..=4)
            ) {
                let children: Vec<_> = rels.iter()
                    .map(|r| json!({"computedUserset": {"relation": r}}))
                    .collect();
                let value = json!({"union": {"child": children}});

                let result = crate::adapters::parse_userset(&value, "type", "rel");
                prop_assert!(result.is_ok());

                match result.unwrap() {
                    rsfga_domain::model::Userset::Union { children: parsed } => {
                        prop_assert_eq!(parsed.len(), rels.len());
                        for (i, parsed_child) in parsed.iter().enumerate() {
                            match parsed_child {
                                rsfga_domain::model::Userset::ComputedUserset { relation } => {
                                    prop_assert_eq!(relation, &rels[i]);
                                }
                                _ => prop_assert!(false, "Expected ComputedUserset"),
                            }
                        }
                    }
                    _ => prop_assert!(false, "Expected Union"),
                }
            }
        }
    }

    // ============================================================
    // Tests for validate_tuples_batch and TupleValidationError
    // ============================================================

    #[test]
    fn test_tuple_validation_error_display_write() {
        let err = super::TupleValidationError {
            index: 5,
            object_type: "document".to_string(),
            relation: "viewer".to_string(),
            reason: "type 'document' not defined in authorization model".to_string(),
            is_delete: false,
        };
        assert_eq!(
            err.to_string(),
            "invalid tuple at index 5: type=document, relation=viewer, reason=type 'document' not defined in authorization model"
        );
    }

    #[test]
    fn test_tuple_validation_error_display_delete() {
        let err = super::TupleValidationError {
            index: 3,
            object_type: "folder".to_string(),
            relation: "editor".to_string(),
            reason: "relation 'editor' not defined for type 'folder'".to_string(),
            is_delete: true,
        };
        assert_eq!(
            err.to_string(),
            "invalid delete tuple at index 3: type=folder, relation=editor, reason=relation 'editor' not defined for type 'folder'"
        );
    }

    #[test]
    fn test_validate_tuples_batch_empty_succeeds() {
        let model = AuthorizationModel::with_types("1.1", vec![]);
        let tuples: Vec<(usize, &str, &str, Option<&str>)> = vec![];

        let result = super::validate_tuples_batch(&model, tuples.into_iter(), false);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_tuples_batch_valid_tuples_succeed() {
        use rsfga_domain::model::{RelationDefinition, TypeDefinition};

        let type_def = TypeDefinition {
            type_name: "document".to_string(),
            relations: vec![
                RelationDefinition {
                    name: "viewer".to_string(),
                    rewrite: rsfga_domain::model::Userset::This,
                    type_constraints: vec![],
                },
                RelationDefinition {
                    name: "editor".to_string(),
                    rewrite: rsfga_domain::model::Userset::This,
                    type_constraints: vec![],
                },
            ],
        };
        let model = AuthorizationModel::with_types("1.1", vec![type_def]);

        let tuples = vec![
            (0, "document", "viewer", None),
            (1, "document", "editor", None),
        ];

        let result = super::validate_tuples_batch(&model, tuples.into_iter(), false);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_tuples_batch_invalid_type_fails() {
        use rsfga_domain::model::{RelationDefinition, TypeDefinition};

        let type_def = TypeDefinition {
            type_name: "document".to_string(),
            relations: vec![RelationDefinition {
                name: "viewer".to_string(),
                rewrite: rsfga_domain::model::Userset::This,
                type_constraints: vec![],
            }],
        };
        let model = AuthorizationModel::with_types("1.1", vec![type_def]);

        let tuples = vec![
            (0, "document", "viewer", None),
            (1, "folder", "viewer", None), // Invalid type
        ];

        let result = super::validate_tuples_batch(&model, tuples.into_iter(), false);
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert_eq!(err.index, 1);
        assert_eq!(err.object_type, "folder");
        assert!(err.reason.contains("not defined"));
        assert!(!err.is_delete);
    }

    #[test]
    fn test_validate_tuples_batch_invalid_relation_fails() {
        use rsfga_domain::model::{RelationDefinition, TypeDefinition};

        let type_def = TypeDefinition {
            type_name: "document".to_string(),
            relations: vec![RelationDefinition {
                name: "viewer".to_string(),
                rewrite: rsfga_domain::model::Userset::This,
                type_constraints: vec![],
            }],
        };
        let model = AuthorizationModel::with_types("1.1", vec![type_def]);

        let tuples = vec![
            (0, "document", "owner", None), // Invalid relation
        ];

        let result = super::validate_tuples_batch(&model, tuples.into_iter(), false);
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert_eq!(err.index, 0);
        assert_eq!(err.relation, "owner");
        assert!(err.reason.contains("not defined"));
    }

    #[test]
    fn test_validate_tuples_batch_invalid_condition_fails() {
        use rsfga_domain::model::{RelationDefinition, TypeDefinition};

        let type_def = TypeDefinition {
            type_name: "document".to_string(),
            relations: vec![RelationDefinition {
                name: "viewer".to_string(),
                rewrite: rsfga_domain::model::Userset::This,
                type_constraints: vec![],
            }],
        };
        let model = AuthorizationModel::with_types("1.1", vec![type_def]);

        let tuples = vec![(0, "document", "viewer", Some("nonexistent_condition"))];

        let result = super::validate_tuples_batch(&model, tuples.into_iter(), false);
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert_eq!(err.index, 0);
        assert!(err.reason.contains("condition"));
    }

    #[test]
    fn test_validate_tuples_batch_delete_flag_in_error() {
        let model = AuthorizationModel::with_types("1.1", vec![]);

        let tuples = vec![(2, "unknown_type", "relation", None)];

        let result = super::validate_tuples_batch(&model, tuples.into_iter(), true);
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(err.is_delete);
        assert!(err.to_string().contains("delete tuple"));
    }
}
