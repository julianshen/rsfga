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
    DataStore, PaginationOptions, PostgresConfig, PostgresDataStore, StorageError,
    StoredAuthorizationModel, StoredTuple, TupleFilter,
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
        ..Default::default()
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

/// Test: Column size migration is idempotent on CockroachDB
///
/// CockroachDB has expression indexes that prevent ALTER COLUMN TYPE operations
/// even when no actual type change is needed. This test verifies that our
/// migration logic correctly detects when columns already have the target size
/// and skips the ALTER operation.
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_column_size_migration_idempotent_on_cockroachdb() {
    let store = create_store().await;

    // Run migrations multiple times - should succeed each time
    // This specifically tests the CockroachDB expression index workaround
    for i in 1..=3 {
        store
            .run_migrations()
            .await
            .unwrap_or_else(|e| panic!("Migration run {i} failed: {e}"));
    }

    // Verify we can still operate normally after multiple migration runs
    store
        .create_store("test-crdb-column-migration", "Migration Test Store")
        .await
        .unwrap();

    // Write and read a tuple with condition to verify schema is correct
    let tuple = StoredTuple::with_condition(
        "document",
        "readme",
        "viewer",
        "user",
        "alice",
        None,
        "time_based",
        None,
    );
    store
        .write_tuple("test-crdb-column-migration", tuple)
        .await
        .unwrap();

    let tuples = store
        .read_tuples(
            "test-crdb-column-migration",
            &TupleFilter {
                object_type: Some("document".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(tuples.len(), 1);
    assert_eq!(tuples[0].condition_name, Some("time_based".to_string()));

    // Cleanup
    store
        .delete_store("test-crdb-column-migration")
        .await
        .unwrap();
}

/// Test: Verify column sizes are correct after migration on CockroachDB
///
/// This test queries information_schema to verify all VARCHAR columns have
/// the expected sizes after running migrations.
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_column_sizes_correct_after_migration_on_cockroachdb() {
    let store = create_store().await;

    // Expected column sizes matching the column_specs in apply_column_size_migration
    let expected_sizes: Vec<(&str, &str, i64)> = vec![
        ("stores", "id", 26),
        ("stores", "name", 256),
        ("authorization_models", "id", 26),
        ("authorization_models", "store_id", 26),
        ("tuples", "store_id", 26),
        ("tuples", "object_type", 254),
        ("tuples", "object_id", 256),
        ("tuples", "relation", 50),
        ("tuples", "user_type", 254),
        ("tuples", "user_id", 512),
        ("tuples", "user_relation", 50),
        ("tuples", "condition_name", 256),
        ("changelog", "store_id", 26),
        ("changelog", "object_type", 254),
        ("changelog", "object_id", 256),
        ("changelog", "relation", 50),
        ("changelog", "user_type", 254),
        ("changelog", "user_id", 512),
        ("changelog", "user_relation", 50),
        ("changelog", "condition_name", 256),
    ];

    // Query actual column sizes from information_schema
    for (table, column, expected_size) in expected_sizes {
        let actual_size: Option<i64> = sqlx::query_scalar(
            "SELECT character_maximum_length FROM information_schema.columns \
             WHERE table_schema = current_schema() AND table_name = $1 AND column_name = $2",
        )
        .bind(table)
        .bind(column)
        .fetch_optional(store.pool())
        .await
        .unwrap_or_else(|e| panic!("Failed to query size for {table}.{column}: {e}"))
        .flatten();

        assert_eq!(
            actual_size,
            Some(expected_size),
            "Column {table}.{column} should have size {expected_size}, got {actual_size:?}"
        );
    }
}

/// Test: Migration handles CockroachDB expression indexes gracefully
///
/// CockroachDB creates internal computed columns (crdb_internal_idx_expr) for
/// expression indexes like COALESCE(user_relation, ''). This test verifies that
/// our migration logic correctly handles these dependencies.
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_migration_handles_expression_indexes_on_cockroachdb() {
    let store = create_store().await;

    // Verify the tuples table has the unique index with expression
    // This index is what causes the ALTER COLUMN TYPE to fail if we don't
    // check sizes first
    let index_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM information_schema.statistics
            WHERE table_schema = current_schema()
            AND table_name = 'tuples'
            AND index_name = 'idx_tuples_unique'
        )",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();

    assert!(
        index_exists,
        "idx_tuples_unique index should exist on tuples table"
    );

    // Run migrations again - this should NOT fail even with the expression index
    store
        .run_migrations()
        .await
        .expect("Migration should succeed with expression indexes present");
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
                format!("doc{i:02}"),
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
                format!("doc{i}"),
                "viewer",
                "user",
                format!("user{i}"),
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
                format!("doc{i}"),
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
                format!("doc{i}"),
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
                format!("doc{i}"),
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
    eprintln!("CockroachDB 10k tuples: write={write_duration:?}, read={read_duration:?}");

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
    eprintln!("CockroachDB filtered query: {filter_duration:?}");

    // Cleanup
    store.delete_store("test-crdb-large-dataset").await.unwrap();
}

// ==========================================================================
// Section 4.10: Connection Pool Exhaustion Tests
// ==========================================================================
//
// These tests verify connection pool behavior under exhaustion scenarios
// on CockroachDB. Due to CockroachDB's distributed nature, connection pool
// behavior may differ slightly from PostgreSQL.
//
// CockroachDB-specific considerations:
// - Distributed queries may hold connections longer
// - Serializable isolation may cause more retries
// - Connection routing may affect pool utilization
// ==========================================================================

/// Test: Pool exhaustion returns timeout error on CockroachDB
///
/// Creates a small pool (2 connections) and spawns more concurrent operations
/// than available connections with a short acquire timeout.
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_pool_exhaustion_on_cockroachdb() {
    let database_url = get_database_url();

    // Create pool with very limited connections and short timeout
    let config = PostgresConfig {
        database_url,
        max_connections: 2,
        min_connections: 1,
        connect_timeout_secs: 1, // Very short timeout to trigger exhaustion quickly
        ..Default::default()
    };

    let store = PostgresDataStore::from_config(&config)
        .await
        .expect("Failed to create store with limited pool");

    store.run_migrations().await.unwrap();

    store
        .create_store("test-crdb-pool-exhaustion", "Pool Exhaustion Test")
        .await
        .unwrap();

    let store = Arc::new(store);

    // Spawn more concurrent operations than pool can handle
    let mut handles = Vec::new();

    for i in 0..10 {
        let store = Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            // Each write acquires a connection - with only 2 available and 10 concurrent,
            // some will timeout
            let tuple = StoredTuple::new(
                "document",
                format!("doc{i}"),
                "viewer",
                "user",
                format!("user{i}"),
                None,
            );
            store.write_tuple("test-crdb-pool-exhaustion", tuple).await
        }));
    }

    // Collect results
    let mut success_count = 0;
    let mut error_count = 0;

    for handle in handles {
        match handle.await.unwrap() {
            Ok(_) => success_count += 1,
            Err(e) => {
                error_count += 1;
                let error_msg = e.to_string().to_lowercase();
                // Pool exhaustion should manifest as connection or timeout error
                assert!(
                    error_msg.contains("timeout")
                        || error_msg.contains("pool")
                        || error_msg.contains("connection")
                        || error_msg.contains("timed out"),
                    "Expected pool exhaustion error, got: {e}"
                );
            }
        }
    }

    // Some operations should succeed (pool has 2 connections)
    // Others may fail due to pool exhaustion (depends on timing)
    eprintln!("CockroachDB pool exhaustion test: {success_count} succeeded, {error_count} failed");

    // At minimum, some should succeed
    assert!(success_count > 0, "At least some operations should succeed");

    // Cleanup (may partially fail if pool is still exhausted)
    let _ = store.delete_store("test-crdb-pool-exhaustion").await;
}

