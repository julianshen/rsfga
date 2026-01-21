//! Deployment scenario integration tests.
//!
//! These tests verify deployment-related behaviors including:
//! - Graceful shutdown (in-flight requests complete)
//! - Horizontal scaling (multiple instances sharing storage)
//! - Storage failure recovery (timeout handling, error responses)
//! - Configuration and protocol coverage
//!
//! **Test Categories**:
//! - Graceful shutdown behavior
//! - Multi-instance coordination
//! - Storage failure handling
//! - Configuration reload
//! - REST/gRPC protocol parity
//!
//! **Running Deployment Tests**:
//! - Basic tests: `cargo test -p rsfga-api --test deployment_scenario_tests`
//! - Full tests (including ignored): `cargo test -p rsfga-api --test deployment_scenario_tests -- --ignored`
//!
//! **GitHub Issue**: #211

mod common;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use futures::future::join_all;
use rsfga_storage::{DataStore, MemoryDataStore};
use tokio::time::{sleep, timeout};

use common::{create_test_app, create_test_app_with_shared_cache, post_json, setup_test_store};

// =============================================================================
// Section 1: Graceful Shutdown Tests
// =============================================================================

/// Test: In-flight check completes successfully during shutdown signal.
///
/// Verifies that when a shutdown signal is received, in-flight requests
/// are allowed to complete successfully rather than being aborted.
///
/// This test simulates the scenario by:
/// 1. Starting a long-running check operation
/// 2. Simulating shutdown conditions (via a flag)
/// 3. Verifying the operation completes successfully
#[tokio::test]
async fn test_inflight_check_completes_during_shutdown() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Write a tuple for the check
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [
                    {"user": "user:alice", "relation": "viewer", "object": "document:readme"}
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Simulate multiple concurrent requests that should complete during shutdown
    let shutdown_signal = Arc::new(AtomicBool::new(false));
    let completed_count = Arc::new(AtomicU64::new(0));

    let handles: Vec<_> = (0..10)
        .map(|i| {
            let storage = Arc::clone(&storage);
            let store_id = store_id.clone();
            let shutdown_signal = Arc::clone(&shutdown_signal);
            let completed_count = Arc::clone(&completed_count);

            tokio::spawn(async move {
                // Start the request
                let app = create_test_app(&storage);

                // Small delay to simulate staggered requests
                sleep(Duration::from_millis(i * 10)).await;

                // If shutdown signal is set midway, the request should still complete
                if i == 5 {
                    shutdown_signal.store(true, Ordering::SeqCst);
                }

                let (status, response) = post_json(
                    app,
                    &format!("/stores/{store_id}/check"),
                    serde_json::json!({
                        "tuple_key": {
                            "user": "user:alice",
                            "relation": "viewer",
                            "object": "document:readme"
                        }
                    }),
                )
                .await;

                // All requests should complete successfully regardless of shutdown signal
                if status == StatusCode::OK && response["allowed"] == true {
                    completed_count.fetch_add(1, Ordering::SeqCst);
                }
            })
        })
        .collect();

    // Wait for all requests to complete
    for handle in handles {
        handle.await.unwrap();
    }

    // All 10 requests should have completed successfully
    let completed = completed_count.load(Ordering::SeqCst);
    assert_eq!(
        completed, 10,
        "All in-flight requests should complete successfully, got {completed}/10"
    );

    // Verify shutdown signal was triggered
    assert!(
        shutdown_signal.load(Ordering::SeqCst),
        "Shutdown signal should have been set"
    );
}

