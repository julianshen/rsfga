//! CockroachDB integration tests.
//!
//! These tests verify that PostgresDataStore works correctly with CockroachDB,
//! which uses the PostgreSQL wire protocol.
//!
//! CockroachDB is a distributed SQL database that provides:
//! - PostgreSQL compatibility
//! - Automatic sharding and replication
//! - Serializable isolation by default
//!
//! To run these tests:
//! 1. Start CockroachDB: docker run --name rsfga-crdb -p 26257:26257 -d cockroachdb/cockroach:latest start-single-node --insecure
//! 2. Create database: docker exec -it rsfga-crdb cockroach sql --insecure -e "CREATE DATABASE IF NOT EXISTS rsfga"
//! 3. Set COCKROACHDB_URL: export COCKROACHDB_URL=postgresql://root@localhost:26257/rsfga
//! 4. Run tests: cargo test -p rsfga-storage --test cockroachdb_integration -- --ignored --test-threads=1
//!
//! Note: Tests use `--test-threads=1` to avoid store ID collisions. Each test
//! creates and deletes its own store, but concurrent runs could interfere.

use rsfga_storage::{
    DataStore, PaginationOptions, PostgresConfig, PostgresDataStore, StorageError, StoredTuple,
    TupleFilter,
};
use std::sync::Arc;

/// Get database URL from environment, defaulting to localhost if not set.
fn get_database_url() -> String {
    std::env::var("COCKROACHDB_URL")
        .unwrap_or_else(|_| "postgresql://root@localhost:26257/rsfga".to_string())
}

/// Create a PostgresDataStore connected to CockroachDB with migrations run.
async fn create_store() -> PostgresDataStore {
    let database_url = get_database_url();

    let config = PostgresConfig {
        database_url,
        max_connections: 5,
        min_connections: 1,
        connect_timeout_secs: 30,
    };

    let store = PostgresDataStore::from_config(&config)
        .await
        .expect("Failed to create PostgresDataStore for CockroachDB - is CockroachDB running?");

    store
        .run_migrations()
        .await
        .expect("Failed to run migrations on CockroachDB");

    store
}

// ==========================================================================
// Section 4.1: Connection and Migration Tests
// ==========================================================================

/// Test: Can connect to CockroachDB using PostgreSQL driver
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_can_connect_to_cockroachdb() {
    let store = create_store().await;

    // Verify we can list stores - the call should succeed without error
    let result = store.list_stores().await;
    assert!(
        result.is_ok(),
        "Should be able to list stores after connecting to CockroachDB"
    );
}

/// Test: Migrations run successfully on CockroachDB
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_migrations_run_on_cockroachdb() {
    let store = create_store().await;

    // Verify tables exist by creating a store
    store
        .create_store("test-crdb-migrations", "CockroachDB Test Store")
        .await
        .unwrap();

    let s = store.get_store("test-crdb-migrations").await.unwrap();
    assert_eq!(s.id, "test-crdb-migrations");

    // Cleanup
    store.delete_store("test-crdb-migrations").await.unwrap();
}

/// Test: Migrations are idempotent on CockroachDB
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_migrations_idempotent_on_cockroachdb() {
    let store = create_store().await;

    // Run migrations again - should succeed without error
    store.run_migrations().await.unwrap();

    // Verify we can still operate normally
    store
        .create_store("test-crdb-idempotent", "Test Store")
        .await
        .unwrap();

    // Cleanup
    store.delete_store("test-crdb-idempotent").await.unwrap();
}

// ==========================================================================
// Section 4.2: Store CRUD Tests
// ==========================================================================

/// Test: Create store on CockroachDB
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_create_store_on_cockroachdb() {
    let store = create_store().await;

    store
        .create_store("test-crdb-create", "CockroachDB Store")
        .await
        .unwrap();

    let s = store.get_store("test-crdb-create").await.unwrap();
    assert_eq!(s.id, "test-crdb-create");
    assert_eq!(s.name, "CockroachDB Store");

    // Cleanup
    store.delete_store("test-crdb-create").await.unwrap();
}

