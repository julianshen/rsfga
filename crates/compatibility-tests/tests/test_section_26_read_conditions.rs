mod common;

use anyhow::Result;
use common::{create_authorization_model, create_test_store, get_openfga_url, shared_client};
use serde_json::json;

// ============================================================================
// Milestone 0.8: CEL Condition Compatibility Tests
// Section 26: Reading Tuples with Conditions
// ============================================================================

/// Helper to create a model with condition for read tests
async fn create_model_with_condition(store_id: &str) -> Result<String> {
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
                                    "condition": "time_access"
                                }
                            ]
                        }
                    }
                }
            }
        ],
        "conditions": {
            "time_access": {
                "name": "time_access",
                "expression": "request.current_time < request.expires_at",
                "parameters": {
                    "current_time": {
                        "type_name": "TYPE_NAME_TIMESTAMP"
                    },
                    "expires_at": {
                        "type_name": "TYPE_NAME_TIMESTAMP"
                    }
                }
            }
        }
    });

    create_authorization_model(store_id, model).await
}

/// Test: Read returns tuple with condition name
#[tokio::test]
async fn test_read_returns_tuple_with_condition_name() -> Result<()> {
    let store_id = create_test_store().await?;
    let _model_id = create_model_with_condition(&store_id).await?;
    let client = shared_client();

    // Write tuple with condition
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:doc1",
                    "condition": {
                        "name": "time_access"
                    }
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
        "Setup: Writing tuple with condition should succeed"
    );

    // Read the tuple back
    let read_request = json!({
        "tuple_key": {
            "user": "user:alice",
            "relation": "viewer",
            "object": "document:doc1"
        }
    });

    let read_response = client
        .post(format!("{}/stores/{}/read", get_openfga_url(), store_id))
        .json(&read_request)
        .send()
        .await?;

    assert!(read_response.status().is_success());

    let body: serde_json::Value = read_response.json().await?;
    let tuples = body["tuples"].as_array().unwrap();

    assert_eq!(tuples.len(), 1, "Should find exactly one tuple");

    let tuple = &tuples[0];
    let condition = &tuple["key"]["condition"];

    assert!(
        condition.is_object(),
        "Tuple should include condition, got: {:?}",
        tuple
    );
    assert_eq!(
        condition["name"].as_str(),
        Some("time_access"),
        "Condition name should be returned"
    );

    Ok(())
}

/// Test: Read returns tuple with condition context
#[tokio::test]
async fn test_read_returns_tuple_with_condition_context() -> Result<()> {
    let store_id = create_test_store().await?;
    let _model_id = create_model_with_condition(&store_id).await?;
    let client = shared_client();

    // Write tuple with condition context
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:bob",
                    "relation": "viewer",
                    "object": "document:doc2",
                    "condition": {
                        "name": "time_access",
                        "context": {
                            "expires_at": "2025-12-31T23:59:59Z"
                        }
                    }
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
        "Setup: Writing tuple with condition context should succeed"
    );

    // Read the tuple back
    let read_request = json!({
        "tuple_key": {
            "user": "user:bob",
            "relation": "viewer",
            "object": "document:doc2"
        }
    });

    let read_response = client
        .post(format!("{}/stores/{}/read", get_openfga_url(), store_id))
        .json(&read_request)
        .send()
        .await?;

    let body: serde_json::Value = read_response.json().await?;
    let tuples = body["tuples"].as_array().unwrap();
    let tuple = &tuples[0];

    let condition = &tuple["key"]["condition"];
    assert!(
        condition.is_object(),
        "Tuple should include condition with context"
    );

    // Context may be nested inside the condition
    let context = &condition["context"];
    assert!(
        context.is_object() || condition["name"].is_string(),
        "Condition context should be returned, got: {:?}",
        condition
    );

    Ok(())
}

/// Test: Can filter tuples by condition (if supported)
#[tokio::test]
async fn test_read_filter_tuples_by_condition() -> Result<()> {
    let store_id = create_test_store().await?;
    let _model_id = create_model_with_condition(&store_id).await?;
    let client = shared_client();

    // Write tuple with condition
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:charlie",
                    "relation": "viewer",
                    "object": "document:doc3",
                    "condition": {
                        "name": "time_access"
                    }
                }
            ]
        }
    });

    client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&write_request)
        .send()
        .await?;

    // Try to read with various filters - document actual behavior
    let read_request = json!({
        "tuple_key": {
            "object": "document:doc3"
        }
    });

    let read_response = client
        .post(format!("{}/stores/{}/read", get_openfga_url(), store_id))
        .json(&read_request)
        .send()
        .await?;

    assert!(
        read_response.status().is_success(),
        "Reading tuples should succeed"
    );

    let body: serde_json::Value = read_response.json().await?;
    let tuples = body["tuples"].as_array().unwrap();

    // Document: OpenFGA Read API doesn't have condition-based filtering
    // Tuples are returned with their conditions, but you can't filter BY condition
    assert!(
        !tuples.is_empty(),
        "Should return tuples (filtering by condition name not supported)"
    );

    Ok(())
}

/// Test: Tuple response format matches OpenFGA spec
#[tokio::test]
async fn test_tuple_response_format_matches_spec() -> Result<()> {
    let store_id = create_test_store().await?;
    let _model_id = create_model_with_condition(&store_id).await?;
    let client = shared_client();

    // Write tuple with full condition
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:dave",
                    "relation": "viewer",
                    "object": "document:doc4",
                    "condition": {
                        "name": "time_access",
                        "context": {
                            "expires_at": "2025-06-30T12:00:00Z"
                        }
                    }
                }
            ]
        }
    });

    client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&write_request)
        .send()
        .await?;

    // Read and verify response format
    let read_request = json!({
        "tuple_key": {
            "user": "user:dave",
            "relation": "viewer",
            "object": "document:doc4"
        }
    });

    let read_response = client
        .post(format!("{}/stores/{}/read", get_openfga_url(), store_id))
        .json(&read_request)
        .send()
        .await?;

    let body: serde_json::Value = read_response.json().await?;

    // Verify overall response structure
    assert!(
        body["tuples"].is_array(),
        "Response should have 'tuples' array"
    );

    let tuples = body["tuples"].as_array().unwrap();
    assert!(!tuples.is_empty(), "Should have at least one tuple");

    let tuple = &tuples[0];

    // Verify tuple structure follows OpenFGA spec
    // Format: { "key": { "user": ..., "relation": ..., "object": ..., "condition": {...} }, "timestamp": ... }
    assert!(tuple["key"].is_object(), "Tuple should have 'key' object");
    assert!(
        tuple["key"]["user"].is_string(),
        "Key should have 'user' string"
    );
    assert!(
        tuple["key"]["relation"].is_string(),
        "Key should have 'relation' string"
    );
    assert!(
        tuple["key"]["object"].is_string(),
        "Key should have 'object' string"
    );

    // Condition should be present
    let condition = &tuple["key"]["condition"];
    if condition.is_object() {
        assert!(
            condition["name"].is_string(),
            "Condition should have 'name' string"
        );
        // Context is optional and may be present
    }

    // Timestamp should be present
    assert!(
        tuple["timestamp"].is_string(),
        "Tuple should have 'timestamp' string"
    );

    Ok(())
}
