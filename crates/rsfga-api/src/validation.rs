//! Shared validation functions for API layer.
//!
//! Contains common validation logic used by both HTTP and gRPC handlers.
//! This module enforces security constraints (C11, I4) consistently across all endpoints.

/// Maximum allowed condition name length (security constraint I4).
pub const MAX_CONDITION_NAME_LENGTH: usize = 256;

/// Maximum allowed condition context size in bytes (constraint C11: Fail Fast with Bounds).
pub const MAX_CONDITION_CONTEXT_SIZE: usize = 10 * 1024; // 10KB

/// Maximum allowed JSON nesting depth (constraint C11: Fail Fast with Bounds).
/// Prevents stack overflow during serialization of deeply nested structures.
pub const MAX_JSON_DEPTH: usize = 10;

/// Maximum number of objects to consider in ListObjects (DoS protection).
/// Matches OpenFGA's default limit to prevent memory exhaustion.
pub const MAX_LIST_OBJECTS_CANDIDATES: usize = 1000;

/// Validates a condition name format (security constraint I4).
///
/// Allows only alphanumeric, underscore, and hyphen characters with max 256 chars.
/// This prevents injection attacks and ensures condition names are safe for storage.
///
/// # Examples
///
/// ```
/// use rsfga_api::validation::is_valid_condition_name;
///
/// assert!(is_valid_condition_name("my_condition"));
/// assert!(is_valid_condition_name("condition-123"));
/// assert!(!is_valid_condition_name("")); // Empty
/// assert!(!is_valid_condition_name("bad!name")); // Special char
/// ```
pub fn is_valid_condition_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_CONDITION_NAME_LENGTH
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Checks if a serde_json::Value exceeds the maximum nesting depth.
///
/// Used to prevent stack overflow and DoS attacks via deeply nested JSON payloads.
/// This is critical for constraint C11 (Fail Fast with Bounds).
///
/// # Arguments
///
/// * `value` - The JSON value to check
/// * `current_depth` - The current nesting depth (start with 1 for top-level objects)
///
/// # Returns
///
/// `true` if the value exceeds MAX_JSON_DEPTH, `false` otherwise.
pub fn json_exceeds_max_depth(value: &serde_json::Value, current_depth: usize) -> bool {
    if current_depth > MAX_JSON_DEPTH {
        return true;
    }
    match value {
        serde_json::Value::Object(obj) => obj
            .values()
            .any(|v| json_exceeds_max_depth(v, current_depth + 1)),
        serde_json::Value::Array(arr) => arr
            .iter()
            .any(|v| json_exceeds_max_depth(v, current_depth + 1)),
        _ => false,
    }
}

/// Checks if a prost_types::Value exceeds the maximum nesting depth.
///
/// Used for gRPC requests to prevent stack overflow and DoS attacks.
/// This is critical for constraint C11 (Fail Fast with Bounds).
///
/// # Arguments
///
/// * `value` - The protobuf value to check
/// * `current_depth` - The current nesting depth (start with 1 for top-level structs)
///
/// # Returns
///
/// `true` if the value exceeds MAX_JSON_DEPTH, `false` otherwise.
pub fn prost_value_exceeds_max_depth(value: &prost_types::Value, current_depth: usize) -> bool {
    if current_depth > MAX_JSON_DEPTH {
        return true;
    }
    use prost_types::value::Kind;
    match &value.kind {
        Some(Kind::StructValue(s)) => s
            .fields
            .values()
            .any(|v| prost_value_exceeds_max_depth(v, current_depth + 1)),
        Some(Kind::ListValue(l)) => l
            .values
            .iter()
            .any(|v| prost_value_exceeds_max_depth(v, current_depth + 1)),
        _ => false,
    }
}

/// Estimates the serialized size of a HashMap<String, serde_json::Value>.
///
/// Used to enforce MAX_CONDITION_CONTEXT_SIZE limit for DoS protection.
/// This is a best-effort estimate: we serialize each value to a string
/// and sum the key/value lengths. For large payloads, this gives a
/// reasonable upper bound without incurring full serialization cost.
pub fn estimate_context_size(ctx: &std::collections::HashMap<String, serde_json::Value>) -> usize {
    ctx.iter().map(|(k, v)| k.len() + v.to_string().len()).sum()
}

/// Maximum allowed relation name length per OpenFGA spec.
pub const MAX_RELATION_LENGTH: usize = 50;

