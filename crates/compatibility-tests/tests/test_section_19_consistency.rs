mod common;

use anyhow::Result;
use common::{create_authorization_model, create_test_store, get_openfga_url, write_tuples};
use serde_json::json;
use std::sync::Arc;
use tokio::task::JoinSet;

// ============================================================================
// Section 19: Consistency & Concurrency Tests
// ============================================================================
//
// These tests verify OpenFGA's consistency guarantees:
// - Strong consistency for reads after writes
// - Behavior under concurrent modifications
// - Model version consistency
//
// ============================================================================

/// Test: Concurrent writes to same tuple
///
/// Verifies that concurrent writes to the same tuple are handled correctly
/// without data corruption or race conditions.
///
/// IMPORTANT DISCOVERY: OpenFGA uses transactional semantics for writes:
/// - 409 Conflict: Concurrent writes may return "Aborted" when transactions conflict
/// - 400 Bad Request: Writing a tuple that already exists returns "write_failed_due_to_invalid_input"
///
/// This means writes are NOT idempotent under concurrency - only one write wins,
/// but the final state is always consistent (tuple exists exactly once).
#[tokio::test]
async fn test_concurrent_writes_to_same_tuple() -> Result<()> {
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

    let client = Arc::new(reqwest::Client::new());
    let store_id = Arc::new(store_id);

    // Act: Make 10 concurrent write requests for the same tuple
    // OpenFGA uses transactional semantics, so conflicts are expected
    let mut join_set = JoinSet::new();

    for i in 0..10 {
        let client = Arc::clone(&client);
        let store_id = Arc::clone(&store_id);
        join_set.spawn(async move {
            let write_request = json!({
                "writes": {
                    "tuple_keys": [{
                        "user": "user:alice",
                        "relation": "viewer",
                        "object": "document:concurrent-test"
                    }]
                }
            });

            let response = client
                .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
                .json(&write_request)
                .send()
                .await;

            (i, response)
        });
    }

    let mut results = Vec::new();
    while let Some(result) = join_set.join_next().await {
        results.push(result);
    }

    // Analyze results - expect exactly one success and rest are conflicts/duplicates
    let mut success_count = 0;
    let mut conflict_count = 0; // 409 Conflict
    let mut duplicate_count = 0; // 400 - tuple already exists
    let mut other_error_count = 0;

    for result in results {
        match result {
            Ok((i, response)) => match response {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    match status {
                        200 => success_count += 1,
                        409 => {
                            // Expected: transactional conflict
                            conflict_count += 1;
                            let body = resp.text().await.unwrap_or_default();
                            eprintln!("Request {i} returned 409 Conflict: {body}");
                        }
                        400 => {
                            // Expected: tuple already exists - verify error code
                            let body = resp.text().await.unwrap_or_default();
                            // Parse the error response to verify it's actually a duplicate tuple error
                            if let Ok(error_json) = serde_json::from_str::<serde_json::Value>(&body)
                            {
                                let error_code = error_json.get("code").and_then(|c| c.as_str());
                                if error_code == Some("write_failed_due_to_invalid_input")
                                    || error_code
                                        == Some("cannot_allow_duplicate_tuples_in_one_request")
                                {
                                    duplicate_count += 1;
                                    eprintln!("Request {i} returned 400 (duplicate): {body}");
                                } else {
                                    // 400 for unexpected reason - count as other error
                                    other_error_count += 1;
                                    eprintln!(
                                        "Request {i} returned 400 with unexpected code {error_code:?}: {body}"
                                    );
                                }
                            } else {
                                // Couldn't parse error body - count as other error
                                other_error_count += 1;
                                eprintln!("Request {i} returned 400 (unparseable): {body}");
                            }
                        }
                        _ => {
                            other_error_count += 1;
                            let body = resp.text().await.unwrap_or_default();
                            eprintln!("Request {i} returned {status}: {body}");
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Request {i} failed: {e}");
                    other_error_count += 1;
                }
            },
            Err(e) => {
                eprintln!("Task join error: {e}");
                other_error_count += 1;
            }
        }
    }

    // Key assertion: At least one write should succeed
    assert!(
        success_count >= 1,
        "At least one concurrent write should succeed (got {success_count})"
    );

    // Expected errors are 409 Conflict or 400 duplicate tuple
    assert_eq!(
        other_error_count, 0,
        "Should only see 409/400 errors for concurrent writes"
    );

    // All 10 requests should be accounted for
    assert_eq!(
        success_count + conflict_count + duplicate_count + other_error_count,
        10,
        "All requests should be accounted for"
    );

    // Verify final state: tuple should exist exactly once
    let read_response = client
        .post(format!("{}/stores/{}/read", get_openfga_url(), store_id))
        .json(&json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:concurrent-test"
            }
        }))
        .send()
        .await?;

    assert!(read_response.status().is_success());

    let read_body: serde_json::Value = read_response.json().await?;
    let tuples = read_body
        .get("tuples")
        .and_then(|t| t.as_array())
        .expect("Should have tuples array");

    assert_eq!(tuples.len(), 1, "Tuple should exist exactly once");

    Ok(())
}

