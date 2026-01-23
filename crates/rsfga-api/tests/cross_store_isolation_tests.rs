//! Cross-store isolation integration tests (Issue #202).
//!
//! These tests verify that data is properly isolated between stores in a multi-tenant
//! environment. Store isolation bugs can cause data leakage between tenants, which is
//! a critical security vulnerability.
//!
//! # Test Categories
//!
//! 1. **Data Isolation** - Tuple/object/user separation between stores
//! 2. **Cache Isolation** - Separate cache namespaces and independent invalidation
//! 3. **Model Isolation** - Authorization models don't cross-contaminate
//! 4. **Lifecycle Isolation** - Deletion and pagination token boundaries
//! 5. **Protocol Coverage** - Both HTTP and gRPC endpoints
//!
//! # Security Context
//!
//! In multi-tenant authorization systems, store isolation is critical for security.
//! Each store represents a tenant's authorization data, and leakage between stores
//! could grant unauthorized access across tenant boundaries.

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use tonic::Request;

use rsfga_api::grpc::OpenFgaGrpcService;
use rsfga_api::proto::openfga::v1::open_fga_service_server::OpenFgaService;
use rsfga_api::proto::openfga::v1::*;
use rsfga_domain::cache::{CacheKey, CheckCache};
use rsfga_storage::{
    DataStore, MemoryDataStore, PaginationOptions, StoredAuthorizationModel, StoredTuple,
};

use common::{
    create_shared_cache, create_test_app, create_test_app_with_shared_cache, post_json,
    setup_simple_model,
};

// =============================================================================
// Constants
// =============================================================================

/// Timeout for cache invalidation polling loops.
const CACHE_INVALIDATION_TIMEOUT: Duration = Duration::from_secs(5);

/// Polling interval for cache checks.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Number of stores for concurrent isolation tests.
const NUM_CONCURRENT_STORES: usize = 10;

/// Number of operations per store in concurrent tests.
const OPS_PER_STORE: usize = 20;

/// Global timeout for concurrent test to prevent hangs.
const CONCURRENT_TEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Simple authorization model JSON for gRPC tests.
const SIMPLE_MODEL_JSON: &str = r#"{
    "type_definitions": [
        {"type": "user"},
        {"type": "document", "relations": {"viewer": {}, "editor": {}, "owner": {}}}
    ]
}"#;

// =============================================================================
// Test Helpers
// =============================================================================

/// Polls the cache until the specified key is invalidated (returns None) or timeout occurs.
///
/// The timeout is checked after each cache probe but before sleeping, ensuring we don't
/// wait indefinitely. Note: actual elapsed time may slightly exceed the timeout by up to
/// one poll interval plus cache operation latency.
async fn wait_for_cache_invalidation(cache: &CheckCache, key: &CacheKey) {
    let start = std::time::Instant::now();
    loop {
        cache.run_pending_tasks().await;
        if cache.get(key).await.is_none() {
            return;
        }
        // Check timeout after cache probe, before sleeping
        if start.elapsed() > CACHE_INVALIDATION_TIMEOUT {
            panic!(
                "Cache invalidation timeout: key {:?} still present after {:?}",
                key, CACHE_INVALIDATION_TIMEOUT
            );
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Polls the cache until pending tasks are processed.
async fn wait_for_cache_sync(cache: &CheckCache) {
    cache.run_pending_tasks().await;
    tokio::time::sleep(POLL_INTERVAL).await;
    cache.run_pending_tasks().await;
}

/// Creates a gRPC service with the given storage.
fn test_grpc_service(storage: Arc<MemoryDataStore>) -> OpenFgaGrpcService<MemoryDataStore> {
    OpenFgaGrpcService::new(storage)
}

/// Creates a more complete authorization model for testing.
fn complete_model_json() -> &'static str {
    r#"{
        "type_definitions": [
            {"type": "user"},
            {"type": "group", "relations": {"member": {}}},
            {"type": "team", "relations": {"member": {}}},
            {"type": "folder", "relations": {"viewer": {}, "editor": {}, "owner": {}}},
            {"type": "document", "relations": {"viewer": {}, "editor": {}, "owner": {}}}
        ]
    }"#
}

/// Sets up a complete authorization model for a store and validates persistence.
async fn setup_complete_model(storage: &MemoryDataStore, store_id: &str) -> String {
    let model = StoredAuthorizationModel::new(
        ulid::Ulid::new().to_string(),
        store_id,
        "1.1",
        complete_model_json().to_string(),
    );
    let model_id = model.id.clone();
    storage.write_authorization_model(model).await.unwrap();

    // Validate model was persisted correctly
    let retrieved = storage
        .get_authorization_model(store_id, &model_id)
        .await
        .expect("Model should be retrievable after write");
    assert_eq!(
        retrieved.id, model_id,
        "Retrieved model ID should match written model"
    );

    model_id
}

// =============================================================================
// Section 1: Data Isolation Tests
// =============================================================================

/// Test: Tuples written to one store are not visible in another store.
///
/// This is the fundamental data isolation test - verifying that tuples
/// are scoped to their store and don't leak across store boundaries.
#[tokio::test]
async fn test_tuples_isolated_between_stores() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create two independent stores
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "tenant-alpha"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_alpha = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_alpha).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "tenant-beta"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_beta = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_beta).await;

    // Write tuple to store Alpha only
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_alpha}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:secret-alpha"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Verify tuple exists in store Alpha
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_alpha}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:secret-alpha"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"], true,
        "Alice should have access in store Alpha"
    );

    // Verify tuple does NOT exist in store Beta (isolation)
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_beta}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:secret-alpha"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"], false,
        "ISOLATION: Alice should NOT have access in store Beta"
    );
}