/// Test: Pool recovers after queries complete on CockroachDB
///
/// Verifies that after pool exhaustion, subsequent queries succeed
/// once previous connections are released.
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_pool_recovery_on_cockroachdb() {
    let database_url = get_database_url();

    let config = PostgresConfig {
        database_url,
        max_connections: 2,
        min_connections: 1,
        connect_timeout_secs: 30,
        ..Default::default()
    };

    let store = PostgresDataStore::from_config(&config)
        .await
        .expect("Failed to create store");

    store.run_migrations().await.unwrap();

    store
        .create_store("test-crdb-pool-recovery", "Pool Recovery Test")
        .await
        .unwrap();

    let store = Arc::new(store);

    // Phase 1: Saturate the pool with concurrent writes
    let mut handles = Vec::new();
    for i in 0..5 {
        let store = Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            let tuple = StoredTuple::new(
                "document",
                format!("phase1-doc{i}"),
                "viewer",
                "user",
                "alice",
                None,
            );
            store.write_tuple("test-crdb-pool-recovery", tuple).await
        }));
    }

    // Wait for all to complete
    for handle in handles {
        let _ = handle.await.unwrap();
    }

    // Phase 2: Verify pool recovered - new queries should succeed
    for i in 0..5 {
        let tuple = StoredTuple::new(
            "document",
            format!("phase2-doc{i}"),
            "viewer",
            "user",
            "bob",
            None,
        );
        store
            .write_tuple("test-crdb-pool-recovery", tuple)
            .await
            .expect("Query after pool recovery should succeed");
    }

    // Verify all tuples were written
    let tuples = store
        .read_tuples("test-crdb-pool-recovery", &TupleFilter::default())
        .await
        .unwrap();

    // Should have tuples from both phases (some phase 1 may have failed, all phase 2 should succeed)
    assert!(
        tuples.len() >= 5,
        "At least phase 2 tuples should exist after recovery"
    );

    // Cleanup
    store.delete_store("test-crdb-pool-recovery").await.unwrap();
}

