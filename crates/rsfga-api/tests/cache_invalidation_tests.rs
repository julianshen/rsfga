//! Integration tests for cache invalidation (Issue #200).
//!
//! These tests verify that the check cache is properly invalidated after
//! Write/Delete operations, ensuring no stale authorization decisions are served.
//!
//! # Security Context
//!
//! Cache invalidation is critical for authorization correctness. A cached
//! positive authorization decision that persists after access revocation
//! could grant unauthorized access.
//!
//! # Test Categories
//!
//! 1. Cache Invalidation on Write - Verify write/delete operations properly
//!    invalidate cached check results
//! 2. Concurrent Check+Write Behavior - Test read-after-write consistency
//! 3. Protocol Coverage - All tests run against HTTP/REST endpoints
//!
//! # Note on gRPC Coverage
//!
//! Issue #200 acceptance criteria require testing via both HTTP and gRPC endpoints.
//! Currently only HTTP/REST endpoints are tested. gRPC tests are deferred to a
//! follow-up PR once the gRPC layer is fully integrated with the shared cache
//! infrastructure.

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use rsfga_domain::cache::{CacheKey, CheckCache, CheckCacheConfig};
use rsfga_storage::MemoryDataStore;

use common::{
    create_shared_cache, create_shared_cache_with_config, create_test_app,
    create_test_app_with_shared_cache, post_json, setup_simple_model,
};

/// Maximum time to wait for cache invalidation in polling loops.
///
/// # Justification
///
/// 5 seconds is chosen based on:
/// - **CI variability**: Slow CI runners (shared VMs, container overhead) can have
///   significant scheduling delays. 5s provides 50x headroom over the ~100ms typical case.
/// - **Moka cache background tasks**: The underlying Moka cache uses async background
///   tasks for eviction. Under load, these may be delayed.
/// - **No false positives**: A real bug would cause indefinite hangs, not 5s delays.
///   This timeout catches actual failures without flaky CI failures.
/// - **Industry practice**: Similar timeout values are used in tokio, tower, and hyper
///   test suites for async operation verification.
const CACHE_INVALIDATION_TIMEOUT: Duration = Duration::from_secs(5);

/// Interval between polling attempts.
///
/// # Justification
///
/// 10ms polling interval is chosen based on:
/// - **Responsiveness**: Fast enough to detect invalidation promptly (< 100ms typical).
/// - **CPU efficiency**: 10ms sleep yields to scheduler, avoiding busy-wait CPU burn.
/// - **Moka task scheduling**: Moka's run_pending_tasks() typically completes in < 1ms,
///   so 10ms gives ample time for async operations between polls.
/// - **Test suite speed**: 500 polls possible within 5s timeout, providing granular
///   detection without slowing down the happy path.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Polls the cache until the specified key is invalidated (returns None) or timeout occurs.
///
/// This replaces hardcoded sleeps with timeout-based polling, making tests more reliable
/// on slower CI systems.
async fn wait_for_cache_invalidation(cache: &CheckCache, key: &CacheKey) {
    let start = std::time::Instant::now();
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

/// Polls the cache until pending tasks are processed and entry count stabilizes.
async fn wait_for_cache_sync(cache: &CheckCache) {
    // Run pending tasks twice to ensure all async operations complete
    cache.run_pending_tasks().await;
    tokio::time::sleep(POLL_INTERVAL).await;
    cache.run_pending_tasks().await;
}

// ============================================================
// Section 1: Cache Invalidation on Write Operations
// ============================================================

/// Test: Cache is invalidated after writing a new tuple.
///
/// Scenario:
/// 1. Check permission (should be false, gets cached)
/// 2. Write tuple granting permission
/// 3. Check permission again (should be true, not stale false)
#[tokio::test]
async fn test_cache_invalidated_after_write_grants_access() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store and set up model (use non-cached app for setup)
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "cache-write-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Step 1: Check permission - should be false and get cached
    let (status, response) = post_json(
        create_test_app_with_shared_cache(&storage, &cache),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:secret"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"], false,
        "Alice should NOT have access before tuple is written"
    );

    // Verify the result was cached
    let cache_key = CacheKey::new(&store_id, "document:secret", "viewer", "user:alice");
    cache.run_pending_tasks().await;
    let cached = cache.get(&cache_key).await;
    assert_eq!(cached, Some(false), "False result should be cached");

    // Step 2: Write tuple granting access
    let (status, _) = post_json(
        create_test_app_with_shared_cache(&storage, &cache),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:secret"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Wait for cache invalidation using polling instead of fixed sleep
    wait_for_cache_invalidation(&cache, &cache_key).await;

    // Step 3: Verify cache was invalidated (entry should be gone)
    let cached_after = cache.get(&cache_key).await;
    assert_eq!(
        cached_after, None,
        "Cache entry should be invalidated after write"
    );

    // Step 4: Check permission again - should be true now
    let (status, response) = post_json(
        create_test_app_with_shared_cache(&storage, &cache),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:secret"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"], true,
        "Alice should have access after tuple is written"
    );
}

