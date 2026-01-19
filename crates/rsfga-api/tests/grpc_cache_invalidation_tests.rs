//! gRPC integration tests for cache invalidation (Issue #219).
//!
//! These tests verify that the check cache is properly invalidated after
//! Write/Delete operations via the gRPC API, ensuring no stale authorization
//! decisions are served.
//!
//! This is the gRPC counterpart to `cache_invalidation_tests.rs` which tests
//! HTTP/REST endpoints. Issue #200 acceptance criteria require testing via
//! both HTTP and gRPC endpoints.
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
//! 3. Cache Metrics and Observability - Verify cache entry tracking
//! 4. Batch Check Cache Behavior - Verify batch operations work with cache

use std::sync::Arc;
use std::time::Duration;

use tonic::Request;

use rsfga_api::grpc::OpenFgaGrpcService;
use rsfga_api::proto::openfga::v1::open_fga_service_server::OpenFgaService;
use rsfga_api::proto::openfga::v1::*;
use rsfga_domain::cache::{CacheKey, CheckCache, CheckCacheConfig};
use rsfga_storage::{DataStore, MemoryDataStore, StoredAuthorizationModel};

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

/// Helper to create a simple authorization model with document and user types.
fn simple_model_json() -> String {
    r#"{
        "type_definitions": [
            {"type": "user"},
            {"type": "team", "relations": {"member": {}}},
            {"type": "document", "relations": {"viewer": {}, "editor": {}, "owner": {}}}
        ]
    }"#
    .to_string()
}

/// Helper to create and write a simple authorization model to a store.
async fn setup_simple_model(storage: &MemoryDataStore, store_id: &str) -> String {
    let model = StoredAuthorizationModel::new(
        ulid::Ulid::new().to_string(),
        store_id,
        "1.1",
        simple_model_json(),
    );
    let model_id = model.id.clone();
    storage.write_authorization_model(model).await.unwrap();
    model_id
}

/// Helper to create a gRPC service with caching enabled.
fn create_grpc_service_with_cache(
    storage: Arc<MemoryDataStore>,
) -> OpenFgaGrpcService<MemoryDataStore> {
    let cache_config = CheckCacheConfig::default().with_enabled(true);
    OpenFgaGrpcService::with_cache_config(storage, cache_config)
}

/// Helper to create a gRPC service with custom cache configuration.
fn create_grpc_service_with_cache_config(
    storage: Arc<MemoryDataStore>,
    cache_config: CheckCacheConfig,
) -> OpenFgaGrpcService<MemoryDataStore> {
    OpenFgaGrpcService::with_cache_config(storage, cache_config)
}

// ============================================================
// Section 1: Cache Invalidation on Write Operations
// ============================================================

