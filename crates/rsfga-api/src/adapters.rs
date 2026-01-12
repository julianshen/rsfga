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

use async_trait::async_trait;

use rsfga_domain::error::{DomainError, DomainResult};
use rsfga_domain::model::{AuthorizationModel, RelationDefinition, TypeDefinition, Userset};
use rsfga_domain::resolver::{ModelReader, StoredTupleRef, TupleReader};
use rsfga_storage::DataStore;

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
pub struct DataStoreModelReader<S: DataStore> {
    storage: Arc<S>,
}

impl<S: DataStore> DataStoreModelReader<S> {
    /// Creates a new adapter wrapping the given storage.
    pub fn new(storage: Arc<S>) -> Self {
        Self { storage }
    }

    /// Helper to get and parse the latest model for a store.
    async fn get_parsed_model(&self, store_id: &str) -> DomainResult<AuthorizationModel> {
        let stored_model = self
            .storage
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
                            // LIMITATION: Complex relation definitions will be simplified to
                            // direct assignment, which may cause incorrect authorization results
                            // for non-trivial models. This adapter is intended for basic models
                            // with simple direct tuple assignments.
                            if !rel_def.is_object()
                                || !rel_def.as_object().map_or(true, |o| o.is_empty())
                            {
                                tracing::warn!(
                                    type_name = type_name,
                                    relation = rel_name.as_str(),
                                    "Complex relation definition found but not fully parsed; \
                                     defaulting to direct assignment (Userset::This)"
                                );
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
}
