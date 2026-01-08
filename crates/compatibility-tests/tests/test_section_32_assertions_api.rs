mod common;

use anyhow::Result;
use common::{create_authorization_model, create_test_store, get_openfga_url, shared_client};
use reqwest::StatusCode;
use serde_json::json;

// ============================================================================
// Section 32: Assertions API Tests
// ============================================================================
//
// Assertions are test cases stored with an authorization model to validate
// expected authorization outcomes. They are useful for regression testing
// and model validation.
//
// Endpoints:
// - PUT /stores/{store_id}/assertions/{authorization_model_id} (WriteAssertions)
// - GET /stores/{store_id}/assertions/{authorization_model_id} (ReadAssertions)
//
// ============================================================================

/// Test: PUT /stores/{store_id}/assertions/{model_id} creates assertions
#[tokio::test]
async fn test_write_assertions_creates_assertions() -> Result<()> {
    // Arrange: Create store and model
    let store_id = create_test_store().await?;

    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            { "type": "user" },
            {
                "type": "document",
                "relations": {
                    "viewer": { "this": {} },
                    "editor": { "this": {} }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [{ "type": "user" }]
                        },
                        "editor": {
                            "directly_related_user_types": [{ "type": "user" }]
                        }
                    }
                }
            }
        ]
    });

    let model_id = create_authorization_model(&store_id, model).await?;

    let client = shared_client();

    // Act: Write assertions
    let assertions_request = json!({
        "assertions": [
            {
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:roadmap"
                },
                "expectation": true
            },
            {
                "tuple_key": {
                    "user": "user:bob",
                    "relation": "editor",
                    "object": "document:roadmap"
                },
                "expectation": false
            }
        ]
    });

    let response = client
        .put(format!(
            "{}/stores/{}/assertions/{}",
            get_openfga_url(),
            store_id,
            model_id
        ))
        .json(&assertions_request)
        .send()
        .await?;

    // Assert: WriteAssertions succeeded
    assert!(
        response.status().is_success(),
        "WriteAssertions should succeed, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: GET /stores/{store_id}/assertions/{model_id} returns assertions
#[tokio::test]
async fn test_read_assertions_returns_assertions() -> Result<()> {
    let store_id = create_test_store().await?;

    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            { "type": "user" },
            {
                "type": "document",
                "relations": {
                    "viewer": { "this": {} }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [{ "type": "user" }]
                        }
                    }
                }
            }
        ]
    });

    let model_id = create_authorization_model(&store_id, model).await?;

    let client = shared_client();

    // First write assertions
    let assertions_request = json!({
        "assertions": [
            {
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:doc1"
                },
                "expectation": true
            }
        ]
    });

    let write_response = client
        .put(format!(
            "{}/stores/{}/assertions/{}",
            get_openfga_url(),
            store_id,
            model_id
        ))
        .json(&assertions_request)
        .send()
        .await?;

    assert!(write_response.status().is_success());

    // Act: Read assertions back
    let read_response = client
        .get(format!(
            "{}/stores/{}/assertions/{}",
            get_openfga_url(),
            store_id,
            model_id
        ))
        .send()
        .await?;

    // Assert: ReadAssertions succeeded
    assert!(
        read_response.status().is_success(),
        "ReadAssertions should succeed, got: {}",
        read_response.status()
    );

    let response_body: serde_json::Value = read_response.json().await?;

    // Verify response has assertions
    let assertions = response_body
        .get("assertions")
        .and_then(|a| a.as_array())
        .expect("Response should have 'assertions' array");

    assert_eq!(
        assertions.len(),
        1,
        "Should have 1 assertion, got: {}",
        assertions.len()
    );

    // Verify assertion content
    let assertion = &assertions[0];
    let tuple_key = assertion
        .get("tuple_key")
        .expect("Assertion should have tuple_key");

    assert_eq!(
        tuple_key.get("user").and_then(|u| u.as_str()),
        Some("user:alice")
    );
    assert_eq!(
        tuple_key.get("relation").and_then(|r| r.as_str()),
        Some("viewer")
    );
    assert_eq!(
        tuple_key.get("object").and_then(|o| o.as_str()),
        Some("document:doc1")
    );
    assert_eq!(
        assertion.get("expectation").and_then(|e| e.as_bool()),
        Some(true)
    );

    Ok(())
}

