//! Batch check handler with two-stage deduplication.
//!
//! This handler processes multiple permission checks in a single request,
//! optimizing throughput through:
//!
//! 1. **Intra-batch deduplication**: Identical checks execute only once
//! 2. **Singleflight**: Concurrent requests for same check share results
//!
//! Note: Cache integration (stage 3) is handled at the API layer where
//! the CheckCache is consulted before invoking the batch handler.
//!
//! # API Format Adaptation (Deferred to M1.7)
//!
//! This module uses a simplified internal format for the domain/server layer.
//! The HTTP API layer (`rsfga-api`) will adapt to OpenFGA's wire format in
//! Milestone 1.7 when HTTP routes are implemented:
//!
//! - **Request**: OpenFGA uses `correlation_id` and nested `tuple_key` objects.
//!   The API layer will extract these and map to our flat `BatchCheckItem`.
//! - **Response**: OpenFGA returns `{ "result": { "<correlation_id>": {...} } }`.
//!   The API layer will map our `Vec<BatchCheckItemResult>` back using correlation IDs.
//!
//! Note: `rsfga-api/src/http/mod.rs` is currently a placeholder stub.
//!
//! # Performance Target (UNVALIDATED - M1.8)
//!
//! - Batch throughput: >500 checks/s
//! - Deduplication effectiveness: >90% on typical workloads

mod handler;
mod singleflight;
mod types;

pub use handler::BatchCheckHandler;
pub use types::{
    BatchCheckError, BatchCheckItem, BatchCheckItemResult, BatchCheckRequest, BatchCheckResponse,
    BatchCheckResult, MAX_BATCH_SIZE,
};

#[cfg(test)]
mod tests;
