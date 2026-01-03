use anyhow::Result;

/// Test: Can generate valid User identifiers
#[test]
fn test_can_generate_valid_user_identifiers() -> Result<()> {
    // Arrange: Set up test data
    let user_ids = vec!["alice", "bob", "user123"];

    // Act: Generate user identifiers
    for user_id in user_ids {
        let user_identifier = generate_user_identifier(user_id);

        // Assert: Verify format is correct (user:<id>)
        assert!(
            user_identifier.starts_with("user:"),
            "User identifier should start with 'user:', got: {}",
            user_identifier
        );
        assert!(
            user_identifier.contains(user_id),
            "User identifier should contain the user ID, got: {}",
            user_identifier
        );

        // Verify it's a valid format for OpenFGA
        assert!(
            is_valid_user_format(&user_identifier),
            "User identifier should be valid: {}",
            user_identifier
        );
    }

    // Test wildcard support (for public access patterns)
    assert!(
        is_valid_user_format("user:*"),
        "Wildcard user identifier 'user:*' should be valid"
    );

    // Test negative cases - invalid formats should be rejected
    assert!(!is_valid_user_format(""), "Empty string should be invalid");
    assert!(
        !is_valid_user_format("user"),
        "Missing colon should be invalid"
    );
    assert!(
        !is_valid_user_format(":alice"),
        "Missing user prefix should be invalid"
    );
    assert!(
        !is_valid_user_format("user:"),
        "Missing ID part should be invalid"
    );
    assert!(
        !is_valid_user_format("user: alice"),
        "Whitespace should be invalid"
    );

    Ok(())
}

/// Generate a user identifier in OpenFGA format
fn generate_user_identifier(user_id: &str) -> String {
    format!("user:{}", user_id)
}

/// Check if a string is a valid user identifier format
fn is_valid_user_format(identifier: &str) -> bool {
    // Valid formats:
    // - user:<id>
    // - user:* (wildcard for public access)
    // - Must not be empty
    // - Must not have spaces

    if identifier.is_empty() {
        return false;
    }

    if identifier.contains(' ') {
        return false;
    }

    if !identifier.starts_with("user:") {
        return false;
    }

    let parts: Vec<&str> = identifier.split(':').collect();
    if parts.len() != 2 {
        return false;
    }

    let id = parts[1];

    // The ID part should not be empty
    // Support wildcard (user:*) for public access patterns
    !id.is_empty() && (id == "*" || !id.contains('*'))
}

/// Test: Can generate valid Object identifiers (type:id format)
#[test]
fn test_can_generate_valid_object_identifiers() -> Result<()> {
    // Arrange: Set up test data with different object types
    let test_cases = vec![
        ("document", "readme"),
        ("folder", "planning"),
        ("organization", "acme"),
        ("repo", "rsfga"),
    ];

    // Act: Generate object identifiers
    for (object_type, object_id) in test_cases {
        let object_identifier = generate_object_identifier(object_type, object_id);

        // Assert: Verify format is correct (type:id)
        assert!(
            object_identifier.contains(':'),
            "Object identifier should contain ':', got: {}",
            object_identifier
        );

        let parts: Vec<&str> = object_identifier.split(':').collect();
        assert_eq!(
            parts.len(),
            2,
            "Object identifier should have exactly 2 parts, got: {}",
            parts.len()
        );
        assert_eq!(
            parts[0], object_type,
            "Object type should match, expected: {}, got: {}",
            object_type, parts[0]
        );
        assert_eq!(
            parts[1], object_id,
            "Object ID should match, expected: {}, got: {}",
            object_id, parts[1]
        );

        // Verify it's a valid format for OpenFGA
        assert!(
            is_valid_object_format(&object_identifier),
            "Object identifier should be valid: {}",
            object_identifier
        );
    }

    // Test negative cases - invalid formats should be rejected
    assert!(
        !is_valid_object_format(""),
        "Empty string should be invalid"
    );
    assert!(
        !is_valid_object_format("document"),
        "Missing colon should be invalid"
    );
    assert!(
        !is_valid_object_format(":readme"),
        "Missing type should be invalid"
    );
    assert!(
        !is_valid_object_format("document:"),
        "Missing ID should be invalid"
    );
    assert!(
        !is_valid_object_format("document: readme"),
        "Whitespace should be invalid"
    );

    Ok(())
}