/// Test: Same user can have different permissions in different stores.
///
/// Verifies that the same user identity can have completely independent
/// permissions across different stores.
#[tokio::test]
async fn test_same_user_different_permissions_per_store() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create two stores
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "store-x"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_x = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_x).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "store-y"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_y = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_y).await;

    // Alice is viewer in store X
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_x}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:shared"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Alice is editor in store Y (different permission)
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_y}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:alice",
                    "relation": "editor",
                    "object": "document:shared"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Verify Alice is viewer in store X
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_x}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:shared"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["allowed"], true);

    // Verify Alice is NOT editor in store X
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_x}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "editor",
                "object": "document:shared"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["allowed"], false);

    // Verify Alice is editor in store Y
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_y}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "editor",
                "object": "document:shared"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["allowed"], true);

    // Verify Alice is NOT viewer in store Y
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_y}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:shared"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["allowed"], false);
}

/// Test: Read API returns only tuples from the requested store.
///
/// Verifies that the Read API properly filters tuples by store.
#[tokio::test]
async fn test_read_api_returns_only_store_tuples() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create two stores
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "store-read-1"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_1 = response["id"].as_str().unwrap().to_string();

    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "store-read-2"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_2 = response["id"].as_str().unwrap().to_string();

    // Write different tuples to each store
    for i in 0..5 {
        storage
            .write_tuple(
                &store_1,
                StoredTuple::new(
                    "document",
                    format!("store1-doc{i}"),
                    "viewer",
                    "user",
                    "alice",
                    None,
                ),
            )
            .await
            .unwrap();
    }

    for i in 0..3 {
        storage
            .write_tuple(
                &store_2,
                StoredTuple::new(
                    "document",
                    format!("store2-doc{i}"),
                    "editor",
                    "user",
                    "bob",
                    None,
                ),
            )
            .await
            .unwrap();
    }

    // Read from store 1 - should get exactly 5 tuples
    let tuples_1 = storage
        .read_tuples(&store_1, &Default::default())
        .await
        .unwrap();
    assert_eq!(
        tuples_1.len(),
        5,
        "Store 1 should have exactly 5 tuples, got {}",
        tuples_1.len()
    );
    for tuple in &tuples_1 {
        assert!(
            tuple.object_id.starts_with("store1-"),
            "Store 1 tuple should have store1 prefix"
        );
    }

    // Read from store 2 - should get exactly 3 tuples
    let tuples_2 = storage
        .read_tuples(&store_2, &Default::default())
        .await
        .unwrap();
    assert_eq!(
        tuples_2.len(),
        3,
        "Store 2 should have exactly 3 tuples, got {}",
        tuples_2.len()
    );
    for tuple in &tuples_2 {
        assert!(
            tuple.object_id.starts_with("store2-"),
            "Store 2 tuple should have store2 prefix"
        );
    }
}

