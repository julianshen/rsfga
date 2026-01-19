//! Storage Backend Parity Integration Tests (Issue #203)
//!
//! These tests verify consistent behavior across all storage backends:
//! - PostgreSQL
//! - MySQL
//! - CockroachDB (uses PostgreSQL driver)
//! - In-Memory
//!
//! Security-Critical: Authorization bugs in one backend but not others
//! could lead to unauthorized access. These tests ensure parity.
//!
//! # Delete Operation Semantics
//!
//! The storage layer follows **idempotent delete semantics**: deleting a tuple
//! that doesn't exist is NOT an error. This matches OpenFGA behavior and
//! simplifies client code (no need to check existence before delete).
//!
//! Some backends may return `TupleNotFound` while others succeed silently.
//! Both behaviors are acceptable per the DataStore trait contract.
//!
//! # Test Scope
//!
//! These tests focus on **storage layer parity** - ensuring all DataStore
//! implementations behave consistently for the same operations. This includes:
//! - CRUD operations on stores, tuples, and authorization models
//! - Pagination and filtering behavior
//! - Concurrent operation handling
//! - Error conditions and edge cases
//!
//! **NOT in scope**: REST/gRPC protocol testing is handled separately in the
//! `compatibility-tests` crate, which validates HTTP/gRPC API behavior against
//! OpenFGA's specification.
//!
//! To run these tests:
//!   # In-memory only (always runs)
//!   cargo test -p rsfga-storage --test storage_backend_parity
//!
//!   # With PostgreSQL
//!   export DATABASE_URL=postgres://postgres:test@localhost:5432/postgres
//!   cargo test -p rsfga-storage --test storage_backend_parity -- --ignored
//!
//!   # With MySQL
//!   export MYSQL_URL=mysql://root:test@localhost:3306/rsfga
//!   cargo test -p rsfga-storage --test storage_backend_parity -- --ignored
//!
//!   # With CockroachDB
//!   export COCKROACHDB_URL=postgresql://root@localhost:26257/rsfga
//!   cargo test -p rsfga-storage --test storage_backend_parity -- --ignored

use rsfga_storage::{
    DataStore, MemoryDataStore, MySQLConfig, MySQLDataStore, PaginatedResult, PaginationOptions,
    PostgresConfig, PostgresDataStore, StorageError, Store, StoredAuthorizationModel, StoredTuple,
    TupleFilter,
};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

// ============================================================================
// Test Constants
// ============================================================================

/// Number of concurrent tasks for parallelism tests.
/// Must be <= MAX_DB_CONNECTIONS to ensure true parallelism.
const CONCURRENT_TASK_COUNT: u32 = 20;

/// Number of tuples each concurrent task writes.
const TUPLES_PER_TASK: u32 = 50;

/// Total tuples expected from concurrent write tests.
const TOTAL_CONCURRENT_TUPLES: usize = (CONCURRENT_TASK_COUNT * TUPLES_PER_TASK) as usize;

/// Number of tuples for batch concurrent write tests.
const BATCH_TUPLES_PER_TASK: u32 = 100;

/// Number of concurrent tasks for batch write tests.
const BATCH_CONCURRENT_TASK_COUNT: u32 = 10;

/// Total tuples expected from batch concurrent tests.
const TOTAL_BATCH_TUPLES: usize = (BATCH_CONCURRENT_TASK_COUNT * BATCH_TUPLES_PER_TASK) as usize;

/// Number of tuples for pagination integrity tests.
const PAGINATION_TEST_TUPLE_COUNT: usize = 100;

/// Page size for pagination tests (odd number to test edge cases).
const PAGINATION_PAGE_SIZE: u32 = 7;

/// Number of tuples per type for filtered pagination tests.
const FILTERED_PAGINATION_TUPLE_COUNT: usize = 50;

/// Page size for filtered pagination tests.
const FILTERED_PAGINATION_PAGE_SIZE: u32 = 11;

/// Number of stores for list_stores tests.
const LIST_STORES_COUNT: usize = 5;

/// Number of stores for paginated list_stores tests.
const LIST_STORES_PAGINATED_COUNT: usize = 25;

/// Page size for paginated list_stores tests.
const LIST_STORES_PAGE_SIZE: u32 = 7;

/// Number of tuples for large dataset tests.
const LARGE_DATASET_TUPLE_COUNT: usize = 10_000;

/// Number of unique users in large dataset tests.
const LARGE_DATASET_USER_COUNT: usize = 100;

/// Page size for large dataset pagination.
const LARGE_DATASET_PAGE_SIZE: u32 = 1000;

/// Maximum database connections - must support CONCURRENT_TASK_COUNT + headroom.
const MAX_DB_CONNECTIONS: u32 = 25;

/// Minimum expected parallelism speedup factor.
/// If sequential would take N*T, parallel should take < N*T/SPEEDUP_FACTOR.
const MIN_PARALLELISM_SPEEDUP: f64 = 2.0;

// ============================================================================
// Test Infrastructure
// ============================================================================

/// Get PostgreSQL database URL from environment
fn get_postgres_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:test@localhost:5432/postgres".to_string())
}

/// Get MySQL database URL from environment
fn get_mysql_url() -> String {
    std::env::var("MYSQL_URL")
        .unwrap_or_else(|_| "mysql://root:test@localhost:3306/rsfga".to_string())
}

/// Get CockroachDB database URL from environment
fn get_cockroachdb_url() -> String {
    std::env::var("COCKROACHDB_URL")
        .unwrap_or_else(|_| "postgresql://root@localhost:26257/rsfga".to_string())
}

fn create_memory_store() -> MemoryDataStore {
    MemoryDataStore::new()
}

async fn create_postgres_store() -> PostgresDataStore {
    let config = PostgresConfig {
        database_url: get_postgres_url(),
        max_connections: MAX_DB_CONNECTIONS,
        min_connections: 1,
        connect_timeout_secs: 30,
        ..Default::default()
    };

    let store = PostgresDataStore::from_config(&config)
        .await
        .expect("Failed to create PostgresDataStore");

    store
        .run_migrations()
        .await
        .expect("Failed to run migrations");

    cleanup_test_stores(&store, "parity-").await;
    store
}

async fn create_mysql_store() -> MySQLDataStore {
    let config = MySQLConfig {
        database_url: get_mysql_url(),
        max_connections: MAX_DB_CONNECTIONS,
        min_connections: 1,
        connect_timeout_secs: 30,
        ..Default::default()
    };

    let store = MySQLDataStore::from_config(&config)
        .await
        .expect("Failed to create MySQLDataStore");

    store
        .run_migrations()
        .await
        .expect("Failed to run migrations");

    cleanup_test_stores(&store, "parity-").await;
    store
}

async fn create_cockroachdb_store() -> PostgresDataStore {
    let config = PostgresConfig {
        database_url: get_cockroachdb_url(),
        max_connections: MAX_DB_CONNECTIONS,
        min_connections: 1,
        connect_timeout_secs: 30,
        ..Default::default()
    };

    let store = PostgresDataStore::from_config(&config)
        .await
        .expect("Failed to create PostgresDataStore for CockroachDB");

    store
        .run_migrations()
        .await
        .expect("Failed to run migrations on CockroachDB");

    cleanup_test_stores(&store, "parity-").await;
    store
}

/// Clean up test stores matching a prefix using concurrent deletion.
async fn cleanup_test_stores<S: DataStore>(store: &S, prefix: &str) {
    if let Ok(stores) = store.list_stores().await {
        let delete_futures: Vec<_> = stores
            .iter()
            .filter(|s| s.id.starts_with(prefix))
            .map(|s| store.delete_store(&s.id))
            .collect();

        // Execute all deletes concurrently
        let _ = futures::future::join_all(delete_futures).await;
    }
}

