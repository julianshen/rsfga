//! rsfga-storage: Storage abstraction layer
//!
//! This crate provides the storage abstraction for RSFGA, including:
//! - DataStore trait for storage operations
//! - In-memory implementation for testing
//! - PostgreSQL implementation for production
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
//! └─────────────────────────────────────────────┘
//! ```

pub mod error;
pub mod memory;
pub mod traits;

// PostgreSQL implementation will be added in Milestone 1.3
// pub mod postgres;

// Re-export commonly used types
pub use error::{StorageError, StorageResult};
pub use memory::MemoryDataStore;
pub use traits::DataStore;
