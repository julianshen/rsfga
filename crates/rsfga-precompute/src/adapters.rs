//! Adapters bridging DataStore to domain resolver traits.
//!
//! The domain layer's `GraphResolver<T, M>` is generic over `TupleReader` and `ModelReader`.
//! The existing adapters live in `rsfga-api`, which we cannot depend on (circular dependency).
//! This module provides minimal, non-cached adapters suitable for the precompute daemon's
//! asynchronous event-driven workload where caching is less critical.
//!
//! These adapters use `Arc<dyn DataStore>` (trait object) rather than being generic over
//! `S: DataStore` because the precompute binary's `create_storage()` returns a type-erased
//! `Arc<dyn DataStore>` depending on runtime configuration (memory, postgres, mysql, rocksdb).

use std::sync::Arc;

use async_trait::async_trait;

use rsfga_domain::error::{DomainError, DomainResult};
use rsfga_domain::model::{
    AuthorizationModel, Condition, ConditionParameter, RelationDefinition, TypeConstraint,
    TypeDefinition, Userset,
};
use rsfga_domain::resolver::{ModelReader, StoredTupleRef, TupleReader};
use rsfga_storage::{DataStore, StorageError, StoredAuthorizationModel, TupleFilter};

use crate::worker::ModelIdProvider;

// ─── TupleReader Adapter ────────────────────────────────────────────────────

/// Adapter implementing `TupleReader` using a `DataStore` trait object.
pub struct StoreTupleReader {
    storage: Arc<dyn DataStore>,
}

impl StoreTupleReader {
    pub fn new(storage: Arc<dyn DataStore>) -> Self {
        Self { storage }
    }
}

#[async_trait]
impl TupleReader for StoreTupleReader {
    async fn read_tuples(
        &self,
        store_id: &str,
        object_type: &str,
        object_id: &str,
        relation: &str,
    ) -> DomainResult<Vec<StoredTupleRef>> {
        let filter = TupleFilter {
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
                StorageError::StoreNotFound { store_id } => DomainError::StoreNotFound { store_id },
                _ => DomainError::StorageOperationFailed {
                    reason: e.to_string(),
                },
            })?;

        Ok(tuples
            .into_iter()
            .map(|t| StoredTupleRef {
                user_type: t.user_type,
                user_id: t.user_id,
                user_relation: t.user_relation,
                condition_name: t.condition_name,
                condition_context: t.condition_context,
            })
            .collect())
    }

    async fn store_exists(&self, store_id: &str) -> DomainResult<bool> {
        match self.storage.get_store(store_id).await {
            Ok(_) => Ok(true),
            Err(StorageError::StoreNotFound { .. }) => Ok(false),
            Err(e) => Err(DomainError::StorageOperationFailed {
                reason: e.to_string(),
            }),
        }
    }
}

// ─── ModelReader Adapter ────────────────────────────────────────────────────

/// Adapter implementing `ModelReader` using a `DataStore` trait object.
///
/// Unlike the API's cached version (`DataStoreModelReader`), this adapter
/// fetches directly from storage on every call. The precompute binary
/// processes events asynchronously so caching is less critical.
pub struct StoreModelReader {
    storage: Arc<dyn DataStore>,
}

impl StoreModelReader {
    pub fn new(storage: Arc<dyn DataStore>) -> Self {
        Self { storage }
    }

    /// Fetches the latest model from storage and parses it.
    async fn fetch_and_parse_model(&self, store_id: &str) -> DomainResult<AuthorizationModel> {
        let stored_model = self
            .storage
            .get_latest_authorization_model(store_id)
            .await
            .map_err(map_storage_to_domain)?;

        parse_stored_model(&stored_model)
    }

    /// Fetches a specific model by ID from storage and parses it.
    async fn fetch_and_parse_model_by_id(
        &self,
        store_id: &str,
        authorization_model_id: &str,
    ) -> DomainResult<AuthorizationModel> {
        let stored_model = self
            .storage
            .get_authorization_model(store_id, authorization_model_id)
            .await
            .map_err(map_storage_to_domain)?;

        parse_stored_model(&stored_model)
    }
}

#[async_trait]
impl ModelReader for StoreModelReader {
    async fn get_model(&self, store_id: &str) -> DomainResult<AuthorizationModel> {
        self.fetch_and_parse_model(store_id).await
    }