/// Generate an object identifier in OpenFGA format
fn generate_object_identifier(object_type: &str, object_id: &str) -> String {
    format!("{}:{}", object_type, object_id)
}

/// Check if a string is a valid object identifier format
fn is_valid_object_format(identifier: &str) -> bool {
    // Valid format: type:id
    // - Must have exactly one colon
    // - Both type and id must be non-empty
    // - Must not have spaces
    // - Type should be alphanumeric with underscores/hyphens
    // - ID can contain alphanumeric, underscores, hyphens

    if identifier.is_empty() {
        return false;
    }

    if identifier.contains(' ') {
        return false;
    }

    let parts: Vec<&str> = identifier.split(':').collect();
    if parts.len() != 2 {
        return false;
    }

    let object_type = parts[0];
    let object_id = parts[1];

    // Type and ID must be non-empty
    if object_type.is_empty() || object_id.is_empty() {
        return false;
    }

    // Type should contain only alphanumeric, underscore, or hyphen
    if !object_type
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return false;
    }

    // ID should contain only alphanumeric, underscore, hyphen, or pipe (for namespacing)
    if !object_id
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '|')
    {
        return false;
    }

    true
}

/// Test: Can generate valid Relation names
#[test]
fn test_can_generate_valid_relation_names() -> Result<()> {
    // Arrange: Common relation names
    let relation_names = vec![
        "viewer", "editor", "owner", "admin", "member", "can_view", "can_edit",
    ];

    // Act & Assert: Verify each relation name is valid
    for relation_name in relation_names {
        assert!(
            is_valid_relation_name(relation_name),
            "Relation name should be valid: {}",
            relation_name
        );
    }

    Ok(())
}

/// Check if a string is a valid relation name
fn is_valid_relation_name(name: &str) -> bool {
    // Valid relation names:
    // - Must not be empty
    // - Should be alphanumeric with underscores
    // - Must be lowercase (OpenFGA convention)
    // - No spaces

    if name.is_empty() {
        return false;
    }

    if name.contains(' ') {
        return false;
    }

    // Check all characters are lowercase alphanumeric or underscore
    name.chars()
        .all(|c| c.is_lowercase() || c.is_numeric() || c == '_')
}

/// Test: Can generate valid Tuples
#[test]
fn test_can_generate_valid_tuples() -> Result<()> {
    // Arrange: Set up test data
    let store_id = "01HZQK9VXG7J8RQXYZ3MABCDEF"; // Example store ID
    let test_cases = vec![
        ("alice", "viewer", "document", "readme"),
        ("bob", "editor", "folder", "planning"),
        ("charlie", "owner", "organization", "acme"),
    ];

    // Act: Generate tuples
    for (user_id, relation, object_type, object_id) in test_cases {
        let tuple = generate_tuple(store_id, user_id, relation, object_type, object_id);

        // Assert: Verify tuple structure
        assert_eq!(tuple.store_id, store_id, "Tuple store_id should match");
        assert_eq!(
            tuple.user,
            generate_user_identifier(user_id),
            "Tuple user should match"
        );
        assert_eq!(tuple.relation, relation, "Tuple relation should match");
        assert_eq!(
            tuple.object,
            generate_object_identifier(object_type, object_id),
            "Tuple object should match"
        );

        // Verify all components are valid
        assert!(
            !tuple.store_id.is_empty(),
            "Tuple store_id should not be empty"
        );
        assert!(
            is_valid_user_format(&tuple.user),
            "Tuple user should be valid: {}",
            tuple.user
        );
        assert!(
            is_valid_relation_name(&tuple.relation),
            "Tuple relation should be valid: {}",
            tuple.relation
        );
        assert!(
            is_valid_object_format(&tuple.object),
            "Tuple object should be valid: {}",
            tuple.object
        );
    }

    Ok(())
}

/// Represents a relationship tuple in OpenFGA
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Tuple {
    store_id: String,
    user: String,
    relation: String,
    object: String,
}

/// Generate a tuple (store_id, user, relation, object)
fn generate_tuple(
    store_id: &str,
    user_id: &str,
    relation: &str,
    object_type: &str,
    object_id: &str,
) -> Tuple {
    Tuple {
        store_id: store_id.to_string(),
        user: generate_user_identifier(user_id),
        relation: relation.to_string(),
        object: generate_object_identifier(object_type, object_id),
    }
}
