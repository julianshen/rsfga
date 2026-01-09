//! CEL execution context for variable bindings

use std::collections::HashMap;

use cel_interpreter::objects::Key;
use cel_interpreter::{Context, Value};

/// A context for CEL expression evaluation containing variable bindings
///
/// The context provides variables that can be accessed in CEL expressions
/// using dot notation, e.g., `context.user_department` or `request.expires_at`.
///
/// # Example
///
/// ```ignore
/// use rsfga_domain::cel::{CelContext, CelExpression};
///
/// let mut ctx = CelContext::new();
/// ctx.set_string("department", "engineering");
///
/// let expr = CelExpression::parse("department == \"engineering\"")?;
/// let result = expr.evaluate(&ctx)?;
/// assert_eq!(result.as_bool(), Some(true));
/// ```
#[derive(Debug, Default, Clone)]
pub struct CelContext {
    variables: HashMap<String, CelValue>,
}

/// A value that can be stored in a CEL context
///
/// This enum wraps the different types of values that CEL expressions can work with.
#[derive(Debug, Clone)]
pub enum CelValue {
    /// A boolean value
    Bool(bool),
    /// A 64-bit signed integer
    Int(i64),
    /// A 64-bit floating point number
    Float(f64),
    /// A string value
    String(String),
    /// A list of values
    List(Vec<CelValue>),
    /// A map of string keys to values
    Map(HashMap<String, CelValue>),
    /// A timestamp (RFC3339 string)
    Timestamp(String),
    /// Null value
    Null,
}

impl CelContext {
    /// Create a new empty context
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a boolean variable
    pub fn set_bool(&mut self, name: impl Into<String>, value: bool) {
        self.variables.insert(name.into(), CelValue::Bool(value));
    }

    /// Set an integer variable
    pub fn set_int(&mut self, name: impl Into<String>, value: i64) {
        self.variables.insert(name.into(), CelValue::Int(value));
    }

    /// Set a float variable
    pub fn set_float(&mut self, name: impl Into<String>, value: f64) {
        self.variables.insert(name.into(), CelValue::Float(value));
    }

    /// Set a string variable
    pub fn set_string(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.variables
            .insert(name.into(), CelValue::String(value.into()));
    }

    /// Set a timestamp variable (RFC3339 format)
    ///
    /// The value should be a valid RFC3339 timestamp string (e.g., "2024-01-15T10:30:00Z").
    /// If the value is not a valid RFC3339 timestamp, it will be treated as a regular
    /// string during CEL evaluation rather than a timestamp type. This allows timestamp
    /// comparison expressions to gracefully handle invalid inputs.
    ///
    /// # Example
    ///
    /// ```ignore
    /// ctx.set_timestamp("expires_at", "2024-12-31T23:59:59Z");
    /// ctx.set_timestamp("now", "2024-01-15T10:30:00Z");
    /// // Expression: now < expires_at
    /// ```
    pub fn set_timestamp(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.variables
            .insert(name.into(), CelValue::Timestamp(value.into()));
    }

    /// Set a list variable
    pub fn set_list(&mut self, name: impl Into<String>, values: Vec<CelValue>) {
        self.variables.insert(name.into(), CelValue::List(values));
    }

    /// Set a map variable
    pub fn set_map(&mut self, name: impl Into<String>, values: HashMap<String, CelValue>) {
        self.variables.insert(name.into(), CelValue::Map(values));
    }

    /// Set any CelValue directly
    pub fn set(&mut self, name: impl Into<String>, value: CelValue) {
        self.variables.insert(name.into(), value);
    }

    /// Check if the context is empty
    pub fn is_empty(&self) -> bool {
        self.variables.is_empty()
    }

    /// Convert to cel_interpreter Context
    ///
    /// Note: `add_variable` returns a `Result` which errors on duplicate variable names.
    /// This is safe to use `expect()` because we are iterating over a `HashMap` which
    /// guarantees unique keys, so `add_variable` will never be called with a duplicate name.
    pub(crate) fn to_cel_context(&self) -> Context<'_> {
        let mut ctx = Context::default();

        for (name, value) in &self.variables {
            ctx.add_variable(name.as_str(), cel_value_to_value(value))
                .expect("Failed to add variable to CEL context - this should not happen");
        }

        ctx
    }
}

/// Convert our CelValue to cel_interpreter Value
fn cel_value_to_value(v: &CelValue) -> Value {
    match v {
        CelValue::Bool(b) => Value::Bool(*b),
        CelValue::Int(i) => Value::Int(*i),
        CelValue::Float(f) => Value::Float(*f),
        CelValue::String(s) => Value::String(s.clone().into()),
        CelValue::List(list) => Value::List(
            list.iter()
                .map(cel_value_to_value)
                .collect::<Vec<_>>()
                .into(),
        ),
        CelValue::Map(map) => {
            let converted: HashMap<Key, Value> = map
                .iter()
                .map(|(k, v)| (Key::String(k.clone().into()), cel_value_to_value(v)))
                .collect();
            Value::Map(converted.into())
        }
        CelValue::Timestamp(ts) => {
            // Parse RFC3339 timestamp. If parsing fails, fall back to string type.
            // This allows graceful handling of invalid timestamps in CEL expressions -
            // the expression will still evaluate but timestamp comparisons may fail.
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
                Value::Timestamp(dt)
            } else {
                Value::String(ts.clone().into())
            }
        }
        CelValue::Null => Value::Null,
    }
}

/// Evaluation result that can be converted to Rust types
#[derive(Debug, Clone)]
pub struct CelResult {
    value: Value,
}

impl CelResult {
    /// Create a new CelResult from a Value
    pub(crate) fn new(value: Value) -> Self {
        Self { value }
    }

    /// Get the result as a boolean, if it is one
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match &self.value {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Get the result as an integer, if it is one
    #[must_use]
    pub fn as_int(&self) -> Option<i64> {
        match &self.value {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// Get the result as a string, if it is one
    #[must_use]
    pub fn as_string(&self) -> Option<&str> {
        match &self.value {
            Value::String(s) => Some(s.as_ref()),
            _ => None,
        }
    }

    /// Check if the result is true (for conditions)
    ///
    /// Returns true only if the value is a boolean true.
    /// Any other value (including non-booleans) returns false.
    #[must_use]
    pub fn is_truthy(&self) -> bool {
        self.as_bool().unwrap_or(false)
    }

    /// Get the raw cel_interpreter Value
    #[must_use]
    pub fn raw(&self) -> &Value {
        &self.value
    }
}
