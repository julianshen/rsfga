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

pub mod error;
pub mod memory;
pub mod mysql;
pub mod postgres;
pub mod traits;

// Re-export commonly used types
pub use error::{ConditionConflictError, StorageError, StorageResult};
pub use memory::MemoryDataStore;
pub use mysql::{MySQLConfig, MySQLDataStore};
pub use postgres::{PostgresConfig, PostgresDataStore};
pub use traits::{
    parse_user_filter, validate_store_id, validate_store_name, validate_tuple, DataStore,
    PaginatedResult, PaginationOptions, Store, StoredAuthorizationModel, StoredTuple, TupleFilter,
};

// Re-export chrono types for timestamp handling
pub use chrono::{DateTime, Utc};
