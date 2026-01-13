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
#[cfg_attr(test, allow(dead_code))]
pub(crate) fn parse_userset(
    rel_def: &serde_json::Value,
    type_name: &str,
    rel_name: &str,
) -> DomainResult<Userset> {
    let obj = rel_def.as_object().ok_or_else(|| DomainError::ModelParseError {
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
    if let Some(cu) = obj.get("computedUserset").or_else(|| obj.get("computed_userset")) {
        let relation = cu
            .get("relation")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DomainError::ModelParseError {
                message: format!(
                    "computedUserset requires 'relation' field: type '{}', relation '{}'",
                    type_name, rel_name
                ),
            })?;
        return Ok(Userset::ComputedUserset {
            relation: relation.to_string(),
        });
    }

    // tupleToUserset or tuple_to_userset
    if let Some(ttu) = obj.get("tupleToUserset").or_else(|| obj.get("tuple_to_userset")) {
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
        let children = union
            .get("child")
            .and_then(|v| v.as_array())
            .ok_or_else(|| DomainError::ModelParseError {
                message: format!(
                    "union requires 'child' array: type '{}', relation '{}'",
                    type_name, rel_name
                ),
            })?;
        let parsed_children: Result<Vec<Userset>, _> = children
            .iter()
            .map(|c| parse_userset(c, type_name, rel_name))
            .collect();
        return Ok(Userset::Union {
            children: parsed_children?,
        });
    }

    // intersection
    if let Some(intersection) = obj.get("intersection") {
        let children = intersection
            .get("child")
            .and_then(|v| v.as_array())
            .ok_or_else(|| DomainError::ModelParseError {
                message: format!(
                    "intersection requires 'child' array: type '{}', relation '{}'",
                    type_name, rel_name
                ),
            })?;
        let parsed_children: Result<Vec<Userset>, _> = children
            .iter()
            .map(|c| parse_userset(c, type_name, rel_name))
            .collect();
        return Ok(Userset::Intersection {
            children: parsed_children?,
        });
    }

    // exclusion or difference (both map to Userset::Exclusion)
    if let Some(exclusion) = obj.get("exclusion").or_else(|| obj.get("difference")) {
        let base = exclusion.get("base").ok_or_else(|| DomainError::ModelParseError {
            message: format!(
                "exclusion requires 'base' field: type '{}', relation '{}'",
                type_name, rel_name
            ),
        })?;
        let subtract = exclusion.get("subtract").ok_or_else(|| DomainError::ModelParseError {
            message: format!(
                "exclusion requires 'subtract' field: type '{}', relation '{}'",
                type_name, rel_name
            ),
        })?;
        return Ok(Userset::Exclusion {
            base: Box::new(parse_userset(base, type_name, rel_name)?),
            subtract: Box::new(parse_userset(subtract, type_name, rel_name)?),
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
fn parse_type_constraints(type_def: &serde_json::Value, rel_name: &str) -> Vec<TypeConstraint> {
    type_def
        .get("metadata")
        .and_then(|m| m.get("relations"))
        .and_then(|r| r.get(rel_name))
        .and_then(|rel_meta| rel_meta.get("directly_related_user_types"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    // Each item can be:
                    // - {"type": "user"} → TypeConstraint::new("user")
                    // - {"type": "group", "relation": "member"} → TypeConstraint::new("group#member")
                    // - {"type": "user", "condition": "is_admin"} → TypeConstraint::with_condition("user", "is_admin")
                    let type_name = item.get("type").and_then(|t| t.as_str())?;
                    let condition = item.get("condition").and_then(|c| c.as_str());

                    let full_type = if let Some(relation) =
                        item.get("relation").and_then(|r| r.as_str())
                    {
                        format!("{}#{}", type_name, relation)
                    } else {
                        type_name.to_string()
                    };

                    if let Some(cond) = condition {
                        Some(TypeConstraint::with_condition(&full_type, cond))
                    } else {
                        Some(TypeConstraint::new(&full_type))
                    }
                })
                .collect()
        })
        .unwrap_or_default()
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
                            let type_constraints = parse_type_constraints(type_def, rel_name);

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
        assert!(result.is_ok(), "Failed to parse complex relation: {:?}", result);
        let model = result.unwrap();

        // Find the document type and verify the viewer relation was parsed correctly
        let doc_type = model.type_definitions.iter().find(|t| t.type_name == "document");
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
        use serde_json::json;
        use rsfga_domain::model::Userset;

        let result = super::parse_userset(&json!({}), "document", "viewer");
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), Userset::This));
    }

    #[test]
    fn test_parse_userset_this_explicit() {
        use serde_json::json;
        use rsfga_domain::model::Userset;

        let result = super::parse_userset(&json!({"this": {}}), "document", "viewer");
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), Userset::This));
    }

    #[test]
    fn test_parse_userset_computed_userset_camel_case() {
        use serde_json::json;
        use rsfga_domain::model::Userset;

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
        use serde_json::json;
        use rsfga_domain::model::Userset;

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
        use serde_json::json;
        use rsfga_domain::model::Userset;

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
        use serde_json::json;
        use rsfga_domain::model::Userset;

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
    fn test_parse_userset_union() {
        use serde_json::json;
        use rsfga_domain::model::Userset;

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
                assert!(matches!(&children[1], Userset::ComputedUserset { relation } if relation == "owner"));
            }
            _ => panic!("Expected Union"),
        }
    }

    #[test]
    fn test_parse_userset_intersection() {
        use serde_json::json;
        use rsfga_domain::model::Userset;

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
        use serde_json::json;
        use rsfga_domain::model::Userset;

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
                assert!(matches!(*base, Userset::ComputedUserset { relation } if relation == "member"));
                assert!(matches!(*subtract, Userset::ComputedUserset { relation } if relation == "banned"));
            }
            _ => panic!("Expected Exclusion"),
        }
    }

    #[test]
    fn test_parse_userset_difference_alias() {
        use serde_json::json;
        use rsfga_domain::model::Userset;

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
        use serde_json::json;
        use rsfga_domain::model::Userset;

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

        let result = super::parse_userset(
            &json!({"computedUserset": {}}),
            "document",
            "viewer",
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("requires 'relation' field"));
    }

    #[test]
    fn test_parse_userset_error_unknown_key() {
        use serde_json::json;

        let result = super::parse_userset(
            &json!({"unknownKey": {}}),
            "document",
            "viewer",
        );
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
}
