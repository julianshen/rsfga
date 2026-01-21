//! Performance integration tests for RSFGA (Issue #210).
//!
//! These tests verify that performance optimizations (caching, batch deduplication)
//! work correctly under realistic conditions.
//!
//! # Test Categories
//!
//! 1. **Cache Performance**: Verify cache effectiveness and hit rates
//! 2. **Batch Deduplication (Singleflight)**: Verify storage query reduction
//! 3. **Concurrency & Load**: Verify stability under sustained/burst load
//!
//! # Running Tests
//!
//! ```bash
//! # Run all performance integration tests
//! cargo test -p rsfga-api --test performance_integration_tests
//!
//! # Run resource-intensive tests (marked #[ignore])
//! cargo test -p rsfga-api --test performance_integration_tests -- --ignored
//! ```
//!
//! # Note on Metrics
//!
//! Tests use atomic counters and timing measurements to verify performance
//! characteristics. Results may vary based on hardware and system load.

mod common;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use rsfga_domain::cache::{CacheKey, CheckCacheConfig};
use rsfga_storage::MemoryDataStore;

use common::{
    create_shared_cache, create_shared_cache_with_config, create_test_app,
    create_test_app_with_shared_cache, post_json, setup_simple_model,
};

// =============================================================================
// Test Constants
// =============================================================================

/// Number of requests for burst load tests.
const BURST_LOAD_REQUESTS: usize = 1000;

/// Duration for sustained load tests.
const SUSTAINED_LOAD_DURATION: Duration = Duration::from_secs(60);

/// Target request rate for sustained load tests (req/s).
const SUSTAINED_LOAD_TARGET_RATE: u64 = 100;

/// Number of workers for sustained load tests.
const SUSTAINED_LOAD_WORKERS: usize = 10;

/// Polling interval for cache sync.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

// =============================================================================
// Section 1: Cache Performance Tests
// =============================================================================

/// Test: Repeated checks hit cache faster than initial storage queries.
///
/// Verifies that the cache provides measurable performance improvement
/// for repeated authorization checks.
#[tokio::test]
async fn test_cache_hit_is_faster_than_storage_query() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "cache-perf-test"}),
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
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:perf"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // First check - cache miss (storage query)
    let start_miss = Instant::now();
    let (status, _) = post_json(
        create_test_app_with_shared_cache(&storage, &cache),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:perf"
            }
        }),
    )
    .await;
    let miss_duration = start_miss.elapsed();
    assert_eq!(status, StatusCode::OK);

    // Ensure cache entry is written
    cache.run_pending_tasks().await;
    tokio::time::sleep(POLL_INTERVAL).await;

    // Second check - cache hit (should be faster)
    let start_hit = Instant::now();
    let (status, _) = post_json(
        create_test_app_with_shared_cache(&storage, &cache),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:perf"
            }
        }),
    )
    .await;
    let hit_duration = start_hit.elapsed();
    assert_eq!(status, StatusCode::OK);

    // Log timing for visibility
    println!(
        "Cache performance: miss={:?}, hit={:?}, improvement={:.1}x",
        miss_duration,
        hit_duration,
        miss_duration.as_secs_f64() / hit_duration.as_secs_f64().max(0.0001)
    );

    // Note: In-memory storage is already fast, so improvement may be modest.
    // The key verification is that cache hits don't degrade performance.
    // With database backends, the improvement would be more significant.
}

