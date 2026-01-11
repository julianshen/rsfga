//! Storage error types.

use thiserror::Error;

/// Details for ConditionConflict error (boxed to reduce StorageError size).
/// OpenFGA does not allow updating conditions on existing tuples.
/// To change a condition, delete the tuple and re-create it.
#[derive(Debug, Error)]
#[error("tuple exists with different condition in store {store_id}: {object_type}:{object_id}#{relation}@{user} (existing: {existing_condition:?}, new: {new_condition:?})")]
pub struct ConditionConflictError {
    pub store_id: String,
    pub object_type: String,
    pub object_id: String,
    pub relation: String,
    pub user: String,
    pub existing_condition: Option<String>,
    pub new_condition: Option<String>,
}

/// Storage-specific errors.
#[derive(Debug, Error)]
pub enum StorageError {
    /// Store not found.
    #[error("store not found: {store_id}")]
    StoreNotFound { store_id: String },

    /// Store already exists.
    #[error("store already exists: {store_id}")]
    StoreAlreadyExists { store_id: String },

    /// Model not found.
    #[error("model not found: {model_id}")]
    ModelNotFound { model_id: String },

    /// Tuple not found.
    #[error("tuple not found: {object_type}:{object_id}#{relation}@{user}")]
    TupleNotFound {
        object_type: String,
        object_id: String,
        relation: String,
        user: String,
    },

    /// Duplicate tuple.
    #[error("duplicate tuple: {object_type}:{object_id}#{relation}@{user}")]
    DuplicateTuple {
        object_type: String,
        object_id: String,
        relation: String,
        user: String,
    },

    /// Tuple exists with different condition (409 Conflict in OpenFGA).
    /// OpenFGA does not allow updating conditions on existing tuples.
    /// To change a condition, delete the tuple and re-create it.
    #[error("{0}")]
    ConditionConflict(#[source] Box<ConditionConflictError>),

    /// Database connection error.
    #[error("database connection error: {message}")]
    ConnectionError { message: String },

    /// Database query error.
    #[error("database query error: {message}")]
    QueryError { message: String },

    /// Transaction error.
    #[error("transaction error: {message}")]
    TransactionError { message: String },

    /// Invalid filter error.
    #[error("invalid filter: {message}")]
    InvalidFilter { message: String },

    /// Invalid input error.
    #[error("invalid input: {message}")]
    InvalidInput { message: String },

    /// Serialization error.
    #[error("serialization error: {message}")]
    SerializationError { message: String },

    /// Internal error.
    #[error("internal storage error: {message}")]
    InternalError { message: String },
}

/// Result type for storage operations.
pub type StorageResult<T> = Result<T, StorageError>;