// ============================================================================
// Section 1: Authorization Model Operations Parity
// ============================================================================

/// Generic helper: Test authorization model CRUD operations
async fn run_authorization_model_parity_test<S: DataStore>(store: &S, store_id: &str) {
    // Create store
    store
        .create_store(store_id, "Authorization Model Parity Test")
        .await
        .expect("Failed to create store");

    // Write an authorization model
    let model_json = serde_json::json!({
        "schema_version": "1.1",
        "type_definitions": [
            {"type": "user"},
            {
                "type": "document",
                "relations": {
                    "viewer": {"this": {}}
                }
            }
        ]
    });

    let model_id = ulid::Ulid::new().to_string();
    let model = StoredAuthorizationModel::new(
        &model_id,
        store_id,
        "1.1",
        serde_json::to_string(&model_json).unwrap(),
    );

    let _written_model = store
        .write_authorization_model(model)
        .await
        .expect("Failed to write authorization model");

    // Get the model back
    let retrieved = store
        .get_authorization_model(store_id, &model_id)
        .await
        .expect("Failed to get authorization model");

    assert_eq!(retrieved.id, model_id);
    assert_eq!(retrieved.schema_version, "1.1");

    // Get latest model
    let latest = store
        .get_latest_authorization_model(store_id)
        .await
        .expect("Failed to get latest model");

    assert_eq!(latest.id, model_id);

    // Write another model
    let model2_id = ulid::Ulid::new().to_string();
    let model2 = StoredAuthorizationModel::new(
        &model2_id,
        store_id,
        "1.1",
        serde_json::to_string(&model_json).unwrap(),
    );

    store
        .write_authorization_model(model2)
        .await
        .expect("Failed to write second model");

    // Latest should now be model2
    let latest = store
        .get_latest_authorization_model(store_id)
        .await
        .expect("Failed to get latest model");

    assert_eq!(latest.id, model2_id);

    // List all models
    let models = store
        .list_authorization_models(store_id)
        .await
        .expect("Failed to list models");

    assert_eq!(models.len(), 2);

    // Models should be returned newest first
    assert_eq!(models[0].id, model2_id);
    assert_eq!(models[1].id, model_id);

    // Cleanup
    store.delete_store(store_id).await.unwrap();
}

#[tokio::test]
async fn test_authorization_model_parity_memory() {
    let store = create_memory_store();
    run_authorization_model_parity_test(&store, "parity-auth-model-memory").await;
}

#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_authorization_model_parity_postgres() {
    let store = create_postgres_store().await;
    run_authorization_model_parity_test(&store, "parity-auth-model-postgres").await;
}

#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_authorization_model_parity_mysql() {
    let store = create_mysql_store().await;
    run_authorization_model_parity_test(&store, "parity-auth-model-mysql").await;
}

#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_authorization_model_parity_cockroachdb() {
    let store = create_cockroachdb_store().await;
    run_authorization_model_parity_test(&store, "parity-auth-model-cockroachdb").await;
}

// ============================================================================
// Section 2: Pagination Data Integrity Tests
// ============================================================================

/// Generic helper: Test that pagination doesn't skip or duplicate items
async fn run_pagination_integrity_test<S: DataStore>(store: &S, store_id: &str) {
    store
        .create_store(store_id, "Pagination Integrity Test")
        .await
        .unwrap();

    // Write tuples with unique IDs
    let tuples: Vec<StoredTuple> = (0..PAGINATION_TEST_TUPLE_COUNT)
        .map(|i| StoredTuple {
            object_type: "document".to_string(),
            object_id: format!("doc{i:03}"), // Zero-padded for consistent ordering
            relation: "viewer".to_string(),
            user_type: "user".to_string(),
            user_id: format!("user{i:03}"),
            user_relation: None,
            condition_name: None,
            condition_context: None,
            created_at: None,
        })
        .collect();

    store.write_tuples(store_id, tuples, vec![]).await.unwrap();

    // Collect all tuples through pagination
    let mut all_collected: Vec<StoredTuple> = Vec::new();
    let mut continuation_token: Option<String> = None;

    loop {
        let pagination = PaginationOptions {
            page_size: Some(PAGINATION_PAGE_SIZE),
            continuation_token: continuation_token.clone(),
        };

        let result: PaginatedResult<StoredTuple> = store
            .read_tuples_paginated(store_id, &TupleFilter::default(), &pagination)
            .await
            .unwrap();

        all_collected.extend(result.items);
        continuation_token = result.continuation_token;

        if continuation_token.is_none() {
            break;
        }
    }

    // Verify no duplicates
    let unique_ids: HashSet<String> = all_collected
        .iter()
        .map(|t| format!("{}:{}", t.object_id, t.user_id))
        .collect();

    assert_eq!(
        unique_ids.len(),
        all_collected.len(),
        "Pagination should not return duplicates"
    );

    // Verify no skipping
    assert_eq!(
        all_collected.len(),
        PAGINATION_TEST_TUPLE_COUNT,
        "Pagination should return all {PAGINATION_TEST_TUPLE_COUNT} tuples without skipping"
    );

    // Verify all expected items are present
    for i in 0..PAGINATION_TEST_TUPLE_COUNT {
        let expected_id = format!("doc{i:03}:user{i:03}");
        assert!(
            unique_ids.contains(&expected_id),
            "Missing tuple: {expected_id}"
        );
    }

    // Cleanup
    store.delete_store(store_id).await.unwrap();
}

/// Generic helper: Test pagination with filters
async fn run_pagination_with_filter_integrity_test<S: DataStore>(store: &S, store_id: &str) {
    store
        .create_store(store_id, "Pagination Filter Integrity Test")
        .await
        .unwrap();

    // Write tuples for "document" type and "folder" type
    let mut tuples: Vec<StoredTuple> = Vec::new();

    for i in 0..FILTERED_PAGINATION_TUPLE_COUNT {
        tuples.push(StoredTuple {
            object_type: "document".to_string(),
            object_id: format!("doc{i:03}"),
            relation: "viewer".to_string(),
            user_type: "user".to_string(),
            user_id: format!("alice{i:03}"),
            user_relation: None,
            condition_name: None,
            condition_context: None,
            created_at: None,
        });
    }

    for i in 0..FILTERED_PAGINATION_TUPLE_COUNT {
        tuples.push(StoredTuple {
            object_type: "folder".to_string(),
            object_id: format!("folder{i:03}"),
            relation: "viewer".to_string(),
            user_type: "user".to_string(),
            user_id: format!("bob{i:03}"),
            user_relation: None,
            condition_name: None,
            condition_context: None,
            created_at: None,
        });
    }

    store.write_tuples(store_id, tuples, vec![]).await.unwrap();

    // Paginate with filter for "document" type only
    let filter = TupleFilter {
        object_type: Some("document".to_string()),
        ..Default::default()
    };

    let mut all_collected: Vec<StoredTuple> = Vec::new();
    let mut continuation_token: Option<String> = None;

    loop {
        let pagination = PaginationOptions {
            page_size: Some(FILTERED_PAGINATION_PAGE_SIZE),
            continuation_token: continuation_token.clone(),
        };

        let result = store
            .read_tuples_paginated(store_id, &filter, &pagination)
            .await
            .unwrap();

        all_collected.extend(result.items);
        continuation_token = result.continuation_token;

        if continuation_token.is_none() {
            break;
        }
    }

    // Should only get document tuples
    assert_eq!(
        all_collected.len(),
        FILTERED_PAGINATION_TUPLE_COUNT,
        "Filter should return exactly {FILTERED_PAGINATION_TUPLE_COUNT} document tuples"
    );

    // All should be documents
    assert!(
        all_collected.iter().all(|t| t.object_type == "document"),
        "All returned tuples should be documents"
    );

    // Verify no duplicates
    let unique_ids: HashSet<String> = all_collected.iter().map(|t| t.object_id.clone()).collect();
    assert_eq!(
        unique_ids.len(),
        FILTERED_PAGINATION_TUPLE_COUNT,
        "No duplicates in filtered results"
    );

    // Cleanup
    store.delete_store(store_id).await.unwrap();
}