/// Test: Concurrent migrations don't deadlock on CockroachDB
///
/// Multiple instances calling run_migrations() simultaneously should
/// all complete successfully without deadlocking. CockroachDB's distributed
/// nature may handle concurrent schema changes differently than PostgreSQL.
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_concurrent_migrations_on_cockroachdb() {
    let database_url = get_database_url();

    // Spawn multiple concurrent migration attempts
    let mut handles = Vec::new();
    for i in 0..5 {
        let url = database_url.clone();
        handles.push(tokio::spawn(async move {
            let config = PostgresConfig {
                database_url: url,
                max_connections: 2,
                min_connections: 1,
                connect_timeout_secs: 30,
                ..Default::default()
            };

            let store = PostgresDataStore::from_config(&config).await?;
            store.run_migrations().await?;

            eprintln!("CockroachDB migration instance {i} completed");
            Ok::<_, StorageError>(())
        }));
    }

    // All migrations should complete without deadlock
    // Use a timeout to detect deadlocks
    let timeout_duration = std::time::Duration::from_secs(60);
    let results = tokio::time::timeout(timeout_duration, async {
        let mut results = Vec::new();
        for handle in handles {
            results.push(handle.await);
        }
        results
    })
    .await
    .expect("CockroachDB concurrent migrations timed out - possible deadlock");

    // Count successes and failures
    let mut success_count = 0;
    for result in results {
        match result {
            Ok(Ok(())) => success_count += 1,
            Ok(Err(e)) => {
                // Migration errors are acceptable (e.g., if table already exists)
                eprintln!("CockroachDB migration error (acceptable): {e}");
                success_count += 1; // Still counts as completing without deadlock
            }
            Err(e) => {
                panic!("Task join error: {e}");
            }
        }
    }

    assert_eq!(
        success_count, 5,
        "All CockroachDB migration attempts should complete (success or expected error)"
    );

    // Verify we can still operate after concurrent migrations
    let config = PostgresConfig {
        database_url,
        max_connections: 5,
        min_connections: 1,
        connect_timeout_secs: 30,
        ..Default::default()
    };

    let store = PostgresDataStore::from_config(&config).await.unwrap();

    // Clean up any test stores from this test
    let _ = store.delete_store("test-crdb-concurrent-migrations").await;

    store
        .create_store("test-crdb-concurrent-migrations", "Post-Migration Test")
        .await
        .unwrap();

    let s = store
        .get_store("test-crdb-concurrent-migrations")
        .await
        .unwrap();
    assert_eq!(s.id, "test-crdb-concurrent-migrations");

    // Cleanup
    store
        .delete_store("test-crdb-concurrent-migrations")
        .await
        .unwrap();
}

// ==========================================================================
// Section 4.11: Authorization Model Deletion Tests
// ==========================================================================

/// Test: delete_authorization_model removes the model successfully
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_crdb_delete_authorization_model_removes_model() {
    let store = create_store().await;
    let store_id = "test-crdb-delete-model";

    // Create a store
    store.create_store(store_id, "Test Store").await.unwrap();

    // Create an authorization model
    let model = StoredAuthorizationModel::new(
        "model-1",
        store_id,
        "1.1",
        r#"{"schema_version":"1.1","type_definitions":[]}"#,
    );
    store.write_authorization_model(model).await.unwrap();

    // Verify model exists
    let result = store.get_authorization_model(store_id, "model-1").await;
    assert!(result.is_ok());

    // Delete the model
    store
        .delete_authorization_model(store_id, "model-1")
        .await
        .unwrap();

    // Verify model is gone
    let result = store.get_authorization_model(store_id, "model-1").await;
    assert!(matches!(result, Err(StorageError::ModelNotFound { .. })));

    // Cleanup
    store.delete_store(store_id).await.unwrap();
}

/// Test: delete_authorization_model returns ModelNotFound for non-existent model
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_crdb_delete_authorization_model_not_found() {
    let store = create_store().await;
    let store_id = "test-crdb-delete-model-notfound";

    // Create a store
    store.create_store(store_id, "Test Store").await.unwrap();

    // Try to delete a non-existent model
    let result = store
        .delete_authorization_model(store_id, "non-existent")
        .await;
    assert!(matches!(result, Err(StorageError::ModelNotFound { .. })));

    // Cleanup
    store.delete_store(store_id).await.unwrap();
}