/// Test: Batch check respects store boundaries.
///
/// Verifies that batch check operations are properly scoped to the requested store.
#[tokio::test]
async fn test_batch_check_respects_store_boundaries() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create two stores
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "batch-store-a"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_a = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_a).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "batch-store-b"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_b = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_b).await;

    // Write tuple to store A only
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_a}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:batch-test"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Batch check in store A - should find the tuple
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_a}/batch-check"),
        serde_json::json!({
            "checks": [{
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:batch-test"
                },
                "correlation_id": "check-a"
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["result"]["check-a"]["allowed"], true,
        "Batch check should find tuple in store A"
    );

    // Batch check in store B - should NOT find the tuple
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_b}/batch-check"),
        serde_json::json!({
            "checks": [{
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:batch-test"
                },
                "correlation_id": "check-b"
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["result"]["check-b"]["allowed"], false,
        "ISOLATION: Batch check should NOT find tuple in store B"
    );
}

// =============================================================================
// Section 2: Cache Isolation Tests
// =============================================================================

/// Test: Cache entries are namespaced by store.
///
/// Verifies that cache keys include store_id, preventing cache pollution
/// between stores.
#[tokio::test]
async fn test_cache_entries_namespaced_by_store() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create two stores
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "cache-store-1"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_1 = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_1).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "cache-store-2"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_2 = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_2).await;

    // Write tuple to store 1 only
    let (status, _) = post_json(
        create_test_app_with_shared_cache(&storage, &cache),
        &format!("/stores/{store_1}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:cached"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Check in store 1 - should be true and cached
    let (status, response) = post_json(
        create_test_app_with_shared_cache(&storage, &cache),
        &format!("/stores/{store_1}/check"),
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

    // Check in store 2 - should be false and cached
    let (status, response) = post_json(
        create_test_app_with_shared_cache(&storage, &cache),
        &format!("/stores/{store_2}/check"),
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
    assert_eq!(response["allowed"], false);

    // Verify both are cached with different values
    wait_for_cache_sync(&cache).await;

    let cache_key_1 = CacheKey::new(&store_1, "document:cached", "viewer", "user:alice");
    let cache_key_2 = CacheKey::new(&store_2, "document:cached", "viewer", "user:alice");

    let cached_1 = cache.get(&cache_key_1).await;
    let cached_2 = cache.get(&cache_key_2).await;

    assert_eq!(cached_1, Some(true), "Store 1 cache should have true value");
    assert_eq!(
        cached_2,
        Some(false),
        "Store 2 cache should have false value"
    );
}

/// Test: Cache invalidation in one store doesn't affect other stores.
///
/// Verifies that write operations to one store only invalidate that store's
/// cache entries, leaving other stores' cache intact.
#[tokio::test]
async fn test_cache_invalidation_isolated_per_store() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create two stores
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "inv-store-a"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_a = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_a).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "inv-store-b"}),
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
                        "object": "document:inv-test"
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    // Check both stores - results should be cached
    for store_id in [&store_a, &store_b] {
        let (status, response) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
            &format!("/stores/{store_id}/check"),
            serde_json::json!({
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:inv-test"
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["allowed"], true);
    }

    wait_for_cache_sync(&cache).await;

    // Verify both are cached
    let cache_key_a = CacheKey::new(&store_a, "document:inv-test", "viewer", "user:alice");
    let cache_key_b = CacheKey::new(&store_b, "document:inv-test", "viewer", "user:alice");

    assert_eq!(
        cache.get(&cache_key_a).await,
        Some(true),
        "Store A should be cached"
    );
    assert_eq!(
        cache.get(&cache_key_b).await,
        Some(true),
        "Store B should be cached"
    );

    // Write new tuple to store A (triggers cache invalidation for store A)
    let (status, _) = post_json(
        create_test_app_with_shared_cache(&storage, &cache),
        &format!("/stores/{store_a}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:bob",
                    "relation": "editor",
                    "object": "document:another"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Wait for store A cache to be invalidated
    wait_for_cache_invalidation(&cache, &cache_key_a).await;

    // Verify store A cache is invalidated
    assert_eq!(
        cache.get(&cache_key_a).await,
        None,
        "Store A cache should be invalidated"
    );

    // Verify store B cache is still intact
    assert_eq!(
        cache.get(&cache_key_b).await,
        Some(true),
        "ISOLATION: Store B cache should NOT be affected by Store A write"
    );
}

/// Test: Concurrent operations on different stores don't interfere.
///
/// Verifies that concurrent writes and checks to different stores
/// maintain isolation and don't cause race conditions.
#[tokio::test]
async fn test_concurrent_operations_different_stores_isolated() {
    use futures::future::join_all;

    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create multiple stores
    let mut store_ids = Vec::new();
    for i in 0..NUM_CONCURRENT_STORES {
        let (status, response) = post_json(
            create_test_app(&storage),
            "/stores",
            serde_json::json!({"name": format!("concurrent-store-{i}")}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let store_id = response["id"].as_str().unwrap().to_string();
        setup_simple_model(&storage, &store_id).await;
        store_ids.push(store_id);
    }

    // Concurrently write and check across all stores
    let futures: Vec<_> = store_ids
        .iter()
        .enumerate()
        .flat_map(|(store_idx, store_id)| {
            let storage = Arc::clone(&storage);
            let cache = Arc::clone(&cache);
            let store_id = store_id.clone();

            (0..OPS_PER_STORE).map(move |op_idx| {
                let storage = Arc::clone(&storage);
                let cache = Arc::clone(&cache);
                let store_id = store_id.clone();

                async move {
                    // Write a unique tuple
                    let (status, _) = post_json(
                        create_test_app_with_shared_cache(&storage, &cache),
                        &format!("/stores/{store_id}/write"),
                        serde_json::json!({
                            "writes": {
                                "tuple_keys": [{
                                    "user": format!("user:user{op_idx}"),
                                    "relation": "viewer",
                                    "object": format!("document:doc{store_idx}-{op_idx}")
                                }]
                            }
                        }),
                    )
                    .await;
                    assert_eq!(status, StatusCode::OK);

                    // Check the tuple
                    let (status, response) = post_json(
                        create_test_app_with_shared_cache(&storage, &cache),
                        &format!("/stores/{store_id}/check"),
                        serde_json::json!({
                            "tuple_key": {
                                "user": format!("user:user{op_idx}"),
                                "relation": "viewer",
                                "object": format!("document:doc{store_idx}-{op_idx}")
                            }
                        }),
                    )
                    .await;
                    assert_eq!(status, StatusCode::OK);
                    (
                        store_idx,
                        op_idx,
                        response["allowed"].as_bool().unwrap_or(false),
                    )
                }
            })
        })
        .collect();

    // Wrap concurrent operations with global timeout to prevent test hangs
    let results = tokio::time::timeout(CONCURRENT_TEST_TIMEOUT, join_all(futures))
        .await
        .expect("Concurrent test timed out - possible deadlock or performance issue");

    // Verify all operations succeeded and returned true
    for (store_idx, op_idx, allowed) in results {
        assert!(
            allowed,
            "Check for store {} op {} should return true",
            store_idx, op_idx
        );
    }

    // Verify each store has exactly the expected tuples AND they belong to the correct store
    for (store_idx, store_id) in store_ids.iter().enumerate() {
        let tuples = storage
            .read_tuples(store_id, &Default::default())
            .await
            .unwrap();
        assert_eq!(
            tuples.len(),
            OPS_PER_STORE,
            "Store {} should have exactly {} tuples",
            store_idx,
            OPS_PER_STORE
        );

        // Verify all tuples belong to this specific store (contain store index in object_id)
        let expected_prefix = format!("doc{store_idx}-");
        for tuple in &tuples {
            assert!(
                tuple.object_id.starts_with(&expected_prefix),
                "ISOLATION: Store {} tuple has wrong object_id '{}', expected prefix '{}'",
                store_idx,
                tuple.object_id,
                expected_prefix
            );
        }
    }
}

// =============================================================================
// Section 3: Model Isolation Tests
// =============================================================================

/// Test: Authorization models are isolated between stores.
///
/// Verifies that each store can have a completely different authorization
/// model without affecting other stores.
#[tokio::test]
async fn test_authorization_models_isolated_between_stores() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create store 1 with simple model (document type)
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "model-store-1"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_1 = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_1).await;

    // Create store 2 with complete model (document + folder types)
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "model-store-2"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_2 = response["id"].as_str().unwrap().to_string();
    setup_complete_model(&storage, &store_2).await;

    // Write tuple to store 2 using "folder" type (not in store 1's model)
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_2}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "folder:shared"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Verify the folder tuple exists in store 2
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_2}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "folder:shared"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"], true,
        "Folder check should succeed in store 2"
    );

    // Store 1 doesn't have folder type in the simple model
    // The API may return either:
    // - 400 Bad Request (type not in model)
    // - 200 OK with allowed=false (no matching tuple)
    // Both are correct isolation behavior - the important thing is NO cross-store data leakage
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_1}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "folder:shared"
            }
        }),
    )
    .await;
    // Either 400 (type not in model) or 200 with false is acceptable
    // The key isolation test is: store 1 must NOT return allowed=true based on store 2's data
    assert!(
        status == StatusCode::BAD_REQUEST
            || (status == StatusCode::OK && response["allowed"] == false),
        "Store 1 check should either fail (type not in model) or return allowed=false, got status={}, allowed={:?}",
        status,
        response["allowed"]
    );
}