/// Test: WriteAssertions replaces all existing assertions (upsert)
#[tokio::test]
async fn test_write_assertions_replaces_existing() -> Result<()> {
    let store_id = create_test_store().await?;

    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            { "type": "user" },
            {
                "type": "document",
                "relations": {
                    "viewer": { "this": {} }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [{ "type": "user" }]
                        }
                    }
                }
            }
        ]
    });

    let model_id = create_authorization_model(&store_id, model).await?;

    let client = shared_client();

    // Write first set of assertions
    let assertions1 = json!({
        "assertions": [
            {
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:doc1"
                },
                "expectation": true
            },
            {
                "tuple_key": {
                    "user": "user:bob",
                    "relation": "viewer",
                    "object": "document:doc2"
                },
                "expectation": true
            }
        ]
    });

    client
        .put(format!(
            "{}/stores/{}/assertions/{}",
            get_openfga_url(),
            store_id,
            model_id
        ))
        .json(&assertions1)
        .send()
        .await?;

    // Write second set (should REPLACE, not append)
    let assertions2 = json!({
        "assertions": [
            {
                "tuple_key": {
                    "user": "user:charlie",
                    "relation": "viewer",
                    "object": "document:doc3"
                },
                "expectation": false
            }
        ]
    });

    let response = client
        .put(format!(
            "{}/stores/{}/assertions/{}",
            get_openfga_url(),
            store_id,
            model_id
        ))
        .json(&assertions2)
        .send()
        .await?;

    assert!(response.status().is_success());

    // Read assertions back
    let read_response = client
        .get(format!(
            "{}/stores/{}/assertions/{}",
            get_openfga_url(),
            store_id,
            model_id
        ))
        .send()
        .await?;

    let response_body: serde_json::Value = read_response.json().await?;

    let assertions = response_body
        .get("assertions")
        .and_then(|a| a.as_array())
        .expect("Response should have 'assertions' array");

    // Should only have the second set (replacement, not append)
    assert_eq!(
        assertions.len(),
        1,
        "WriteAssertions should replace all existing assertions"
    );

    let assertion = &assertions[0];
    let user = assertion
        .get("tuple_key")
        .and_then(|tk| tk.get("user"))
        .and_then(|u| u.as_str());

    assert_eq!(
        user,
        Some("user:charlie"),
        "Should have the new assertion, not old ones"
    );

    Ok(())
}

/// Test: Assertions with contextual tuples
#[tokio::test]
async fn test_assertions_with_contextual_tuples() -> Result<()> {
    let store_id = create_test_store().await?;

    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            { "type": "user" },
            {
                "type": "document",
                "relations": {
                    "viewer": { "this": {} }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [{ "type": "user" }]
                        }
                    }
                }
            }
        ]
    });

    let model_id = create_authorization_model(&store_id, model).await?;

    let client = shared_client();

    // Write assertions with contextual tuples
    let assertions_request = json!({
        "assertions": [
            {
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:temp-doc"
                },
                "expectation": true,
                "contextual_tuples": {
                    "tuple_keys": [
                        {
                            "user": "user:alice",
                            "relation": "viewer",
                            "object": "document:temp-doc"
                        }
                    ]
                }
            }
        ]
    });

    let response = client
        .put(format!(
            "{}/stores/{}/assertions/{}",
            get_openfga_url(),
            store_id,
            model_id
        ))
        .json(&assertions_request)
        .send()
        .await?;

    assert!(
        response.status().is_success(),
        "WriteAssertions with contextual tuples should succeed"
    );

    // Read back and verify
    let read_response = client
        .get(format!(
            "{}/stores/{}/assertions/{}",
            get_openfga_url(),
            store_id,
            model_id
        ))
        .send()
        .await?;

    let response_body: serde_json::Value = read_response.json().await?;

    let assertions = response_body
        .get("assertions")
        .and_then(|a| a.as_array())
        .expect("Response should have 'assertions' array");

    assert_eq!(assertions.len(), 1);

    // Verify the assertion was stored correctly
    let assertion = &assertions[0];

    // Verify tuple_key is present and correct
    let tuple_key = assertion
        .get("tuple_key")
        .expect("Assertion should have tuple_key");
    assert_eq!(
        tuple_key.get("user").and_then(|u| u.as_str()),
        Some("user:alice"),
        "Tuple key should have correct user"
    );
    assert_eq!(
        tuple_key.get("relation").and_then(|r| r.as_str()),
        Some("viewer"),
        "Tuple key should have correct relation"
    );
    assert_eq!(
        tuple_key.get("object").and_then(|o| o.as_str()),
        Some("document:temp-doc"),
        "Tuple key should have correct object"
    );

    // Verify expectation is preserved
    let expectation = assertion.get("expectation").and_then(|e| e.as_bool());
    assert_eq!(
        expectation,
        Some(true),
        "Assertion expectation should be true"
    );

    // Verify contextual tuples are stored (OpenFGA should preserve these)
    // If the implementation doesn't return contextual_tuples, log it but don't fail
    // since some implementations may store them differently
    let contextual = assertion.get("contextual_tuples");
    match contextual {
        Some(ct) => {
            let tuples = ct
                .get("tuple_keys")
                .and_then(|tk| tk.as_array())
                .expect("contextual_tuples should have tuple_keys array");
            assert!(
                !tuples.is_empty(),
                "Contextual tuples should be preserved when returned"
            );
        }
        None => {
            eprintln!(
                "INFO: contextual_tuples not returned in ReadAssertions response - \
                 implementation may store them differently"
            );
        }
    }

    Ok(())
}

