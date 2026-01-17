//! In-memory storage implementation for testing.
//!
//! Uses `HashSet<StoredTuple>` for O(1) write/delete operations instead of
//! `Vec<StoredTuple>` which has O(N) complexity. This optimization addresses
//! issue #30 and significantly improves performance for stores with many tuples.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use tracing::instrument;

use crate::error::{HealthStatus, StorageError, StorageResult};
use crate::traits::{
    parse_continuation_token, parse_tuple_cursor, parse_user_filter, validate_store_id,
    validate_store_name, validate_tuple, DataStore, PaginatedResult, PaginationOptions, Store,
    StoredAuthorizationModel, StoredTuple, TupleCursor, TupleFilter, validate_object_type,
};

/// In-memory implementation of DataStore.


/// In-memory implementation of DataStore.
///
/// # Performance Characteristics
///
/// - **Write tuple**: O(1) average (HashSet insert)
/// - **Delete tuple**: O(1) average (HashSet remove)
/// - **Read tuples**: O(N) where N is total tuples in store (linear scan for filtering)
/// - **Store operations**: O(1) (DashMap lookup)
///
/// Uses DashMap for thread-safe concurrent access without locks, and HashSet
/// for tuple storage to enable constant-time membership tests and modifications.
#[derive(Debug, Default)]
pub struct MemoryDataStore {
    stores: DashMap<String, Store>,
    /// Tuples stored as HashSet for O(1) insert/delete/contains operations.
    /// Trade-off: Read operations still require iteration, but writes are much faster.
    tuples: DashMap<String, HashSet<StoredTuple>>,
    /// Authorization models keyed by store_id.
    /// Models are stored in insertion order (newest at the end), but list methods
    /// return them newest-first (reversed) for API consistency.
    authorization_models: DashMap<String, Vec<StoredAuthorizationModel>>,
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

/// Sorts authorization models newest-first (created_at DESC, id DESC).
/// Matches the ordering contract in the DataStore trait and DB backends.
fn sort_models_newest_first(models: &mut [StoredAuthorizationModel]) {
    models.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.id.cmp(&a.id))
    });
}

#[async_trait]
impl DataStore for MemoryDataStore {
    async fn create_store(&self, id: &str, name: &str) -> StorageResult<Store> {
        // Validate inputs
        validate_store_id(id)?;
        validate_store_name(name)?;

        let now = chrono::Utc::now();
        let store = Store {
            id: id.to_string(),
            name: name.to_string(),
            created_at: now,
            updated_at: now,
        };

        // Use atomic entry API to prevent race condition between check and insert
        use dashmap::mapref::entry::Entry;
        match self.stores.entry(id.to_string()) {
            Entry::Occupied(_) => {
                return Err(StorageError::StoreAlreadyExists {
                    store_id: id.to_string(),
                });
            }
            Entry::Vacant(entry) => {
                entry.insert(store.clone());
            }
        }

        // Use entry API to avoid overwriting if another task already initialized tuples
        self.tuples.entry(id.to_string()).or_default();

        Ok(store)
    }

    async fn get_store(&self, id: &str) -> StorageResult<Store> {
        self.stores
            .get(id)
            .map(|s| s.value().clone())
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
        self.authorization_models.remove(id);
        Ok(())
    }

    #[instrument(skip(self), fields(store_id = %id))]
    async fn update_store(&self, id: &str, name: &str) -> StorageResult<Store> {
        // Validate inputs
        validate_store_id(id)?;
        validate_store_name(name)?;

        // Capture timestamp before acquiring lock to minimize lock hold time
        let now = chrono::Utc::now();

        // Get and update the store
        let mut entry = self
            .stores
            .get_mut(id)
            .ok_or_else(|| StorageError::StoreNotFound {
                store_id: id.to_string(),
            })?;

        entry.name = name.to_string();
        entry.updated_at = now;

        Ok(entry.clone())
    }

    async fn list_stores(&self) -> StorageResult<Vec<Store>> {
        Ok(self.stores.iter().map(|s| s.value().clone()).collect())
    }

    async fn list_stores_paginated(
        &self,
        pagination: &PaginationOptions,
    ) -> StorageResult<PaginatedResult<Store>> {
        let mut stores: Vec<Store> = self.stores.iter().map(|s| s.value().clone()).collect();
        // Sort by created_at descending for consistent pagination
        stores.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        let page_size = pagination.page_size.unwrap_or(100) as usize;
        let offset = parse_continuation_token(&pagination.continuation_token)? as usize;

        let items: Vec<Store> = stores.into_iter().skip(offset).take(page_size).collect();

        let next_offset = offset + items.len();
        let continuation_token = if items.len() == page_size {
            Some(next_offset.to_string())
        } else {
            None
        };

        Ok(PaginatedResult {
            items,
            continuation_token,
        })
    }