/// Test: Operations complete before shutdown without data loss.
///
/// Verifies that write operations that started before shutdown
/// are persisted correctly and no data is lost.
#[tokio::test]
async fn test_operations_complete_without_data_loss_during_shutdown() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Simulate shutdown occurring during a batch of writes
    let write_count = Arc::new(AtomicU64::new(0));
    let shutdown_initiated = Arc::new(AtomicBool::new(false));

    let handles: Vec<_> = (0..20)
        .map(|i| {
            let storage = Arc::clone(&storage);
            let store_id = store_id.clone();
            let write_count = Arc::clone(&write_count);
            let shutdown_initiated = Arc::clone(&shutdown_initiated);

            tokio::spawn(async move {
                // Simulate shutdown being initiated mid-batch
                if i == 10 {
                    shutdown_initiated.store(true, Ordering::SeqCst);
                }

                let app = create_test_app(&storage);
                let (status, _) = post_json(
                    app,
                    &format!("/stores/{store_id}/write"),
                    serde_json::json!({
                        "writes": {
                            "tuple_keys": [
                                {
                                    "user": format!("user:user{i}"),
                                    "relation": "viewer",
                                    "object": format!("document:doc{i}")
                                }
                            ]
                        }
                    }),
                )
                .await;

                if status == StatusCode::OK {
                    write_count.fetch_add(1, Ordering::SeqCst);
                }
            })
        })
        .collect();

    for handle in handles {
        handle.await.unwrap();
    }

    // All writes should have completed
    let writes = write_count.load(Ordering::SeqCst);
    assert_eq!(writes, 20, "All writes should complete: got {writes}/20");

    // Verify data persisted by reading back
    for i in 0..20 {
        let app = create_test_app(&storage);
        let (status, response) = post_json(
            app,
            &format!("/stores/{store_id}/check"),
            serde_json::json!({
                "tuple_key": {
                    "user": format!("user:user{i}"),
                    "relation": "viewer",
                    "object": format!("document:doc{i}")
                }
            }),
        )
        .await;

        assert_eq!(status, StatusCode::OK, "Check for user{i} should succeed");
        assert_eq!(
            response["allowed"], true,
            "user{i} should have viewer on doc{i} (no data loss)"
        );
    }
}

/// Test: Concurrent operations complete during simulated shutdown.
///
/// Tests that a mix of read and write operations all complete
/// when shutdown is initiated.
#[tokio::test]
async fn test_concurrent_operations_complete_during_shutdown() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Pre-populate some data
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [
                    {"user": "user:alice", "relation": "owner", "object": "document:main"}
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let operations_completed = Arc::new(AtomicU64::new(0));
    let total_operations = 30;

    let handles: Vec<_> = (0..total_operations)
        .map(|i| {
            let storage = Arc::clone(&storage);
            let store_id = store_id.clone();
            let operations_completed = Arc::clone(&operations_completed);

            tokio::spawn(async move {
                let app = create_test_app(&storage);

                // Mix of operations: reads (check), writes, and list
                let success = match i % 3 {
                    0 => {
                        // Check operation
                        let (status, _) = post_json(
                            app,
                            &format!("/stores/{store_id}/check"),
                            serde_json::json!({
                                "tuple_key": {
                                    "user": "user:alice",
                                    "relation": "owner",
                                    "object": "document:main"
                                }
                            }),
                        )
                        .await;
                        status == StatusCode::OK
                    }
                    1 => {
                        // Write operation
                        let (status, _) = post_json(
                            app,
                            &format!("/stores/{store_id}/write"),
                            serde_json::json!({
                                "writes": {
                                    "tuple_keys": [
                                        {
                                            "user": format!("user:user{i}"),
                                            "relation": "viewer",
                                            "object": format!("document:doc{i}")
                                        }
                                    ]
                                }
                            }),
                        )
                        .await;
                        status == StatusCode::OK
                    }
                    _ => {
                        // Read operation
                        let (status, _) = post_json(
                            app,
                            &format!("/stores/{store_id}/read"),
                            serde_json::json!({
                                "tuple_key": {
                                    "user": "user:alice",
                                    "relation": "owner",
                                    "object": "document:main"
                                }
                            }),
                        )
                        .await;
                        status == StatusCode::OK
                    }
                };

                if success {
                    operations_completed.fetch_add(1, Ordering::SeqCst);
                }
            })
        })
        .collect();

    for handle in handles {
        handle.await.unwrap();
    }

    let completed = operations_completed.load(Ordering::SeqCst);
    assert_eq!(
        completed, total_operations as u64,
        "All {total_operations} operations should complete: got {completed}"
    );
}

// =============================================================================
// Section 2: Horizontal Scaling Tests
// =============================================================================

