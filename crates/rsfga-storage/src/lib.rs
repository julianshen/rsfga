//! rsfga-storage: Storage abstraction layer
//!
//! This crate provides the storage abstraction for RSFGA, including:
//! - DataStore trait for storage operations
//! - In-memory implementation for testing
//! - PostgreSQL implementation for production
//! - MySQL/MariaDB implementation for production
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │               rsfga-storage                  │
//! ├─────────────────────────────────────────────┤
//! │  traits.rs   - DataStore trait definition   │
//! │  memory.rs   - In-memory implementation     │
//! │  postgres.rs - PostgreSQL implementation    │
//! │  mysql.rs    - MySQL/MariaDB implementation │
//! └─────────────────────────────────────────────┘
//! ```
//!
//! # Query Timeout Protection
//!
//! Both PostgreSQL and MySQL implementations include query timeout protection
//! to prevent slow queries from blocking the system (DoS protection). Configure
//! the timeout via `query_timeout_secs` in the config structs:
//!
//! ```rust,ignore
//! let config = PostgresConfig {
//!     database_url: "postgres://localhost/rsfga".to_string(),
//!     query_timeout_secs: 30,  // Default: 30 seconds
//!     ..Default::default()
//! };
//! ```
//!
//! When a query exceeds the timeout, `StorageError::QueryTimeout` is returned
//! with details about the operation and timeout duration.
//!
//! ## Transaction Behavior
//!
//! For transactional operations like `write_tuples`, individual queries within
//! the transaction rely on SQLx pool `acquire_timeout` and database statement
//! timeouts. The pre-transaction store existence check is protected by the
//! configured query timeout.
//!
//! # Health Checks
//!
//! All storage backends implement `health_check()` for Kubernetes liveness and
//! readiness probes. The method returns `HealthStatus` with:
//! - `healthy`: Whether the backend is operational
//! - `latency`: Time taken for the health ping
//! - `pool_stats`: Connection pool statistics (for database backends)
//! - `message`: Backend-specific message (e.g., "postgresql", "mysql")

pub mod error;
pub mod memory;
pub mod mysql;
pub mod postgres;
pub mod traits;

// Re-export commonly used types
pub use error::{ConditionConflictError, HealthStatus, PoolStats, StorageError, StorageResult};
pub use memory::MemoryDataStore;
pub use mysql::{MySQLConfig, MySQLDataStore};
pub use postgres::{PostgresConfig, PostgresDataStore};
pub use traits::{
    parse_continuation_token, parse_user_filter, validate_store_id, validate_store_name,
    validate_tuple, DataStore, PaginatedResult, PaginationOptions, Store, StoredAuthorizationModel,
    StoredTuple, TupleFilter,
};

// Re-export chrono types for timestamp handling
pub use chrono::{DateTime, Utc};