#[tokio::test]
async fn test_pagination_integrity_memory() {
    let store = create_memory_store();
    run_pagination_integrity_test(&store, "parity-pagination-memory").await;
}

#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_pagination_integrity_postgres() {
    let store = create_postgres_store().await;
    run_pagination_integrity_test(&store, "parity-pagination-postgres").await;
}

#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_pagination_integrity_mysql() {
    let store = create_mysql_store().await;
    run_pagination_integrity_test(&store, "parity-pagination-mysql").await;
}

#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_pagination_integrity_cockroachdb() {
    let store = create_cockroachdb_store().await;
    run_pagination_integrity_test(&store, "parity-pagination-cockroachdb").await;
}

#[tokio::test]
async fn test_pagination_with_filter_integrity_memory() {
    let store = create_memory_store();
    run_pagination_with_filter_integrity_test(&store, "parity-pag-filter-memory").await;
}

#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_pagination_with_filter_integrity_postgres() {
    let store = create_postgres_store().await;
    run_pagination_with_filter_integrity_test(&store, "parity-pag-filter-postgres").await;
}

#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_pagination_with_filter_integrity_mysql() {
    let store = create_mysql_store().await;
    run_pagination_with_filter_integrity_test(&store, "parity-pag-filter-mysql").await;
}

#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_pagination_with_filter_integrity_cockroachdb() {
    let store = create_cockroachdb_store().await;
    run_pagination_with_filter_integrity_test(&store, "parity-pag-filter-cockroachdb").await;
}

/// Generic helper: Test invalid continuation token handling
async fn run_invalid_continuation_token_test<S: DataStore>(store: &S, store_id: &str) {
    store
        .create_store(store_id, "Invalid Token Test")
        .await
        .unwrap();

    // Write some tuples to ensure store has data
    let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);
    store.write_tuple(store_id, tuple).await.unwrap();

    // Test 1: Completely invalid base64 token
    let pagination = PaginationOptions {
        page_size: Some(10),
        continuation_token: Some("not-valid-base64!!!".to_string()),
    };
    let result = store
        .read_tuples_paginated(store_id, &TupleFilter::default(), &pagination)
        .await;
    assert!(
        matches!(result, Err(StorageError::InvalidInput { .. })),
        "Invalid base64 token should return InvalidInput, got: {result:?}"
    );

    // Test 2: Valid base64 but invalid JSON content
    let pagination = PaginationOptions {
        page_size: Some(10),
        continuation_token: Some(base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            "not-valid-json",
        )),
    };
    let result = store
        .read_tuples_paginated(store_id, &TupleFilter::default(), &pagination)
        .await;
    assert!(
        matches!(result, Err(StorageError::InvalidInput { .. })),
        "Invalid JSON in token should return InvalidInput, got: {result:?}"
    );

    // Test 3: Negative page size (if supported) - backends should handle gracefully
    // Note: This tests backend-specific behavior, some may accept 0 or negative
    let pagination = PaginationOptions {
        page_size: Some(0),
        continuation_token: None,
    };
    let result = store
        .read_tuples_paginated(store_id, &TupleFilter::default(), &pagination)
        .await;
    // Either succeeds with empty result or returns error - both acceptable
    assert!(
        result.is_ok() || matches!(result, Err(StorageError::InvalidInput { .. })),
        "Zero page size should either succeed or return InvalidInput, got: {result:?}"
    );

    // Cleanup
    store.delete_store(store_id).await.unwrap();
}

#[tokio::test]
async fn test_invalid_continuation_token_memory() {
    let store = create_memory_store();
    run_invalid_continuation_token_test(&store, "parity-invalid-token-memory").await;
}

#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_invalid_continuation_token_postgres() {
    let store = create_postgres_store().await;
    run_invalid_continuation_token_test(&store, "parity-invalid-token-postgres").await;
}

#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_invalid_continuation_token_mysql() {
    let store = create_mysql_store().await;
    run_invalid_continuation_token_test(&store, "parity-invalid-token-mysql").await;
}

#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_invalid_continuation_token_cockroachdb() {
    let store = create_cockroachdb_store().await;
    run_invalid_continuation_token_test(&store, "parity-invalid-token-cockroachdb").await;
}

// ============================================================================
// Section 3: Concurrent Write Consistency Tests
// ============================================================================

/// Generic helper: Test concurrent writes don't corrupt state.
///
/// This test also verifies actual parallelism by measuring elapsed time.
/// If operations were serialized, total time would be ~N * single_op_time.
/// Parallel execution should complete in significantly less time.
async fn run_concurrent_write_consistency_test<S: DataStore + 'static>(
    store: Arc<S>,
    store_id: &str,
) {
    store
        .create_store(store_id, "Concurrent Write Test")
        .await
        .unwrap();

    let store_id_owned = store_id.to_string();

    // Measure a single write to establish baseline
    let baseline_start = Instant::now();
    let baseline_tuple = StoredTuple {
        object_type: "document".to_string(),
        object_id: "baseline-doc".to_string(),
        relation: "viewer".to_string(),
        user_type: "user".to_string(),
        user_id: "baseline-user".to_string(),
        user_relation: None,
        condition_name: None,
        condition_context: None,
        created_at: None,
    };
    store
        .write_tuple(&store_id_owned, baseline_tuple)
        .await
        .unwrap();
    let single_write_time = baseline_start.elapsed();

    // Spawn concurrent tasks, each writing unique tuples
    let mut handles = Vec::new();
    let parallel_start = Instant::now();

    for task_id in 0..CONCURRENT_TASK_COUNT {
        let store = Arc::clone(&store);
        let store_id = store_id_owned.clone();

        handles.push(tokio::spawn(async move {
            for tuple_id in 0..TUPLES_PER_TASK {
                let tuple = StoredTuple {
                    object_type: "document".to_string(),
                    object_id: format!("doc-{task_id}-{tuple_id}"),
                    relation: "viewer".to_string(),
                    user_type: "user".to_string(),
                    user_id: format!("user-{task_id}"),
                    user_relation: None,
                    condition_name: None,
                    condition_context: None,
                    created_at: None,
                };

                store
                    .write_tuple(&store_id, tuple)
                    .await
                    .expect("Concurrent write should succeed");
            }
        }));
    }

    // Wait for all writes to complete
    for handle in handles {
        handle.await.expect("Task should complete");
    }

    let parallel_time = parallel_start.elapsed();

    // Verify all tuples were written (excluding baseline)
    let all_tuples = store
        .read_tuples(&store_id_owned, &TupleFilter::default())
        .await
        .unwrap();

    // Should have baseline + TOTAL_CONCURRENT_TUPLES
    assert_eq!(
        all_tuples.len(),
        TOTAL_CONCURRENT_TUPLES + 1,
        "All concurrent writes should be persisted without loss"
    );

    // Verify no duplicates
    let unique_ids: HashSet<String> = all_tuples.iter().map(|t| t.object_id.clone()).collect();
    assert_eq!(
        unique_ids.len(),
        TOTAL_CONCURRENT_TUPLES + 1,
        "No duplicates should exist after concurrent writes"
    );

    // Verify parallelism: if truly parallel, time should be much less than sequential
    // Sequential would take: single_write_time * TOTAL_CONCURRENT_TUPLES
    // We expect at least MIN_PARALLELISM_SPEEDUP improvement
    let expected_sequential_time = single_write_time * TOTAL_CONCURRENT_TUPLES as u32;
    let min_expected_speedup = expected_sequential_time.as_secs_f64() / MIN_PARALLELISM_SPEEDUP;

    // Only assert on speedup if single writes take meaningful time (>1ms)
    // In-memory stores may be too fast for timing assertions
    if single_write_time > Duration::from_millis(1) {
        assert!(
            parallel_time.as_secs_f64() < min_expected_speedup,
            "Parallel execution should be at least {MIN_PARALLELISM_SPEEDUP}x faster than sequential. \
             Single write: {:?}, Parallel total: {:?}, Expected sequential: {:?}",
            single_write_time,
            parallel_time,
            expected_sequential_time
        );
    }

    // Cleanup
    store.delete_store(&store_id_owned).await.unwrap();
}