    async fn write_tuples(
        &self,
        store_id: &str,
        writes: Vec<StoredTuple>,
        deletes: Vec<StoredTuple>,
    ) -> StorageResult<()> {
        // Validate inputs
        validate_store_id(store_id)?;
        for tuple in &writes {
            validate_tuple(tuple)?;
        }
        for tuple in &deletes {
            validate_tuple(tuple)?;
        }

        // Verify store exists
        if !self.stores.contains_key(store_id) {
            return Err(StorageError::StoreNotFound {
                store_id: store_id.to_string(),
            });
        }

        let mut tuples = self.tuples.entry(store_id.to_string()).or_default();

        // Process deletes - O(1) per tuple with HashSet
        for tuple in deletes {
            tuples.remove(&tuple);
        }

        // Process writes - O(1) per tuple with HashSet
        // HashSet::insert automatically handles duplicates (idempotent)
        for tuple in writes {
            tuples.insert(tuple);
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

        // Parse and validate user filter upfront
        let user_filter = if let Some(ref user) = filter.user {
            Some(parse_user_filter(user)?)
        } else {
            None
        };

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
                            && user_filter.as_ref().map_or(true, |(ut, ui, ur)| {
                                &t.user_type == ut && &t.user_id == ui && &t.user_relation == ur
                            })
                            && filter
                                .condition_name
                                .as_ref()
                                .map_or(true, |cn| t.condition_name.as_ref() == Some(cn))
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();

        Ok(filtered)
    }

    async fn read_tuples_paginated(
        &self,
        store_id: &str,
        filter: &TupleFilter,
        pagination: &PaginationOptions,
    ) -> StorageResult<PaginatedResult<StoredTuple>> {
        // Verify store exists
        if !self.stores.contains_key(store_id) {
            return Err(StorageError::StoreNotFound {
                store_id: store_id.to_string(),
            });
        }

        // Parse and validate user filter upfront
        let user_filter = if let Some(ref user) = filter.user {
            Some(parse_user_filter(user)?)
        } else {
            None
        };

        // Filter tuples
        let mut filtered: Vec<StoredTuple> = self
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
                            && user_filter.as_ref().map_or(true, |(ut, ui, ur)| {
                                &t.user_type == ut && &t.user_id == ui && &t.user_relation == ur
                            })
                            && filter
                                .condition_name
                                .as_ref()
                                .map_or(true, |cn| t.condition_name.as_ref() == Some(cn))
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();

        // Sort for consistent pagination (by object_type, object_id, relation, user_type, user_id)
        filtered.sort_by(|a, b| {
            (
                &a.object_type,
                &a.object_id,
                &a.relation,
                &a.user_type,
                &a.user_id,
            )
                .cmp(&(
                    &b.object_type,
                    &b.object_id,
                    &b.relation,
                    &b.user_type,
                    &b.user_id,
                ))
        });

        let page_size = pagination.page_size.unwrap_or(100) as usize;

        // Use cursor-based pagination with binary search for O(log N) seek
        let cursor = parse_tuple_cursor(&pagination.continuation_token)?;

        let start_idx = match cursor {
            Some(ref cursor) => {
                // Binary search to find first tuple AFTER the cursor
                let cursor_key = cursor.sort_key();
                filtered
                    .binary_search_by(|t| {
                        let t_key = (
                            t.object_type.as_str(),
                            t.object_id.as_str(),
                            t.relation.as_str(),
                            t.user_type.as_str(),
                            t.user_id.as_str(),
                        );
                        t_key.cmp(&cursor_key)
                    })
                    // If found, start after it; if not found, start at insertion point
                    .map(|i| i + 1)
                    .unwrap_or_else(|i| i)
            }
            None => 0,
        };

        let items: Vec<StoredTuple> = filtered
            .into_iter()
            .skip(start_idx)
            .take(page_size)
            .collect();

        // Generate cursor from last item for next page
        let continuation_token = if items.len() == page_size {
            items.last().map(|t| TupleCursor::from_tuple(t).encode())
        } else {
            None
        };

        Ok(PaginatedResult {
            items,
            continuation_token,
        })
    }

    // Authorization model operations

    async fn write_authorization_model(
        &self,
        model: StoredAuthorizationModel,
    ) -> StorageResult<StoredAuthorizationModel> {
        // Validate input bounds (consistent with DB backends)
        validate_store_id(&model.store_id)?;

        // Verify store exists
        if !self.stores.contains_key(&model.store_id) {
            return Err(StorageError::StoreNotFound {
                store_id: model.store_id.clone(),
            });
        }

        // Add model to the store's list
        // Note: We don't check for duplicate IDs because:
        // 1. IDs are ULIDs generated by the HTTP layer, effectively unique
        // 2. DB backends rely on PRIMARY KEY constraints rather than explicit checks
        // 3. In production, duplicate model writes with same ID never occur
        let mut models = self
            .authorization_models
            .entry(model.store_id.clone())
            .or_default();

        models.push(model.clone());

        Ok(model)
    }

    async fn get_authorization_model(
        &self,
        store_id: &str,
        model_id: &str,
    ) -> StorageResult<StoredAuthorizationModel> {
        // Validate input bounds
        validate_store_id(store_id)?;

        // Verify store exists
        if !self.stores.contains_key(store_id) {
            return Err(StorageError::StoreNotFound {
                store_id: store_id.to_string(),
            });
        }

        // Find the model by ID
        self.authorization_models
            .get(store_id)
            .and_then(|models| models.iter().find(|m| m.id == model_id).cloned())
            .ok_or_else(|| StorageError::ModelNotFound {
                model_id: model_id.to_string(),
            })
    }

    async fn list_authorization_models(
        &self,
        store_id: &str,
    ) -> StorageResult<Vec<StoredAuthorizationModel>> {
        // Validate input bounds
        validate_store_id(store_id)?;

        // Verify store exists
        if !self.stores.contains_key(store_id) {
            return Err(StorageError::StoreNotFound {
                store_id: store_id.to_string(),
            });
        }

        // Return models ordered by created_at DESC, id DESC (newest first, deterministic)
        let mut models: Vec<StoredAuthorizationModel> = self
            .authorization_models
            .get(store_id)
            .map(|models| models.iter().cloned().collect())
            .unwrap_or_default();

        sort_models_newest_first(&mut models);

        Ok(models)
    }

    async fn list_authorization_models_paginated(
        &self,
        store_id: &str,
        pagination: &PaginationOptions,
    ) -> StorageResult<PaginatedResult<StoredAuthorizationModel>> {
        // Validate input bounds
        validate_store_id(store_id)?;

        // Verify store exists
        if !self.stores.contains_key(store_id) {
            return Err(StorageError::StoreNotFound {
                store_id: store_id.to_string(),
            });
        }

        // Get models ordered by created_at DESC, id DESC (newest first, deterministic)
        let mut all_models: Vec<StoredAuthorizationModel> = self
            .authorization_models
            .get(store_id)
            .map(|models| models.iter().cloned().collect())
            .unwrap_or_default();

        sort_models_newest_first(&mut all_models);

        let page_size = pagination.page_size.unwrap_or(100) as usize;
        let offset = parse_continuation_token(&pagination.continuation_token)? as usize;

        let items: Vec<StoredAuthorizationModel> = all_models
            .into_iter()
            .skip(offset)
            .take(page_size)
            .collect();

        let next_offset = offset + items.len();
        let continuation_token = if items.len() == page_size {
            Some(next_offset.to_string())
        } else {
            None
        };

        Ok(PaginatedResult {
            items,
            continuation_token,

})
    }

    async fn get_latest_authorization_model(
        &self,
        store_id: &str,
    ) -> StorageResult<StoredAuthorizationModel> {
        // Validate input bounds
        validate_store_id(store_id)?;

        // Verify store exists
        if !self.stores.contains_key(store_id) {
            return Err(StorageError::StoreNotFound {
                store_id: store_id.to_string(),
            });
        }

        // Get the latest model (newest by created_at DESC, id DESC)
        let mut models: Vec<StoredAuthorizationModel> = self
            .authorization_models
            .get(store_id)
            .map(|models| models.iter().cloned().collect())
            .unwrap_or_default();

        if models.is_empty() {
            return Err(StorageError::ModelNotFound {
                model_id: format!("latest (no models exist for store {store_id})"),
            });
        }

        // Sort and return the first (newest)
        sort_models_newest_first(&mut models);

        Ok(models.into_iter().next().unwrap())
    }

    async fn health_check(&self) -> StorageResult<HealthStatus> {
        // In-memory storage is always healthy - no external dependencies
        Ok(HealthStatus {
            healthy: true,
            latency: std::time::Duration::ZERO,
            pool_stats: None, // No connection pool for in-memory storage
            message: Some("in-memory storage".to_string()),
        })
    }
    async fn list_objects_by_type(
        &self,
        store_id: &str,
        object_type: &str,
        limit: usize,
    ) -> StorageResult<Vec<String>> {
        // Validate inputs
        validate_store_id(store_id)?;
        validate_object_type(object_type)?;

        // Verify store exists
        if !self.stores.contains_key(store_id) {
            return Err(StorageError::StoreNotFound {
                store_id: store_id.to_string(),
            });
        }

        let mut unique_ids: Vec<String> = self
            .tuples
            .get(store_id)
            .map(|tuples| {
                tuples
                    .iter()
                    .filter(|t| t.object_type == object_type)
                    .map(|t| t.object_id.clone())
                    .collect::<HashSet<_>>() // Dedup using HashSet
                    .into_iter()
                    .collect()
            })
            .unwrap_or_default();

        // Sort for deterministic results (matches Postgres ORDER BY)
        unique_ids.sort();

        // Apply limit
        if unique_ids.len() > limit {
            unique_ids.truncate(limit);
        }

        Ok(unique_ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // Test: InMemoryStore can be created
    #[tokio::test]
    async fn test_memory_store_can_be_created() {
        let store = MemoryDataStore::new();
        // Verify the store can be used
        let result = store.list_stores().await.unwrap();
        assert!(result.is_empty());
    }

    // Test: InMemoryStore can be created as shared Arc
    #[tokio::test]
    async fn test_memory_store_shared() {
        let store = MemoryDataStore::new_shared();
        store.create_store("test-id", "Test Store").await.unwrap();

        // Clone Arc and verify state is shared
        let store2 = Arc::clone(&store);
        let retrieved = store2.get_store("test-id").await.unwrap();
        assert_eq!(retrieved.id, "test-id");
    }

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

    // Test: Can write a single tuple
    #[tokio::test]
    async fn test_write_single_tuple() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);

        // Use write_tuple (singular) convenience method
        store.write_tuple("test-store", tuple).await.unwrap();

        let tuples = store
            .read_tuples("test-store", &TupleFilter::default())
            .await
            .unwrap();
        assert_eq!(tuples.len(), 1);
    }

    // Test: Can read back written tuple
    #[tokio::test]
    async fn test_write_and_read_tuple() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);

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

