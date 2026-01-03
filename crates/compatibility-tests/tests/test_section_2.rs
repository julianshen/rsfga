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
    // - user:*
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

    // The ID part should not be empty
    !parts[1].is_empty()
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
    if !object_type.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
        return false;
    }

    // ID should contain only alphanumeric, underscore, hyphen, or pipe (for namespacing)
    if !object_id.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '|') {
        return false;
    }

    true
}