/// Test: Delete store cascades to tuples on CockroachDB
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_delete_store_cascades_on_cockroachdb() {
    let store = create_store().await;
    store
        .create_store("test-crdb-cascade", "Test Store")
        .await
        .unwrap();

    // Write a tuple
    let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);
    store.write_tuple("test-crdb-cascade", tuple).await.unwrap();

    // Delete the store
    store.delete_store("test-crdb-cascade").await.unwrap();

    // Store should be gone
    let result = store.get_store("test-crdb-cascade").await;
    assert!(matches!(result, Err(StorageError::StoreNotFound { .. })));
}

/// Test: Duplicate store creation fails on CockroachDB
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_duplicate_store_fails_on_cockroachdb() {
    let store = create_store().await;

    store
        .create_store("test-crdb-dup", "First Store")
        .await
        .unwrap();

    let result = store.create_store("test-crdb-dup", "Second Store").await;
    assert!(matches!(
        result,
        Err(StorageError::StoreAlreadyExists { .. })
    ));

    // Cleanup
    store.delete_store("test-crdb-dup").await.unwrap();
}

// ==========================================================================
// Section 4.3: Tuple CRUD Tests
// ==========================================================================

/// Test: Write and read tuple on CockroachDB
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_write_read_tuple_on_cockroachdb() {
    let store = create_store().await;
    store
        .create_store("test-crdb-tuple", "Test Store")
        .await
        .unwrap();

    let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);
    store
        .write_tuple("test-crdb-tuple", tuple.clone())
        .await
        .unwrap();

    let tuples = store
        .read_tuples("test-crdb-tuple", &TupleFilter::default())
        .await
        .unwrap();

    assert_eq!(tuples.len(), 1);
    assert_eq!(tuples[0].object_type, "document");
    assert_eq!(tuples[0].object_id, "doc1");
    assert_eq!(tuples[0].relation, "viewer");
    assert_eq!(tuples[0].user_type, "user");
    assert_eq!(tuples[0].user_id, "alice");

    // Cleanup
    store.delete_store("test-crdb-tuple").await.unwrap();
}

/// Test: Delete tuple on CockroachDB
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_delete_tuple_on_cockroachdb() {
    let store = create_store().await;
    store
        .create_store("test-crdb-delete-tuple", "Test Store")
        .await
        .unwrap();

    let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);
    store
        .write_tuple("test-crdb-delete-tuple", tuple.clone())
        .await
        .unwrap();

    // Verify tuple exists
    let tuples = store
        .read_tuples("test-crdb-delete-tuple", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1);

    // Delete tuple
    store
        .delete_tuple("test-crdb-delete-tuple", tuple)
        .await
        .unwrap();

    // Verify tuple is gone
    let tuples = store
        .read_tuples("test-crdb-delete-tuple", &TupleFilter::default())
        .await
        .unwrap();
    assert!(tuples.is_empty());

    // Cleanup
    store.delete_store("test-crdb-delete-tuple").await.unwrap();
}

/// Test: Duplicate tuple write is idempotent on CockroachDB
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_duplicate_tuple_idempotent_on_cockroachdb() {
    let store = create_store().await;
    store
        .create_store("test-crdb-idem-tuple", "Test Store")
        .await
        .unwrap();

    let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);

    // Write same tuple twice
    store
        .write_tuple("test-crdb-idem-tuple", tuple.clone())
        .await
        .unwrap();
    store
        .write_tuple("test-crdb-idem-tuple", tuple.clone())
        .await
        .unwrap();

    // Should only have one tuple
    let tuples = store
        .read_tuples("test-crdb-idem-tuple", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1);

    // Cleanup
    store.delete_store("test-crdb-idem-tuple").await.unwrap();
}

