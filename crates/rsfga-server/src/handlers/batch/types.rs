//! Data types for batch check operations.

/// Maximum batch size per OpenFGA specification.
/// OpenFGA enforces a limit of 50 items per batch-check request.
pub const MAX_BATCH_SIZE: usize = 50;

/// A single check within a batch request.
#[derive(Debug, Clone)]
pub struct BatchCheckItem {
    /// The user performing the access (e.g., "user:alice").
    pub user: String,
    /// The relation to check (e.g., "viewer").
    pub relation: String,
    /// The object identifier (e.g., "document:readme").
    pub object: String,
}

/// Request for batch permission checks.
#[derive(Debug, Clone)]
pub struct BatchCheckRequest {
    /// The store ID to check against.
    pub store_id: String,
    /// The list of checks to perform.
    pub checks: Vec<BatchCheckItem>,
}

impl BatchCheckRequest {
    /// Creates a new batch check request.
    pub fn new(store_id: impl Into<String>, checks: Vec<BatchCheckItem>) -> Self {
        Self {
            store_id: store_id.into(),
            checks,
        }
    }
}

/// Result of a single check within a batch.
#[derive(Debug, Clone)]
pub struct BatchCheckItemResult {
    /// Whether the check is allowed.
    pub allowed: bool,
    /// Error message if the check failed (optional).
    pub error: Option<String>,
}

/// Response from a batch check operation.
#[derive(Debug, Clone)]
pub struct BatchCheckResponse {
    /// Results for each check, in the same order as the request.
    pub results: Vec<BatchCheckItemResult>,
}

/// Errors that can occur during batch check operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum BatchCheckError {
    /// The batch request is empty.
    #[error("batch request cannot be empty")]
    EmptyBatch,

    /// The batch request exceeds the maximum allowed size.
    #[error("batch size {size} exceeds maximum allowed {max}")]
    BatchTooLarge { size: usize, max: usize },

    /// A check item has invalid format.
    #[error("invalid check at index {index}: {message}")]
    InvalidCheck { index: usize, message: String },

    /// Domain error during check execution.
    #[error("check error: {0}")]
    DomainError(String),
}

impl From<rsfga_domain::error::DomainError> for BatchCheckError {
    fn from(err: rsfga_domain::error::DomainError) -> Self {
        BatchCheckError::DomainError(err.to_string())
    }
}

/// Result type for batch check operations.
pub type BatchCheckResult<T> = Result<T, BatchCheckError>;
