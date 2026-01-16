mod common;

use anyhow::Result;
use common::{create_authorization_model, create_test_store, get_openfga_url, write_tuples};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

// ============================================================================
// Section 20: Limits & Boundaries Tests
// ============================================================================
//
// These tests document OpenFGA's built-in limits:
// - Batch check limit: 50 items
// - Tuple write limit: 100 tuples per request
// - Object identifier limit: 256 characters
// - Relation depth limit: 25 levels (server-wide configuration)
//
// ============================================================================

#[derive(Debug, Deserialize)]
struct ErrorResponse {
    code: String,
    message: String,
}

/// Test: Maximum tuple size (user + relation + object length)
#[tokio::test]
async fn test_maximum_tuple_size() -> Result<()> {
    // Arrange
    let store_id = create_test_store().await?;

    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            {"type": "user"},
            {
                "type": "document",
                "relations": {
                    "viewer": {"this": {}}
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [{"type": "user"}]
                        }
                    }
                }
            }
        ]
    });

    let _model_id = create_authorization_model(&store_id, model).await?;
    let client = reqwest::Client::new();

    // OpenFGA limits:
    // - Object: max 256 characters (validated by regex ^[^\s]{2,256}$)
    // - User: no strict limit in current version, but should be reasonable
    // - Relation: max 50 characters (validated by regex ^[^:#@\s]{1,50}$)

    // Test 1: Object at exactly 256 characters (should succeed)
    let max_object = format!("document:{}", "x".repeat(256 - 9)); // "document:" is 9 chars
    let response = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:test",
                    "relation": "viewer",
                    "object": &max_object
                }]
            }
        }))
        .send()
        .await?;

    assert!(
        response.status().is_success(),
        "Object with 256 chars should be accepted"
    );

    // Test 2: Object exceeding 256 characters (should fail)
    let oversized_object = format!("document:{}", "x".repeat(256 - 9 + 1)); // 257 total
    let response = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:test",
                    "relation": "viewer",
                    "object": &oversized_object
                }]
            }
        }))
        .send()
        .await?;

    assert_eq!(
        response.status().as_u16(),
        400,
        "Object exceeding 256 chars should be rejected"
    );

    let error: ErrorResponse = response.json().await?;
    assert_eq!(error.code, "validation_error");
    assert!(
        error.message.contains("Object") || error.message.contains("256"),
        "Error should mention object size limit: {}",
        error.message
    );

    Ok(())
}

/// Test: Maximum number of tuples in single write
#[tokio::test]
async fn test_maximum_tuples_in_single_write() -> Result<()> {
    // Arrange
    let store_id = create_test_store().await?;

    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            {"type": "user"},
            {
                "type": "document",
                "relations": {
                    "viewer": {"this": {}}
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [{"type": "user"}]
                        }
                    }
                }
            }
        ]
    });

    let _model_id = create_authorization_model(&store_id, model.clone()).await?;
    let client = reqwest::Client::new();

    // OpenFGA limit: 100 tuples per write request

    // Test 1: Exactly 100 tuples (should succeed)
    let tuples_100: Vec<serde_json::Value> = (0..100)
        .map(|i| {
            json!({
                "user": format!("user:{}", i),
                "relation": "viewer",
                "object": format!("document:max-{}", i)
            })
        })
        .collect();

    let response = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&json!({
            "writes": {
                "tuple_keys": tuples_100
            }
        }))
        .send()
        .await?;

    assert!(
        response.status().is_success(),
        "Writing 100 tuples should succeed"
    );

    // Test 2: 101 tuples (should fail)
    // Use a new store to avoid duplicate tuple errors
    let store_id2 = create_test_store().await?;
    let _model_id2 = create_authorization_model(&store_id2, model.clone()).await?;

    let tuples_101: Vec<serde_json::Value> = (0..101)
        .map(|i| {
            json!({
                "user": format!("user:{}", i),
                "relation": "viewer",
                "object": format!("document:over-{}", i)
            })
        })
        .collect();

    let response = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id2))
        .json(&json!({
            "writes": {
                "tuple_keys": tuples_101
            }
        }))
        .send()
        .await?;

    assert_eq!(
        response.status().as_u16(),
        400,
        "Writing 101 tuples should fail"
    );

    let error: ErrorResponse = response.json().await?;
    assert_eq!(
        error.code, "exceeded_entity_limit",
        "Should return exceeded_entity_limit error"
    );
    assert!(
        error.message.contains("100"),
        "Error should mention the 100 limit: {}",
        error.message
    );

    Ok(())
}

