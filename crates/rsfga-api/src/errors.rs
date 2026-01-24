//! Shared error handling utilities for API layers.
//!
//! This module provides a common classification of domain errors that can be
//! used by both HTTP and gRPC layers to map domain errors to their respective
//! error types consistently.
//!
//! # Error Detail Configuration
//!
//! The [`ErrorConfig`] struct controls how much detail is exposed in error messages.
//! In production, you may want to hide internal details like type names, relation names,
//! and object IDs to prevent schema leakage.
//!
//! ```rust
//! use rsfga_api::errors::{ErrorConfig, classify_domain_error_with_config};
//! use rsfga_domain::DomainError;
//!
//! // Production: hide details
//! let config = ErrorConfig::production();
//! let err = DomainError::TypeNotFound { type_name: "secret_document".to_string() };
//! let kind = classify_domain_error_with_config(&err, &config);
//! // Error message won't include "secret_document"
//!
//! // Development: show details
//! let config = ErrorConfig::development();
//! let kind = classify_domain_error_with_config(&err, &config);
//! // Error message will include "secret_document"
//! ```

use rsfga_domain::DomainError;

/// Configuration for error message detail level.
///
/// Controls whether detailed error messages (including type names, relation names,
/// object IDs, etc.) are exposed to clients or replaced with generic messages.
#[derive(Debug, Clone, Default)]
pub struct ErrorConfig {
    /// Whether to include detailed error messages in responses.
    ///
    /// When `true` (development mode):
    /// - Error messages include specific type names, relation names, object IDs
    /// - Useful for debugging during development
    ///
    /// When `false` (production mode):
    /// - Error messages are generic to prevent schema leakage
    /// - Internal error messages are completely hidden
    pub detailed_errors: bool,
}

impl ErrorConfig {
    /// Create a production configuration that hides error details.
    ///
    /// This is the recommended setting for production deployments to prevent
    /// leaking schema information through error messages.
    pub fn production() -> Self {
        Self {
            detailed_errors: false,
        }
    }

    /// Create a development configuration that shows full error details.
    ///
    /// This is useful during development and testing to see exactly what went wrong.
    pub fn development() -> Self {
        Self {
            detailed_errors: true,
        }
    }
}

/// Represents the classification of a domain error for API responses.
///
/// This enum provides a protocol-agnostic classification that can be converted
/// to either HTTP status codes or gRPC status codes.
#[derive(Debug, Clone)]
pub enum DomainErrorKind {
    /// Client error - invalid input from the user (400/INVALID_ARGUMENT)
    InvalidInput(String),
    /// Not found error - resource doesn't exist (404/NOT_FOUND)
    NotFound(String),
    /// Timeout error - operation exceeded time limit (504/DEADLINE_EXCEEDED)
    Timeout(String),
    /// Internal error - unexpected server error (500/INTERNAL)
    Internal(String),
}

/// Classifies a domain error into a protocol-agnostic error kind.
///
/// This function uses generic error messages (production mode) to prevent
/// schema leakage. This is the safe default for all client-facing error responses.
///
/// For debugging or development environments where detailed errors are needed,
/// use [`classify_domain_error_with_config`] with [`ErrorConfig::development()`].
///
/// This function contains the shared logic for determining how domain errors
/// should be presented to clients, ensuring consistent error handling across
/// HTTP and gRPC layers.
pub fn classify_domain_error(err: &DomainError) -> DomainErrorKind {
    classify_domain_error_with_config(err, &ErrorConfig::production())
}

