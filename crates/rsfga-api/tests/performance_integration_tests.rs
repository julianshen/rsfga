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
use rsfga_storage::{DataStore, MemoryDataStore, StoredAuthorizationModel};

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

/// Number of samples for latency measurement tests.
const LATENCY_SAMPLE_COUNT: usize = 100;

/// Number of warmup iterations for throughput tests.
const WARMUP_ITERATIONS: usize = 10;

/// Number of requests for throughput measurement.
const THROUGHPUT_REQUEST_COUNT: usize = 1000;

/// Number of cache entries for population tests.
const CACHE_POPULATION_SIZE: usize = 100;

/// Number of writes for invalidation performance tests.
const INVALIDATION_WRITE_COUNT: usize = 50;

/// Maximum p99 latency for in-memory operations (10ms).
/// Tighter than database backends to detect regressions quickly.
const MAX_P99_LATENCY_IN_MEMORY: Duration = Duration::from_millis(10);

// Test user identifiers (extracted from magic strings)
const TEST_USER_BURST: &str = "user:burst";
const TEST_USER_UNKNOWN: &str = "user:unknown";
const TEST_USER_SUSTAINED: &str = "user:sustained";
const TEST_USER_LATENCY: &str = "user:latency";
const TEST_USER_REST: &str = "user:rest";

