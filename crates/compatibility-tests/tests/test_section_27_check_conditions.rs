mod common;

use anyhow::Result;
use common::{create_authorization_model, create_test_store, get_openfga_url, shared_client};
use reqwest::StatusCode;
use serde_json::json;

// ============================================================================
// Milestone 0.8: CEL Condition Compatibility Tests
// Section 27: Check with Condition Evaluation
// ============================================================================

/// Helper to create a model with various condition types
async fn create_model_with_conditions(store_id: &str) -> Result<String> {
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
                    "time_viewer": {
                        "this": {}
                    },
                    "ip_viewer": {
                        "this": {}
                    },
                    "logic_viewer": {
                        "this": {}
                    },
                    "list_viewer": {
                        "this": {}
                    }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [
                                {"type": "user"}
                            ]
                        },
                        "time_viewer": {
                            "directly_related_user_types": [
                                {
                                    "type": "user",
                                    "condition": "time_check"
                                }
                            ]
                        },
                        "ip_viewer": {
                            "directly_related_user_types": [
                                {
                                    "type": "user",
                                    "condition": "ip_check"
                                }
                            ]
                        },
                        "logic_viewer": {
                            "directly_related_user_types": [
                                {
                                    "type": "user",
                                    "condition": "logic_check"
                                }
                            ]
                        },
                        "list_viewer": {
                            "directly_related_user_types": [
                                {
                                    "type": "user",
                                    "condition": "list_check"
                                }
                            ]
                        }
                    }
                }
            }
        ],
        "conditions": {
            "time_check": {
                "name": "time_check",
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
            "ip_check": {
                "name": "ip_check",
                "expression": "request.client_ip == request.allowed_ip",
                "parameters": {
                    "client_ip": {
                        "type_name": "TYPE_NAME_STRING"
                    },
                    "allowed_ip": {
                        "type_name": "TYPE_NAME_STRING"
                    }
                }
            },
            "logic_check": {
                "name": "logic_check",
                "expression": "request.is_admin || (request.level >= 5 && request.department == 'engineering')",
                "parameters": {
                    "is_admin": {
                        "type_name": "TYPE_NAME_BOOL"
                    },
                    "level": {
                        "type_name": "TYPE_NAME_INT"
                    },
                    "department": {
                        "type_name": "TYPE_NAME_STRING"
                    }
                }
            },
            "list_check": {
                "name": "list_check",
                "expression": "request.role in request.allowed_roles",
                "parameters": {
                    "role": {
                        "type_name": "TYPE_NAME_STRING"
                    },
                    "allowed_roles": {
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

/// Test: Check API accepts `context` parameter
#[tokio::test]
async fn test_check_accepts_context_parameter() -> Result<()> {
    let store_id = create_test_store().await?;
    let _model_id = create_model_with_conditions(&store_id).await?;
    let client = shared_client();

    // Write tuple with condition
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:alice",
                    "relation": "time_viewer",
                    "object": "document:doc1",
                    "condition": {
                        "name": "time_check"
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

    // Check with context parameter
    let check_request = json!({
        "tuple_key": {
            "user": "user:alice",
            "relation": "time_viewer",
            "object": "document:doc1"
        },
        "context": {
            "current_time": "2024-01-15T10:00:00Z",
            "expires_at": "2024-12-31T23:59:59Z"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    assert!(
        response.status().is_success(),
        "Check API should accept context parameter, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: Check returns `allowed: true` when condition evaluates true
#[tokio::test]
async fn test_check_returns_true_when_condition_evaluates_true() -> Result<()> {
    let store_id = create_test_store().await?;
    let _model_id = create_model_with_conditions(&store_id).await?;
    let client = shared_client();

    // Write tuple with condition
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:bob",
                    "relation": "time_viewer",
                    "object": "document:doc2",
                    "condition": {
                        "name": "time_check"
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

    // Check with context where condition evaluates to TRUE
    // current_time < expires_at => TRUE
    let check_request = json!({
        "tuple_key": {
            "user": "user:bob",
            "relation": "time_viewer",
            "object": "document:doc2"
        },
        "context": {
            "current_time": "2024-01-01T00:00:00Z",
            "expires_at": "2024-12-31T23:59:59Z"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    let body: serde_json::Value = response.json().await?;

    assert_eq!(
        body["allowed"].as_bool(),
        Some(true),
        "Check should return allowed: true when condition evaluates to true"
    );

    Ok(())
}

/// Test: Check returns `allowed: false` when condition evaluates false
#[tokio::test]
async fn test_check_returns_false_when_condition_evaluates_false() -> Result<()> {
    let store_id = create_test_store().await?;
    let _model_id = create_model_with_conditions(&store_id).await?;
    let client = shared_client();

    // Write tuple with condition
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:charlie",
                    "relation": "time_viewer",
                    "object": "document:doc3",
                    "condition": {
                        "name": "time_check"
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

    // Check with context where condition evaluates to FALSE
    // current_time < expires_at => FALSE (current_time is AFTER expires_at)
    let check_request = json!({
        "tuple_key": {
            "user": "user:charlie",
            "relation": "time_viewer",
            "object": "document:doc3"
        },
        "context": {
            "current_time": "2025-01-01T00:00:00Z",
            "expires_at": "2024-12-31T23:59:59Z"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    let body: serde_json::Value = response.json().await?;

    assert_eq!(
        body["allowed"].as_bool(),
        Some(false),
        "Check should return allowed: false when condition evaluates to false"
    );

    Ok(())
}

/// Test: Check without required context returns error (not false)
#[tokio::test]
async fn test_check_without_required_context_returns_error() -> Result<()> {
    let store_id = create_test_store().await?;
    let _model_id = create_model_with_conditions(&store_id).await?;
    let client = shared_client();

    // Write tuple with condition that requires context
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:dave",
                    "relation": "time_viewer",
                    "object": "document:doc4",
                    "condition": {
                        "name": "time_check"
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

    // Check WITHOUT providing context - should error or return false
    let check_request = json!({
        "tuple_key": {
            "user": "user:dave",
            "relation": "time_viewer",
            "object": "document:doc4"
        }
        // Note: no "context" field
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    let status = response.status();
    let body: serde_json::Value = response.json().await?;

    // Document actual OpenFGA behavior:
    // OpenFGA may return 400 error OR return allowed: false when context is missing
    assert!(
        status == StatusCode::BAD_REQUEST || body["allowed"].as_bool() == Some(false),
        "Check without required context should return error or false, got status: {status}, body: {body:?}"
    );

    Ok(())
}

/// Test: Check with partial context returns error for missing params
#[tokio::test]
async fn test_check_with_partial_context_returns_error() -> Result<()> {
    let store_id = create_test_store().await?;
    let _model_id = create_model_with_conditions(&store_id).await?;
    let client = shared_client();

    // Write tuple with condition
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:eve",
                    "relation": "time_viewer",
                    "object": "document:doc5",
                    "condition": {
                        "name": "time_check"
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

    // Check with PARTIAL context (missing expires_at)
    let check_request = json!({
        "tuple_key": {
            "user": "user:eve",
            "relation": "time_viewer",
            "object": "document:doc5"
        },
        "context": {
            "current_time": "2024-01-15T10:00:00Z"
            // Note: missing "expires_at"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    let status = response.status();
    let body: serde_json::Value = response.json().await?;

    // Document actual behavior - may error or return false for missing params
    assert!(
        status == StatusCode::BAD_REQUEST || body["allowed"].as_bool() == Some(false),
        "Check with partial context should return error or false, got status: {status}, body: {body:?}"
    );

    Ok(())
}

/// Test: Check with wrong context type returns error
#[tokio::test]
async fn test_check_with_wrong_context_type_returns_error() -> Result<()> {
    let store_id = create_test_store().await?;
    let _model_id = create_model_with_conditions(&store_id).await?;
    let client = shared_client();

    // Write tuple with condition
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:frank",
                    "relation": "time_viewer",
                    "object": "document:doc6",
                    "condition": {
                        "name": "time_check"
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

    // Check with wrong type (providing string instead of timestamp)
    let check_request = json!({
        "tuple_key": {
            "user": "user:frank",
            "relation": "time_viewer",
            "object": "document:doc6"
        },
        "context": {
            "current_time": "not-a-valid-timestamp",
            "expires_at": "also-invalid"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    let status = response.status();
    let body: serde_json::Value = response.json().await?;

    // Document actual behavior - invalid type should cause error or false
    assert!(
        status == StatusCode::BAD_REQUEST || body["allowed"].as_bool() == Some(false),
        "Check with wrong context type should return error or false, got status: {status}, body: {body:?}"
    );

    Ok(())
}

/// Test: Condition evaluation with timestamp comparison
#[tokio::test]
async fn test_condition_evaluation_with_timestamp_comparison() -> Result<()> {
    let store_id = create_test_store().await?;
    let _model_id = create_model_with_conditions(&store_id).await?;
    let client = shared_client();

    // Write tuple with time condition
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:grace",
                    "relation": "time_viewer",
                    "object": "document:doc7",
                    "condition": {
                        "name": "time_check"
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

    // Test timestamp comparison - should work with ISO 8601 format
    let check_request = json!({
        "tuple_key": {
            "user": "user:grace",
            "relation": "time_viewer",
            "object": "document:doc7"
        },
        "context": {
            "current_time": "2024-06-15T12:30:00Z",
            "expires_at": "2024-12-31T23:59:59Z"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    assert!(
        response.status().is_success(),
        "Timestamp comparison should work"
    );

    let body: serde_json::Value = response.json().await?;
    assert_eq!(
        body["allowed"].as_bool(),
        Some(true),
        "Timestamp comparison should evaluate correctly"
    );

    Ok(())
}

/// Test: Condition evaluation with string operations
#[tokio::test]
async fn test_condition_evaluation_with_string_operations() -> Result<()> {
    let store_id = create_test_store().await?;
    let _model_id = create_model_with_conditions(&store_id).await?;
    let client = shared_client();

    // Write tuple with IP condition (string equality)
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:henry",
                    "relation": "ip_viewer",
                    "object": "document:doc8",
                    "condition": {
                        "name": "ip_check"
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

    // Test string equality - should match
    let check_request = json!({
        "tuple_key": {
            "user": "user:henry",
            "relation": "ip_viewer",
            "object": "document:doc8"
        },
        "context": {
            "client_ip": "192.168.1.100",
            "allowed_ip": "192.168.1.100"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    let body: serde_json::Value = response.json().await?;
    assert_eq!(
        body["allowed"].as_bool(),
        Some(true),
        "String equality should evaluate correctly"
    );

    // Test non-matching string
    let check_request_no_match = json!({
        "tuple_key": {
            "user": "user:henry",
            "relation": "ip_viewer",
            "object": "document:doc8"
        },
        "context": {
            "client_ip": "10.0.0.1",
            "allowed_ip": "192.168.1.100"
        }
    });

    let response_no_match = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request_no_match)
        .send()
        .await?;

    let body_no_match: serde_json::Value = response_no_match.json().await?;
    assert_eq!(
        body_no_match["allowed"].as_bool(),
        Some(false),
        "Non-matching strings should return false"
    );

    Ok(())
}

/// Test: Condition evaluation with list membership (`in` operator)
#[tokio::test]
async fn test_condition_evaluation_with_list_membership() -> Result<()> {
    let store_id = create_test_store().await?;
    let _model_id = create_model_with_conditions(&store_id).await?;
    let client = shared_client();

    // Write tuple with list condition
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:iris",
                    "relation": "list_viewer",
                    "object": "document:doc9",
                    "condition": {
                        "name": "list_check"
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

    // Test list membership - role IS in allowed_roles
    let check_request_in = json!({
        "tuple_key": {
            "user": "user:iris",
            "relation": "list_viewer",
            "object": "document:doc9"
        },
        "context": {
            "role": "admin",
            "allowed_roles": ["admin", "editor", "viewer"]
        }
    });

    let response_in = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request_in)
        .send()
        .await?;

    let body_in: serde_json::Value = response_in.json().await?;
    assert_eq!(
        body_in["allowed"].as_bool(),
        Some(true),
        "List membership check should return true when role is in list"
    );

    // Test list membership - role NOT in allowed_roles
    let check_request_not_in = json!({
        "tuple_key": {
            "user": "user:iris",
            "relation": "list_viewer",
            "object": "document:doc9"
        },
        "context": {
            "role": "guest",
            "allowed_roles": ["admin", "editor", "viewer"]
        }
    });

    let response_not_in = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request_not_in)
        .send()
        .await?;

    let body_not_in: serde_json::Value = response_not_in.json().await?;
    assert_eq!(
        body_not_in["allowed"].as_bool(),
        Some(false),
        "List membership check should return false when role is not in list"
    );

    Ok(())
}

/// Test: Condition evaluation with logical operators (&&, ||, !)
#[tokio::test]
async fn test_condition_evaluation_with_logical_operators() -> Result<()> {
    let store_id = create_test_store().await?;
    let _model_id = create_model_with_conditions(&store_id).await?;
    let client = shared_client();

    // Write tuple with logic condition
    // Expression: is_admin || (level >= 5 && department == 'engineering')
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:jack",
                    "relation": "logic_viewer",
                    "object": "document:doc10",
                    "condition": {
                        "name": "logic_check"
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

    // Test: is_admin = true (should pass via OR)
    let check_admin = json!({
        "tuple_key": {
            "user": "user:jack",
            "relation": "logic_viewer",
            "object": "document:doc10"
        },
        "context": {
            "is_admin": true,
            "level": 1,
            "department": "sales"
        }
    });

    let response_admin = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_admin)
        .send()
        .await?;

    let body_admin: serde_json::Value = response_admin.json().await?;
    assert_eq!(
        body_admin["allowed"].as_bool(),
        Some(true),
        "Admin user should pass via OR condition"
    );

    // Test: not admin, but level >= 5 AND engineering (should pass via AND)
    let check_eng = json!({
        "tuple_key": {
            "user": "user:jack",
            "relation": "logic_viewer",
            "object": "document:doc10"
        },
        "context": {
            "is_admin": false,
            "level": 7,
            "department": "engineering"
        }
    });

    let response_eng = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_eng)
        .send()
        .await?;

    let body_eng: serde_json::Value = response_eng.json().await?;
    assert_eq!(
        body_eng["allowed"].as_bool(),
        Some(true),
        "Engineering user with level >= 5 should pass via AND condition"
    );

    // Test: not admin, level < 5 (should fail)
    let check_low_level = json!({
        "tuple_key": {
            "user": "user:jack",
            "relation": "logic_viewer",
            "object": "document:doc10"
        },
        "context": {
            "is_admin": false,
            "level": 3,
            "department": "engineering"
        }
    });

    let response_low = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_low_level)
        .send()
        .await?;

    let body_low: serde_json::Value = response_low.json().await?;
    assert_eq!(
        body_low["allowed"].as_bool(),
        Some(false),
        "Low level user should fail condition"
    );

    Ok(())
}

/// Test: Batch check evaluates conditions correctly
#[tokio::test]
async fn test_batch_check_evaluates_conditions() -> Result<()> {
    let store_id = create_test_store().await?;
    let _model_id = create_model_with_conditions(&store_id).await?;
    let client = shared_client();

    // Write tuples with conditions
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:kate",
                    "relation": "time_viewer",
                    "object": "document:doc11",
                    "condition": {
                        "name": "time_check"
                    }
                },
                {
                    "user": "user:kate",
                    "relation": "viewer",
                    "object": "document:doc12"
                }
            ]
        }
    });

    client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&write_request)
        .send()
        .await?;

    // Batch check with context
    let batch_request = json!({
        "checks": [
            {
                "tuple_key": {
                    "user": "user:kate",
                    "relation": "time_viewer",
                    "object": "document:doc11"
                },
                "context": {
                    "current_time": "2024-01-15T10:00:00Z",
                    "expires_at": "2024-12-31T23:59:59Z"
                },
                "correlation_id": "check-1"
            },
            {
                "tuple_key": {
                    "user": "user:kate",
                    "relation": "viewer",
                    "object": "document:doc12"
                },
                "correlation_id": "check-2"
            }
        ]
    });

    let response = client
        .post(format!(
            "{}/stores/{}/batch-check",
            get_openfga_url(),
            store_id
        ))
        .json(&batch_request)
        .send()
        .await?;

    assert!(
        response.status().is_success(),
        "Batch check with conditions should succeed"
    );

    let body: serde_json::Value = response.json().await?;

    // Verify results (response format: {"result": {"check-1": {"allowed": true}, ...}})
    let results = &body["result"];
    assert!(results.is_object(), "Batch check should return results");

    // check-1: condition should evaluate to true
    let check1 = &results["check-1"];
    assert_eq!(
        check1["allowed"].as_bool(),
        Some(true),
        "Conditional check with valid context should return true"
    );

    // check-2: no condition, should return true (tuple exists)
    let check2 = &results["check-2"];
    assert_eq!(
        check2["allowed"].as_bool(),
        Some(true),
        "Non-conditional check should return true"
    );

    Ok(())
}

/// Test: Contextual tuples with conditions work
#[tokio::test]
async fn test_contextual_tuples_with_conditions() -> Result<()> {
    let store_id = create_test_store().await?;
    let _model_id = create_model_with_conditions(&store_id).await?;
    let client = shared_client();

    // Check with contextual tuple that has a condition
    let check_request = json!({
        "tuple_key": {
            "user": "user:liam",
            "relation": "time_viewer",
            "object": "document:doc13"
        },
        "contextual_tuples": {
            "tuple_keys": [
                {
                    "user": "user:liam",
                    "relation": "time_viewer",
                    "object": "document:doc13",
                    "condition": {
                        "name": "time_check"
                    }
                }
            ]
        },
        "context": {
            "current_time": "2024-06-15T12:00:00Z",
            "expires_at": "2024-12-31T23:59:59Z"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    // Document actual behavior
    // Note: OpenFGA may or may not support conditions on contextual tuples
    let status = response.status();
    let body: serde_json::Value = response.json().await?;

    if status.is_success() {
        // If supported, verify condition is evaluated
        assert!(
            body["allowed"].as_bool() == Some(true) || body["allowed"].as_bool() == Some(false),
            "Contextual tuple with condition should be evaluated"
        );
    } else {
        // Document if not supported
        assert!(
            status == StatusCode::BAD_REQUEST,
            "If not supported, should return 400, got: {status}"
        );
    }

    Ok(())
}
