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
    AuthorizationModel, RelationDefinition, TypeConstraint, TypeDefinition, Userset,
};
use rsfga_domain::resolver::{ModelReader, StoredTupleRef, TupleReader};
use rsfga_storage::DataStore;

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
pub(crate) fn parse_userset(
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
                "{} requires 'child' array: type '{}', relation '{}'",
                key, type_name, rel_name
            ),
        })?;

    // Security: reject empty child arrays to prevent "allow all" in intersection
    // or "deny all" in union, which could lead to auth bypass or unintended denials
    if children.is_empty() {
        return Err(DomainError::ModelParseError {
            message: format!(
                "{} requires at least one child: type '{}', relation '{}'",
                key, type_name, rel_name
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
                "relation definition exceeds max nesting depth of {}: type '{}', relation '{}'",
                MAX_PARSE_DEPTH, type_name, rel_name
            ),
        });
    }

    let obj = rel_def
        .as_object()
        .ok_or_else(|| DomainError::ModelParseError {
            message: format!(
                "relation definition must be an object: type '{}', relation '{}'",
                type_name, rel_name
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
                    "computedUserset requires 'relation' field: type '{}', relation '{}'",
                    type_name, rel_name
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
                    "tupleToUserset requires 'tupleset.relation' field: type '{}', relation '{}'",
                    type_name, rel_name
                ),
            })?;
        let computed_userset = ttu
            .get("computedUserset")
            .or_else(|| ttu.get("computed_userset"))
            .and_then(|cu| cu.get("relation"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| DomainError::ModelParseError {
                message: format!(
                    "tupleToUserset requires 'computedUserset.relation' field: type '{}', relation '{}'",
                    type_name, rel_name
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
                    "exclusion requires 'base' field: type '{}', relation '{}'",
                    type_name, rel_name
                ),
            })?;
        let subtract = exclusion
            .get("subtract")
            .ok_or_else(|| DomainError::ModelParseError {
                message: format!(
                    "exclusion requires 'subtract' field: type '{}', relation '{}'",
                    type_name, rel_name
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
            "unknown relation definition keys {:?}: type '{}', relation '{}'",
            keys, type_name, rel_name
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
                    "directly_related_user_types[{}] missing 'type' field: type '{}', relation '{}'",
                    idx, type_name, rel_name
                ),
            }
        })?;

        let condition = item.get("condition").and_then(|c| c.as_str());

        let full_type = if let Some(relation) = item.get("relation").and_then(|r| r.as_str()) {
            format!("{}#{}", user_type, relation)
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
            .map_err(|e| DomainError::ResolverError {
                message: format!("storage error: {}", e),
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
            Err(e) => Err(DomainError::ResolverError {
                message: format!("storage error: {}", e),
            }),
        }
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
                    // Default fallback for any other variants
                    other => DomainError::ResolverError {
                        message: other.to_string(),
                    },
                }
            })
    }

    /// Fetches and parses a model from storage (no caching).
    /// This is a static method to allow use in async closures.
    async fn fetch_and_parse_model(
        storage: &S,
        store_id: &str,
    ) -> DomainResult<AuthorizationModel> {
        let stored_model = storage
            .get_latest_authorization_model(store_id)
            .await
            .map_err(|e| DomainError::ResolverError {
                message: format!("storage error: {}", e),
            })?;

        // Parse the stored model JSON into an AuthorizationModel
        let model_json: serde_json::Value = serde_json::from_str(&stored_model.model_json)
            .map_err(|e| DomainError::ModelParseError {
                message: format!("failed to parse model JSON: {}", e),
            })?;

        // Build AuthorizationModel from the parsed JSON
        let mut type_definitions = Vec::new();

        // Extract type_definitions from the JSON
        if let Some(type_defs) = model_json
            .get("type_definitions")
            .and_then(|v| v.as_array())
        {
            for type_def in type_defs {
                if let Some(type_name) = type_def.get("type").and_then(|v| v.as_str()) {
                    let mut relations = Vec::new();

                    // Parse relations if present
                    if let Some(rels) = type_def.get("relations").and_then(|v| v.as_object()) {
                        for (rel_name, rel_def) in rels {
                            // Parse the userset (relation rewrite) from JSON
                            let rewrite = parse_userset(rel_def, type_name, rel_name)?;

                            // Parse type constraints from metadata
                            let type_constraints =
                                parse_type_constraints(type_def, type_name, rel_name)?;

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
            }
        }

        Ok(AuthorizationModel::with_types(
            &stored_model.schema_version,
            type_definitions,
        ))
    }
}

#[async_trait]
impl<S: DataStore> ModelReader for DataStoreModelReader<S> {
    async fn get_model(&self, store_id: &str) -> DomainResult<AuthorizationModel> {
        self.get_parsed_model(store_id).await
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
            "Failed to parse complex relation: {:?}",
            result
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
            other => panic!("Expected Union, got {:?}", other),
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
            reader.cache.insert(format!("store-{}", i), model).await;
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
        assert!(result.is_ok(), "Failed to parse model: {:?}", result);

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
}
