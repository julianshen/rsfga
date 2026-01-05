//! Storage error types.

use thiserror::Error;

/// Storage-specific errors.
#[derive(Debug, Error)]
pub enum StorageError {
    /// Store not found.
    #[error("store not found: {store_id}")]
    StoreNotFound { store_id: String },

    /// Model not found.
    #[error("model not found: {model_id}")]
    ModelNotFound { model_id: String },

    /// Tuple not found.
    #[error("tuple not found")]
    TupleNotFound,

    /// Duplicate tuple.
    #[error("duplicate tuple")]
    DuplicateTuple,

    /// Database connection error.
    #[error("database connection error: {message}")]
    ConnectionError { message: String },

    /// Database query error.
    #[error("database query error: {message}")]
    QueryError { message: String },

    /// Transaction error.
    #[error("transaction error: {message}")]
    TransactionError { message: String },

    /// Serialization error.
    #[error("serialization error: {message}")]
    SerializationError { message: String },

    /// Internal error.
    #[error("internal storage error: {message}")]
    InternalError { message: String },
}

/// Result type for storage operations.
pub type StorageResult<T> = Result<T, StorageError>;