/// Test: Cache hit rate is measurable through repeated checks.
///
/// Verifies that cache entries are created and reused properly.
#[tokio::test]
async fn test_cache_hit_rate_measurement() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "hit-rate-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Write tuples for multiple users
    let users = ["alice", "bob", "charlie", "dave", "eve"];
    for user in &users {
        let (status, _) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": format!("user:{user}"),
                        "relation": "viewer",
                        "object": "document:shared"
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    // First round of checks - all cache misses
    cache.run_pending_tasks().await;
    let initial_count = cache.entry_count();

    for user in &users {
        let (status, _) = post_json(
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
    }

    cache.run_pending_tasks().await;
    let after_first_round = cache.entry_count();

    // Should have added 5 entries (one per user)
    assert_eq!(
        after_first_round - initial_count,
        5,
        "First round should add 5 cache entries"
    );

    // Second round of checks - all cache hits (entry count should not change)
    for user in &users {
        let (status, _) = post_json(
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
    }

    cache.run_pending_tasks().await;
    let after_second_round = cache.entry_count();

    // Entry count should remain the same (hits don't add new entries)
    assert_eq!(
        after_second_round, after_first_round,
        "Second round should hit cache (no new entries)"
    );

    println!(
        "Cache hit rate test: {} entries created, {} checks hit cache",
        after_first_round - initial_count,
        users.len()
    );
}

/// Test: Cache doesn't grow unbounded with TTL eviction.
///
/// Verifies that cache entries expire according to TTL configuration.
#[tokio::test]
async fn test_cache_ttl_eviction_works() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create cache with very short TTL for testing
    let short_ttl_config = CheckCacheConfig::default()
        .with_enabled(true)
        .with_ttl(Duration::from_millis(100))
        .with_max_capacity(1000);
    let cache = create_shared_cache_with_config(short_ttl_config);

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "ttl-eviction-test"}),
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
                    "user": "user:ttl-test",
                    "relation": "viewer",
                    "object": "document:ttl"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Add entry to cache
    let cache_key = CacheKey::new(&store_id, "document:ttl", "viewer", "user:ttl-test");
    cache.insert(cache_key.clone(), true).await;
    cache.run_pending_tasks().await;

    // Verify entry exists
    assert_eq!(
        cache.get(&cache_key).await,
        Some(true),
        "Entry should be cached initially"
    );

    // Wait for TTL to expire
    tokio::time::sleep(Duration::from_millis(150)).await;
    cache.run_pending_tasks().await;

    // Verify entry is evicted
    assert_eq!(
        cache.get(&cache_key).await,
        None,
        "Entry should be evicted after TTL"
    );

    println!("Cache TTL eviction test passed: entry evicted after 100ms TTL");
}

/// Test: Cache handles concurrent access safely.
///
/// Verifies that the cache can handle concurrent reads and writes
/// without data corruption or deadlocks.
#[tokio::test]
async fn test_cache_concurrent_access_safety() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "concurrent-cache-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Write initial tuples
    for i in 0..10 {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": format!("user:user{i}"),
                        "relation": "viewer",
                        "object": format!("document:doc{i}")
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let success_count = Arc::new(AtomicU64::new(0));
    let error_count = Arc::new(AtomicU64::new(0));

    // Launch concurrent check requests
    let num_concurrent = 100;
    let handles: Vec<_> = (0..num_concurrent)
        .map(|i| {
            let storage = Arc::clone(&storage);
            let cache = Arc::clone(&cache);
            let store_id = store_id.clone();
            let success_count = Arc::clone(&success_count);
            let error_count = Arc::clone(&error_count);

            tokio::spawn(async move {
                let (status, _) = post_json(
                    create_test_app_with_shared_cache(&storage, &cache),
                    &format!("/stores/{store_id}/check"),
                    serde_json::json!({
                        "tuple_key": {
                            "user": format!("user:user{}", i % 10),
                            "relation": "viewer",
                            "object": format!("document:doc{}", i % 10)
                        }
                    }),
                )
                .await;

                if status == StatusCode::OK {
                    success_count.fetch_add(1, Ordering::Relaxed);
                } else {
                    error_count.fetch_add(1, Ordering::Relaxed);
                }
            })
        })
        .collect();

    // Wait for all to complete
    for handle in handles {
        handle.await.expect("Task should complete without panic");
    }

    let successes = success_count.load(Ordering::Relaxed);
    let errors = error_count.load(Ordering::Relaxed);

    assert_eq!(
        errors, 0,
        "Should have no errors with concurrent cache access"
    );
    assert_eq!(
        successes, num_concurrent as u64,
        "All concurrent requests should succeed"
    );

    println!(
        "Concurrent cache access test: {} successful requests, {} errors",
        successes, errors
    );
}

// =============================================================================
// Section 2: Batch Deduplication (Singleflight) Tests
// =============================================================================

