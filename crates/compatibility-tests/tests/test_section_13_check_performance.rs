mod common;

use anyhow::Result;
use common::{create_test_model, create_test_store, get_openfga_url, write_tuples};
use serde_json::json;

/// Test: Measure check latency for direct relations (baseline)
#[tokio::test]
async fn test_check_latency_direct_relation() -> Result<()> {
    // Arrange: Create store, model, and write tuple
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    write_tuples(
        &store_id,
        vec![("user:alice", "viewer", "document:doc1")],
    )
    .await?;

    let client = reqwest::Client::new();

    let check_request = json!({
        "tuple_key": {
            "user": "user:alice",
            "relation": "viewer",
            "object": "document:doc1"
        }
    });

    // Warm up: Run once to establish connections
    client
        .post(format!(
            "{}/stores/{}/check",
            get_openfga_url(),
            store_id
        ))
        .json(&check_request)
        .send()
        .await?;

    // Act: Measure latency over 10 checks
    let iterations = 10;
    let mut total_duration = std::time::Duration::ZERO;

    for _ in 0..iterations {
        let start = std::time::Instant::now();

        let response = client
            .post(format!(
                "{}/stores/{}/check",
                get_openfga_url(),
                store_id
            ))
            .json(&check_request)
            .send()
            .await?;

        total_duration += start.elapsed();

        assert!(response.status().is_success(), "Check should succeed");
    }

    let avg_latency = total_duration / iterations;

    // Assert: Performance baseline documented
    println!(
        "OpenFGA direct relation check average latency: {:?}",
        avg_latency
    );

    // Sanity check: Should complete in reasonable time (< 1 second per check)
    assert!(
        avg_latency.as_millis() < 1000,
        "Direct relation check should complete in < 1s, got: {:?}",
        avg_latency
    );

    Ok(())
}

/// Test: Measure check latency for computed relations (union)
#[tokio::test]
async fn test_check_latency_computed_union() -> Result<()> {
    // Arrange: Create store with union relation
    let store_id = create_test_store().await?;

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
                    },
                    "can_view": {
                        "union": {
                            "child": [
                                {
                                    "computedUserset": {
                                        "relation": "viewer"
                                    }
                                },
                                {
                                    "computedUserset": {
                                        "relation": "editor"
                                    }
                                }
                            ]
                        }
                    }
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

    let client = reqwest::Client::new();

    client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model)
        .send()
        .await?;

    write_tuples(&store_id, vec![("user:bob", "editor", "document:doc1")])
        .await?;

    let check_request = json!({
        "tuple_key": {
            "user": "user:bob",
            "relation": "can_view",
            "object": "document:doc1"
        }
    });

    // Warm up
    client
        .post(format!(
            "{}/stores/{}/check",
            get_openfga_url(),
            store_id
        ))
        .json(&check_request)
        .send()
        .await?;

    // Act: Measure latency over 10 checks
    let iterations = 10;
    let mut total_duration = std::time::Duration::ZERO;

    for _ in 0..iterations {
        let start = std::time::Instant::now();

        let response = client
            .post(format!(
                "{}/stores/{}/check",
                get_openfga_url(),
                store_id
            ))
            .json(&check_request)
            .send()
            .await?;

        total_duration += start.elapsed();

        assert!(response.status().is_success(), "Check should succeed");
    }

    let avg_latency = total_duration / iterations;

    // Assert: Performance baseline documented
    println!(
        "OpenFGA union relation check average latency: {:?}",
        avg_latency
    );

    assert!(
        avg_latency.as_millis() < 1000,
        "Union relation check should complete in < 1s, got: {:?}",
        avg_latency
    );

    Ok(())
}

