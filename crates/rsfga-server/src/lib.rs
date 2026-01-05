//! rsfga-server: Request handlers and business logic
//!
//! This crate contains the business logic layer including:
//! - Check handler for permission checks
//! - Batch check handler with deduplication
//! - Write handler for tuple operations
//! - Read handler for tuple queries
//! - ListObjects handler
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │               rsfga-server                   │
//! ├─────────────────────────────────────────────┤
//! │  handlers/   - Request handlers             │
//! │    check.rs       - Single check            │
//! │    batch_check.rs - Batch checks            │
//! │    write.rs       - Tuple writes            │
//! │    read.rs        - Tuple reads             │
//! └─────────────────────────────────────────────┘
//! ```

pub mod handlers;

// Re-exports will be added when handlers are implemented