/// Test: Model updates in one store don't affect other stores.
///
/// Verifies that updating the authorization model in one store doesn't
/// change the behavior of other stores.
#[tokio::test]
async fn test_model_update_isolated_between_stores() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create two stores with identical initial models
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "update-store-1"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_1 = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_1).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "update-store-2"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_2 = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_2).await;

    // Write identical tuples to both stores
    for store_id in [&store_1, &store_2] {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": "user:alice",
                        "relation": "viewer",
                        "object": "document:shared"
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    // Update model in store 1 only (add new model version)
    setup_complete_model(&storage, &store_1).await;

    // Both stores should still work and return the same result
    for store_id in [&store_1, &store_2] {
        let (status, response) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/check"),
            serde_json::json!({
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:shared"
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            response["allowed"], true,
            "Check should still work after model update"
        );
    }
}

// =============================================================================
// Section 4: Lifecycle Isolation Tests
// =============================================================================

/// Test: Deleting a store doesn't affect other stores.
///
/// Verifies that deleting one store doesn't impact the data or
/// functionality of other stores.
#[tokio::test]
async fn test_store_deletion_isolated() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create three stores
    let mut store_ids = Vec::new();
    for i in 0..3 {
        let (status, response) = post_json(
            create_test_app(&storage),
            "/stores",
            serde_json::json!({"name": format!("delete-store-{i}")}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let store_id = response["id"].as_str().unwrap().to_string();
        setup_simple_model(&storage, &store_id).await;

        // Write tuple to each store
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": "user:alice",
                        "relation": "viewer",
                        "object": format!("document:doc{i}")
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        store_ids.push(store_id);
    }

    // Delete the middle store
    storage.delete_store(&store_ids[1]).await.unwrap();

    // Verify store 0 still works
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{}/check", store_ids[0]),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:doc0"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"], true,
        "Store 0 should still work after Store 1 deletion"
    );

    // Verify store 2 still works
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{}/check", store_ids[2]),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:doc2"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"], true,
        "Store 2 should still work after Store 1 deletion"
    );

    // Verify deleted store returns appropriate error
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{}/check", store_ids[1]),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:doc1"
            }
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "Deleted store should return NOT_FOUND, got {}",
        status
    );
}