/// Test: Measure check latency for deep nested relations
#[tokio::test]
async fn test_check_latency_deep_nested() -> Result<()> {
    // Arrange: Create store with parent-child hierarchy
    let store_id = create_test_store().await?;

    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            {
                "type": "user"
            },
            {
                "type": "folder",
                "relations": {
                    "parent": {
                        "this": {}
                    },
                    "viewer": {
                        "union": {
                            "child": [
                                {
                                    "this": {}
                                },
                                {
                                    "tupleToUserset": {
                                        "tupleset": {
                                            "relation": "parent"
                                        },
                                        "computedUserset": {
                                            "relation": "viewer"
                                        }
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

    let client = reqwest::Client::new();

    client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model)
        .send()
        .await?;

    // Create 5-level deep hierarchy
    write_tuples(
        &store_id,
        vec![
            ("folder:level1", "parent", "folder:level2"),
            ("folder:level2", "parent", "folder:level3"),
            ("folder:level3", "parent", "folder:level4"),
            ("folder:level4", "parent", "folder:level5"),
            ("user:charlie", "viewer", "folder:level1"),
        ],
    )
    .await?;

    let check_request = json!({
        "tuple_key": {
            "user": "user:charlie",
            "relation": "viewer",
            "object": "folder:level5"
        }
    });

    // Warm up
    client
        .post(format!(
            "{}/stores/{}/check",
            get_openfga_url(),
            store_id
        ))
        .json(&check_request)
        .send()
        .await?;

    // Act: Measure latency over 10 checks
    let iterations = 10;
    let mut total_duration = std::time::Duration::ZERO;

    for _ in 0..iterations {
        let start = std::time::Instant::now();

        let response = client
            .post(format!(
                "{}/stores/{}/check",
                get_openfga_url(),
                store_id
            ))
            .json(&check_request)
            .send()
            .await?;

        total_duration += start.elapsed();

        assert!(response.status().is_success(), "Check should succeed");
    }

    let avg_latency = total_duration / iterations;

    // Assert: Performance baseline documented
    println!(
        "OpenFGA deep nested (5-hop) check average latency: {:?}",
        avg_latency
    );

    assert!(
        avg_latency.as_millis() < 2000,
        "Deep nested check should complete in < 2s, got: {:?}",
        avg_latency
    );

    Ok(())
}

/// Test: Check immediately after write reflects new tuple (consistency)
#[tokio::test]
async fn test_check_consistency_after_write() -> Result<()> {
    // Arrange: Create store and model
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    let client = reqwest::Client::new();

    // Act: Write tuple
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:dave",
                    "relation": "viewer",
                    "object": "document:doc1"
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
        "Write should succeed"
    );

    // Act: Check IMMEDIATELY after write (no delay)
    let check_request = json!({
        "tuple_key": {
            "user": "user:dave",
            "relation": "viewer",
            "object": "document:doc1"
        }
    });

    let check_response = client
        .post(format!(
            "{}/stores/{}/check",
            get_openfga_url(),
            store_id
        ))
        .json(&check_request)
        .send()
        .await?;

    let response_body: serde_json::Value = check_response.json().await?;

    // Assert: Check immediately reflects write (strong consistency)
    assert_eq!(
        response_body.get("allowed").and_then(|v| v.as_bool()),
        Some(true),
        "Check should immediately reflect write (strong consistency)"
    );

    Ok(())
}

/// Test: Check after delete reflects removed tuple
#[tokio::test]
async fn test_check_consistency_after_delete() -> Result<()> {
    // Arrange: Create store, model, and write tuple
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    write_tuples(&store_id, vec![("user:eve", "viewer", "document:doc1")])
        .await?;

    let client = reqwest::Client::new();

    // Verify tuple exists
    let check_before = json!({
        "tuple_key": {
            "user": "user:eve",
            "relation": "viewer",
            "object": "document:doc1"
        }
    });

    let response_before = client
        .post(format!(
            "{}/stores/{}/check",
            get_openfga_url(),
            store_id
        ))
        .json(&check_before)
        .send()
        .await?;

    let before_body: serde_json::Value = response_before.json().await?;

    assert_eq!(
        before_body.get("allowed").and_then(|v| v.as_bool()),
        Some(true),
        "Tuple should exist before delete"
    );

    // Act: Delete the tuple
    let delete_request = json!({
        "deletes": {
            "tuple_keys": [
                {
                    "user": "user:eve",
                    "relation": "viewer",
                    "object": "document:doc1"
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
        "Delete should succeed"
    );

    // Act: Check IMMEDIATELY after delete (no delay)
    let check_after = json!({
        "tuple_key": {
            "user": "user:eve",
            "relation": "viewer",
            "object": "document:doc1"
        }
    });

    let response_after = client
        .post(format!(
            "{}/stores/{}/check",
            get_openfga_url(),
            store_id
        ))
        .json(&check_after)
        .send()
        .await?;

    let after_body: serde_json::Value = response_after.json().await?;

    // Assert: Check immediately reflects delete (strong consistency)
    assert_eq!(
        after_body.get("allowed").and_then(|v| v.as_bool()),
        Some(false),
        "Check should immediately reflect delete (strong consistency)"
    );

    Ok(())
}