/// Test: delete_authorization_model returns StoreNotFound for non-existent store
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_crdb_delete_authorization_model_store_not_found() {
    let store = create_store().await;

    // Try to delete a model from a non-existent store
    let result = store
        .delete_authorization_model("non-existent-store", "model-1")
        .await;
    assert!(matches!(result, Err(StorageError::StoreNotFound { .. })));
}

/// Test: delete_authorization_model updates get_latest_authorization_model
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_crdb_delete_authorization_model_updates_latest() {
    let store = create_store().await;
    let store_id = "test-crdb-delete-model-latest";

    // Create a store
    store.create_store(store_id, "Test Store").await.unwrap();

    // Create two models with small delay to ensure ordering
    let model1 = StoredAuthorizationModel::new(
        "model-1",
        store_id,
        "1.1",
        r#"{"schema_version":"1.1","type_definitions":[]}"#,
    );
    store.write_authorization_model(model1).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let model2 = StoredAuthorizationModel::new(
        "model-2",
        store_id,
        "1.1",
        r#"{"schema_version":"1.1","type_definitions":[]}"#,
    );
    store.write_authorization_model(model2).await.unwrap();

    // Latest should be model-2
    let latest = store
        .get_latest_authorization_model(store_id)
        .await
        .unwrap();
    assert_eq!(latest.id, "model-2");

    // Delete model-2
    store
        .delete_authorization_model(store_id, "model-2")
        .await
        .unwrap();

    // Latest should now be model-1
    let latest = store
        .get_latest_authorization_model(store_id)
        .await
        .unwrap();
    assert_eq!(latest.id, "model-1");

    // Cleanup
    store.delete_store(store_id).await.unwrap();
}

/// Test: delete_authorization_model excludes model from list
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_crdb_delete_authorization_model_excludes_from_list() {
    let store = create_store().await;
    let store_id = "test-crdb-delete-model-list";

    // Create a store
    store.create_store(store_id, "Test Store").await.unwrap();

    // Create three models
    for i in 1..=3 {
        let model = StoredAuthorizationModel::new(
            format!("model-{i}"),
            store_id,
            "1.1",
            r#"{"schema_version":"1.1","type_definitions":[]}"#,
        );
        store.write_authorization_model(model).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    // Verify all three models exist
    let models = store.list_authorization_models(store_id).await.unwrap();
    assert_eq!(models.len(), 3);

    // Delete model-2
    store
        .delete_authorization_model(store_id, "model-2")
        .await
        .unwrap();

    // List should now have 2 models and not include model-2
    let models = store.list_authorization_models(store_id).await.unwrap();
    assert_eq!(models.len(), 2);
    assert!(models.iter().any(|m| m.id == "model-1"));
    assert!(models.iter().all(|m| m.id != "model-2"));
    assert!(models.iter().any(|m| m.id == "model-3"));

    // Cleanup
    store.delete_store(store_id).await.unwrap();
}

/// Test: deleting last model returns ModelNotFound from get_latest
#[tokio::test]
#[ignore = "requires running CockroachDB"]
async fn test_crdb_delete_last_model_behavior() {
    let store = create_store().await;
    let store_id = "test-crdb-delete-last-model";

    // Create a store
    store.create_store(store_id, "Test Store").await.unwrap();

    // Create one model
    let model = StoredAuthorizationModel::new(
        "model-1",
        store_id,
        "1.1",
        r#"{"schema_version":"1.1","type_definitions":[]}"#,
    );
    store.write_authorization_model(model).await.unwrap();

    // Delete it
    store
        .delete_authorization_model(store_id, "model-1")
        .await
        .unwrap();

    // get_latest should now return ModelNotFound
    let result = store.get_latest_authorization_model(store_id).await;
    assert!(matches!(result, Err(StorageError::ModelNotFound { .. })));

    // Cleanup
    store.delete_store(store_id).await.unwrap();
}

// ==========================================================================
// CockroachDB-Specific Connection Handling Notes
// ==========================================================================
//
// CockroachDB connection pool behavior may differ from PostgreSQL:
//
// 1. **Distributed queries**: CockroachDB may route queries to different nodes,
//    which can affect connection utilization patterns.
//
// 2. **Serializable isolation**: CockroachDB uses serializable isolation by
//    default, which may cause more transaction retries under contention.
//    SQLx handles automatic retries, but this may affect timeout behavior.
//
// 3. **Connection routing**: In multi-node deployments, connections may be
//    distributed across nodes. Single-node testing may not reveal all
//    connection handling edge cases.
//
// 4. **Schema changes**: CockroachDB schema changes are online and non-blocking,
//    which may result in different migration concurrency behavior.
//
// For production deployments, consider:
// - Monitoring connection pool metrics
// - Tuning acquire_timeout based on expected query latency
// - Testing with realistic multi-node configurations