/// Classifies a domain error with configurable detail level.
///
/// When `config.detailed_errors` is `false`, error messages are generic to prevent
/// leaking schema information (type names, relation names, object IDs, etc.).
///
/// # Arguments
///
/// * `err` - The domain error to classify
/// * `config` - Configuration controlling error message detail level
///
/// # Examples
///
/// ```rust
/// use rsfga_api::errors::{ErrorConfig, classify_domain_error_with_config, DomainErrorKind};
/// use rsfga_domain::DomainError;
///
/// let err = DomainError::TypeNotFound { type_name: "secret".to_string() };
///
/// // Production mode - generic message
/// let kind = classify_domain_error_with_config(&err, &ErrorConfig::production());
/// match kind {
///     DomainErrorKind::InvalidInput(msg) => assert!(!msg.contains("secret")),
///     _ => panic!("Expected InvalidInput"),
/// }
///
/// // Development mode - detailed message
/// let kind = classify_domain_error_with_config(&err, &ErrorConfig::development());
/// match kind {
///     DomainErrorKind::InvalidInput(msg) => assert!(msg.contains("secret")),
///     _ => panic!("Expected InvalidInput"),
/// }
/// ```
pub fn classify_domain_error_with_config(
    err: &DomainError,
    config: &ErrorConfig,
) -> DomainErrorKind {
    if config.detailed_errors {
        classify_domain_error_detailed(err)
    } else {
        classify_domain_error_generic(err)
    }
}

/// Classify error with full details (development mode).
fn classify_domain_error_detailed(err: &DomainError) -> DomainErrorKind {
    match err {
        DomainError::DepthLimitExceeded { max_depth } => {
            DomainErrorKind::InvalidInput(format!("depth limit exceeded (max: {max_depth})"))
        }
        DomainError::CycleDetected { path } => {
            DomainErrorKind::InvalidInput(format!("cycle detected in authorization model: {path}"))
        }
        DomainError::Timeout { duration_ms } => {
            DomainErrorKind::Timeout(format!("authorization check timeout after {duration_ms}ms"))
        }
        DomainError::TypeNotFound { type_name } => {
            DomainErrorKind::InvalidInput(format!("type not found: {type_name}"))
        }
        DomainError::RelationNotFound {
            type_name,
            relation,
        } => DomainErrorKind::InvalidInput(format!(
            "relation '{relation}' not found on type '{type_name}'"
        )),
        DomainError::InvalidUserFormat { value } => {
            DomainErrorKind::InvalidInput(format!("invalid user format: {value}"))
        }
        DomainError::InvalidObjectFormat { value } => {
            DomainErrorKind::InvalidInput(format!("invalid object format: {value}"))
        }
        DomainError::InvalidRelationFormat { value } => {
            DomainErrorKind::InvalidInput(format!("invalid relation format: {value}"))
        }
        DomainError::OperationTimeout {
            operation,
            timeout_secs,
        } => DomainErrorKind::Timeout(format!(
            "operation '{operation}' timed out after {timeout_secs}s"
        )),
        DomainError::StoreNotFound { store_id } => {
            DomainErrorKind::NotFound(format!("store not found: {store_id}"))
        }
        // Structured resolver error variants
        DomainError::AuthorizationModelNotFound { store_id } => DomainErrorKind::InvalidInput(
            format!("no authorization model found for store: {store_id}"),
        ),
        DomainError::MissingContextKey { key } => {
            DomainErrorKind::InvalidInput(format!("missing required context parameter: {key}"))
        }
        DomainError::ConditionParseError { expression, reason } => DomainErrorKind::InvalidInput(
            format!("failed to parse condition '{expression}': {reason}"),
        ),
        DomainError::ConditionEvalError { reason } => {
            DomainErrorKind::InvalidInput(format!("condition evaluation failed: {reason}"))
        }
        DomainError::InvalidParameter { parameter, reason } => {
            DomainErrorKind::InvalidInput(format!("invalid parameter '{parameter}': {reason}"))
        }
        DomainError::InvalidFilter { reason } => {
            DomainErrorKind::InvalidInput(format!("invalid filter: {reason}"))
        }
        DomainError::StorageOperationFailed { reason } => {
            DomainErrorKind::Internal(format!("storage operation failed: {reason}"))
        }
        // Legacy resolver error - kept for backwards compatibility
        DomainError::ResolverError { message } => DomainErrorKind::Internal(message.clone()),
        // All other errors are treated as internal errors
        _ => DomainErrorKind::Internal(err.to_string()),
    }
}

