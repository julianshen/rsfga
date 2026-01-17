//! Storage error types.

use std::time::Duration;
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

    /// Query timeout error.
    #[error("query timeout after {timeout:?}: {operation}")]
    QueryTimeout {
        /// The operation that timed out.
        operation: String,
        /// The timeout duration that was exceeded.
        timeout: Duration,
    },

    /// Health check failed.
    #[error("health check failed: {message}")]
    HealthCheckFailed { message: String },

    /// Migration blocked due to data that doesn't fit new column size limits.
    ///
    /// This error is returned when upgrading a database with column size changes
    /// (e.g., for MariaDB/TiDB compatibility) and existing data exceeds the new limits.
    /// The message includes details about which fields have oversized data and
    /// instructions for viewing and fixing the affected rows.
    #[error("{message}")]
    MigrationBlocked { message: String },
}

/// Result type for storage operations.
pub type StorageResult<T> = Result<T, StorageError>;

/// Health status of a storage backend.
///
/// Provides detailed diagnostics about the storage backend's health,
/// including connection pool statistics and database reachability.
#[derive(Debug, Clone, PartialEq)]
pub struct HealthStatus {
    /// Whether the storage is healthy and ready to serve requests.
    pub healthy: bool,
    /// Latency of the health check ping (e.g., `SELECT 1`).
    pub latency: Duration,
    /// Connection pool statistics (if applicable).
    pub pool_stats: Option<PoolStats>,
    /// Optional message with additional details.
    pub message: Option<String>,
}

/// Connection pool statistics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolStats {
    /// Number of connections currently in use.
    pub active_connections: u32,
    /// Number of idle connections available.
    pub idle_connections: u32,
    /// Maximum connections allowed in the pool.
    pub max_connections: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_timeout_error_message() {
        let error = StorageError::QueryTimeout {
            operation: "read_tuples".to_string(),
            timeout: Duration::from_secs(30),
        };

        let message = error.to_string();
        assert!(
            message.contains("read_tuples"),
            "Error message should contain operation name"
        );
        assert!(
            message.contains("30"),
            "Error message should contain timeout duration"
        );
    }

    #[test]
    fn test_health_check_failed_error_message() {
        let error = StorageError::HealthCheckFailed {
            message: "database unreachable".to_string(),
        };

        let message = error.to_string();
        assert!(
            message.contains("database unreachable"),
            "Error message should contain failure reason"
        );
    }

    #[test]
    fn test_health_status_struct() {
        let status = HealthStatus {
            healthy: true,
            latency: Duration::from_millis(5),
            pool_stats: Some(PoolStats {
                active_connections: 3,
                idle_connections: 7,
                max_connections: 10,
            }),
            message: Some("postgresql".to_string()),
        };

        assert!(status.healthy);
        assert_eq!(status.latency, Duration::from_millis(5));

        let pool = status.pool_stats.unwrap();
        assert_eq!(pool.active_connections, 3);
        assert_eq!(pool.idle_connections, 7);
        assert_eq!(pool.max_connections, 10);
    }

    #[test]
    fn test_migration_blocked_error_message() {
        let error = StorageError::MigrationBlocked {
            message:
                "Migration blocked: Found data exceeding limits.\n  - user_id > 128 chars: 3 rows"
                    .to_string(),
        };

        let message = error.to_string();
        assert!(
            message.contains("user_id > 128 chars: 3 rows"),
            "Error message should contain field details"
        );
        assert!(
            message.contains("Migration blocked"),
            "Error message should indicate migration was blocked"
        );
    }
}