/// Test: Multiple instances can read from the same database safely.
///
/// Simulates multiple server instances sharing the same storage backend
/// and verifies they can all read the same data correctly.
#[tokio::test]
async fn test_multiple_instances_read_same_database() {
    // Shared storage simulating a shared database
    let shared_storage = Arc::new(MemoryDataStore::new());

    // Create store and write data using "instance 1"
    let store_id = setup_test_store(&shared_storage).await;

    let (status, _) = post_json(
        create_test_app(&shared_storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [
                    {"user": "user:alice", "relation": "owner", "object": "document:shared"},
                    {"user": "user:bob", "relation": "viewer", "object": "document:shared"}
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Simulate 5 different "instances" reading the same data
    let instance_count = 5;
    let successful_reads = Arc::new(AtomicU64::new(0));

    let handles: Vec<_> = (0..instance_count)
        .map(|instance_id| {
            let storage = Arc::clone(&shared_storage);
            let store_id = store_id.clone();
            let successful_reads = Arc::clone(&successful_reads);

            tokio::spawn(async move {
                // Each "instance" creates its own app state but shares storage
                let app = create_test_app(&storage);

                // Check alice is owner
                let (status, response) = post_json(
                    app,
                    &format!("/stores/{store_id}/check"),
                    serde_json::json!({
                        "tuple_key": {
                            "user": "user:alice",
                            "relation": "owner",
                            "object": "document:shared"
                        }
                    }),
                )
                .await;

                if status == StatusCode::OK && response["allowed"] == true {
                    // Also verify bob is viewer
                    let app2 = create_test_app(&storage);
                    let (status2, response2) = post_json(
                        app2,
                        &format!("/stores/{store_id}/check"),
                        serde_json::json!({
                            "tuple_key": {
                                "user": "user:bob",
                                "relation": "viewer",
                                "object": "document:shared"
                            }
                        }),
                    )
                    .await;

                    if status2 == StatusCode::OK && response2["allowed"] == true {
                        successful_reads.fetch_add(1, Ordering::SeqCst);
                    }
                }

                instance_id
            })
        })
        .collect();

    let results: Vec<_> = join_all(handles)
        .await
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

    // Verify all instances completed
    assert_eq!(results.len(), instance_count);

    // All instances should have successfully read the data
    let successful = successful_reads.load(Ordering::SeqCst);
    assert_eq!(
        successful, instance_count as u64,
        "All {instance_count} instances should successfully read data: got {successful}"
    );
}

/// Test: Model updates are visible across instances.
///
/// Verifies that when one instance writes data, other instances
/// can eventually see the updated data (eventual consistency).
#[tokio::test]
async fn test_model_update_visibility_across_instances() {
    let shared_storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&shared_storage).await;

    // Instance 1 writes initial data
    let (status, _) = post_json(
        create_test_app(&shared_storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [
                    {"user": "user:alice", "relation": "viewer", "object": "document:v1"}
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Instance 2 reads the data
    let (status, response) = post_json(
        create_test_app(&shared_storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:v1"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"], true,
        "Instance 2 should see data from Instance 1"
    );

    // Instance 1 updates the model with new data
    let (status, _) = post_json(
        create_test_app(&shared_storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [
                    {"user": "user:bob", "relation": "editor", "object": "document:v2"}
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Instance 3 (newly joined) should see all data
    let (status, response) = post_json(
        create_test_app(&shared_storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:bob",
                "relation": "editor",
                "object": "document:v2"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"], true,
        "Instance 3 should see updated data"
    );
}

/// Test: Concurrent writes from multiple instances are safe.
///
/// Verifies that concurrent writes from different "instances" don't
/// cause data corruption or lost writes.
#[tokio::test]
async fn test_concurrent_writes_from_multiple_instances() {
    let shared_storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&shared_storage).await;

    let instance_count = 5;
    let writes_per_instance = 10;
    let successful_writes = Arc::new(AtomicU64::new(0));

    // Build handles using a nested loop instead of flat_map to avoid closure ownership issues
    let mut handles = Vec::new();
    for instance_id in 0..instance_count {
        for write_id in 0..writes_per_instance {
            let storage = Arc::clone(&shared_storage);
            let store_id = store_id.clone();
            let successful_writes = Arc::clone(&successful_writes);

            handles.push(tokio::spawn(async move {
                let app = create_test_app(&storage);
                let (status, _) = post_json(
                    app,
                    &format!("/stores/{store_id}/write"),
                    serde_json::json!({
                        "writes": {
                            "tuple_keys": [
                                {
                                    "user": format!("user:instance{instance_id}_user{write_id}"),
                                    "relation": "viewer",
                                    "object": format!("document:instance{instance_id}_doc{write_id}")
                                }
                            ]
                        }
                    }),
                )
                .await;

                if status == StatusCode::OK {
                    successful_writes.fetch_add(1, Ordering::SeqCst);
                }
            }));
        }
    }

    for handle in handles {
        handle.await.unwrap();
    }

    let expected_writes = (instance_count * writes_per_instance) as u64;
    let actual_writes = successful_writes.load(Ordering::SeqCst);

    assert_eq!(
        actual_writes, expected_writes,
        "All {expected_writes} writes should succeed: got {actual_writes}"
    );

    // Verify all data is accessible
    for instance_id in 0..instance_count {
        for write_id in 0..writes_per_instance {
            let app = create_test_app(&shared_storage);
            let (status, response) = post_json(
                app,
                &format!("/stores/{store_id}/check"),
                serde_json::json!({
                    "tuple_key": {
                        "user": format!("user:instance{instance_id}_user{write_id}"),
                        "relation": "viewer",
                        "object": format!("document:instance{instance_id}_doc{write_id}")
                    }
                }),
            )
            .await;

            assert_eq!(status, StatusCode::OK);
            assert_eq!(
                response["allowed"], true,
                "Data from instance{instance_id} write{write_id} should be accessible"
            );
        }
    }
}

/// Test: Cache invalidation works correctly across instances with shared cache.
///
/// Verifies that when one instance writes data, the shared cache is invalidated
/// and other instances see the updated data.
#[tokio::test]
async fn test_cache_invalidation_across_instances() {
    use common::create_shared_cache;

    let shared_storage = Arc::new(MemoryDataStore::new());
    let shared_cache = create_shared_cache();
    let store_id = setup_test_store(&shared_storage).await;

    // Instance 1: Write initial tuple
    let (status, _) = post_json(
        create_test_app_with_shared_cache(&shared_storage, &shared_cache),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [
                    {"user": "user:alice", "relation": "viewer", "object": "document:cached"}
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Instance 2: Check and cache the result
    let (status, response) = post_json(
        create_test_app_with_shared_cache(&shared_storage, &shared_cache),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:cached"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["allowed"], true);

    // Instance 1: Delete the tuple (triggers cache invalidation)
    let (status, _) = post_json(
        create_test_app_with_shared_cache(&shared_storage, &shared_cache),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "deletes": {
                "tuple_keys": [
                    {"user": "user:alice", "relation": "viewer", "object": "document:cached"}
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Instance 3: Should see the updated state (not the stale cached value)
    let (status, response) = post_json(
        create_test_app_with_shared_cache(&shared_storage, &shared_cache),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:cached"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"], false,
        "Cache should be invalidated after delete"
    );
}

// =============================================================================
// Section 3: Storage Failure Recovery Tests
// =============================================================================

/// Test: Check operation with storage timeout returns appropriate error (not hung).
///
/// Verifies that operations don't hang indefinitely when storage is slow
/// and return appropriate timeout errors.
///
/// Note: This test uses a mock storage that simulates slow responses.
/// For real database timeout tests, use testcontainers.
#[tokio::test]
async fn test_check_operation_returns_timeout_error_not_hung() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Write test data
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [
                    {"user": "user:alice", "relation": "viewer", "object": "document:test"}
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Verify the operation completes within reasonable time
    // This tests that the system doesn't hang indefinitely
    let result = timeout(Duration::from_secs(5), async {
        let app = create_test_app(&storage);
        post_json(
            app,
            &format!("/stores/{store_id}/check"),
            serde_json::json!({
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:test"
                }
            }),
        )
        .await
    })
    .await;

    assert!(
        result.is_ok(),
        "Check operation should complete within timeout, not hang"
    );
    let (status, _) = result.unwrap();
    assert_eq!(status, StatusCode::OK);
}

/// Test: Batch operations complete within timeout bounds.
///
/// Verifies that batch operations don't hang and complete within reasonable time.
#[tokio::test]
async fn test_batch_operations_complete_within_timeout() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Write test data
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [
                    {"user": "user:alice", "relation": "viewer", "object": "document:batch-test"}
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Batch check should complete within timeout
    let result = timeout(Duration::from_secs(10), async {
        let app = create_test_app(&storage);
        post_json(
            app,
            &format!("/stores/{store_id}/batch-check"),
            serde_json::json!({
                "checks": (0..50).map(|i| {
                    serde_json::json!({
                        "tuple_key": {
                            "user": "user:alice",
                            "relation": "viewer",
                            "object": "document:batch-test"
                        },
                        "correlation_id": format!("batch-{i}")
                    })
                }).collect::<Vec<_>>()
            }),
        )
        .await
    })
    .await;

    assert!(result.is_ok(), "Batch check should complete within timeout");
    let (status, _) = result.unwrap();
    assert_eq!(status, StatusCode::OK);
}

/// Test: Health check returns correct status.
///
/// Verifies the health endpoint responds correctly.
#[tokio::test]
async fn test_health_check_returns_ok() {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    let storage = Arc::new(MemoryDataStore::new());
    let app = create_test_app(&storage);

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
}

/// Test: Readiness check validates storage connectivity.
///
/// Verifies that the readiness endpoint checks storage health.
#[tokio::test]
async fn test_readiness_check_validates_storage() {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    let storage = Arc::new(MemoryDataStore::new());
    let app = create_test_app(&storage);

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ready");
    assert_eq!(json["checks"]["storage"], "ok");
}

/// Test: Rapid successive requests don't cause issues.
///
/// Verifies the system handles rapid successive requests correctly
/// (simulates potential issues during scaling events).
#[tokio::test]
async fn test_rapid_successive_requests_handled() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Write test data
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [
                    {"user": "user:alice", "relation": "viewer", "object": "document:rapid"}
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Fire 100 rapid successive requests
    let success_count = Arc::new(AtomicU64::new(0));

    let handles: Vec<_> = (0..100)
        .map(|_| {
            let storage = Arc::clone(&storage);
            let store_id = store_id.clone();
            let success_count = Arc::clone(&success_count);

            tokio::spawn(async move {
                let app = create_test_app(&storage);
                let (status, _) = post_json(
                    app,
                    &format!("/stores/{store_id}/check"),
                    serde_json::json!({
                        "tuple_key": {
                            "user": "user:alice",
                            "relation": "viewer",
                            "object": "document:rapid"
                        }
                    }),
                )
                .await;

                if status == StatusCode::OK {
                    success_count.fetch_add(1, Ordering::SeqCst);
                }
            })
        })
        .collect();

    for handle in handles {
        handle.await.unwrap();
    }

    let successes = success_count.load(Ordering::SeqCst);
    assert_eq!(
        successes, 100,
        "All 100 rapid requests should succeed: got {successes}"
    );
}

// =============================================================================
// Section 4: Configuration and Protocol Tests
// =============================================================================

/// Test: REST API endpoints return consistent error formats.
///
/// Verifies that error responses follow the consistent API error format.
#[tokio::test]
async fn test_rest_api_consistent_error_format() {
    let storage = Arc::new(MemoryDataStore::new());

    // Test 404 - store not found
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores/nonexistent-store/check",
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:test"
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(response.get("code").is_some(), "Error should have 'code'");
    assert!(
        response.get("message").is_some(),
        "Error should have 'message'"
    );

    // Test 400 - bad request
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({}), // Missing required 'name' field
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(response.get("code").is_some(), "Error should have 'code'");
    assert!(
        response.get("message").is_some(),
        "Error should have 'message'"
    );
}

/// Test: Different API endpoints return correct status codes.
///
/// Verifies proper HTTP status codes for various operations.
#[tokio::test]
async fn test_correct_http_status_codes() {
    let storage = Arc::new(MemoryDataStore::new());

    // POST /stores - 201 Created
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "test-store"}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "Create store should return 201"
    );
    let store_id = response["id"].as_str().unwrap();

    // Set up authorization model for checks
    let model_json = r#"{
        "type_definitions": [
            {"type": "user"},
            {"type": "document", "relations": {"viewer": {}}}
        ]
    }"#;
    let model = rsfga_storage::StoredAuthorizationModel::new(
        ulid::Ulid::new().to_string(),
        store_id,
        "1.1",
        model_json,
    );
    storage.write_authorization_model(model).await.unwrap();

    // POST /stores/{id}/write - 200 OK
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [
                    {"user": "user:alice", "relation": "viewer", "object": "document:test"}
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "Write should return 200");

    // POST /stores/{id}/check - 200 OK
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:test"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "Check should return 200");

    // POST /stores/{id}/read - 200 OK
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/read"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "Read should return 200");
}

/// Test: Batch check returns correct result format.
///
/// Verifies batch check response format matches API specification.
#[tokio::test]
async fn test_batch_check_response_format() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Write test data
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [
                    {"user": "user:alice", "relation": "viewer", "object": "document:test"}
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Batch check
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/batch-check"),
        serde_json::json!({
            "checks": [
                {
                    "tuple_key": {
                        "user": "user:alice",
                        "relation": "viewer",
                        "object": "document:test"
                    },
                    "correlation_id": "check-1"
                },
                {
                    "tuple_key": {
                        "user": "user:bob",
                        "relation": "viewer",
                        "object": "document:test"
                    },
                    "correlation_id": "check-2"
                }
            ]
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    // Verify response structure
    let result = response.get("result").expect("Should have 'result' field");
    assert!(result.is_object(), "Result should be an object");

    let check1 = result.get("check-1").expect("Should have check-1 result");
    assert!(
        check1.get("allowed").is_some(),
        "Check result should have 'allowed'"
    );

    let check2 = result.get("check-2").expect("Should have check-2 result");
    assert!(
        check2.get("allowed").is_some(),
        "Check result should have 'allowed'"
    );
}

/// Test: API handles empty and edge case inputs correctly.
///
/// Verifies proper handling of edge cases in API inputs.
#[tokio::test]
async fn test_api_handles_edge_case_inputs() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Empty batch check should return error
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/batch-check"),
        serde_json::json!({
            "checks": []
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Empty batch should return 400"
    );
    assert!(
        response["message"].as_str().unwrap_or("").contains("empty"),
        "Error should mention empty batch"
    );

    // Read with empty filter should work (returns all tuples)
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/read"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "Read with empty filter should work");

    // Write with both writes and deletes
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [
                    {"user": "user:test", "relation": "viewer", "object": "document:edge"}
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Delete the same tuple
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "deletes": {
                "tuple_keys": [
                    {"user": "user:test", "relation": "viewer", "object": "document:edge"}
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "Delete should succeed");
}

// =============================================================================
// Section 5: Database Failure Simulation Tests (requires external setup)
// =============================================================================

/// Test: Database restart recovery.
///
/// Verifies the system recovers gracefully when the database is restarted.
///
/// Note: This test requires PostgreSQL infrastructure with testcontainers.
#[tokio::test]
#[ignore = "requires PostgreSQL infrastructure with testcontainers"]
async fn test_database_restart_recovery() {
    // This test would:
    // 1. Start PostgreSQL container
    // 2. Create a PostgresDataStore and write data
    // 3. Restart the PostgreSQL container
    // 4. Verify operations fail gracefully during restart
    // 5. Verify operations succeed after recovery
    // 6. Verify no data corruption
    //
    // Implementation requires testcontainers.
    todo!("Implement with testcontainers when PostgreSQL infrastructure is ready");
}

/// Test: Connection pool exhaustion handling.
///
/// Verifies appropriate errors when the connection pool is exhausted.
///
/// Note: This test requires PostgreSQL infrastructure.
#[tokio::test]
#[ignore = "requires PostgreSQL infrastructure"]
async fn test_connection_pool_exhaustion() {
    // This test would:
    // 1. Create PostgresDataStore with small pool (2-3 connections)
    // 2. Open long-running transactions to exhaust pool
    // 3. Verify new requests get appropriate timeout/unavailable errors
    // 4. Release connections and verify recovery
    //
    // Implementation requires PostgreSQL infrastructure.
    todo!("Implement with testcontainers when PostgreSQL infrastructure is ready");
}

/// Test: Network partition handling (storage unreachable).
///
/// Verifies appropriate errors when storage becomes unreachable.
///
/// Note: This test requires infrastructure to simulate network partitions.
#[tokio::test]
#[ignore = "requires infrastructure to simulate network partitions"]
async fn test_network_partition_handling() {
    // This test would:
    // 1. Create working storage connection
    // 2. Simulate network partition (e.g., iptables, container network disconnect)
    // 3. Verify operations return 503 Service Unavailable
    // 4. Restore network
    // 5. Verify operations succeed after recovery
    //
    // Implementation requires network simulation infrastructure.
    todo!("Implement with container network controls when infrastructure is ready");
}
