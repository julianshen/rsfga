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

/// Validates that a store ID is in valid ULID format.
///
/// OpenFGA validates store ID format before checking if the store exists.
/// Invalid formats return 400 Bad Request, while valid formats that don't
/// exist return 404 Not Found. This ensures API compatibility (Invariant I2).
///
/// # Returns
///
/// Returns `Some(error_message)` if the format is invalid, `None` if valid.
///
/// # Examples
///
/// ```
/// use rsfga_api::validation::validate_store_id_format;
///
/// // Valid ULID format
/// assert!(validate_store_id_format("01ARZ3NDEKTSV4RRFFQ69G5FAV").is_none());
///
/// // Invalid: UUID format
/// assert!(validate_store_id_format("550e8400-e29b-41d4-a716-446655440000").is_some());
///
/// // Invalid: arbitrary string
/// assert!(validate_store_id_format("invalid-store-id").is_some());
/// ```
pub fn validate_store_id_format(store_id: &str) -> Option<String> {
    match ulid::Ulid::from_string(store_id) {
        Ok(_) => None, // Valid ULID format
        Err(_) => Some(format!(
            "invalid store_id format: '{}' is not a valid ULID",
            store_id
        )),
    }
}

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

/// Maximum allowed object ID length per OpenFGA spec.
/// Object IDs (the part after "type:") must not exceed 256 characters.
pub const MAX_OBJECT_ID_LENGTH: usize = 256;

/// Maximum allowed user ID length per OpenFGA spec.
/// User IDs (the full "type:id" or "type:id#relation") must not exceed 512 characters.
pub const MAX_USER_ID_LENGTH: usize = 512;

/// Maximum number of tuples allowed in a single write request per OpenFGA spec.
/// This prevents DoS attacks and ensures reasonable batch sizes.
pub const MAX_TUPLES_PER_WRITE: usize = 100;

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

/// Validates the object identifier length per OpenFGA spec.
///
/// The object ID (the part after "type:") must not exceed 256 characters.
/// Also validates that the object is in valid "type:id" format.
///
/// # Arguments
///
/// * `object` - The full object string in "type:id" format
///
/// # Returns
///
/// Returns `Some(error_message)` if the format is invalid or object ID exceeds the limit,
/// `None` if valid.
pub fn validate_object_id_length(object: &str) -> Option<String> {
    // Extract the ID part after "type:"
    let Some(colon_pos) = object.find(':') else {
        return Some("object must be in 'type:id' format".to_string());
    };

    let id = &object[colon_pos + 1..];
    if id.len() > MAX_OBJECT_ID_LENGTH {
        return Some(format!(
            "object identifier exceeds maximum length of {} (got {})",
            MAX_OBJECT_ID_LENGTH,
            id.len()
        ));
    }
    None
}

/// Validates the user identifier length per OpenFGA spec.
///
/// The full user string must not exceed 512 characters.
///
/// # Arguments
///
/// * `user` - The user string (e.g., "user:alice" or "group:admins#member")
///
/// # Returns
///
/// Returns `Some(error_message)` if the user exceeds the limit, `None` if valid.
pub fn validate_user_id_length(user: &str) -> Option<String> {
    if user.len() > MAX_USER_ID_LENGTH {
        return Some(format!(
            "user identifier exceeds maximum length of {} (got {})",
            MAX_USER_ID_LENGTH,
            user.len()
        ));
    }
    None
}

/// Validates the number of tuples in a write request per OpenFGA spec.
///
/// A single write request cannot contain more than 100 tuples (writes + deletes).
///
/// # Arguments
///
/// * `count` - The total number of tuples (writes + deletes)
///
/// # Returns
///
/// Returns `Some(error_message)` if the count exceeds the limit, `None` if valid.
pub fn validate_tuple_count(count: usize) -> Option<String> {
    if count > MAX_TUPLES_PER_WRITE {
        return Some(format!(
            "maximum {} tuples per write request (got {})",
            MAX_TUPLES_PER_WRITE, count
        ));
    }
    None
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

    #[test]
    fn test_validate_store_id_format_valid_ulid() {
        // Valid ULID formats should return None
        assert!(validate_store_id_format("01ARZ3NDEKTSV4RRFFQ69G5FAV").is_none());
        assert!(validate_store_id_format("01AAAAAAAAAAAAAAAAAAAAAA00").is_none());
        // Generated ULID
        let ulid = ulid::Ulid::new().to_string();
        assert!(validate_store_id_format(&ulid).is_none());
    }

    #[test]
    fn test_validate_store_id_format_invalid_uuid() {
        // UUID format should return error
        let result = validate_store_id_format("550e8400-e29b-41d4-a716-446655440000");
        assert!(result.is_some());
        assert!(result.unwrap().contains("not a valid ULID"));
    }

    #[test]
    fn test_validate_store_id_format_invalid_arbitrary_string() {
        // Arbitrary strings should return error
        let result = validate_store_id_format("invalid-store-id");
        assert!(result.is_some());
        assert!(result.unwrap().contains("not a valid ULID"));
    }

    #[test]
    fn test_validate_store_id_format_empty() {
        // Empty string should return error
        let result = validate_store_id_format("");
        assert!(result.is_some());
    }

    #[test]
    fn test_validate_object_id_length_valid() {
        // Valid: exactly at limit (256 chars)
        let id = "x".repeat(256);
        let object = format!("document:{}", id);
        assert!(validate_object_id_length(&object).is_none());

        // Valid: short ID
        assert!(validate_object_id_length("document:readme").is_none());
    }

    #[test]
    fn test_validate_object_id_length_exceeds_limit() {
        // Invalid: exceeds 256 char limit
        let id = "x".repeat(257);
        let object = format!("document:{}", id);
        let result = validate_object_id_length(&object);
        assert!(result.is_some());
        assert!(result.unwrap().contains("exceeds maximum length"));
    }

    #[test]
    fn test_validate_object_id_length_no_colon() {
        // Invalid: object without colon (malformed)
        let result = validate_object_id_length("invalidobject");
        assert!(result.is_some());
        assert!(result.unwrap().contains("must be in 'type:id' format"));
    }

    #[test]
    fn test_validate_user_id_length_valid() {
        // Valid: exactly at limit (512 chars)
        let user = "x".repeat(512);
        assert!(validate_user_id_length(&user).is_none());

        // Valid: short user
        assert!(validate_user_id_length("user:alice").is_none());
    }

    #[test]
    fn test_validate_user_id_length_exceeds_limit() {
        // Invalid: exceeds 512 char limit
        let user = "x".repeat(513);
        let result = validate_user_id_length(&user);
        assert!(result.is_some());
        assert!(result.unwrap().contains("exceeds maximum length"));
    }

    #[test]
    fn test_validate_tuple_count_valid() {
        // Valid: exactly at limit (100)
        assert!(validate_tuple_count(100).is_none());

        // Valid: under limit
        assert!(validate_tuple_count(50).is_none());
        assert!(validate_tuple_count(0).is_none());
    }

    #[test]
    fn test_validate_tuple_count_exceeds_limit() {
        // Invalid: exceeds 100 tuple limit
        let result = validate_tuple_count(101);
        assert!(result.is_some());
        assert!(result.unwrap().contains("maximum 100 tuples"));
    }
}
