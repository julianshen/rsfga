mod common;

use anyhow::Result;
use common::{create_test_store, create_userset_model, create_wildcard_model, get_openfga_url};
use reqwest::StatusCode;
use serde_json::json;

// OpenFGA identifier size limits
const USER_ID_MAX_BYTES: usize = 512;
const OBJECT_ID_MAX_BYTES: usize = 256;

/// Test: Writing tuple with user wildcard (user:*)
#[tokio::test]
async fn test_write_tuple_with_wildcard() -> Result<()> {
    // Arrange: Create store and model with wildcard support
    let store_id = create_test_store().await?;
    let _model_id = create_wildcard_model(&store_id).await?;

    let client = reqwest::Client::new();

    // Act: Write tuple with wildcard user
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:*",
                    "relation": "viewer",
                    "object": "document:public"
                }
            ]
        }
    });

    let response = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&write_request)
        .send()
        .await?;

    // Assert: Write succeeded
    assert!(
        response.status().is_success(),
        "Writing tuple with wildcard should succeed, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: Reading tuple with userset relation
#[tokio::test]
async fn test_read_tuple_with_userset() -> Result<()> {
    // Arrange: Create store and model with userset support
    let store_id = create_test_store().await?;
    let _model_id = create_userset_model(&store_id).await?;

    let client = reqwest::Client::new();

    // Write tuple with userset (folder#viewer)
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "folder:parent#viewer",
                    "relation": "viewer",
                    "object": "folder:child"
                }
            ]
        }
    });

    let write_response = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&write_request)
        .send()
        .await?;

    assert!(
        write_response.status().is_success(),
        "Writing userset tuple should succeed"
    );

    // Act: Read the userset tuple
    let read_request = json!({
        "tuple_key": {
            "object": "folder:child"
        }
    });

    let read_response = client
        .post(format!("{}/stores/{}/read", get_openfga_url(), store_id))
        .json(&read_request)
        .send()
        .await?;

    let read_body: serde_json::Value = read_response.json().await?;
    let tuples = read_body
        .get("tuples")
        .and_then(|v| v.as_array())
        .expect("tuples field should be an array");

    // Assert: Userset tuple was written and can be read
    assert_eq!(
        tuples.len(),
        1,
        "Should find the userset tuple, got: {}",
        tuples.len()
    );

    let user = tuples[0]
        .get("key")
        .and_then(|k| k.get("user"))
        .and_then(|v| v.as_str())
        .expect("Tuple should have user field");

    assert_eq!(
        user, "folder:parent#viewer",
        "User should be the userset"
    );

    Ok(())
}