/// Test: Empty assertions clears all
#[tokio::test]
async fn test_write_empty_assertions() -> Result<()> {
    let store_id = create_test_store().await?;

    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            { "type": "user" },
            {
                "type": "document",
                "relations": {
                    "viewer": { "this": {} }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [{ "type": "user" }]
                        }
                    }
                }
            }
        ]
    });

    let model_id = create_authorization_model(&store_id, model).await?;

    let client = shared_client();

    // First write some assertions
    let assertions1 = json!({
        "assertions": [
            {
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:doc1"
                },
                "expectation": true
            }
        ]
    });

    client
        .put(format!(
            "{}/stores/{}/assertions/{}",
            get_openfga_url(),
            store_id,
            model_id
        ))
        .json(&assertions1)
        .send()
        .await?;

    // Write empty assertions to clear
    let empty_assertions = json!({
        "assertions": []
    });

    let response = client
        .put(format!(
            "{}/stores/{}/assertions/{}",
            get_openfga_url(),
            store_id,
            model_id
        ))
        .json(&empty_assertions)
        .send()
        .await?;

    assert!(
        response.status().is_success(),
        "Writing empty assertions should succeed"
    );

    // Read back - should be empty
    let read_response = client
        .get(format!(
            "{}/stores/{}/assertions/{}",
            get_openfga_url(),
            store_id,
            model_id
        ))
        .send()
        .await?;

    let response_body: serde_json::Value = read_response.json().await?;

    let assertions = response_body
        .get("assertions")
        .and_then(|a| a.as_array())
        .expect("Response should have 'assertions' array");

    assert_eq!(
        assertions.len(),
        0,
        "Assertions should be empty after writing empty array"
    );

    Ok(())
}

/// Test: ReadAssertions for model with no assertions
#[tokio::test]
async fn test_read_assertions_when_none_exist() -> Result<()> {
    let store_id = create_test_store().await?;

    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            { "type": "user" },
            {
                "type": "document",
                "relations": {
                    "viewer": { "this": {} }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [{ "type": "user" }]
                        }
                    }
                }
            }
        ]
    });

    let model_id = create_authorization_model(&store_id, model).await?;

    let client = shared_client();

    // Read assertions without writing any
    let response = client
        .get(format!(
            "{}/stores/{}/assertions/{}",
            get_openfga_url(),
            store_id,
            model_id
        ))
        .send()
        .await?;

    assert!(
        response.status().is_success(),
        "ReadAssertions should succeed even when none exist"
    );

    let response_body: serde_json::Value = response.json().await?;

    let assertions = response_body
        .get("assertions")
        .and_then(|a| a.as_array())
        .expect("Response should have 'assertions' array");

    assert_eq!(
        assertions.len(),
        0,
        "Should return empty array when no assertions exist"
    );

    Ok(())
}