/// Test: Maximum number of checks in batch
#[tokio::test]
async fn test_maximum_checks_in_batch() -> Result<()> {
    // Arrange
    let store_id = create_test_store().await?;

    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            {"type": "user"},
            {
                "type": "document",
                "relations": {
                    "viewer": {"this": {}}
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [{"type": "user"}]
                        }
                    }
                }
            }
        ]
    });

    let _model_id = create_authorization_model(&store_id, model).await?;
    let client = reqwest::Client::new();

    // OpenFGA limit: 50 checks per batch-check request

    // Test 1: Exactly 50 checks (should succeed)
    let checks_50: Vec<serde_json::Value> = (0..50)
        .map(|i| {
            json!({
                "tuple_key": {
                    "user": format!("user:{}", i),
                    "relation": "viewer",
                    "object": format!("document:{}", i)
                },
                "correlation_id": format!("check-{}", i)
            })
        })
        .collect();

    let response = client
        .post(format!(
            "{}/stores/{}/batch-check",
            get_openfga_url(),
            store_id
        ))
        .json(&json!({
            "checks": checks_50
        }))
        .send()
        .await?;

    assert!(
        response.status().is_success(),
        "Batch check with 50 items should succeed"
    );

    let body: serde_json::Value = response.json().await?;
    let results = body.get("result").expect("Should have result field");
    assert_eq!(
        results.as_object().map(|m| m.len()),
        Some(50),
        "Should return 50 results"
    );

    // Test 2: 51 checks (should fail)
    let checks_51: Vec<serde_json::Value> = (0..51)
        .map(|i| {
            json!({
                "tuple_key": {
                    "user": format!("user:{}", i),
                    "relation": "viewer",
                    "object": format!("document:{}", i)
                },
                "correlation_id": format!("check-{}", i)
            })
        })
        .collect();

    let response = client
        .post(format!(
            "{}/stores/{}/batch-check",
            get_openfga_url(),
            store_id
        ))
        .json(&json!({
            "checks": checks_51
        }))
        .send()
        .await?;

    assert_eq!(
        response.status().as_u16(),
        400,
        "Batch check with 51 items should fail"
    );

    let error: ErrorResponse = response.json().await?;
    assert_eq!(error.code, "validation_error");
    assert!(
        error.message.contains("50") || error.message.contains("51"),
        "Error should mention the batch size limit: {}",
        error.message
    );

    Ok(())
}

/// Test: Maximum authorization model size
#[tokio::test]
async fn test_maximum_authorization_model_size() -> Result<()> {
    // Arrange
    let store_id = create_test_store().await?;
    let client = reqwest::Client::new();

    // OpenFGA requires at least 1 type definition and imposes limits on:
    // - Number of type definitions
    // - Number of relations per type
    // - Complexity of relation rewrites

    // Test 1: Model with many types (should work within reasonable limits)
    let type_defs: Vec<serde_json::Value> = (0..50)
        .map(|i| {
            json!({
                "type": format!("type{}", i),
                "relations": {
                    "member": {"this": {}}
                },
                "metadata": {
                    "relations": {
                        "member": {
                            "directly_related_user_types": [{"type": "user"}]
                        }
                    }
                }
            })
        })
        .collect();

    let mut all_types = vec![json!({"type": "user"})];
    all_types.extend(type_defs);

    let model = json!({
        "schema_version": "1.1",
        "type_definitions": all_types
    });

    let response = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model)
        .send()
        .await?;

    // This should succeed (50 types is reasonable)
    assert!(
        response.status().is_success(),
        "Model with 51 types should succeed (got {})",
        response.status()
    );

    // Test 2: Empty type definitions (should fail - minimum 1 required)
    let empty_model = json!({
        "schema_version": "1.1",
        "type_definitions": []
    });

    let response = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&empty_model)
        .send()
        .await?;

    assert_eq!(
        response.status().as_u16(),
        400,
        "Empty type definitions should fail"
    );

    let error: ErrorResponse = response.json().await?;
    assert_eq!(error.code, "type_definitions_too_few_items");

    Ok(())
}