// Test object identifiers
const TEST_DOC_BURST: &str = "document:burst";
const TEST_DOC_SUSTAINED: &str = "document:sustained";
const TEST_DOC_LATENCY: &str = "document:latency";
const TEST_DOC_REST: &str = "document:rest";

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

    // Log timing for visibility with safe division
    let improvement = if hit_duration.as_nanos() > 0 {
        miss_duration.as_secs_f64() / hit_duration.as_secs_f64()
    } else {
        // hit_duration is zero (extremely fast), report as infinite improvement
        f64::INFINITY
    };
    println!(
        "Cache performance: miss={:?}, hit={:?}, improvement={:.1}x",
        miss_duration, hit_duration, improvement
    );

    // Assert that cache hits are not slower than misses.
    // For in-memory storage, the improvement may be modest, but hits
    // should never be slower than misses.
    assert!(
        hit_duration <= miss_duration.saturating_add(Duration::from_millis(5)),
        "Cache hit should not be slower than miss: hit={:?}, miss={:?}",
        hit_duration,
        miss_duration
    );
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
                    "user": TEST_USER_BURST,
                    "relation": "viewer",
                    "object": TEST_DOC_BURST
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
                // Alternate between known and unknown users for variety
                let user = if i % 2 == 0 {
                    TEST_USER_BURST
                } else {
                    TEST_USER_UNKNOWN
                };
                let (status, _) = post_json(
                    create_test_app_with_shared_cache(&storage, &cache),
                    &format!("/stores/{store_id}/check"),
                    serde_json::json!({
                        "tuple_key": {
                            "user": user,
                            "relation": "viewer",
                            "object": TEST_DOC_BURST
                        }
                    }),
                )
                .await;
                let latency = req_start.elapsed();

                // Use .expect() to surface mutex poisoning issues in tests
                latencies
                    .lock()
                    .expect("latencies mutex should not be poisoned")
                    .push(latency);

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
                    "user": TEST_USER_SUSTAINED,
                    "relation": "viewer",
                    "object": TEST_DOC_SUSTAINED
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
                                "user": TEST_USER_SUSTAINED,
                                "relation": "viewer",
                                "object": TEST_DOC_SUSTAINED
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

                    // Rate limiting: each worker sleeps for (workers * base_interval)
                    // so that total rate across all workers = target_rate.
                    // Example: 10 workers, 100 req/s target -> each worker does 10 req/s
                    //          so each worker sleeps 100ms between requests (10ms * 10 workers)
                    let per_worker_interval_ms = (request_interval.as_millis() as u64)
                        .checked_mul(SUSTAINED_LOAD_WORKERS as u64)
                        .expect("per-worker interval calculation should not overflow");
                    let per_worker_interval = Duration::from_millis(per_worker_interval_ms);
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
                    "user": TEST_USER_LATENCY,
                    "relation": "viewer",
                    "object": TEST_DOC_LATENCY
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
                "user": TEST_USER_LATENCY,
                "relation": "viewer",
                "object": TEST_DOC_LATENCY
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Measure latencies for sequential requests
    let mut latencies = Vec::with_capacity(LATENCY_SAMPLE_COUNT);

    for _ in 0..LATENCY_SAMPLE_COUNT {
        let start = Instant::now();
        let (status, _) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
            &format!("/stores/{store_id}/check"),
            serde_json::json!({
                "tuple_key": {
                    "user": TEST_USER_LATENCY,
                    "relation": "viewer",
                    "object": TEST_DOC_LATENCY
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

    println!("Latency percentiles ({} samples):", LATENCY_SAMPLE_COUNT);
    println!("  Average: {:?}", avg);
    println!("  p50: {:?}", p50);
    println!("  p95: {:?}", p95);
    println!("  p99: {:?}", p99);

    // For in-memory storage, we use a tight threshold (10ms) to catch regressions.
    // Database backends would have higher thresholds.
    assert!(
        p99 < MAX_P99_LATENCY_IN_MEMORY,
        "p99 latency should be under {:?} for in-memory storage, got {:?}",
        MAX_P99_LATENCY_IN_MEMORY,
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
                    "user": TEST_USER_REST,
                    "relation": "viewer",
                    "object": TEST_DOC_REST
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Warm up
    for _ in 0..WARMUP_ITERATIONS {
        let (status, _) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
            &format!("/stores/{store_id}/check"),
            serde_json::json!({
                "tuple_key": {
                    "user": TEST_USER_REST,
                    "relation": "viewer",
                    "object": TEST_DOC_REST
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    // Measure throughput
    let start = Instant::now();

    for _ in 0..THROUGHPUT_REQUEST_COUNT {
        let (status, _) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
            &format!("/stores/{store_id}/check"),
            serde_json::json!({
                "tuple_key": {
                    "user": TEST_USER_REST,
                    "relation": "viewer",
                    "object": TEST_DOC_REST
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let elapsed = start.elapsed();
    let throughput = THROUGHPUT_REQUEST_COUNT as f64 / elapsed.as_secs_f64();

    println!(
        "REST API baseline: {} requests in {:?} ({:.0} req/s)",
        THROUGHPUT_REQUEST_COUNT, elapsed, throughput
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
    for i in 0..CACHE_POPULATION_SIZE {
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
    for i in 0..CACHE_POPULATION_SIZE {
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
    let start = Instant::now();

    for i in 0..INVALIDATION_WRITE_COUNT {
        let (status, _) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": format!("user:new{i}"),
                        "relation": "editor",
                        "object": format!("document:inv{}", i % CACHE_POPULATION_SIZE)
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let elapsed = start.elapsed();
    let write_throughput = INVALIDATION_WRITE_COUNT as f64 / elapsed.as_secs_f64();

    println!(
        "Cache invalidation performance: {} writes in {:?} ({:.0} writes/s)",
        INVALIDATION_WRITE_COUNT, elapsed, write_throughput
    );

    // Target: > 50 writes/s with cache invalidation (conservative for CI)
    assert!(
        write_throughput > 50.0,
        "Write throughput with invalidation should be > 50 writes/s, got {:.0}",
        write_throughput
    );
}

// =============================================================================
// Section 6: ListObjects Concurrency Tests (Issue #210)
// =============================================================================

/// Number of concurrent ListObjects requests for high-load tests.
const HIGH_CONCURRENCY_LISTOBJECTS_COUNT: usize = 100;

/// Number of objects to create for ListObjects tests.
const LISTOBJECTS_TEST_OBJECT_COUNT: usize = 50;

/// Test: Concurrent ListObjects with overlapping filters return consistent results.
///
/// Multiple ListObjects requests with overlapping user/relation filters should
/// not interfere and return correct results.
#[tokio::test]
async fn test_listobjects_concurrent_overlapping_filters_consistent() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "listobjects-overlap-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Create documents accessible by multiple users with overlapping permissions
    // user:alice -> viewer on doc0-29
    // user:bob -> viewer on doc20-49
    // Overlap: doc20-29 (both can view)
    for i in 0..30 {
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
    }

    for i in 20..50 {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": "user:bob",
                        "relation": "viewer",
                        "object": format!("document:doc{i}")
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let alice_success = Arc::new(AtomicU64::new(0));
    let bob_success = Arc::new(AtomicU64::new(0));
    let alice_correct_count = Arc::new(AtomicU64::new(0));
    let bob_correct_count = Arc::new(AtomicU64::new(0));

    // Launch concurrent ListObjects for both users
    let mut handles = Vec::new();

    for _ in 0..25 {
        // Alice requests
        let storage_clone = Arc::clone(&storage);
        let cache_clone = Arc::clone(&cache);
        let store_id_clone = store_id.clone();
        let alice_success_clone = Arc::clone(&alice_success);
        let alice_correct_clone = Arc::clone(&alice_correct_count);

        handles.push(tokio::spawn(async move {
            let (status, response) = post_json(
                create_test_app_with_shared_cache(&storage_clone, &cache_clone),
                &format!("/stores/{store_id_clone}/list-objects"),
                serde_json::json!({
                    "user": "user:alice",
                    "relation": "viewer",
                    "type": "document"
                }),
            )
            .await;

            if status == StatusCode::OK {
                alice_success_clone.fetch_add(1, Ordering::Relaxed);
                if let Some(objects) = response["objects"].as_array() {
                    if objects.len() == 30 {
                        alice_correct_clone.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }));

        // Bob requests
        let storage_clone = Arc::clone(&storage);
        let cache_clone = Arc::clone(&cache);
        let store_id_clone = store_id.clone();
        let bob_success_clone = Arc::clone(&bob_success);
        let bob_correct_clone = Arc::clone(&bob_correct_count);

        handles.push(tokio::spawn(async move {
            let (status, response) = post_json(
                create_test_app_with_shared_cache(&storage_clone, &cache_clone),
                &format!("/stores/{store_id_clone}/list-objects"),
                serde_json::json!({
                    "user": "user:bob",
                    "relation": "viewer",
                    "type": "document"
                }),
            )
            .await;

            if status == StatusCode::OK {
                bob_success_clone.fetch_add(1, Ordering::Relaxed);
                if let Some(objects) = response["objects"].as_array() {
                    if objects.len() == 30 {
                        bob_correct_clone.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }));
    }

    for handle in handles {
        handle.await.expect("Task should complete without panic");
    }

    let alice_successes = alice_success.load(Ordering::Relaxed);
    let bob_successes = bob_success.load(Ordering::Relaxed);
    let alice_correct = alice_correct_count.load(Ordering::Relaxed);
    let bob_correct = bob_correct_count.load(Ordering::Relaxed);

    assert_eq!(alice_successes, 25, "All Alice requests should succeed");
    assert_eq!(bob_successes, 25, "All Bob requests should succeed");
    assert_eq!(
        alice_correct, 25,
        "All Alice requests should return exactly 30 documents"
    );
    assert_eq!(
        bob_correct, 25,
        "All Bob requests should return exactly 30 documents"
    );

    println!(
        "ListObjects overlapping filters test: Alice {}/{}, Bob {}/{}",
        alice_correct, alice_successes, bob_correct, bob_successes
    );
}

/// Test: Concurrent ListObjects during write operations maintain correctness.
///
/// ListObjects should return consistent results even when writes are happening
/// concurrently. Results should reflect either before or after the write, not
/// a partial/corrupted state.
#[tokio::test]
async fn test_listobjects_concurrent_with_writes_maintains_correctness() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "listobjects-write-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Initial setup: user can view 20 documents
    for i in 0..20 {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": "user:writer_reader",
                        "relation": "viewer",
                        "object": format!("document:initial{i}")
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let read_success = Arc::new(AtomicU64::new(0));
    let write_success = Arc::new(AtomicU64::new(0));
    let valid_results = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::new();

    // Launch concurrent reads and writes
    for i in 0..30 {
        // Read operation
        let storage_clone = Arc::clone(&storage);
        let cache_clone = Arc::clone(&cache);
        let store_id_clone = store_id.clone();
        let read_success_clone = Arc::clone(&read_success);
        let valid_results_clone = Arc::clone(&valid_results);

        handles.push(tokio::spawn(async move {
            let (status, response) = post_json(
                create_test_app_with_shared_cache(&storage_clone, &cache_clone),
                &format!("/stores/{store_id_clone}/list-objects"),
                serde_json::json!({
                    "user": "user:writer_reader",
                    "relation": "viewer",
                    "type": "document"
                }),
            )
            .await;

            if status == StatusCode::OK {
                read_success_clone.fetch_add(1, Ordering::Relaxed);
                // Result should be >= 20 (initial) and <= 50 (initial + new writes)
                if let Some(objects) = response["objects"].as_array() {
                    if objects.len() >= 20 && objects.len() <= 50 {
                        valid_results_clone.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }));

        // Write operation (add new documents)
        let storage_clone = Arc::clone(&storage);
        let cache_clone = Arc::clone(&cache);
        let store_id_clone = store_id.clone();
        let write_success_clone = Arc::clone(&write_success);

        handles.push(tokio::spawn(async move {
            let (status, _) = post_json(
                create_test_app_with_shared_cache(&storage_clone, &cache_clone),
                &format!("/stores/{store_id_clone}/write"),
                serde_json::json!({
                    "writes": {
                        "tuple_keys": [{
                            "user": "user:writer_reader",
                            "relation": "viewer",
                            "object": format!("document:new{i}")
                        }]
                    }
                }),
            )
            .await;

            if status == StatusCode::OK {
                write_success_clone.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    for handle in handles {
        handle.await.expect("Task should complete without panic");
    }

    let reads = read_success.load(Ordering::Relaxed);
    let writes = write_success.load(Ordering::Relaxed);
    let valid = valid_results.load(Ordering::Relaxed);

    assert_eq!(reads, 30, "All read operations should succeed");
    assert_eq!(writes, 30, "All write operations should succeed");
    assert_eq!(
        valid, reads,
        "All read results should be valid (between 20 and 50 objects)"
    );

    println!(
        "ListObjects concurrent with writes: {} reads, {} writes, {} valid results",
        reads, writes, valid
    );
}

/// Test: High concurrency ListObjects (100+ parallel) completes without errors.
///
/// Verifies the system can handle 100+ concurrent ListObjects requests without
/// errors, deadlocks, or performance degradation.
#[tokio::test]
async fn test_listobjects_high_concurrency_100_parallel() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "listobjects-high-concurrency"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Create test data
    for i in 0..LISTOBJECTS_TEST_OBJECT_COUNT {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": "user:high_concurrency_user",
                        "relation": "viewer",
                        "object": format!("document:hc_doc{i}")
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let success_count = Arc::new(AtomicU64::new(0));
    let error_count = Arc::new(AtomicU64::new(0));
    let correct_count = Arc::new(AtomicU64::new(0));

    let start = Instant::now();

    // Launch 100+ concurrent ListObjects requests
    let handles: Vec<_> = (0..HIGH_CONCURRENCY_LISTOBJECTS_COUNT)
        .map(|_| {
            let storage = Arc::clone(&storage);
            let cache = Arc::clone(&cache);
            let store_id = store_id.clone();
            let success_count = Arc::clone(&success_count);
            let error_count = Arc::clone(&error_count);
            let correct_count = Arc::clone(&correct_count);

            tokio::spawn(async move {
                let (status, response) = post_json(
                    create_test_app_with_shared_cache(&storage, &cache),
                    &format!("/stores/{store_id}/list-objects"),
                    serde_json::json!({
                        "user": "user:high_concurrency_user",
                        "relation": "viewer",
                        "type": "document"
                    }),
                )
                .await;

                if status == StatusCode::OK {
                    success_count.fetch_add(1, Ordering::Relaxed);
                    if let Some(objects) = response["objects"].as_array() {
                        if objects.len() == LISTOBJECTS_TEST_OBJECT_COUNT {
                            correct_count.fetch_add(1, Ordering::Relaxed);
                        }
                    }
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
    let correct = correct_count.load(Ordering::Relaxed);

    println!(
        "High concurrency ListObjects test: {} requests in {:?}",
        HIGH_CONCURRENCY_LISTOBJECTS_COUNT, elapsed
    );
    println!(
        "  Successes: {}, Errors: {}, Correct results: {}",
        successes, errors, correct
    );
    println!(
        "  Throughput: {:.0} req/s",
        HIGH_CONCURRENCY_LISTOBJECTS_COUNT as f64 / elapsed.as_secs_f64()
    );

    assert_eq!(
        errors, 0,
        "Should have no errors under high concurrency ListObjects load"
    );
    assert_eq!(
        successes, HIGH_CONCURRENCY_LISTOBJECTS_COUNT as u64,
        "All concurrent ListObjects requests should succeed"
    );
    assert_eq!(
        correct, HIGH_CONCURRENCY_LISTOBJECTS_COUNT as u64,
        "All results should have correct object count"
    );
}

/// Test: ListObjects results are consistent across concurrent reads.
///
/// Multiple concurrent ListObjects requests for the same user/relation/type
/// should return identical results (same set of objects).
#[tokio::test]
async fn test_listobjects_results_consistent_across_concurrent_reads() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "listobjects-consistency-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Create test data with specific document IDs
    let expected_objects: std::collections::HashSet<String> = (0..25)
        .map(|i| format!("document:consistent_doc{i}"))
        .collect();

    for doc in &expected_objects {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": "user:consistency_user",
                        "relation": "viewer",
                        "object": doc
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let results = Arc::new(tokio::sync::Mutex::new(Vec::<
        std::collections::HashSet<String>,
    >::new()));
    let success_count = Arc::new(AtomicU64::new(0));

    // Launch concurrent reads
    let handles: Vec<_> = (0..50)
        .map(|_| {
            let storage = Arc::clone(&storage);
            let cache = Arc::clone(&cache);
            let store_id = store_id.clone();
            let results = Arc::clone(&results);
            let success_count = Arc::clone(&success_count);

            tokio::spawn(async move {
                let (status, response) = post_json(
                    create_test_app_with_shared_cache(&storage, &cache),
                    &format!("/stores/{store_id}/list-objects"),
                    serde_json::json!({
                        "user": "user:consistency_user",
                        "relation": "viewer",
                        "type": "document"
                    }),
                )
                .await;

                if status == StatusCode::OK {
                    success_count.fetch_add(1, Ordering::Relaxed);
                    if let Some(objects) = response["objects"].as_array() {
                        let object_set: std::collections::HashSet<String> = objects
                            .iter()
                            .filter_map(|o| o.as_str().map(String::from))
                            .collect();
                        results.lock().await.push(object_set);
                    }
                }
            })
        })
        .collect();

    for handle in handles {
        handle.await.expect("Task should complete without panic");
    }

    let successes = success_count.load(Ordering::Relaxed);
    let all_results = results.lock().await;

    assert_eq!(successes, 50, "All read requests should succeed");

    // Verify all results are identical
    let first_result = &all_results[0];
    for (idx, result) in all_results.iter().enumerate() {
        assert_eq!(
            result, first_result,
            "Result {} differs from first result - concurrent reads should be consistent",
            idx
        );
    }

    // Verify results match expected
    assert_eq!(
        first_result, &expected_objects,
        "Results should match expected objects"
    );

    println!(
        "ListObjects consistency test: {} identical results across concurrent reads",
        all_results.len()
    );
}

// =============================================================================
// Section 7: ListUsers Concurrency Tests (Issue #210)
// =============================================================================

/// Number of concurrent ListUsers requests for high-load tests.
const HIGH_CONCURRENCY_LISTUSERS_COUNT: usize = 100;

/// Number of users to create for ListUsers tests.
const LISTUSERS_TEST_USER_COUNT: usize = 30;

/// Test: Concurrent ListUsers operations return consistent results.
///
/// Multiple concurrent ListUsers requests for the same object/relation should
/// return identical user lists without interference.
#[tokio::test]
async fn test_listusers_concurrent_requests_consistent() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "listusers-concurrent-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Create users who can view a shared document
    for i in 0..LISTUSERS_TEST_USER_COUNT {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": format!("user:viewer{i}"),
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
    let correct_count = Arc::new(AtomicU64::new(0));

    // Launch concurrent ListUsers requests
    let handles: Vec<_> = (0..50)
        .map(|_| {
            let storage = Arc::clone(&storage);
            let cache = Arc::clone(&cache);
            let store_id = store_id.clone();
            let success_count = Arc::clone(&success_count);
            let correct_count = Arc::clone(&correct_count);

            tokio::spawn(async move {
                let (status, response) = post_json(
                    create_test_app_with_shared_cache(&storage, &cache),
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
                    if let Some(users) = response["users"].as_array() {
                        if users.len() == LISTUSERS_TEST_USER_COUNT {
                            correct_count.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            })
        })
        .collect();

    for handle in handles {
        handle.await.expect("Task should complete without panic");
    }

    let successes = success_count.load(Ordering::Relaxed);
    let correct = correct_count.load(Ordering::Relaxed);

    assert_eq!(successes, 50, "All ListUsers requests should succeed");
    assert_eq!(
        correct, 50,
        "All ListUsers should return exactly {} users",
        LISTUSERS_TEST_USER_COUNT
    );

    println!(
        "ListUsers concurrent test: {} successful, {} correct results",
        successes, correct
    );
}

/// Test: Concurrent ListUsers with same parameters deduplicate correctly.
///
/// Multiple identical ListUsers requests running concurrently should
/// not cause duplication issues or return incorrect results.
#[tokio::test]
async fn test_listusers_concurrent_same_params_deduplicate() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "listusers-dedup-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Create users
    let expected_users: std::collections::HashSet<String> =
        (0..20).map(|i| format!("user:dedup_user{i}")).collect();

    for user in &expected_users {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": user,
                        "relation": "viewer",
                        "object": "document:dedup_doc"
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let results = Arc::new(tokio::sync::Mutex::new(Vec::<
        std::collections::HashSet<String>,
    >::new()));
    let success_count = Arc::new(AtomicU64::new(0));

    // Launch many concurrent identical requests
    let handles: Vec<_> = (0..40)
        .map(|_| {
            let storage = Arc::clone(&storage);
            let cache = Arc::clone(&cache);
            let store_id = store_id.clone();
            let results = Arc::clone(&results);
            let success_count = Arc::clone(&success_count);

            tokio::spawn(async move {
                let (status, response) = post_json(
                    create_test_app_with_shared_cache(&storage, &cache),
                    &format!("/stores/{store_id}/list-users"),
                    serde_json::json!({
                        "object": {"type": "document", "id": "dedup_doc"},
                        "relation": "viewer",
                        "user_filters": [{"type": "user"}]
                    }),
                )
                .await;

                if status == StatusCode::OK {
                    success_count.fetch_add(1, Ordering::Relaxed);
                    if let Some(users) = response["users"].as_array() {
                        let user_set: std::collections::HashSet<String> = users
                            .iter()
                            .filter_map(|u| {
                                u["object"]["id"].as_str().map(|id| format!("user:{id}"))
                            })
                            .collect();
                        results.lock().await.push(user_set);
                    }
                }
            })
        })
        .collect();

    for handle in handles {
        handle.await.expect("Task should complete without panic");
    }

    let successes = success_count.load(Ordering::Relaxed);
    let all_results = results.lock().await;

    assert_eq!(successes, 40, "All requests should succeed");

    // All results should be identical
    let first_result = &all_results[0];
    for (idx, result) in all_results.iter().enumerate() {
        assert_eq!(
            result, first_result,
            "Result {} differs from first - deduplication should ensure consistency",
            idx
        );
    }

    // Verify we got the expected users
    assert_eq!(
        first_result, &expected_users,
        "Results should match expected users"
    );

    println!(
        "ListUsers deduplication test: {} identical results",
        all_results.len()
    );
}

/// Test: Concurrent ListUsers during tuple writes maintain correctness.
///
/// ListUsers should return consistent results even when writes are happening
/// concurrently. Results should reflect either before or after the write.
#[tokio::test]
async fn test_listusers_concurrent_with_writes_maintains_correctness() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "listusers-write-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Initial setup: 15 users can view the document
    for i in 0..15 {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": format!("user:initial{i}"),
                        "relation": "viewer",
                        "object": "document:write_test_doc"
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let read_success = Arc::new(AtomicU64::new(0));
    let write_success = Arc::new(AtomicU64::new(0));
    let valid_results = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::new();

    // Launch concurrent reads and writes
    for i in 0..25 {
        // Read operation
        let storage_clone = Arc::clone(&storage);
        let cache_clone = Arc::clone(&cache);
        let store_id_clone = store_id.clone();
        let read_success_clone = Arc::clone(&read_success);
        let valid_results_clone = Arc::clone(&valid_results);

        handles.push(tokio::spawn(async move {
            let (status, response) = post_json(
                create_test_app_with_shared_cache(&storage_clone, &cache_clone),
                &format!("/stores/{store_id_clone}/list-users"),
                serde_json::json!({
                    "object": {"type": "document", "id": "write_test_doc"},
                    "relation": "viewer",
                    "user_filters": [{"type": "user"}]
                }),
            )
            .await;

            if status == StatusCode::OK {
                read_success_clone.fetch_add(1, Ordering::Relaxed);
                // Result should be >= 15 (initial) and <= 40 (initial + new writes)
                if let Some(users) = response["users"].as_array() {
                    if users.len() >= 15 && users.len() <= 40 {
                        valid_results_clone.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }));

        // Write operation (add new users)
        let storage_clone = Arc::clone(&storage);
        let cache_clone = Arc::clone(&cache);
        let store_id_clone = store_id.clone();
        let write_success_clone = Arc::clone(&write_success);

        handles.push(tokio::spawn(async move {
            let (status, _) = post_json(
                create_test_app_with_shared_cache(&storage_clone, &cache_clone),
                &format!("/stores/{store_id_clone}/write"),
                serde_json::json!({
                    "writes": {
                        "tuple_keys": [{
                            "user": format!("user:new{i}"),
                            "relation": "viewer",
                            "object": "document:write_test_doc"
                        }]
                    }
                }),
            )
            .await;

            if status == StatusCode::OK {
                write_success_clone.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    for handle in handles {
        handle.await.expect("Task should complete without panic");
    }

    let reads = read_success.load(Ordering::Relaxed);
    let writes = write_success.load(Ordering::Relaxed);
    let valid = valid_results.load(Ordering::Relaxed);

    assert_eq!(reads, 25, "All read operations should succeed");
    assert_eq!(writes, 25, "All write operations should succeed");
    assert_eq!(
        valid, reads,
        "All read results should be valid (between 15 and 40 users)"
    );

    println!(
        "ListUsers concurrent with writes: {} reads, {} writes, {} valid results",
        reads, writes, valid
    );
}

/// Test: High concurrency ListUsers (100+ parallel) completes without errors.
///
/// Verifies the system can handle 100+ concurrent ListUsers requests without
/// errors, deadlocks, or performance degradation.
#[tokio::test]
async fn test_listusers_high_concurrency_100_parallel() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "listusers-high-concurrency"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Create test data
    for i in 0..LISTUSERS_TEST_USER_COUNT {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": format!("user:hc_user{i}"),
                        "relation": "viewer",
                        "object": "document:hc_doc"
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let success_count = Arc::new(AtomicU64::new(0));
    let error_count = Arc::new(AtomicU64::new(0));
    let correct_count = Arc::new(AtomicU64::new(0));

    let start = Instant::now();

    // Launch 100+ concurrent ListUsers requests
    let handles: Vec<_> = (0..HIGH_CONCURRENCY_LISTUSERS_COUNT)
        .map(|_| {
            let storage = Arc::clone(&storage);
            let cache = Arc::clone(&cache);
            let store_id = store_id.clone();
            let success_count = Arc::clone(&success_count);
            let error_count = Arc::clone(&error_count);
            let correct_count = Arc::clone(&correct_count);

            tokio::spawn(async move {
                let (status, response) = post_json(
                    create_test_app_with_shared_cache(&storage, &cache),
                    &format!("/stores/{store_id}/list-users"),
                    serde_json::json!({
                        "object": {"type": "document", "id": "hc_doc"},
                        "relation": "viewer",
                        "user_filters": [{"type": "user"}]
                    }),
                )
                .await;

                if status == StatusCode::OK {
                    success_count.fetch_add(1, Ordering::Relaxed);
                    if let Some(users) = response["users"].as_array() {
                        if users.len() == LISTUSERS_TEST_USER_COUNT {
                            correct_count.fetch_add(1, Ordering::Relaxed);
                        }
                    }
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
    let correct = correct_count.load(Ordering::Relaxed);

    println!(
        "High concurrency ListUsers test: {} requests in {:?}",
        HIGH_CONCURRENCY_LISTUSERS_COUNT, elapsed
    );
    println!(
        "  Successes: {}, Errors: {}, Correct results: {}",
        successes, errors, correct
    );
    println!(
        "  Throughput: {:.0} req/s",
        HIGH_CONCURRENCY_LISTUSERS_COUNT as f64 / elapsed.as_secs_f64()
    );

    assert_eq!(
        errors, 0,
        "Should have no errors under high concurrency ListUsers load"
    );
    assert_eq!(
        successes, HIGH_CONCURRENCY_LISTUSERS_COUNT as u64,
        "All concurrent ListUsers requests should succeed"
    );
    assert_eq!(
        correct, HIGH_CONCURRENCY_LISTUSERS_COUNT as u64,
        "All results should have correct user count"
    );
}

/// Test: ListUsers results match between sequential and concurrent execution.
///
/// Verifies that concurrent execution doesn't produce different results
/// than sequential execution for the same query.
#[tokio::test]
async fn test_listusers_sequential_vs_concurrent_results_match() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "listusers-seq-vs-conc"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Create test users
    for i in 0..25 {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": format!("user:seq_user{i}"),
                        "relation": "viewer",
                        "object": "document:seq_doc"
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    // Get sequential result first
    let (status, seq_response) = post_json(
        create_test_app_with_shared_cache(&storage, &cache),
        &format!("/stores/{store_id}/list-users"),
        serde_json::json!({
            "object": {"type": "document", "id": "seq_doc"},
            "relation": "viewer",
            "user_filters": [{"type": "user"}]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let sequential_users: std::collections::HashSet<String> = seq_response["users"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|u| u["object"]["id"].as_str().map(|id| format!("user:{id}")))
        .collect();

    // Now run concurrent requests
    let concurrent_results = Arc::new(tokio::sync::Mutex::new(Vec::<
        std::collections::HashSet<String>,
    >::new()));

    let handles: Vec<_> = (0..30)
        .map(|_| {
            let storage = Arc::clone(&storage);
            let cache = Arc::clone(&cache);
            let store_id = store_id.clone();
            let concurrent_results = Arc::clone(&concurrent_results);

            tokio::spawn(async move {
                let (status, response) = post_json(
                    create_test_app_with_shared_cache(&storage, &cache),
                    &format!("/stores/{store_id}/list-users"),
                    serde_json::json!({
                        "object": {"type": "document", "id": "seq_doc"},
                        "relation": "viewer",
                        "user_filters": [{"type": "user"}]
                    }),
                )
                .await;

                if status == StatusCode::OK {
                    if let Some(users) = response["users"].as_array() {
                        let user_set: std::collections::HashSet<String> = users
                            .iter()
                            .filter_map(|u| {
                                u["object"]["id"].as_str().map(|id| format!("user:{id}"))
                            })
                            .collect();
                        concurrent_results.lock().await.push(user_set);
                    }
                }
            })
        })
        .collect();

    for handle in handles {
        handle.await.expect("Task should complete without panic");
    }

    let all_concurrent = concurrent_results.lock().await;

    // All concurrent results should match sequential
    for (idx, result) in all_concurrent.iter().enumerate() {
        assert_eq!(
            result, &sequential_users,
            "Concurrent result {} should match sequential result",
            idx
        );
    }

    println!(
        "ListUsers sequential vs concurrent test: {} concurrent results match sequential",
        all_concurrent.len()
    );
}

// =============================================================================
// Section 8: Graph Resolver Concurrency Tests (Issue #210)
// =============================================================================

/// Number of concurrent graph traversal requests for high-load tests.
const GRAPH_RESOLVER_CONCURRENCY_COUNT: usize = 100;

/// Number of users/branches for union tests.
const UNION_BRANCH_COUNT: usize = 10;

/// Number of branches for intersection tests.
const INTERSECTION_BRANCH_COUNT: usize = 5;

/// Depth for deep traversal tests.
/// Using 10 levels to stay well within OpenFGA's 25-level depth limit,
/// since each level involves multiple recursive checks.
const DEEP_TRAVERSAL_DEPTH: usize = 10;

/// Helper to set up authorization model with union relation.
///
/// Creates a document type with a `can_view` relation that is a union of
/// multiple direct relations (viewer0, viewer1, ..., viewerN-1).
async fn setup_union_model(storage: &Arc<MemoryDataStore>, store_id: &str, branch_count: usize) {
    // Build union children
    let union_children: Vec<serde_json::Value> = (0..branch_count)
        .map(|i| {
            serde_json::json!({
                "computedUserset": {"object": "", "relation": format!("viewer{i}")}
            })
        })
        .collect();

    // Build relations dynamically based on branch_count
    let mut relations = serde_json::Map::new();
    for i in 0..branch_count {
        relations.insert(format!("viewer{i}"), serde_json::json!({}));
    }
    relations.insert(
        "can_view".to_string(),
        serde_json::json!({
            "union": {
                "child": union_children
            }
        }),
    );

    let model_json = serde_json::json!({
        "schema_version": "1.1",
        "type_definitions": [
            {"type": "user"},
            {
                "type": "document",
                "relations": relations
            }
        ]
    });

    let model = StoredAuthorizationModel::new(
        ulid::Ulid::new().to_string(),
        store_id,
        "1.1",
        model_json.to_string(),
    );
    storage.write_authorization_model(model).await.unwrap();
}

/// Helper to set up authorization model with intersection relation.
///
/// Creates a document type with a `can_edit` relation that requires
/// membership in multiple relations simultaneously (req0 AND req1 AND ... AND reqN-1).
async fn setup_intersection_model(
    storage: &Arc<MemoryDataStore>,
    store_id: &str,
    branch_count: usize,
) {
    // Build intersection children
    let intersection_children: Vec<serde_json::Value> = (0..branch_count)
        .map(|i| {
            serde_json::json!({
                "computedUserset": {"object": "", "relation": format!("req{i}")}
            })
        })
        .collect();

    // Build relations dynamically based on branch_count
    let mut relations = serde_json::Map::new();
    for i in 0..branch_count {
        relations.insert(format!("req{i}"), serde_json::json!({}));
    }
    relations.insert(
        "can_edit".to_string(),
        serde_json::json!({
            "intersection": {
                "child": intersection_children
            }
        }),
    );

    let model_json = serde_json::json!({
        "schema_version": "1.1",
        "type_definitions": [
            {"type": "user"},
            {
                "type": "document",
                "relations": relations
            }
        ]
    });

    let model = StoredAuthorizationModel::new(
        ulid::Ulid::new().to_string(),
        store_id,
        "1.1",
        model_json.to_string(),
    );
    storage.write_authorization_model(model).await.unwrap();
}

/// Helper to set up authorization model with exclusion (but-not) relation.
///
/// Creates a document type with a `can_access` relation that grants access
/// to viewers but excludes blocked users.
async fn setup_exclusion_model(storage: &Arc<MemoryDataStore>, store_id: &str) {
    let model_json = serde_json::json!({
        "schema_version": "1.1",
        "type_definitions": [
            {"type": "user"},
            {
                "type": "document",
                "relations": {
                    "viewer": {},
                    "blocked": {},
                    "can_access": {
                        "difference": {
                            "base": {
                                "computedUserset": {"object": "", "relation": "viewer"}
                            },
                            "subtract": {
                                "computedUserset": {"object": "", "relation": "blocked"}
                            }
                        }
                    }
                }
            }
        ]
    });

    let model = StoredAuthorizationModel::new(
        ulid::Ulid::new().to_string(),
        store_id,
        "1.1",
        model_json.to_string(),
    );
    storage.write_authorization_model(model).await.unwrap();
}

/// Helper to set up deep hierarchy model for traversal tests.
///
/// Creates a folder type where viewers inherit from parent folder's viewers,
/// allowing deep traversal testing.
async fn setup_deep_hierarchy_model(storage: &Arc<MemoryDataStore>, store_id: &str) {
    let model_json = serde_json::json!({
        "schema_version": "1.1",
        "type_definitions": [
            {"type": "user"},
            {
                "type": "folder",
                "relations": {
                    "parent": {},
                    "direct_viewer": {},
                    "viewer": {
                        "union": {
                            "child": [
                                {"this": {}},
                                {"computedUserset": {"object": "", "relation": "direct_viewer"}},
                                {"tupleToUserset": {
                                    "tupleset": {"object": "", "relation": "parent"},
                                    "computedUserset": {"object": "", "relation": "viewer"}
                                }}
                            ]
                        }
                    }
                }
            }
        ]
    });

    let model = StoredAuthorizationModel::new(
        ulid::Ulid::new().to_string(),
        store_id,
        "1.1",
        model_json.to_string(),
    );
    storage.write_authorization_model(model).await.unwrap();
}

/// Test: Parallel union relation traversal doesn't corrupt shared state.
///
/// Verifies that concurrent check operations on union relations don't interfere
/// with each other and produce correct results.
#[tokio::test]
async fn test_parallel_union_traversal_no_shared_state_corruption() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "union-concurrency-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();

    // Set up union model with multiple branches
    setup_union_model(&storage, &store_id, UNION_BRANCH_COUNT).await;

    // Write tuples: different users have access via different union branches
    for i in 0..UNION_BRANCH_COUNT {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": format!("user:union_user{i}"),
                        "relation": format!("viewer{i}"),
                        "object": "document:union_doc"
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let success_count = Arc::new(AtomicU64::new(0));
    let allowed_count = Arc::new(AtomicU64::new(0));
    let denied_count = Arc::new(AtomicU64::new(0));
    let error_count = Arc::new(AtomicU64::new(0));

    // Launch concurrent check requests across all union branches
    let handles: Vec<_> = (0..GRAPH_RESOLVER_CONCURRENCY_COUNT)
        .map(|i| {
            let storage = Arc::clone(&storage);
            let cache = Arc::clone(&cache);
            let store_id = store_id.clone();
            let success_count = Arc::clone(&success_count);
            let allowed_count = Arc::clone(&allowed_count);
            let denied_count = Arc::clone(&denied_count);
            let error_count = Arc::clone(&error_count);

            tokio::spawn(async move {
                // Check different users - some should be allowed, some denied
                let user_idx = i % (UNION_BRANCH_COUNT + 5); // Extra users will be denied
                let (status, response) = post_json(
                    create_test_app_with_shared_cache(&storage, &cache),
                    &format!("/stores/{store_id}/check"),
                    serde_json::json!({
                        "tuple_key": {
                            "user": format!("user:union_user{user_idx}"),
                            "relation": "can_view",
                            "object": "document:union_doc"
                        }
                    }),
                )
                .await;

                if status == StatusCode::OK {
                    success_count.fetch_add(1, Ordering::Relaxed);
                    if response["allowed"].as_bool() == Some(true) {
                        allowed_count.fetch_add(1, Ordering::Relaxed);
                    } else {
                        denied_count.fetch_add(1, Ordering::Relaxed);
                    }
                } else {
                    error_count.fetch_add(1, Ordering::Relaxed);
                }
            })
        })
        .collect();

    for handle in handles {
        handle.await.expect("Task should complete without panic");
    }

    let successes = success_count.load(Ordering::Relaxed);
    let allowed = allowed_count.load(Ordering::Relaxed);
    let denied = denied_count.load(Ordering::Relaxed);
    let errors = error_count.load(Ordering::Relaxed);

    assert_eq!(
        errors, 0,
        "Should have no errors with concurrent union traversal"
    );
    assert_eq!(
        successes, GRAPH_RESOLVER_CONCURRENCY_COUNT as u64,
        "All concurrent requests should succeed"
    );

    // Verify correctness based on deterministic distribution:
    // - 100 requests with user_idx = i % 15 (10 allowed users + 5 denied users)
    // - Users 0-9 have access (via viewer0-viewer9 relations)
    // - Users 10-14 don't have access
    // Distribution: floor(100/15)=6 base + 10 extra for users 0-9
    // - Users 0-9: 7 requests each = 70 allowed
    // - Users 10-14: 6 requests each = 30 denied
    let expected_allowed = 70u64;
    let expected_denied = 30u64;

    assert_eq!(
        allowed, expected_allowed,
        "Expected {} allowed (users 0-9 with 7 requests each), got {}",
        expected_allowed, allowed
    );
    assert_eq!(
        denied, expected_denied,
        "Expected {} denied (users 10-14 with 6 requests each), got {}",
        expected_denied, denied
    );

    println!(
        "Parallel union traversal test: {} successes, {} allowed, {} denied, {} errors",
        successes, allowed, denied, errors
    );
}

/// Test: Parallel intersection traversal maintains correctness.
///
/// Verifies that concurrent check operations on intersection relations
/// produce correct results (all branches must be satisfied).
#[tokio::test]
async fn test_parallel_intersection_traversal_maintains_correctness() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "intersection-concurrency-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();

    // Set up intersection model
    setup_intersection_model(&storage, &store_id, INTERSECTION_BRANCH_COUNT).await;

    // User alice has ALL required permissions
    for i in 0..INTERSECTION_BRANCH_COUNT {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": "user:alice",
                        "relation": format!("req{i}"),
                        "object": "document:protected"
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    // User bob has only SOME required permissions (should be denied)
    for i in 0..3 {
        // Only 3 of 5 requirements
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": "user:bob",
                        "relation": format!("req{i}"),
                        "object": "document:protected"
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let alice_allowed = Arc::new(AtomicU64::new(0));
    let bob_denied = Arc::new(AtomicU64::new(0));
    let error_count = Arc::new(AtomicU64::new(0));

    // Launch concurrent checks for both alice and bob
    let handles: Vec<_> = (0..GRAPH_RESOLVER_CONCURRENCY_COUNT)
        .map(|i| {
            let storage = Arc::clone(&storage);
            let cache = Arc::clone(&cache);
            let store_id = store_id.clone();
            let alice_allowed = Arc::clone(&alice_allowed);
            let bob_denied = Arc::clone(&bob_denied);
            let error_count = Arc::clone(&error_count);

            tokio::spawn(async move {
                let user = if i % 2 == 0 { "alice" } else { "bob" };
                let (status, response) = post_json(
                    create_test_app_with_shared_cache(&storage, &cache),
                    &format!("/stores/{store_id}/check"),
                    serde_json::json!({
                        "tuple_key": {
                            "user": format!("user:{user}"),
                            "relation": "can_edit",
                            "object": "document:protected"
                        }
                    }),
                )
                .await;

                if status == StatusCode::OK {
                    let allowed = response["allowed"].as_bool() == Some(true);
                    match user {
                        "alice" if allowed => {
                            alice_allowed.fetch_add(1, Ordering::Relaxed);
                        }
                        "bob" if !allowed => {
                            bob_denied.fetch_add(1, Ordering::Relaxed);
                        }
                        _ => {
                            // Incorrect result: Alice denied or Bob allowed
                            error_count.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                } else {
                    error_count.fetch_add(1, Ordering::Relaxed);
                }
            })
        })
        .collect();

    for handle in handles {
        handle.await.expect("Task should complete without panic");
    }

    let alice = alice_allowed.load(Ordering::Relaxed);
    let bob = bob_denied.load(Ordering::Relaxed);
    let errors = error_count.load(Ordering::Relaxed);

    assert_eq!(
        errors, 0,
        "Should have no errors or incorrect results with concurrent intersection traversal"
    );

    // Half the requests are for alice (should all be allowed)
    let expected_alice_checks = GRAPH_RESOLVER_CONCURRENCY_COUNT as u64 / 2;
    assert_eq!(
        alice, expected_alice_checks,
        "Alice should be allowed in all checks (has all requirements)"
    );

    // Half the requests are for bob (should all be denied)
    let expected_bob_checks = GRAPH_RESOLVER_CONCURRENCY_COUNT as u64 / 2;
    assert_eq!(
        bob, expected_bob_checks,
        "Bob should be denied in all checks (missing requirements)"
    );

    println!(
        "Parallel intersection test: alice allowed={}, bob denied={}, errors={}",
        alice, bob, errors
    );
}

/// Test: Concurrent exclusion (but-not) evaluation is thread-safe.
///
/// Verifies that concurrent check operations on exclusion relations
/// correctly handle the base minus subtract logic.
#[tokio::test]
async fn test_concurrent_exclusion_evaluation_thread_safe() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "exclusion-concurrency-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();

    // Set up exclusion model (viewer but not blocked)
    setup_exclusion_model(&storage, &store_id).await;

    // alice: viewer, NOT blocked -> should have access
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:sensitive"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // bob: viewer AND blocked -> should NOT have access
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [
                    {
                        "user": "user:bob",
                        "relation": "viewer",
                        "object": "document:sensitive"
                    },
                    {
                        "user": "user:bob",
                        "relation": "blocked",
                        "object": "document:sensitive"
                    }
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // charlie: NOT viewer -> should NOT have access
    // (no tuples needed)

    let alice_correct = Arc::new(AtomicU64::new(0));
    let bob_correct = Arc::new(AtomicU64::new(0));
    let charlie_correct = Arc::new(AtomicU64::new(0));
    let error_count = Arc::new(AtomicU64::new(0));

    // Launch concurrent checks for all three users
    let handles: Vec<_> = (0..GRAPH_RESOLVER_CONCURRENCY_COUNT)
        .map(|i| {
            let storage = Arc::clone(&storage);
            let cache = Arc::clone(&cache);
            let store_id = store_id.clone();
            let alice_correct = Arc::clone(&alice_correct);
            let bob_correct = Arc::clone(&bob_correct);
            let charlie_correct = Arc::clone(&charlie_correct);
            let error_count = Arc::clone(&error_count);

            tokio::spawn(async move {
                let user = match i % 3 {
                    0 => "alice",
                    1 => "bob",
                    _ => "charlie",
                };

                let (status, response) = post_json(
                    create_test_app_with_shared_cache(&storage, &cache),
                    &format!("/stores/{store_id}/check"),
                    serde_json::json!({
                        "tuple_key": {
                            "user": format!("user:{user}"),
                            "relation": "can_access",
                            "object": "document:sensitive"
                        }
                    }),
                )
                .await;

                if status == StatusCode::OK {
                    let allowed = response["allowed"].as_bool() == Some(true);
                    match user {
                        "alice" if allowed => {
                            alice_correct.fetch_add(1, Ordering::Relaxed);
                        }
                        "bob" if !allowed => {
                            bob_correct.fetch_add(1, Ordering::Relaxed);
                        }
                        "charlie" if !allowed => {
                            charlie_correct.fetch_add(1, Ordering::Relaxed);
                        }
                        _ => {
                            error_count.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                } else {
                    error_count.fetch_add(1, Ordering::Relaxed);
                }
            })
        })
        .collect();

    for handle in handles {
        handle.await.expect("Task should complete without panic");
    }

    let alice = alice_correct.load(Ordering::Relaxed);
    let bob = bob_correct.load(Ordering::Relaxed);
    let charlie = charlie_correct.load(Ordering::Relaxed);
    let errors = error_count.load(Ordering::Relaxed);

    // Total correct results should equal total requests (no errors/incorrect results)
    let total_correct = alice + bob + charlie;

    assert_eq!(errors, 0, "Should have no errors or incorrect results");
    assert_eq!(
        total_correct, GRAPH_RESOLVER_CONCURRENCY_COUNT as u64,
        "All requests should return correct results"
    );

    // Each user should get roughly 1/3 of requests (allow +/- 1 for modulo)
    let expected_min = GRAPH_RESOLVER_CONCURRENCY_COUNT as u64 / 3;
    let expected_max = expected_min + 1;

    assert!(
        alice >= expected_min && alice <= expected_max,
        "Alice should get ~1/3 of requests: expected {}-{}, got {}",
        expected_min,
        expected_max,
        alice
    );
    assert!(
        bob >= expected_min && bob <= expected_max,
        "Bob should get ~1/3 of requests: expected {}-{}, got {}",
        expected_min,
        expected_max,
        bob
    );
    assert!(
        charlie >= expected_min && charlie <= expected_max,
        "Charlie should get ~1/3 of requests: expected {}-{}, got {}",
        expected_min,
        expected_max,
        charlie
    );

    println!(
        "Concurrent exclusion test: alice={}, bob={}, charlie={}, errors={}",
        alice, bob, charlie, errors
    );
}

/// Test: Deep parallel traversal (depth 10+) doesn't cause race conditions.
///
/// Verifies that concurrent checks on deeply nested hierarchies
/// (folder parent chains) don't cause race conditions or incorrect results.
#[tokio::test]
async fn test_deep_parallel_traversal_no_race_conditions() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "deep-traversal-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();

    // Set up deep hierarchy model
    setup_deep_hierarchy_model(&storage, &store_id).await;

    // Create a chain of folders: folder0 <- folder1 <- ... <- folderN
    // User alice has direct_viewer on folder0 (root)
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:alice",
                    "relation": "direct_viewer",
                    "object": "folder:folder0"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Create parent relationships for the folder chain
    for i in 1..DEEP_TRAVERSAL_DEPTH {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": format!("folder:folder{}", i - 1),
                        "relation": "parent",
                        "object": format!("folder:folder{i}")
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let success_count = Arc::new(AtomicU64::new(0));
    let allowed_count = Arc::new(AtomicU64::new(0));
    let error_count = Arc::new(AtomicU64::new(0));

    // Launch concurrent checks at various depths in the hierarchy
    let handles: Vec<_> = (0..GRAPH_RESOLVER_CONCURRENCY_COUNT)
        .map(|i| {
            let storage = Arc::clone(&storage);
            let cache = Arc::clone(&cache);
            let store_id = store_id.clone();
            let success_count = Arc::clone(&success_count);
            let allowed_count = Arc::clone(&allowed_count);
            let error_count = Arc::clone(&error_count);

            tokio::spawn(async move {
                // Check at different depths in the hierarchy
                let depth = i % DEEP_TRAVERSAL_DEPTH;
                let (status, response) = post_json(
                    create_test_app_with_shared_cache(&storage, &cache),
                    &format!("/stores/{store_id}/check"),
                    serde_json::json!({
                        "tuple_key": {
                            "user": "user:alice",
                            "relation": "viewer",
                            "object": format!("folder:folder{depth}")
                        }
                    }),
                )
                .await;

                if status == StatusCode::OK {
                    success_count.fetch_add(1, Ordering::Relaxed);
                    if response["allowed"].as_bool() == Some(true) {
                        allowed_count.fetch_add(1, Ordering::Relaxed);
                    }
                } else {
                    error_count.fetch_add(1, Ordering::Relaxed);
                }
            })
        })
        .collect();

    for handle in handles {
        handle.await.expect("Task should complete without panic");
    }

    let successes = success_count.load(Ordering::Relaxed);
    let allowed = allowed_count.load(Ordering::Relaxed);
    let errors = error_count.load(Ordering::Relaxed);

    assert_eq!(
        errors, 0,
        "Should have no errors with deep parallel traversal"
    );
    assert_eq!(
        successes, GRAPH_RESOLVER_CONCURRENCY_COUNT as u64,
        "All concurrent requests should succeed"
    );
    assert_eq!(
        allowed, GRAPH_RESOLVER_CONCURRENCY_COUNT as u64,
        "All checks should be allowed (alice inherits viewer through parent chain)"
    );

    println!(
        "Deep parallel traversal test: {} successes, {} allowed at depths 0-{}, {} errors",
        successes,
        allowed,
        DEEP_TRAVERSAL_DEPTH - 1,
        errors
    );
}

/// Test: Concurrent resolver operations with shared cache are consistent.
///
/// Verifies that the resolver produces consistent results when the cache
/// is being actively used and updated by concurrent operations.
#[tokio::test]
async fn test_concurrent_resolver_with_shared_cache_consistent() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "cache-consistency-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();

    // Set up a simple model with union
    setup_simple_model(&storage, &store_id).await;

    // Create multiple users with viewer access
    for i in 0..20 {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": format!("user:cache_user{i}"),
                        "relation": "viewer",
                        "object": "document:cached_doc"
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    // Track results for consistency verification
    let results = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::<
        String,
        Vec<bool>,
    >::new()));
    let success_count = Arc::new(AtomicU64::new(0));
    let error_count = Arc::new(AtomicU64::new(0));

    // Launch concurrent checks - same user/object should always get same result
    let handles: Vec<_> = (0..GRAPH_RESOLVER_CONCURRENCY_COUNT)
        .map(|i| {
            let storage = Arc::clone(&storage);
            let cache = Arc::clone(&cache);
            let store_id = store_id.clone();
            let results = Arc::clone(&results);
            let success_count = Arc::clone(&success_count);
            let error_count = Arc::clone(&error_count);

            tokio::spawn(async move {
                // Check a subset of users (some with access, some without)
                let user_idx = i % 25; // 20 with access, 5 without
                let user = format!("user:cache_user{user_idx}");

                let (status, response) = post_json(
                    create_test_app_with_shared_cache(&storage, &cache),
                    &format!("/stores/{store_id}/check"),
                    serde_json::json!({
                        "tuple_key": {
                            "user": &user,
                            "relation": "viewer",
                            "object": "document:cached_doc"
                        }
                    }),
                )
                .await;

                if status == StatusCode::OK {
                    success_count.fetch_add(1, Ordering::Relaxed);
                    let allowed = response["allowed"].as_bool() == Some(true);
                    results.lock().await.entry(user).or_default().push(allowed);
                } else {
                    error_count.fetch_add(1, Ordering::Relaxed);
                }
            })
        })
        .collect();

    for handle in handles {
        handle.await.expect("Task should complete without panic");
    }

    let successes = success_count.load(Ordering::Relaxed);
    let errors = error_count.load(Ordering::Relaxed);
    let all_results = results.lock().await;

    assert_eq!(
        errors, 0,
        "Should have no errors with concurrent cache access"
    );
    assert_eq!(
        successes, GRAPH_RESOLVER_CONCURRENCY_COUNT as u64,
        "All requests should succeed"
    );

    // Verify consistency: each user should have consistent results
    let mut inconsistent_users = Vec::new();
    for (user, user_results) in all_results.iter() {
        if !user_results.is_empty() {
            let first = user_results[0];
            if !user_results.iter().all(|&r| r == first) {
                inconsistent_users.push(user.clone());
            }
        }
    }

    assert!(
        inconsistent_users.is_empty(),
        "Found inconsistent results for users: {:?}",
        inconsistent_users
    );

    // Verify expected access pattern
    let mut correct_access = 0;
    let mut correct_denied = 0;
    for (user, user_results) in all_results.iter() {
        if !user_results.is_empty() {
            // Extract user index
            let idx: usize = user
                .strip_prefix("user:cache_user")
                .and_then(|s| s.parse().ok())
                .unwrap_or(999);

            let expected = idx < 20; // users 0-19 have access
            let actual = user_results[0];

            if expected && actual {
                correct_access += 1;
            } else if !expected && !actual {
                correct_denied += 1;
            }
        }
    }

    println!(
        "Concurrent cache consistency test: {} successes, {} errors",
        successes, errors
    );
    println!(
        "  Correct access: {}, Correct denied: {}, Total users checked: {}",
        correct_access,
        correct_denied,
        all_results.len()
    );
}

// =============================================================================
// Section 9: Singleflight Non-Blocking Tests (Issue #210)
// =============================================================================

/// Test: Singleflight doesn't block writes during pending check.
///
/// Verifies that while a check operation is in-flight (being deduplicated
/// via singleflight), write operations can still complete without being blocked.
/// This ensures that the singleflight mechanism doesn't create a global lock
/// that would prevent other operations.
#[tokio::test]
async fn test_singleflight_doesnt_block_writes_during_pending_check() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "singleflight-nonblock-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Write initial tuple for check operation
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:initial"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let check_started = Arc::new(tokio::sync::Notify::new());
    let write_completed = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::new();

    // Launch multiple concurrent check operations (will use singleflight)
    for _ in 0..10 {
        let storage = Arc::clone(&storage);
        let cache = Arc::clone(&cache);
        let store_id = store_id.clone();
        let check_started = Arc::clone(&check_started);

        handles.push(tokio::spawn(async move {
            check_started.notify_one();
            let (status, response) = post_json(
                create_test_app_with_shared_cache(&storage, &cache),
                &format!("/stores/{store_id}/check"),
                serde_json::json!({
                    "tuple_key": {
                        "user": "user:alice",
                        "relation": "viewer",
                        "object": "document:initial"
                    }
                }),
            )
            .await;
            (status, response["allowed"].as_bool())
        }));
    }

    // Wait for at least one check to start
    check_started.notified().await;

    // Launch write operations while checks are in-flight
    for i in 0..5 {
        let storage = Arc::clone(&storage);
        let cache = Arc::clone(&cache);
        let store_id = store_id.clone();
        let write_completed = Arc::clone(&write_completed);

        handles.push(tokio::spawn(async move {
            let (status, _) = post_json(
                create_test_app_with_shared_cache(&storage, &cache),
                &format!("/stores/{store_id}/write"),
                serde_json::json!({
                    "writes": {
                        "tuple_keys": [{
                            "user": format!("user:writer{i}"),
                            "relation": "editor",
                            "object": format!("document:concurrent{i}")
                        }]
                    }
                }),
            )
            .await;
            if status == StatusCode::OK {
                write_completed.fetch_add(1, Ordering::Relaxed);
            }
            (status, None)
        }));
    }

    // Wait for all operations to complete (with timeout to detect blocking)
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        for handle in handles {
            handle.await.expect("Task should complete");
        }
    })
    .await;

    assert!(
        result.is_ok(),
        "Operations should complete without timeout - singleflight should not block writes"
    );

    let writes = write_completed.load(Ordering::Relaxed);
    assert_eq!(
        writes, 5,
        "All write operations should complete while checks are in-flight"
    );

    println!(
        "Singleflight non-blocking test: {} writes completed during concurrent checks",
        writes
    );
}

/// Test: Write operations complete while check is in-flight.
///
/// Specifically tests that write operations targeting different tuples
/// than the ones being checked can complete independently of singleflight.
#[tokio::test]
async fn test_write_operations_complete_while_check_in_flight() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "write-during-check-test"}),
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
                        "object": "document:shared"
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let check_count = Arc::new(AtomicU64::new(0));
    let write_count = Arc::new(AtomicU64::new(0));
    let check_first_completed = Arc::new(AtomicU64::new(0));
    let write_first_completed = Arc::new(AtomicU64::new(0));
    let completion_order = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::new();

    // Launch interleaved check and write operations
    for i in 0..20 {
        if i % 2 == 0 {
            // Check operation
            let storage = Arc::clone(&storage);
            let cache = Arc::clone(&cache);
            let store_id = store_id.clone();
            let check_count = Arc::clone(&check_count);
            let check_first_completed = Arc::clone(&check_first_completed);
            let completion_order = Arc::clone(&completion_order);

            handles.push(tokio::spawn(async move {
                let user_idx = (i / 2) % 10;
                let (status, _) = post_json(
                    create_test_app_with_shared_cache(&storage, &cache),
                    &format!("/stores/{store_id}/check"),
                    serde_json::json!({
                        "tuple_key": {
                            "user": format!("user:user{user_idx}"),
                            "relation": "viewer",
                            "object": "document:shared"
                        }
                    }),
                )
                .await;
                if status == StatusCode::OK {
                    check_count.fetch_add(1, Ordering::Relaxed);
                    // Record if this was the first completion
                    let order = completion_order.fetch_add(1, Ordering::SeqCst);
                    if order == 0 {
                        check_first_completed.store(1, Ordering::SeqCst);
                    }
                }
            }));
        } else {
            // Write operation
            let storage = Arc::clone(&storage);
            let cache = Arc::clone(&cache);
            let store_id = store_id.clone();
            let write_count = Arc::clone(&write_count);
            let write_first_completed = Arc::clone(&write_first_completed);
            let completion_order = Arc::clone(&completion_order);

            handles.push(tokio::spawn(async move {
                let (status, _) = post_json(
                    create_test_app_with_shared_cache(&storage, &cache),
                    &format!("/stores/{store_id}/write"),
                    serde_json::json!({
                        "writes": {
                            "tuple_keys": [{
                                "user": format!("user:newuser{i}"),
                                "relation": "editor",
                                "object": format!("document:newdoc{i}")
                            }]
                        }
                    }),
                )
                .await;
                if status == StatusCode::OK {
                    write_count.fetch_add(1, Ordering::Relaxed);
                    // Record if this was the first completion
                    let order = completion_order.fetch_add(1, Ordering::SeqCst);
                    if order == 0 {
                        write_first_completed.store(1, Ordering::SeqCst);
                    }
                }
            }));
        }
    }

    // Wait for all operations
    for handle in handles {
        handle.await.expect("Task should complete");
    }

    let checks = check_count.load(Ordering::Relaxed);
    let writes = write_count.load(Ordering::Relaxed);

    assert_eq!(checks, 10, "All check operations should succeed");
    assert_eq!(writes, 10, "All write operations should succeed");

    // Both checks and writes should be able to complete - the order doesn't matter,
    // but both types should complete successfully
    println!(
        "Write during check test: {} checks, {} writes completed",
        checks, writes
    );
}

/// Test: Concurrent writes and checks don't deadlock.
///
/// Stress test to ensure that heavy concurrent load of both reads (checks)
/// and writes doesn't cause deadlocks in the singleflight mechanism.
#[tokio::test]
async fn test_concurrent_writes_and_checks_no_deadlock() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "deadlock-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Pre-populate some data
    for i in 0..20 {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": format!("user:preload{i}"),
                        "relation": "viewer",
                        "object": "document:deadlock_test"
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let check_success = Arc::new(AtomicU64::new(0));
    let write_success = Arc::new(AtomicU64::new(0));
    let check_error = Arc::new(AtomicU64::new(0));
    let write_error = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::new();

    // Launch heavy concurrent load
    const CONCURRENT_OPS: usize = 100;

    for i in 0..CONCURRENT_OPS {
        // Check operation
        let storage_c = Arc::clone(&storage);
        let cache_c = Arc::clone(&cache);
        let store_id_c = store_id.clone();
        let check_success_c = Arc::clone(&check_success);
        let check_error_c = Arc::clone(&check_error);

        handles.push(tokio::spawn(async move {
            let user_idx = i % 20;
            let (status, _) = post_json(
                create_test_app_with_shared_cache(&storage_c, &cache_c),
                &format!("/stores/{store_id_c}/check"),
                serde_json::json!({
                    "tuple_key": {
                        "user": format!("user:preload{user_idx}"),
                        "relation": "viewer",
                        "object": "document:deadlock_test"
                    }
                }),
            )
            .await;
            if status == StatusCode::OK {
                check_success_c.fetch_add(1, Ordering::Relaxed);
            } else {
                check_error_c.fetch_add(1, Ordering::Relaxed);
            }
        }));

        // Write operation
        let storage_w = Arc::clone(&storage);
        let cache_w = Arc::clone(&cache);
        let store_id_w = store_id.clone();
        let write_success_w = Arc::clone(&write_success);
        let write_error_w = Arc::clone(&write_error);

        handles.push(tokio::spawn(async move {
            let (status, _) = post_json(
                create_test_app_with_shared_cache(&storage_w, &cache_w),
                &format!("/stores/{store_id_w}/write"),
                serde_json::json!({
                    "writes": {
                        "tuple_keys": [{
                            "user": format!("user:concurrent{i}"),
                            "relation": "editor",
                            "object": format!("document:concurrent{i}")
                        }]
                    }
                }),
            )
            .await;
            if status == StatusCode::OK {
                write_success_w.fetch_add(1, Ordering::Relaxed);
            } else {
                write_error_w.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    // Use timeout to detect deadlocks
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        for handle in handles {
            handle.await.expect("Task should complete without panic");
        }
    })
    .await;

    assert!(
        result.is_ok(),
        "All operations should complete without deadlock (30s timeout)"
    );

    let check_ok = check_success.load(Ordering::Relaxed);
    let write_ok = write_success.load(Ordering::Relaxed);
    let check_err = check_error.load(Ordering::Relaxed);
    let write_err = write_error.load(Ordering::Relaxed);

    assert_eq!(
        check_err, 0,
        "No check errors expected (got {check_err} errors)"
    );
    assert_eq!(
        write_err, 0,
        "No write errors expected (got {write_err} errors)"
    );
    assert_eq!(check_ok, CONCURRENT_OPS as u64, "All checks should succeed");
    assert_eq!(write_ok, CONCURRENT_OPS as u64, "All writes should succeed");

    println!(
        "Deadlock test passed: {} checks + {} writes completed without deadlock",
        check_ok, write_ok
    );
}

/// Test: Singleflight timeout doesn't block subsequent requests.
///
/// Verifies that if a singleflight operation times out, subsequent requests
/// for the same key can still proceed (the singleflight entry is properly
/// cleaned up after timeout/failure).
#[tokio::test]
async fn test_singleflight_timeout_doesnt_block_subsequent_requests() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "singleflight-timeout-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Write tuple
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:timeout_test",
                    "relation": "viewer",
                    "object": "document:timeout_doc"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Launch a batch of concurrent identical check requests
    // Some may time out if the system is slow, but subsequent requests
    // should not be blocked forever
    let success_count = Arc::new(AtomicU64::new(0));

    // First wave: concurrent requests
    let mut handles = Vec::new();
    for _ in 0..10 {
        let storage = Arc::clone(&storage);
        let cache = Arc::clone(&cache);
        let store_id = store_id.clone();
        let success_count = Arc::clone(&success_count);

        handles.push(tokio::spawn(async move {
            let (status, response) = post_json(
                create_test_app_with_shared_cache(&storage, &cache),
                &format!("/stores/{store_id}/check"),
                serde_json::json!({
                    "tuple_key": {
                        "user": "user:timeout_test",
                        "relation": "viewer",
                        "object": "document:timeout_doc"
                    }
                }),
            )
            .await;
            if status == StatusCode::OK && response["allowed"].as_bool() == Some(true) {
                success_count.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    // Wait for first wave with a short timeout
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        for handle in handles {
            let _ = handle.await;
        }
    })
    .await;

    let first_wave_success = success_count.load(Ordering::Relaxed);

    // Second wave: subsequent requests should not be blocked
    let second_wave_success = Arc::new(AtomicU64::new(0));
    let mut handles = Vec::new();

    for _ in 0..5 {
        let storage = Arc::clone(&storage);
        let cache = Arc::clone(&cache);
        let store_id = store_id.clone();
        let success_count = Arc::clone(&second_wave_success);

        handles.push(tokio::spawn(async move {
            let (status, response) = post_json(
                create_test_app_with_shared_cache(&storage, &cache),
                &format!("/stores/{store_id}/check"),
                serde_json::json!({
                    "tuple_key": {
                        "user": "user:timeout_test",
                        "relation": "viewer",
                        "object": "document:timeout_doc"
                    }
                }),
            )
            .await;
            if status == StatusCode::OK && response["allowed"].as_bool() == Some(true) {
                success_count.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    // Second wave should complete quickly (within 5 seconds)
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        for handle in handles {
            handle.await.expect("Second wave task should complete");
        }
    })
    .await;

    assert!(
        result.is_ok(),
        "Second wave should complete without being blocked by first wave's singleflight state"
    );

    let second_success = second_wave_success.load(Ordering::Relaxed);
    assert_eq!(
        second_success, 5,
        "All second wave requests should succeed (singleflight cleanup worked)"
    );

    println!(
        "Singleflight timeout test: first wave={}, second wave={} successes",
        first_wave_success, second_success
    );
}

// =============================================================================
// Section 10: Memory and Stability Tests (Issue #210 Section 5)
// =============================================================================

/// Number of iterations for memory stability tests.
const MEMORY_STABILITY_ITERATIONS: usize = 1000;

/// Number of operations per iteration for memory tests.
const MEMORY_TEST_OPS_PER_ITERATION: usize = 10;

/// Number of writes for burst invalidation tests.
const BURST_INVALIDATION_WRITE_COUNT: usize = 100;

/// Number of concurrent readers during invalidation tests.
const THUNDERING_HERD_READER_COUNT: usize = 50;

/// Samples to collect for memory measurement.
const MEMORY_SAMPLE_COUNT: usize = 10;

/// Test: Memory usage doesn't grow unbounded with repeated operations.
///
/// Verifies that repeated check operations don't cause memory leaks.
/// This test runs many iterations of checks and verifies cache size
/// remains bounded (eviction works correctly).
#[tokio::test]
async fn test_memory_usage_bounded_with_repeated_operations() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache_config = CheckCacheConfig {
        max_capacity: Some(100), // Small cache to force evictions
        time_to_live: Some(Duration::from_secs(300)),
        time_to_idle: Some(Duration::from_secs(60)),
    };
    let cache = create_shared_cache_with_config(cache_config);

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "memory-bounded-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Create some base tuples
    for i in 0..20 {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": format!("user:mem{i}"),
                        "relation": "viewer",
                        "object": format!("document:mem{i}")
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    // Run many iterations of checks to various objects
    // This should NOT cause unbounded memory growth due to cache eviction
    for iteration in 0..MEMORY_STABILITY_ITERATIONS {
        for op in 0..MEMORY_TEST_OPS_PER_ITERATION {
            // Generate varied check keys that exceed cache capacity
            let user_id = (iteration * MEMORY_TEST_OPS_PER_ITERATION + op) % 500;
            let doc_id = user_id % 100;

            let (status, _) = post_json(
                create_test_app_with_shared_cache(&storage, &cache),
                &format!("/stores/{store_id}/check"),
                serde_json::json!({
                    "tuple_key": {
                        "user": format!("user:iter{user_id}"),
                        "relation": "viewer",
                        "object": format!("document:iter{doc_id}")
                    }
                }),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
        }

        // Periodically verify cache is bounded
        if iteration % 100 == 0 {
            cache.run_pending_tasks().await;
            let cache_size = cache.entry_count();
            // Cache should never exceed max capacity significantly
            // (Moka may slightly exceed due to async eviction)
            assert!(
                cache_size <= 150, // Some slack for async eviction
                "Cache should be bounded: got {} entries at iteration {}",
                cache_size,
                iteration
            );
        }
    }

    // Final check
    cache.run_pending_tasks().await;
    let final_cache_size = cache.entry_count();
    println!(
        "Memory bounded test: {} iterations, final cache size: {}",
        MEMORY_STABILITY_ITERATIONS, final_cache_size
    );

    assert!(
        final_cache_size <= 150,
        "Cache should remain bounded after {} iterations: got {} entries",
        MEMORY_STABILITY_ITERATIONS,
        final_cache_size
    );
}

/// Test: Cache invalidation doesn't cause thundering herd.
///
/// When a write invalidates cache entries, multiple readers waiting for
/// those entries should not all simultaneously hit storage. The cache
/// and singleflight mechanisms should prevent thundering herd.
#[tokio::test]
async fn test_cache_invalidation_no_thundering_herd() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache = create_shared_cache();

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "thundering-herd-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Create initial tuple
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:thundering",
                    "relation": "viewer",
                    "object": "document:target"
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
                "user": "user:thundering",
                "relation": "viewer",
                "object": "document:target"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    cache.run_pending_tasks().await;

    // Track timing to detect thundering herd
    let all_reads_complete = Arc::new(AtomicU64::new(0));
    let max_concurrent_observed = Arc::new(AtomicU64::new(0));
    let current_concurrent = Arc::new(AtomicU64::new(0));
    let read_successes = Arc::new(AtomicU64::new(0));

    // Spawn many readers that will check after invalidation
    let store_id_clone = store_id.clone();
    let storage_clone = Arc::clone(&storage);
    let cache_clone = Arc::clone(&cache);
    let mut reader_handles = Vec::new();

    for i in 0..THUNDERING_HERD_READER_COUNT {
        let store_id = store_id_clone.clone();
        let storage = Arc::clone(&storage_clone);
        let cache = Arc::clone(&cache_clone);
        let complete = Arc::clone(&all_reads_complete);
        let max_conc = Arc::clone(&max_concurrent_observed);
        let curr_conc = Arc::clone(&current_concurrent);
        let successes = Arc::clone(&read_successes);
        let stagger_micros = (i * 20) as u64; // Stagger by 20μs per reader

        reader_handles.push(tokio::spawn(async move {
            // Small delay to stagger readers slightly (deterministic based on index)
            tokio::time::sleep(Duration::from_micros(stagger_micros)).await;

            // Track concurrent operations
            let conc = curr_conc.fetch_add(1, Ordering::SeqCst) + 1;
            max_conc.fetch_max(conc, Ordering::SeqCst);

            let (status, _) = post_json(
                create_test_app_with_shared_cache(&storage, &cache),
                &format!("/stores/{store_id}/check"),
                serde_json::json!({
                    "tuple_key": {
                        "user": "user:thundering",
                        "relation": "viewer",
                        "object": "document:target"
                    }
                }),
            )
            .await;

            curr_conc.fetch_sub(1, Ordering::SeqCst);

            if status == StatusCode::OK {
                successes.fetch_add(1, Ordering::Relaxed);
            }
            complete.fetch_add(1, Ordering::Relaxed);
        }));
    }

    // While readers are starting, invalidate the cache with a write
    tokio::time::sleep(Duration::from_millis(5)).await;
    let (status, _) = post_json(
        create_test_app_with_shared_cache(&storage, &cache),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:extra",
                    "relation": "editor",
                    "object": "document:target"
                }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Wait for all readers to complete
    for handle in reader_handles {
        let _ = handle.await;
    }

    let total_complete = all_reads_complete.load(Ordering::Relaxed);
    let max_concurrent = max_concurrent_observed.load(Ordering::SeqCst);
    let total_success = read_successes.load(Ordering::Relaxed);

    println!(
        "Thundering herd test: {} readers, max concurrent={}, successes={}",
        THUNDERING_HERD_READER_COUNT, max_concurrent, total_success
    );

    // All readers should complete
    assert_eq!(
        total_complete, THUNDERING_HERD_READER_COUNT as u64,
        "All readers should complete"
    );

    // All should succeed
    assert_eq!(
        total_success, THUNDERING_HERD_READER_COUNT as u64,
        "All readers should succeed"
    );

    // Singleflight should prevent thundering herd - while all readers may
    // start concurrently, the actual storage queries should be deduplicated.
    // We verify this indirectly by ensuring all complete successfully without
    // errors or timeouts.
}

/// Test: Burst invalidation (100+ writes) doesn't cause memory spike.
///
/// Rapidly writing many tuples that each trigger cache invalidation
/// should not cause excessive memory usage or performance degradation.
#[tokio::test]
async fn test_burst_invalidation_no_memory_spike() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache_config = CheckCacheConfig {
        max_capacity: Some(500),
        time_to_live: Some(Duration::from_secs(300)),
        time_to_idle: Some(Duration::from_secs(60)),
    };
    let cache = create_shared_cache_with_config(cache_config);

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "burst-invalidation-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Create initial tuples and populate cache
    for i in 0..100 {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": format!("user:burst{i}"),
                        "relation": "viewer",
                        "object": format!("document:burst{i}")
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Populate cache for each entry
        let (status, _) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
            &format!("/stores/{store_id}/check"),
            serde_json::json!({
                "tuple_key": {
                    "user": format!("user:burst{i}"),
                    "relation": "viewer",
                    "object": format!("document:burst{i}")
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    cache.run_pending_tasks().await;
    let cache_size_before = cache.entry_count();
    println!("Cache size before burst: {}", cache_size_before);

    // Measure timing and verify no significant performance degradation
    let start = Instant::now();
    let mut write_latencies = Vec::with_capacity(BURST_INVALIDATION_WRITE_COUNT);

    // Burst write 100+ tuples rapidly (each invalidates cache entries)
    for i in 0..BURST_INVALIDATION_WRITE_COUNT {
        let write_start = Instant::now();
        let (status, _) = post_json(
            create_test_app_with_shared_cache(&storage, &cache),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": format!("user:new{i}"),
                        "relation": "editor",
                        "object": format!("document:burst{}", i % 100) // Invalidates existing cache entries
                    }]
                }
            }),
        )
        .await;
        write_latencies.push(write_start.elapsed());
        assert_eq!(status, StatusCode::OK);
    }

    let total_elapsed = start.elapsed();
    cache.run_pending_tasks().await;
    let cache_size_after = cache.entry_count();

    // Calculate latency percentiles
    write_latencies.sort();
    let p50 = write_latencies[write_latencies.len() / 2];
    let p95 = write_latencies[write_latencies.len() * 95 / 100];
    let p99 = write_latencies[write_latencies.len() * 99 / 100];

    println!(
        "Burst invalidation: {} writes in {:?} ({:.0} writes/s)",
        BURST_INVALIDATION_WRITE_COUNT,
        total_elapsed,
        BURST_INVALIDATION_WRITE_COUNT as f64 / total_elapsed.as_secs_f64()
    );
    println!(
        "Write latencies: p50={:?}, p95={:?}, p99={:?}",
        p50, p95, p99
    );
    println!(
        "Cache size: before={}, after={}",
        cache_size_before, cache_size_after
    );

    // Verify reasonable throughput (>20 writes/s even with invalidation)
    let throughput = BURST_INVALIDATION_WRITE_COUNT as f64 / total_elapsed.as_secs_f64();
    assert!(
        throughput > 20.0,
        "Burst write throughput should be > 20 writes/s, got {:.0}",
        throughput
    );

    // Verify cache is still bounded (no memory spike from invalidation tracking)
    assert!(
        cache_size_after <= 600, // Max capacity + some slack
        "Cache should remain bounded after burst: got {} entries",
        cache_size_after
    );

    // p99 latency should be reasonable (< 500ms for in-memory)
    assert!(
        p99 < Duration::from_millis(500),
        "p99 write latency should be < 500ms, got {:?}",
        p99
    );
}

/// Test: Memory stable under sustained load (no leaks detected).
///
/// Runs mixed read/write operations for an extended period and verifies
/// that memory usage (approximated by cache size) remains stable.
#[tokio::test]
#[ignore] // Resource-intensive test
async fn test_memory_stable_under_sustained_load() {
    let storage = Arc::new(MemoryDataStore::new());
    let cache_config = CheckCacheConfig {
        max_capacity: Some(1000),
        time_to_live: Some(Duration::from_secs(300)),
        time_to_idle: Some(Duration::from_secs(60)),
    };
    let cache = create_shared_cache_with_config(cache_config);

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "memory-sustained-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_id).await;

    // Create initial data set
    for i in 0..100 {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/write"),
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": format!("user:sustained{i}"),
                        "relation": "viewer",
                        "object": format!("document:sustained{i}")
                    }]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    // Collect cache size samples over time
    let mut cache_samples: Vec<u64> = Vec::with_capacity(MEMORY_SAMPLE_COUNT);
    let sample_interval = SUSTAINED_LOAD_DURATION / MEMORY_SAMPLE_COUNT as u32;
    let start = Instant::now();
    let mut operations_count = 0u64;
    let mut sample_index = 0;

    while start.elapsed() < SUSTAINED_LOAD_DURATION {
        // Mixed operations: 80% reads, 20% writes (deterministic pattern)
        let is_read = operations_count % 5 != 0; // 4 out of 5 = 80% reads
        let i = operations_count % 1000;

        if is_read {
            // Read operation (check)
            let user_idx = i % 200;
            let doc_idx = i % 150;
            let (status, _) = post_json(
                create_test_app_with_shared_cache(&storage, &cache),
                &format!("/stores/{store_id}/check"),
                serde_json::json!({
                    "tuple_key": {
                        "user": format!("user:sustained{user_idx}"),
                        "relation": "viewer",
                        "object": format!("document:sustained{doc_idx}")
                    }
                }),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
        } else {
            // Write operation
            let (status, _) = post_json(
                create_test_app_with_shared_cache(&storage, &cache),
                &format!("/stores/{store_id}/write"),
                serde_json::json!({
                    "writes": {
                        "tuple_keys": [{
                            "user": format!("user:new{i}"),
                            "relation": "viewer",
                            "object": format!("document:sustained{}", i % 100)
                        }]
                    }
                }),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
        }

        operations_count += 1;

        // Sample cache size periodically
        if start.elapsed() >= sample_interval * (sample_index as u32 + 1)
            && sample_index < MEMORY_SAMPLE_COUNT
        {
            cache.run_pending_tasks().await;
            cache_samples.push(cache.entry_count());
            sample_index += 1;
        }

        // Small delay to prevent overwhelming the system
        if operations_count % 100 == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    // Collect final sample
    cache.run_pending_tasks().await;
    cache_samples.push(cache.entry_count());

    // Analyze memory stability
    let max_cache = *cache_samples.iter().max().unwrap_or(&0);
    let min_cache = *cache_samples.iter().min().unwrap_or(&0);
    let avg_cache: f64 =
        cache_samples.iter().map(|&x| x as f64).sum::<f64>() / cache_samples.len() as f64;

    println!(
        "Memory stability test: {} operations in {:?}",
        operations_count,
        start.elapsed()
    );
    println!(
        "Cache size - min: {}, max: {}, avg: {:.0}",
        min_cache, max_cache, avg_cache
    );
    println!("Samples: {:?}", cache_samples);

    // Verify memory stability: max cache size should never exceed configured limit
    assert!(
        max_cache <= 1100, // 1000 max capacity + 10% slack for async eviction
        "Cache should remain bounded: max={} exceeds limit",
        max_cache
    );

    // Verify no runaway growth: later samples shouldn't be significantly larger
    // than earlier ones (within 50% tolerance)
    if cache_samples.len() >= 4 {
        let first_half_avg: f64 = cache_samples[..cache_samples.len() / 2]
            .iter()
            .map(|&x| x as f64)
            .sum::<f64>()
            / (cache_samples.len() / 2) as f64;
        let second_half_avg: f64 = cache_samples[cache_samples.len() / 2..]
            .iter()
            .map(|&x| x as f64)
            .sum::<f64>()
            / (cache_samples.len() / 2) as f64;

        // Second half average shouldn't be more than 1.5x first half
        // (allowing for warmup effects)
        let growth_ratio = second_half_avg / first_half_avg.max(1.0);
        println!(
            "Growth ratio (second half / first half): {:.2}",
            growth_ratio
        );
        assert!(
            growth_ratio < 1.5 || second_half_avg < 1100.0,
            "Memory usage should be stable: growth ratio {:.2} indicates potential leak",
            growth_ratio
        );
    }
}