/// Test: Read-after-write consistency
///
/// Verifies that a read immediately after a write reflects the new data.
/// OpenFGA provides strong consistency.
#[tokio::test]
async fn test_read_after_write_consistency() -> Result<()> {
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

    // Perform 20 write-then-read cycles to verify consistency
    for i in 0..20 {
        let user = format!("user:test{i}");
        let object = format!("document:doc{i}");

        // Write tuple
        let write_request = json!({
            "writes": {
                "tuple_keys": [{
                    "user": &user,
                    "relation": "viewer",
                    "object": &object
                }]
            }
        });

        let write_response = client
            .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
            .json(&write_request)
            .send()
            .await?;

        assert!(
            write_response.status().is_success(),
            "Write {i} should succeed"
        );

        // Immediately read - should see the new tuple
        let read_request = json!({
            "tuple_key": {
                "user": &user,
                "relation": "viewer",
                "object": &object
            }
        });

        let read_response = client
            .post(format!("{}/stores/{}/read", get_openfga_url(), store_id))
            .json(&read_request)
            .send()
            .await?;

        assert!(read_response.status().is_success());

        let read_body: serde_json::Value = read_response.json().await?;
        let tuples = read_body
            .get("tuples")
            .and_then(|t| t.as_array())
            .expect("Should have tuples array");

        assert_eq!(
            tuples.len(),
            1,
            "Iteration {i}: Read immediately after write should return the tuple"
        );
    }

    Ok(())
}

/// Test: Check during model update
///
/// Verifies behavior when a check is performed while the authorization
/// model is being updated. OpenFGA should use the model specified in
/// the request or the latest model.
#[tokio::test]
async fn test_check_during_model_update() -> Result<()> {
    // Arrange
    let store_id = create_test_store().await?;

    // Create initial model with viewer relation
    let model_v1 = json!({
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

    let model_id_v1 = create_authorization_model(&store_id, model_v1).await?;

    // Write tuple using v1 model
    write_tuples(&store_id, vec![("user:alice", "viewer", "document:doc1")]).await?;

    let client = reqwest::Client::new();

    // Check with explicit model_id (v1) should succeed
    let check_v1 = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:doc1"
            },
            "authorization_model_id": &model_id_v1
        }))
        .send()
        .await?;

    assert!(check_v1.status().is_success());
    let check_v1_body: serde_json::Value = check_v1.json().await?;
    assert_eq!(
        check_v1_body.get("allowed"),
        Some(&json!(true)),
        "Check with v1 model should return allowed=true"
    );

    // Create v2 model (adds editor relation)
    let model_v2 = json!({
        "schema_version": "1.1",
        "type_definitions": [
            {"type": "user"},
            {
                "type": "document",
                "relations": {
                    "viewer": {"this": {}},
                    "editor": {"this": {}}
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [{"type": "user"}]
                        },
                        "editor": {
                            "directly_related_user_types": [{"type": "user"}]
                        }
                    }
                }
            }
        ]
    });

    let model_id_v2 = create_authorization_model(&store_id, model_v2).await?;

    // Check with v1 should still work (model still exists)
    let check_v1_after = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:doc1"
            },
            "authorization_model_id": &model_id_v1
        }))
        .send()
        .await?;

    assert!(
        check_v1_after.status().is_success(),
        "Check with old model ID should still work"
    );

    // Check with v2 (latest) should also work
    let check_v2 = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:doc1"
            },
            "authorization_model_id": &model_id_v2
        }))
        .send()
        .await?;

    assert!(check_v2.status().is_success());
    let check_v2_body: serde_json::Value = check_v2.json().await?;
    assert_eq!(
        check_v2_body.get("allowed"),
        Some(&json!(true)),
        "Check with v2 model should also return allowed=true"
    );

    // Check without model_id uses latest (v2)
    let check_latest = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:doc1"
            }
        }))
        .send()
        .await?;

    assert!(
        check_latest.status().is_success(),
        "Check without model_id should use latest model"
    );

    Ok(())
}

/// Test: Check with stale model_id
///
/// Verifies that checking with an old (but valid) model_id still works.
/// OpenFGA stores multiple model versions and allows explicit version selection.
#[tokio::test]
async fn test_check_with_stale_model_id() -> Result<()> {
    // Arrange
    let store_id = create_test_store().await?;

    // Create version 1
    let model_v1 = json!({
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

    let model_id_v1 = create_authorization_model(&store_id, model_v1).await?;

    // Write tuple
    write_tuples(&store_id, vec![("user:bob", "viewer", "document:file1")]).await?;

    // Create versions 2 and 3 (to make v1 "stale")
    for i in 2..=3 {
        let model = json!({
            "schema_version": "1.1",
            "type_definitions": [
                {"type": "user"},
                {
                    "type": "document",
                    "relations": {
                        "viewer": {"this": {}},
                        "role": {"this": {}}
                    },
                    "metadata": {
                        "relations": {
                            "viewer": {
                                "directly_related_user_types": [{"type": "user"}]
                            },
                            "role": {
                                "directly_related_user_types": [{"type": "user"}]
                            }
                        }
                    }
                }
            ]
        });

        let _new_model_id = create_authorization_model(&store_id, model).await?;
        eprintln!("Created model version {i}");
    }

    let client = reqwest::Client::new();

    // Act: Check using the old v1 model (now 2 versions behind)
    let check_response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&json!({
            "tuple_key": {
                "user": "user:bob",
                "relation": "viewer",
                "object": "document:file1"
            },
            "authorization_model_id": &model_id_v1
        }))
        .send()
        .await?;

    // Assert: Checking with stale (but valid) model should still work
    assert!(
        check_response.status().is_success(),
        "Check with stale model_id should succeed (status: {})",
        check_response.status()
    );

    let body: serde_json::Value = check_response.json().await?;
    assert_eq!(
        body.get("allowed"),
        Some(&json!(true)),
        "Check with stale model should return correct result"
    );

    // Also verify the model can be retrieved
    let model_response = client
        .get(format!(
            "{}/stores/{}/authorization-models/{}",
            get_openfga_url(),
            store_id,
            model_id_v1
        ))
        .send()
        .await?;

    assert!(
        model_response.status().is_success(),
        "Old model should still be retrievable"
    );

    Ok(())
}
