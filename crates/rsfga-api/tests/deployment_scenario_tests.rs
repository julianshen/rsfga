//! Deployment scenario integration tests.
//!
//! These tests verify deployment-related behaviors including:
//! - Concurrent request completion under load
//! - Horizontal scaling (multiple instances sharing storage)
//! - Operation timeout bounds (no hanging)
//! - Configuration and protocol coverage
//!
//! **Test Categories**:
//! - Section 1: Concurrent request completion tests
//! - Section 2: Multi-instance coordination with shared storage
//! - Section 3: Operation timeout and health check tests
//! - Section 4: API consistency and protocol tests
//! - Section 5: Database failure simulation (requires external infrastructure)
//!
//! **Limitations**:
//! - Graceful shutdown tests validate concurrent completion, not actual signal handling
//! - Cache invalidation tests use in-process cache, not distributed cache
//! - Storage timeout tests verify completion bounds, not actual timeout errors
//!
//! For production deployment validation, consider:
//! - Integration tests with actual server processes and signal handling
//! - Distributed cache solutions (Redis) for multi-instance cache invalidation
//! - Mock DataStore implementations for timeout error testing
//! - Testcontainers for database failure scenarios
//!
//! **Running Deployment Tests**:
//! - Basic tests: `cargo test -p rsfga-api --test deployment_scenario_tests`
//! - Full tests (including ignored): `cargo test -p rsfga-api --test deployment_scenario_tests -- --ignored`
//!
//! **GitHub Issue**: #211

mod common;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use rsfga_storage::{DataStore, MemoryDataStore};
use tokio::time::timeout;

use common::{create_test_app, create_test_app_with_shared_cache, post_json, setup_test_store};

// =============================================================================
// Section 1: Concurrent Request Completion Tests
//
// These tests verify that the server handles concurrent requests correctly
// without race conditions, deadlocks, or data corruption.
// =============================================================================

// Number of concurrent requests for load tests
const CONCURRENT_CHECK_REQUESTS: usize = 10;
const CONCURRENT_WRITE_REQUESTS: usize = 20;
const CONCURRENT_MIXED_OPERATIONS: usize = 30;

/// Test: Concurrent check requests complete successfully.
///
/// Verifies that multiple concurrent check requests all complete successfully
/// without race conditions or errors.
#[tokio::test]
async fn test_concurrent_check_requests_complete_successfully() {
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
    assert_eq!(status, StatusCode::OK, "Setup: write tuple should succeed");

    // Fire concurrent check requests
    let success_count = Arc::new(AtomicU64::new(0));

    let handles: Vec<_> = (0..CONCURRENT_CHECK_REQUESTS)
        .map(|_| {
            let storage = Arc::clone(&storage);
            let store_id = store_id.clone();
            let success_count = Arc::clone(&success_count);

            tokio::spawn(async move {
                let app = create_test_app(&storage);
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

                if status == StatusCode::OK && response["allowed"] == true {
                    success_count.fetch_add(1, Ordering::SeqCst);
                }
            })
        })
        .collect();

    for handle in handles {
        handle.await.expect("Task should not panic");
    }

    let completed = success_count.load(Ordering::SeqCst);
    assert_eq!(
        completed, CONCURRENT_CHECK_REQUESTS as u64,
        "All {CONCURRENT_CHECK_REQUESTS} concurrent check requests should succeed, got {completed}"
    );
}

/// Test: Concurrent write operations complete without data loss.
///
/// Verifies that concurrent write operations all persist correctly
/// and no data is lost due to race conditions.
#[tokio::test]
async fn test_concurrent_writes_complete_without_data_loss() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let success_count = Arc::new(AtomicU64::new(0));

    let handles: Vec<_> = (0..CONCURRENT_WRITE_REQUESTS)
        .map(|i| {
            let storage = Arc::clone(&storage);
            let store_id = store_id.clone();
            let success_count = Arc::clone(&success_count);

            tokio::spawn(async move {
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
                    success_count.fetch_add(1, Ordering::SeqCst);
                }
            })
        })
        .collect();

    for handle in handles {
        handle.await.expect("Task should not panic");
    }

    let writes = success_count.load(Ordering::SeqCst);
    assert_eq!(
        writes, CONCURRENT_WRITE_REQUESTS as u64,
        "All {CONCURRENT_WRITE_REQUESTS} concurrent writes should succeed, got {writes}"
    );

    // Verify all data persisted by reading back
    for i in 0..CONCURRENT_WRITE_REQUESTS {
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

        assert_eq!(
            status,
            StatusCode::OK,
            "Check for user{i} should return OK status"
        );
        assert_eq!(
            response["allowed"], true,
            "user{i} should have viewer permission on doc{i} (no data loss)"
        );
    }
}

