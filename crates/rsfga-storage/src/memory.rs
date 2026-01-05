//! In-memory storage implementation for testing.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;

use crate::error::{StorageError, StorageResult};
use crate::traits::{DataStore, Store, StoredModel, StoredTuple, TupleFilter};

/// In-memory implementation of DataStore.
///
/// Uses DashMap for thread-safe concurrent access without locks.
#[derive(Debug, Default)]
pub struct MemoryDataStore {
    stores: DashMap<String, Store>,
    tuples: DashMap<String, Vec<StoredTuple>>,
    /// Models stored by store_id -> Vec<StoredModel>
    models: DashMap<String, Vec<StoredModel>>,
    /// Counter for generating unique model IDs.
    model_id_counter: AtomicU64,
}

impl MemoryDataStore {
    /// Creates a new in-memory data store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new in-memory data store wrapped in Arc.
    pub fn new_shared() -> Arc<Self> {
        Arc::new(Self::new())
    }
}

#[async_trait]
impl DataStore for MemoryDataStore {
    async fn create_store(&self, id: &str, name: &str) -> StorageResult<Store> {
        // Check if store already exists
        if self.stores.contains_key(id) {
            return Err(StorageError::StoreAlreadyExists {
                store_id: id.to_string(),
            });
        }

        let now = chrono::Utc::now();
        let store = Store {
            id: id.to_string(),
            name: name.to_string(),
            created_at: now,
            updated_at: now,
        };

        self.stores.insert(id.to_string(), store.clone());
        self.tuples.insert(id.to_string(), Vec::new());

        Ok(store)
    }

    async fn get_store(&self, id: &str) -> StorageResult<Store> {
        self.stores
            .get(id)
            .map(|s| s.clone())
            .ok_or_else(|| StorageError::StoreNotFound {
                store_id: id.to_string(),
            })
    }

    async fn delete_store(&self, id: &str) -> StorageResult<()> {
        if self.stores.remove(id).is_none() {
            return Err(StorageError::StoreNotFound {
                store_id: id.to_string(),
            });
        }
        self.tuples.remove(id);
        Ok(())
    }

    async fn list_stores(&self) -> StorageResult<Vec<Store>> {
        Ok(self.stores.iter().map(|s| s.value().clone()).collect())
    }

    async fn write_tuples(
        &self,
        store_id: &str,
        writes: Vec<StoredTuple>,
        deletes: Vec<StoredTuple>,
    ) -> StorageResult<()> {
        // Verify store exists
        if !self.stores.contains_key(store_id) {
            return Err(StorageError::StoreNotFound {
                store_id: store_id.to_string(),
            });
        }

        let mut tuples = self.tuples.entry(store_id.to_string()).or_default();

        // Process deletes
        for delete in deletes {
            tuples.retain(|t| {
                !(t.object_type == delete.object_type
                    && t.object_id == delete.object_id
                    && t.relation == delete.relation
                    && t.user_type == delete.user_type
                    && t.user_id == delete.user_id
                    && t.user_relation == delete.user_relation)
            });
        }

        // Process writes
        for write in writes {
            // Check for duplicates (including user_relation)
            let exists = tuples.iter().any(|t| {
                t.object_type == write.object_type
                    && t.object_id == write.object_id
                    && t.relation == write.relation
                    && t.user_type == write.user_type
                    && t.user_id == write.user_id
                    && t.user_relation == write.user_relation
            });

            if !exists {
                tuples.push(write);
            }
        }

        Ok(())
    }