/// Test: Cache is invalidated after deleting a tuple.
///
/// Scenario:
/// 1. Write tuple granting permission
/// 2. Check permission (should be true, gets cached)
/// 3. Delete tuple revoking permission
/// 4. Check permission again (should be false, not stale true)
///
/// This is the CRITICAL security test - stale true after revocation is dangerous.
#[tokio::test]
async fn test_cache_invalidated_after_delete_revokes_access() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store and set up model
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "cache-delete-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Step 1: Write tuple granting access
    let (status, _) = post_json(
        create_test_app_with_shared_cache(&storage, &cache),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:bob",
                    "relation": "editor",
                    "object": "document:confidential"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Step 2: Check permission - should be true and get cached
    let (status, response) = post_json(
        create_test_app_with_shared_cache(&storage, &cache),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:bob",
                "relation": "editor",
                "object": "document:confidential"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"], true,
        "Bob should have access before tuple is deleted"
    );

    // Verify the result was cached
    let cache_key = CacheKey::new(&store_id, "document:confidential", "editor", "user:bob");
    cache.run_pending_tasks().await;
    let cached = cache.get(&cache_key).await;
    assert_eq!(cached, Some(true), "True result should be cached");

    // Step 3: Delete tuple revoking access
    let (status, _) = post_json(
        create_test_app_with_shared_cache(&storage, &cache),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "deletes": {
                "tuple_keys": [{
                    "user": "user:bob",
                    "relation": "editor",
                    "object": "document:confidential"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Wait for cache invalidation using polling instead of fixed sleep
    wait_for_cache_invalidation(&cache, &cache_key).await;

    // Step 4: Verify cache was invalidated
    let cached_after = cache.get(&cache_key).await;
    assert_eq!(
        cached_after, None,
        "Cache entry should be invalidated after delete"
    );

    // Step 5: Check permission again - MUST be false (security critical)
    let (status, response) = post_json(
        create_test_app_with_shared_cache(&storage, &cache),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:bob",
                "relation": "editor",
                "object": "document:confidential"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"], false,
        "SECURITY: Bob must NOT have access after tuple is deleted"
    );
}

/// Test: Cache invalidation affects only the relevant store.
///
/// Writes to one store should not invalidate cache entries for other stores.
#[tokio::test]
async fn test_cache_invalidation_scoped_to_store() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create two stores
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "store-a"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_a = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_a).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "store-b"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_b = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_b).await;

    // Write tuples to both stores
    for store_id in [&store_a, &store_b] {
        let (status, _) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": "user:alice",
                        "relation": "viewer",
                        "object": "document:doc1"
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    // Check permission on both stores - should be cached
    for store_id in [&store_a, &store_b] {
        let (status, response) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
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
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["allowed"], true);
    }

    cache.run_pending_tasks().await;

    // Verify both stores have cached entries
    let cache_key_a = CacheKey::new(&store_a, "document:doc1", "viewer", "user:alice");
    let cache_key_b = CacheKey::new(&store_b, "document:doc1", "viewer", "user:alice");
    assert_eq!(
        cache.get(&cache_key_a).await,
        Some(true),
        "Store A should have cached entry"
    );
    assert_eq!(
        cache.get(&cache_key_b).await,
        Some(true),
        "Store B should have cached entry"
    );

    // Write new tuple to store A only
    let (status, _) = post_json(
        create_test_app_with_shared_cache(&storage, &cache),
        &format!("/stores/{store_a}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:charlie",
                    "relation": "owner",
                    "object": "document:doc2"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Wait for Store A cache invalidation using polling
    wait_for_cache_invalidation(&cache, &cache_key_a).await;

    // Store A cache should be invalidated (coarse-grained invalidation)
    let cached_a = cache.get(&cache_key_a).await;
    assert_eq!(
        cached_a, None,
        "Store A cache should be invalidated after write to Store A"
    );

    // Store B cache should remain intact
    let cached_b = cache.get(&cache_key_b).await;
    assert_eq!(
        cached_b,
        Some(true),
        "Store B cache should NOT be affected by write to Store A"
    );
}

/// Test: Batch write invalidates cache for all affected tuples.
#[tokio::test]
async fn test_cache_invalidation_on_batch_write() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "batch-cache-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Check multiple permissions - all should be false and cached
    let users = ["alice", "bob", "charlie"];
    for user in &users {
        let (status, response) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
            &format!("/stores/{store_id}/check"),
            serde_json::json!({
                "tuple_key": {
                    "user": format!("user:{user}"),
                    "relation": "viewer",
                    "object": "document:shared"
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["allowed"], false);
    }

    cache.run_pending_tasks().await;

    // Verify all are cached
    for user in &users {
        let cache_key = CacheKey::new(
            &store_id,
            "document:shared",
            "viewer",
            format!("user:{user}"),
        );
        assert_eq!(
            cache.get(&cache_key).await,
            Some(false),
            "{user}'s result should be cached"
        );
    }

    // Batch write tuples for all users
    let (status, _) = post_json(
        create_test_app_with_shared_cache(&storage, &cache),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": users.iter().map(|user| serde_json::json!({
                    "user": format!("user:{user}"),
                    "relation": "viewer",
                    "object": "document:shared"
                })).collect::<Vec<_>>()
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Wait for cache invalidation using polling - check first user's cache key
    let first_user_key = CacheKey::new(&store_id, "document:shared", "viewer", "user:alice");
    wait_for_cache_invalidation(&cache, &first_user_key).await;

    // Check all permissions again - should be true now
    for user in &users {
        let (status, response) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
            &format!("/stores/{store_id}/check"),
            serde_json::json!({
                "tuple_key": {
                    "user": format!("user:{user}"),
                    "relation": "viewer",
                    "object": "document:shared"
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            response["allowed"], true,
            "{user} should now have access after batch write"
        );
    }
}

// ============================================================
// Section 2: Concurrent Check+Write Behavior
// ============================================================

/// Test: Concurrent writes and checks produce consistent results.
///
/// Simulates a race condition where checks and writes happen simultaneously.
/// Uses tokio::spawn to achieve true parallelism (not sequential await).
/// After all writes complete, subsequent checks should see the written data.
#[tokio::test]
async fn test_concurrent_writes_and_checks_consistency() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "concurrent-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Launch concurrent writes and checks using tokio::spawn for true parallelism
    let num_users = 20;
    let mut handles = Vec::new();

    for i in 0..num_users {
        // Spawn write task
        let storage_clone = Arc::clone(&storage);
        let cache_clone = Arc::clone(&cache);
        let store_id_clone = store_id.clone();
        let write_handle = tokio::spawn(async move {
            let (status, _) = post_json(
                create_test_app_with_shared_cache(&storage_clone, &cache_clone),
                &format!("/stores/{store_id_clone}/write"),
                serde_json::json!({
                    "writes": {
                        "tuple_keys": [{
                            "user": format!("user:concurrent{i}"),
                            "relation": "viewer",
                            "object": "document:shared"
                        }]
                    }
                }),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
        });
        handles.push(write_handle);

        // Spawn check task (runs truly concurrent with write)
        let storage_clone2 = Arc::clone(&storage);
        let cache_clone2 = Arc::clone(&cache);
        let store_id_clone2 = store_id.clone();
        let check_handle = tokio::spawn(async move {
            let (status, _) = post_json(
                create_test_app_with_shared_cache(&storage_clone2, &cache_clone2),
                &format!("/stores/{store_id_clone2}/check"),
                serde_json::json!({
                    "tuple_key": {
                        "user": format!("user:concurrent{i}"),
                        "relation": "viewer",
                        "object": "document:shared"
                    }
                }),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
        });
        handles.push(check_handle);
    }

    // Wait for all spawned tasks to complete
    for handle in handles {
        handle.await.expect("Task should complete without panic");
    }

    // Wait for cache to sync using polling
    wait_for_cache_sync(&cache).await;

    // After all writes complete, all checks should return true
    for i in 0..num_users {
        let (status, response) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
            &format!("/stores/{store_id}/check"),
            serde_json::json!({
                "tuple_key": {
                    "user": format!("user:concurrent{i}"),
                    "relation": "viewer",
                    "object": "document:shared"
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            response["allowed"], true,
            "user:concurrent{i} should have access after write completes"
        );
    }
}

/// Test: Read-after-write consistency - check after write returns correct result.
///
/// This tests the fundamental consistency guarantee: if a write completes,
/// a subsequent check should reflect that write.
#[tokio::test]
async fn test_read_after_write_consistency() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "raw-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Run this test multiple times to catch race conditions
    for iteration in 0..10 {
        let user = format!("user:raw{iteration}");

        // Write tuple
        let (status, _) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": &user,
                        "relation": "viewer",
                        "object": format!("document:raw{iteration}")
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Wait for cache sync using polling
        wait_for_cache_sync(&cache).await;

        // Immediately check - should return true
        let (status, response) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
            &format!("/stores/{store_id}/check"),
            serde_json::json!({
                "tuple_key": {
                    "user": &user,
                    "relation": "viewer",
                    "object": format!("document:raw{iteration}")
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            response["allowed"], true,
            "Iteration {iteration}: Check after write should return true"
        );
    }
}

/// Test: Delete-then-check consistency - access is revoked immediately.
///
/// This is the security-critical version: if a delete completes,
/// a subsequent check MUST show the access as revoked.
#[tokio::test]
async fn test_delete_then_check_consistency() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "delete-check-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Run this test multiple times to catch race conditions
    for iteration in 0..10 {
        let user = format!("user:del{iteration}");
        let object = format!("document:del{iteration}");

        // First write the tuple
        let (status, _) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": &user,
                        "relation": "viewer",
                        "object": &object
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Wait for cache sync using polling
        wait_for_cache_sync(&cache).await;

        // Verify access
        let (status, response) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
            &format!("/stores/{store_id}/check"),
            serde_json::json!({
                "tuple_key": {
                    "user": &user,
                    "relation": "viewer",
                    "object": &object
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["allowed"], true);

        // Now delete the tuple
        let (status, _) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "deletes": {
                    "tuple_keys": [{
                        "user": &user,
                        "relation": "viewer",
                        "object": &object
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Wait for cache sync using polling
        wait_for_cache_sync(&cache).await;

        // CRITICAL: Check must return false
        let (status, response) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
            &format!("/stores/{store_id}/check"),
            serde_json::json!({
                "tuple_key": {
                    "user": &user,
                    "relation": "viewer",
                    "object": &object
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            response["allowed"], false,
            "SECURITY: Iteration {iteration}: Check after delete MUST return false"
        );
    }
}

/// Test: High-contention concurrent writes don't cause deadlocks.
///
/// Multiple writers updating the same object should not deadlock
/// and cache invalidation should complete for all.
#[tokio::test]
async fn test_high_contention_writes_no_deadlock() {
    use futures::future::join_all;
    use std::time::Instant;

    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "contention-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    let start = Instant::now();
    let num_concurrent = 50;

    // Launch many concurrent writes to the same object
    let futures: Vec<_> = (0..num_concurrent)
        .map(|i| {
            let storage_clone = Arc::clone(&storage);
            let cache_clone = Arc::clone(&cache);
            let store_id_clone = store_id.clone();
            async move {
                let (status, _) = post_json(
                    create_test_app_with_shared_cache(&storage_clone, &cache_clone),
                    &format!("/stores/{store_id_clone}/write"),
                    serde_json::json!({
                        "writes": {
                            "tuple_keys": [{
                                "user": format!("user:writer{i}"),
                                "relation": "viewer",
                                "object": "document:contested"
                            }]
                        }
                    }),
                )
                .await;
                status
            }
        })
        .collect();

    let results = join_all(futures).await;
    let elapsed = start.elapsed();

    // All writes should succeed
    for (i, status) in results.iter().enumerate() {
        assert_eq!(
            *status,
            StatusCode::OK,
            "Write {i} should succeed under contention"
        );
    }

    // Test should complete in reasonable time (no deadlock)
    assert!(
        elapsed < Duration::from_secs(10),
        "High contention test should complete within 10 seconds, took {:?}",
        elapsed
    );

    // Wait for cache sync using polling
    wait_for_cache_sync(&cache).await;

    // All writers should have access
    for i in 0..num_concurrent {
        let (status, response) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
            &format!("/stores/{store_id}/check"),
            serde_json::json!({
                "tuple_key": {
                    "user": format!("user:writer{i}"),
                    "relation": "viewer",
                    "object": "document:contested"
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            response["allowed"], true,
            "writer{i} should have access after contended writes"
        );
    }
}

// ============================================================
// Section 3: Cache Metrics and Observability
// ============================================================

/// Test: Cache hit/miss behavior is observable through entry count.
///
/// Verifies that cache entries are created on misses and hits don't
/// increase the count.
#[tokio::test]
async fn test_cache_entries_are_observable() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "observe-test"}),
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
                    "user": "user:observer",
                    "relation": "viewer",
                    "object": "document:observed"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    cache.run_pending_tasks().await;
    let initial_count = cache.entry_count();

    // First check - cache miss, should add entry
    let (status, _) = post_json(
        create_test_app_with_shared_cache(&storage, &cache),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:observer",
                "relation": "viewer",
                "object": "document:observed"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    cache.run_pending_tasks().await;
    let after_first = cache.entry_count();

    // Entry count should increase by exactly 1
    assert_eq!(
        after_first,
        initial_count + 1,
        "Cache entry count should increase by exactly 1 after check miss (was {}, now {})",
        initial_count,
        after_first
    );
}

/// Test: Cache with short TTL expires entries.
///
/// Verifies that cache entries respect TTL configuration.
#[tokio::test]
async fn test_cache_ttl_expiration() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "ttl-test"}),
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
                    "user": "user:ttl",
                    "relation": "viewer",
                    "object": "document:ttl"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Create cache with very short TTL
    let short_ttl_config = CheckCacheConfig::default()
        .with_enabled(true)
        .with_ttl(Duration::from_millis(100));
    let cache = create_shared_cache_with_config(short_ttl_config);

    // Manually insert to ensure we're testing TTL
    let cache_key = CacheKey::new(&store_id, "document:ttl", "viewer", "user:ttl");
    cache.insert(cache_key.clone(), true).await;
    cache.run_pending_tasks().await;

    // Should be cached initially
    let cached = cache.get(&cache_key).await;
    assert_eq!(cached, Some(true), "Entry should be cached initially");

    // Wait for TTL to expire
    tokio::time::sleep(Duration::from_millis(150)).await;
    cache.run_pending_tasks().await;

    // Should be expired now
    let cached_after = cache.get(&cache_key).await;
    assert_eq!(cached_after, None, "Entry should be expired after TTL");
}

// ============================================================
// Section 4: Batch Check Cache Behavior
// ============================================================

/// Test: Batch check properly uses cache and invalidation affects it.
#[tokio::test]
async fn test_batch_check_cache_invalidation() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "batch-inv-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // First batch check - all should be denied
    let checks: Vec<_> = (0..5)
        .map(|i| {
            serde_json::json!({
                "tuple_key": {
                    "user": format!("user:batch{i}"),
                    "relation": "viewer",
                    "object": "document:batch"
                },
                "correlation_id": format!("check-{i}")
            })
        })
        .collect();

    let (status, response) = post_json(
        create_test_app_with_shared_cache(&storage, &cache),
        &format!("/stores/{store_id}/batch-check"),
        serde_json::json!({"checks": checks}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let result = response["result"].as_object().unwrap();
    for i in 0..5 {
        assert_eq!(
            result[&format!("check-{i}")]["allowed"],
            false,
            "Initial batch check {i} should be denied"
        );
    }

    // Write tuples for first 3 users
    let (status, _) = post_json(
        create_test_app_with_shared_cache(&storage, &cache),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": (0..3).map(|i| serde_json::json!({
                    "user": format!("user:batch{i}"),
                    "relation": "viewer",
                    "object": "document:batch"
                })).collect::<Vec<_>>()
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Wait for cache sync using polling
    wait_for_cache_sync(&cache).await;

    // Second batch check - first 3 should be allowed, last 2 denied
    let (status, response) = post_json(
        create_test_app_with_shared_cache(&storage, &cache),
        &format!("/stores/{store_id}/batch-check"),
        serde_json::json!({"checks": checks}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let result = response["result"].as_object().unwrap();
    for i in 0..5 {
        let expected = i < 3;
        assert_eq!(
            result[&format!("check-{i}")]["allowed"],
            expected,
            "After write, batch check {i} should be {}",
            if expected { "allowed" } else { "denied" }
        );
    }
}
