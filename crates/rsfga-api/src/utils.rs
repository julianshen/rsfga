//! Shared utility functions for API layer.
//!
//! Contains common parsing and formatting functions used by both HTTP and gRPC handlers.

/// Maximum number of checks allowed in a single batch request.
/// This matches the OpenFGA specification limit.
pub const MAX_BATCH_SIZE: usize = 50;

/// Parses an object string into (type, id).
///
/// Format: `"document:readme"` → `("document", "readme")`
///
/// Returns `None` if the format is invalid (missing colon separator or empty parts).
pub fn parse_object(object: &str) -> Option<(&str, &str)> {
    let (object_type, object_id) = object.split_once(':')?;
    // Validate both parts are non-empty
    if object_type.is_empty() || object_id.is_empty() {
        return None;
    }
    Some((object_type, object_id))
}

/// Parses a user string into (type, id, optional_relation).
///
/// Supports two formats:
/// - Simple: `"user:alice"` → `("user", "alice", None)`
/// - Userset: `"team:eng#member"` → `("team", "eng", Some("member"))`
///
/// Returns `None` if the format is invalid (missing colon separator).
pub fn parse_user(user: &str) -> Option<(&str, &str, Option<&str>)> {
    // Check for userset format: "team:eng#member"
    if let Some((type_id, relation)) = user.split_once('#') {
        let (user_type, user_id) = type_id.split_once(':')?;
        Some((user_type, user_id, Some(relation)))
    } else {
        // Simple format: "user:alice"
        let (user_type, user_id) = user.split_once(':')?;
        Some((user_type, user_id, None))
    }
}

/// Formats a user for API responses.
///
/// Produces:
/// - `"user:alice"` when `user_relation` is `None`
/// - `"team:eng#member"` when `user_relation` is `Some("member")`
pub fn format_user(user_type: &str, user_id: &str, user_relation: Option<&str>) -> String {
    if let Some(rel) = user_relation {
        format!("{}:{}#{}", user_type, user_id, rel)
    } else {
        format!("{}:{}", user_type, user_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_object_valid() {
        assert_eq!(
            parse_object("document:readme"),
            Some(("document", "readme"))
        );
        assert_eq!(parse_object("folder:root"), Some(("folder", "root")));
    }

    #[test]
    fn test_parse_object_invalid() {
        assert_eq!(parse_object("document"), None); // Missing colon
        assert_eq!(parse_object(":readme"), None); // Empty type
        assert_eq!(parse_object("document:"), None); // Empty id
        assert_eq!(parse_object(""), None); // Empty string
        assert_eq!(parse_object(":"), None); // Just colon
    }

    #[test]
    fn test_parse_user_simple() {
        let result = parse_user("user:alice");
        assert_eq!(result, Some(("user", "alice", None)));
    }

    #[test]
    fn test_parse_user_userset() {
        let result = parse_user("team:eng#member");
        assert_eq!(result, Some(("team", "eng", Some("member"))));
    }

    #[test]
    fn test_parse_user_invalid() {
        assert_eq!(parse_user("invalid"), None);
        assert_eq!(parse_user(""), None);
    }

    #[test]
    fn test_format_user_simple() {
        assert_eq!(format_user("user", "alice", None), "user:alice");
    }

    #[test]
    fn test_format_user_userset() {
        assert_eq!(
            format_user("team", "eng", Some("member")),
            "team:eng#member"
        );
    }
}
