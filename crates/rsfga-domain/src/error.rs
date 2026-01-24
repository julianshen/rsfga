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

    /// Generic resolver error (legacy - prefer specific variants).
    ///
    /// # Deprecated
    /// This variant is being phased out in favor of specific error variants.
    /// New code should use the structured variants below instead.
    #[error("resolver error: {message}")]
    ResolverError { message: String },

    /// Authorization model not found for store during resolution.
    #[error("authorization model not found for store: {store_id}")]
    AuthorizationModelNotFound { store_id: String },

    /// Missing required context key for condition evaluation.
    #[error("missing required context key: {key}")]
    MissingContextKey { key: String },

    /// Failed to parse condition expression.
    #[error("failed to parse condition '{expression}': {reason}")]
    ConditionParseError { expression: String, reason: String },

    /// Condition evaluation failed.
    #[error("condition evaluation failed: {reason}")]
    ConditionEvalError { reason: String },

    /// Invalid parameter value.
    #[error("invalid parameter '{parameter}': {reason}")]
    InvalidParameter { parameter: String, reason: String },

    /// Invalid filter specification.
    #[error("invalid filter: {reason}")]
    InvalidFilter { reason: String },

    /// Storage operation failed during resolution.
    #[error("storage operation failed: {reason}")]
    StorageOperationFailed { reason: String },

    /// Depth limit exceeded during graph traversal.
    #[error("depth limit exceeded (max: {max_depth})")]
    DepthLimitExceeded { max_depth: u32 },

    /// Timeout during permission check.
    #[error("timeout after {duration_ms}ms")]
    Timeout { duration_ms: u64 },

    /// Operation timeout (for list operations).
    #[error("operation '{operation}' timed out after {timeout_secs}s")]
    OperationTimeout {
        operation: String,
        timeout_secs: u64,
    },

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

    /// Condition not found in authorization model.
    #[error("condition '{condition_name}' not defined in authorization model")]
    ConditionNotFound { condition_name: String },

    /// Store not found.
    #[error("store not found: {store_id}")]
    StoreNotFound { store_id: String },
}

/// Result type for domain operations.
pub type DomainResult<T> = Result<T, DomainError>;
