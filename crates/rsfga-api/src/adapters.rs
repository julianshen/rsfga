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
use rsfga_domain::model::{AuthorizationModel, RelationDefinition, TypeDefinition, Userset};
use rsfga_domain::resolver::{ModelReader, StoredTupleRef, TupleReader};
use rsfga_storage::DataStore;

/// Default cache TTL for parsed authorization models.
/// Slightly longer TTL (30s) is safe with moka's automatic eviction.
const MODEL_CACHE_TTL: Duration = Duration::from_secs(30);

/// Maximum number of models to cache. Prevents unbounded memory growth.
const MODEL_CACHE_MAX_CAPACITY: u64 = 1000;

/// Known complex relation definition keys in OpenFGA.
/// These represent relation rewrites beyond simple direct assignment.
const COMPLEX_RELATION_KEYS: &[&str] = &[
    "union",
    "intersection",
    "exclusion",
    "computedUserset",
    "tupleToUserset",
    // OpenFGA v1.1+ uses snake_case in some contexts
    "computed_userset",
    "tuple_to_userset",
];

/// Checks if a relation definition JSON object represents a complex relation.
///
/// Returns true only if the definition contains known complex relation keys
/// (union, intersection, exclusion, computedUserset, tupleToUserset).
/// Empty objects or objects with only metadata keys (e.g., future OpenFGA
/// additions) are not considered complex.
fn is_complex_relation_def(rel_def: &serde_json::Value) -> bool {
    rel_def.as_object().is_some_and(|obj| {
        obj.keys()
            .any(|key| COMPLEX_RELATION_KEYS.contains(&key.as_str()))
    })
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
                            // TODO(#124): Full relation parsing (union, intersection, exclusion,
                            // computedUserset, tupleToUserset) should be implemented when the
                            // model parser is fully integrated. Currently only direct assignment
                            // (Userset::This) is supported.
                            //
                            // Per Invariant I1 (Correctness > Performance), we fail fast on
                            // complex relations rather than silently defaulting to potentially
                            // incorrect behavior. This prevents security-critical authorization
                            // bugs from going unnoticed.
                            if is_complex_relation_def(rel_def) {
                                return Err(DomainError::ModelParseError {
                                    message: format!(
                                        "Complex relation definition not supported: type '{}', \
                                         relation '{}'. Only direct tuple assignments are \
                                         currently supported. See issue #124.",
                                        type_name, rel_name
                                    ),
                                });
                            }
                            relations.push(RelationDefinition {
                                name: rel_name.clone(),
                                type_constraints: vec![],
                                rewrite: Userset::This,
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
    async fn test_model_reader_rejects_complex_relations() {
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

        // Should fail with ModelParseError for complex relations
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, DomainError::ModelParseError { .. }));
        if let DomainError::ModelParseError { message } = err {
            assert!(message.contains("Complex relation definition not supported"));
            assert!(message.contains("viewer"));
        }
    }
}