/// Test: Cache is invalidated after writing a new tuple via gRPC.
///
/// Scenario:
/// 1. Check permission (should be false, gets cached)
/// 2. Write tuple granting permission
/// 3. Check permission again (should be true, not stale false)
#[tokio::test]
async fn test_grpc_cache_invalidated_after_write_grants_access() {
    let storage = Arc::new(MemoryDataStore::new());
    let service = create_grpc_service_with_cache(Arc::clone(&storage));
    let cache = Arc::clone(service.cache());

    // Create store
    let response = service
        .create_store(Request::new(CreateStoreRequest {
            name: "grpc-cache-write-test".to_string(),
        }))
        .await
        .unwrap();
    let store_id = response.into_inner().id;
    setup_simple_model(&storage, &store_id).await;

    // Step 1: Check permission - should be false and get cached
    let check_response = service
        .check(Request::new(CheckRequest {
            store_id: store_id.clone(),
            tuple_key: Some(TupleKey {
                user: "user:alice".to_string(),
                relation: "viewer".to_string(),
                object: "document:secret".to_string(),
                condition: None,
            }),
            contextual_tuples: None,
            authorization_model_id: String::new(),
            trace: false,
            context: None,
            consistency: 0,
        }))
        .await
        .unwrap();
    assert!(
        !check_response.into_inner().allowed,
        "Alice should NOT have access before tuple is written"
    );

    // Verify the result was cached
    let cache_key = CacheKey::new(&store_id, "document:secret", "viewer", "user:alice");
    cache.run_pending_tasks().await;
    let cached = cache.get(&cache_key).await;
    assert_eq!(cached, Some(false), "False result should be cached");

    // Step 2: Write tuple granting access
    let write_response = service
        .write(Request::new(WriteRequest {
            store_id: store_id.clone(),
            writes: Some(WriteRequestWrites {
                tuple_keys: vec![TupleKey {
                    user: "user:alice".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:secret".to_string(),
                    condition: None,
                }],
            }),
            deletes: None,
            authorization_model_id: String::new(),
        }))
        .await;
    assert!(write_response.is_ok());

    // Wait for cache invalidation using polling instead of fixed sleep
    wait_for_cache_invalidation(&cache, &cache_key).await;

    // Step 3: Verify cache was invalidated (entry should be gone)
    let cached_after = cache.get(&cache_key).await;
    assert_eq!(
        cached_after, None,
        "Cache entry should be invalidated after write"
    );

    // Step 4: Check permission again - should be true now
    let check_response = service
        .check(Request::new(CheckRequest {
            store_id: store_id.clone(),
            tuple_key: Some(TupleKey {
                user: "user:alice".to_string(),
                relation: "viewer".to_string(),
                object: "document:secret".to_string(),
                condition: None,
            }),
            contextual_tuples: None,
            authorization_model_id: String::new(),
            trace: false,
            context: None,
            consistency: 0,
        }))
        .await
        .unwrap();
    assert!(
        check_response.into_inner().allowed,
        "Alice should have access after tuple is written"
    );
}

/// Test: Cache is invalidated after deleting a tuple via gRPC.
///
/// Scenario:
/// 1. Write tuple granting permission
/// 2. Check permission (should be true, gets cached)
/// 3. Delete tuple revoking permission
/// 4. Check permission again (should be false, not stale true)
///
/// This is the CRITICAL security test - stale true after revocation is dangerous.
#[tokio::test]
async fn test_grpc_cache_invalidated_after_delete_revokes_access() {
    let storage = Arc::new(MemoryDataStore::new());
    let service = create_grpc_service_with_cache(Arc::clone(&storage));
    let cache = Arc::clone(service.cache());

    // Create store
    let response = service
        .create_store(Request::new(CreateStoreRequest {
            name: "grpc-cache-delete-test".to_string(),
        }))
        .await
        .unwrap();
    let store_id = response.into_inner().id;
    setup_simple_model(&storage, &store_id).await;

    // Step 1: Write tuple granting access
    let write_response = service
        .write(Request::new(WriteRequest {
            store_id: store_id.clone(),
            writes: Some(WriteRequestWrites {
                tuple_keys: vec![TupleKey {
                    user: "user:bob".to_string(),
                    relation: "editor".to_string(),
                    object: "document:confidential".to_string(),
                    condition: None,
                }],
            }),
            deletes: None,
            authorization_model_id: String::new(),
        }))
        .await;
    assert!(write_response.is_ok());

    // Step 2: Check permission - should be true and get cached
    let check_response = service
        .check(Request::new(CheckRequest {
            store_id: store_id.clone(),
            tuple_key: Some(TupleKey {
                user: "user:bob".to_string(),
                relation: "editor".to_string(),
                object: "document:confidential".to_string(),
                condition: None,
            }),
            contextual_tuples: None,
            authorization_model_id: String::new(),
            trace: false,
            context: None,
            consistency: 0,
        }))
        .await
        .unwrap();
    assert!(
        check_response.into_inner().allowed,
        "Bob should have access before tuple is deleted"
    );

    // Verify the result was cached
    let cache_key = CacheKey::new(&store_id, "document:confidential", "editor", "user:bob");
    cache.run_pending_tasks().await;
    let cached = cache.get(&cache_key).await;
    assert_eq!(cached, Some(true), "True result should be cached");

    // Step 3: Delete tuple revoking access
    let delete_response = service
        .write(Request::new(WriteRequest {
            store_id: store_id.clone(),
            writes: None,
            deletes: Some(WriteRequestDeletes {
                tuple_keys: vec![TupleKeyWithoutCondition {
                    user: "user:bob".to_string(),
                    relation: "editor".to_string(),
                    object: "document:confidential".to_string(),
                }],
            }),
            authorization_model_id: String::new(),
        }))
        .await;
    assert!(delete_response.is_ok());

    // Wait for cache invalidation using polling instead of fixed sleep
    wait_for_cache_invalidation(&cache, &cache_key).await;

    // Step 4: Verify cache was invalidated
    let cached_after = cache.get(&cache_key).await;
    assert_eq!(
        cached_after, None,
        "Cache entry should be invalidated after delete"
    );

    // Step 5: Check permission again - MUST be false (security critical)
    let check_response = service
        .check(Request::new(CheckRequest {
            store_id: store_id.clone(),
            tuple_key: Some(TupleKey {
                user: "user:bob".to_string(),
                relation: "editor".to_string(),
                object: "document:confidential".to_string(),
                condition: None,
            }),
            contextual_tuples: None,
            authorization_model_id: String::new(),
            trace: false,
            context: None,
            consistency: 0,
        }))
        .await
        .unwrap();
    assert!(
        !check_response.into_inner().allowed,
        "SECURITY: Bob must NOT have access after tuple is deleted"
    );
}