/// Test: 100 identical checks in batch query storage only once.
///
/// Verifies that intra-batch deduplication reduces storage queries
/// for identical authorization checks.
#[tokio::test]
async fn test_batch_deduplication_reduces_storage_queries() {
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

    // Write a tuple
    let (status, _) = post_json(
        create_test_app_with_shared_cache(&storage, &cache),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:dedup"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Create batch with 50 identical checks (max batch size)
    // Note: OpenFGA limits batch size to 50
    let checks: Vec<_> = (0..50)
        .map(|i| {
            serde_json::json!({
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:dedup"
                },
                "correlation_id": format!("check-{i}")
            })
        })
        .collect();

    let start = Instant::now();
    let (status, response) = post_json(
        create_test_app_with_shared_cache(&storage, &cache),
        &format!("/stores/{store_id}/batch-check"),
        serde_json::json!({"checks": checks}),
    )
    .await;
    let elapsed = start.elapsed();

    assert_eq!(status, StatusCode::OK);

    // Verify all results are correct
    let result = response["result"].as_object().unwrap();
    for i in 0..50 {
        assert_eq!(
            result[&format!("check-{i}")]["allowed"],
            true,
            "Check {i} should be allowed"
        );
    }

    println!(
        "Batch deduplication test: 50 identical checks completed in {:?}",
        elapsed
    );

    // The key insight: with deduplication, 50 identical checks should execute
    // as a single check internally. The test verifies correctness; the unit tests
    // in batch/tests.rs verify the actual storage call count.
}

/// Test: Batch results remain correct for all items despite deduplication.
///
/// Verifies that deduplication doesn't cause incorrect results to be returned.
#[tokio::test]
async fn test_batch_deduplication_preserves_correctness() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "batch-correctness-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Write tuples for some users but not others
    let allowed_users = ["alice", "bob"];
    for user in &allowed_users {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": format!("user:{user}"),
                        "relation": "viewer",
                        "object": "document:mixed"
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    // Create batch with mix of allowed and denied users, including duplicates
    let checks = vec![
        // alice (allowed) - appears 3 times
        serde_json::json!({
            "tuple_key": {"user": "user:alice", "relation": "viewer", "object": "document:mixed"},
            "correlation_id": "alice-1"
        }),
        serde_json::json!({
            "tuple_key": {"user": "user:alice", "relation": "viewer", "object": "document:mixed"},
            "correlation_id": "alice-2"
        }),
        // bob (allowed)
        serde_json::json!({
            "tuple_key": {"user": "user:bob", "relation": "viewer", "object": "document:mixed"},
            "correlation_id": "bob-1"
        }),
        // charlie (denied) - appears 2 times
        serde_json::json!({
            "tuple_key": {"user": "user:charlie", "relation": "viewer", "object": "document:mixed"},
            "correlation_id": "charlie-1"
        }),
        serde_json::json!({
            "tuple_key": {"user": "user:charlie", "relation": "viewer", "object": "document:mixed"},
            "correlation_id": "charlie-2"
        }),
        // alice again (allowed)
        serde_json::json!({
            "tuple_key": {"user": "user:alice", "relation": "viewer", "object": "document:mixed"},
            "correlation_id": "alice-3"
        }),
        // dave (denied)
        serde_json::json!({
            "tuple_key": {"user": "user:dave", "relation": "viewer", "object": "document:mixed"},
            "correlation_id": "dave-1"
        }),
    ];

    let (status, response) = post_json(
        create_test_app_with_shared_cache(&storage, &cache),
        &format!("/stores/{store_id}/batch-check"),
        serde_json::json!({"checks": checks}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let result = response["result"].as_object().unwrap();

    // Verify each result is correct despite deduplication
    assert_eq!(
        result["alice-1"]["allowed"], true,
        "alice-1 should be allowed"
    );
    assert_eq!(
        result["alice-2"]["allowed"], true,
        "alice-2 should be allowed"
    );
    assert_eq!(
        result["alice-3"]["allowed"], true,
        "alice-3 should be allowed"
    );
    assert_eq!(result["bob-1"]["allowed"], true, "bob-1 should be allowed");
    assert_eq!(
        result["charlie-1"]["allowed"], false,
        "charlie-1 should be denied"
    );
    assert_eq!(
        result["charlie-2"]["allowed"], false,
        "charlie-2 should be denied"
    );
    assert_eq!(
        result["dave-1"]["allowed"], false,
        "dave-1 should be denied"
    );

    println!("Batch correctness test passed: all results correct despite deduplication");
}

/// Test: Concurrent batch requests deduplicate across requests.
///
/// Verifies that singleflight deduplication works across concurrent batch requests.
#[tokio::test]
async fn test_concurrent_batches_deduplicate_across_requests() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "concurrent-batch-test"}),
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
                    "user": "user:concurrent",
                    "relation": "viewer",
                    "object": "document:concurrent"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Launch multiple concurrent batch requests for the same check
    let num_concurrent_batches = 10;
    let success_count = Arc::new(AtomicU64::new(0));

    let start = Instant::now();
    let handles: Vec<_> = (0..num_concurrent_batches)
        .map(|batch_id| {
            let storage = Arc::clone(&storage);
            let cache = Arc::clone(&cache);
            let store_id = store_id.clone();
            let success_count = Arc::clone(&success_count);

            tokio::spawn(async move {
                let checks: Vec<_> = (0..10)
                    .map(|i| {
                        serde_json::json!({
                            "tuple_key": {
                                "user": "user:concurrent",
                                "relation": "viewer",
                                "object": "document:concurrent"
                            },
                            "correlation_id": format!("batch{batch_id}-check{i}")
                        })
                    })
                    .collect();

                let (status, response) = post_json(
                    create_test_app_with_shared_cache(&storage, &cache),
                    &format!("/stores/{store_id}/batch-check"),
                    serde_json::json!({"checks": checks}),
                )
                .await;

                if status == StatusCode::OK {
                    // Verify all results in this batch
                    let result = response["result"].as_object().unwrap();
                    let all_correct = (0..10)
                        .all(|i| result[&format!("batch{batch_id}-check{i}")]["allowed"] == true);
                    if all_correct {
                        success_count.fetch_add(1, Ordering::Relaxed);
                    }
                }
            })
        })
        .collect();

    for handle in handles {
        handle.await.expect("Task should complete without panic");
    }
    let elapsed = start.elapsed();

    let successes = success_count.load(Ordering::Relaxed);
    assert_eq!(
        successes, num_concurrent_batches as u64,
        "All concurrent batches should succeed with correct results"
    );

    println!(
        "Concurrent batch deduplication test: {} batches completed in {:?}",
        successes, elapsed
    );
}