/// Test: Tuple deletion in one store doesn't affect other stores.
///
/// Verifies that deleting tuples in one store doesn't delete matching
/// tuples in other stores.
#[tokio::test]
async fn test_tuple_deletion_isolated_between_stores() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create two stores
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "tuple-del-1"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_1 = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_1).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "tuple-del-2"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_2 = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_2).await;

    // Write identical tuples to both stores
    for store_id in [&store_1, &store_2] {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": "user:alice",
                        "relation": "viewer",
                        "object": "document:shared"
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    // Delete the tuple from store 1 only
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_1}/write"),
        serde_json::json!({
            "deletes": {
                "tuple_keys": [{
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:shared"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Verify tuple is deleted from store 1
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_1}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:shared"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"], false,
        "Tuple should be deleted from store 1"
    );

    // Verify tuple still exists in store 2
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_2}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:shared"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"], true,
        "ISOLATION: Tuple should still exist in store 2"
    );
}

/// Test: Pagination tokens are scoped to their store.
///
/// Verifies that pagination tokens from one store cannot be used
/// to access data from another store.
#[tokio::test]
async fn test_pagination_tokens_scoped_to_store() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create two stores
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "page-store-1"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_1 = response["id"].as_str().unwrap().to_string();

    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "page-store-2"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_2 = response["id"].as_str().unwrap().to_string();

    // Write many tuples to store 1 to trigger pagination
    for i in 0..50 {
        storage
            .write_tuple(
                &store_1,
                StoredTuple::new(
                    "document",
                    format!("doc{i}"),
                    "viewer",
                    "user",
                    "alice",
                    None,
                ),
            )
            .await
            .unwrap();
    }

    // Write few tuples to store 2
    for i in 0..5 {
        storage
            .write_tuple(
                &store_2,
                StoredTuple::new("document", format!("doc{i}"), "viewer", "user", "bob", None),
            )
            .await
            .unwrap();
    }

    // Read first page from store 1 with small page size
    let pagination = PaginationOptions {
        page_size: Some(10),
        continuation_token: None,
    };
    let result = storage
        .read_tuples_paginated(&store_1, &Default::default(), &pagination)
        .await
        .unwrap();
    assert_eq!(
        result.items.len(),
        10,
        "Should return 10 tuples in first page"
    );
    assert!(
        result.continuation_token.is_some(),
        "Should have continuation token"
    );

    // Attempt to use store 1's continuation token with store 2
    let pagination_store_2 = PaginationOptions {
        page_size: Some(10),
        continuation_token: result.continuation_token.clone(),
    };
    let result_store_2 = storage
        .read_tuples_paginated(&store_2, &Default::default(), &pagination_store_2)
        .await
        .unwrap();

    // The token from store 1 should not give access to store 1's data via store 2
    // It may be ignored, reset, or return store 2's data - but should NEVER leak store 1 data
    for tuple in &result_store_2.items {
        // All returned tuples must belong to bob (store 2's user)
        // and must not be from alice (store 1's user)
        assert!(
            tuple.user_id != "alice" || tuple.user_type != "user",
            "ISOLATION: Store 2 query should not return Store 1 data"
        );
    }
}

// =============================================================================
// Section 5: Protocol Coverage - HTTP Tests
// =============================================================================

