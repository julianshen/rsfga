//! Shared error handling utilities for API layers.
//!
//! This module provides a common classification of domain errors that can be
//! used by both HTTP and gRPC layers to map domain errors to their respective
//! error types consistently.

use rsfga_domain::DomainError;

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
/// This function contains the shared logic for determining how domain errors
/// should be presented to clients, ensuring consistent error handling across
/// HTTP and gRPC layers.
pub fn classify_domain_error(err: &DomainError) -> DomainErrorKind {
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
        DomainError::ResolverError { message } => {
            // Check if this is a "store not found" error from the resolver
            if message.starts_with("store not found:") {
                DomainErrorKind::NotFound(message.clone())
            } else if message.contains("No such key:") {
                // Missing context parameter - this is a client error
                DomainErrorKind::InvalidInput(format!(
                    "missing required context parameter: {}",
                    message
                ))
            } else if message.starts_with("condition not found:") {
                // Condition referenced in tuple doesn't exist in model
                DomainErrorKind::InvalidInput(message.clone())
            } else if message.contains("condition evaluation failed:") {
                // CEL evaluation errors due to invalid context
                DomainErrorKind::InvalidInput(message.clone())
            } else {
                DomainErrorKind::Internal(message.clone())
            }
        }
        // All other errors are treated as internal errors
        _ => DomainErrorKind::Internal(err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_depth_limit_exceeded() {
        let err = DomainError::DepthLimitExceeded { max_depth: 25 };
        match classify_domain_error(&err) {
            DomainErrorKind::InvalidInput(msg) => {
                assert!(msg.contains("depth limit exceeded"));
                assert!(msg.contains("25"));
            }
            _ => panic!("Expected InvalidInput"),
        }
    }

    #[test]
    fn test_classify_cycle_detected() {
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
    fn test_classify_timeout() {
        let err = DomainError::Timeout { duration_ms: 5000 };
        match classify_domain_error(&err) {
            DomainErrorKind::Timeout(msg) => {
                assert!(msg.contains("timeout"));
                assert!(msg.contains("5000"));
            }
            _ => panic!("Expected Timeout"),
        }
    }

    #[test]
    fn test_classify_store_not_found() {
        let err = DomainError::ResolverError {
            message: "store not found: xyz".to_string(),
        };
        match classify_domain_error(&err) {
            DomainErrorKind::NotFound(msg) => {
                assert!(msg.contains("store not found"));
            }
            _ => panic!("Expected NotFound"),
        }
    }

    #[test]
    fn test_classify_resolver_error_internal() {
        let err = DomainError::ResolverError {
            message: "some internal issue".to_string(),
        };
        match classify_domain_error(&err) {
            DomainErrorKind::Internal(_) => {}
            _ => panic!("Expected Internal"),
        }
    }

    #[test]
    fn test_classify_type_not_found() {
        let err = DomainError::TypeNotFound {
            type_name: "document".to_string(),
        };
        match classify_domain_error(&err) {
            DomainErrorKind::InvalidInput(msg) => {
                assert!(msg.contains("type not found"));
                assert!(msg.contains("document"));
            }
            _ => panic!("Expected InvalidInput"),
        }
    }

    #[test]
    fn test_classify_relation_not_found() {
        let err = DomainError::RelationNotFound {
            type_name: "document".to_string(),
            relation: "viewer".to_string(),
        };
        match classify_domain_error(&err) {
            DomainErrorKind::InvalidInput(msg) => {
                assert!(msg.contains("relation"));
                assert!(msg.contains("viewer"));
                assert!(msg.contains("document"));
            }
            _ => panic!("Expected InvalidInput"),
        }
    }
}