/// Generic helper: Test concurrent batch writes
async fn run_concurrent_batch_write_test<S: DataStore + 'static>(store: Arc<S>, store_id: &str) {
    store
        .create_store(store_id, "Concurrent Batch Write Test")
        .await
        .unwrap();

    let store_id_owned = store_id.to_string();

    // Spawn concurrent tasks, each batch-writing tuples
    let mut handles = Vec::new();
    let start = Instant::now();

    for task_id in 0..BATCH_CONCURRENT_TASK_COUNT {
        let store = Arc::clone(&store);
        let store_id = store_id_owned.clone();

        handles.push(tokio::spawn(async move {
            let tuples: Vec<StoredTuple> = (0..BATCH_TUPLES_PER_TASK)
                .map(|tuple_id| StoredTuple {
                    object_type: "document".to_string(),
                    object_id: format!("batch-{task_id}-{tuple_id}"),
                    relation: "viewer".to_string(),
                    user_type: "user".to_string(),
                    user_id: format!("batch-user-{task_id}"),
                    user_relation: None,
                    condition_name: None,
                    condition_context: None,
                    created_at: None,
                })
                .collect();

            store
                .write_tuples(&store_id, tuples, vec![])
                .await
                .expect("Batch write should succeed");
        }));
    }

    // Wait for all writes
    for handle in handles {
        handle.await.expect("Task should complete");
    }

    let elapsed = start.elapsed();

    // Verify all tuples
    let all_tuples = store
        .read_tuples(&store_id_owned, &TupleFilter::default())
        .await
        .unwrap();

    assert_eq!(
        all_tuples.len(),
        TOTAL_BATCH_TUPLES,
        "All batch writes should be persisted"
    );

    // Log timing for visibility (useful for debugging parallelism)
    eprintln!(
        "Batch concurrent write: {} tuples in {:?}",
        TOTAL_BATCH_TUPLES, elapsed
    );

    // Cleanup
    store.delete_store(&store_id_owned).await.unwrap();
}

#[tokio::test]
async fn test_concurrent_write_consistency_memory() {
    let store = Arc::new(create_memory_store());
    run_concurrent_write_consistency_test(store, "parity-concurrent-memory").await;
}

#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_concurrent_write_consistency_postgres() {
    let store = Arc::new(create_postgres_store().await);
    run_concurrent_write_consistency_test(store, "parity-concurrent-postgres").await;
}

#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_concurrent_write_consistency_mysql() {
    let store = Arc::new(create_mysql_store().await);
    run_concurrent_write_consistency_test(store, "parity-concurrent-mysql").await;
}

#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_concurrent_write_consistency_cockroachdb() {
    let store = Arc::new(create_cockroachdb_store().await);
    run_concurrent_write_consistency_test(store, "parity-concurrent-cockroachdb").await;
}

#[tokio::test]
async fn test_concurrent_batch_write_memory() {
    let store = Arc::new(create_memory_store());
    run_concurrent_batch_write_test(store, "parity-batch-concurrent-memory").await;
}

#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_concurrent_batch_write_postgres() {
    let store = Arc::new(create_postgres_store().await);
    run_concurrent_batch_write_test(store, "parity-batch-concurrent-postgres").await;
}

#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_concurrent_batch_write_mysql() {
    let store = Arc::new(create_mysql_store().await);
    run_concurrent_batch_write_test(store, "parity-batch-concurrent-mysql").await;
}

#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_concurrent_batch_write_cockroachdb() {
    let store = Arc::new(create_cockroachdb_store().await);
    run_concurrent_batch_write_test(store, "parity-batch-concurrent-cockroachdb").await;
}

// ============================================================================
// Section 4: Error Handling Parity Tests
// ============================================================================

/// Generic helper: Test error responses are consistent
async fn run_error_handling_parity_test<S: DataStore>(store: &S, store_id: &str) {
    // Test 1: StoreNotFound error
    let result = store.get_store("nonexistent-store-12345").await;
    assert!(
        matches!(result, Err(StorageError::StoreNotFound { .. })),
        "Should return StoreNotFound for non-existent store, got: {result:?}"
    );

    // Test 2: StoreAlreadyExists error
    store
        .create_store(store_id, "Error Test Store")
        .await
        .unwrap();

    let result = store.create_store(store_id, "Duplicate Store").await;
    assert!(
        matches!(result, Err(StorageError::StoreAlreadyExists { .. })),
        "Should return StoreAlreadyExists for duplicate store, got: {result:?}"
    );

    // Test 3: Invalid store_id validation
    let result = store.create_store("", "Empty ID Store").await;
    assert!(
        matches!(result, Err(StorageError::InvalidInput { .. })),
        "Should return InvalidInput for empty store_id, got: {result:?}"
    );

    // Test 4: Invalid store name validation
    let result = store.create_store("valid-id", "").await;
    assert!(
        matches!(result, Err(StorageError::InvalidInput { .. })),
        "Should return InvalidInput for empty store name, got: {result:?}"
    );

    // Test 5: Store ID too long (>255 chars)
    let long_id = "x".repeat(256);
    let result = store.create_store(&long_id, "Long ID Store").await;
    assert!(
        matches!(result, Err(StorageError::InvalidInput { .. })),
        "Should return InvalidInput for store_id > 255 chars, got: {result:?}"
    );

    // Test 6: ModelNotFound error
    let result = store
        .get_authorization_model(store_id, "nonexistent-model")
        .await;
    assert!(
        matches!(result, Err(StorageError::ModelNotFound { .. })),
        "Should return ModelNotFound for non-existent model, got: {result:?}"
    );

    // Test 7: Read tuples from non-existent store
    let result = store
        .read_tuples("nonexistent-store-xyz", &TupleFilter::default())
        .await;
    assert!(
        matches!(result, Err(StorageError::StoreNotFound { .. })),
        "Should return StoreNotFound when reading from non-existent store, got: {result:?}"
    );

    // Test 8: Write tuple to non-existent store
    let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);
    let result = store.write_tuple("nonexistent-store-abc", tuple).await;
    assert!(
        matches!(result, Err(StorageError::StoreNotFound { .. })),
        "Should return StoreNotFound when writing to non-existent store, got: {result:?}"
    );

    // Cleanup
    store.delete_store(store_id).await.unwrap();
}

