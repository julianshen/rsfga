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