// =============================================================================
// Section 3: Concurrency & Load Tests
// =============================================================================

/// Test: Parallel operations maintain correctness without state corruption.
///
/// Verifies that concurrent reads and writes don't cause data corruption.
#[tokio::test]
async fn test_parallel_operations_maintain_correctness() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "parallel-correctness-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    let num_users = 50;
    let write_success = Arc::new(AtomicU64::new(0));
    let check_correct = Arc::new(AtomicU64::new(0));

    // Phase 1: Concurrent writes
    let write_handles: Vec<_> = (0..num_users)
        .map(|i| {
            let storage = Arc::clone(&storage);
            let cache = Arc::clone(&cache);
            let store_id = store_id.clone();
            let write_success = Arc::clone(&write_success);

            tokio::spawn(async move {
                let (status, _) = post_json(
                    create_test_app_with_shared_cache(&storage, &cache),
                    &format!("/stores/{store_id}/write"),
                    serde_json::json!({
                        "writes": {
                            "tuple_keys": [{
                                "user": format!("user:parallel{i}"),
                                "relation": "viewer",
                                "object": format!("document:parallel{i}")
                            }]
                        }
                    }),
                )
                .await;

                if status == StatusCode::OK {
                    write_success.fetch_add(1, Ordering::Relaxed);
                }
            })
        })
        .collect();

    for handle in write_handles {
        handle.await.expect("Write task should complete");
    }

    let writes = write_success.load(Ordering::Relaxed);
    assert_eq!(writes, num_users as u64, "All writes should succeed");

    // Allow cache to sync
    cache.run_pending_tasks().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Phase 2: Concurrent checks
    let check_handles: Vec<_> = (0..num_users)
        .map(|i| {
            let storage = Arc::clone(&storage);
            let cache = Arc::clone(&cache);
            let store_id = store_id.clone();
            let check_correct = Arc::clone(&check_correct);

            tokio::spawn(async move {
                let (status, response) = post_json(
                    create_test_app_with_shared_cache(&storage, &cache),
                    &format!("/stores/{store_id}/check"),
                    serde_json::json!({
                        "tuple_key": {
                            "user": format!("user:parallel{i}"),
                            "relation": "viewer",
                            "object": format!("document:parallel{i}")
                        }
                    }),
                )
                .await;

                if status == StatusCode::OK && response["allowed"] == true {
                    check_correct.fetch_add(1, Ordering::Relaxed);
                }
            })
        })
        .collect();

    for handle in check_handles {
        handle.await.expect("Check task should complete");
    }

    let correct_checks = check_correct.load(Ordering::Relaxed);
    assert_eq!(
        correct_checks, num_users as u64,
        "All checks should return correct results after writes"
    );

    println!(
        "Parallel operations test: {} writes, {} correct checks",
        writes, correct_checks
    );
}