/// Test error message consistency across backends
async fn run_error_message_parity_test<S: DataStore>(store: &S) {
    // Get StoreNotFound error and verify message format
    let result = store.get_store("error-test-store-not-found").await;

    if let Err(StorageError::StoreNotFound { store_id }) = result {
        assert_eq!(store_id, "error-test-store-not-found");
    } else {
        panic!("Expected StoreNotFound error");
    }

    // Get InvalidInput error and verify it mentions the field
    let result = store.create_store("", "Test").await;

    if let Err(StorageError::InvalidInput { message }) = result {
        assert!(
            message.contains("store_id"),
            "Error message should mention store_id: {message}"
        );
    } else {
        panic!("Expected InvalidInput error");
    }
}

/// Test invalid tuple field validation across backends
async fn run_invalid_tuple_field_test<S: DataStore>(store: &S, store_id: &str) {
    store
        .create_store(store_id, "Invalid Tuple Field Test")
        .await
        .unwrap();

    // Test 1: Empty object_type should fail
    let invalid_tuple = StoredTuple {
        object_type: "".to_string(), // Empty - invalid
        object_id: "doc1".to_string(),
        relation: "viewer".to_string(),
        user_type: "user".to_string(),
        user_id: "alice".to_string(),
        user_relation: None,
        condition_name: None,
        condition_context: None,
        created_at: None,
    };
    let result = store.write_tuple(store_id, invalid_tuple).await;
    assert!(
        matches!(result, Err(StorageError::InvalidInput { .. })),
        "Empty object_type should return InvalidInput, got: {result:?}"
    );

    // Test 2: Empty object_id should fail
    let invalid_tuple = StoredTuple {
        object_type: "document".to_string(),
        object_id: "".to_string(), // Empty - invalid
        relation: "viewer".to_string(),
        user_type: "user".to_string(),
        user_id: "alice".to_string(),
        user_relation: None,
        condition_name: None,
        condition_context: None,
        created_at: None,
    };
    let result = store.write_tuple(store_id, invalid_tuple).await;
    assert!(
        matches!(result, Err(StorageError::InvalidInput { .. })),
        "Empty object_id should return InvalidInput, got: {result:?}"
    );

    // Test 3: Empty relation should fail
    let invalid_tuple = StoredTuple {
        object_type: "document".to_string(),
        object_id: "doc1".to_string(),
        relation: "".to_string(), // Empty - invalid
        user_type: "user".to_string(),
        user_id: "alice".to_string(),
        user_relation: None,
        condition_name: None,
        condition_context: None,
        created_at: None,
    };
    let result = store.write_tuple(store_id, invalid_tuple).await;
    assert!(
        matches!(result, Err(StorageError::InvalidInput { .. })),
        "Empty relation should return InvalidInput, got: {result:?}"
    );

    // Test 4: Empty user_type should fail
    let invalid_tuple = StoredTuple {
        object_type: "document".to_string(),
        object_id: "doc1".to_string(),
        relation: "viewer".to_string(),
        user_type: "".to_string(), // Empty - invalid
        user_id: "alice".to_string(),
        user_relation: None,
        condition_name: None,
        condition_context: None,
        created_at: None,
    };
    let result = store.write_tuple(store_id, invalid_tuple).await;
    assert!(
        matches!(result, Err(StorageError::InvalidInput { .. })),
        "Empty user_type should return InvalidInput, got: {result:?}"
    );

    // Test 5: Empty user_id should fail
    let invalid_tuple = StoredTuple {
        object_type: "document".to_string(),
        object_id: "doc1".to_string(),
        relation: "viewer".to_string(),
        user_type: "user".to_string(),
        user_id: "".to_string(), // Empty - invalid
        user_relation: None,
        condition_name: None,
        condition_context: None,
        created_at: None,
    };
    let result = store.write_tuple(store_id, invalid_tuple).await;
    assert!(
        matches!(result, Err(StorageError::InvalidInput { .. })),
        "Empty user_id should return InvalidInput, got: {result:?}"
    );

    // Test 6: Object type with colon (SQL injection attempt) should fail
    let invalid_tuple = StoredTuple {
        object_type: "document:admin".to_string(), // Contains colon - invalid
        object_id: "doc1".to_string(),
        relation: "viewer".to_string(),
        user_type: "user".to_string(),
        user_id: "alice".to_string(),
        user_relation: None,
        condition_name: None,
        condition_context: None,
        created_at: None,
    };
    let result = store.write_tuple(store_id, invalid_tuple).await;
    assert!(
        matches!(result, Err(StorageError::InvalidInput { .. })),
        "Object type with colon should return InvalidInput, got: {result:?}"
    );

    // Cleanup
    store.delete_store(store_id).await.unwrap();
}

#[tokio::test]
async fn test_error_handling_parity_memory() {
    let store = create_memory_store();
    run_error_handling_parity_test(&store, "parity-error-memory").await;
    run_error_message_parity_test(&store).await;
}

#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_error_handling_parity_postgres() {
    let store = create_postgres_store().await;
    run_error_handling_parity_test(&store, "parity-error-postgres").await;
    run_error_message_parity_test(&store).await;
}

#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_error_handling_parity_mysql() {
    let store = create_mysql_store().await;
    run_error_handling_parity_test(&store, "parity-error-mysql").await;
    run_error_message_parity_test(&store).await;
}

#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_error_handling_parity_cockroachdb() {
    let store = create_cockroachdb_store().await;
    run_error_handling_parity_test(&store, "parity-error-cockroachdb").await;
    run_error_message_parity_test(&store).await;
}

#[tokio::test]
async fn test_invalid_tuple_field_memory() {
    let store = create_memory_store();
    run_invalid_tuple_field_test(&store, "parity-invalid-tuple-memory").await;
}

#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_invalid_tuple_field_postgres() {
    let store = create_postgres_store().await;
    run_invalid_tuple_field_test(&store, "parity-invalid-tuple-postgres").await;
}

#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_invalid_tuple_field_mysql() {
    let store = create_mysql_store().await;
    run_invalid_tuple_field_test(&store, "parity-invalid-tuple-mysql").await;
}

#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_invalid_tuple_field_cockroachdb() {
    let store = create_cockroachdb_store().await;
    run_invalid_tuple_field_test(&store, "parity-invalid-tuple-cockroachdb").await;
}

// ============================================================================
// Section 5: Conditional Tuple Tests (PostgreSQL-specific with parity check)
// ============================================================================

/// Generic helper: Test tuple storage with condition context (PostgreSQL feature)
async fn run_conditional_tuple_test_postgres(store: &PostgresDataStore, store_id: &str) {
    store
        .create_store(store_id, "Conditional Tuple Test")
        .await
        .unwrap();

    // Create tuple with condition
    let mut condition_context = std::collections::HashMap::new();
    condition_context.insert("ip_address".to_string(), serde_json::json!("192.168.1.1"));
    condition_context.insert("time_of_day".to_string(), serde_json::json!("morning"));

    let tuple = StoredTuple {
        object_type: "document".to_string(),
        object_id: "confidential-doc".to_string(),
        relation: "viewer".to_string(),
        user_type: "user".to_string(),
        user_id: "conditional-user".to_string(),
        user_relation: None,
        condition_name: Some("ip_based_access".to_string()),
        condition_context: Some(condition_context.clone()),
        created_at: None,
    };

    store.write_tuple(store_id, tuple).await.unwrap();

    // Read tuple back
    let tuples = store
        .read_tuples(store_id, &TupleFilter::default())
        .await
        .unwrap();

    assert_eq!(tuples.len(), 1);

    let retrieved = &tuples[0];
    assert_eq!(
        retrieved.condition_name,
        Some("ip_based_access".to_string())
    );
    assert!(retrieved.condition_context.is_some());

    let ctx = retrieved.condition_context.as_ref().unwrap();
    assert_eq!(
        ctx.get("ip_address"),
        Some(&serde_json::json!("192.168.1.1"))
    );
    assert_eq!(ctx.get("time_of_day"), Some(&serde_json::json!("morning")));

    // Cleanup
    store.delete_store(store_id).await.unwrap();
}

