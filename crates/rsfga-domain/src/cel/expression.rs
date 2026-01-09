//! CEL expression parsing and representation

use std::panic;

use cel_interpreter::Program;

use super::context::{CelContext, CelResult as EvalResult};
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

    /// Evaluate the CEL expression against a context
    ///
    /// # Arguments
    ///
    /// * `context` - The context containing variable bindings
    ///
    /// # Returns
    ///
    /// * `Ok(EvalResult)` - The result of evaluating the expression
    /// * `Err(CelError)` - If evaluation fails
    ///
    /// # Example
    ///
    /// ```ignore
    /// let expr = CelExpression::parse("x > 5")?;
    /// let mut ctx = CelContext::new();
    /// ctx.set_int("x", 10);
    /// let result = expr.evaluate(&ctx)?;
    /// assert!(result.is_truthy());
    /// ```
    pub fn evaluate(&self, context: &CelContext) -> CelResult<EvalResult> {
        let cel_ctx = context.to_cel_context();

        // Execute the program with the context
        let result = self.program.execute(&cel_ctx).map_err(|e| {
            // Check if this is a missing variable error
            let error_str = e.to_string();
            if error_str.contains("undefined")
                || error_str.contains("no such key")
                || error_str.contains("not found")
            {
                // Extract variable name if possible
                CelError::EvaluationError {
                    expression: self.source.clone(),
                    message: error_str,
                }
            } else {
                CelError::EvaluationError {
                    expression: self.source.clone(),
                    message: error_str,
                }
            }
        })?;

        Ok(EvalResult::new(result))
    }

    /// Evaluate the expression and return a boolean result
    ///
    /// This is a convenience method for condition evaluation.
    /// Returns `Ok(true)` if the expression evaluates to boolean true,
    /// `Ok(false)` if it evaluates to boolean false,
    /// and `Err` if the expression doesn't evaluate to a boolean.
    pub fn evaluate_bool(&self, context: &CelContext) -> CelResult<bool> {
        let result = self.evaluate(context)?;
        result.as_bool().ok_or_else(|| CelError::TypeError {
            expected: "bool".to_string(),
            actual: format!("{:?}", result.raw()),
        })
    }
}