/// Test: Tuple with user_relation (userset) on CockroachDB
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_tuple_with_user_relation_on_cockroachdb() {
    let store = create_store().await;
    store
        .create_store("test-crdb-userset", "Test Store")
        .await
        .unwrap();

    // Write tuple with user_relation (userset like group:engineering#member)
    let tuple = StoredTuple::new(
        "document",
        "doc1",
        "viewer",
        "group",
        "engineering",
        Some("member".to_string()),
    );

    store
        .write_tuple("test-crdb-userset", tuple.clone())
        .await
        .unwrap();

    // Read it back
    let tuples = store
        .read_tuples("test-crdb-userset", &TupleFilter::default())
        .await
        .unwrap();

    assert_eq!(tuples.len(), 1);
    assert_eq!(tuples[0].user_type, "group");
    assert_eq!(tuples[0].user_id, "engineering");
    assert_eq!(tuples[0].user_relation, Some("member".to_string()));

    // Filter by userset
    let filter = TupleFilter {
        user: Some("group:engineering#member".to_string()),
        ..Default::default()
    };
    let tuples = store
        .read_tuples("test-crdb-userset", &filter)
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1);

    // Cleanup
    store.delete_store("test-crdb-userset").await.unwrap();
}

// ==========================================================================
// Section 4.4: Batch Operations Tests
// ==========================================================================

