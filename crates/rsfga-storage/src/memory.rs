//! In-memory storage implementation for testing.

use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;

use crate::error::{StorageError, StorageResult};
use crate::traits::{DataStore, Store, StoredTuple, TupleFilter};

/// In-memory implementation of DataStore.
///
/// Uses DashMap for thread-safe concurrent access without locks.
#[derive(Debug, Default)]
pub struct MemoryDataStore {
    stores: DashMap<String, Store>,
    tuples: DashMap<String, Vec<StoredTuple>>,
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
}