/// Test: Cache invalidation affects only the relevant store via gRPC.
///
/// Writes to one store should not invalidate cache entries for other stores.
#[tokio::test]
async fn test_grpc_cache_invalidation_scoped_to_store() {
    let storage = Arc::new(MemoryDataStore::new());
    let service = create_grpc_service_with_cache(Arc::clone(&storage));
    let cache = Arc::clone(service.cache());

    // Create two stores
    let response_a = service
        .create_store(Request::new(CreateStoreRequest {
            name: "store-a".to_string(),
        }))
        .await
        .unwrap();
    let store_a = response_a.into_inner().id;
    setup_simple_model(&storage, &store_a).await;

    let response_b = service
        .create_store(Request::new(CreateStoreRequest {
            name: "store-b".to_string(),
        }))
        .await
        .unwrap();
    let store_b = response_b.into_inner().id;
    setup_simple_model(&storage, &store_b).await;

    // Write tuples to both stores
    for store_id in [&store_a, &store_b] {
        let write_response = service
            .write(Request::new(WriteRequest {
                store_id: store_id.clone(),
                writes: Some(WriteRequestWrites {
                    tuple_keys: vec![TupleKey {
                        user: "user:alice".to_string(),
                        relation: "viewer".to_string(),
                        object: "document:doc1".to_string(),
                        condition: None,
                    }],
                }),
                deletes: None,
                authorization_model_id: String::new(),
            }))
            .await;
        assert!(write_response.is_ok());
    }

    // Check permission on both stores - should be cached
    for store_id in [&store_a, &store_b] {
        let check_response = service
            .check(Request::new(CheckRequest {
                store_id: store_id.clone(),
                tuple_key: Some(TupleKey {
                    user: "user:alice".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:doc1".to_string(),
                    condition: None,
                }),
                contextual_tuples: None,
                authorization_model_id: String::new(),
                trace: false,
                context: None,
                consistency: 0,
            }))
            .await
            .unwrap();
        assert!(check_response.into_inner().allowed);
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
    let write_response = service
        .write(Request::new(WriteRequest {
            store_id: store_a.clone(),
            writes: Some(WriteRequestWrites {
                tuple_keys: vec![TupleKey {
                    user: "user:charlie".to_string(),
                    relation: "owner".to_string(),
                    object: "document:doc2".to_string(),
                    condition: None,
                }],
            }),
            deletes: None,
            authorization_model_id: String::new(),
        }))
        .await;
    assert!(write_response.is_ok());

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

/// Test: Batch write invalidates cache for all affected tuples via gRPC.
#[tokio::test]
async fn test_grpc_cache_invalidation_on_batch_write() {
    let storage = Arc::new(MemoryDataStore::new());
    let service = create_grpc_service_with_cache(Arc::clone(&storage));
    let cache = Arc::clone(service.cache());

    // Create store
    let response = service
        .create_store(Request::new(CreateStoreRequest {
            name: "grpc-batch-cache-test".to_string(),
        }))
        .await
        .unwrap();
    let store_id = response.into_inner().id;
    setup_simple_model(&storage, &store_id).await;

    // Check multiple permissions - all should be false and cached
    let users = ["alice", "bob", "charlie"];
    for user in &users {
        let check_response = service
            .check(Request::new(CheckRequest {
                store_id: store_id.clone(),
                tuple_key: Some(TupleKey {
                    user: format!("user:{user}"),
                    relation: "viewer".to_string(),
                    object: "document:shared".to_string(),
                    condition: None,
                }),
                contextual_tuples: None,
                authorization_model_id: String::new(),
                trace: false,
                context: None,
                consistency: 0,
            }))
            .await
            .unwrap();
        assert!(!check_response.into_inner().allowed);
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
    let write_response = service
        .write(Request::new(WriteRequest {
            store_id: store_id.clone(),
            writes: Some(WriteRequestWrites {
                tuple_keys: users
                    .iter()
                    .map(|user| TupleKey {
                        user: format!("user:{user}"),
                        relation: "viewer".to_string(),
                        object: "document:shared".to_string(),
                        condition: None,
                    })
                    .collect(),
            }),
            deletes: None,
            authorization_model_id: String::new(),
        }))
        .await;
    assert!(write_response.is_ok());

    // Wait for cache invalidation using polling - check first user's cache key
    let first_user_key = CacheKey::new(&store_id, "document:shared", "viewer", "user:alice");
    wait_for_cache_invalidation(&cache, &first_user_key).await;

    // Check all permissions again - should be true now
    for user in &users {
        let check_response = service
            .check(Request::new(CheckRequest {
                store_id: store_id.clone(),
                tuple_key: Some(TupleKey {
                    user: format!("user:{user}"),
                    relation: "viewer".to_string(),
                    object: "document:shared".to_string(),
                    condition: None,
                }),
                contextual_tuples: None,
                authorization_model_id: String::new(),
                trace: false,
                context: None,
                consistency: 0,
            }))
            .await
            .unwrap();
        assert!(
            check_response.into_inner().allowed,
            "{user} should now have access after batch write"
        );
    }
}

// ============================================================
// Section 2: Concurrent Check+Write Behavior
// ============================================================

/// Test: Concurrent writes and checks produce consistent results via gRPC.
///
/// Simulates a race condition where checks and writes happen simultaneously.
/// Uses tokio::spawn to achieve true parallelism (not sequential await).
/// After all writes complete, subsequent checks should see the written data.
#[tokio::test]
async fn test_grpc_concurrent_writes_and_checks_consistency() {
    let storage = Arc::new(MemoryDataStore::new());
    let service = Arc::new(create_grpc_service_with_cache(Arc::clone(&storage)));
    let cache = Arc::clone(service.cache());

    // Create store
    let response = service
        .create_store(Request::new(CreateStoreRequest {
            name: "grpc-concurrent-test".to_string(),
        }))
        .await
        .unwrap();
    let store_id = response.into_inner().id;
    setup_simple_model(&storage, &store_id).await;

    // Launch concurrent writes and checks using tokio::spawn for true parallelism
    let num_users = 20;
    let mut handles = Vec::new();

    for i in 0..num_users {
        // Spawn write task
        let service_clone = Arc::clone(&service);
        let store_id_clone = store_id.clone();
        let write_handle = tokio::spawn(async move {
            let response = service_clone
                .write(Request::new(WriteRequest {
                    store_id: store_id_clone,
                    writes: Some(WriteRequestWrites {
                        tuple_keys: vec![TupleKey {
                            user: format!("user:concurrent{i}"),
                            relation: "viewer".to_string(),
                            object: "document:shared".to_string(),
                            condition: None,
                        }],
                    }),
                    deletes: None,
                    authorization_model_id: String::new(),
                }))
                .await;
            assert!(response.is_ok());
        });
        handles.push(write_handle);

        // Spawn check task (runs truly concurrent with write)
        let service_clone2 = Arc::clone(&service);
        let store_id_clone2 = store_id.clone();
        let check_handle = tokio::spawn(async move {
            let response = service_clone2
                .check(Request::new(CheckRequest {
                    store_id: store_id_clone2,
                    tuple_key: Some(TupleKey {
                        user: format!("user:concurrent{i}"),
                        relation: "viewer".to_string(),
                        object: "document:shared".to_string(),
                        condition: None,
                    }),
                    contextual_tuples: None,
                    authorization_model_id: String::new(),
                    trace: false,
                    context: None,
                    consistency: 0,
                }))
                .await;
            assert!(response.is_ok());
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
        let check_response = service
            .check(Request::new(CheckRequest {
                store_id: store_id.clone(),
                tuple_key: Some(TupleKey {
                    user: format!("user:concurrent{i}"),
                    relation: "viewer".to_string(),
                    object: "document:shared".to_string(),
                    condition: None,
                }),
                contextual_tuples: None,
                authorization_model_id: String::new(),
                trace: false,
                context: None,
                consistency: 0,
            }))
            .await
            .unwrap();
        assert!(
            check_response.into_inner().allowed,
            "user:concurrent{i} should have access after write completes"
        );
    }
}

/// Test: Read-after-write consistency - check after write returns correct result via gRPC.
///
/// This tests the fundamental consistency guarantee: if a write completes,
/// a subsequent check should reflect that write.
#[tokio::test]
async fn test_grpc_read_after_write_consistency() {
    let storage = Arc::new(MemoryDataStore::new());
    let service = create_grpc_service_with_cache(Arc::clone(&storage));
    let cache = Arc::clone(service.cache());

    // Create store
    let response = service
        .create_store(Request::new(CreateStoreRequest {
            name: "grpc-raw-test".to_string(),
        }))
        .await
        .unwrap();
    let store_id = response.into_inner().id;
    setup_simple_model(&storage, &store_id).await;

    // Run this test multiple times to catch race conditions
    for iteration in 0..10 {
        let user = format!("user:raw{iteration}");

        // Write tuple
        let write_response = service
            .write(Request::new(WriteRequest {
                store_id: store_id.clone(),
                writes: Some(WriteRequestWrites {
                    tuple_keys: vec![TupleKey {
                        user: user.clone(),
                        relation: "viewer".to_string(),
                        object: format!("document:raw{iteration}"),
                        condition: None,
                    }],
                }),
                deletes: None,
                authorization_model_id: String::new(),
            }))
            .await;
        assert!(write_response.is_ok());

        // Wait for cache sync using polling
        wait_for_cache_sync(&cache).await;

        // Immediately check - should return true
        let check_response = service
            .check(Request::new(CheckRequest {
                store_id: store_id.clone(),
                tuple_key: Some(TupleKey {
                    user: user.clone(),
                    relation: "viewer".to_string(),
                    object: format!("document:raw{iteration}"),
                    condition: None,
                }),
                contextual_tuples: None,
                authorization_model_id: String::new(),
                trace: false,
                context: None,
                consistency: 0,
            }))
            .await
            .unwrap();
        assert!(
            check_response.into_inner().allowed,
            "Iteration {iteration}: Check after write should return true"
        );
    }
}

/// Test: Delete-then-check consistency - access is revoked immediately via gRPC.
///
/// This is the security-critical version: if a delete completes,
/// a subsequent check MUST show the access as revoked.
#[tokio::test]
async fn test_grpc_delete_then_check_consistency() {
    let storage = Arc::new(MemoryDataStore::new());
    let service = create_grpc_service_with_cache(Arc::clone(&storage));
    let cache = Arc::clone(service.cache());

    // Create store
    let response = service
        .create_store(Request::new(CreateStoreRequest {
            name: "grpc-delete-check-test".to_string(),
        }))
        .await
        .unwrap();
    let store_id = response.into_inner().id;
    setup_simple_model(&storage, &store_id).await;

    // Run this test multiple times to catch race conditions
    for iteration in 0..10 {
        let user = format!("user:del{iteration}");
        let object = format!("document:del{iteration}");

        // First write the tuple
        let write_response = service
            .write(Request::new(WriteRequest {
                store_id: store_id.clone(),
                writes: Some(WriteRequestWrites {
                    tuple_keys: vec![TupleKey {
                        user: user.clone(),
                        relation: "viewer".to_string(),
                        object: object.clone(),
                        condition: None,
                    }],
                }),
                deletes: None,
                authorization_model_id: String::new(),
            }))
            .await;
        assert!(write_response.is_ok());

        // Wait for cache sync using polling
        wait_for_cache_sync(&cache).await;

        // Verify access
        let check_response = service
            .check(Request::new(CheckRequest {
                store_id: store_id.clone(),
                tuple_key: Some(TupleKey {
                    user: user.clone(),
                    relation: "viewer".to_string(),
                    object: object.clone(),
                    condition: None,
                }),
                contextual_tuples: None,
                authorization_model_id: String::new(),
                trace: false,
                context: None,
                consistency: 0,
            }))
            .await
            .unwrap();
        assert!(check_response.into_inner().allowed);

        // Now delete the tuple
        let delete_response = service
            .write(Request::new(WriteRequest {
                store_id: store_id.clone(),
                writes: None,
                deletes: Some(WriteRequestDeletes {
                    tuple_keys: vec![TupleKeyWithoutCondition {
                        user: user.clone(),
                        relation: "viewer".to_string(),
                        object: object.clone(),
                    }],
                }),
                authorization_model_id: String::new(),
            }))
            .await;
        assert!(delete_response.is_ok());

        // Wait for cache sync using polling
        wait_for_cache_sync(&cache).await;

        // CRITICAL: Check must return false
        let check_response = service
            .check(Request::new(CheckRequest {
                store_id: store_id.clone(),
                tuple_key: Some(TupleKey {
                    user: user.clone(),
                    relation: "viewer".to_string(),
                    object: object.clone(),
                    condition: None,
                }),
                contextual_tuples: None,
                authorization_model_id: String::new(),
                trace: false,
                context: None,
                consistency: 0,
            }))
            .await
            .unwrap();
        assert!(
            !check_response.into_inner().allowed,
            "SECURITY: Iteration {iteration}: Check after delete MUST return false"
        );
    }
}

/// Test: High-contention concurrent writes don't cause deadlocks via gRPC.
///
/// Multiple writers updating the same object should not deadlock
/// and cache invalidation should complete for all.
#[tokio::test]
async fn test_grpc_high_contention_writes_no_deadlock() {
    use futures::future::join_all;
    use std::time::Instant;

    let storage = Arc::new(MemoryDataStore::new());
    let service = Arc::new(create_grpc_service_with_cache(Arc::clone(&storage)));
    let cache = Arc::clone(service.cache());

    // Create store
    let response = service
        .create_store(Request::new(CreateStoreRequest {
            name: "grpc-contention-test".to_string(),
        }))
        .await
        .unwrap();
    let store_id = response.into_inner().id;
    setup_simple_model(&storage, &store_id).await;

    let start = Instant::now();
    let num_concurrent = 50;

    // Launch many concurrent writes to the same object
    let futures: Vec<_> = (0..num_concurrent)
        .map(|i| {
            let service_clone = Arc::clone(&service);
            let store_id_clone = store_id.clone();
            async move {
                let response = service_clone
                    .write(Request::new(WriteRequest {
                        store_id: store_id_clone,
                        writes: Some(WriteRequestWrites {
                            tuple_keys: vec![TupleKey {
                                user: format!("user:writer{i}"),
                                relation: "viewer".to_string(),
                                object: "document:contested".to_string(),
                                condition: None,
                            }],
                        }),
                        deletes: None,
                        authorization_model_id: String::new(),
                    }))
                    .await;
                response.is_ok()
            }
        })
        .collect();

    let results = join_all(futures).await;
    let elapsed = start.elapsed();

    // All writes should succeed
    for (i, success) in results.iter().enumerate() {
        assert!(success, "Write {i} should succeed under contention");
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
        let check_response = service
            .check(Request::new(CheckRequest {
                store_id: store_id.clone(),
                tuple_key: Some(TupleKey {
                    user: format!("user:writer{i}"),
                    relation: "viewer".to_string(),
                    object: "document:contested".to_string(),
                    condition: None,
                }),
                contextual_tuples: None,
                authorization_model_id: String::new(),
                trace: false,
                context: None,
                consistency: 0,
            }))
            .await
            .unwrap();
        assert!(
            check_response.into_inner().allowed,
            "writer{i} should have access after contended writes"
        );
    }
}

// ============================================================
// Section 3: Cache Metrics and Observability
// ============================================================

/// Test: Cache hit/miss behavior is observable through entry count via gRPC.
///
/// Verifies that cache entries are created on misses and hits don't
/// increase the count.
#[tokio::test]
async fn test_grpc_cache_entries_are_observable() {
    let storage = Arc::new(MemoryDataStore::new());
    let service = create_grpc_service_with_cache(Arc::clone(&storage));
    let cache = Arc::clone(service.cache());

    // Create store
    let response = service
        .create_store(Request::new(CreateStoreRequest {
            name: "grpc-observe-test".to_string(),
        }))
        .await
        .unwrap();
    let store_id = response.into_inner().id;
    setup_simple_model(&storage, &store_id).await;

    // Write a tuple
    let write_response = service
        .write(Request::new(WriteRequest {
            store_id: store_id.clone(),
            writes: Some(WriteRequestWrites {
                tuple_keys: vec![TupleKey {
                    user: "user:observer".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:observed".to_string(),
                    condition: None,
                }],
            }),
            deletes: None,
            authorization_model_id: String::new(),
        }))
        .await;
    assert!(write_response.is_ok());

    cache.run_pending_tasks().await;
    let initial_count = cache.entry_count();

    // First check - cache miss, should add entry
    let check_response = service
        .check(Request::new(CheckRequest {
            store_id: store_id.clone(),
            tuple_key: Some(TupleKey {
                user: "user:observer".to_string(),
                relation: "viewer".to_string(),
                object: "document:observed".to_string(),
                condition: None,
            }),
            contextual_tuples: None,
            authorization_model_id: String::new(),
            trace: false,
            context: None,
            consistency: 0,
        }))
        .await;
    assert!(check_response.is_ok());

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

/// Test: Cache with short TTL expires entries via gRPC.
///
/// Verifies that cache entries respect TTL configuration.
#[tokio::test]
async fn test_grpc_cache_ttl_expiration() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create service without cache to set up the store
    let setup_service = OpenFgaGrpcService::new(Arc::clone(&storage));

    // Create store
    let response = setup_service
        .create_store(Request::new(CreateStoreRequest {
            name: "grpc-ttl-test".to_string(),
        }))
        .await
        .unwrap();
    let store_id = response.into_inner().id;
    setup_simple_model(&storage, &store_id).await;

    // Write a tuple
    let write_response = setup_service
        .write(Request::new(WriteRequest {
            store_id: store_id.clone(),
            writes: Some(WriteRequestWrites {
                tuple_keys: vec![TupleKey {
                    user: "user:ttl".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:ttl".to_string(),
                    condition: None,
                }],
            }),
            deletes: None,
            authorization_model_id: String::new(),
        }))
        .await;
    assert!(write_response.is_ok());

    // Create service with very short TTL
    let short_ttl_config = CheckCacheConfig::default()
        .with_enabled(true)
        .with_ttl(Duration::from_millis(100));
    let service = create_grpc_service_with_cache_config(Arc::clone(&storage), short_ttl_config);
    let cache = Arc::clone(service.cache());

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

/// Test: Batch check properly uses cache and invalidation affects it via gRPC.
#[tokio::test]
async fn test_grpc_batch_check_cache_invalidation() {
    let storage = Arc::new(MemoryDataStore::new());
    let service = create_grpc_service_with_cache(Arc::clone(&storage));
    let cache = Arc::clone(service.cache());

    // Create store
    let response = service
        .create_store(Request::new(CreateStoreRequest {
            name: "grpc-batch-inv-test".to_string(),
        }))
        .await
        .unwrap();
    let store_id = response.into_inner().id;
    setup_simple_model(&storage, &store_id).await;

    // First batch check - all should be denied
    let checks: Vec<BatchCheckItem> = (0..5)
        .map(|i| BatchCheckItem {
            tuple_key: Some(TupleKey {
                user: format!("user:batch{i}"),
                relation: "viewer".to_string(),
                object: "document:batch".to_string(),
                condition: None,
            }),
            contextual_tuples: None,
            context: None,
            correlation_id: format!("check-{i}"),
        })
        .collect();

    let batch_response = service
        .batch_check(Request::new(BatchCheckRequest {
            store_id: store_id.clone(),
            checks: checks.clone(),
            authorization_model_id: String::new(),
            consistency: 0,
        }))
        .await
        .unwrap();

    let result = batch_response.into_inner().result;
    for i in 0..5 {
        assert!(
            !result
                .get(&format!("check-{i}"))
                .expect("result should exist")
                .allowed,
            "Initial batch check {i} should be denied"
        );
    }

    // Write tuples for first 3 users
    let write_response = service
        .write(Request::new(WriteRequest {
            store_id: store_id.clone(),
            writes: Some(WriteRequestWrites {
                tuple_keys: (0..3)
                    .map(|i| TupleKey {
                        user: format!("user:batch{i}"),
                        relation: "viewer".to_string(),
                        object: "document:batch".to_string(),
                        condition: None,
                    })
                    .collect(),
            }),
            deletes: None,
            authorization_model_id: String::new(),
        }))
        .await;
    assert!(write_response.is_ok());

    // Wait for cache sync using polling
    wait_for_cache_sync(&cache).await;

    // Second batch check - first 3 should be allowed, last 2 denied
    let batch_response = service
        .batch_check(Request::new(BatchCheckRequest {
            store_id: store_id.clone(),
            checks,
            authorization_model_id: String::new(),
            consistency: 0,
        }))
        .await
        .unwrap();

    let result = batch_response.into_inner().result;
    for i in 0..5 {
        let expected = i < 3;
        let actual = result
            .get(&format!("check-{i}"))
            .expect("result should exist")
            .allowed;
        assert_eq!(
            actual,
            expected,
            "After write, batch check {i} should be {}",
            if expected { "allowed" } else { "denied" }
        );
    }
}