/// Test: Mixed concurrent operations (check, write, read) complete successfully.
///
/// Verifies that a mix of different operation types can be executed
/// concurrently without race conditions or errors.
#[tokio::test]
async fn test_mixed_concurrent_operations_complete_successfully() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Pre-populate data for check/read operations
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
    assert_eq!(
        status,
        StatusCode::OK,
        "Setup: write initial tuple should succeed"
    );

    let success_count = Arc::new(AtomicU64::new(0));

    let handles: Vec<_> = (0..CONCURRENT_MIXED_OPERATIONS)
        .map(|i| {
            let storage = Arc::clone(&storage);
            let store_id = store_id.clone();
            let success_count = Arc::clone(&success_count);

            tokio::spawn(async move {
                let app = create_test_app(&storage);

                // Distribute operations: check (33%), write (33%), read (34%)
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
                        // Write operation (each writes unique tuple to avoid conflicts)
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
                    success_count.fetch_add(1, Ordering::SeqCst);
                }
            })
        })
        .collect();

    for handle in handles {
        handle.await.expect("Task should not panic");
    }

    let completed = success_count.load(Ordering::SeqCst);
    assert_eq!(
        completed, CONCURRENT_MIXED_OPERATIONS as u64,
        "All {CONCURRENT_MIXED_OPERATIONS} mixed operations should succeed, got {completed}"
    );
}

// =============================================================================
// Section 2: Horizontal Scaling Tests
//
// These tests verify behavior when multiple server instances share
// the same storage backend, simulating horizontal scaling scenarios.
// =============================================================================

// Number of simulated instances for scaling tests
const SIMULATED_INSTANCE_COUNT: usize = 5;
const WRITES_PER_INSTANCE: usize = 10;

/// Test: Multiple instances can read from the same database safely.
///
/// Simulates multiple server instances sharing the same storage backend
/// and verifies they can all read the same data correctly.
#[tokio::test]
async fn test_multiple_instances_read_same_database() {
    let shared_storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&shared_storage).await;

    // Write data using first "instance"
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
    assert_eq!(status, StatusCode::OK, "Setup: write tuples should succeed");

    // Simulate multiple instances reading the same data
    let success_count = Arc::new(AtomicU64::new(0));

    let handles: Vec<_> = (0..SIMULATED_INSTANCE_COUNT)
        .map(|_| {
            let storage = Arc::clone(&shared_storage);
            let store_id = store_id.clone();
            let success_count = Arc::clone(&success_count);

            tokio::spawn(async move {
                // Each "instance" creates its own Router but shares storage
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
                        success_count.fetch_add(1, Ordering::SeqCst);
                    }
                }
            })
        })
        .collect();

    for handle in handles {
        handle.await.expect("Task should not panic");
    }

    let successful = success_count.load(Ordering::SeqCst);
    assert_eq!(
        successful, SIMULATED_INSTANCE_COUNT as u64,
        "All {SIMULATED_INSTANCE_COUNT} instances should read data successfully, got {successful}"
    );
}

/// Test: Data written by one instance is visible to other instances.
///
/// Verifies that with shared storage, writes from one instance are
/// immediately visible to other instances (strong consistency).
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
    assert_eq!(status, StatusCode::OK, "Instance 1: write should succeed");

    // Instance 2 should immediately see the data
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
    assert_eq!(status, StatusCode::OK, "Instance 2: check should return OK");
    assert_eq!(
        response["allowed"], true,
        "Instance 2 should see data from Instance 1"
    );

    // Instance 1 writes additional data
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
    assert_eq!(
        status,
        StatusCode::OK,
        "Instance 1: second write should succeed"
    );

    // Instance 3 (new instance) should see all data
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
    assert_eq!(status, StatusCode::OK, "Instance 3: check should return OK");
    assert_eq!(
        response["allowed"], true,
        "Instance 3 should see data written by Instance 1"
    );
}

