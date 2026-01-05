//! DataStore trait definition.

use async_trait::async_trait;

use crate::error::StorageResult;

/// Filter for reading tuples.
#[derive(Debug, Clone, Default)]
pub struct TupleFilter {
    /// Filter by object type.
    pub object_type: Option<String>,
    /// Filter by object ID.
    pub object_id: Option<String>,
    /// Filter by relation.
    pub relation: Option<String>,
    /// Filter by user.
    pub user: Option<String>,
}

/// A stored tuple.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StoredTuple {
    pub object_type: String,
    pub object_id: String,
    pub relation: String,
    pub user_type: String,
    pub user_id: String,
    pub user_relation: Option<String>,
}

/// Store metadata.
#[derive(Debug, Clone)]
pub struct Store {
    pub id: String,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// A stored authorization model.
#[derive(Debug, Clone)]
pub struct StoredModel {
    /// Unique model identifier.
    pub id: String,
    /// Store this model belongs to.
    pub store_id: String,
    /// Schema version (e.g., "1.1").
    pub schema_version: String,
    /// Serialized model data (JSON).
    pub model_json: String,
    /// When the model was created.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Abstract storage interface for authorization data.
///
/// Implementations must be thread-safe (Send + Sync) and support
/// async operations.
#[async_trait]
pub trait DataStore: Send + Sync + 'static {
    // Store operations

    /// Creates a new store.
    async fn create_store(&self, id: &str, name: &str) -> StorageResult<Store>;

    /// Gets a store by ID.
    async fn get_store(&self, id: &str) -> StorageResult<Store>;

    /// Deletes a store.
    async fn delete_store(&self, id: &str) -> StorageResult<()>;

    /// Lists all stores.
    async fn list_stores(&self) -> StorageResult<Vec<Store>>;

    // Tuple operations

    /// Writes tuples to storage (insert and delete).
    async fn write_tuples(
        &self,
        store_id: &str,
        writes: Vec<StoredTuple>,
        deletes: Vec<StoredTuple>,
    ) -> StorageResult<()>;

    /// Reads tuples matching the filter.
    async fn read_tuples(
        &self,
        store_id: &str,
        filter: &TupleFilter,
    ) -> StorageResult<Vec<StoredTuple>>;

    // Model operations

    /// Writes an authorization model to storage.
    ///
    /// Returns the generated model ID.
    async fn write_model(
        &self,
        store_id: &str,
        schema_version: &str,
        model_json: &str,
    ) -> StorageResult<String>;

    /// Reads an authorization model by ID.
    async fn read_model(&self, store_id: &str, model_id: &str) -> StorageResult<StoredModel>;

    /// Reads the latest authorization model for a store.
    async fn read_latest_model(&self, store_id: &str) -> StorageResult<StoredModel>;

    /// Lists all authorization models for a store.
    async fn list_models(&self, store_id: &str) -> StorageResult<Vec<StoredModel>>;
}