/// Test: Identifiers at OpenFGA size limits should be accepted
#[tokio::test]
async fn test_identifiers_at_size_limit() -> Result<()> {
    // Arrange: Create store and simple model
    let store_id = create_test_store().await?;
    let _model_id = create_wildcard_model(&store_id).await?;

    let client = reqwest::Client::new();

    // Create IDs at exact limits
    // Format is "type:id", so we need to account for the prefix length
    let user_id_limit = USER_ID_MAX_BYTES - "user:".len(); // 507 chars
    let object_id_limit = OBJECT_ID_MAX_BYTES - "document:".len(); // 247 chars

    let at_limit_user = format!("user:{}", "a".repeat(user_id_limit));
    let at_limit_object = format!("document:{}", "b".repeat(object_id_limit));

    // Act: Write tuple with IDs at exact limits
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": at_limit_user,
                    "relation": "viewer",
                    "object": at_limit_object
                }
            ]
        }
    });

    let response = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&write_request)
        .send()
        .await?;

    // Assert: IDs at limit should be accepted
    assert!(
        response.status().is_success(),
        "Write with IDs at size limits should succeed, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: Identifiers over OpenFGA size limits should be rejected
#[tokio::test]
async fn test_identifiers_over_size_limit() -> Result<()> {
    // Arrange: Create store and simple model
    let store_id = create_test_store().await?;
    let _model_id = create_wildcard_model(&store_id).await?;

    let client = reqwest::Client::new();

    // Create IDs just over limits
    let user_over_limit = format!("user:{}", "a".repeat(USER_ID_MAX_BYTES + 1)); // 518 bytes total
    let object_over_limit = format!("document:{}", "b".repeat(OBJECT_ID_MAX_BYTES + 1)); // 266 bytes total

    // Act: Write tuple with IDs over limits
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": user_over_limit,
                    "relation": "viewer",
                    "object": object_over_limit
                }
            ]
        }
    });

    let response = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&write_request)
        .send()
        .await?;

    // Assert: IDs over limit should be rejected with 400
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "Write with IDs over size limits should return 400, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: Special characters in user/object IDs
#[tokio::test]
async fn test_special_characters_in_identifiers() -> Result<()> {
    // Arrange: Create store and model
    let store_id = create_test_store().await?;
    let _model_id = create_wildcard_model(&store_id).await?;

    let client = reqwest::Client::new();

    // Test various special characters
    let test_cases = vec![
        ("user:alice@example.com", "document:my-doc"),
        ("user:alice.smith", "document:doc_123"),
        ("user:alice-123", "document:doc.pdf"),
        ("user:alice_test", "document:my|doc"),
    ];

    for (user, object) in test_cases {
        // Act: Write tuple with special characters
        let write_request = json!({
            "writes": {
                "tuple_keys": [
                    {
                        "user": user,
                        "relation": "viewer",
                        "object": object
                    }
                ]
            }
        });

        let response = client
            .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
            .json(&write_request)
            .send()
            .await?;

        // Assert: Common special characters should be accepted
        assert!(
            response.status().is_success(),
            "Write with special chars ({}, {}) should succeed, got: {}",
            user,
            object,
            response.status()
        );
    }

    Ok(())
}

/// Test: Unicode characters in identifiers
#[tokio::test]
async fn test_unicode_in_identifiers() -> Result<()> {
    // Arrange: Create store and model
    let store_id = create_test_store().await?;
    let _model_id = create_wildcard_model(&store_id).await?;

    let client = reqwest::Client::new();

    // Test various Unicode characters (Chinese, Japanese, Emoji, Arabic)
    let test_cases = vec![
        ("user:用户123", "document:文档"),                          // Chinese
        ("user:ユーザー", "document:ドキュメント"),                 // Japanese
        ("user:👤", "document:📄"),                                // Emoji
        ("user:مستخدم", "document:وثيقة"),                        // Arabic
        ("user:Müller", "document:résumé"),                       // Accented Latin
    ];

    for (user, object) in test_cases {
        // Act: Write tuple with Unicode
        let write_request = json!({
            "writes": {
                "tuple_keys": [
                    {
                        "user": user,
                        "relation": "viewer",
                        "object": object
                    }
                ]
            }
        });

        let response = client
            .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
            .json(&write_request)
            .send()
            .await?;

        // Assert: Unicode should be accepted (UTF-8 support)
        assert!(
            response.status().is_success(),
            "Write with Unicode ({}, {}) should succeed, got: {}",
            user,
            object,
            response.status()
        );

        // Try to read it back to verify Unicode round-trip
        let read_request = json!({
            "tuple_key": {
                "user": user,
                "object": object
            }
        });

        let read_response = client
            .post(format!("{}/stores/{}/read", get_openfga_url(), store_id))
            .json(&read_request)
            .send()
            .await?;

        assert!(
            read_response.status().is_success(),
            "Read should succeed for Unicode tuple ({}, {}), got: {}",
            user,
            object,
            read_response.status()
        );

        let read_body: serde_json::Value = read_response.json().await?;
        let tuples = read_body
            .get("tuples")
            .and_then(|v| v.as_array())
            .expect("tuples field should be an array");

        assert_eq!(
            tuples.len(),
            1,
            "Should read back the Unicode tuple for {}, {}",
            user,
            object
        );
    }

    Ok(())
}
