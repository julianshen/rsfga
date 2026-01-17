//! Graph resolver for permission checks.
//!
//! The resolver performs async graph traversal to determine
//! if a user has a specific permission on an object.
//!
//! # Architecture Decisions
//!
//! - **Parallel Execution (ADR-003)**: Union and intersection operations use
//!   `FuturesUnordered` for parallel branch evaluation with short-circuiting.
//!   Performance benefits are UNVALIDATED - benchmarks needed in M1.7.
//!
//! - **Cycle Detection**: Tracks visited nodes to prevent infinite loops.
//!   Uses `Arc<HashSet>` for efficient cloning during traversal.
//!   Memory growth is O(depth) per path. Profile in M1.7 if needed.
//!
//! - **Depth Limiting**: Default max depth of 25 matches OpenFGA behavior.
//!   Prevents stack overflow and DoS attacks (ADR-003, Constraint C11).
//!
//! - **Timeout Handling**: Configurable timeout (default 30s) prevents
//!   hanging on pathological graphs.
//!
//! # Module Organization
//!
//! - [`config`] - Resolver configuration (depth limits, timeouts)
//! - [`types`] - Request/response types (CheckRequest, CheckResult, etc.)
//! - [`traits`] - Storage traits (TupleReader, ModelReader)
//! - [`graph_resolver`] - Main resolver implementation

mod config;
mod context;
mod graph_resolver;
mod traits;
mod types;

#[cfg(test)]
mod tests;

// Re-export public API
pub use config::ResolverConfig;
pub use graph_resolver::{CacheMetrics, CacheMetricsSnapshot, GraphResolver};
pub use traits::{ModelReader, TupleReader};
pub use types::{
    CheckRequest, CheckResult, ContextualTuple, ExpandLeaf, ExpandLeafValue, ExpandNode,
    ExpandRequest, ExpandResult, StoredTupleRef, UsersetTree,
};