/// Test: HTTP Check endpoint respects store boundaries.
#[tokio::test]
async fn test_http_check_respects_store_boundaries() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create two stores
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "http-store-a"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_a = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_a).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "http-store-b"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_b = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_b).await;

    // Write to store A
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_a}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:http-alice",
                    "relation": "viewer",
                    "object": "document:http-doc"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // HTTP Check in store A - should be true
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_a}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:http-alice",
                "relation": "viewer",
                "object": "document:http-doc"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["allowed"], true);

    // HTTP Check in store B - should be false
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_b}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:http-alice",
                "relation": "viewer",
                "object": "document:http-doc"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["allowed"], false);
}

/// Test: HTTP Write endpoint respects store boundaries.
#[tokio::test]
async fn test_http_write_respects_store_boundaries() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create two stores
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "http-write-1"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_1 = response["id"].as_str().unwrap().to_string();

    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "http-write-2"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_2 = response["id"].as_str().unwrap().to_string();

    // Write via HTTP to store 1
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_1}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:writer",
                    "relation": "viewer",
                    "object": "document:http-write-doc"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Verify tuple exists in store 1
    let tuples_1 = storage
        .read_tuples(&store_1, &Default::default())
        .await
        .unwrap();
    assert_eq!(tuples_1.len(), 1);

    // Verify tuple does NOT exist in store 2
    let tuples_2 = storage
        .read_tuples(&store_2, &Default::default())
        .await
        .unwrap();
    assert_eq!(tuples_2.len(), 0);
}

// =============================================================================
// Section 5: Protocol Coverage - gRPC Tests
// =============================================================================

/// Test: gRPC Check respects store boundaries.
#[tokio::test]
async fn test_grpc_check_respects_store_boundaries() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create two stores
    storage
        .create_store("grpc-store-a", "gRPC Store A")
        .await
        .unwrap();
    storage
        .create_store("grpc-store-b", "gRPC Store B")
        .await
        .unwrap();

    // Set up models using shared constant
    let model_a = StoredAuthorizationModel::new(
        ulid::Ulid::new().to_string(),
        "grpc-store-a",
        "1.1",
        SIMPLE_MODEL_JSON.to_string(),
    );
    storage.write_authorization_model(model_a).await.unwrap();

    let model_b = StoredAuthorizationModel::new(
        ulid::Ulid::new().to_string(),
        "grpc-store-b",
        "1.1",
        SIMPLE_MODEL_JSON.to_string(),
    );
    storage.write_authorization_model(model_b).await.unwrap();

    // Write tuple to store A only
    storage
        .write_tuple(
            "grpc-store-a",
            StoredTuple::new("document", "grpc-doc", "viewer", "user", "alice", None),
        )
        .await
        .unwrap();

    let service = test_grpc_service(storage);

    // gRPC Check in store A - should be true
    let request = Request::new(CheckRequest {
        store_id: "grpc-store-a".to_string(),
        tuple_key: Some(TupleKey {
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:grpc-doc".to_string(),
            condition: None,
        }),
        contextual_tuples: None,
        authorization_model_id: String::new(),
        trace: false,
        context: None,
        consistency: 0,
    });

    let response = service.check(request).await;
    assert!(response.is_ok());
    assert!(
        response.unwrap().into_inner().allowed,
        "gRPC Check should return true in store A"
    );

    // gRPC Check in store B - should be false (isolation)
    let request = Request::new(CheckRequest {
        store_id: "grpc-store-b".to_string(),
        tuple_key: Some(TupleKey {
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:grpc-doc".to_string(),
            condition: None,
        }),
        contextual_tuples: None,
        authorization_model_id: String::new(),
        trace: false,
        context: None,
        consistency: 0,
    });

    let response = service.check(request).await;
    assert!(response.is_ok());
    assert!(
        !response.unwrap().into_inner().allowed,
        "ISOLATION: gRPC Check should return false in store B"
    );
}

