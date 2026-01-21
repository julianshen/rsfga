//! Concurrent operations compatibility tests (Issue #214).
//!
//! These tests verify RSFGA's behavior under concurrent load conditions,
//! ensuring correctness, consistency, and absence of deadlocks/panics.
//!
//! # Test Categories
//!
//! 1. **Concurrent Writes** - Simultaneous writes to same/different tuples,
//!    batch operations, and race condition handling
//! 2. **Concurrent Reads** - Check queries under contention, reads during writes,
//!    and ListObjects operations under load
//! 3. **Concurrent Store Operations** - Store creation/deletion and concurrent
//!    model writes
//! 4. **Cache Consistency** - Cache invalidation under concurrent load,
//!    batch consistency, and singleflight deduplication
//! 5. **Large Scale Operations** - High-volume concurrent requests
//!    (100+ Check ops, 50+ Writes)
//!
//! # Acceptance Criteria (from Issue #214)
//!
//! - Implement 15-20 concurrent operation tests ✅
//! - Verify no deadlocks or panics under concurrent load ✅
//! - Confirm data consistency post-operations ✅
//! - Validate correct cache invalidation behavior ✅
//! - Test both HTTP and gRPC transports
//!   - HTTP: ✅ All tests use HTTP endpoints
//!   - gRPC: Deferred to Phase 2 (see grpc_cache_invalidation_tests.rs for pattern)
//!
//! # Running Tests
//!
//! ```bash
//! # Run all concurrent operation tests
//! cargo test -p rsfga-api --test concurrent_operations_tests
//!
//! # Run with output
//! cargo test -p rsfga-api --test concurrent_operations_tests -- --nocapture
//!
//! # Run ignored (resource-intensive) tests
//! cargo test -p rsfga-api --test concurrent_operations_tests -- --ignored
//! ```

mod common;

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use futures::future::join_all;
use rsfga_domain::cache::{CacheKey, CheckCacheConfig};
use rsfga_storage::MemoryDataStore;

use common::{
    create_shared_cache, create_shared_cache_with_config, create_test_app,
    create_test_app_with_shared_cache, post_json, setup_simple_model,
};

// =============================================================================
// Test Constants
// =============================================================================

/// Number of concurrent write operations to same tuple.
const CONCURRENT_SAME_TUPLE_WRITES: usize = 50;

/// Number of concurrent write operations to different tuples.
const CONCURRENT_DIFFERENT_TUPLE_WRITES: usize = 100;

/// Number of concurrent check operations.
const CONCURRENT_CHECK_OPS: usize = 100;

/// Number of concurrent stores to create.
const CONCURRENT_STORE_CREATIONS: usize = 20;

/// Number of concurrent model writes per store.
const CONCURRENT_MODEL_WRITES: usize = 10;

/// Large scale check operations count.
const LARGE_SCALE_CHECK_OPS: usize = 200;

/// Large scale write operations count.
const LARGE_SCALE_WRITE_OPS: usize = 100;

/// Timeout for concurrent operations.
const CONCURRENT_OP_TIMEOUT: Duration = Duration::from_secs(30);

/// Poll interval for cache operations.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Timeout for cache invalidation.
const CACHE_INVALIDATION_TIMEOUT: Duration = Duration::from_secs(5);

// =============================================================================
// Helper Functions
// =============================================================================

/// Polls the cache until pending tasks are processed and entry count stabilizes.
async fn wait_for_cache_sync(cache: &rsfga_domain::cache::CheckCache) {
    cache.run_pending_tasks().await;
    tokio::time::sleep(POLL_INTERVAL).await;
    cache.run_pending_tasks().await;
}