/// Test: Concurrent writes from multiple instances don't lose data.
///
/// Verifies that concurrent writes from different "instances" all
/// persist correctly without data corruption or lost writes.
#[tokio::test]
async fn test_concurrent_writes_from_multiple_instances() {
    let shared_storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&shared_storage).await;

    let success_count = Arc::new(AtomicU64::new(0));
    let expected_total = SIMULATED_INSTANCE_COUNT * WRITES_PER_INSTANCE;

    // Each simulated instance writes multiple tuples concurrently
    let mut handles = Vec::new();
    for instance_id in 0..SIMULATED_INSTANCE_COUNT {
        for write_id in 0..WRITES_PER_INSTANCE {
            let storage = Arc::clone(&shared_storage);
            let store_id = store_id.clone();
            let success_count = Arc::clone(&success_count);

            handles.push(tokio::spawn(async move {
                let app = create_test_app(&storage);
                let (status, _) = post_json(
                    app,
                    &format!("/stores/{store_id}/write"),
                    serde_json::json!({
                        "writes": {
                            "tuple_keys": [
                                {
                                    // Unique tuple per instance+write to avoid conflicts
                                    "user": format!("user:inst{instance_id}_user{write_id}"),
                                    "relation": "viewer",
                                    "object": format!("document:inst{instance_id}_doc{write_id}")
                                }
                            ]
                        }
                    }),
                )
                .await;

                if status == StatusCode::OK {
                    success_count.fetch_add(1, Ordering::SeqCst);
                }
            }));
        }
    }

    for handle in handles {
        handle.await.expect("Task should not panic");
    }

    let actual_count = success_count.load(Ordering::SeqCst);
    assert_eq!(
        actual_count, expected_total as u64,
        "All {expected_total} writes from {SIMULATED_INSTANCE_COUNT} instances should succeed, got {actual_count}"
    );

    // Verify all data is accessible (no data loss)
    for instance_id in 0..SIMULATED_INSTANCE_COUNT {
        for write_id in 0..WRITES_PER_INSTANCE {
            let app = create_test_app(&shared_storage);
            let (status, response) = post_json(
                app,
                &format!("/stores/{store_id}/check"),
                serde_json::json!({
                    "tuple_key": {
                        "user": format!("user:inst{instance_id}_user{write_id}"),
                        "relation": "viewer",
                        "object": format!("document:inst{instance_id}_doc{write_id}")
                    }
                }),
            )
            .await;

            assert_eq!(
                status,
                StatusCode::OK,
                "Check for inst{instance_id}_user{write_id} should return OK"
            );
            assert_eq!(
                response["allowed"], true,
                "inst{instance_id}_user{write_id} should have viewer permission (no data loss)"
            );
        }
    }
}

/// Test: In-process cache invalidation with shared cache reference.
///
/// Verifies that when data is written, the in-process cache is invalidated
/// and subsequent reads see the updated data.
///
/// **Important**: This test uses multiple `Router` instances sharing the same
/// in-memory cache reference. It validates in-process cache invalidation only.
/// For true multi-instance scenarios (e.g., distributed deployment), a
/// distributed cache solution like Redis would be needed, and cache
/// invalidation would need to propagate across network boundaries.
///
/// This test demonstrates:
/// - Cache population on first read
/// - Cache invalidation on write/delete
/// - Subsequent reads reflect the updated state
#[tokio::test]
async fn test_inprocess_cache_invalidation_with_shared_cache() {
    use common::create_shared_cache;

    let shared_storage = Arc::new(MemoryDataStore::new());
    let shared_cache = create_shared_cache();
    let store_id = setup_test_store(&shared_storage).await;

    // Step 1: Write initial tuple
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
    assert_eq!(
        status,
        StatusCode::OK,
        "Step 1: write initial tuple should succeed"
    );

    // Step 2: Check (this populates the cache)
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
    assert_eq!(status, StatusCode::OK, "Step 2: check should return OK");
    assert_eq!(
        response["allowed"], true,
        "Step 2: alice should have viewer permission (cache populated)"
    );

    // Step 3: Delete the tuple (should invalidate cache)
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
    assert_eq!(
        status,
        StatusCode::OK,
        "Step 3: delete tuple should succeed"
    );

    // Step 4: Check again (should see updated state, not stale cache)
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
    assert_eq!(status, StatusCode::OK, "Step 4: check should return OK");
    assert_eq!(
        response["allowed"], false,
        "Step 4: alice should NOT have viewer permission (cache invalidated after delete)"
    );
}

// =============================================================================
// Section 3: Operation Timeout and Health Check Tests
//
// These tests verify that operations complete within reasonable time bounds
// and that health/readiness endpoints work correctly.
// =============================================================================

// Timeout bounds for operations (generous for CI environments)
const CHECK_TIMEOUT_SECS: u64 = 5;
const BATCH_TIMEOUT_SECS: u64 = 10;
const BATCH_CHECK_SIZE: usize = 50;
const RAPID_REQUEST_COUNT: usize = 100;

