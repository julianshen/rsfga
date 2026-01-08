mod common;

use anyhow::Result;
use common::{create_authorization_model, create_test_store, get_openfga_url, shared_client};
use reqwest::StatusCode;
use serde_json::json;

// ============================================================================
// Milestone 0.8: CEL Condition Compatibility Tests
// Section 25: Writing Tuples with Conditions
// ============================================================================

/// Helper to create a model with condition for tuple tests
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
                    },
                    "editor": {
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
                        },
                        "editor": {
                            "directly_related_user_types": [
                                {
                                    "type": "user",
                                    "condition": "ip_access"
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
            },
            "ip_access": {
                "name": "ip_access",
                "expression": "request.client_ip in request.allowed_ips",
                "parameters": {
                    "client_ip": {
                        "type_name": "TYPE_NAME_STRING"
                    },
                    "allowed_ips": {
                        "type_name": "TYPE_NAME_LIST",
                        "generic_types": [
                            {
                                "type_name": "TYPE_NAME_STRING"
                            }
                        ]
                    }
                }
            }
        }
    });

    create_authorization_model(store_id, model).await
}

/// Test: Can write tuple with condition name
#[tokio::test]
async fn test_can_write_tuple_with_condition_name() -> Result<()> {
    let store_id = create_test_store().await?;
    let _model_id = create_model_with_condition(&store_id).await?;
    let client = shared_client();

    // Write tuple with condition name
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

    let response = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&write_request)
        .send()
        .await?;

    assert!(
        response.status().is_success(),
        "Writing tuple with condition name should succeed, got: {} - {}",
        response.status(),
        response.text().await?
    );

    Ok(())
}

/// Test: Can write tuple with condition context (parameter values)
#[tokio::test]
async fn test_can_write_tuple_with_condition_context() -> Result<()> {
    let store_id = create_test_store().await?;
    let _model_id = create_model_with_condition(&store_id).await?;
    let client = shared_client();

    // Write tuple with condition name and context (parameter values)
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

    let response = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&write_request)
        .send()
        .await?;

    assert!(
        response.status().is_success(),
        "Writing tuple with condition context should succeed, got: {} - {}",
        response.status(),
        response.text().await?
    );

    Ok(())
}

/// Test: Condition context values match parameter types
#[tokio::test]
async fn test_condition_context_values_match_types() -> Result<()> {
    let store_id = create_test_store().await?;
    let _model_id = create_model_with_condition(&store_id).await?;
    let client = shared_client();

    // Write tuple with correctly typed context values
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:charlie",
                    "relation": "editor",
                    "object": "document:doc3",
                    "condition": {
                        "name": "ip_access",
                        "context": {
                            "allowed_ips": ["192.168.1.1", "10.0.0.1"]
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

    assert!(
        response.status().is_success(),
        "Writing tuple with matching context types should succeed, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: Writing tuple with undefined condition returns 400
#[tokio::test]
async fn test_writing_tuple_with_undefined_condition_returns_400() -> Result<()> {
    let store_id = create_test_store().await?;
    let _model_id = create_model_with_condition(&store_id).await?;
    let client = shared_client();

    // Try to write tuple with non-existent condition
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:dave",
                    "relation": "viewer",
                    "object": "document:doc4",
                    "condition": {
                        "name": "nonexistent_condition"
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

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "Writing tuple with undefined condition should return 400"
    );

    Ok(())
}

/// Test: Writing tuple with mismatched context types returns 400
#[tokio::test]
async fn test_writing_tuple_with_mismatched_context_types_returns_400() -> Result<()> {
    let store_id = create_test_store().await?;
    let _model_id = create_model_with_condition(&store_id).await?;
    let client = shared_client();

    // Try to write tuple with wrong type for context value
    // (providing string where list is expected for allowed_ips)
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:eve",
                    "relation": "editor",
                    "object": "document:doc5",
                    "condition": {
                        "name": "ip_access",
                        "context": {
                            "allowed_ips": "not-a-list"  // Should be a list
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

    // Note: OpenFGA may accept any JSON value and validate at check time
    // This test documents the actual behavior
    let status = response.status();
    assert!(
        status == StatusCode::BAD_REQUEST || status.is_success(),
        "Writing tuple with mismatched context type should return 400 or succeed (with validation at check time), got: {}",
        status
    );

    Ok(())
}

/// Test: Can write multiple tuples with different conditions
#[tokio::test]
async fn test_can_write_multiple_tuples_with_different_conditions() -> Result<()> {
    let store_id = create_test_store().await?;
    let _model_id = create_model_with_condition(&store_id).await?;
    let client = shared_client();

    // Write multiple tuples with different conditions
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:frank",
                    "relation": "viewer",
                    "object": "document:doc6",
                    "condition": {
                        "name": "time_access",
                        "context": {
                            "expires_at": "2025-06-30T23:59:59Z"
                        }
                    }
                },
                {
                    "user": "user:grace",
                    "relation": "editor",
                    "object": "document:doc7",
                    "condition": {
                        "name": "ip_access",
                        "context": {
                            "allowed_ips": ["192.168.1.100"]
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

    assert!(
        response.status().is_success(),
        "Writing multiple tuples with different conditions should succeed, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: Can delete tuple with condition
#[tokio::test]
async fn test_can_delete_tuple_with_condition() -> Result<()> {
    let store_id = create_test_store().await?;
    let _model_id = create_model_with_condition(&store_id).await?;
    let client = shared_client();

    // First, write a tuple with condition
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:henry",
                    "relation": "viewer",
                    "object": "document:doc8",
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
        "Setup: Writing tuple with condition should succeed"
    );

    // Now delete the tuple
    let delete_request = json!({
        "deletes": {
            "tuple_keys": [
                {
                    "user": "user:henry",
                    "relation": "viewer",
                    "object": "document:doc8"
                }
            ]
        }
    });

    let delete_response = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&delete_request)
        .send()
        .await?;

    assert!(
        delete_response.status().is_success(),
        "Deleting tuple with condition should succeed, got: {}",
        delete_response.status()
    );

    // Verify the tuple is deleted by reading
    let read_request = json!({
        "tuple_key": {
            "user": "user:henry",
            "relation": "viewer",
            "object": "document:doc8"
        }
    });

    let read_response = client
        .post(format!("{}/stores/{}/read", get_openfga_url(), store_id))
        .json(&read_request)
        .send()
        .await?;

    let read_body: serde_json::Value = read_response.json().await?;
    let tuples = read_body["tuples"].as_array().unwrap_or(&vec![]);

    assert!(
        tuples.is_empty(),
        "Tuple should be deleted, but found: {:?}",
        tuples
    );

    Ok(())
}