    async fn get_model_by_id(
        &self,
        store_id: &str,
        authorization_model_id: &str,
    ) -> DomainResult<AuthorizationModel> {
        self.fetch_and_parse_model_by_id(store_id, authorization_model_id)
            .await
    }

    async fn get_type_definition(
        &self,
        store_id: &str,
        type_name: &str,
    ) -> DomainResult<TypeDefinition> {
        let model = self.fetch_and_parse_model(store_id).await?;

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

// ─── ModelIdProvider Adapter ────────────────────────────────────────────────

/// Adapter implementing `ModelIdProvider` using a `DataStore` trait object.
///
/// Used by the worker pool to fetch the latest authorization model ID
/// when building check keys for Valkey storage.
pub struct StoreModelIdProvider {
    storage: Arc<dyn DataStore>,
}

impl StoreModelIdProvider {
    pub fn new(storage: Arc<dyn DataStore>) -> Self {
        Self { storage }
    }
}

#[async_trait]
impl ModelIdProvider for StoreModelIdProvider {
    async fn get_latest_model_id(&self, store_id: &str) -> Option<String> {
        self.storage
            .get_latest_authorization_model(store_id)
            .await
            .ok()
            .map(|m| m.id)
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Maps storage errors to domain errors for model fetching.
fn map_storage_to_domain(e: StorageError) -> DomainError {
    match e {
        StorageError::ModelNotFound { .. } => DomainError::AuthorizationModelNotFound {
            store_id: String::new(),
        },
        StorageError::StoreNotFound { store_id } => DomainError::StoreNotFound { store_id },
        _ => DomainError::StorageOperationFailed {
            reason: e.to_string(),
        },
    }
}

/// Maximum nesting depth for parsing userset definitions.
/// Matches the resolver's depth limit of 25 for consistency.
const MAX_PARSE_DEPTH: usize = 25;

/// Valid OpenFGA type names for condition parameters.
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

/// Valid schema versions supported by RSFGA.
const VALID_SCHEMA_VERSIONS: &[&str] = &["1.1", "1.2"];

/// Parse a `StoredAuthorizationModel` into a domain `AuthorizationModel`.
///
/// This replicates the parsing logic from `rsfga-api::adapters::parse_stored_model`
/// since we cannot depend on rsfga-api (circular dependency).
fn parse_stored_model(stored_model: &StoredAuthorizationModel) -> DomainResult<AuthorizationModel> {
    // Validate schema version
    if !VALID_SCHEMA_VERSIONS.contains(&stored_model.schema_version.as_str()) {
        return Err(DomainError::ModelParseError {
            message: format!(
                "unsupported schema version '{}'. Supported versions: {}",
                stored_model.schema_version,
                VALID_SCHEMA_VERSIONS.join(", ")
            ),
        });
    }

    // Parse the stored model JSON
    let model_json: serde_json::Value =
        serde_json::from_str(&stored_model.model_json).map_err(|e| {
            DomainError::ModelParseError {
                message: format!("failed to parse model JSON: {e}"),
            }
        })?;

    // Extract and parse type_definitions
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

/// Parse type definitions from a model JSON array.
fn parse_type_definitions_from_json(
    type_defs: &[serde_json::Value],
) -> DomainResult<Vec<TypeDefinition>> {
    let mut type_definitions = Vec::with_capacity(type_defs.len());

    for (idx, type_def) in type_defs.iter().enumerate() {
        let type_name = type_def
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DomainError::ModelParseError {
                message: format!("type_definitions[{idx}] missing required 'type' field"),
            })?;

        let mut relations = Vec::new();

        if let Some(rels) = type_def.get("relations").and_then(|v| v.as_object()) {
            for (rel_name, rel_def) in rels {
                let rewrite = parse_userset(rel_def, type_name, rel_name, 0)?;
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

/// Parse a JSON relation definition into a Userset.
fn parse_userset(
    rel_def: &serde_json::Value,
    type_name: &str,
    rel_name: &str,
    depth: usize,
) -> DomainResult<Userset> {
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
        let children = parse_child_array(union, "union", type_name, rel_name, depth)?;
        return Ok(Userset::Union { children });
    }

    // intersection
    if let Some(intersection) = obj.get("intersection") {
        let children = parse_child_array(intersection, "intersection", type_name, rel_name, depth)?;
        return Ok(Userset::Intersection { children });
    }

    // exclusion or difference
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
            base: Box::new(parse_userset(base, type_name, rel_name, depth + 1)?),
            subtract: Box::new(parse_userset(subtract, type_name, rel_name, depth + 1)?),
        });
    }

    let keys: Vec<_> = obj.keys().collect();
    Err(DomainError::ModelParseError {
        message: format!(
            "unknown relation definition keys {keys:?}: type '{type_name}', relation '{rel_name}'"
        ),
    })
}

/// Parse child arrays for union/intersection.
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

    if children.is_empty() {
        return Err(DomainError::ModelParseError {
            message: format!(
                "{key} requires at least one child: type '{type_name}', relation '{rel_name}'"
            ),
        });
    }

    children
        .iter()
        .map(|c| parse_userset(c, type_name, rel_name, depth + 1))
        .collect()
}

/// Parse type constraints from metadata if present.
fn parse_type_constraints(
    type_def: &serde_json::Value,
    type_name: &str,
    rel_name: &str,
) -> DomainResult<Vec<TypeConstraint>> {
    let Some(arr) = type_def
        .get("metadata")
        .and_then(|m| m.get("relations"))
        .and_then(|r| r.get(rel_name))
        .and_then(|rel_meta| rel_meta.get("directly_related_user_types"))
        .and_then(|v| v.as_array())
    else {
        return Ok(vec![]);
    };

    let mut constraints = Vec::with_capacity(arr.len());
    for (idx, item) in arr.iter().enumerate() {
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

/// Parse conditions from the model JSON.
fn parse_conditions(model_json: &serde_json::Value) -> DomainResult<Vec<Condition>> {
    let Some(conditions_obj) = model_json.get("conditions").and_then(|c| c.as_object()) else {
        return Ok(vec![]);
    };

    let mut conditions = Vec::with_capacity(conditions_obj.len());

    for (name, cond_def) in conditions_obj {
        if let Some(declared_name) = cond_def.get("name").and_then(|n| n.as_str()) {
            if declared_name != name {
                return Err(DomainError::ModelParseError {
                    message: format!(
                        "condition '{name}' has mismatched name field '{declared_name}'"
                    ),
                });
            }
        }

        let expression = cond_def
            .get("expression")
            .and_then(|e| e.as_str())
            .ok_or_else(|| DomainError::ModelParseError {
                message: format!("condition '{name}' missing 'expression' field"),
            })?;

        let mut parameters = Vec::new();
        if let Some(params_obj) = cond_def.get("parameters").and_then(|p| p.as_object()) {
            for (param_name, param_def) in params_obj {
                let type_name = param_def
                    .get("type_name")
                    .and_then(|t| t.as_str())
                    .unwrap_or("TYPE_NAME_ANY");

                if !VALID_TYPE_NAMES.contains(&type_name) {
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

        let condition = Condition::with_parameters(name, parameters, expression).map_err(|e| {
            DomainError::ModelParseError {
                message: format!("invalid condition '{name}': {e}"),
            }
        })?;

        conditions.push(condition);
    }

    Ok(conditions)
}

/// Extract relation dependencies from a parsed authorization model.
///
/// Returns a map of `type_name -> relation_name -> [referenced_relations]`
/// suitable for `build_relation_dependencies()`.
///
/// For each relation, traverses the userset rewrite tree to find all
/// `ComputedUserset` and `TupleToUserset` references that create dependencies.
pub fn extract_relation_refs(
    model: &AuthorizationModel,
) -> std::collections::HashMap<String, std::collections::HashMap<String, Vec<String>>> {
    let mut result = std::collections::HashMap::new();

    for type_def in &model.type_definitions {
        let mut type_rels = std::collections::HashMap::new();
        for rel_def in &type_def.relations {
            let mut refs = Vec::new();
            collect_userset_refs(&rel_def.rewrite, &mut refs);
            type_rels.insert(rel_def.name.clone(), refs);
        }
        result.insert(type_def.type_name.clone(), type_rels);
    }

    result
}

/// Recursively collect relation references from a userset rewrite tree.
fn collect_userset_refs(userset: &Userset, refs: &mut Vec<String>) {
    match userset {
        Userset::This => {}
        Userset::ComputedUserset { relation } => {
            refs.push(relation.clone());
        }
        Userset::TupleToUserset {
            tupleset,
            computed_userset: _,
        } => {
            // The tupleset relation is read to find the parent object;
            // changes to tuples with this relation can affect the result.
            refs.push(tupleset.clone());
        }
        Userset::Union { children } | Userset::Intersection { children } => {
            for child in children {
                collect_userset_refs(child, refs);
            }
        }
        Userset::Exclusion { base, subtract } => {
            collect_userset_refs(base, refs);
            collect_userset_refs(subtract, refs);
        }
    }
}

/// Load the latest authorization model for a store and extract relation deps.
///
/// Returns `None` if no model exists for the store.
pub async fn load_store_relation_refs(
    storage: &dyn DataStore,
    store_id: &str,
) -> Option<std::collections::HashMap<String, std::collections::HashMap<String, Vec<String>>>> {
    let stored_model = match storage.get_latest_authorization_model(store_id).await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(
                store_id,
                error = %e,
                "Failed to load authorization model for relation refs"
            );
            return None;
        }
    };

    match parse_stored_model(&stored_model) {
        Ok(model) => Some(extract_relation_refs(&model)),
        Err(e) => {
            tracing::warn!(
                store_id,
                error = %e,
                "Failed to parse authorization model for relation refs"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsfga_storage::MemoryDataStore;

    #[tokio::test]
    async fn test_store_tuple_reader_store_exists() {
        let storage: Arc<dyn DataStore> = Arc::new(MemoryDataStore::new());
        storage
            .create_store("test-store", "Test Store")
            .await
            .unwrap();

        let reader = StoreTupleReader::new(storage);

        assert!(reader.store_exists("test-store").await.unwrap());
        assert!(!reader.store_exists("nonexistent").await.unwrap());
    }

    #[tokio::test]
    async fn test_store_model_id_provider_returns_none_for_missing_store() {
        let storage: Arc<dyn DataStore> = Arc::new(MemoryDataStore::new());
        let provider = StoreModelIdProvider::new(storage);

        assert!(provider.get_latest_model_id("nonexistent").await.is_none());
    }

    #[test]
    fn test_parse_stored_model_invalid_json() {
        let stored = StoredAuthorizationModel::new("id1", "store1", "1.1", "not valid json");
        let result = parse_stored_model(&stored);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_stored_model_invalid_schema_version() {
        let stored = StoredAuthorizationModel::new("id1", "store1", "2.0", "{}");
        let result = parse_stored_model(&stored);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_stored_model_empty_type_definitions() {
        let json = serde_json::json!({
            "type_definitions": []
        });
        let stored = StoredAuthorizationModel::new("id1", "store1", "1.1", json.to_string());
        let result = parse_stored_model(&stored).unwrap();
        assert!(result.type_definitions.is_empty());
    }

    #[test]
    fn test_parse_stored_model_with_type_and_relation() {
        let json = serde_json::json!({
            "type_definitions": [
                {
                    "type": "document",
                    "relations": {
                        "viewer": {},
                        "editor": {
                            "computedUserset": {
                                "relation": "owner"
                            }
                        }
                    }
                }
            ]
        });
        let stored = StoredAuthorizationModel::new("id1", "store1", "1.1", json.to_string());
        let model = parse_stored_model(&stored).unwrap();
        assert_eq!(model.type_definitions.len(), 1);
        assert_eq!(model.type_definitions[0].type_name, "document");
        assert_eq!(model.type_definitions[0].relations.len(), 2);
    }

    // ─── extract_relation_refs tests ──────────────────────────────────────────

    /// Helper: build an AuthorizationModel from JSON, then extract relation refs.
    fn refs_from_json(
        json: serde_json::Value,
    ) -> std::collections::HashMap<String, std::collections::HashMap<String, Vec<String>>> {
        let stored = StoredAuthorizationModel::new("id1", "store1", "1.1", json.to_string());
        let model = parse_stored_model(&stored).unwrap();
        extract_relation_refs(&model)
    }

    #[test]
    fn test_extract_relation_refs_direct_only() {
        let refs = refs_from_json(serde_json::json!({
            "type_definitions": [{
                "type": "document",
                "relations": {
                    "viewer": {},
                    "editor": {}
                }
            }]
        }));
        // Direct relations (This) have no refs
        assert!(refs["document"]["viewer"].is_empty());
        assert!(refs["document"]["editor"].is_empty());
    }

    #[test]
    fn test_extract_relation_refs_computed_userset() {
        let refs = refs_from_json(serde_json::json!({
            "type_definitions": [{
                "type": "document",
                "relations": {
                    "owner": {},
                    "editor": {
                        "computedUserset": { "relation": "owner" }
                    }
                }
            }]
        }));
        assert!(refs["document"]["owner"].is_empty());
        assert_eq!(refs["document"]["editor"], vec!["owner".to_string()]);
    }

    #[test]
    fn test_extract_relation_refs_tuple_to_userset() {
        let refs = refs_from_json(serde_json::json!({
            "type_definitions": [{
                "type": "document",
                "relations": {
                    "parent": {},
                    "viewer": {
                        "tupleToUserset": {
                            "tupleset": { "relation": "parent" },
                            "computedUserset": { "relation": "viewer" }
                        }
                    }
                }
            }]
        }));
        // TupleToUserset references the tupleset relation
        assert_eq!(refs["document"]["viewer"], vec!["parent".to_string()]);
    }

    #[test]
    fn test_extract_relation_refs_union() {
        let refs = refs_from_json(serde_json::json!({
            "type_definitions": [{
                "type": "document",
                "relations": {
                    "owner": {},
                    "editor": {},
                    "viewer": {
                        "union": {
                            "child": [
                                { "this": {} },
                                { "computedUserset": { "relation": "editor" } },
                                { "computedUserset": { "relation": "owner" } }
                            ]
                        }
                    }
                }
            }]
        }));
        let viewer_refs = &refs["document"]["viewer"];
        assert_eq!(viewer_refs.len(), 2);
        assert!(viewer_refs.contains(&"editor".to_string()));
        assert!(viewer_refs.contains(&"owner".to_string()));
    }

    #[test]
    fn test_extract_relation_refs_exclusion() {
        let refs = refs_from_json(serde_json::json!({
            "type_definitions": [{
                "type": "document",
                "relations": {
                    "viewer": {},
                    "blocked": {},
                    "can_view": {
                        "exclusion": {
                            "base": { "computedUserset": { "relation": "viewer" } },
                            "subtract": { "computedUserset": { "relation": "blocked" } }
                        }
                    }
                }
            }]
        }));
        let can_view_refs = &refs["document"]["can_view"];
        assert_eq!(can_view_refs.len(), 2);
        assert!(can_view_refs.contains(&"viewer".to_string()));
        assert!(can_view_refs.contains(&"blocked".to_string()));
    }

    #[test]
    fn test_extract_relation_refs_multiple_types() {
        let refs = refs_from_json(serde_json::json!({
            "type_definitions": [
                {
                    "type": "document",
                    "relations": {
                        "owner": {},
                        "viewer": { "computedUserset": { "relation": "owner" } }
                    }
                },
                {
                    "type": "folder",
                    "relations": {
                        "editor": {},
                        "viewer": { "computedUserset": { "relation": "editor" } }
                    }
                }
            ]
        }));
        assert_eq!(refs["document"]["viewer"], vec!["owner".to_string()]);
        assert_eq!(refs["folder"]["viewer"], vec!["editor".to_string()]);
    }

    #[test]
    fn test_extract_relation_refs_empty_model() {
        let refs = refs_from_json(serde_json::json!({
            "type_definitions": []
        }));
        assert!(refs.is_empty());
    }

    #[test]
    fn test_collect_userset_refs_this() {
        let mut refs = Vec::new();
        collect_userset_refs(&Userset::This, &mut refs);
        assert!(refs.is_empty());
    }

    #[test]
    fn test_collect_userset_refs_computed() {
        let mut refs = Vec::new();
        collect_userset_refs(
            &Userset::ComputedUserset {
                relation: "editor".to_string(),
            },
            &mut refs,
        );
        assert_eq!(refs, vec!["editor".to_string()]);
    }

    #[test]
    fn test_collect_userset_refs_nested_union() {
        let userset = Userset::Union {
            children: vec![
                Userset::This,
                Userset::ComputedUserset {
                    relation: "editor".to_string(),
                },
                Userset::Union {
                    children: vec![Userset::ComputedUserset {
                        relation: "owner".to_string(),
                    }],
                },
            ],
        };
        let mut refs = Vec::new();
        collect_userset_refs(&userset, &mut refs);
        assert_eq!(refs, vec!["editor".to_string(), "owner".to_string()]);
    }
}