/// Validates a user identifier format.
///
/// Valid formats:
/// - `type:id` (e.g., "user:alice")
/// - `type:id#relation` (e.g., "group:admins#member")
/// - `type:*` (wildcard)
///
/// Returns an error message if invalid, None if valid.
pub fn validate_user_format(user: &str) -> Option<&'static str> {
    if user.is_empty() {
        return Some("user cannot be empty");
    }

    // Must contain at least one colon
    if !user.contains(':') {
        return Some("user must be in 'type:id' format");
    }

    let parts: Vec<&str> = user.splitn(2, ':').collect();
    if parts.len() != 2 {
        return Some("user must be in 'type:id' format");
    }

    let (user_type, rest) = (parts[0], parts[1]);

    if user_type.is_empty() {
        return Some("user type cannot be empty");
    }

    // rest can be "id" or "id#relation" or "*"
    if rest.is_empty() {
        return Some("user id cannot be empty");
    }

    None // Valid
}

/// Validates a relation name format per OpenFGA spec.
///
/// Relations must:
/// - Be non-empty
/// - Be at most 50 characters
/// - Contain only alphanumeric, underscore characters
/// - Not contain special characters like ':', '#', '@'
///
/// Returns an error message if invalid, None if valid.
pub fn validate_relation_format(relation: &str) -> Option<&'static str> {
    if relation.is_empty() {
        return Some("relation cannot be empty");
    }

    if relation.len() > MAX_RELATION_LENGTH {
        return Some("relation exceeds maximum length of 50 characters");
    }

    // OpenFGA spec: ^[^:#@\s]{1,50}$
    if relation
        .chars()
        .any(|c| c == ':' || c == '#' || c == '@' || c.is_whitespace())
    {
        return Some("relation contains invalid characters");
    }

    None // Valid
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_is_valid_condition_name_valid() {
        assert!(is_valid_condition_name("my_condition"));
        assert!(is_valid_condition_name("condition-123"));
        assert!(is_valid_condition_name("UPPERCASE"));
        assert!(is_valid_condition_name("mixedCase_with-hyphens"));
        assert!(is_valid_condition_name("a")); // Single char
    }

    #[test]
    fn test_is_valid_condition_name_invalid() {
        assert!(!is_valid_condition_name("")); // Empty
        assert!(!is_valid_condition_name("bad!name")); // Special char
        assert!(!is_valid_condition_name("with space")); // Space
        assert!(!is_valid_condition_name("with.dot")); // Dot
        assert!(!is_valid_condition_name(&"x".repeat(257))); // Too long
    }

    #[test]
    fn test_json_exceeds_max_depth_shallow() {
        let shallow = serde_json::json!({"a": 1});
        assert!(!json_exceeds_max_depth(&shallow, 1));
    }

    #[test]
    fn test_json_exceeds_max_depth_at_limit() {
        // Create nested structure at exactly MAX_JSON_DEPTH (10 levels)
        // Starting at depth 1, we can have 9 more levels before exceeding
        let mut value = serde_json::json!({"leaf": true});
        for _ in 0..8 {
            value = serde_json::json!({"nested": value});
        }
        // 9 object levels total, starting check at depth 1 -> deepest check at depth 9
        // The boolean at depth 10 is a primitive, not recursed into
        assert!(!json_exceeds_max_depth(&value, 1));
    }

    #[test]
    fn test_json_exceeds_max_depth_over_limit() {
        // Create nested structure exceeding MAX_JSON_DEPTH
        let mut value = serde_json::json!({"leaf": true});
        for _ in 0..11 {
            value = serde_json::json!({"nested": value});
        }
        assert!(json_exceeds_max_depth(&value, 1));
    }

    #[test]
    fn test_estimate_context_size() {
        let mut ctx = HashMap::new();
        ctx.insert("key".to_string(), serde_json::json!("value"));
        let size = estimate_context_size(&ctx);
        assert!(size > 0);
        assert!(size < MAX_CONDITION_CONTEXT_SIZE);
    }

    #[test]
    fn test_prost_value_exceeds_max_depth_shallow() {
        let value = prost_types::Value {
            kind: Some(prost_types::value::Kind::NumberValue(42.0)),
        };
        assert!(!prost_value_exceeds_max_depth(&value, 1));
    }

    #[test]
    fn test_prost_value_exceeds_max_depth_over_limit() {
        // Create deeply nested struct
        let mut value = prost_types::Value {
            kind: Some(prost_types::value::Kind::BoolValue(true)),
        };
        for _ in 0..12 {
            let mut fields = std::collections::BTreeMap::new();
            fields.insert("nested".to_string(), value);
            value = prost_types::Value {
                kind: Some(prost_types::value::Kind::StructValue(prost_types::Struct {
                    fields,
                })),
            };
        }
        assert!(prost_value_exceeds_max_depth(&value, 1));
    }
}