/// Test: Batch write tuples on CockroachDB
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_batch_write_on_cockroachdb() {
    let store = create_store().await;
    store
        .create_store("test-crdb-batch", "Test Store")
        .await
        .unwrap();

    let tuples = vec![
        StoredTuple::new("document", "doc1", "viewer", "user", "alice", None),
        StoredTuple::new("document", "doc2", "viewer", "user", "bob", None),
        StoredTuple::new("document", "doc3", "editor", "user", "charlie", None),
    ];

    store
        .write_tuples("test-crdb-batch", tuples, vec![])
        .await
        .unwrap();

    let result = store
        .read_tuples("test-crdb-batch", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(result.len(), 3);

    // Cleanup
    store.delete_store("test-crdb-batch").await.unwrap();
}

/// Test: Atomic write and delete on CockroachDB
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_atomic_write_delete_on_cockroachdb() {
    let store = create_store().await;
    store
        .create_store("test-crdb-atomic", "Test Store")
        .await
        .unwrap();

    // Initial tuples
    let tuple1 = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);
    let tuple2 = StoredTuple::new("document", "doc2", "viewer", "user", "bob", None);

    store
        .write_tuples(
            "test-crdb-atomic",
            vec![tuple1.clone(), tuple2.clone()],
            vec![],
        )
        .await
        .unwrap();

    // Atomic: delete tuple1, add tuple3
    let tuple3 = StoredTuple::new("document", "doc3", "viewer", "user", "charlie", None);
    store
        .write_tuples("test-crdb-atomic", vec![tuple3], vec![tuple1])
        .await
        .unwrap();

    let result = store
        .read_tuples("test-crdb-atomic", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(result.len(), 2);

    let ids: Vec<_> = result.iter().map(|t| t.object_id.as_str()).collect();
    assert!(ids.contains(&"doc2"));
    assert!(ids.contains(&"doc3"));
    assert!(!ids.contains(&"doc1"));

    // Cleanup
    store.delete_store("test-crdb-atomic").await.unwrap();
}

// ==========================================================================
// Section 4.5: Filter Tests
// ==========================================================================

/// Test: Filter by object type on CockroachDB
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_filter_by_object_type_on_cockroachdb() {
    let store = create_store().await;
    store
        .create_store("test-crdb-filter", "Test Store")
        .await
        .unwrap();

    let tuples = vec![
        StoredTuple::new("document", "doc1", "viewer", "user", "alice", None),
        StoredTuple::new("folder", "folder1", "viewer", "user", "alice", None),
        StoredTuple::new("document", "doc2", "viewer", "user", "bob", None),
    ];

    store
        .write_tuples("test-crdb-filter", tuples, vec![])
        .await
        .unwrap();

    let filter = TupleFilter {
        object_type: Some("document".to_string()),
        ..Default::default()
    };

    let result = store
        .read_tuples("test-crdb-filter", &filter)
        .await
        .unwrap();
    assert_eq!(result.len(), 2);
    assert!(result.iter().all(|t| t.object_type == "document"));

    // Cleanup
    store.delete_store("test-crdb-filter").await.unwrap();
}

/// Test: Filter by user on CockroachDB
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_filter_by_user_on_cockroachdb() {
    let store = create_store().await;
    store
        .create_store("test-crdb-user-filter", "Test Store")
        .await
        .unwrap();

    let tuples = vec![
        StoredTuple::new("document", "doc1", "viewer", "user", "alice", None),
        StoredTuple::new("document", "doc2", "viewer", "user", "bob", None),
        StoredTuple::new("document", "doc3", "viewer", "user", "alice", None),
    ];

    store
        .write_tuples("test-crdb-user-filter", tuples, vec![])
        .await
        .unwrap();

    let filter = TupleFilter {
        user: Some("user:alice".to_string()),
        ..Default::default()
    };

    let result = store
        .read_tuples("test-crdb-user-filter", &filter)
        .await
        .unwrap();
    assert_eq!(result.len(), 2);
    assert!(result.iter().all(|t| t.user_id == "alice"));

    // Cleanup
    store.delete_store("test-crdb-user-filter").await.unwrap();
}

// ==========================================================================
// Section 4.6: Pagination Tests
// ==========================================================================

/// Test: Pagination works on CockroachDB
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_pagination_on_cockroachdb() {
    let store = create_store().await;
    store
        .create_store("test-crdb-pagination", "Test Store")
        .await
        .unwrap();

    // Write 25 tuples
    let tuples: Vec<StoredTuple> = (0..25)
        .map(|i| {
            StoredTuple::new(
                "document",
                format!("doc{:02}", i),
                "viewer",
                "user",
                "alice",
                None,
            )
        })
        .collect();

    store
        .write_tuples("test-crdb-pagination", tuples, vec![])
        .await
        .unwrap();

    // First page
    let pagination = PaginationOptions {
        page_size: Some(10),
        continuation_token: None,
    };
    let result = store
        .read_tuples_paginated("test-crdb-pagination", &TupleFilter::default(), &pagination)
        .await
        .unwrap();

    assert_eq!(result.items.len(), 10);
    assert!(result.continuation_token.is_some());

    // Second page
    let pagination = PaginationOptions {
        page_size: Some(10),
        continuation_token: result.continuation_token,
    };
    let result = store
        .read_tuples_paginated("test-crdb-pagination", &TupleFilter::default(), &pagination)
        .await
        .unwrap();

    assert_eq!(result.items.len(), 10);
    assert!(result.continuation_token.is_some());

    // Third page (partial)
    let pagination = PaginationOptions {
        page_size: Some(10),
        continuation_token: result.continuation_token,
    };
    let result = store
        .read_tuples_paginated("test-crdb-pagination", &TupleFilter::default(), &pagination)
        .await
        .unwrap();

    assert_eq!(result.items.len(), 5);
    assert!(result.continuation_token.is_none()); // No more pages

    // Cleanup
    store.delete_store("test-crdb-pagination").await.unwrap();
}

// ==========================================================================
// Section 4.7: Concurrent Access Tests
// ==========================================================================

/// Test: Concurrent writes on CockroachDB
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_concurrent_writes_on_cockroachdb() {
    let store = Arc::new(create_store().await);
    store
        .create_store("test-crdb-concurrent", "Test Store")
        .await
        .unwrap();

    // Run concurrent writes
    let mut handles = Vec::new();
    for i in 0..20 {
        let store = Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            let tuple = StoredTuple::new(
                "document",
                format!("doc{}", i),
                "viewer",
                "user",
                format!("user{}", i),
                None,
            );
            store
                .write_tuple("test-crdb-concurrent", tuple)
                .await
                .unwrap();
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    // All writes should be present
    let tuples = store
        .read_tuples("test-crdb-concurrent", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 20);

    // Cleanup
    store.delete_store("test-crdb-concurrent").await.unwrap();
}

// ==========================================================================
// Section 4.8: CockroachDB-Specific Behavior Tests
// ==========================================================================

/// Test: Basic write and read consistency on CockroachDB
///
/// This test verifies that a batch write is readable afterwards.
/// It's a basic sanity check, not a comprehensive isolation test.
/// True isolation testing would require concurrent transactions,
/// which the DataStore trait doesn't expose.
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_cockroachdb_basic_consistency() {
    let store = create_store().await;
    store
        .create_store("test-crdb-consistency", "Test Store")
        .await
        .unwrap();

    let tuples: Vec<StoredTuple> = (0..10)
        .map(|i| {
            StoredTuple::new(
                "document",
                format!("doc{}", i),
                "viewer",
                "user",
                "alice",
                None,
            )
        })
        .collect();

    store
        .write_tuples("test-crdb-consistency", tuples, vec![])
        .await
        .unwrap();

    // Read count should be consistent
    let result = store
        .read_tuples("test-crdb-consistency", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(result.len(), 10);

    // Cleanup
    store.delete_store("test-crdb-consistency").await.unwrap();
}

/// Test: Large batch operations on CockroachDB (1000+ tuples)
///
/// CockroachDB uses PostgreSQL wire protocol and benefits from batched writes.
/// This test validates that batch operations perform well at scale.
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_large_batch_on_cockroachdb() {
    let store = create_store().await;
    store
        .create_store("test-crdb-large-batch", "Large Batch Test")
        .await
        .unwrap();

    // Generate 1000 tuples to validate batch performance
    // CockroachDB handles this well via PostgreSQL protocol batching
    let tuples: Vec<StoredTuple> = (0..1000)
        .map(|i| {
            StoredTuple::new(
                "document",
                format!("doc{}", i),
                "viewer",
                "user",
                format!("user{}", i % 100),
                None,
            )
        })
        .collect();

    store
        .write_tuples("test-crdb-large-batch", tuples, vec![])
        .await
        .expect("Large batch write should succeed on CockroachDB");

    let result = store
        .read_tuples("test-crdb-large-batch", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(result.len(), 1000);

    // Cleanup
    store.delete_store("test-crdb-large-batch").await.unwrap();
}

// ==========================================================================
// Section 4.9: Large Dataset Performance Test (10k+ tuples)
// ==========================================================================

/// Test: Large dataset performance (10k+ tuples)
///
/// CockroachDB excels at distributed workloads. This test validates
/// that it handles large datasets similar to PostgreSQL.
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_large_dataset_performance_cockroachdb() {
    let store = create_store().await;
    store
        .create_store("test-crdb-large-dataset", "Large Dataset Test")
        .await
        .unwrap();

    // Generate 10,000 tuples for performance validation
    let tuples: Vec<StoredTuple> = (0..10_000)
        .map(|i| {
            StoredTuple::new(
                "document",
                format!("doc{}", i),
                "viewer",
                "user",
                format!("user{}", i % 500),
                None,
            )
        })
        .collect();

    let start = std::time::Instant::now();
    store
        .write_tuples("test-crdb-large-dataset", tuples, vec![])
        .await
        .expect("Large dataset write should succeed");
    let write_duration = start.elapsed();

    // Read all tuples
    let start = std::time::Instant::now();
    let result = store
        .read_tuples("test-crdb-large-dataset", &TupleFilter::default())
        .await
        .unwrap();
    let read_duration = start.elapsed();

    assert_eq!(result.len(), 10_000);

    // Log performance (not asserted to avoid flaky tests)
    eprintln!(
        "CockroachDB 10k tuples: write={:?}, read={:?}",
        write_duration, read_duration
    );

    // Test filtered query performance
    let filter = TupleFilter {
        user: Some("user:user42".to_string()),
        ..Default::default()
    };
    let start = std::time::Instant::now();
    let filtered = store
        .read_tuples("test-crdb-large-dataset", &filter)
        .await
        .unwrap();
    let filter_duration = start.elapsed();

    // user42 appears at indices 42, 542, 1042, ... (every 500)
    assert_eq!(filtered.len(), 20);
    eprintln!("CockroachDB filtered query: {:?}", filter_duration);

    // Cleanup
    store.delete_store("test-crdb-large-dataset").await.unwrap();
}