/// Test: Check operation completes within timeout bounds.
///
/// Verifies that check operations complete within a reasonable time
/// and don't hang indefinitely.
///
/// **Note**: This test uses in-memory storage which completes quickly.
/// It validates that operations don't hang, but does not test actual
/// timeout error handling. For storage timeout error tests, implement
/// a mock DataStore that simulates slow responses.
#[tokio::test]
async fn test_check_operation_completes_within_timeout() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Setup: write test data
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
    assert_eq!(
        status,
        StatusCode::OK,
        "Setup: write test tuple should succeed"
    );

    // Test: operation should complete within timeout
    let result = timeout(Duration::from_secs(CHECK_TIMEOUT_SECS), async {
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
        "Check operation should complete within {CHECK_TIMEOUT_SECS}s timeout"
    );
    let (status, _) = result.expect("Timeout should not occur");
    assert_eq!(status, StatusCode::OK, "Check should return OK status");
}

/// Test: Batch check operations complete within timeout bounds.
///
/// Verifies that batch operations with multiple checks don't hang
/// and complete within reasonable time.
#[tokio::test]
async fn test_batch_operations_complete_within_timeout() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Setup: write test data
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
    assert_eq!(
        status,
        StatusCode::OK,
        "Setup: write test tuple should succeed"
    );

    // Test: batch check with BATCH_CHECK_SIZE items should complete within timeout
    let result = timeout(Duration::from_secs(BATCH_TIMEOUT_SECS), async {
        let app = create_test_app(&storage);
        post_json(
            app,
            &format!("/stores/{store_id}/batch-check"),
            serde_json::json!({
                "checks": (0..BATCH_CHECK_SIZE).map(|i| {
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

    assert!(
        result.is_ok(),
        "Batch check with {BATCH_CHECK_SIZE} items should complete within {BATCH_TIMEOUT_SECS}s"
    );
    let (status, _) = result.expect("Timeout should not occur");
    assert_eq!(
        status,
        StatusCode::OK,
        "Batch check should return OK status"
    );
}

/// Test: Health endpoint returns OK status.
///
/// Verifies the /health liveness probe endpoint works correctly.
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
                .expect("Request builder should succeed"),
        )
        .await
        .expect("Request should succeed");

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "Health endpoint should return 200 OK"
    );

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("Body should be readable");
    let json: serde_json::Value =
        serde_json::from_slice(&body).expect("Response should be valid JSON");
    assert_eq!(
        json["status"], "ok",
        "Health response should have status 'ok'"
    );
}

/// Test: Readiness endpoint validates storage connectivity.
///
/// Verifies the /ready readiness probe endpoint checks storage health.
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
                .expect("Request builder should succeed"),
        )
        .await
        .expect("Request should succeed");

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "Readiness endpoint should return 200 OK when storage is healthy"
    );

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("Body should be readable");
    let json: serde_json::Value =
        serde_json::from_slice(&body).expect("Response should be valid JSON");
    assert_eq!(
        json["status"], "ready",
        "Readiness response should have status 'ready'"
    );
    assert_eq!(
        json["checks"]["storage"], "ok",
        "Storage check should be 'ok'"
    );
}

/// Test: Rapid successive requests are handled without errors.
///
/// Verifies the system handles a burst of rapid concurrent requests
/// without failures (simulates traffic spikes during scaling events).
#[tokio::test]
async fn test_rapid_successive_requests_handled() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Setup: write test data
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
    assert_eq!(
        status,
        StatusCode::OK,
        "Setup: write test tuple should succeed"
    );

    // Test: fire RAPID_REQUEST_COUNT concurrent requests
    let success_count = Arc::new(AtomicU64::new(0));

    let handles: Vec<_> = (0..RAPID_REQUEST_COUNT)
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
        handle.await.expect("Task should not panic");
    }

    let successes = success_count.load(Ordering::SeqCst);
    assert_eq!(
        successes, RAPID_REQUEST_COUNT as u64,
        "All {RAPID_REQUEST_COUNT} rapid requests should succeed, got {successes}"
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

    // Test 404 - store not found (using valid ULID format)
    let nonexistent_store_id = ulid::Ulid::new().to_string();
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{}/check", nonexistent_store_id),
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

    // Test 404 - non-existent store (regardless of ID format)
    // OpenFGA returns 404 for any non-existent store, not 400 for invalid format
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores/invalid-store-id/check",
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

    // Test 400 - missing required 'name' field
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
