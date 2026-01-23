//! Stress tests for RSFGA API.
//!
//! These tests verify system behavior under high load and stress conditions.
//!
//! **Test Categories**:
//! - Concurrent connection handling
//! - Sustained load behavior
//! - Graceful degradation under overload
//! - Recovery after failures
//!
//! **Running Stress Tests**:
//! - Basic tests: `cargo test -p rsfga-api --test stress_tests`
//! - Full load tests require external tools (k6, locust)
//!
//! **Note**: Some tests are marked `#[ignore]` as they require significant
//! resources or external infrastructure.

mod common;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use axum::http::StatusCode;
use futures::future::join_all;
use rsfga_storage::{DataStore, MemoryDataStore, StoredAuthorizationModel};

use common::{
    create_test_app, post_json, OVERLOAD_TEST_CONCURRENT_REQUESTS, STRESS_TEST_CONCURRENT_REQUESTS,
    STRESS_TEST_TIMEOUT, SUSTAINED_LOAD_DURATION, SUSTAINED_LOAD_WORKERS,
    WRITE_STRESS_CONCURRENT_OPS,
};

// =============================================================================
// Section 4: Stress Tests
// =============================================================================

/// Test: Server handles 1000 concurrent connections
///
/// Verifies the system can handle a high number of concurrent requests
/// without errors or excessive latency.
///
/// Note: This test is resource-intensive and may be flaky in CI environments.
/// Run manually with: `cargo test -p rsfga-api --test stress_tests -- --ignored`
#[tokio::test]
#[ignore = "resource-intensive load test - run manually"]
async fn test_server_handles_1000_concurrent_requests() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create a store and add some test data
    let store_id = {
        let (status, response) = post_json(
            create_test_app(&storage),
            "/stores",
            serde_json::json!({"name": "stress-test"}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        response["id"].as_str().unwrap().to_string()
    };

    // Write some tuples
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [
                    {"user": "user:alice", "relation": "viewer", "object": "document:doc1"}
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Track metrics
    let success_count = Arc::new(AtomicU64::new(0));
    let error_count = Arc::new(AtomicU64::new(0));

    let start = Instant::now();

    // Launch concurrent check requests
    let futures: Vec<_> = (0..STRESS_TEST_CONCURRENT_REQUESTS)
        .map(|i| {
            let storage = Arc::clone(&storage);
            let store_id = store_id.clone();
            let success_count = Arc::clone(&success_count);
            let error_count = Arc::clone(&error_count);

            async move {
                let app = create_test_app(&storage);
                let (status, _) = post_json(
                    app,
                    &format!("/stores/{store_id}/check"),
                    serde_json::json!({
                        "tuple_key": {
                            "user": if i % 2 == 0 { "user:alice" } else { "user:bob" },
                            "relation": "viewer",
                            "object": "document:doc1"
                        }
                    }),
                )
                .await;

                if status == StatusCode::OK {
                    success_count.fetch_add(1, Ordering::Relaxed);
                } else {
                    error_count.fetch_add(1, Ordering::Relaxed);
                }
            }
        })
        .collect();

    join_all(futures).await;

    let elapsed = start.elapsed();
    let successes = success_count.load(Ordering::Relaxed);
    let errors = error_count.load(Ordering::Relaxed);

    // Verify results
    assert_eq!(
        errors, 0,
        "Should have no errors with {STRESS_TEST_CONCURRENT_REQUESTS} concurrent requests"
    );
    assert_eq!(
        successes, STRESS_TEST_CONCURRENT_REQUESTS as u64,
        "All {STRESS_TEST_CONCURRENT_REQUESTS} requests should succeed"
    );

    // Performance check: should complete in reasonable time
    // Note: This is a soft assertion - actual performance depends on hardware
    assert!(
        elapsed < STRESS_TEST_TIMEOUT,
        "{STRESS_TEST_CONCURRENT_REQUESTS} concurrent requests should complete within {STRESS_TEST_TIMEOUT:?}, took {elapsed:?}"
    );

    println!("Stress test completed: {successes} successes, {errors} errors in {elapsed:?}");
}

/// Test: No memory leaks under sustained load (basic check)
///
/// Runs a sustained load test and verifies the system remains stable.
/// Note: Full memory leak detection requires external tools (valgrind, heaptrack).
///
/// This test runs for 5 seconds with 10 workers and may be flaky in CI.
/// Run manually with: `cargo test -p rsfga-api --test stress_tests -- --ignored`
#[tokio::test]
#[ignore = "resource-intensive load test - run manually"]
async fn test_sustained_load_stability() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create store
    let store_id = {
        let (status, response) = post_json(
            create_test_app(&storage),
            "/stores",
            serde_json::json!({"name": "sustained-load-test"}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        response["id"].as_str().unwrap().to_string()
    };

    // Write test data
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [
                    {"user": "user:alice", "relation": "viewer", "object": "document:doc1"}
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Run sustained load
    let duration = SUSTAINED_LOAD_DURATION;
    let start = Instant::now();
    let request_count = Arc::new(AtomicU64::new(0));
    let error_count = Arc::new(AtomicU64::new(0));

    // Spawn multiple worker tasks
    let handles: Vec<_> = (0..SUSTAINED_LOAD_WORKERS)
        .map(|_| {
            let storage = Arc::clone(&storage);
            let store_id = store_id.clone();
            let request_count = Arc::clone(&request_count);
            let error_count = Arc::clone(&error_count);

            tokio::spawn(async move {
                while Instant::now().duration_since(start) < duration {
                    let app = create_test_app(&storage);
                    let (status, _) = post_json(
                        app,
                        &format!("/stores/{store_id}/check"),
                        serde_json::json!({
                            "tuple_key": {
                                "user": "user:alice",
                                "relation": "viewer",
                                "object": "document:doc1"
                            }
                        }),
                    )
                    .await;

                    request_count.fetch_add(1, Ordering::Relaxed);
                    if status != StatusCode::OK {
                        error_count.fetch_add(1, Ordering::Relaxed);
                    }
                }
            })
        })
        .collect();

    // Wait for all workers
    for handle in handles {
        handle.await.unwrap();
    }

    let total_requests = request_count.load(Ordering::Relaxed);
    let total_errors = error_count.load(Ordering::Relaxed);
    let elapsed = start.elapsed();
    let throughput = total_requests as f64 / elapsed.as_secs_f64();

    // Verify no errors during sustained load
    assert_eq!(
        total_errors, 0,
        "Should have no errors during sustained load"
    );

    // Verify reasonable throughput maintained
    assert!(
        total_requests > 100,
        "Should process significant number of requests: got {total_requests}"
    );

    println!(
        "Sustained load test: {total_requests} requests in {elapsed:?} ({throughput:.0} req/s), {total_errors} errors"
    );
}

/// Test: Graceful degradation under overload
///
/// Verifies the system degrades gracefully when overloaded rather than crashing.
/// Tests that errors are appropriate (429 Too Many Requests, etc.) when overloaded.
///
/// This test fires 5000 concurrent requests and may be flaky in CI.
/// Run manually with: `cargo test -p rsfga-api --test stress_tests -- --ignored`
#[tokio::test]
#[ignore = "resource-intensive load test - run manually"]
async fn test_graceful_degradation_under_overload() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create store
    let store_id = {
        let (status, response) = post_json(
            create_test_app(&storage),
            "/stores",
            serde_json::json!({"name": "overload-test"}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        response["id"].as_str().unwrap().to_string()
    };

    // Write test data
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [
                    {"user": "user:alice", "relation": "viewer", "object": "document:doc1"}
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Spawn extreme concurrent load
    let success_count = Arc::new(AtomicU64::new(0));
    let client_error_count = Arc::new(AtomicU64::new(0)); // 4xx
    let server_error_count = Arc::new(AtomicU64::new(0)); // 5xx

    let futures: Vec<_> = (0..OVERLOAD_TEST_CONCURRENT_REQUESTS)
        .map(|_| {
            let storage = Arc::clone(&storage);
            let store_id = store_id.clone();
            let success_count = Arc::clone(&success_count);
            let client_error_count = Arc::clone(&client_error_count);
            let server_error_count = Arc::clone(&server_error_count);

            async move {
                let app = create_test_app(&storage);
                let (status, _) = post_json(
                    app,
                    &format!("/stores/{store_id}/check"),
                    serde_json::json!({
                        "tuple_key": {
                            "user": "user:alice",
                            "relation": "viewer",
                            "object": "document:doc1"
                        }
                    }),
                )
                .await;

                if status.is_success() {
                    success_count.fetch_add(1, Ordering::Relaxed);
                } else if status.is_client_error() {
                    client_error_count.fetch_add(1, Ordering::Relaxed);
                } else if status.is_server_error() {
                    server_error_count.fetch_add(1, Ordering::Relaxed);
                }
            }
        })
        .collect();

    join_all(futures).await;

    let successes = success_count.load(Ordering::Relaxed);
    let client_errors = client_error_count.load(Ordering::Relaxed);
    let server_errors = server_error_count.load(Ordering::Relaxed);

    // Key assertion: NO server errors (no crashes, panics, or 5xx)
    assert_eq!(
        server_errors, 0,
        "Should have no server errors (5xx) under overload"
    );

    // We expect mostly successes for in-memory storage
    // Note: With rate limiting enabled, some 429s would be acceptable
    println!(
        "Overload test: {successes} successes, {client_errors} client errors, {server_errors} server errors"
    );

    // Verify the system processed most requests
    assert!(
        successes + client_errors == OVERLOAD_TEST_CONCURRENT_REQUESTS as u64,
        "All requests should complete (success or client error)"
    );
}

/// Test: Recovery after database failure (PostgreSQL)
///
/// Verifies the system recovers gracefully when the database becomes unavailable
/// and then returns.
///
/// Note: This test requires PostgreSQL infrastructure.
#[tokio::test]
#[ignore = "requires running PostgreSQL and ability to simulate failure"]
async fn test_recovery_after_database_failure() {
    // This test would:
    // 1. Create a PostgresDataStore
    // 2. Perform successful operations
    // 3. Stop/pause PostgreSQL (simulating failure)
    // 4. Verify operations fail gracefully (appropriate errors, no panics)
    // 5. Restart PostgreSQL
    // 6. Verify operations succeed again
    // 7. Verify no data corruption
    //
    // Implementation requires testcontainers or similar infrastructure.
    todo!("Implement with testcontainers when PostgreSQL infrastructure is ready");
}

/// Test: High write throughput under concurrent load
///
/// Verifies the system handles high write throughput correctly.
#[tokio::test]
async fn test_high_write_throughput() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create store
    let store_id = {
        let (status, response) = post_json(
            create_test_app(&storage),
            "/stores",
            serde_json::json!({"name": "write-stress-test"}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        response["id"].as_str().unwrap().to_string()
    };

    // Set up authorization model (required for tuple validation)
    let model_json = r#"{
        "type_definitions": [
            {"type": "user"},
            {"type": "document", "relations": {"viewer": {}, "editor": {}, "owner": {}}}
        ]
    }"#;
    let model =
        StoredAuthorizationModel::new(ulid::Ulid::new().to_string(), &store_id, "1.1", model_json);
    storage.write_authorization_model(model).await.unwrap();

    let start = Instant::now();
    let success_count = Arc::new(AtomicU64::new(0));
    let error_count = Arc::new(AtomicU64::new(0));

    // Launch concurrent write operations
    let futures: Vec<_> = (0..WRITE_STRESS_CONCURRENT_OPS)
        .map(|i| {
            let storage = Arc::clone(&storage);
            let store_id = store_id.clone();
            let success_count = Arc::clone(&success_count);
            let error_count = Arc::clone(&error_count);

            async move {
                let app = create_test_app(&storage);
                let (status, _) = post_json(
                    app,
                    &format!("/stores/{store_id}/write"),
                    serde_json::json!({
                        "writes": {
                            "tuple_keys": [
                                {
                                    "user": format!("user:user{}", i),
                                    "relation": "viewer",
                                    "object": format!("document:doc{}", i)
                                }
                            ]
                        }
                    }),
                )
                .await;

                if status == StatusCode::OK {
                    success_count.fetch_add(1, Ordering::Relaxed);
                } else {
                    error_count.fetch_add(1, Ordering::Relaxed);
                }
            }
        })
        .collect();

    join_all(futures).await;

    let elapsed = start.elapsed();
    let successes = success_count.load(Ordering::Relaxed);
    let errors = error_count.load(Ordering::Relaxed);

    assert_eq!(errors, 0, "Should have no write errors");
    assert_eq!(
        successes, WRITE_STRESS_CONCURRENT_OPS as u64,
        "All writes should succeed"
    );

    println!(
        "Write stress test: {} writes in {:?} ({:.0} writes/s)",
        successes,
        elapsed,
        successes as f64 / elapsed.as_secs_f64()
    );
}
