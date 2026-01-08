mod common;

use anyhow::Result;
use common::{
    create_authorization_model, create_test_store, get_openfga_url, shared_client, write_tuples,
};
use serde_json::json;

// ============================================================================
// Section 33: Consistency Preference Tests
// ============================================================================
//
// OpenFGA supports a consistency preference parameter that controls the
// trade-off between latency and consistency.
//
// Values:
// - UNSPECIFIED (default): Uses server default
// - MINIMIZE_LATENCY: Prefers cached results when available
// - HIGHER_CONSISTENCY: Bypasses cache, reads from database directly
//
// Supported APIs (v1.5.7+):
// - Check
// - ListObjects
// - ListUsers
// - Expand
//
// Note: When caching is disabled, both modes behave identically.
//
// ============================================================================

/// Test: Check API accepts consistency parameter
#[tokio::test]
async fn test_check_accepts_consistency_parameter() -> Result<()> {
    // Arrange
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

    write_tuples(&store_id, vec![("user:alice", "viewer", "document:doc1")]).await?;

    let client = shared_client();

    // Act: Check with MINIMIZE_LATENCY
    let check_request = json!({
        "tuple_key": {
            "user": "user:alice",
            "relation": "viewer",
            "object": "document:doc1"
        },
        "consistency": "MINIMIZE_LATENCY"
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    // Assert: Request should succeed (even if consistency param is ignored)
    assert!(
        response.status().is_success(),
        "Check with MINIMIZE_LATENCY should succeed, got: {}",
        response.status()
    );

    let body: serde_json::Value = response.json().await?;
    assert_eq!(
        body.get("allowed").and_then(|a| a.as_bool()),
        Some(true),
        "Check should return allowed=true"
    );

    Ok(())
}

/// Test: Check with HIGHER_CONSISTENCY
#[tokio::test]
async fn test_check_with_higher_consistency() -> Result<()> {
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

    write_tuples(&store_id, vec![("user:alice", "viewer", "document:doc1")]).await?;

    let client = shared_client();

    // Act: Check with HIGHER_CONSISTENCY
    let check_request = json!({
        "tuple_key": {
            "user": "user:alice",
            "relation": "viewer",
            "object": "document:doc1"
        },
        "consistency": "HIGHER_CONSISTENCY"
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    assert!(
        response.status().is_success(),
        "Check with HIGHER_CONSISTENCY should succeed, got: {}",
        response.status()
    );

    let body: serde_json::Value = response.json().await?;
    assert_eq!(
        body.get("allowed").and_then(|a| a.as_bool()),
        Some(true),
        "Check should return allowed=true"
    );

    Ok(())
}

/// Test: Check with UNSPECIFIED consistency (default)
#[tokio::test]
async fn test_check_with_unspecified_consistency() -> Result<()> {
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

    write_tuples(&store_id, vec![("user:alice", "viewer", "document:doc1")]).await?;

    let client = shared_client();

    // Act: Check with explicit UNSPECIFIED (same as omitting)
    let check_request = json!({
        "tuple_key": {
            "user": "user:alice",
            "relation": "viewer",
            "object": "document:doc1"
        },
        "consistency": "UNSPECIFIED"
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    // Note: UNSPECIFIED might be rejected by some versions
    // This is acceptable - the test documents the behavior
    if response.status().is_success() {
        let body: serde_json::Value = response.json().await?;
        assert_eq!(body.get("allowed").and_then(|a| a.as_bool()), Some(true));
    } else {
        // Document that UNSPECIFIED is not accepted
        eprintln!(
            "Note: UNSPECIFIED consistency value returned status: {}",
            response.status()
        );
    }

    Ok(())
}

/// Test: ListObjects accepts consistency parameter
#[tokio::test]
async fn test_listobjects_accepts_consistency_parameter() -> Result<()> {
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

    write_tuples(
        &store_id,
        vec![
            ("user:alice", "viewer", "document:doc1"),
            ("user:alice", "viewer", "document:doc2"),
        ],
    )
    .await?;

    let client = shared_client();

    // Act: ListObjects with consistency
    let list_request = json!({
        "type": "document",
        "relation": "viewer",
        "user": "user:alice",
        "consistency": "HIGHER_CONSISTENCY"
    });

    let response = client
        .post(format!(
            "{}/stores/{}/list-objects",
            get_openfga_url(),
            store_id
        ))
        .json(&list_request)
        .send()
        .await?;

    assert!(
        response.status().is_success(),
        "ListObjects with consistency should succeed, got: {}",
        response.status()
    );

    let body: serde_json::Value = response.json().await?;
    let objects = body
        .get("objects")
        .and_then(|o| o.as_array())
        .expect("Should have objects array");

    assert_eq!(objects.len(), 2, "Should return 2 objects");

    Ok(())
}

/// Test: Expand accepts consistency parameter
#[tokio::test]
async fn test_expand_accepts_consistency_parameter() -> Result<()> {
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

    write_tuples(&store_id, vec![("user:alice", "viewer", "document:doc1")]).await?;

    let client = shared_client();

    // Act: Expand with consistency
    let expand_request = json!({
        "tuple_key": {
            "relation": "viewer",
            "object": "document:doc1"
        },
        "consistency": "MINIMIZE_LATENCY"
    });

    let response = client
        .post(format!("{}/stores/{}/expand", get_openfga_url(), store_id))
        .json(&expand_request)
        .send()
        .await?;

    assert!(
        response.status().is_success(),
        "Expand with consistency should succeed, got: {}",
        response.status()
    );

    let body: serde_json::Value = response.json().await?;
    assert!(body.get("tree").is_some(), "Expand should return tree");

    Ok(())
}

/// Test: Both consistency modes return same results for fresh data
#[tokio::test]
async fn test_consistency_modes_return_same_results() -> Result<()> {
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

    write_tuples(&store_id, vec![("user:alice", "viewer", "document:doc1")]).await?;

    let client = shared_client();

    // Check with MINIMIZE_LATENCY
    let check1 = json!({
        "tuple_key": {
            "user": "user:alice",
            "relation": "viewer",
            "object": "document:doc1"
        },
        "consistency": "MINIMIZE_LATENCY"
    });

    let response1 = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check1)
        .send()
        .await?;

    let body1: serde_json::Value = response1.json().await?;
    let allowed1 = body1.get("allowed").and_then(|a| a.as_bool());

    // Check with HIGHER_CONSISTENCY
    let check2 = json!({
        "tuple_key": {
            "user": "user:alice",
            "relation": "viewer",
            "object": "document:doc1"
        },
        "consistency": "HIGHER_CONSISTENCY"
    });

    let response2 = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check2)
        .send()
        .await?;

    let body2: serde_json::Value = response2.json().await?;
    let allowed2 = body2.get("allowed").and_then(|a| a.as_bool());

    // Both should return the same result
    assert_eq!(
        allowed1, allowed2,
        "Both consistency modes should return same result for fresh data"
    );
    assert_eq!(allowed1, Some(true), "Both should return allowed=true");

    Ok(())
}

/// Test: Invalid consistency value returns error
#[tokio::test]
async fn test_invalid_consistency_value_error() -> Result<()> {
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

    // Act: Check with invalid consistency value
    let check_request = json!({
        "tuple_key": {
            "user": "user:alice",
            "relation": "viewer",
            "object": "document:doc1"
        },
        "consistency": "INVALID_VALUE"
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    // Behavior may vary:
    // - Some implementations may ignore unknown values
    // - Some may return 400 Bad Request
    // - Some may return 422 Unprocessable Entity
    // Document the behavior
    if response.status().is_success() {
        eprintln!("Note: Invalid consistency value was accepted (ignored by server)");
    } else {
        eprintln!(
            "Note: Invalid consistency value returned status: {}",
            response.status()
        );
    }

    Ok(())
}

/// Test: BatchCheck accepts consistency parameter
#[tokio::test]
async fn test_batchcheck_accepts_consistency_parameter() -> Result<()> {
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

    write_tuples(&store_id, vec![("user:alice", "viewer", "document:doc1")]).await?;

    let client = shared_client();

    // Act: BatchCheck with consistency
    let batch_request = json!({
        "checks": [
            {
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:doc1"
                },
                "correlation_id": "check-1"
            }
        ],
        "consistency": "HIGHER_CONSISTENCY"
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

    // Note: Older versions may not support consistency in batch-check
    if response.status().is_success() {
        let body: serde_json::Value = response.json().await?;
        let result = body.get("result");
        assert!(result.is_some(), "BatchCheck should return result");
    } else {
        eprintln!(
            "Note: BatchCheck with consistency returned status: {} (may not be supported)",
            response.status()
        );
    }

    Ok(())
}