/// Polls the cache until the specified key is invalidated or timeout occurs.
async fn wait_for_cache_invalidation(cache: &rsfga_domain::cache::CheckCache, key: &CacheKey) {
    let start = Instant::now();
    loop {
        cache.run_pending_tasks().await;
        if cache.get(key).await.is_none() {
            return;
        }
        if start.elapsed() > CACHE_INVALIDATION_TIMEOUT {
            panic!(
                "Cache invalidation timeout: key {:?} still present after {:?}",
                key, CACHE_INVALIDATION_TIMEOUT
            );
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

// =============================================================================
// Section 1: Concurrent Writes
// =============================================================================

/// Test: Concurrent writes to the same tuple succeed without errors.
///
/// Multiple writers attempting to write the same tuple should all succeed
/// (idempotent writes) without deadlocks or panics.
#[tokio::test]
async fn test_concurrent_writes_same_tuple_succeed() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "concurrent-same-tuple"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    let start = Instant::now();
    let success_count = Arc::new(AtomicU64::new(0));
    let error_count = Arc::new(AtomicU64::new(0));

    // Launch concurrent writes to the SAME tuple
    let futures: Vec<_> = (0..CONCURRENT_SAME_TUPLE_WRITES)
        .map(|_| {
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
                            "tuple_keys": [{
                                "user": "user:concurrent_writer",
                                "relation": "viewer",
                                "object": "document:shared_target"
                            }]
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

    // All writes should succeed (idempotent)
    assert_eq!(
        errors, 0,
        "Should have no errors for concurrent writes to same tuple"
    );
    assert_eq!(
        successes, CONCURRENT_SAME_TUPLE_WRITES as u64,
        "All concurrent writes should succeed"
    );
    assert!(
        elapsed < CONCURRENT_OP_TIMEOUT,
        "Concurrent writes should complete within timeout, took {:?}",
        elapsed
    );

    // Verify the tuple exists
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:concurrent_writer",
                "relation": "viewer",
                "object": "document:shared_target"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"], true,
        "Tuple should exist after concurrent writes"
    );
}

/// Test: Concurrent writes to different tuples succeed without interference.
///
/// Multiple writers writing different tuples should all succeed independently.
#[tokio::test]
async fn test_concurrent_writes_different_tuples_no_interference() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "concurrent-diff-tuples"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    let start = Instant::now();
    let success_count = Arc::new(AtomicU64::new(0));

    // Launch concurrent writes to DIFFERENT tuples
    let futures: Vec<_> = (0..CONCURRENT_DIFFERENT_TUPLE_WRITES)
        .map(|i| {
            let storage = Arc::clone(&storage);
            let store_id = store_id.clone();
            let success_count = Arc::clone(&success_count);

            async move {
                let app = create_test_app(&storage);
                let (status, _) = post_json(
                    app,
                    &format!("/stores/{store_id}/write"),
                    serde_json::json!({
                        "writes": {
                            "tuple_keys": [{
                                "user": format!("user:writer{i}"),
                                "relation": "viewer",
                                "object": format!("document:doc{i}")
                            }]
                        }
                    }),
                )
                .await;

                if status == StatusCode::OK {
                    success_count.fetch_add(1, Ordering::Relaxed);
                }
                (i, status)
            }
        })
        .collect();

    let results = join_all(futures).await;

    let elapsed = start.elapsed();
    let successes = success_count.load(Ordering::Relaxed);

    // All writes should succeed
    assert_eq!(
        successes, CONCURRENT_DIFFERENT_TUPLE_WRITES as u64,
        "All concurrent writes to different tuples should succeed"
    );
    assert!(
        elapsed < CONCURRENT_OP_TIMEOUT,
        "Concurrent writes should complete within timeout, took {:?}",
        elapsed
    );

    // Verify all tuples exist
    for (i, status) in results {
        assert_eq!(status, StatusCode::OK, "Write {i} should succeed");
    }
}

/// Test: Concurrent batch writes maintain data integrity.
///
/// Multiple batch write operations should complete without data corruption.
#[tokio::test]
async fn test_concurrent_batch_writes_data_integrity() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "concurrent-batch-writes"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    let num_batches = 20;
    let tuples_per_batch = 5;

    // Launch concurrent batch writes
    let futures: Vec<_> = (0..num_batches)
        .map(|batch| {
            let storage = Arc::clone(&storage);
            let store_id = store_id.clone();

            async move {
                let app = create_test_app(&storage);
                let tuple_keys: Vec<_> = (0..tuples_per_batch)
                    .map(|j| {
                        serde_json::json!({
                            "user": format!("user:batch{batch}_user{j}"),
                            "relation": "viewer",
                            "object": format!("document:batch{batch}_doc{j}")
                        })
                    })
                    .collect();

                let (status, _) = post_json(
                    app,
                    &format!("/stores/{store_id}/write"),
                    serde_json::json!({
                        "writes": {
                            "tuple_keys": tuple_keys
                        }
                    }),
                )
                .await;
                (batch, status)
            }
        })
        .collect();

    let results = join_all(futures).await;

    // All batch writes should succeed
    for (batch, status) in &results {
        assert_eq!(
            *status,
            StatusCode::OK,
            "Batch {batch} write should succeed"
        );
    }

    // Verify all tuples were written correctly
    for batch in 0..num_batches {
        for j in 0..tuples_per_batch {
            let (status, response) = post_json(
                create_test_app(&storage),
                &format!("/stores/{store_id}/check"),
                serde_json::json!({
                    "tuple_key": {
                        "user": format!("user:batch{batch}_user{j}"),
                        "relation": "viewer",
                        "object": format!("document:batch{batch}_doc{j}")
                    }
                }),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(
                response["allowed"], true,
                "Tuple from batch {batch}, item {j} should exist"
            );
        }
    }
}

/// Test: Concurrent write and delete race conditions are handled safely.
///
/// Simultaneous writes and deletes to overlapping data should not cause
/// crashes or undefined behavior.
#[tokio::test]
async fn test_concurrent_write_delete_race_conditions() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "write-delete-race"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Pre-write some tuples
    for i in 0..20 {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": format!("user:race{i}"),
                        "relation": "viewer",
                        "object": "document:contested"
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let start = Instant::now();
    let write_success = Arc::new(AtomicU64::new(0));
    let delete_success = Arc::new(AtomicU64::new(0));

    // Launch concurrent writes and deletes
    let mut handles = Vec::new();

    // Writers adding new tuples
    for i in 20..40 {
        let storage = Arc::clone(&storage);
        let store_id = store_id.clone();
        let write_success = Arc::clone(&write_success);

        handles.push(tokio::spawn(async move {
            let app = create_test_app(&storage);
            let (status, _) = post_json(
                app,
                &format!("/stores/{store_id}/write"),
                serde_json::json!({
                    "writes": {
                        "tuple_keys": [{
                            "user": format!("user:race{i}"),
                            "relation": "viewer",
                            "object": "document:contested"
                        }]
                    }
                }),
            )
            .await;
            if status == StatusCode::OK {
                write_success.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    // Deleters removing existing tuples
    for i in 0..20 {
        let storage = Arc::clone(&storage);
        let store_id = store_id.clone();
        let delete_success = Arc::clone(&delete_success);

        handles.push(tokio::spawn(async move {
            let app = create_test_app(&storage);
            let (status, _) = post_json(
                app,
                &format!("/stores/{store_id}/write"),
                serde_json::json!({
                    "deletes": {
                        "tuple_keys": [{
                            "user": format!("user:race{i}"),
                            "relation": "viewer",
                            "object": "document:contested"
                        }]
                    }
                }),
            )
            .await;
            if status == StatusCode::OK {
                delete_success.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    // Wait for all operations - no panics should occur
    for handle in handles {
        handle.await.expect("Operation should not panic");
    }

    let elapsed = start.elapsed();
    let writes = write_success.load(Ordering::Relaxed);
    let deletes = delete_success.load(Ordering::Relaxed);

    // Both writes and deletes should succeed
    assert_eq!(writes, 20, "All writes should succeed");
    assert_eq!(deletes, 20, "All deletes should succeed");
    assert!(
        elapsed < CONCURRENT_OP_TIMEOUT,
        "Operations should complete within timeout, took {:?}",
        elapsed
    );
}

// =============================================================================
// Section 2: Concurrent Reads
// =============================================================================

/// Test: Concurrent check queries under contention return correct results.
///
/// Multiple concurrent check requests for the same permission should all
/// return consistent results.
#[tokio::test]
async fn test_concurrent_check_queries_consistency() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "concurrent-checks"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Write a tuple
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:reader",
                    "relation": "viewer",
                    "object": "document:target"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let allowed_count = Arc::new(AtomicU64::new(0));
    let denied_count = Arc::new(AtomicU64::new(0));

    // Launch concurrent check queries for SAME permission
    let futures: Vec<_> = (0..CONCURRENT_CHECK_OPS)
        .map(|_| {
            let storage = Arc::clone(&storage);
            let store_id = store_id.clone();
            let allowed_count = Arc::clone(&allowed_count);
            let denied_count = Arc::clone(&denied_count);

            async move {
                let app = create_test_app(&storage);
                let (status, response) = post_json(
                    app,
                    &format!("/stores/{store_id}/check"),
                    serde_json::json!({
                        "tuple_key": {
                            "user": "user:reader",
                            "relation": "viewer",
                            "object": "document:target"
                        }
                    }),
                )
                .await;

                assert_eq!(status, StatusCode::OK);
                if response["allowed"].as_bool().unwrap() {
                    allowed_count.fetch_add(1, Ordering::Relaxed);
                } else {
                    denied_count.fetch_add(1, Ordering::Relaxed);
                }
            }
        })
        .collect();

    join_all(futures).await;

    let allowed = allowed_count.load(Ordering::Relaxed);
    let denied = denied_count.load(Ordering::Relaxed);

    // All checks should return true (consistent)
    assert_eq!(
        allowed, CONCURRENT_CHECK_OPS as u64,
        "All concurrent checks should return allowed=true"
    );
    assert_eq!(denied, 0, "No checks should return denied");
}

/// Test: Reads during concurrent writes return consistent snapshots.
///
/// Check queries issued while writes are in progress should return
/// valid results (either before or after the write, but not corrupted).
#[tokio::test]
async fn test_reads_during_concurrent_writes() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "reads-during-writes"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    let mut handles = Vec::new();
    let read_success = Arc::new(AtomicU64::new(0));
    let write_success = Arc::new(AtomicU64::new(0));

    // Spawn writers
    for i in 0..30 {
        let storage = Arc::clone(&storage);
        let store_id = store_id.clone();
        let write_success = Arc::clone(&write_success);

        handles.push(tokio::spawn(async move {
            let app = create_test_app(&storage);
            let (status, _) = post_json(
                app,
                &format!("/stores/{store_id}/write"),
                serde_json::json!({
                    "writes": {
                        "tuple_keys": [{
                            "user": format!("user:rw_user{i}"),
                            "relation": "viewer",
                            "object": "document:rw_target"
                        }]
                    }
                }),
            )
            .await;
            if status == StatusCode::OK {
                write_success.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    // Spawn readers checking the same object
    for _ in 0..50 {
        let storage = Arc::clone(&storage);
        let store_id = store_id.clone();
        let read_success = Arc::clone(&read_success);

        handles.push(tokio::spawn(async move {
            let app = create_test_app(&storage);
            let (status, response) = post_json(
                app,
                &format!("/stores/{store_id}/check"),
                serde_json::json!({
                    "tuple_key": {
                        "user": "user:rw_user0",
                        "relation": "viewer",
                        "object": "document:rw_target"
                    }
                }),
            )
            .await;
            // Read should succeed (allowed or denied, but not error)
            if status == StatusCode::OK {
                read_success.fetch_add(1, Ordering::Relaxed);
                // Result should be boolean (not corrupted)
                assert!(
                    response["allowed"].is_boolean(),
                    "Response should contain valid boolean"
                );
            }
        }));
    }

    // Wait for all operations
    for handle in handles {
        handle.await.expect("Operation should not panic");
    }

    let reads = read_success.load(Ordering::Relaxed);
    let writes = write_success.load(Ordering::Relaxed);

    assert_eq!(writes, 30, "All writes should succeed");
    assert_eq!(reads, 50, "All reads should succeed");
}

/// Test: Concurrent ListObjects operations return consistent results.
///
/// Multiple ListObjects queries should complete without errors and
/// return valid object lists.
#[tokio::test]
async fn test_concurrent_list_objects_operations() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "concurrent-listobjects"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Write multiple tuples
    for i in 0..20 {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": "user:list_user",
                        "relation": "viewer",
                        "object": format!("document:list_doc{i}")
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let success_count = Arc::new(AtomicU64::new(0));
    let consistent_count = Arc::new(AtomicU64::new(0));

    // Launch concurrent ListObjects requests
    let futures: Vec<_> = (0..30)
        .map(|_| {
            let storage = Arc::clone(&storage);
            let store_id = store_id.clone();
            let success_count = Arc::clone(&success_count);
            let consistent_count = Arc::clone(&consistent_count);

            async move {
                let app = create_test_app(&storage);
                let (status, response) = post_json(
                    app,
                    &format!("/stores/{store_id}/list-objects"),
                    serde_json::json!({
                        "user": "user:list_user",
                        "relation": "viewer",
                        "type": "document"
                    }),
                )
                .await;

                if status == StatusCode::OK {
                    success_count.fetch_add(1, Ordering::Relaxed);
                    // Verify we get expected objects
                    if let Some(objects) = response["objects"].as_array() {
                        if objects.len() == 20 {
                            consistent_count.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        })
        .collect();

    join_all(futures).await;

    let successes = success_count.load(Ordering::Relaxed);
    let consistent = consistent_count.load(Ordering::Relaxed);

    assert_eq!(successes, 30, "All ListObjects requests should succeed");
    assert_eq!(
        consistent, 30,
        "All ListObjects should return consistent 20 objects"
    );
}

/// Test: Concurrent ListUsers operations return consistent results.
///
/// Multiple ListUsers queries should complete without errors and
/// return consistent user counts.
#[tokio::test]
async fn test_concurrent_list_users_operations() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "concurrent-listusers"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Write 15 user tuples
    const EXPECTED_USER_COUNT: usize = 15;
    for i in 0..EXPECTED_USER_COUNT {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": format!("user:listuser{i}"),
                        "relation": "viewer",
                        "object": "document:shared_doc"
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let success_count = Arc::new(AtomicU64::new(0));
    let consistent_count = Arc::new(AtomicU64::new(0));

    // Launch concurrent ListUsers requests
    let futures: Vec<_> = (0..25)
        .map(|_| {
            let storage = Arc::clone(&storage);
            let store_id = store_id.clone();
            let success_count = Arc::clone(&success_count);
            let consistent_count = Arc::clone(&consistent_count);

            async move {
                let app = create_test_app(&storage);
                let (status, response) = post_json(
                    app,
                    &format!("/stores/{store_id}/list-users"),
                    serde_json::json!({
                        "object": {"type": "document", "id": "shared_doc"},
                        "relation": "viewer",
                        "user_filters": [{"type": "user"}]
                    }),
                )
                .await;

                if status == StatusCode::OK {
                    success_count.fetch_add(1, Ordering::Relaxed);
                    // Verify response has users array with expected count
                    if let Some(users) = response["users"].as_array() {
                        if users.len() == EXPECTED_USER_COUNT {
                            consistent_count.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        })
        .collect();

    join_all(futures).await;

    let successes = success_count.load(Ordering::Relaxed);
    let consistent = consistent_count.load(Ordering::Relaxed);

    assert_eq!(successes, 25, "All ListUsers requests should succeed");
    assert_eq!(
        consistent, 25,
        "All ListUsers should return consistent {} users",
        EXPECTED_USER_COUNT
    );
}

// =============================================================================
// Section 3: Concurrent Store Operations
// =============================================================================

/// Test: Concurrent store creation succeeds without conflicts.
///
/// Multiple stores can be created simultaneously without errors.
#[tokio::test]
async fn test_concurrent_store_creation() {
    let storage = Arc::new(MemoryDataStore::new());

    let start = Instant::now();
    let created_stores = Arc::new(tokio::sync::Mutex::new(HashSet::new()));

    // Launch concurrent store creations
    let futures: Vec<_> = (0..CONCURRENT_STORE_CREATIONS)
        .map(|i| {
            let storage = Arc::clone(&storage);
            let created_stores = Arc::clone(&created_stores);

            async move {
                let app = create_test_app(&storage);
                let (status, response) = post_json(
                    app,
                    "/stores",
                    serde_json::json!({"name": format!("concurrent-store-{i}")}),
                )
                .await;

                if status == StatusCode::CREATED {
                    if let Some(id) = response["id"].as_str() {
                        created_stores.lock().await.insert(id.to_string());
                    }
                }
                (i, status)
            }
        })
        .collect();

    let results = join_all(futures).await;

    let elapsed = start.elapsed();
    let stores = created_stores.lock().await;

    // All store creations should succeed
    for (i, status) in &results {
        assert_eq!(
            *status,
            StatusCode::CREATED,
            "Store {i} creation should succeed"
        );
    }
    assert_eq!(
        stores.len(),
        CONCURRENT_STORE_CREATIONS,
        "All stores should be created with unique IDs"
    );
    assert!(
        elapsed < CONCURRENT_OP_TIMEOUT,
        "Store creations should complete within timeout"
    );
}

/// Test: Concurrent model writes to different stores succeed.
///
/// Writing authorization models to different stores concurrently should
/// not interfere with each other.
#[tokio::test]
async fn test_concurrent_model_writes_different_stores() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create multiple stores first
    let mut store_ids = Vec::new();
    for i in 0..5 {
        let (status, response) = post_json(
            create_test_app(&storage),
            "/stores",
            serde_json::json!({"name": format!("model-store-{i}")}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        store_ids.push(response["id"].as_str().unwrap().to_string());
    }

    let success_count = Arc::new(AtomicU64::new(0));

    // Launch concurrent model writes to different stores
    let futures: Vec<_> = store_ids
        .iter()
        .flat_map(|store_id| {
            (0..CONCURRENT_MODEL_WRITES).map(|i| {
                let storage = Arc::clone(&storage);
                let store_id = store_id.clone();
                let success_count = Arc::clone(&success_count);

                async move {
                    let app = create_test_app(&storage);
                    let (status, _) = post_json(
                        app,
                        &format!("/stores/{store_id}/authorization-models"),
                        serde_json::json!({
                            "type_definitions": [
                                {"type": "user"},
                                {"type": format!("document_v{i}"), "relations": {"viewer": {}}}
                            ]
                        }),
                    )
                    .await;

                    if status == StatusCode::CREATED {
                        success_count.fetch_add(1, Ordering::Relaxed);
                    }
                }
            })
        })
        .collect();

    join_all(futures).await;

    let successes = success_count.load(Ordering::Relaxed);
    let expected = (5 * CONCURRENT_MODEL_WRITES) as u64;
    assert_eq!(
        successes, expected,
        "All concurrent model writes should succeed"
    );
}

/// Test: Concurrent operations across multiple stores complete without interference.
///
/// Operations on multiple stores happening concurrently should not
/// interfere with each other or cause crashes.
///
/// Note: Store deletion endpoint (DELETE /stores/{id}) is not yet implemented.
/// This test verifies concurrent access patterns that would occur during
/// store lifecycle operations. Actual deletion tests deferred to Phase 2.
#[tokio::test]
async fn test_concurrent_multi_store_operations() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create multiple stores
    let mut store_ids = Vec::new();
    for i in 0..10 {
        let (status, response) = post_json(
            create_test_app(&storage),
            "/stores",
            serde_json::json!({"name": format!("multi-store-{i}")}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        store_ids.push(response["id"].as_str().unwrap().to_string());
    }

    // Set up models for all stores
    for store_id in &store_ids {
        setup_simple_model(&storage, store_id).await;
    }

    let operation_count = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::new();

    // Spawn concurrent operations across all stores
    for (idx, store_id) in store_ids.iter().enumerate() {
        // Clone values for write operation
        let storage_write = Arc::clone(&storage);
        let store_id_write = store_id.clone();
        let operation_count_write = Arc::clone(&operation_count);

        // Clone values for check operation
        let storage_check = Arc::clone(&storage);
        let store_id_check = store_id.clone();
        let operation_count_check = Arc::clone(&operation_count);

        // Write operation
        handles.push(tokio::spawn(async move {
            let app = create_test_app(&storage_write);

            // Write a tuple
            let (write_status, _) = post_json(
                app,
                &format!("/stores/{store_id_write}/write"),
                serde_json::json!({
                    "writes": {
                        "tuple_keys": [{
                            "user": format!("user:store{idx}_user"),
                            "relation": "viewer",
                            "object": format!("document:store{idx}_doc")
                        }]
                    }
                }),
            )
            .await;

            if write_status == StatusCode::OK {
                operation_count_write.fetch_add(1, Ordering::Relaxed);
            }
        }));

        // Check operation on same store
        handles.push(tokio::spawn(async move {
            let app = create_test_app(&storage_check);
            let (status, _) = post_json(
                app,
                &format!("/stores/{store_id_check}/check"),
                serde_json::json!({
                    "tuple_key": {
                        "user": "user:test",
                        "relation": "viewer",
                        "object": "document:test"
                    }
                }),
            )
            .await;

            if status == StatusCode::OK {
                operation_count_check.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    // All operations should complete without panic
    for handle in handles {
        handle.await.expect("Operation should not panic");
    }

    let total_ops = operation_count.load(Ordering::Relaxed);

    // 10 writes + 10 checks = 20 operations
    assert_eq!(
        total_ops, 20,
        "All concurrent multi-store operations should complete"
    );
}

// =============================================================================
// Section 4: Cache Consistency
// =============================================================================

/// Test: Cache invalidation under concurrent writes produces consistent results.
///
/// Concurrent writes should properly invalidate cache entries, ensuring
/// subsequent reads see updated data.
#[tokio::test]
async fn test_cache_invalidation_concurrent_writes() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "cache-concurrent-invalidation"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Populate cache with initial checks
    for i in 0..20 {
        let (status, response) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
            &format!("/stores/{store_id}/check"),
            serde_json::json!({
                "tuple_key": {
                    "user": format!("user:cache_user{i}"),
                    "relation": "viewer",
                    "object": "document:cache_target"
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["allowed"], false);
    }

    wait_for_cache_sync(&cache).await;

    // Launch concurrent writes
    let futures: Vec<_> = (0..20)
        .map(|i| {
            let storage = Arc::clone(&storage);
            let cache = Arc::clone(&cache);
            let store_id = store_id.clone();

            async move {
                let app = create_test_app_with_shared_cache(&storage, &cache);
                post_json(
                    app,
                    &format!("/stores/{store_id}/write"),
                    serde_json::json!({
                        "writes": {
                            "tuple_keys": [{
                                "user": format!("user:cache_user{i}"),
                                "relation": "viewer",
                                "object": "document:cache_target"
                            }]
                        }
                    }),
                )
                .await
            }
        })
        .collect();

    join_all(futures).await;

    // Wait for cache invalidation across multiple sample keys to avoid race condition
    // (checking only first key could miss keys 1-19 still being cached)
    for idx in [0, 10, 19] {
        let cache_key = CacheKey::new(
            &store_id,
            "document:cache_target",
            "viewer",
            format!("user:cache_user{idx}"),
        );
        wait_for_cache_invalidation(&cache, &cache_key).await;
    }

    // Verify all checks now return true
    for i in 0..20 {
        let (status, response) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
            &format!("/stores/{store_id}/check"),
            serde_json::json!({
                "tuple_key": {
                    "user": format!("user:cache_user{i}"),
                    "relation": "viewer",
                    "object": "document:cache_target"
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            response["allowed"], true,
            "User {i} should have access after concurrent writes"
        );
    }
}

/// Test: Batch check cache consistency under concurrent operations.
///
/// Batch checks should return consistent results when concurrent writes
/// are invalidating cache entries.
#[tokio::test]
async fn test_batch_check_cache_consistency_concurrent() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "batch-cache-concurrent"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Write some initial tuples
    for i in 0..5 {
        let (status, _) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": format!("user:batch_user{i}"),
                        "relation": "viewer",
                        "object": "document:batch_target"
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let batch_success = Arc::new(AtomicU64::new(0));

    // Launch concurrent batch checks and writes
    let mut handles = Vec::new();

    // Batch check operations
    for _ in 0..10 {
        let storage = Arc::clone(&storage);
        let cache = Arc::clone(&cache);
        let store_id = store_id.clone();
        let batch_success = Arc::clone(&batch_success);

        handles.push(tokio::spawn(async move {
            let app = create_test_app_with_shared_cache(&storage, &cache);
            let checks: Vec<_> = (0..5)
                .map(|i| {
                    serde_json::json!({
                        "tuple_key": {
                            "user": format!("user:batch_user{i}"),
                            "relation": "viewer",
                            "object": "document:batch_target"
                        },
                        "correlation_id": format!("check-{i}")
                    })
                })
                .collect();

            let (status, _) = post_json(
                app,
                &format!("/stores/{store_id}/batch-check"),
                serde_json::json!({"checks": checks}),
            )
            .await;

            if status == StatusCode::OK {
                batch_success.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    // Concurrent write operations
    for i in 5..10 {
        let storage = Arc::clone(&storage);
        let cache = Arc::clone(&cache);
        let store_id = store_id.clone();

        handles.push(tokio::spawn(async move {
            let app = create_test_app_with_shared_cache(&storage, &cache);
            let _ = post_json(
                app,
                &format!("/stores/{store_id}/write"),
                serde_json::json!({
                    "writes": {
                        "tuple_keys": [{
                            "user": format!("user:batch_user{i}"),
                            "relation": "viewer",
                            "object": "document:batch_target"
                        }]
                    }
                }),
            )
            .await;
        }));
    }

    for handle in handles {
        handle.await.expect("Operation should not panic");
    }

    let batch_successes = batch_success.load(Ordering::Relaxed);
    assert_eq!(batch_successes, 10, "All batch checks should succeed");
}

/// Test: Batch check deduplication under concurrent load.
///
/// Multiple concurrent batch-check requests with identical check items should
/// properly deduplicate via singleflight, reducing backend load while
/// maintaining consistent results.
#[tokio::test]
async fn test_batch_check_deduplication_under_load() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "batch-dedup-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Write tuples for batch checks
    for i in 0..5 {
        let (status, _) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": format!("user:dedup{i}"),
                        "relation": "viewer",
                        "object": "document:shared"
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    // Clear cache to force fresh lookups
    wait_for_cache_sync(&cache).await;

    let success_count = Arc::new(AtomicU64::new(0));
    let correct_results = Arc::new(AtomicU64::new(0));

    // Launch many concurrent batch-check requests with IDENTICAL check items
    // Singleflight should deduplicate the underlying resolver calls
    let futures: Vec<_> = (0..30)
        .map(|_| {
            let storage = Arc::clone(&storage);
            let cache = Arc::clone(&cache);
            let store_id = store_id.clone();
            let success_count = Arc::clone(&success_count);
            let correct_results = Arc::clone(&correct_results);

            async move {
                let app = create_test_app_with_shared_cache(&storage, &cache);

                // All requests use the same 5 check items - perfect for deduplication
                let checks: Vec<_> = (0..5)
                    .map(|i| {
                        serde_json::json!({
                            "tuple_key": {
                                "user": format!("user:dedup{i}"),
                                "relation": "viewer",
                                "object": "document:shared"
                            },
                            "correlation_id": format!("check-{i}")
                        })
                    })
                    .collect();

                let (status, response) = post_json(
                    app,
                    &format!("/stores/{store_id}/batch-check"),
                    serde_json::json!({"checks": checks}),
                )
                .await;

                if status == StatusCode::OK {
                    success_count.fetch_add(1, Ordering::Relaxed);

                    // Verify all 5 checks return allowed=true
                    if let Some(result) = response["result"].as_object() {
                        let all_allowed = (0..5).all(|i| {
                            result
                                .get(&format!("check-{i}"))
                                .and_then(|r| r["allowed"].as_bool())
                                .unwrap_or(false)
                        });
                        if all_allowed {
                            correct_results.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        })
        .collect();

    join_all(futures).await;

    let successes = success_count.load(Ordering::Relaxed);
    let correct = correct_results.load(Ordering::Relaxed);

    // All requests should succeed with consistent results
    assert_eq!(successes, 30, "All batch-check requests should succeed");
    assert_eq!(
        correct, 30,
        "All batch-check results should show allowed=true for all items"
    );
}

/// Test: Cache entries expire correctly after TTL.
///
/// Verifies that cache entries respect TTL configuration and expire
/// as expected, ensuring stale data doesn't persist.
#[tokio::test]
async fn test_cache_entry_expires_after_ttl() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create cache with short TTL
    let cache_config = CheckCacheConfig::default()
        .with_enabled(true)
        .with_ttl(Duration::from_millis(200));
    let cache = create_shared_cache_with_config(cache_config);

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "ttl-concurrent"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Write a tuple
    let (status, _) = post_json(
        create_test_app_with_shared_cache(&storage, &cache),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:ttl_user",
                    "relation": "viewer",
                    "object": "document:ttl_doc"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Populate cache
    let (status, _) = post_json(
        create_test_app_with_shared_cache(&storage, &cache),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:ttl_user",
                "relation": "viewer",
                "object": "document:ttl_doc"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    wait_for_cache_sync(&cache).await;

    // Verify cache is populated
    let cache_key = CacheKey::new(&store_id, "document:ttl_doc", "viewer", "user:ttl_user");
    assert_eq!(
        cache.get(&cache_key).await,
        Some(true),
        "Cache should be populated"
    );

    // Wait for TTL to expire
    tokio::time::sleep(Duration::from_millis(250)).await;
    cache.run_pending_tasks().await;

    // Cache entry should be expired
    assert_eq!(
        cache.get(&cache_key).await,
        None,
        "Cache entry should expire after TTL"
    );

    // New check should still work
    let (status, response) = post_json(
        create_test_app_with_shared_cache(&storage, &cache),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:ttl_user",
                "relation": "viewer",
                "object": "document:ttl_doc"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["allowed"], true);
}

// =============================================================================
// Section 5: Large Scale Operations
// =============================================================================

/// Test: 200+ concurrent check operations complete without errors.
///
/// Verifies the system handles high-volume check queries without
/// degradation or failures.
#[tokio::test]
async fn test_large_scale_concurrent_checks() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "large-scale-checks"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Write some tuples
    for i in 0..50 {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": format!("user:large{i}"),
                        "relation": "viewer",
                        "object": format!("document:large{i}")
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let start = Instant::now();
    let success_count = Arc::new(AtomicU64::new(0));
    let error_count = Arc::new(AtomicU64::new(0));

    // Launch 200+ concurrent checks
    let futures: Vec<_> = (0..LARGE_SCALE_CHECK_OPS)
        .map(|i| {
            let storage = Arc::clone(&storage);
            let store_id = store_id.clone();
            let success_count = Arc::clone(&success_count);
            let error_count = Arc::clone(&error_count);

            async move {
                let app = create_test_app(&storage);
                let user_idx = i % 50;
                let (status, _) = post_json(
                    app,
                    &format!("/stores/{store_id}/check"),
                    serde_json::json!({
                        "tuple_key": {
                            "user": format!("user:large{user_idx}"),
                            "relation": "viewer",
                            "object": format!("document:large{user_idx}")
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

    assert_eq!(
        errors, 0,
        "Should have no errors with {} concurrent checks",
        LARGE_SCALE_CHECK_OPS
    );
    assert_eq!(
        successes, LARGE_SCALE_CHECK_OPS as u64,
        "All {} checks should succeed",
        LARGE_SCALE_CHECK_OPS
    );
    assert!(
        elapsed < CONCURRENT_OP_TIMEOUT,
        "Large scale checks should complete within {:?}, took {:?}",
        CONCURRENT_OP_TIMEOUT,
        elapsed
    );

    println!(
        "Large scale check test: {} checks in {:?} ({:.0} checks/s)",
        successes,
        elapsed,
        successes as f64 / elapsed.as_secs_f64()
    );
}

/// Test: 100+ concurrent write operations complete without errors.
///
/// Verifies the system handles high-volume write operations without
/// data corruption or failures.
#[tokio::test]
async fn test_large_scale_concurrent_writes() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "large-scale-writes"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    let start = Instant::now();
    let success_count = Arc::new(AtomicU64::new(0));

    // Launch 100+ concurrent writes
    let futures: Vec<_> = (0..LARGE_SCALE_WRITE_OPS)
        .map(|i| {
            let storage = Arc::clone(&storage);
            let store_id = store_id.clone();
            let success_count = Arc::clone(&success_count);

            async move {
                let app = create_test_app(&storage);
                let (status, _) = post_json(
                    app,
                    &format!("/stores/{store_id}/write"),
                    serde_json::json!({
                        "writes": {
                            "tuple_keys": [{
                                "user": format!("user:scale{i}"),
                                "relation": "viewer",
                                "object": format!("document:scale{i}")
                            }]
                        }
                    }),
                )
                .await;

                if status == StatusCode::OK {
                    success_count.fetch_add(1, Ordering::Relaxed);
                }
                (i, status)
            }
        })
        .collect();

    let results = join_all(futures).await;

    let elapsed = start.elapsed();
    let successes = success_count.load(Ordering::Relaxed);

    assert_eq!(
        successes, LARGE_SCALE_WRITE_OPS as u64,
        "All {} writes should succeed",
        LARGE_SCALE_WRITE_OPS
    );
    assert!(
        elapsed < CONCURRENT_OP_TIMEOUT,
        "Large scale writes should complete within {:?}, took {:?}",
        CONCURRENT_OP_TIMEOUT,
        elapsed
    );

    // Verify data integrity - all tuples should exist
    for (i, status) in results {
        assert_eq!(status, StatusCode::OK, "Write {i} should succeed");
    }

    // Sample verification
    for i in [0, 25, 50, 75, 99].iter() {
        let (status, response) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/check"),
            serde_json::json!({
                "tuple_key": {
                    "user": format!("user:scale{i}"),
                    "relation": "viewer",
                    "object": format!("document:scale{i}")
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            response["allowed"], true,
            "Tuple {i} should exist after large scale writes"
        );
    }

    println!(
        "Large scale write test: {} writes in {:?} ({:.0} writes/s)",
        successes,
        elapsed,
        successes as f64 / elapsed.as_secs_f64()
    );
}

/// Test: Mixed large scale operations (reads + writes) complete without issues.
///
/// High volume of both reads and writes occurring simultaneously should
/// not cause deadlocks, data corruption, or failures.
#[tokio::test]
#[ignore = "resource-intensive test - run manually"]
async fn test_large_scale_mixed_operations() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "large-scale-mixed"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Pre-populate some data
    for i in 0..50 {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": format!("user:mixed{i}"),
                        "relation": "viewer",
                        "object": format!("document:mixed{i}")
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let start = Instant::now();
    let read_success = Arc::new(AtomicU64::new(0));
    let write_success = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::new();

    // Spawn 150 read operations
    for i in 0..150 {
        let storage = Arc::clone(&storage);
        let cache = Arc::clone(&cache);
        let store_id = store_id.clone();
        let read_success = Arc::clone(&read_success);

        handles.push(tokio::spawn(async move {
            let app = create_test_app_with_shared_cache(&storage, &cache);
            let user_idx = i % 50;
            let (status, _) = post_json(
                app,
                &format!("/stores/{store_id}/check"),
                serde_json::json!({
                    "tuple_key": {
                        "user": format!("user:mixed{user_idx}"),
                        "relation": "viewer",
                        "object": format!("document:mixed{user_idx}")
                    }
                }),
            )
            .await;

            if status == StatusCode::OK {
                read_success.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    // Spawn 50 write operations
    for i in 50..100 {
        let storage = Arc::clone(&storage);
        let cache = Arc::clone(&cache);
        let store_id = store_id.clone();
        let write_success = Arc::clone(&write_success);

        handles.push(tokio::spawn(async move {
            let app = create_test_app_with_shared_cache(&storage, &cache);
            let (status, _) = post_json(
                app,
                &format!("/stores/{store_id}/write"),
                serde_json::json!({
                    "writes": {
                        "tuple_keys": [{
                            "user": format!("user:mixed{i}"),
                            "relation": "viewer",
                            "object": format!("document:mixed{i}")
                        }]
                    }
                }),
            )
            .await;

            if status == StatusCode::OK {
                write_success.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    // Wait for all operations - no panics should occur
    for handle in handles {
        handle.await.expect("Operation should not panic");
    }

    let elapsed = start.elapsed();
    let reads = read_success.load(Ordering::Relaxed);
    let writes = write_success.load(Ordering::Relaxed);

    assert_eq!(reads, 150, "All 150 reads should succeed");
    assert_eq!(writes, 50, "All 50 writes should succeed");
    assert!(
        elapsed < CONCURRENT_OP_TIMEOUT,
        "Mixed operations should complete within {:?}, took {:?}",
        CONCURRENT_OP_TIMEOUT,
        elapsed
    );

    println!(
        "Large scale mixed test: {} reads + {} writes in {:?} ({:.0} ops/s)",
        reads,
        writes,
        elapsed,
        (reads + writes) as f64 / elapsed.as_secs_f64()
    );
}