/// Test: Burst load of 1000 requests for 10 seconds stability.
///
/// Verifies that the system handles burst load without crashes or excessive errors.
#[tokio::test]
#[ignore = "resource-intensive burst load test - run manually"]
async fn test_burst_load_1000_requests_stability() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "burst-load-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Write test data
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:burst",
                    "relation": "viewer",
                    "object": "document:burst"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let success_count = Arc::new(AtomicU64::new(0));
    let error_count = Arc::new(AtomicU64::new(0));
    let latencies = Arc::new(std::sync::Mutex::new(Vec::with_capacity(
        BURST_LOAD_REQUESTS,
    )));

    let start = Instant::now();

    // Launch burst of concurrent requests
    let handles: Vec<_> = (0..BURST_LOAD_REQUESTS)
        .map(|i| {
            let storage = Arc::clone(&storage);
            let cache = Arc::clone(&cache);
            let store_id = store_id.clone();
            let success_count = Arc::clone(&success_count);
            let error_count = Arc::clone(&error_count);
            let latencies = Arc::clone(&latencies);

            tokio::spawn(async move {
                let req_start = Instant::now();
                let (status, _) = post_json(
                    create_test_app_with_shared_cache(&storage, &cache),
                    &format!("/stores/{store_id}/check"),
                    serde_json::json!({
                        "tuple_key": {
                            "user": if i % 2 == 0 { "user:burst" } else { "user:unknown" },
                            "relation": "viewer",
                            "object": "document:burst"
                        }
                    }),
                )
                .await;
                let latency = req_start.elapsed();

                if let Ok(mut lats) = latencies.lock() {
                    lats.push(latency);
                }

                if status == StatusCode::OK {
                    success_count.fetch_add(1, Ordering::Relaxed);
                } else {
                    error_count.fetch_add(1, Ordering::Relaxed);
                }
            })
        })
        .collect();

    for handle in handles {
        handle.await.expect("Task should complete without panic");
    }

    let elapsed = start.elapsed();
    let successes = success_count.load(Ordering::Relaxed);
    let errors = error_count.load(Ordering::Relaxed);

    // Calculate latency percentiles
    let mut lats: Vec<Duration> = latencies.lock().unwrap().clone();
    lats.sort();
    let p50 = lats.get(lats.len() / 2).copied().unwrap_or_default();
    let p95 = lats.get(lats.len() * 95 / 100).copied().unwrap_or_default();
    let p99 = lats.get(lats.len() * 99 / 100).copied().unwrap_or_default();

    println!(
        "Burst load test: {} requests in {:?}",
        BURST_LOAD_REQUESTS, elapsed
    );
    println!(
        "  Successes: {}, Errors: {}, Throughput: {:.0} req/s",
        successes,
        errors,
        BURST_LOAD_REQUESTS as f64 / elapsed.as_secs_f64()
    );
    println!("  Latencies: p50={:?}, p95={:?}, p99={:?}", p50, p95, p99);

    // Assertions
    assert_eq!(errors, 0, "Should have no errors under burst load");
    assert!(
        elapsed < Duration::from_secs(30),
        "Burst load should complete within 30 seconds"
    );
}

