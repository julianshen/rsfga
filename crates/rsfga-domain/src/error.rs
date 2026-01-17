//! Domain error types for authorization operations.

use thiserror::Error;

/// Domain-specific errors for authorization operations.
#[derive(Debug, Error)]
pub enum DomainError {
    /// Error parsing authorization model DSL.
    #[error("model parse error: {message}")]
    ModelParseError { message: String },

    /// Error validating authorization model.
    #[error("model validation error: {message}")]
    ModelValidationError { message: String },

    /// Error during permission check resolution.
    #[error("resolver error: {message}")]
    ResolverError { message: String },

    /// Depth limit exceeded during graph traversal.
    #[error("depth limit exceeded (max: {max_depth})")]
    DepthLimitExceeded { max_depth: u32 },

    /// Timeout during permission check.
    #[error("timeout after {duration_ms}ms")]
    Timeout { duration_ms: u64 },

    /// Cycle detected in relation graph.
    #[error("cycle detected in relation graph: {path}")]
    CycleDetected { path: String },

    /// Invalid user format.
    #[error("invalid user format: {value}")]
    InvalidUserFormat { value: String },

    /// Invalid object format.
    #[error("invalid object format: {value}")]
    InvalidObjectFormat { value: String },

    /// Invalid relation format.
    #[error("invalid relation format: {value}")]
    InvalidRelationFormat { value: String },

    /// Type not found in authorization model.
    #[error("type not found: {type_name}")]
    TypeNotFound { type_name: String },

    /// Relation not found on type.
    #[error("relation '{relation}' not found on type '{type_name}'")]
    RelationNotFound { type_name: String, relation: String },

    /// Store not found.
    #[error("store not found: {store_id}")]
    StoreNotFound { store_id: String },
}

/// Result type for domain operations.
pub type DomainResult<T> = Result<T, DomainError>;
