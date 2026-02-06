//! Shared utility functions for parsing OpenFGA identifiers.
//!
//! These functions are used by both the NATS consumer and the API layer
//! to parse user and object identifiers in a consistent way.

/// Parse a user string into (type, id, optional_relation).
///
/// Formats:
/// - "user:alice" -> ("user", "alice", None)
/// - "team:eng#member" -> ("team", "eng", Some("member"))
///
/// Returns `None` if the format is invalid (no colon separator).
///
/// # Examples
///
/// ```
/// use rsfga_nats::utils::parse_user;
///
/// // Simple user
/// assert_eq!(parse_user("user:alice"), Some(("user", "alice", None)));
///
/// // Userset with relation
/// assert_eq!(parse_user("team:eng#member"), Some(("team", "eng", Some("member"))));
///
/// // Invalid format
/// assert_eq!(parse_user("invalid"), None);
/// ```
pub fn parse_user(user: &str) -> Option<(&str, &str, Option<&str>)> {
    let (type_part, rest) = user.split_once(':')?;

    if let Some((id, relation)) = rest.split_once('#') {
        Some((type_part, id, Some(relation)))
    } else {
        Some((type_part, rest, None))
    }
}

/// Parse an object string into (type, id).
///
/// Format: "document:readme" -> ("document", "readme")
///
/// Returns `None` if the format is invalid (no colon separator).
///
/// # Examples
///
/// ```
/// use rsfga_nats::utils::parse_object;
///
/// assert_eq!(parse_object("document:readme"), Some(("document", "readme")));
/// assert_eq!(parse_object("folder:root"), Some(("folder", "root")));
/// assert_eq!(parse_object("invalid"), None);
/// ```
pub fn parse_object(object: &str) -> Option<(&str, &str)> {
    object.split_once(':')
}

/// Format a user string from components.
///
/// Inverse of `parse_user`.
///
/// # Examples
///
/// ```
/// use rsfga_nats::utils::format_user;
///
/// assert_eq!(format_user("user", "alice", None), "user:alice");
/// assert_eq!(format_user("team", "eng", Some("member")), "team:eng#member");
/// ```
pub fn format_user(user_type: &str, user_id: &str, user_relation: Option<&str>) -> String {
    match user_relation {
        Some(rel) => format!("{}:{}#{}", user_type, user_id, rel),
        None => format!("{}:{}", user_type, user_id),
    }
}

/// Format an object string from components.
///
/// Inverse of `parse_object`.
///
/// # Examples
///
/// ```
/// use rsfga_nats::utils::format_object;
///
/// assert_eq!(format_object("document", "readme"), "document:readme");
/// ```
pub fn format_object(object_type: &str, object_id: &str) -> String {
    format!("{}:{}", object_type, object_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_user_simple() {
        let result = parse_user("user:alice");
        assert_eq!(result, Some(("user", "alice", None)));
    }

    #[test]
    fn test_parse_user_with_relation() {
        let result = parse_user("team:eng#member");
        assert_eq!(result, Some(("team", "eng", Some("member"))));
    }

    #[test]
    fn test_parse_user_invalid() {
        let result = parse_user("invalid");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_user_empty() {
        assert_eq!(parse_user(""), None);
    }

    #[test]
    fn test_parse_user_only_colon() {
        let result = parse_user(":");
        assert_eq!(result, Some(("", "", None)));
    }

    #[test]
    fn test_parse_object_simple() {
        let result = parse_object("document:readme");
        assert_eq!(result, Some(("document", "readme")));
    }

    #[test]
    fn test_parse_object_invalid() {
        let result = parse_object("invalid");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_object_empty() {
        assert_eq!(parse_object(""), None);
    }

    #[test]
    fn test_format_user_simple() {
        assert_eq!(format_user("user", "alice", None), "user:alice");
    }

    #[test]
    fn test_format_user_with_relation() {
        assert_eq!(
            format_user("team", "eng", Some("member")),
            "team:eng#member"
        );
    }

    #[test]
    fn test_format_object() {
        assert_eq!(format_object("document", "readme"), "document:readme");
    }

    #[test]
    fn test_roundtrip_user() {
        let user = "team:eng#member";
        let (t, id, rel) = parse_user(user).unwrap();
        assert_eq!(format_user(t, id, rel), user);
    }

    #[test]
    fn test_roundtrip_object() {
        let object = "document:readme";
        let (t, id) = parse_object(object).unwrap();
        assert_eq!(format_object(t, id), object);
    }
}
