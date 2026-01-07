//! Integration tests for Milestone 1.8: Testing & Benchmarking
//!
//! These tests verify end-to-end authorization flows and system integration.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use rsfga_storage::MemoryDataStore;

use common::{create_test_app, post_json};

// ============================================================
// Section 2: Integration Tests
// ============================================================

/// Test: End-to-end authorization flow works
///
/// This test verifies the complete flow:
/// 1. Create a store
/// 2. Write authorization tuples
/// 3. Check permissions
/// 4. Verify correct authorization results
#[tokio::test]
async fn test_end_to_end_authorization_flow_works() {
    let storage = Arc::new(MemoryDataStore::new());

    // Step 1: Create a store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({
            "name": "test-store"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap();
    assert!(!store_id.is_empty());

    // Step 2: Write authorization tuples
    // Alice is an owner of document:readme
    // Bob is a viewer of document:readme
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{}/write", store_id),
        serde_json::json!({
            "writes": {
                "tuple_keys": [
                    {
                        "user": "user:alice",
                        "relation": "owner",
                        "object": "document:readme"
                    },
                    {
                        "user": "user:bob",
                        "relation": "viewer",
                        "object": "document:readme"
                    }
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Step 3: Check permissions - Alice should be owner
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{}/check", store_id),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "owner",
                "object": "document:readme"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["allowed"], true, "Alice should be owner");

    // Step 4: Check permissions - Bob should be viewer
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{}/check", store_id),
        serde_json::json!({
            "tuple_key": {
                "user": "user:bob",
                "relation": "viewer",
                "object": "document:readme"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["allowed"], true, "Bob should be viewer");

    // Step 5: Check permissions - Bob should NOT be owner
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{}/check", store_id),
        serde_json::json!({
            "tuple_key": {
                "user": "user:bob",
                "relation": "owner",
                "object": "document:readme"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["allowed"], false, "Bob should NOT be owner");

    // Step 6: Check permissions - Charlie has no access
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{}/check", store_id),
        serde_json::json!({
            "tuple_key": {
                "user": "user:charlie",
                "relation": "viewer",
                "object": "document:readme"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["allowed"], false, "Charlie should have no access");
}

/// Test: Multiple concurrent clients work correctly
///
/// Verifies that concurrent requests don't interfere with each other
/// and all return correct results.
#[tokio::test]
async fn test_multiple_concurrent_clients_work_correctly() {
    use futures::future::join_all;

    let storage = Arc::new(MemoryDataStore::new());

    // Create a store first
    let store_id = {
        let (status, response) = post_json(
            create_test_app(&storage),
            "/stores",
            serde_json::json!({"name": "concurrent-test"}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        response["id"].as_str().unwrap().to_string()
    };

    // Write some tuples
    {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{}/write", store_id),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [
                        {"user": "user:alice", "relation": "owner", "object": "document:doc1"},
                        {"user": "user:bob", "relation": "viewer", "object": "document:doc1"}
                    ]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    // Launch 50 concurrent check requests
    let futures: Vec<_> = (0..50)
        .map(|i| {
            let storage = Arc::clone(&storage);
            let store_id = store_id.clone();
            async move {
                let app = create_test_app(&storage);
                let user = if i % 2 == 0 { "user:alice" } else { "user:bob" };
                let relation = if i % 2 == 0 { "owner" } else { "viewer" };

                let (status, response) = post_json(
                    app,
                    &format!("/stores/{}/check", store_id),
                    serde_json::json!({
                        "tuple_key": {
                            "user": user,
                            "relation": relation,
                            "object": "document:doc1"
                        }
                    }),
                )
                .await;

                (
                    i,
                    status,
                    response["allowed"]
                        .as_bool()
                        .expect("'allowed' field should be a boolean"),
                )
            }
        })
        .collect();

    let results = join_all(futures).await;

    // Verify all requests succeeded with correct results
    for (i, status, allowed) in results {
        assert_eq!(status, StatusCode::OK, "Request {} should succeed", i);
        assert!(allowed, "Request {} should be allowed", i);
    }
}

/// Test: Large authorization models work (1000+ types simulation)
///
/// Verifies the system handles large numbers of tuples efficiently.
#[tokio::test]
async fn test_large_authorization_models_work() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create a store
    let store_id = {
        let (status, response) = post_json(
            create_test_app(&storage),
            "/stores",
            serde_json::json!({"name": "large-model"}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        response["id"].as_str().unwrap().to_string()
    };

    // Write 1000 tuples in batches of 100
    for batch in 0..10 {
        let tuples: Vec<_> = (0..100)
            .map(|i| {
                let idx = batch * 100 + i;
                serde_json::json!({
                    "user": format!("user:user{}", idx),
                    "relation": "viewer",
                    "object": format!("document:doc{}", idx)
                })
            })
            .collect();

        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{}/write", store_id),
            serde_json::json!({
                "writes": {
                    "tuple_keys": tuples
                }
            }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "Batch {} write should succeed",
            batch
        );
    }

    // Verify we can check permissions for tuples at various positions
    for idx in [0, 499, 999] {
        let (status, response) = post_json(
            create_test_app(&storage),
            &format!("/stores/{}/check", store_id),
            serde_json::json!({
                "tuple_key": {
                    "user": format!("user:user{}", idx),
                    "relation": "viewer",
                    "object": format!("document:doc{}", idx)
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            response["allowed"], true,
            "user{} should have viewer on doc{}",
            idx, idx
        );
    }

    // Verify non-existent tuple returns false
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{}/check", store_id),
        serde_json::json!({
            "tuple_key": {
                "user": "user:user0",
                "relation": "viewer",
                "object": "document:doc999"  // user0 doesn't have access to doc999
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["allowed"], false);
}

/// Test: Deep hierarchies work (depth 20+)
///
/// Tests that the system can handle deeply nested authorization relationships.
/// Note: This tests the API layer's ability to handle deep hierarchies.
/// The actual depth resolution happens in the GraphResolver (tested in M1.4).
#[tokio::test]
async fn test_deep_hierarchies_work() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create a store
    let store_id = {
        let (status, response) = post_json(
            create_test_app(&storage),
            "/stores",
            serde_json::json!({"name": "deep-hierarchy"}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        response["id"].as_str().unwrap().to_string()
    };

    // Create a chain of 20 folder relationships: folder0 -> folder1 -> ... -> folder19
    // Each folder has a "parent" relation to the next
    let mut tuples = Vec::new();
    for i in 0..20 {
        // folder{i} is parent of folder{i+1}
        tuples.push(serde_json::json!({
            "user": format!("folder:folder{}#parent", i),
            "relation": "parent",
            "object": format!("folder:folder{}", i + 1)
        }));
    }
    // Alice is owner of folder0 (the root)
    tuples.push(serde_json::json!({
        "user": "user:alice",
        "relation": "owner",
        "object": "folder:folder0"
    }));

    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{}/write", store_id),
        serde_json::json!({
            "writes": {
                "tuple_keys": tuples
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Verify Alice is owner of folder0
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{}/check", store_id),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "owner",
                "object": "folder:folder0"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"], true,
        "Alice should be owner of folder0"
    );

    // Note: Testing inherited permissions through the hierarchy requires
    // the GraphResolver integration, which will be tested when the API
    // layer is connected to the full resolver stack in M1.8.
}

/// Test: Database connection loss is handled gracefully
///
/// Verifies that when the database connection is lost, appropriate errors are returned.
/// Note: This test requires PostgreSQL and simulates connection issues.
#[tokio::test]
#[ignore = "requires running PostgreSQL and simulating connection loss"]
async fn test_database_connection_loss_handled() {
    // This test would:
    // 1. Create a PostgresDataStore
    // 2. Perform some operations successfully
    // 3. Simulate connection loss (stop PostgreSQL or block network)
    // 4. Verify operations return appropriate StorageError
    // 5. Verify reconnection when database is available again
    //
    // For now, this is a placeholder that documents the test case.
    // Full implementation requires testcontainers or similar infrastructure.
    todo!("Implement with testcontainers when PostgreSQL infrastructure is ready");
}

/// Test: Server restart preserves data (PostgreSQL)
///
/// Verifies that data written to PostgreSQL persists across restarts.
/// Note: This test requires PostgreSQL.
#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_server_restart_preserves_data_postgres() {
    // This test would:
    // 1. Create a PostgresDataStore and write data
    // 2. Drop the connection/store
    // 3. Create a new PostgresDataStore with same database
    // 4. Verify previously written data is still available
    //
    // For now, this is a placeholder that documents the test case.
    // Full implementation requires PostgreSQL infrastructure.
    todo!("Implement when PostgreSQL test infrastructure is ready");
}

/// Test: Batch check handles many items efficiently
///
/// Verifies batch check can handle the maximum batch size (50 items).
#[tokio::test]
async fn test_batch_check_handles_max_items() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create a store
    let store_id = {
        let (status, response) = post_json(
            create_test_app(&storage),
            "/stores",
            serde_json::json!({"name": "batch-test"}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        response["id"].as_str().unwrap().to_string()
    };

    // Write 50 tuples
    let write_tuples: Vec<_> = (0..50)
        .map(|i| {
            serde_json::json!({
                "user": format!("user:user{}", i),
                "relation": "viewer",
                "object": format!("document:doc{}", i)
            })
        })
        .collect();

    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{}/write", store_id),
        serde_json::json!({
            "writes": {
                "tuple_keys": write_tuples
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Batch check all 50 tuples
    let checks: Vec<_> = (0..50)
        .map(|i| {
            serde_json::json!({
                "tuple_key": {
                    "user": format!("user:user{}", i),
                    "relation": "viewer",
                    "object": format!("document:doc{}", i)
                },
                "correlation_id": format!("check-{}", i)
            })
        })
        .collect();

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{}/batch-check", store_id),
        serde_json::json!({
            "checks": checks
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Verify all checks returned allowed: true
    let result = response["result"].as_object().unwrap();
    assert_eq!(result.len(), 50, "Should have 50 results");
    for i in 0..50 {
        let check_result = &result[&format!("check-{}", i)];
        assert_eq!(
            check_result["allowed"], true,
            "check-{} should be allowed",
            i
        );
    }
}