    // Test: Read returns empty vec when no tuples match
    #[tokio::test]
    async fn test_read_returns_empty_when_no_match() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);

        store
            .write_tuples("test-store", vec![tuple], vec![])
            .await
            .unwrap();

        // Search for non-existent object type
        let filter = TupleFilter {
            object_type: Some("folder".to_string()),
            ..Default::default()
        };

        let tuples = store.read_tuples("test-store", &filter).await.unwrap();
        assert!(
            tuples.is_empty(),
            "Expected empty result for non-matching filter"
        );
    }

    // Test: Can write multiple tuples
    #[tokio::test]
    async fn test_write_multiple_tuples() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        let tuples = vec![
            StoredTuple::new("document", "doc1", "viewer", "user", "alice", None),
            StoredTuple::new("document", "doc2", "viewer", "user", "bob", None),
            StoredTuple::new("folder", "folder1", "owner", "user", "charlie", None),
        ];

        store
            .write_tuples("test-store", tuples.clone(), vec![])
            .await
            .unwrap();

        let result = store
            .read_tuples("test-store", &TupleFilter::default())
            .await
            .unwrap();
        assert_eq!(result.len(), 3);
    }

    // Test: Can filter tuples by user
    #[tokio::test]
    async fn test_filter_tuples_by_user() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        let tuples = vec![
            StoredTuple::new("document", "doc1", "viewer", "user", "alice", None),
            StoredTuple::new("document", "doc2", "viewer", "user", "bob", None),
            StoredTuple::new("document", "doc3", "editor", "user", "alice", None),
        ];

        store
            .write_tuples("test-store", tuples, vec![])
            .await
            .unwrap();

        let filter = TupleFilter {
            user: Some("user:alice".to_string()),
            ..Default::default()
        };

        let result = store.read_tuples("test-store", &filter).await.unwrap();
        assert_eq!(result.len(), 2, "Expected 2 tuples for user:alice");
        assert!(result.iter().all(|t| t.user_id == "alice"));
    }

    // Test: Can filter tuples by object
    #[tokio::test]
    async fn test_filter_tuples_by_object() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        let tuples = vec![
            StoredTuple::new("document", "doc1", "viewer", "user", "alice", None),
            StoredTuple::new("document", "doc1", "editor", "user", "bob", None),
            StoredTuple::new("document", "doc2", "viewer", "user", "charlie", None),
        ];

        store
            .write_tuples("test-store", tuples, vec![])
            .await
            .unwrap();

        let filter = TupleFilter {
            object_type: Some("document".to_string()),
            object_id: Some("doc1".to_string()),
            ..Default::default()
        };

        let result = store.read_tuples("test-store", &filter).await.unwrap();
        assert_eq!(result.len(), 2, "Expected 2 tuples for document:doc1");
        assert!(result.iter().all(|t| t.object_id == "doc1"));
    }

    // Test: Can filter tuples by relation
    #[tokio::test]
    async fn test_filter_tuples_by_relation() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        let tuples = vec![
            StoredTuple::new("document", "doc1", "viewer", "user", "alice", None),
            StoredTuple::new("document", "doc1", "editor", "user", "bob", None),
            StoredTuple::new("document", "doc2", "viewer", "user", "charlie", None),
        ];

        store
            .write_tuples("test-store", tuples, vec![])
            .await
            .unwrap();

        let filter = TupleFilter {
            relation: Some("viewer".to_string()),
            ..Default::default()
        };

        let result = store.read_tuples("test-store", &filter).await.unwrap();
        assert_eq!(result.len(), 2, "Expected 2 tuples with viewer relation");
        assert!(result.iter().all(|t| t.relation == "viewer"));
    }

    // Test: Can delete tuple
    #[tokio::test]
    async fn test_delete_tuple() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);

        store
            .write_tuples("test-store", vec![tuple.clone()], vec![])
            .await
            .unwrap();

        // Verify tuple exists
        let result = store
            .read_tuples("test-store", &TupleFilter::default())
            .await
            .unwrap();
        assert_eq!(result.len(), 1);

        // Delete tuple using delete_tuple convenience method
        store.delete_tuple("test-store", tuple).await.unwrap();

        // Verify tuple is gone
        let result = store
            .read_tuples("test-store", &TupleFilter::default())
            .await
            .unwrap();
        assert!(result.is_empty(), "Expected tuple to be deleted");
    }

    // Test: Delete is idempotent (deleting non-existent tuple is ok)
    #[tokio::test]
    async fn test_delete_is_idempotent() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);

        // Delete a tuple that doesn't exist - should succeed
        let result = store.delete_tuple("test-store", tuple.clone()).await;
        assert!(result.is_ok(), "Deleting non-existent tuple should succeed");

        // Write and delete the tuple
        store
            .write_tuple("test-store", tuple.clone())
            .await
            .unwrap();
        store
            .delete_tuple("test-store", tuple.clone())
            .await
            .unwrap();

        // Delete again - should still succeed
        let result = store.delete_tuple("test-store", tuple).await;
        assert!(
            result.is_ok(),
            "Deleting already-deleted tuple should succeed"
        );
    }

    // Test: Concurrent writes don't lose data
    #[tokio::test]
    async fn test_concurrent_writes_dont_lose_data() {
        let store = MemoryDataStore::new_shared();
        store.create_store("test-store", "Test").await.unwrap();

        let num_tasks = 100;
        let mut handles = Vec::with_capacity(num_tasks);

        for i in 0..num_tasks {
            let store = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                let tuple = StoredTuple {
                    object_type: "document".to_string(),
                    object_id: format!("doc{i}"),
                    relation: "viewer".to_string(),
                    user_type: "user".to_string(),
                    user_id: format!("user{i}"),
                    user_relation: None,
                    condition_name: None,
                    condition_context: None,
                };
                store.write_tuple("test-store", tuple).await.unwrap();
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        let result = store
            .read_tuples("test-store", &TupleFilter::default())
            .await
            .unwrap();
        assert_eq!(
            result.len(),
            num_tasks,
            "All concurrent writes should be preserved"
        );
    }

    // Test: Concurrent reads while writing return consistent data
    #[tokio::test]
    async fn test_concurrent_reads_while_writing() {
        let store = MemoryDataStore::new_shared();
        store.create_store("test-store", "Test").await.unwrap();

        // Pre-populate with some data
        for i in 0..50 {
            let tuple = StoredTuple {
                object_type: "document".to_string(),
                object_id: format!("doc{i}"),
                relation: "viewer".to_string(),
                user_type: "user".to_string(),
                user_id: format!("user{i}"),
                user_relation: None,
                condition_name: None,
                condition_context: None,
            };
            store.write_tuple("test-store", tuple).await.unwrap();
        }

        // Start concurrent reads and writes
        let mut handles = Vec::new();

        // Writers
        for i in 50..100 {
            let store = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                let tuple = StoredTuple {
                    object_type: "document".to_string(),
                    object_id: format!("doc{i}"),
                    relation: "viewer".to_string(),
                    user_type: "user".to_string(),
                    user_id: format!("user{i}"),
                    user_relation: None,
                    condition_name: None,
                    condition_context: None,
                };
                store.write_tuple("test-store", tuple).await.unwrap();
            }));
        }

        // Readers - should see consistent state (no partial writes)
        for _ in 0..50 {
            let store = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                let result = store
                    .read_tuples("test-store", &TupleFilter::default())
                    .await
                    .unwrap();
                // Should see at least the initial 50 tuples
                assert!(
                    result.len() >= 50,
                    "Should see at least initial tuples, got {}",
                    result.len()
                );
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        // Final state should have all 100 tuples
        let result = store
            .read_tuples("test-store", &TupleFilter::default())
            .await
            .unwrap();
        assert_eq!(
            result.len(),
            100,
            "Should have all tuples after concurrent operations"
        );
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
        let tuple_without = StoredTuple::new("document", "doc1", "viewer", "group", "eng", None);

        let tuple_with = StoredTuple::new(
            "document",
            "doc1",
            "viewer",
            "group",
            "eng",
            Some("member".to_string()),
        );

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

    // Test: Delete store removes all tuples
    #[tokio::test]
    async fn test_delete_store_removes_tuples() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);

        store.write_tuple("test-store", tuple).await.unwrap();

        // Delete the store
        store.delete_store("test-store").await.unwrap();

        // Store should be gone
        let result = store.get_store("test-store").await;
        assert!(matches!(result, Err(StorageError::StoreNotFound { .. })));

        // Reading tuples from deleted store should fail
        let result = store
            .read_tuples("test-store", &TupleFilter::default())
            .await;
        assert!(matches!(result, Err(StorageError::StoreNotFound { .. })));
    }

    // Test: List stores returns all stores
    #[tokio::test]
    async fn test_list_stores() {
        let store = MemoryDataStore::new();
        store.create_store("store1", "Store 1").await.unwrap();
        store.create_store("store2", "Store 2").await.unwrap();
        store.create_store("store3", "Store 3").await.unwrap();

        let stores = store.list_stores().await.unwrap();
        assert_eq!(stores.len(), 3);

        let ids: Vec<_> = stores.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"store1"));
        assert!(ids.contains(&"store2"));
        assert!(ids.contains(&"store3"));
    }

    // Test: Writing to non-existent store fails
    #[tokio::test]
    async fn test_write_to_nonexistent_store_fails() {
        let store = MemoryDataStore::new();

        let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);

        let result = store.write_tuple("nonexistent", tuple).await;
        assert!(matches!(result, Err(StorageError::StoreNotFound { .. })));
    }

    // Test: Write is idempotent (writing same tuple twice succeeds)
    #[tokio::test]
    async fn test_write_is_idempotent() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);

        // Write the same tuple twice
        store
            .write_tuple("test-store", tuple.clone())
            .await
            .unwrap();
        store.write_tuple("test-store", tuple).await.unwrap();

        // Should only have one tuple
        let result = store
            .read_tuples("test-store", &TupleFilter::default())
            .await
            .unwrap();
        assert_eq!(
            result.len(),
            1,
            "Writing same tuple twice should not create duplicates"
        );
    }

    // Test: Pagination - read tuples with pagination
    #[tokio::test]
    async fn test_read_tuples_paginated() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        // Write 10 tuples
        for i in 0..10 {
            let tuple = StoredTuple {
                object_type: "document".to_string(),
                object_id: format!("doc{i}"),
                relation: "viewer".to_string(),
                user_type: "user".to_string(),
                user_id: format!("user{i}"),
                user_relation: None,
                condition_name: None,
                condition_context: None,
            };
            store.write_tuple("test-store", tuple).await.unwrap();
        }

        // First page of 3
        let pagination = PaginationOptions {
            page_size: Some(3),
            continuation_token: None,
        };
        let result = store
            .read_tuples_paginated("test-store", &TupleFilter::default(), &pagination)
            .await
            .unwrap();
        assert_eq!(result.items.len(), 3);
        assert!(result.continuation_token.is_some());

        // Second page
        let pagination = PaginationOptions {
            page_size: Some(3),
            continuation_token: result.continuation_token,
        };
        let result = store
            .read_tuples_paginated("test-store", &TupleFilter::default(), &pagination)
            .await
            .unwrap();
        assert_eq!(result.items.len(), 3);
        assert!(result.continuation_token.is_some());

        // Last page (only 4 items left, page_size is 5)
        let pagination = PaginationOptions {
            page_size: Some(5),
            continuation_token: result.continuation_token,
        };
        let result = store
            .read_tuples_paginated("test-store", &TupleFilter::default(), &pagination)
            .await
            .unwrap();
        assert_eq!(result.items.len(), 4);
        assert!(result.continuation_token.is_none()); // No more pages
    }

    // Test: Pagination - list stores with pagination
    #[tokio::test]
    async fn test_list_stores_paginated() {
        let store = MemoryDataStore::new();

        // Create 5 stores
        for i in 0..5 {
            store
                .create_store(&format!("store{i}"), &format!("Store {i}"))
                .await
                .unwrap();
        }

        // First page of 2
        let pagination = PaginationOptions {
            page_size: Some(2),
            continuation_token: None,
        };
        let result = store.list_stores_paginated(&pagination).await.unwrap();
        assert_eq!(result.items.len(), 2);
        assert!(result.continuation_token.is_some());

        // Continue to get all stores
        let mut all_stores = result.items;
        let mut token = result.continuation_token;

        while token.is_some() {
            let pagination = PaginationOptions {
                page_size: Some(2),
                continuation_token: token,
            };
            let result = store.list_stores_paginated(&pagination).await.unwrap();
            all_stores.extend(result.items);
            token = result.continuation_token;
        }

        assert_eq!(all_stores.len(), 5);
    }

    // Test: Pagination with filters
    #[tokio::test]
    async fn test_read_tuples_paginated_with_filter() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        // Write 10 tuples with different object types
        for i in 0..10 {
            let tuple = StoredTuple {
                object_type: if i % 2 == 0 {
                    "document".to_string()
                } else {
                    "folder".to_string()
                },
                object_id: format!("obj{i}"),
                relation: "viewer".to_string(),
                user_type: "user".to_string(),
                user_id: format!("user{i}"),
                user_relation: None,
                condition_name: None,
                condition_context: None,
            };
            store.write_tuple("test-store", tuple).await.unwrap();
        }

        // Filter for documents only and paginate
        let filter = TupleFilter {
            object_type: Some("document".to_string()),
            ..Default::default()
        };
        let pagination = PaginationOptions {
            page_size: Some(2),
            continuation_token: None,
        };

        let result = store
            .read_tuples_paginated("test-store", &filter, &pagination)
            .await
            .unwrap();
        assert_eq!(result.items.len(), 2);
        assert!(result.items.iter().all(|t| t.object_type == "document"));

        // Get remaining documents
        let pagination = PaginationOptions {
            page_size: Some(10),
            continuation_token: result.continuation_token,
        };
        let result = store
            .read_tuples_paginated("test-store", &filter, &pagination)
            .await
            .unwrap();
        assert_eq!(result.items.len(), 3); // 5 total documents - 2 already fetched = 3
        assert!(result.continuation_token.is_none());
    }

    // Test: Invalid user filter returns error
    #[tokio::test]
    async fn test_invalid_user_filter_returns_error() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        // Missing colon separator
        let filter = TupleFilter {
            user: Some("invalid".to_string()),
            ..Default::default()
        };
        let result = store.read_tuples("test-store", &filter).await;
        assert!(matches!(result, Err(StorageError::InvalidFilter { .. })));

        // Empty type
        let filter = TupleFilter {
            user: Some(":alice".to_string()),
            ..Default::default()
        };
        let result = store.read_tuples("test-store", &filter).await;
        assert!(matches!(result, Err(StorageError::InvalidFilter { .. })));

        // Too many colons
        let filter = TupleFilter {
            user: Some("user:alice:extra".to_string()),
            ..Default::default()
        };
        let result = store.read_tuples("test-store", &filter).await;
        assert!(matches!(result, Err(StorageError::InvalidFilter { .. })));

        // Invalid userset format (missing colon in type:id part)
        let filter = TupleFilter {
            user: Some("invalid#member".to_string()),
            ..Default::default()
        };
        let result = store.read_tuples("test-store", &filter).await;
        assert!(matches!(result, Err(StorageError::InvalidFilter { .. })));
    }

    // Test: Valid user filter formats work correctly
    #[tokio::test]
    async fn test_valid_user_filter_formats() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        // Direct user format
        let tuple1 = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);

        // Userset format
        let tuple2 = StoredTuple::new(
            "document",
            "doc2",
            "viewer",
            "group",
            "engineering",
            Some("member".to_string()),
        );

        store
            .write_tuples("test-store", vec![tuple1, tuple2], vec![])
            .await
            .unwrap();

        // Filter by direct user format "type:id"
        let filter = TupleFilter {
            user: Some("user:alice".to_string()),
            ..Default::default()
        };
        let result = store.read_tuples("test-store", &filter).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].user_id, "alice");

        // Filter by userset format "type:id#relation"
        let filter = TupleFilter {
            user: Some("group:engineering#member".to_string()),
            ..Default::default()
        };
        let result = store.read_tuples("test-store", &filter).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].user_id, "engineering");
        assert_eq!(result[0].user_relation, Some("member".to_string()));
    }

    // Test: Invalid user filter in paginated query returns error
    #[tokio::test]
    async fn test_invalid_user_filter_paginated_returns_error() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        let filter = TupleFilter {
            user: Some("invalid-format".to_string()),
            ..Default::default()
        };
        let pagination = PaginationOptions::default();

        let result = store
            .read_tuples_paginated("test-store", &filter, &pagination)
            .await;
        assert!(matches!(result, Err(StorageError::InvalidFilter { .. })));
    }

    // ==========================================================================
    // Section 4: Tuple Storage with Conditions
    // ==========================================================================

    // Test: Can write tuple with condition name
    #[tokio::test]
    async fn test_can_write_tuple_with_condition_name() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        let tuple = StoredTuple::with_condition(
            "document",
            "doc1",
            "viewer",
            "user",
            "alice",
            None,
            "time_bound",
            None,
        );

        store.write_tuple("test-store", tuple).await.unwrap();

        let tuples = store
            .read_tuples("test-store", &TupleFilter::default())
            .await
            .unwrap();
        assert_eq!(tuples.len(), 1);
        assert_eq!(tuples[0].condition_name, Some("time_bound".to_string()));
    }

    // Test: Can read tuple with condition
    #[tokio::test]
    async fn test_can_read_tuple_with_condition() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        let mut context = std::collections::HashMap::new();
        context.insert(
            "expires_at".to_string(),
            serde_json::json!("2024-12-31T23:59:59Z"),
        );

        let tuple = StoredTuple::with_condition(
            "document",
            "doc1",
            "viewer",
            "user",
            "alice",
            None,
            "time_bound",
            Some(context.clone()),
        );

        store.write_tuple("test-store", tuple).await.unwrap();

        // Read it back
        let filter = TupleFilter {
            object_type: Some("document".to_string()),
            object_id: Some("doc1".to_string()),
            ..Default::default()
        };
        let tuples = store.read_tuples("test-store", &filter).await.unwrap();

        assert_eq!(tuples.len(), 1);
        assert_eq!(tuples[0].condition_name, Some("time_bound".to_string()));
        assert!(tuples[0].condition_context.is_some());
        let read_context = tuples[0].condition_context.as_ref().unwrap();
        assert_eq!(
            read_context.get("expires_at"),
            Some(&serde_json::json!("2024-12-31T23:59:59Z"))
        );
    }

    // Test: Condition parameters stored in tuple
    #[tokio::test]
    async fn test_condition_parameters_stored_in_tuple() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        let mut context = std::collections::HashMap::new();
        context.insert("max_uses".to_string(), serde_json::json!(10));
        context.insert("region".to_string(), serde_json::json!("us-west-2"));
        context.insert("is_active".to_string(), serde_json::json!(true));

        let tuple = StoredTuple::with_condition(
            "resource",
            "resource1",
            "access",
            "user",
            "bob",
            None,
            "usage_limit",
            Some(context),
        );

        store.write_tuple("test-store", tuple).await.unwrap();

        let tuples = store
            .read_tuples("test-store", &TupleFilter::default())
            .await
            .unwrap();
        assert_eq!(tuples.len(), 1);

        let stored_context = tuples[0].condition_context.as_ref().unwrap();
        assert_eq!(stored_context.get("max_uses"), Some(&serde_json::json!(10)));
        assert_eq!(
            stored_context.get("region"),
            Some(&serde_json::json!("us-west-2"))
        );
        assert_eq!(
            stored_context.get("is_active"),
            Some(&serde_json::json!(true))
        );
    }

    // Test: InMemoryStore stores condition data correctly
    #[tokio::test]
    async fn test_inmemory_store_stores_condition_data() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        // Write tuples with and without conditions
        let tuple_no_condition = StoredTuple::new("doc", "d1", "viewer", "user", "alice", None);

        let mut context = std::collections::HashMap::new();
        context.insert("key".to_string(), serde_json::json!("value"));
        let tuple_with_condition = StoredTuple::with_condition(
            "doc",
            "d2",
            "viewer",
            "user",
            "bob",
            None,
            "cond1",
            Some(context),
        );

        let tuple_condition_only = StoredTuple::with_condition(
            "doc", "d3", "viewer", "user", "charlie", None, "cond2", None,
        );

        store
            .write_tuples(
                "test-store",
                vec![
                    tuple_no_condition.clone(),
                    tuple_with_condition.clone(),
                    tuple_condition_only.clone(),
                ],
                vec![],
            )
            .await
            .unwrap();

        let tuples = store
            .read_tuples("test-store", &TupleFilter::default())
            .await
            .unwrap();
        assert_eq!(tuples.len(), 3);

        // Verify each tuple's condition data
        let d1 = tuples.iter().find(|t| t.object_id == "d1").unwrap();
        assert!(d1.condition_name.is_none());
        assert!(d1.condition_context.is_none());

        let d2 = tuples.iter().find(|t| t.object_id == "d2").unwrap();
        assert_eq!(d2.condition_name, Some("cond1".to_string()));
        assert!(d2.condition_context.is_some());
        assert_eq!(
            d2.condition_context.as_ref().unwrap().get("key"),
            Some(&serde_json::json!("value"))
        );

        let d3 = tuples.iter().find(|t| t.object_id == "d3").unwrap();
        assert_eq!(d3.condition_name, Some("cond2".to_string()));
        assert!(d3.condition_context.is_none());
    }

    // Test: Tuples with same fields but different conditions are stored separately
    #[tokio::test]
    async fn test_tuples_with_different_conditions_stored_separately() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        // Same user/object/relation but different conditions
        let tuple1 = StoredTuple::new("doc", "d1", "viewer", "user", "alice", None);

        let tuple2 = StoredTuple::with_condition(
            "doc", "d1", "viewer", "user", "alice", None, "cond1", None,
        );

        store
            .write_tuples("test-store", vec![tuple1, tuple2], vec![])
            .await
            .unwrap();

        let tuples = store
            .read_tuples("test-store", &TupleFilter::default())
            .await
            .unwrap();

        // Both tuples should be stored since they have different condition_name
        assert_eq!(tuples.len(), 2);

        let without_cond = tuples.iter().find(|t| t.condition_name.is_none()).unwrap();
        let with_cond = tuples
            .iter()
            .find(|t| t.condition_name == Some("cond1".to_string()))
            .unwrap();

        assert_eq!(without_cond.object_id, "d1");
        assert_eq!(with_cond.object_id, "d1");
    }

    // Test: Can query tuples by condition name using TupleFilter
    #[tokio::test]
    async fn test_can_query_tuples_by_condition_name() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        // Write tuples with different conditions
        let tuple_no_cond = StoredTuple::new("doc", "d1", "viewer", "user", "alice", None);
        let tuple_time_bound = StoredTuple::with_condition(
            "doc",
            "d2",
            "viewer",
            "user",
            "bob",
            None,
            "time_bound",
            None,
        );
        let tuple_region = StoredTuple::with_condition(
            "doc",
            "d3",
            "viewer",
            "user",
            "charlie",
            None,
            "region_check",
            None,
        );
        let tuple_time_bound2 = StoredTuple::with_condition(
            "doc",
            "d4",
            "editor",
            "user",
            "dave",
            None,
            "time_bound",
            None,
        );

        store
            .write_tuples(
                "test-store",
                vec![
                    tuple_no_cond,
                    tuple_time_bound,
                    tuple_region,
                    tuple_time_bound2,
                ],
                vec![],
            )
            .await
            .unwrap();

        // Query for tuples with "time_bound" condition
        let filter = TupleFilter {
            condition_name: Some("time_bound".to_string()),
            ..Default::default()
        };
        let tuples = store.read_tuples("test-store", &filter).await.unwrap();
        assert_eq!(tuples.len(), 2);
        assert!(tuples
            .iter()
            .all(|t| t.condition_name == Some("time_bound".to_string())));

        // Query for tuples with "region_check" condition
        let filter = TupleFilter {
            condition_name: Some("region_check".to_string()),
            ..Default::default()
        };
        let tuples = store.read_tuples("test-store", &filter).await.unwrap();
        assert_eq!(tuples.len(), 1);
        assert_eq!(tuples[0].user_id, "charlie");

        // Query for non-existent condition
        let filter = TupleFilter {
            condition_name: Some("nonexistent".to_string()),
            ..Default::default()
        };
        let tuples = store.read_tuples("test-store", &filter).await.unwrap();
        assert!(tuples.is_empty());

        // Query without condition filter returns all
        let tuples = store
            .read_tuples("test-store", &TupleFilter::default())
            .await
            .unwrap();
        assert_eq!(tuples.len(), 4);
    }

    // Test: Can combine condition filter with other filters
    #[tokio::test]
    async fn test_can_combine_condition_filter_with_other_filters() {
        let store = MemoryDataStore::new();
        store.create_store("test-store", "Test").await.unwrap();

        // Write tuples with various attributes and conditions
        let tuples = vec![
            StoredTuple::with_condition(
                "doc",
                "d1",
                "viewer",
                "user",
                "alice",
                None,
                "time_bound",
                None,
            ),
            StoredTuple::with_condition(
                "doc",
                "d1",
                "editor",
                "user",
                "alice",
                None,
                "time_bound",
                None,
            ),
            StoredTuple::with_condition(
                "doc",
                "d2",
                "viewer",
                "user",
                "alice",
                None,
                "time_bound",
                None,
            ),
            StoredTuple::with_condition(
                "doc",
                "d1",
                "viewer",
                "user",
                "bob",
                None,
                "region_check",
                None,
            ),
        ];

        store
            .write_tuples("test-store", tuples, vec![])
            .await
            .unwrap();

        // Combine condition filter with object filter
        let filter = TupleFilter {
            object_type: Some("doc".to_string()),
            object_id: Some("d1".to_string()),
            condition_name: Some("time_bound".to_string()),
            ..Default::default()
        };
        let result = store.read_tuples("test-store", &filter).await.unwrap();
        assert_eq!(result.len(), 2); // viewer and editor for alice on d1 with time_bound

        // Combine condition filter with relation filter
        let filter = TupleFilter {
            relation: Some("viewer".to_string()),
            condition_name: Some("time_bound".to_string()),
            ..Default::default()
        };
        let result = store.read_tuples("test-store", &filter).await.unwrap();
        assert_eq!(result.len(), 2); // d1 and d2 viewers with time_bound

        // Combine condition filter with user filter
        let filter = TupleFilter {
            user: Some("user:alice".to_string()),
            condition_name: Some("time_bound".to_string()),
            ..Default::default()
        };
        let result = store.read_tuples("test-store", &filter).await.unwrap();
        assert_eq!(result.len(), 3); // All alice tuples have time_bound
    }

    // Test: Concurrent create_store calls should not cause race condition
    // This test verifies that the atomic entry API is working correctly
    #[tokio::test]
    async fn test_concurrent_create_store_no_race_condition() {
        let store = Arc::new(MemoryDataStore::new());
        let store_id = "concurrent-test-store";
        let num_tasks = 100;

        // Spawn many concurrent tasks trying to create the same store
        let handles: Vec<_> = (0..num_tasks)
            .map(|i| {
                let store = Arc::clone(&store);
                let name = format!("Store {i}");
                tokio::spawn(async move { store.create_store(store_id, &name).await })
            })
            .collect();

        // Wait for all tasks to complete
        let results: Vec<_> = futures::future::join_all(handles)
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        // Partition into successes and failures using owned values
        let (successes, failures): (Vec<_>, Vec<_>) = results.into_iter().partition(|r| r.is_ok());

        // Exactly one task should succeed
        assert_eq!(
            successes.len(),
            1,
            "Expected exactly one successful create_store, got {}",
            successes.len()
        );

        // All other tasks should fail with StoreAlreadyExists
        assert_eq!(
            failures.len(),
            num_tasks - 1,
            "Expected {} failures, got {}",
            num_tasks - 1,
            failures.len()
        );

        for failure in failures {
            assert!(
                matches!(failure, Err(StorageError::StoreAlreadyExists { .. })),
                "Expected StoreAlreadyExists error, got {failure:?}"
            );
        }

        // Verify only one store exists
        let stores = store.list_stores().await.unwrap();
        assert_eq!(stores.len(), 1, "Expected exactly one store in the list");
        assert_eq!(stores[0].id, store_id);
    }

    // Test: health_check returns healthy status for in-memory storage
    #[tokio::test]
    async fn test_in_memory_health_check_always_returns_healthy() {
        let store = MemoryDataStore::new();

        let status = store.health_check().await.unwrap();

        assert!(status.healthy, "In-memory store should always be healthy");
        assert!(status.pool_stats.is_none(), "In-memory store has no pool");
        assert_eq!(
            status.message,
            Some("in-memory storage".to_string()),
            "Should identify as in-memory storage"
        );
        assert_eq!(
            status.latency,
            std::time::Duration::ZERO,
            "In-memory latency should be zero"
        );
    }


    // Test: list_objects_by_type operations
    // Verifies deduplication, sorting, and pagination behavior
    #[tokio::test]
    async fn test_list_objects_by_type() {
        let store = MemoryDataStore::new();
        let store_id = "list-objects-store";
        store.create_store(store_id, "List Objects Store").await.unwrap();

        // 1. Create duplicate tuples for the same object
        // 2. Create tuples for different objects
        let tuples = vec![
            StoredTuple::new("document", "doc1", "viewer", "user", "alice", None),
            StoredTuple::new("document", "doc1", "editor", "user", "bob", None), // duplicate object
            StoredTuple::new("document", "doc2", "viewer", "user", "charlie", None),
            StoredTuple::new("document", "doc3", "viewer", "user", "dave", None),
            StoredTuple::new("folder", "folder1", "viewer", "user", "alice", None), // different type
        ];

        store.write_tuples(store_id, tuples, vec![]).await.unwrap();

        // Case 1: Simple list of all objects of a type
        let objects = store.list_objects_by_type(store_id, "document", 100).await.unwrap();
        
        // Should be sorted: doc1, doc2, doc3
        // Should be unique: doc1 appears only once despite 2 tuples
        assert_eq!(objects.len(), 3);
        assert_eq!(objects[0], "doc1");
        assert_eq!(objects[1], "doc2");
        assert_eq!(objects[2], "doc3");

        // Case 2: Ensure other types are ignored
        let folders = store.list_objects_by_type(store_id, "folder", 100).await.unwrap();
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0], "folder1");

        // Case 3: Test limit
        let limited = store.list_objects_by_type(store_id, "document", 2).await.unwrap();
        assert_eq!(limited.len(), 2);
        assert_eq!(limited[0], "doc1");
        assert_eq!(limited[1], "doc2");

        // Case 4: Invalid inputs
        assert!(store.list_objects_by_type(store_id, "", 100).await.is_err()); // empty type
        assert!(store.list_objects_by_type("non-existent", "document", 100).await.is_err()); // bad store
    }
}
