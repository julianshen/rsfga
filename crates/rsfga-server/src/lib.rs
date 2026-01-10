//! rsfga-server: Request handlers and business logic
//!
//! This crate contains the business logic layer including:
//! - Check handler for permission checks
//! - Batch check handler with deduplication
//! - Write handler for tuple operations
//! - Read handler for tuple queries
//! - ListObjects handler
//! - Configuration management
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │               rsfga-server                   │
//! ├─────────────────────────────────────────────┤
//! │  config.rs   - Configuration management     │
//! │  handlers/   - Request handlers             │
//! │    check.rs       - Single check            │
//! │    batch_check.rs - Batch checks            │
//! │    write.rs       - Tuple writes            │
//! │    read.rs        - Tuple reads             │
//! └─────────────────────────────────────────────┘
//! ```

pub mod config;
pub mod handlers;

// Re-exports for convenience
pub use config::{ConfigLoadError, ServerConfig};
