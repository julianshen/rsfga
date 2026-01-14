//! CEL expression parsing and representation

use std::panic;
use std::sync::Arc;
use std::time::Duration;

use cel_interpreter::Program;
use tokio::time::timeout;

use super::context::{CelContext, CelResult as EvalResult};
use super::{CelError, CelResult};

/// A parsed CEL (Common Expression Language) expression
///
/// This struct holds a compiled CEL expression that can be evaluated
/// against a context containing variables.
///
/// The compiled `Program` is wrapped in `Arc` to enable efficient sharing
/// across threads for timeout-protected evaluation without re-parsing.
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
    /// The compiled CEL program (wrapped in Arc for thread-safe sharing)
    program: Arc<Program>,
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
            program: Arc::new(program),
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
        let cel_ctx = context.to_cel_context()?;

        // Execute the program with the context
        let result = self
            .program
            .execute(&cel_ctx)
            .map_err(|e| CelError::EvaluationError {
                expression: self.source.clone(),
                message: e.to_string(),
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

    /// Evaluate the CEL expression with a timeout to prevent DoS attacks
    ///
    /// This method wraps evaluation in an async timeout. If evaluation takes
    /// longer than the specified duration, it returns a `CelError::Timeout`.
    ///
    /// # Arguments
    ///
    /// * `context` - The context containing variable bindings
    /// * `timeout_duration` - Maximum time allowed for evaluation
    ///
    /// # Returns
    ///
    /// * `Ok(EvalResult)` - The result of evaluating the expression
    /// * `Err(CelError::Timeout)` - If evaluation exceeds the timeout
    /// * `Err(CelError::*)` - Other evaluation errors
    ///
    /// # Example
    ///
    /// ```ignore
    /// let expr = CelExpression::parse("x > 5")?;
    /// let mut ctx = CelContext::new();
    /// ctx.set_int("x", 10);
    /// let result = expr.evaluate_with_timeout(&ctx, Duration::from_millis(100)).await?;
    /// assert!(result.is_truthy());
    /// ```
    pub async fn evaluate_with_timeout(
        &self,
        context: &CelContext,
        timeout_duration: Duration,
    ) -> CelResult<EvalResult> {
        let source = self.source.clone();
        let program = Arc::clone(&self.program);
        let ctx_data = context.clone();
        let duration_ms = timeout_duration.as_millis() as u64;

        // Spawn blocking evaluation in a separate thread.
        // The Program is wrapped in Arc and is Send+Sync, so we can share it
        // across threads without re-parsing.
        let eval_future = tokio::task::spawn_blocking(move || {
            let cel_ctx = ctx_data.to_cel_context()?;
            let result = program
                .execute(&cel_ctx)
                .map_err(|e| CelError::EvaluationError {
                    expression: source.clone(),
                    message: e.to_string(),
                })?;

            Ok(EvalResult::new(result))
        });

        // Apply timeout
        match timeout(timeout_duration, eval_future).await {
            Ok(Ok(result)) => result,
            Ok(Err(join_error)) => Err(CelError::EvaluationError {
                expression: self.source.clone(),
                message: format!("Task join error: {}", join_error),
            }),
            Err(_elapsed) => Err(CelError::Timeout {
                expression: self.source.clone(),
                duration_ms,
            }),
        }
    }

    /// Evaluate the expression with timeout and return a boolean result
    ///
    /// Combines timeout protection with boolean result extraction.
    pub async fn evaluate_bool_with_timeout(
        &self,
        context: &CelContext,
        timeout_duration: Duration,
    ) -> CelResult<bool> {
        let result = self
            .evaluate_with_timeout(context, timeout_duration)
            .await?;
        result.as_bool().ok_or_else(|| CelError::TypeError {
            expected: "bool".to_string(),
            actual: format!("{:?}", result.raw()),
        })
    }
}
