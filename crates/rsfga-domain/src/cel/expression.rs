//! CEL expression parsing and representation

use std::panic;

use cel_interpreter::Program;

use super::{CelError, CelResult};

/// A parsed CEL (Common Expression Language) expression
///
/// This struct holds a compiled CEL expression that can be evaluated
/// against a context containing variables.
///
/// # Example
///
/// ```ignore
/// use rsfga_domain::cel::CelExpression;
///
/// let expr = CelExpression::parse("context.user_department == \"engineering\"")?;
/// // Later: evaluate against a context
/// ```
pub struct CelExpression {
    /// The original source expression
    source: String,
    /// The compiled CEL program
    program: Program,
}

impl std::fmt::Debug for CelExpression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CelExpression")
            .field("source", &self.source)
            .finish()
    }
}

impl CelExpression {
    /// Parse a CEL expression string into a CelExpression
    ///
    /// # Arguments
    ///
    /// * `expression` - A string containing valid CEL syntax
    ///
    /// # Returns
    ///
    /// * `Ok(CelExpression)` - Successfully parsed expression
    /// * `Err(CelError::ParseError)` - Invalid CEL syntax
    ///
    /// # Example
    ///
    /// ```ignore
    /// let expr = CelExpression::parse("a == b")?;
    /// assert_eq!(expr.source(), "a == b");
    /// ```
    pub fn parse(expression: &str) -> CelResult<Self> {
        // The underlying ANTLR parser may panic on some malformed input.
        // We catch panics to provide a clean error instead.
        let expression_owned = expression.to_string();

        let result = panic::catch_unwind(panic::AssertUnwindSafe(|| Program::compile(expression)));

        let program = match result {
            Ok(Ok(program)) => program,
            Ok(Err(e)) => {
                return Err(CelError::ParseError {
                    expression: expression_owned,
                    message: e.to_string(),
                });
            }
            Err(_panic) => {
                return Err(CelError::ParseError {
                    expression: expression_owned,
                    message: "Parser encountered an internal error while parsing this expression"
                        .to_string(),
                });
            }
        };

        Ok(Self {
            source: expression_owned,
            program,
        })
    }

    /// Returns the original source expression
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Returns a reference to the compiled CEL program
    ///
    /// This is useful for evaluating the expression against a context.
    pub fn program(&self) -> &Program {
        &self.program
    }
}