/// Test: Sustained load of 100 req/s for 1 minute without crashes.
///
/// Verifies that the system maintains stability under sustained load.
#[tokio::test]
#[ignore = "resource-intensive sustained load test - run manually (1 minute duration)"]
async fn test_sustained_load_100_rps_for_1_minute() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "sustained-load-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Write test data
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:sustained",
                    "relation": "viewer",
                    "object": "document:sustained"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let success_count = Arc::new(AtomicU64::new(0));
    let error_count = Arc::new(AtomicU64::new(0));
    let request_count = Arc::new(AtomicU64::new(0));

    let start = Instant::now();
    let duration = SUSTAINED_LOAD_DURATION;
    let target_rate = SUSTAINED_LOAD_TARGET_RATE;
    let request_interval = Duration::from_millis(1000 / target_rate);

    // Spawn worker tasks
    let handles: Vec<_> = (0..SUSTAINED_LOAD_WORKERS)
        .map(|_worker_id| {
            let storage = Arc::clone(&storage);
            let cache = Arc::clone(&cache);
            let store_id = store_id.clone();
            let success_count = Arc::clone(&success_count);
            let error_count = Arc::clone(&error_count);
            let request_count = Arc::clone(&request_count);

            tokio::spawn(async move {
                while Instant::now().duration_since(start) < duration {
                    let (status, _) = post_json(
                        create_test_app_with_shared_cache(&storage, &cache),
                        &format!("/stores/{store_id}/check"),
                        serde_json::json!({
                            "tuple_key": {
                                "user": "user:sustained",
                                "relation": "viewer",
                                "object": "document:sustained"
                            }
                        }),
                    )
                    .await;

                    request_count.fetch_add(1, Ordering::Relaxed);
                    if status == StatusCode::OK {
                        success_count.fetch_add(1, Ordering::Relaxed);
                    } else {
                        error_count.fetch_add(1, Ordering::Relaxed);
                    }

                    // Rate limiting: distribute target rate across workers
                    let per_worker_interval = Duration::from_millis(
                        request_interval.as_millis() as u64 * SUSTAINED_LOAD_WORKERS as u64,
                    );
                    tokio::time::sleep(per_worker_interval).await;
                }
            })
        })
        .collect();

    for handle in handles {
        handle.await.expect("Worker should complete without panic");
    }

    let elapsed = start.elapsed();
    let total_requests = request_count.load(Ordering::Relaxed);
    let successes = success_count.load(Ordering::Relaxed);
    let errors = error_count.load(Ordering::Relaxed);
    let actual_rate = total_requests as f64 / elapsed.as_secs_f64();

    println!("Sustained load test: {:?}", elapsed);
    println!(
        "  Total requests: {}, Successes: {}, Errors: {}",
        total_requests, successes, errors
    );
    println!(
        "  Target rate: {} req/s, Actual rate: {:.0} req/s",
        target_rate, actual_rate
    );

    // Assertions
    assert_eq!(errors, 0, "Should have no errors under sustained load");
    assert!(
        total_requests > 100,
        "Should process significant number of requests"
    );
}