/// Test: gRPC BatchCheck respects store boundaries.
#[tokio::test]
async fn test_grpc_batch_check_respects_store_boundaries() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create two stores
    storage
        .create_store("grpc-batch-a", "gRPC Batch A")
        .await
        .unwrap();
    storage
        .create_store("grpc-batch-b", "gRPC Batch B")
        .await
        .unwrap();

    // Set up models using shared constant
    let model_a = StoredAuthorizationModel::new(
        ulid::Ulid::new().to_string(),
        "grpc-batch-a",
        "1.1",
        SIMPLE_MODEL_JSON.to_string(),
    );
    storage.write_authorization_model(model_a).await.unwrap();

    let model_b = StoredAuthorizationModel::new(
        ulid::Ulid::new().to_string(),
        "grpc-batch-b",
        "1.1",
        SIMPLE_MODEL_JSON.to_string(),
    );
    storage.write_authorization_model(model_b).await.unwrap();

    // Write tuple to store A only
    storage
        .write_tuple(
            "grpc-batch-a",
            StoredTuple::new("document", "batch-doc", "viewer", "user", "bob", None),
        )
        .await
        .unwrap();

    let service = test_grpc_service(storage);

    // gRPC BatchCheck in store A - should be true
    let request = Request::new(BatchCheckRequest {
        store_id: "grpc-batch-a".to_string(),
        checks: vec![BatchCheckItem {
            tuple_key: Some(TupleKey {
                user: "user:bob".to_string(),
                relation: "viewer".to_string(),
                object: "document:batch-doc".to_string(),
                condition: None,
            }),
            contextual_tuples: None,
            context: None,
            correlation_id: "check-a".to_string(),
        }],
        authorization_model_id: String::new(),
        consistency: 0,
    });

    let response = service.batch_check(request).await;
    assert!(response.is_ok());
    let result = response.unwrap().into_inner();
    assert!(
        result.result.get("check-a").unwrap().allowed,
        "gRPC BatchCheck should return true in store A"
    );

    // gRPC BatchCheck in store B - should be false (isolation)
    let request = Request::new(BatchCheckRequest {
        store_id: "grpc-batch-b".to_string(),
        checks: vec![BatchCheckItem {
            tuple_key: Some(TupleKey {
                user: "user:bob".to_string(),
                relation: "viewer".to_string(),
                object: "document:batch-doc".to_string(),
                condition: None,
            }),
            contextual_tuples: None,
            context: None,
            correlation_id: "check-b".to_string(),
        }],
        authorization_model_id: String::new(),
        consistency: 0,
    });

    let response = service.batch_check(request).await;
    assert!(response.is_ok());
    let result = response.unwrap().into_inner();
    assert!(
        !result.result.get("check-b").unwrap().allowed,
        "ISOLATION: gRPC BatchCheck should return false in store B"
    );
}

/// Test: gRPC Write respects store boundaries.
#[tokio::test]
async fn test_grpc_write_respects_store_boundaries() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create two stores
    storage
        .create_store("grpc-write-1", "gRPC Write 1")
        .await
        .unwrap();
    storage
        .create_store("grpc-write-2", "gRPC Write 2")
        .await
        .unwrap();

    let service = test_grpc_service(Arc::clone(&storage));

    // gRPC Write to store 1
    let request = Request::new(WriteRequest {
        store_id: "grpc-write-1".to_string(),
        writes: Some(WriteRequestWrites {
            tuple_keys: vec![TupleKey {
                user: "user:grpc-writer".to_string(),
                relation: "viewer".to_string(),
                object: "document:grpc-write-doc".to_string(),
                condition: None,
            }],
        }),
        deletes: None,
        authorization_model_id: String::new(),
    });

    let response = service.write(request).await;
    assert!(response.is_ok());

    // Verify tuple exists in store 1
    let tuples_1 = storage
        .read_tuples("grpc-write-1", &Default::default())
        .await
        .unwrap();
    assert_eq!(tuples_1.len(), 1, "Store 1 should have 1 tuple");

    // Verify tuple does NOT exist in store 2
    let tuples_2 = storage
        .read_tuples("grpc-write-2", &Default::default())
        .await
        .unwrap();
    assert_eq!(tuples_2.len(), 0, "ISOLATION: Store 2 should have 0 tuples");
}

/// Test: gRPC Read respects store boundaries.
#[tokio::test]
async fn test_grpc_read_respects_store_boundaries() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create two stores
    storage
        .create_store("grpc-read-1", "gRPC Read 1")
        .await
        .unwrap();
    storage
        .create_store("grpc-read-2", "gRPC Read 2")
        .await
        .unwrap();

    // Write different tuples to each store
    storage
        .write_tuple(
            "grpc-read-1",
            StoredTuple::new("document", "read-doc-1", "viewer", "user", "reader1", None),
        )
        .await
        .unwrap();

    storage
        .write_tuple(
            "grpc-read-2",
            StoredTuple::new("document", "read-doc-2", "editor", "user", "reader2", None),
        )
        .await
        .unwrap();

    let service = test_grpc_service(storage);

    // gRPC Read from store 1
    let request = Request::new(ReadRequest {
        store_id: "grpc-read-1".to_string(),
        tuple_key: None,
        page_size: None,
        continuation_token: String::new(),
        consistency: 0,
    });

    let response = service.read(request).await;
    assert!(response.is_ok());
    let result = response.unwrap().into_inner();
    assert_eq!(result.tuples.len(), 1);
    assert_eq!(
        result.tuples[0].key.as_ref().unwrap().object,
        "document:read-doc-1"
    );

    // gRPC Read from store 2
    let request = Request::new(ReadRequest {
        store_id: "grpc-read-2".to_string(),
        tuple_key: None,
        page_size: None,
        continuation_token: String::new(),
        consistency: 0,
    });

    let response = service.read(request).await;
    assert!(response.is_ok());
    let result = response.unwrap().into_inner();
    assert_eq!(result.tuples.len(), 1);
    assert_eq!(
        result.tuples[0].key.as_ref().unwrap().object,
        "document:read-doc-2"
    );
}

