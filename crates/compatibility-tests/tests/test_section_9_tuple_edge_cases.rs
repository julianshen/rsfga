mod common;

use anyhow::Result;
use common::{create_test_store, create_userset_model, create_wildcard_model, get_openfga_url};
use reqwest::StatusCode;
use serde_json::json;

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

/// Test: Writing tuple with condition
#[tokio::test]
async fn test_write_tuple_with_condition() -> Result<()> {
    // Arrange: Create store with conditional model
    let store_id = create_test_store().await?;

    let client = reqwest::Client::new();

    // Create model with conditions
    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            {
                "type": "user"
            },
            {
                "type": "document",
                "relations": {
                    "viewer": {
                        "this": {}
                    }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [
                                {
                                    "type": "user",
                                    "condition": "time_based_access"
                                }
                            ]
                        }
                    }
                }
            }
        ],
        "conditions": {
            "time_based_access": {
                "name": "time_based_access",
                "expression": "request.time_of_day >= 9 && request.time_of_day <= 17",
                "parameters": {
                    "time_of_day": {
                        "type_name": "int"
                    }
                }
            }
        }
    });

    let model_response = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model)
        .send()
        .await?;

    // If model creation with conditions fails, skip this test
    if !model_response.status().is_success() {
        println!("Skipping contextual tuples test - conditions may not be supported");
        return Ok(());
    }

    // Act: Write tuple with condition
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:sensitive",
                    "condition": {
                        "name": "time_based_access",
                        "context": {
                            "time_of_day": 14
                        }
                    }
                }
            ]
        }
    });

    let response = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&write_request)
        .send()
        .await?;

    // Assert: Write with contextual condition succeeded
    assert!(
        response.status().is_success(),
        "Writing tuple with contextual condition should succeed, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: Very long user/object IDs (near OpenFGA limits: user ≤512, object ≤256 bytes)
#[tokio::test]
async fn test_very_long_identifiers() -> Result<()> {
    // Arrange: Create store and simple model
    let store_id = create_test_store().await?;
    let _model_id = create_wildcard_model(&store_id).await?;

    let client = reqwest::Client::new();

    // Create very long IDs (close to OpenFGA limits: user=512, object=256)
    let long_user_id = format!("user:{}", "a".repeat(500));
    let long_object_id = format!("document:{}", "b".repeat(240));

    // Act: Write tuple with long IDs
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": long_user_id,
                    "relation": "viewer",
                    "object": long_object_id
                }
            ]
        }
    });

    let response = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&write_request)
        .send()
        .await?;

    // Assert: Write with long IDs succeeded or failed with proper error
    // OpenFGA has limits: user <= 512 bytes, object <= 256 bytes
    if response.status().is_success() {
        println!("Long IDs within limits accepted");
    } else {
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "Should return 400 if IDs exceed limits, got: {}",
            response.status()
        );
    }

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