/// Test: Maximum relation depth
#[tokio::test]
async fn test_maximum_relation_depth() -> Result<()> {
    // Arrange
    let store_id = create_test_store().await?;

    // OpenFGA has a server-wide OPENFGA_RESOLVE_NODE_LIMIT (default: 25 levels)
    // This test creates a hierarchy and verifies behavior at the limits

    // Create model with parent hierarchy that allows deep nesting
    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            {"type": "user"},
            {
                "type": "folder",
                "relations": {
                    "parent": {"this": {}},
                    "viewer": {
                        "union": {
                            "child": [
                                {"this": {}},
                                {
                                    "tupleToUserset": {
                                        "tupleset": {"relation": "parent"},
                                        "computedUserset": {"relation": "viewer"}
                                    }
                                }
                            ]
                        }
                    }
                },
                "metadata": {
                    "relations": {
                        "parent": {
                            "directly_related_user_types": [{"type": "folder"}]
                        },
                        "viewer": {
                            "directly_related_user_types": [{"type": "user"}]
                        }
                    }
                }
            }
        ]
    });

    let _model_id = create_authorization_model(&store_id, model).await?;

    // Create 24-level deep hierarchy (just under the 25 limit)
    let mut tuples = Vec::new();
    for i in 1..24 {
        tuples.push((
            format!("folder:level{}", i),
            "parent".to_string(),
            format!("folder:level{}", i + 1),
        ));
    }
    // Add viewer at the root
    tuples.push((
        "user:alice".to_string(),
        "viewer".to_string(),
        "folder:level1".to_string(),
    ));

    let tuple_refs: Vec<(&str, &str, &str)> = tuples
        .iter()
        .map(|(u, r, o)| (u.as_str(), r.as_str(), o.as_str()))
        .collect();

    write_tuples(&store_id, tuple_refs).await?;

    let client = reqwest::Client::new();

    // Act: Check at level 24 (should succeed - within limit)
    let check_response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "folder:level24"
            }
        }))
        .send()
        .await?;

    // Assert: Should succeed (24 hops is within 25 limit)
    assert!(
        check_response.status().is_success(),
        "Check at depth 24 should succeed (status: {})",
        check_response.status()
    );

    let body: serde_json::Value = check_response.json().await?;
    assert_eq!(
        body.get("allowed"),
        Some(&json!(true)),
        "Alice should be viewer at level 24 via hierarchy"
    );

    Ok(())
}

/// Test: Request timeout behavior
///
/// Note: OpenFGA uses configurable server-side timeouts.
/// This test verifies that long-running operations don't hang indefinitely.
#[tokio::test]
async fn test_request_timeout_behavior() -> Result<()> {
    // Arrange
    let store_id = create_test_store().await?;

    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            {"type": "user"},
            {
                "type": "document",
                "relations": {
                    "viewer": {"this": {}}
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [{"type": "user"}]
                        }
                    }
                }
            }
        ]
    });

    let _model_id = create_authorization_model(&store_id, model).await?;

    // Create client with explicit timeout
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;

    // Act: Make a request with our client timeout
    // This tests that normal operations complete well within timeout
    let start = std::time::Instant::now();

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&json!({
            "tuple_key": {
                "user": "user:timeout-test",
                "relation": "viewer",
                "object": "document:test"
            }
        }))
        .send()
        .await?;

    let duration = start.elapsed();

    // Assert: Request should complete quickly (well under timeout)
    assert!(
        response.status().is_success(),
        "Simple check should succeed"
    );
    assert!(
        duration.as_secs() < 5,
        "Simple check should complete in < 5s (took {:?})",
        duration
    );

    // Document: OpenFGA server-side timeouts are configured via:
    // - OPENFGA_HTTP_TIMEOUT: HTTP request timeout
    // - OPENFGA_RESOLVE_NODE_LIMIT: Graph traversal node limit
    // - OPENFGA_REQUEST_TIMEOUT: Overall request timeout
    //
    // When implementing RSFGA, ensure timeouts are configurable and
    // that timeout errors return appropriate 408 or 504 status codes.

    Ok(())
}