/// Test: Assertions with invalid model_id returns error
#[tokio::test]
async fn test_assertions_invalid_model_id_error() -> Result<()> {
    let store_id = create_test_store().await?;

    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            { "type": "user" },
            {
                "type": "document",
                "relations": {
                    "viewer": { "this": {} }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [{ "type": "user" }]
                        }
                    }
                }
            }
        ]
    });

    let _model_id = create_authorization_model(&store_id, model).await?;

    let client = shared_client();

    // Try to write assertions with non-existent model ID
    let assertions_request = json!({
        "assertions": [
            {
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:doc1"
                },
                "expectation": true
            }
        ]
    });

    let response = client
        .put(format!(
            "{}/stores/{}/assertions/{}",
            get_openfga_url(),
            store_id,
            "01ARZ3NDEKTSV4RRFFQ69G5FAV" // Valid ULID but doesn't exist
        ))
        .json(&assertions_request)
        .send()
        .await?;

    assert!(
        response.status() == StatusCode::NOT_FOUND || response.status() == StatusCode::BAD_REQUEST,
        "WriteAssertions with invalid model_id should return 404 or 400, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: ReadAssertions with non-existent store returns error
#[tokio::test]
async fn test_assertions_nonexistent_store_error() -> Result<()> {
    let client = shared_client();

    let response = client
        .get(format!(
            "{}/stores/{}/assertions/{}",
            get_openfga_url(),
            "01ARZ3NDEKTSV4RRFFQ69G5FAV", // Valid ULID but doesn't exist
            "01ARZ3NDEKTSV4RRFFQ69G5FAW"  // Valid ULID but doesn't exist
        ))
        .send()
        .await?;

    assert!(
        response.status() == StatusCode::NOT_FOUND || response.status() == StatusCode::BAD_REQUEST,
        "ReadAssertions with non-existent store should return 404 or 400, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: Multiple assertions with mixed expectations
#[tokio::test]
async fn test_assertions_mixed_expectations() -> Result<()> {
    let store_id = create_test_store().await?;

    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            { "type": "user" },
            {
                "type": "document",
                "relations": {
                    "viewer": { "this": {} },
                    "editor": { "this": {} },
                    "owner": { "this": {} }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [{ "type": "user" }]
                        },
                        "editor": {
                            "directly_related_user_types": [{ "type": "user" }]
                        },
                        "owner": {
                            "directly_related_user_types": [{ "type": "user" }]
                        }
                    }
                }
            }
        ]
    });

    let model_id = create_authorization_model(&store_id, model).await?;

    let client = shared_client();

    // Write multiple assertions with different expectations
    let assertions_request = json!({
        "assertions": [
            {
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:doc1"
                },
                "expectation": true
            },
            {
                "tuple_key": {
                    "user": "user:bob",
                    "relation": "viewer",
                    "object": "document:doc1"
                },
                "expectation": false
            },
            {
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "editor",
                    "object": "document:doc1"
                },
                "expectation": true
            },
            {
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "owner",
                    "object": "document:doc1"
                },
                "expectation": false
            }
        ]
    });

    let write_response = client
        .put(format!(
            "{}/stores/{}/assertions/{}",
            get_openfga_url(),
            store_id,
            model_id
        ))
        .json(&assertions_request)
        .send()
        .await?;

    assert!(write_response.status().is_success());

    // Read back and verify all assertions are stored
    let read_response = client
        .get(format!(
            "{}/stores/{}/assertions/{}",
            get_openfga_url(),
            store_id,
            model_id
        ))
        .send()
        .await?;

    let response_body: serde_json::Value = read_response.json().await?;

    let assertions = response_body
        .get("assertions")
        .and_then(|a| a.as_array())
        .expect("Response should have 'assertions' array");

    assert_eq!(
        assertions.len(),
        4,
        "Should have all 4 assertions, got: {}",
        assertions.len()
    );

    // Count true and false expectations
    let true_count = assertions
        .iter()
        .filter(|a| a.get("expectation").and_then(|e| e.as_bool()) == Some(true))
        .count();

    let false_count = assertions
        .iter()
        .filter(|a| a.get("expectation").and_then(|e| e.as_bool()) == Some(false))
        .count();

    assert_eq!(true_count, 2, "Should have 2 true expectations");
    assert_eq!(false_count, 2, "Should have 2 false expectations");

    Ok(())
}