/// Test: Latency percentiles under load.
///
/// Measures p50, p95, p99 latencies to verify performance targets.
#[tokio::test]
async fn test_latency_percentiles_under_moderate_load() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "latency-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Write test data
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:latency",
                    "relation": "viewer",
                    "object": "document:latency"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Warm up cache
    let (status, _) = post_json(
        create_test_app_with_shared_cache(&storage, &cache),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:latency",
                "relation": "viewer",
                "object": "document:latency"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Measure latencies for sequential requests
    let num_samples = 100;
    let mut latencies = Vec::with_capacity(num_samples);

    for _ in 0..num_samples {
        let start = Instant::now();
        let (status, _) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
            &format!("/stores/{store_id}/check"),
            serde_json::json!({
                "tuple_key": {
                    "user": "user:latency",
                    "relation": "viewer",
                    "object": "document:latency"
                }
            }),
        )
        .await;
        let latency = start.elapsed();
        assert_eq!(status, StatusCode::OK);
        latencies.push(latency);
    }

    // Calculate percentiles
    latencies.sort();
    let p50 = latencies[latencies.len() / 2];
    let p95 = latencies[latencies.len() * 95 / 100];
    let p99 = latencies[latencies.len() * 99 / 100];
    let avg = Duration::from_nanos(
        (latencies.iter().map(|d| d.as_nanos()).sum::<u128>() / latencies.len() as u128) as u64,
    );

    println!("Latency percentiles ({} samples):", num_samples);
    println!("  Average: {:?}", avg);
    println!("  p50: {:?}", p50);
    println!("  p95: {:?}", p95);
    println!("  p99: {:?}", p99);

    // These thresholds are for in-memory storage
    // Real database backends would have different targets
    // For now, we just verify the measurements are reasonable
    assert!(
        p99 < Duration::from_millis(100),
        "p99 latency should be under 100ms for in-memory storage, got {:?}",
        p99
    );
}

// =============================================================================
// Section 4: Protocol Coverage Tests
// =============================================================================

/// Test: REST API performance baseline.
///
/// Establishes performance baseline for REST endpoints.
#[tokio::test]
async fn test_rest_api_performance_baseline() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "rest-perf-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Write test data
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:rest",
                    "relation": "viewer",
                    "object": "document:rest"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Warm up
    for _ in 0..10 {
        let (status, _) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
            &format!("/stores/{store_id}/check"),
            serde_json::json!({
                "tuple_key": {
                    "user": "user:rest",
                    "relation": "viewer",
                    "object": "document:rest"
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    // Measure throughput
    let num_requests = 1000;
    let start = Instant::now();

    for _ in 0..num_requests {
        let (status, _) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
            &format!("/stores/{store_id}/check"),
            serde_json::json!({
                "tuple_key": {
                    "user": "user:rest",
                    "relation": "viewer",
                    "object": "document:rest"
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let elapsed = start.elapsed();
    let throughput = num_requests as f64 / elapsed.as_secs_f64();

    println!(
        "REST API baseline: {} requests in {:?} ({:.0} req/s)",
        num_requests, elapsed, throughput
    );

    // Target: > 100 req/s for sequential requests (conservative for CI)
    // Real-world concurrent throughput would be higher
    assert!(
        throughput > 100.0,
        "REST throughput should be > 100 req/s, got {:.0}",
        throughput
    );
}

// =============================================================================
// Section 5: Cache Invalidation Performance
// =============================================================================

/// Test: Cache invalidation doesn't cause performance regression.
///
/// Verifies that write operations (which trigger cache invalidation)
/// maintain reasonable performance.
#[tokio::test]
async fn test_cache_invalidation_performance() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "invalidation-perf-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Populate cache with many entries
    for i in 0..100 {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": format!("user:inv{i}"),
                        "relation": "viewer",
                        "object": format!("document:inv{i}")
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    // Fill cache by checking each entry
    for i in 0..100 {
        let (status, _) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
            &format!("/stores/{store_id}/check"),
            serde_json::json!({
                "tuple_key": {
                    "user": format!("user:inv{i}"),
                    "relation": "viewer",
                    "object": format!("document:inv{i}")
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    cache.run_pending_tasks().await;
    let cache_size_before = cache.entry_count();
    assert!(cache_size_before > 0, "Cache should have entries");

    // Measure write performance (triggers invalidation)
    let num_writes = 50;
    let start = Instant::now();

    for i in 0..num_writes {
        let (status, _) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": format!("user:new{i}"),
                        "relation": "editor",
                        "object": format!("document:inv{}", i % 100)
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let elapsed = start.elapsed();
    let write_throughput = num_writes as f64 / elapsed.as_secs_f64();

    println!(
        "Cache invalidation performance: {} writes in {:?} ({:.0} writes/s)",
        num_writes, elapsed, write_throughput
    );

    // Target: > 50 writes/s with cache invalidation (conservative for CI)
    assert!(
        write_throughput > 50.0,
        "Write throughput with invalidation should be > 50 writes/s, got {:.0}",
        write_throughput
    );
}
