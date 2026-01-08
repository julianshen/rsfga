//! CEL-specific error types

use thiserror::Error;

/// Errors that can occur during CEL expression parsing and evaluation
#[derive(Debug, Error)]
pub enum CelError {
    /// Failed to parse the CEL expression
    #[error("Failed to parse CEL expression: {message}")]
    ParseError {
        /// The expression that failed to parse
        expression: String,
        /// Description of the parse error
        message: String,
    },

    /// Failed to evaluate the CEL expression
    #[error("Failed to evaluate CEL expression: {message}")]
    EvaluationError {
        /// The expression that failed to evaluate
        expression: String,
        /// Description of the evaluation error
        message: String,
    },

    /// A required variable is missing from the context
    #[error("Missing required variable '{variable}' in CEL context")]
    MissingVariable {
        /// Name of the missing variable
        variable: String,
    },

    /// Type mismatch during evaluation
    #[error("Type mismatch: expected {expected}, got {actual}")]
    TypeError {
        /// Expected type
        expected: String,
        /// Actual type received
        actual: String,
    },
}
