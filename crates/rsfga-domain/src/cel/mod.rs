//! CEL (Common Expression Language) evaluation module
//!
//! This module provides CEL expression parsing and evaluation for
//! conditional authorization tuples. CEL enables ABAC (Attribute-Based
//! Access Control) patterns in authorization models.
//!
//! # Example
//!
//! ```text
//! // Authorization model defines a condition
//! condition department_match(department: string) {
//!     context.user_department == department
//! }
//!
//! // Tuple uses the condition
//! user:alice#viewer@document:budget[condition: department_match]
//!
//! // Check evaluates with context
//! Check(user:alice, viewer, document:budget, context: {user_department: "finance"})
//! ```
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │               CEL Module                     │
//! ├─────────────────────────────────────────────┤
//! │  CelExpression  - Parsed CEL expression     │
//! │  CelContext     - Evaluation context        │
//! │  CelEvaluator   - Expression evaluator      │
//! │  CelError       - CEL-specific errors       │
//! └─────────────────────────────────────────────┘
//! ```

mod error;
mod expression;

pub use error::CelError;
pub use expression::CelExpression;

/// Result type for CEL operations
pub type CelResult<T> = Result<T, CelError>;

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Section 1: CEL Expression Parser Tests
    // =========================================================================

    /// Test: Can parse simple CEL expression (e.g., `a == b`)
    ///
    /// This is the most basic CEL expression - comparing two identifiers
    /// for equality. The parser should accept valid CEL syntax and return
    /// a CelExpression that can be evaluated later.
    #[test]
    fn test_can_parse_simple_cel_expression() {
        // Arrange
        let expression_str = "a == b";

        // Act
        let result = CelExpression::parse(expression_str);

        // Assert
        assert!(
            result.is_ok(),
            "Should successfully parse simple equality expression: {:?}",
            result.err()
        );

        let expr = result.unwrap();
        assert_eq!(expr.source(), expression_str);
    }

    /// Test: Can parse arithmetic expressions (e.g., `a + b > c`)
    #[test]
    fn test_can_parse_arithmetic_expressions() {
        // Arrange
        let expression_str = "a + b > c";

        // Act
        let result = CelExpression::parse(expression_str);

        // Assert
        assert!(
            result.is_ok(),
            "Should successfully parse arithmetic expression: {:?}",
            result.err()
        );
    }

    /// Test: Can parse comparison operators (==, !=, <, >, <=, >=)
    #[test]
    fn test_can_parse_comparison_operators() {
        let operators = vec![
            ("a == b", "equality"),
            ("a != b", "inequality"),
            ("a < b", "less than"),
            ("a > b", "greater than"),
            ("a <= b", "less than or equal"),
            ("a >= b", "greater than or equal"),
        ];

        for (expr_str, desc) in operators {
            let result = CelExpression::parse(expr_str);
            assert!(
                result.is_ok(),
                "Should parse {} operator '{}': {:?}",
                desc,
                expr_str,
                result.err()
            );
        }
    }

    /// Test: Can parse logical operators (&&, ||, !)
    ///
    /// Logical operators are essential for combining multiple conditions
    /// in authorization rules, e.g., `isOwner || (isMember && hasAccess)`
    #[test]
    fn test_can_parse_logical_operators() {
        let expressions = vec![
            ("a && b", "logical AND"),
            ("a || b", "logical OR"),
            ("!a", "logical NOT"),
            ("a && b || c", "combined AND/OR"),
            ("!(a && b)", "negated group"),
            ("a && !b", "AND with negation"),
        ];

        for (expr_str, desc) in expressions {
            let result = CelExpression::parse(expr_str);
            assert!(
                result.is_ok(),
                "Should parse {} expression '{}': {:?}",
                desc,
                expr_str,
                result.err()
            );
        }
    }

    /// Test: Can parse string operations (contains, startsWith, endsWith)
    ///
    /// String operations are common in conditions like
    /// `context.email.endsWith("@company.com")`
    #[test]
    fn test_can_parse_string_operations() {
        let expressions = vec![
            ("s.contains(\"test\")", "contains"),
            ("s.startsWith(\"prefix\")", "startsWith"),
            ("s.endsWith(\"suffix\")", "endsWith"),
            ("s.size() > 0", "size"),
            ("\"hello\" + \" world\"", "string concatenation"),
        ];

        for (expr_str, desc) in expressions {
            let result = CelExpression::parse(expr_str);
            assert!(
                result.is_ok(),
                "Should parse {} expression '{}': {:?}",
                desc,
                expr_str,
                result.err()
            );
        }
    }

    /// Test: Can parse list operations (in, size, all, exists)
    ///
    /// List operations enable checking membership and collection properties,
    /// e.g., `context.user_roles.exists(r, r == "admin")`
    #[test]
    fn test_can_parse_list_operations() {
        let expressions = vec![
            ("x in [1, 2, 3]", "in operator"),
            ("[1, 2, 3].size()", "list size"),
            ("[1, 2, 3].exists(x, x > 2)", "exists macro"),
            ("[1, 2, 3].all(x, x > 0)", "all macro"),
            ("[1, 2, 3][0]", "list index"),
        ];

        for (expr_str, desc) in expressions {
            let result = CelExpression::parse(expr_str);
            assert!(
                result.is_ok(),
                "Should parse {} expression '{}': {:?}",
                desc,
                expr_str,
                result.err()
            );
        }
    }

    /// Test: Can parse timestamp comparisons
    ///
    /// Timestamp operations enable time-based access control,
    /// e.g., `request.time < context.expires_at`
    #[test]
    fn test_can_parse_timestamp_comparisons() {
        let expressions = vec![
            ("timestamp(\"2024-01-01T00:00:00Z\")", "timestamp literal"),
            ("t1 < t2", "timestamp comparison"),
            (
                "timestamp(\"2024-01-01T00:00:00Z\") < timestamp(\"2024-12-31T23:59:59Z\")",
                "timestamp literals comparison",
            ),
        ];

        for (expr_str, desc) in expressions {
            let result = CelExpression::parse(expr_str);
            assert!(
                result.is_ok(),
                "Should parse {} expression '{}': {:?}",
                desc,
                expr_str,
                result.err()
            );
        }
    }

    /// Test: Parser rejects invalid CEL syntax with clear error
    ///
    /// When users provide invalid CEL, we should return a helpful error message
    /// that helps them understand what went wrong.
    #[test]
    fn test_parser_rejects_invalid_cel_syntax() {
        let invalid_expressions = vec![
            ("a ==", "incomplete expression"),
            ("&&", "missing operands"),
            ("a b c", "missing operator"),
            ("(a", "unclosed parenthesis"),
            ("[1, 2,", "unclosed list"),
            ("\"unclosed string", "unclosed string"),
        ];

        for (expr_str, desc) in invalid_expressions {
            let result = CelExpression::parse(expr_str);
            assert!(
                result.is_err(),
                "Should reject invalid {} expression '{}' but got: {:?}",
                desc,
                expr_str,
                result
            );

            // Verify error contains the original expression
            if let Err(CelError::ParseError {
                expression,
                message,
            }) = result
            {
                assert_eq!(
                    expression, expr_str,
                    "Error should reference the original expression"
                );
                assert!(
                    !message.is_empty(),
                    "Error message should not be empty for '{}'",
                    desc
                );
            }
        }
    }

    /// Test: Parser handles nested expressions
    ///
    /// Complex authorization logic often requires deeply nested expressions
    /// like `(a && (b || c)) && d`
    #[test]
    fn test_parser_handles_nested_expressions() {
        let expressions = vec![
            ("(a)", "simple grouping"),
            ("(a && b)", "grouped AND"),
            ("(a || b) && c", "grouped OR with AND"),
            ("a && (b || c)", "AND with grouped OR"),
            ("((a && b) || (c && d))", "double nested"),
            ("(a + b) * (c - d)", "arithmetic nesting"),
            ("a.b.c.d", "chained member access"),
            ("f(g(h(x)))", "nested function calls"),
        ];

        for (expr_str, desc) in expressions {
            let result = CelExpression::parse(expr_str);
            assert!(
                result.is_ok(),
                "Should parse nested {} expression '{}': {:?}",
                desc,
                expr_str,
                result.err()
            );
        }
    }
}