/// Test that backends without condition support handle tuples correctly
async fn run_tuple_without_condition_test<S: DataStore>(store: &S, store_id: &str) {
    store
        .create_store(store_id, "No Condition Tuple Test")
        .await
        .unwrap();

    // Write tuple without condition
    let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);

    store.write_tuple(store_id, tuple).await.unwrap();

    // Read back
    let tuples = store
        .read_tuples(store_id, &TupleFilter::default())
        .await
        .unwrap();

    assert_eq!(tuples.len(), 1);
    assert!(tuples[0].condition_name.is_none());
    assert!(tuples[0].condition_context.is_none());

    // Cleanup
    store.delete_store(store_id).await.unwrap();
}

#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_conditional_tuple_postgres() {
    let store = create_postgres_store().await;
    run_conditional_tuple_test_postgres(&store, "parity-conditional-postgres").await;
}

#[tokio::test]
async fn test_tuple_without_condition_memory() {
    let store = create_memory_store();
    run_tuple_without_condition_test(&store, "parity-no-cond-memory").await;
}

#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_tuple_without_condition_postgres() {
    let store = create_postgres_store().await;
    run_tuple_without_condition_test(&store, "parity-no-cond-postgres").await;
}

#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_tuple_without_condition_mysql() {
    let store = create_mysql_store().await;
    run_tuple_without_condition_test(&store, "parity-no-cond-mysql").await;
}

#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_tuple_without_condition_cockroachdb() {
    let store = create_cockroachdb_store().await;
    run_tuple_without_condition_test(&store, "parity-no-cond-cockroachdb").await;
}

// ============================================================================
// Section 6: List Operations Parity Tests
// ============================================================================

/// Generic helper: Test list_stores parity
async fn run_list_stores_parity_test<S: DataStore>(store: &S, prefix: &str) {
    // Create multiple stores
    for i in 0..LIST_STORES_COUNT {
        store
            .create_store(&format!("{prefix}-store-{i}"), &format!("Test Store {i}"))
            .await
            .unwrap();
    }

    // List all stores
    let stores = store.list_stores().await.unwrap();

    // Should have at least LIST_STORES_COUNT stores
    let test_stores: Vec<&Store> = stores.iter().filter(|s| s.id.starts_with(prefix)).collect();
    assert_eq!(
        test_stores.len(),
        LIST_STORES_COUNT,
        "Should find all {LIST_STORES_COUNT} created stores"
    );

    // Verify each store has expected fields
    for s in &test_stores {
        assert!(!s.id.is_empty());
        assert!(!s.name.is_empty());
    }

    // Cleanup
    for i in 0..LIST_STORES_COUNT {
        let _ = store.delete_store(&format!("{prefix}-store-{i}")).await;
    }
}

/// Generic helper: Test list_stores_paginated parity
async fn run_list_stores_paginated_parity_test<S: DataStore>(store: &S, prefix: &str) {
    // Create stores for pagination test
    for i in 0..LIST_STORES_PAGINATED_COUNT {
        store
            .create_store(
                &format!("{prefix}-pag-store-{i:02}"),
                &format!("Paginated Store {i}"),
            )
            .await
            .unwrap();
    }

    // Paginate through stores
    let mut all_stores: Vec<Store> = Vec::new();
    let mut continuation_token: Option<String> = None;

    loop {
        let pagination = PaginationOptions {
            page_size: Some(LIST_STORES_PAGE_SIZE),
            continuation_token: continuation_token.clone(),
        };

        let result = store.list_stores_paginated(&pagination).await.unwrap();

        all_stores.extend(result.items);
        continuation_token = result.continuation_token;

        if continuation_token.is_none() {
            break;
        }
    }

    // Filter to our test stores
    let test_stores: Vec<&Store> = all_stores
        .iter()
        .filter(|s| s.id.starts_with(&format!("{prefix}-pag-store")))
        .collect();

    // Verify no duplicates
    let unique_ids: HashSet<&String> = test_stores.iter().map(|s| &s.id).collect();
    assert_eq!(
        unique_ids.len(),
        test_stores.len(),
        "No duplicate stores in pagination"
    );

    // Should have all stores
    assert_eq!(
        test_stores.len(),
        LIST_STORES_PAGINATED_COUNT,
        "Should paginate through all {LIST_STORES_PAGINATED_COUNT} stores"
    );

    // Cleanup
    for i in 0..LIST_STORES_PAGINATED_COUNT {
        let _ = store
            .delete_store(&format!("{prefix}-pag-store-{i:02}"))
            .await;
    }
}

#[tokio::test]
async fn test_list_stores_parity_memory() {
    let store = create_memory_store();
    run_list_stores_parity_test(&store, "parity-list-memory").await;
}

#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_list_stores_parity_postgres() {
    let store = create_postgres_store().await;
    run_list_stores_parity_test(&store, "parity-list-postgres").await;
}

#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_list_stores_parity_mysql() {
    let store = create_mysql_store().await;
    run_list_stores_parity_test(&store, "parity-list-mysql").await;
}

#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_list_stores_parity_cockroachdb() {
    let store = create_cockroachdb_store().await;
    run_list_stores_parity_test(&store, "parity-list-cockroachdb").await;
}

#[tokio::test]
async fn test_list_stores_paginated_parity_memory() {
    let store = create_memory_store();
    run_list_stores_paginated_parity_test(&store, "parity-lstpag-memory").await;
}

#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_list_stores_paginated_parity_postgres() {
    let store = create_postgres_store().await;
    run_list_stores_paginated_parity_test(&store, "parity-lstpag-postgres").await;
}

#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_list_stores_paginated_parity_mysql() {
    let store = create_mysql_store().await;
    run_list_stores_paginated_parity_test(&store, "parity-lstpag-mysql").await;
}

#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_list_stores_paginated_parity_cockroachdb() {
    let store = create_cockroachdb_store().await;
    run_list_stores_paginated_parity_test(&store, "parity-lstpag-cockroachdb").await;
}

// ============================================================================
// Section 7: Atomic Write/Delete Operations Parity
// ============================================================================

/// Generic helper: Test atomic write and delete in single operation
async fn run_atomic_write_delete_parity_test<S: DataStore>(store: &S, store_id: &str) {
    store
        .create_store(store_id, "Atomic Write/Delete Test")
        .await
        .unwrap();

    // Initial tuples
    let tuple1 = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);
    let tuple2 = StoredTuple::new("document", "doc2", "viewer", "user", "bob", None);
    let tuple3 = StoredTuple::new("document", "doc3", "viewer", "user", "charlie", None);

    // Write initial tuples
    store
        .write_tuples(store_id, vec![tuple1.clone(), tuple2.clone()], vec![])
        .await
        .unwrap();

    // Verify initial state
    let tuples = store
        .read_tuples(store_id, &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 2);

    // Atomic operation: add tuple3, delete tuple1
    store
        .write_tuples(store_id, vec![tuple3.clone()], vec![tuple1.clone()])
        .await
        .unwrap();

    // Verify final state
    let tuples = store
        .read_tuples(store_id, &TupleFilter::default())
        .await
        .unwrap();

    assert_eq!(tuples.len(), 2, "Should have 2 tuples after atomic op");

    let ids: HashSet<&str> = tuples.iter().map(|t| t.object_id.as_str()).collect();
    assert!(!ids.contains("doc1"), "doc1 should be deleted in atomic op");
    assert!(ids.contains("doc2"), "doc2 should still exist");
    assert!(ids.contains("doc3"), "doc3 should be added in atomic op");

    // Cleanup
    store.delete_store(store_id).await.unwrap();
}