/// Classify error with generic messages (production mode).
///
/// This function intentionally hides specific details to prevent schema leakage.
fn classify_domain_error_generic(err: &DomainError) -> DomainErrorKind {
    match err {
        DomainError::DepthLimitExceeded { .. } => {
            DomainErrorKind::InvalidInput("depth limit exceeded".to_string())
        }
        DomainError::CycleDetected { .. } => {
            DomainErrorKind::InvalidInput("cycle detected in authorization model".to_string())
        }
        DomainError::Timeout { .. } => {
            DomainErrorKind::Timeout("authorization check timeout".to_string())
        }
        DomainError::TypeNotFound { .. } => {
            DomainErrorKind::InvalidInput("type not found in authorization model".to_string())
        }
        DomainError::RelationNotFound { .. } => {
            DomainErrorKind::InvalidInput("relation not found on type".to_string())
        }
        DomainError::InvalidUserFormat { .. } => {
            DomainErrorKind::InvalidInput("invalid user format".to_string())
        }
        DomainError::InvalidObjectFormat { .. } => {
            DomainErrorKind::InvalidInput("invalid object format".to_string())
        }
        DomainError::InvalidRelationFormat { .. } => {
            DomainErrorKind::InvalidInput("invalid relation format".to_string())
        }
        DomainError::OperationTimeout { .. } => {
            DomainErrorKind::Timeout("operation timed out".to_string())
        }
        DomainError::StoreNotFound { .. } => {
            DomainErrorKind::NotFound("store not found".to_string())
        }
        // Structured resolver error variants (generic messages)
        DomainError::AuthorizationModelNotFound { .. } => {
            DomainErrorKind::InvalidInput("no authorization model found".to_string())
        }
        DomainError::MissingContextKey { .. } => {
            DomainErrorKind::InvalidInput("missing required context parameter".to_string())
        }
        DomainError::ConditionParseError { .. } => {
            DomainErrorKind::InvalidInput("invalid condition expression".to_string())
        }
        DomainError::ConditionEvalError { .. } => {
            DomainErrorKind::InvalidInput("condition evaluation failed".to_string())
        }
        DomainError::InvalidParameter { .. } => {
            DomainErrorKind::InvalidInput("invalid parameter".to_string())
        }
        DomainError::InvalidFilter { .. } => {
            DomainErrorKind::InvalidInput("invalid filter".to_string())
        }
        DomainError::StorageOperationFailed { .. } => {
            DomainErrorKind::Internal("internal error".to_string())
        }
        // Legacy resolver error - kept for backwards compatibility
        DomainError::ResolverError { .. } => {
            DomainErrorKind::Internal("internal error".to_string())
        }
        // All other errors are treated as internal errors with generic message
        _ => DomainErrorKind::Internal("internal error".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ================================================================
    // Default (Production Mode) Classification Tests
    // ================================================================
    // These tests verify that classify_domain_error() uses production
    // mode by default, hiding specific details like type names and IDs.

    #[test]
    fn test_classify_depth_limit_exceeded_uses_production_mode() {
        let err = DomainError::DepthLimitExceeded { max_depth: 25 };
        match classify_domain_error(&err) {
            DomainErrorKind::InvalidInput(msg) => {
                assert!(msg.contains("depth limit exceeded"));
                // Production mode should NOT include specific depth value
                assert!(
                    !msg.contains("25"),
                    "Production mode should hide depth: {msg}"
                );
            }
            _ => panic!("Expected InvalidInput"),
        }
    }

    #[test]
    fn test_classify_cycle_detected_uses_production_mode() {
        let err = DomainError::CycleDetected {
            path: "a -> b -> a".to_string(),
        };
        match classify_domain_error(&err) {
            DomainErrorKind::InvalidInput(msg) => {
                assert!(msg.contains("cycle detected"));
            }
            _ => panic!("Expected InvalidInput"),
        }
    }

    #[test]
    fn test_classify_timeout_uses_production_mode() {
        let err = DomainError::Timeout { duration_ms: 5000 };
        match classify_domain_error(&err) {
            DomainErrorKind::Timeout(msg) => {
                assert!(msg.contains("timeout"));
                // Production mode should NOT include specific duration
                assert!(
                    !msg.contains("5000"),
                    "Production mode should hide duration: {msg}"
                );
            }
            _ => panic!("Expected Timeout"),
        }
    }

    #[test]
    fn test_classify_store_not_found_uses_production_mode() {
        let err = DomainError::StoreNotFound {
            store_id: "xyz".to_string(),
        };
        match classify_domain_error(&err) {
            DomainErrorKind::NotFound(msg) => {
                assert!(msg.contains("store not found"));
                // Production mode should NOT include specific store ID
                assert!(
                    !msg.contains("xyz"),
                    "Production mode should hide store ID: {msg}"
                );
            }
            _ => panic!("Expected NotFound"),
        }
    }

    #[test]
    fn test_classify_resolver_error_uses_production_mode() {
        let err = DomainError::ResolverError {
            message: "some internal issue".to_string(),
        };
        match classify_domain_error(&err) {
            DomainErrorKind::Internal(msg) => {
                // Production mode should NOT include internal error details
                assert!(
                    !msg.contains("internal issue"),
                    "Production mode should hide details: {msg}"
                );
            }
            _ => panic!("Expected Internal"),
        }
    }

    #[test]
    fn test_classify_type_not_found_uses_production_mode() {
        let err = DomainError::TypeNotFound {
            type_name: "document".to_string(),
        };
        match classify_domain_error(&err) {
            DomainErrorKind::InvalidInput(msg) => {
                assert!(msg.contains("type not found"));
                // Production mode should NOT include specific type name
                assert!(
                    !msg.contains("document"),
                    "Production mode should hide type name: {msg}"
                );
            }
            _ => panic!("Expected InvalidInput"),
        }
    }

    #[test]
    fn test_classify_relation_not_found_uses_production_mode() {
        let err = DomainError::RelationNotFound {
            type_name: "document".to_string(),
            relation: "viewer".to_string(),
        };
        match classify_domain_error(&err) {
            DomainErrorKind::InvalidInput(msg) => {
                assert!(msg.contains("relation"));
                // Production mode should NOT include specific details
                assert!(
                    !msg.contains("viewer"),
                    "Production mode should hide relation: {msg}"
                );
                assert!(
                    !msg.contains("document"),
                    "Production mode should hide type: {msg}"
                );
            }
            _ => panic!("Expected InvalidInput"),
        }
    }

    // ================================================================
    // Development Mode Classification Tests
    // ================================================================
    // These tests verify that detailed error information is available
    // when explicitly using ErrorConfig::development().

    #[test]
    fn test_development_mode_shows_depth_limit() {
        let err = DomainError::DepthLimitExceeded { max_depth: 25 };
        let config = ErrorConfig::development();
        match classify_domain_error_with_config(&err, &config) {
            DomainErrorKind::InvalidInput(msg) => {
                assert!(msg.contains("depth limit exceeded"));
                assert!(
                    msg.contains("25"),
                    "Development mode should show depth: {msg}"
                );
            }
            _ => panic!("Expected InvalidInput"),
        }
    }

    #[test]
    fn test_development_mode_shows_timeout_duration() {
        let err = DomainError::Timeout { duration_ms: 5000 };
        let config = ErrorConfig::development();
        match classify_domain_error_with_config(&err, &config) {
            DomainErrorKind::Timeout(msg) => {
                assert!(msg.contains("timeout"));
                assert!(
                    msg.contains("5000"),
                    "Development mode should show duration: {msg}"
                );
            }
            _ => panic!("Expected Timeout"),
        }
    }

    #[test]
    fn test_development_mode_shows_store_id() {
        let err = DomainError::StoreNotFound {
            store_id: "xyz".to_string(),
        };
        let config = ErrorConfig::development();
        match classify_domain_error_with_config(&err, &config) {
            DomainErrorKind::NotFound(msg) => {
                assert!(msg.contains("store not found"));
                assert!(
                    msg.contains("xyz"),
                    "Development mode should show store ID: {msg}"
                );
            }
            _ => panic!("Expected NotFound"),
        }
    }

    #[test]
    fn test_development_mode_shows_type_and_relation() {
        let err = DomainError::RelationNotFound {
            type_name: "document".to_string(),
            relation: "viewer".to_string(),
        };
        let config = ErrorConfig::development();
        match classify_domain_error_with_config(&err, &config) {
            DomainErrorKind::InvalidInput(msg) => {
                assert!(
                    msg.contains("viewer"),
                    "Development mode should show relation: {msg}"
                );
                assert!(
                    msg.contains("document"),
                    "Development mode should show type: {msg}"
                );
            }
            _ => panic!("Expected InvalidInput"),
        }
    }

    // ================================================================
    // Error Configuration Tests
    // ================================================================

    #[test]
    fn test_error_config_production_hides_type_name() {
        let err = DomainError::TypeNotFound {
            type_name: "secret_document".to_string(),
        };
        let config = ErrorConfig::production();
        match classify_domain_error_with_config(&err, &config) {
            DomainErrorKind::InvalidInput(msg) => {
                assert!(
                    !msg.contains("secret_document"),
                    "Production mode should not expose type name: {msg}"
                );
                assert!(
                    msg.contains("type not found"),
                    "Message should still indicate error type: {msg}"
                );
            }
            _ => panic!("Expected InvalidInput"),
        }
    }

    #[test]
    fn test_error_config_development_shows_type_name() {
        let err = DomainError::TypeNotFound {
            type_name: "secret_document".to_string(),
        };
        let config = ErrorConfig::development();
        match classify_domain_error_with_config(&err, &config) {
            DomainErrorKind::InvalidInput(msg) => {
                assert!(
                    msg.contains("secret_document"),
                    "Development mode should expose type name: {msg}"
                );
            }
            _ => panic!("Expected InvalidInput"),
        }
    }

    #[test]
    fn test_error_config_production_hides_relation_details() {
        let err = DomainError::RelationNotFound {
            type_name: "secret_type".to_string(),
            relation: "secret_relation".to_string(),
        };
        let config = ErrorConfig::production();
        match classify_domain_error_with_config(&err, &config) {
            DomainErrorKind::InvalidInput(msg) => {
                assert!(
                    !msg.contains("secret_type"),
                    "Production mode should not expose type name: {msg}"
                );
                assert!(
                    !msg.contains("secret_relation"),
                    "Production mode should not expose relation name: {msg}"
                );
            }
            _ => panic!("Expected InvalidInput"),
        }
    }

    #[test]
    fn test_error_config_production_hides_store_id() {
        let err = DomainError::StoreNotFound {
            store_id: "secret_store_id".to_string(),
        };
        let config = ErrorConfig::production();
        match classify_domain_error_with_config(&err, &config) {
            DomainErrorKind::NotFound(msg) => {
                assert!(
                    !msg.contains("secret_store_id"),
                    "Production mode should not expose store ID: {msg}"
                );
                assert!(
                    msg.contains("store not found"),
                    "Message should still indicate error type: {msg}"
                );
            }
            _ => panic!("Expected NotFound"),
        }
    }

    #[test]
    fn test_error_config_production_hides_internal_error_details() {
        let err = DomainError::ResolverError {
            message: "database connection failed: secret_connection_string".to_string(),
        };
        let config = ErrorConfig::production();
        match classify_domain_error_with_config(&err, &config) {
            DomainErrorKind::Internal(msg) => {
                assert!(
                    !msg.contains("database"),
                    "Production mode should not expose internal details: {msg}"
                );
                assert!(
                    !msg.contains("secret_connection_string"),
                    "Production mode should not expose sensitive data: {msg}"
                );
                assert_eq!(msg, "internal error", "Should use generic message");
            }
            _ => panic!("Expected Internal"),
        }
    }

    #[test]
    fn test_error_config_default_is_not_detailed() {
        let config = ErrorConfig::default();
        assert!(
            !config.detailed_errors,
            "Default config should be production-safe (not detailed)"
        );
    }
}