// =============================================================================
// Section 6: Behavioral Parity Tests (HTTP vs gRPC)
// =============================================================================

/// Test: HTTP and gRPC return identical isolation behavior.
///
/// Verifies that both protocols behave identically for store isolation,
/// ensuring no protocol-specific isolation bugs.
#[tokio::test]
async fn test_http_grpc_behavioral_parity_for_isolation() {
    let storage = Arc::new(MemoryDataStore::new());

    // Use valid ULID format for store IDs (OpenFGA compatibility)
    let store_id_a = ulid::Ulid::new().to_string();
    let store_id_b = ulid::Ulid::new().to_string();

    // Create stores
    storage.create_store(&store_id_a, "Parity A").await.unwrap();
    storage.create_store(&store_id_b, "Parity B").await.unwrap();

    // Set up models for both stores using shared constant
    for store_id in [&store_id_a, &store_id_b] {
        let model = StoredAuthorizationModel::new(
            ulid::Ulid::new().to_string(),
            store_id,
            "1.1",
            SIMPLE_MODEL_JSON.to_string(),
        );
        storage.write_authorization_model(model).await.unwrap();
    }

    // Write tuple to store A only
    storage
        .write_tuple(
            &store_id_a,
            StoredTuple::new(
                "document",
                "parity-doc",
                "viewer",
                "user",
                "parity-user",
                None,
            ),
        )
        .await
        .unwrap();

    // HTTP Check in store A
    let (http_status_a, http_response_a) = post_json(
        create_test_app(&storage),
        &format!("/stores/{}/check", store_id_a),
        serde_json::json!({
            "tuple_key": {
                "user": "user:parity-user",
                "relation": "viewer",
                "object": "document:parity-doc"
            }
        }),
    )
    .await;
    assert_eq!(http_status_a, StatusCode::OK);
    let http_allowed_a = http_response_a["allowed"].as_bool().unwrap();

    // gRPC Check in store A
    let service = test_grpc_service(Arc::clone(&storage));
    let request = Request::new(CheckRequest {
        store_id: store_id_a.clone(),
        tuple_key: Some(TupleKey {
            user: "user:parity-user".to_string(),
            relation: "viewer".to_string(),
            object: "document:parity-doc".to_string(),
            condition: None,
        }),
        contextual_tuples: None,
        authorization_model_id: String::new(),
        trace: false,
        context: None,
        consistency: 0,
    });
    let grpc_response_a = service.check(request).await.unwrap().into_inner();
    let grpc_allowed_a = grpc_response_a.allowed;

    // HTTP and gRPC should return the same result for store A
    assert_eq!(
        http_allowed_a, grpc_allowed_a,
        "HTTP and gRPC should return identical results for store A"
    );
    assert!(http_allowed_a, "Store A should allow access");

    // HTTP Check in store B
    let (http_status_b, http_response_b) = post_json(
        create_test_app(&storage),
        &format!("/stores/{}/check", store_id_b),
        serde_json::json!({
            "tuple_key": {
                "user": "user:parity-user",
                "relation": "viewer",
                "object": "document:parity-doc"
            }
        }),
    )
    .await;
    assert_eq!(http_status_b, StatusCode::OK);
    let http_allowed_b = http_response_b["allowed"].as_bool().unwrap();

    // gRPC Check in store B
    let request = Request::new(CheckRequest {
        store_id: store_id_b.clone(),
        tuple_key: Some(TupleKey {
            user: "user:parity-user".to_string(),
            relation: "viewer".to_string(),
            object: "document:parity-doc".to_string(),
            condition: None,
        }),
        contextual_tuples: None,
        authorization_model_id: String::new(),
        trace: false,
        context: None,
        consistency: 0,
    });
    let grpc_response_b = service.check(request).await.unwrap().into_inner();
    let grpc_allowed_b = grpc_response_b.allowed;

    // HTTP and gRPC should return the same result for store B (isolation)
    assert_eq!(
        http_allowed_b, grpc_allowed_b,
        "HTTP and gRPC should return identical results for store B"
    );
    assert!(!http_allowed_b, "ISOLATION: Store B should deny access");
}