/// Generic helper: Test delete non-existent tuple behavior
async fn run_delete_nonexistent_tuple_test<S: DataStore>(store: &S, store_id: &str) {
    store
        .create_store(store_id, "Delete Non-existent Test")
        .await
        .unwrap();

    // Try to delete a tuple that doesn't exist
    let nonexistent_tuple =
        StoredTuple::new("document", "nonexistent", "viewer", "user", "alice", None);

    // This should succeed (idempotent delete) or return TupleNotFound
    // depending on backend implementation
    let result = store.delete_tuple(store_id, nonexistent_tuple).await;

    // Either succeeds (idempotent) or returns TupleNotFound - both are valid
    assert!(
        result.is_ok() || matches!(result, Err(StorageError::TupleNotFound { .. })),
        "Deleting non-existent tuple should succeed or return TupleNotFound"
    );

    // Cleanup
    store.delete_store(store_id).await.unwrap();
}

#[tokio::test]
async fn test_atomic_write_delete_parity_memory() {
    let store = create_memory_store();
    run_atomic_write_delete_parity_test(&store, "parity-atomic-memory").await;
}

#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_atomic_write_delete_parity_postgres() {
    let store = create_postgres_store().await;
    run_atomic_write_delete_parity_test(&store, "parity-atomic-postgres").await;
}

#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_atomic_write_delete_parity_mysql() {
    let store = create_mysql_store().await;
    run_atomic_write_delete_parity_test(&store, "parity-atomic-mysql").await;
}

#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_atomic_write_delete_parity_cockroachdb() {
    let store = create_cockroachdb_store().await;
    run_atomic_write_delete_parity_test(&store, "parity-atomic-cockroachdb").await;
}

#[tokio::test]
async fn test_delete_nonexistent_tuple_memory() {
    let store = create_memory_store();
    run_delete_nonexistent_tuple_test(&store, "parity-del-nonexist-memory").await;
}

#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_delete_nonexistent_tuple_postgres() {
    let store = create_postgres_store().await;
    run_delete_nonexistent_tuple_test(&store, "parity-del-nonexist-postgres").await;
}

#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_delete_nonexistent_tuple_mysql() {
    let store = create_mysql_store().await;
    run_delete_nonexistent_tuple_test(&store, "parity-del-nonexist-mysql").await;
}

#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_delete_nonexistent_tuple_cockroachdb() {
    let store = create_cockroachdb_store().await;
    run_delete_nonexistent_tuple_test(&store, "parity-del-nonexist-cockroachdb").await;
}

// ============================================================================
// Section 8: Large Dataset Parity Tests (10k+ tuples)
// ============================================================================

/// Generic helper: Test large dataset handling is consistent
async fn run_large_dataset_parity_test<S: DataStore>(store: &S, store_id: &str) {
    store
        .create_store(store_id, "Large Dataset Parity Test")
        .await
        .unwrap();

    // Generate tuples for large dataset test
    let tuples: Vec<StoredTuple> = (0..LARGE_DATASET_TUPLE_COUNT)
        .map(|i| StoredTuple {
            object_type: "document".to_string(),
            object_id: format!("large-doc-{i:05}"),
            relation: "viewer".to_string(),
            user_type: "user".to_string(),
            user_id: format!("large-user-{}", i % LARGE_DATASET_USER_COUNT),
            user_relation: None,
            condition_name: None,
            condition_context: None,
            created_at: None,
        })
        .collect();

    // Write all tuples
    store.write_tuples(store_id, tuples, vec![]).await.unwrap();

    // Read all tuples (non-paginated)
    let all_tuples = store
        .read_tuples(store_id, &TupleFilter::default())
        .await
        .unwrap();

    assert_eq!(
        all_tuples.len(),
        LARGE_DATASET_TUPLE_COUNT,
        "Should read all {LARGE_DATASET_TUPLE_COUNT} tuples"
    );

    // Filter by user
    let filter = TupleFilter {
        user: Some("user:large-user-0".to_string()),
        ..Default::default()
    };

    let filtered = store.read_tuples(store_id, &filter).await.unwrap();

    // User 0 should have LARGE_DATASET_TUPLE_COUNT / LARGE_DATASET_USER_COUNT tuples
    let expected_per_user = LARGE_DATASET_TUPLE_COUNT / LARGE_DATASET_USER_COUNT;
    assert_eq!(
        filtered.len(),
        expected_per_user,
        "User filter should return {expected_per_user} tuples for user-0"
    );

    // Paginate through entire dataset
    let mut paginated_count = 0;
    let mut continuation_token: Option<String> = None;

    loop {
        let pagination = PaginationOptions {
            page_size: Some(LARGE_DATASET_PAGE_SIZE),
            continuation_token: continuation_token.clone(),
        };

        let result = store
            .read_tuples_paginated(store_id, &TupleFilter::default(), &pagination)
            .await
            .unwrap();

        paginated_count += result.items.len();
        continuation_token = result.continuation_token;

        if continuation_token.is_none() {
            break;
        }
    }

    assert_eq!(
        paginated_count, LARGE_DATASET_TUPLE_COUNT,
        "Pagination should return all {LARGE_DATASET_TUPLE_COUNT} tuples"
    );

    // Cleanup
    store.delete_store(store_id).await.unwrap();
}

#[tokio::test]
async fn test_large_dataset_parity_memory() {
    let store = create_memory_store();
    run_large_dataset_parity_test(&store, "parity-large-memory").await;
}

#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_large_dataset_parity_postgres() {
    let store = create_postgres_store().await;
    run_large_dataset_parity_test(&store, "parity-large-postgres").await;
}

#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_large_dataset_parity_mysql() {
    let store = create_mysql_store().await;
    run_large_dataset_parity_test(&store, "parity-large-mysql").await;
}

#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_large_dataset_parity_cockroachdb() {
    let store = create_cockroachdb_store().await;
    run_large_dataset_parity_test(&store, "parity-large-cockroachdb").await;
}

// ============================================================================
// Section 9: User Filter Parity Tests
// ============================================================================

/// Generic helper: Test various user filter formats
async fn run_user_filter_parity_test<S: DataStore>(store: &S, store_id: &str) {
    store
        .create_store(store_id, "User Filter Parity Test")
        .await
        .unwrap();

    // Write tuples with different user formats
    let tuples = vec![
        // Direct user
        StoredTuple::new("document", "doc1", "viewer", "user", "alice", None),
        // Another direct user
        StoredTuple::new("document", "doc2", "viewer", "user", "bob", None),
        // Userset (group#member)
        StoredTuple::new(
            "document",
            "doc3",
            "viewer",
            "group",
            "engineering",
            Some("member".to_string()),
        ),
        // Wildcard-like (everyone)
        StoredTuple::new("document", "doc4", "viewer", "user", "*", None),
    ];

    store.write_tuples(store_id, tuples, vec![]).await.unwrap();

    // Filter by direct user
    let filter = TupleFilter {
        user: Some("user:alice".to_string()),
        ..Default::default()
    };
    let results = store.read_tuples(store_id, &filter).await.unwrap();
    assert_eq!(results.len(), 1, "Should find 1 tuple for user:alice");
    assert_eq!(results[0].user_id, "alice");

    // Filter by userset
    let filter = TupleFilter {
        user: Some("group:engineering#member".to_string()),
        ..Default::default()
    };
    let results = store.read_tuples(store_id, &filter).await.unwrap();
    assert_eq!(results.len(), 1, "Should find 1 tuple for group userset");
    assert_eq!(results[0].user_type, "group");
    assert_eq!(results[0].user_id, "engineering");
    assert_eq!(results[0].user_relation, Some("member".to_string()));

    // Filter by wildcard user
    let filter = TupleFilter {
        user: Some("user:*".to_string()),
        ..Default::default()
    };
    let results = store.read_tuples(store_id, &filter).await.unwrap();
    assert_eq!(results.len(), 1, "Should find 1 tuple for wildcard user");
    assert_eq!(results[0].user_id, "*");

    // Cleanup
    store.delete_store(store_id).await.unwrap();
}