    async fn read_tuples(
        &self,
        store_id: &str,
        filter: &TupleFilter,
    ) -> StorageResult<Vec<StoredTuple>> {
        // Verify store exists
        if !self.stores.contains_key(store_id) {
            return Err(StorageError::StoreNotFound {
                store_id: store_id.to_string(),
            });
        }

        // Filter first, then clone only matching tuples (more efficient)
        let filtered: Vec<StoredTuple> = self
            .tuples
            .get(store_id)
            .map(|tuples| {
                tuples
                    .iter()
                    .filter(|t| {
                        filter
                            .object_type
                            .as_ref()
                            .map_or(true, |ot| &t.object_type == ot)
                            && filter
                                .object_id
                                .as_ref()
                                .map_or(true, |oi| &t.object_id == oi)
                            && filter.relation.as_ref().map_or(true, |r| &t.relation == r)
                            && filter.user.as_ref().map_or(true, |u| {
                                let user_str = if let Some(ref rel) = t.user_relation {
                                    format!("{}:{}#{}", t.user_type, t.user_id, rel)
                                } else {
                                    format!("{}:{}", t.user_type, t.user_id)
                                };
                                &user_str == u
                            })
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();

        Ok(filtered)
    }

    async fn write_model(
        &self,
        store_id: &str,
        schema_version: &str,
        model_json: &str,
    ) -> StorageResult<String> {
        // Verify store exists
        if !self.stores.contains_key(store_id) {
            return Err(StorageError::StoreNotFound {
                store_id: store_id.to_string(),
            });
        }

        // Generate unique model ID
        let model_id = format!(
            "{}",
            self.model_id_counter.fetch_add(1, Ordering::SeqCst) + 1
        );

        let model = StoredModel {
            id: model_id.clone(),
            store_id: store_id.to_string(),
            schema_version: schema_version.to_string(),
            model_json: model_json.to_string(),
            created_at: chrono::Utc::now(),
        };

        self.models
            .entry(store_id.to_string())
            .or_default()
            .push(model);

        Ok(model_id)
    }

    async fn read_model(&self, store_id: &str, model_id: &str) -> StorageResult<StoredModel> {
        // Verify store exists
        if !self.stores.contains_key(store_id) {
            return Err(StorageError::StoreNotFound {
                store_id: store_id.to_string(),
            });
        }

        let models = self.models.get(store_id);
        let model = models
            .as_ref()
            .and_then(|m| m.iter().find(|m| m.id == model_id))
            .ok_or_else(|| StorageError::ModelNotFound {
                model_id: model_id.to_string(),
            })?;

        Ok(model.clone())
    }

    async fn read_latest_model(&self, store_id: &str) -> StorageResult<StoredModel> {
        // Verify store exists
        if !self.stores.contains_key(store_id) {
            return Err(StorageError::StoreNotFound {
                store_id: store_id.to_string(),
            });
        }

        let models = self.models.get(store_id);
        let model =
            models
                .as_ref()
                .and_then(|m| m.last())
                .ok_or_else(|| StorageError::ModelNotFound {
                    model_id: "latest".to_string(),
                })?;

        Ok(model.clone())
    }

    async fn list_models(&self, store_id: &str) -> StorageResult<Vec<StoredModel>> {
        // Verify store exists
        if !self.stores.contains_key(store_id) {
            return Err(StorageError::StoreNotFound {
                store_id: store_id.to_string(),
            });
        }

        Ok(self
            .models
            .get(store_id)
            .map(|m| m.clone())
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_and_get_store() {
        let store = MemoryDataStore::new();
        let created = store.create_store("test-id", "Test Store").await.unwrap();

        assert_eq!(created.id, "test-id");
        assert_eq!(created.name, "Test Store");

        let retrieved = store.get_store("test-id").await.unwrap();
        assert_eq!(retrieved.id, created.id);
    }

    #[tokio::test]
    async fn test_get_nonexistent_store() {
        let store = MemoryDataStore::new();
        let result = store.get_store("nonexistent").await;

        assert!(matches!(result, Err(StorageError::StoreNotFound { .. })));
    }

    #[tokio::test]
    async fn test_write_and_read_tuple() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        let tuple = StoredTuple {
            object_type: "document".to_string(),
            object_id: "doc1".to_string(),
            relation: "viewer".to_string(),
            user_type: "user".to_string(),
            user_id: "alice".to_string(),
            user_relation: None,
        };

        store
            .write_tuples("test-store", vec![tuple.clone()], vec![])
            .await
            .unwrap();

        let filter = TupleFilter {
            object_type: Some("document".to_string()),
            ..Default::default()
        };

        let tuples = store.read_tuples("test-store", &filter).await.unwrap();
        assert_eq!(tuples.len(), 1);
        assert_eq!(tuples[0], tuple);
    }

    #[tokio::test]
    async fn test_create_duplicate_store_fails() {
        let store = MemoryDataStore::new();
        store.create_store("test-id", "Test Store").await.unwrap();

        // Attempting to create a store with the same ID should fail
        let result = store.create_store("test-id", "Another Store").await;

        assert!(matches!(
            result,
            Err(StorageError::StoreAlreadyExists { .. })
        ));
    }

    #[tokio::test]
    async fn test_tuple_with_user_relation() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        // Create tuples with and without user_relation
        let tuple_without = StoredTuple {
            object_type: "document".to_string(),
            object_id: "doc1".to_string(),
            relation: "viewer".to_string(),
            user_type: "group".to_string(),
            user_id: "eng".to_string(),
            user_relation: None,
        };

        let tuple_with = StoredTuple {
            object_type: "document".to_string(),
            object_id: "doc1".to_string(),
            relation: "viewer".to_string(),
            user_type: "group".to_string(),
            user_id: "eng".to_string(),
            user_relation: Some("member".to_string()),
        };

        // Both should be stored (they're different due to user_relation)
        store
            .write_tuples(
                "test-store",
                vec![tuple_without.clone(), tuple_with.clone()],
                vec![],
            )
            .await
            .unwrap();

        let filter = TupleFilter::default();
        let tuples = store.read_tuples("test-store", &filter).await.unwrap();
        assert_eq!(
            tuples.len(),
            2,
            "Both tuples should be stored as they differ by user_relation"
        );
    }

    // ========== Model Operation Tests ==========

    #[tokio::test]
    async fn test_write_and_read_model() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        let model_json = r#"{"schema_version":"1.1","type_definitions":[]}"#;
        let model_id = store
            .write_model("test-store", "1.1", model_json)
            .await
            .unwrap();

        assert!(!model_id.is_empty());

        let model = store.read_model("test-store", &model_id).await.unwrap();
        assert_eq!(model.id, model_id);
        assert_eq!(model.store_id, "test-store");
        assert_eq!(model.schema_version, "1.1");
        assert_eq!(model.model_json, model_json);
    }

    #[tokio::test]
    async fn test_write_model_nonexistent_store() {
        let store = MemoryDataStore::new();

        let result = store.write_model("nonexistent", "1.1", "{}").await;

        assert!(matches!(result, Err(StorageError::StoreNotFound { .. })));
    }

    #[tokio::test]
    async fn test_read_model_not_found() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        let result = store.read_model("test-store", "nonexistent").await;
        assert!(matches!(result, Err(StorageError::ModelNotFound { .. })));
    }

    #[tokio::test]
    async fn test_read_latest_model() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        // Write multiple models
        let _id1 = store
            .write_model("test-store", "1.0", r#"{"version":"1.0"}"#)
            .await
            .unwrap();
        let id2 = store
            .write_model("test-store", "1.1", r#"{"version":"1.1"}"#)
            .await
            .unwrap();

        // Latest should be the second model
        let latest = store.read_latest_model("test-store").await.unwrap();
        assert_eq!(latest.id, id2);
        assert_eq!(latest.schema_version, "1.1");
    }

    #[tokio::test]
    async fn test_read_latest_model_no_models() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        let result = store.read_latest_model("test-store").await;
        assert!(matches!(result, Err(StorageError::ModelNotFound { .. })));
    }

    #[tokio::test]
    async fn test_list_models() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        // Initially empty
        let models = store.list_models("test-store").await.unwrap();
        assert!(models.is_empty());

        // Add models
        store
            .write_model("test-store", "1.0", r#"{"v":"1.0"}"#)
            .await
            .unwrap();
        store
            .write_model("test-store", "1.1", r#"{"v":"1.1"}"#)
            .await
            .unwrap();

        let models = store.list_models("test-store").await.unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].schema_version, "1.0");
        assert_eq!(models[1].schema_version, "1.1");
    }

    #[tokio::test]
    async fn test_list_models_nonexistent_store() {
        let store = MemoryDataStore::new();

        let result = store.list_models("nonexistent").await;
        assert!(matches!(result, Err(StorageError::StoreNotFound { .. })));
    }

    #[tokio::test]
    async fn test_model_id_uniqueness() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        let id1 = store.write_model("test-store", "1.1", "{}").await.unwrap();
        let id2 = store.write_model("test-store", "1.1", "{}").await.unwrap();
        let id3 = store.write_model("test-store", "1.1", "{}").await.unwrap();

        // All IDs should be unique
        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert_ne!(id1, id3);
    }
}