#[tokio::test]
async fn test_user_filter_parity_memory() {
    let store = create_memory_store();
    run_user_filter_parity_test(&store, "parity-user-filter-memory").await;
}

#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_user_filter_parity_postgres() {
    let store = create_postgres_store().await;
    run_user_filter_parity_test(&store, "parity-user-filter-postgres").await;
}

#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_user_filter_parity_mysql() {
    let store = create_mysql_store().await;
    run_user_filter_parity_test(&store, "parity-user-filter-mysql").await;
}

#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_user_filter_parity_cockroachdb() {
    let store = create_cockroachdb_store().await;
    run_user_filter_parity_test(&store, "parity-user-filter-cockroachdb").await;
}

// ============================================================================
// Section 10: Health Check Parity Tests
// ============================================================================

/// Generic helper: Test health check functionality
async fn run_health_check_test<S: DataStore>(store: &S) {
    let status = store
        .health_check()
        .await
        .expect("Health check should succeed");

    // Health check should return a valid status
    assert!(status.healthy, "Store should be healthy");
    assert!(
        status.latency.as_millis() < 5000,
        "Health check should complete quickly"
    );
}

#[tokio::test]
async fn test_health_check_memory() {
    let store = create_memory_store();
    run_health_check_test(&store).await;
}

#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_health_check_postgres() {
    let store = create_postgres_store().await;
    run_health_check_test(&store).await;

    // PostgreSQL should report pool stats
    let status = store.health_check().await.unwrap();
    assert!(
        status.pool_stats.is_some(),
        "PostgreSQL should report pool stats"
    );
    assert_eq!(status.message, Some("postgresql".to_string()));
}

#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_health_check_mysql() {
    let store = create_mysql_store().await;
    run_health_check_test(&store).await;

    // MySQL should report pool stats
    let status = store.health_check().await.unwrap();
    assert!(
        status.pool_stats.is_some(),
        "MySQL should report pool stats"
    );
    assert_eq!(status.message, Some("mysql".to_string()));
}

#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_health_check_cockroachdb() {
    let store = create_cockroachdb_store().await;
    run_health_check_test(&store).await;

    // CockroachDB uses PostgreSQL driver, should report as postgresql
    let status = store.health_check().await.unwrap();
    assert!(
        status.pool_stats.is_some(),
        "CockroachDB should report pool stats"
    );
    assert_eq!(status.message, Some("postgresql".to_string()));
}

// ============================================================================
// Section 11: Duplicate Tuple Idempotency Tests
// ============================================================================

/// Generic helper: Test duplicate tuple write is idempotent
async fn run_duplicate_tuple_idempotency_test<S: DataStore>(store: &S, store_id: &str) {
    store
        .create_store(store_id, "Duplicate Idempotency Test")
        .await
        .unwrap();

    let tuple = StoredTuple::new("document", "dup-doc", "viewer", "user", "dup-user", None);

    // Write same tuple multiple times
    store.write_tuple(store_id, tuple.clone()).await.unwrap();
    store.write_tuple(store_id, tuple.clone()).await.unwrap();
    store.write_tuple(store_id, tuple.clone()).await.unwrap();

    // Should only have one tuple
    let tuples = store
        .read_tuples(store_id, &TupleFilter::default())
        .await
        .unwrap();

    assert_eq!(
        tuples.len(),
        1,
        "Duplicate writes should be idempotent - only one tuple"
    );

    // Cleanup
    store.delete_store(store_id).await.unwrap();
}

#[tokio::test]
async fn test_duplicate_tuple_idempotency_memory() {
    let store = create_memory_store();
    run_duplicate_tuple_idempotency_test(&store, "parity-dup-memory").await;
}

#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_duplicate_tuple_idempotency_postgres() {
    let store = create_postgres_store().await;
    run_duplicate_tuple_idempotency_test(&store, "parity-dup-postgres").await;
}

#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_duplicate_tuple_idempotency_mysql() {
    let store = create_mysql_store().await;
    run_duplicate_tuple_idempotency_test(&store, "parity-dup-mysql").await;
}

#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_duplicate_tuple_idempotency_cockroachdb() {
    let store = create_cockroachdb_store().await;
    run_duplicate_tuple_idempotency_test(&store, "parity-dup-cockroachdb").await;
}

// ============================================================================
// Section 12: Store Cascade Delete Tests
// ============================================================================

/// Generic helper: Test that deleting a store deletes all associated data
async fn run_store_cascade_delete_test<S: DataStore>(store: &S, store_id: &str) {
    store
        .create_store(store_id, "Cascade Delete Test")
        .await
        .unwrap();

    // Write tuples
    let tuples: Vec<StoredTuple> = (0..10)
        .map(|i| {
            StoredTuple::new(
                "document",
                format!("doc{i}"),
                "viewer",
                "user",
                "alice",
                None,
            )
        })
        .collect();

    store.write_tuples(store_id, tuples, vec![]).await.unwrap();

    // Write authorization model with valid schema
    let model_json = serde_json::json!({
        "schema_version": "1.1",
        "type_definitions": [{"type": "user"}, {"type": "document"}]
    });
    let model = StoredAuthorizationModel::new(
        ulid::Ulid::new().to_string(),
        store_id,
        "1.1",
        serde_json::to_string(&model_json).unwrap(),
    );

    store.write_authorization_model(model).await.unwrap();

    // Verify data exists
    let tuples_before = store
        .read_tuples(store_id, &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples_before.len(), 10);

    let models_before = store.list_authorization_models(store_id).await.unwrap();
    assert_eq!(models_before.len(), 1);

    // Delete store
    store.delete_store(store_id).await.unwrap();

    // Verify store is gone
    let result = store.get_store(store_id).await;
    assert!(matches!(result, Err(StorageError::StoreNotFound { .. })));

    // Reading from deleted store should fail
    let result = store.read_tuples(store_id, &TupleFilter::default()).await;
    assert!(matches!(result, Err(StorageError::StoreNotFound { .. })));
}

#[tokio::test]
async fn test_store_cascade_delete_memory() {
    let store = create_memory_store();
    run_store_cascade_delete_test(&store, "parity-cascade-memory").await;
}

#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_store_cascade_delete_postgres() {
    let store = create_postgres_store().await;
    run_store_cascade_delete_test(&store, "parity-cascade-postgres").await;
}

#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_store_cascade_delete_mysql() {
    let store = create_mysql_store().await;
    run_store_cascade_delete_test(&store, "parity-cascade-mysql").await;
}

#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_store_cascade_delete_cockroachdb() {
    let store = create_cockroachdb_store().await;
    run_store_cascade_delete_test(&store, "parity-cascade-cockroachdb").await;
}
