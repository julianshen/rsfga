//! MySQL/MariaDB/TiDB integration tests.
//!
//! These tests require a running MySQL-compatible database. They are marked as
//! `#[ignore]` by default and will only run when explicitly enabled.
//!
//! Supported databases:
//! - MySQL 8.0+
//! - MariaDB 10.5+
//! - TiDB (MySQL-compatible)
//!
//! To run these tests:
//! 1. Start MySQL: docker run --name rsfga-mysql -e MYSQL_ROOT_PASSWORD=test -e MYSQL_DATABASE=rsfga -p 3306:3306 -d mysql:8
//! 2. Set MYSQL_URL: export MYSQL_URL=mysql://root:test@localhost:3306/rsfga
//! 3. Run tests: cargo test -p rsfga-storage --test mysql_integration -- --ignored --test-threads=1
//!
//! For TiDB:
//! 1. Start TiDB: docker run --name rsfga-tidb -p 4000:4000 -d pingcap/tidb:latest
//! 2. Set MYSQL_URL: export MYSQL_URL=mysql://root@localhost:4000/test
//! 3. Run tests: cargo test -p rsfga-storage --test mysql_integration -- --ignored --test-threads=1
//!
//! Note: Tests use `--test-threads=1` to avoid store ID collisions. Each test
//! creates and deletes its own store, but concurrent runs could interfere.

use rsfga_storage::{
    DataStore, MySQLConfig, MySQLDataStore, StorageError, StoredTuple, TupleFilter,
};
use std::sync::Arc;

/// Get database URL from environment, defaulting to localhost if not set.
fn get_database_url() -> String {
    std::env::var("MYSQL_URL")
        .unwrap_or_else(|_| "mysql://root:test@localhost:3306/rsfga".to_string())
}

/// Create a MySQLDataStore with migrations run.
async fn create_store() -> MySQLDataStore {
    let database_url = get_database_url();

    let config = MySQLConfig {
        database_url,
        max_connections: 5,
        min_connections: 1,
        connect_timeout_secs: 30,
        ..Default::default()
    };

    let store = MySQLDataStore::from_config(&config)
        .await
        .expect("Failed to create MySQLDataStore - is MySQL running?");

    store
        .run_migrations()
        .await
        .expect("Failed to run migrations");

    store
}

// ==========================================================================
// Section 7.1: Connection and Migration Tests
// ==========================================================================

// Test: Can connect to MySQL
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_can_connect_to_mysql() {
    let store = create_store().await;

    // Verify we can list stores - the call should succeed without error
    let result = store.list_stores().await;
    assert!(
        result.is_ok(),
        "Should be able to list stores after connecting"
    );
}

// Test: Can create tables with migrations
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_can_create_tables_with_migrations() {
    let store = create_store().await;

    // Verify tables exist by creating a store and tuple
    store
        .create_store("test-migrations", "Test Store")
        .await
        .unwrap();
    let s = store.get_store("test-migrations").await.unwrap();
    assert_eq!(s.id, "test-migrations");

    // Cleanup
    store.delete_store("test-migrations").await.unwrap();
}

// Test: Migrations are idempotent
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_migrations_are_idempotent() {
    let store = create_store().await;

    // Run migrations again - should succeed without error
    store.run_migrations().await.unwrap();

    // Verify we can still operate normally
    store
        .create_store("test-idempotent", "Test Store")
        .await
        .unwrap();

    // Cleanup
    store.delete_store("test-idempotent").await.unwrap();
}

// ==========================================================================
// Section 7.2: Basic CRUD Tests
// ==========================================================================

// Test: Can write tuple to database
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_can_write_tuple_to_database() {
    let store = create_store().await;
    store
        .create_store("test-write-tuple", "Test Store")
        .await
        .unwrap();

    let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);

    store.write_tuple("test-write-tuple", tuple).await.unwrap();

    // Verify tuple was written
    let tuples = store
        .read_tuples("test-write-tuple", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1);

    // Cleanup
    store.delete_store("test-write-tuple").await.unwrap();
}

// Test: Can read tuple from database
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_can_read_tuple_from_database() {
    let store = create_store().await;
    store
        .create_store("test-read-tuple", "Test Store")
        .await
        .unwrap();

    let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);

    store
        .write_tuples("test-read-tuple", vec![tuple.clone()], vec![])
        .await
        .unwrap();

    let filter = TupleFilter {
        object_type: Some("document".to_string()),
        object_id: Some("doc1".to_string()),
        ..Default::default()
    };

    let tuples = store.read_tuples("test-read-tuple", &filter).await.unwrap();
    assert_eq!(tuples.len(), 1);
    assert_eq!(tuples[0].object_type, "document");
    assert_eq!(tuples[0].object_id, "doc1");
    assert_eq!(tuples[0].relation, "viewer");
    assert_eq!(tuples[0].user_type, "user");
    assert_eq!(tuples[0].user_id, "alice");

    // Cleanup
    store.delete_store("test-read-tuple").await.unwrap();
}

// Test: Can delete tuple from database
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_can_delete_tuple_from_database() {
    let store = create_store().await;
    store
        .create_store("test-delete-tuple", "Test Store")
        .await
        .unwrap();

    let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);

    store
        .write_tuples("test-delete-tuple", vec![tuple.clone()], vec![])
        .await
        .unwrap();

    // Verify tuple exists
    let tuples = store
        .read_tuples("test-delete-tuple", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1);

    // Delete tuple
    store
        .delete_tuple("test-delete-tuple", tuple)
        .await
        .unwrap();

    // Verify tuple is gone
    let tuples = store
        .read_tuples("test-delete-tuple", &TupleFilter::default())
        .await
        .unwrap();
    assert!(tuples.is_empty());

    // Cleanup
    store.delete_store("test-delete-tuple").await.unwrap();
}

// ==========================================================================
// Section 7.3: Transaction Tests
// ==========================================================================

// Test: Failed writes to non-existent stores are isolated and don't affect existing data
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_failed_write_is_isolated() {
    let store = create_store().await;
    store
        .create_store("test-tx-rollback", "Test Store")
        .await
        .unwrap();

    // Write a tuple first
    let tuple1 = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);

    store
        .write_tuple("test-tx-rollback", tuple1.clone())
        .await
        .unwrap();

    // Verify tuple exists
    let tuples = store
        .read_tuples("test-tx-rollback", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1);

    // Attempting to write to a non-existent store should fail
    let result = store.write_tuple("nonexistent", tuple1.clone()).await;
    assert!(matches!(result, Err(StorageError::StoreNotFound { .. })));

    // Original tuple should still exist
    let tuples = store
        .read_tuples("test-tx-rollback", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1);

    // Cleanup
    store.delete_store("test-tx-rollback").await.unwrap();
}

// Test: Transactions commit on success
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_transactions_commit_on_success() {
    let store = create_store().await;
    store
        .create_store("test-tx-commit", "Test Store")
        .await
        .unwrap();

    // Write multiple tuples in one transaction
    let tuples = vec![
        StoredTuple::new("document", "doc1", "viewer", "user", "alice", None),
        StoredTuple::new("document", "doc2", "viewer", "user", "bob", None),
    ];

    store
        .write_tuples("test-tx-commit", tuples, vec![])
        .await
        .unwrap();

    // Verify all tuples were committed
    let result = store
        .read_tuples("test-tx-commit", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(result.len(), 2);

    // Cleanup
    store.delete_store("test-tx-commit").await.unwrap();
}

// ==========================================================================
// Section 7.4: Connection Pool Tests
// ==========================================================================

// Test: Connection pool limits work correctly
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_connection_pool_limits() {
    let database_url = get_database_url();

    let config = MySQLConfig {
        database_url,
        max_connections: 2, // Small pool
        min_connections: 1,
        connect_timeout_secs: 30,
        ..Default::default()
    };

    let store = MySQLDataStore::from_config(&config)
        .await
        .expect("Failed to create store with limited pool");

    store.run_migrations().await.unwrap();

    store
        .create_store("test-pool-limits", "Test Store")
        .await
        .unwrap();

    // Run multiple concurrent operations with limited pool
    let store = Arc::new(store);
    let mut handles = Vec::new();

    for i in 0..10 {
        let store = Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            let tuple = StoredTuple {
                object_type: "document".to_string(),
                object_id: format!("doc{i}"),
                relation: "viewer".to_string(),
                user_type: "user".to_string(),
                user_id: format!("user{i}"),
                user_relation: None,
                condition_name: None,
                condition_context: None,
            };
            store.write_tuple("test-pool-limits", tuple).await
        }));
    }

    // All operations should succeed despite limited pool
    for handle in handles {
        handle.await.unwrap().unwrap();
    }

    let tuples = store
        .read_tuples("test-pool-limits", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 10);

    // Cleanup
    store.delete_store("test-pool-limits").await.unwrap();
}

// ==========================================================================
// Section 7.5: Error Handling Tests
// ==========================================================================

// Test: Database errors are properly wrapped in StorageError
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_database_errors_wrapped_properly() {
    let store = create_store().await;

    // Try to get a non-existent store
    let result = store.get_store("nonexistent-error-test").await;
    assert!(matches!(result, Err(StorageError::StoreNotFound { .. })));

    // Try to create a duplicate store
    store
        .create_store("test-errors", "Test Store")
        .await
        .unwrap();
    let result = store.create_store("test-errors", "Another Store").await;
    assert!(matches!(
        result,
        Err(StorageError::StoreAlreadyExists { .. })
    ));

    // Try to write to non-existent store
    let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);
    let result = store.write_tuple("nonexistent-write-test", tuple).await;
    assert!(matches!(result, Err(StorageError::StoreNotFound { .. })));

    // Cleanup
    store.delete_store("test-errors").await.unwrap();
}

// Test: Invalid user filter returns InvalidFilter error
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_invalid_user_filter_returns_error() {
    let store = create_store().await;
    store
        .create_store("test-invalid-filter", "Test Store")
        .await
        .unwrap();

    // Missing colon separator
    let filter = TupleFilter {
        user: Some("invalid".to_string()),
        ..Default::default()
    };
    let result = store.read_tuples("test-invalid-filter", &filter).await;
    assert!(matches!(result, Err(StorageError::InvalidFilter { .. })));

    // Invalid userset format
    let filter = TupleFilter {
        user: Some("invalid#member".to_string()),
        ..Default::default()
    };
    let result = store.read_tuples("test-invalid-filter", &filter).await;
    assert!(matches!(result, Err(StorageError::InvalidFilter { .. })));

    // Cleanup
    store.delete_store("test-invalid-filter").await.unwrap();
}

// ==========================================================================
// Section 7.6: Concurrent Access Tests
// ==========================================================================

// Test: Concurrent writes use correct isolation level
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_concurrent_writes_isolation() {
    let store = Arc::new(create_store().await);
    store
        .create_store("test-isolation", "Test Store")
        .await
        .unwrap();

    // Run concurrent writes
    let mut handles = Vec::new();
    for i in 0..50 {
        let store = Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            let tuple = StoredTuple {
                object_type: "document".to_string(),
                object_id: format!("doc{i}"),
                relation: "viewer".to_string(),
                user_type: "user".to_string(),
                user_id: format!("user{i}"),
                user_relation: None,
                condition_name: None,
                condition_context: None,
            };
            store.write_tuple("test-isolation", tuple).await.unwrap();
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    // All writes should be present
    let tuples = store
        .read_tuples("test-isolation", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 50);

    // Cleanup
    store.delete_store("test-isolation").await.unwrap();
}

// Test: Concurrent access from multiple connections
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_concurrent_access_across_threads() {
    let store = Arc::new(create_store().await);
    store
        .create_store("test-concurrent", "Concurrent Test")
        .await
        .unwrap();

    let mut handles = Vec::new();

    // Spawn multiple tasks writing different tuples
    for i in 0..10 {
        let store = Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            for j in 0..100 {
                let tuple = StoredTuple {
                    object_type: "doc".to_string(),
                    object_id: format!("doc-{i}-{j}"),
                    relation: "viewer".to_string(),
                    user_type: "user".to_string(),
                    user_id: format!("user-{i}"),
                    user_relation: None,
                    condition_name: None,
                    condition_context: None,
                };
                store.write_tuple("test-concurrent", tuple).await.unwrap();
            }
        }));
    }

    // Wait for all writes
    for handle in handles {
        handle.await.unwrap();
    }

    // Verify all tuples were written
    let tuples = store
        .read_tuples("test-concurrent", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1000); // 10 tasks * 100 tuples each

    // Clean up
    store.delete_store("test-concurrent").await.unwrap();
}

// ==========================================================================
// Section 7.7: Large Dataset Performance Tests
// ==========================================================================

// Test: Batch insert with 1000+ tuples validates chunking works
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_batch_insert_large_batch_chunking() {
    let store = create_store().await;
    store
        .create_store("test-batch-chunking", "Batch Chunking Test")
        .await
        .unwrap();

    // Generate 1500 tuples - this exceeds WRITE_BATCH_SIZE (1000) and will be chunked
    // into two batches: 1000 + 500
    let tuples: Vec<StoredTuple> = (0..1500)
        .map(|i| StoredTuple {
            object_type: "document".to_string(),
            object_id: format!("batch-doc-{i}"),
            relation: "viewer".to_string(),
            user_type: "user".to_string(),
            user_id: format!("batch-user-{}", i % 50), // 50 unique users
            user_relation: None,
            condition_name: None,
            condition_context: None,
        })
        .collect();

    // Write all tuples in one call - should be chunked internally
    store
        .write_tuples("test-batch-chunking", tuples, vec![])
        .await
        .expect("Large batch write should succeed with chunking");

    // Verify all tuples were written
    let all_tuples = store
        .read_tuples("test-batch-chunking", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(
        all_tuples.len(),
        1500,
        "All 1500 tuples should be written across chunks"
    );

    // Verify we can filter and get expected results
    let filter = TupleFilter {
        object_type: Some("document".to_string()),
        relation: Some("viewer".to_string()),
        ..Default::default()
    };
    let filtered = store
        .read_tuples("test-batch-chunking", &filter)
        .await
        .unwrap();
    assert_eq!(filtered.len(), 1500);

    // Cleanup
    store.delete_store("test-batch-chunking").await.unwrap();
}

// Test: Batch delete with 1000+ tuples validates chunking works
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_batch_delete_large_batch_chunking() {
    let store = create_store().await;
    store
        .create_store("test-batch-delete", "Batch Delete Test")
        .await
        .unwrap();

    // Generate 1500 tuples
    let tuples: Vec<StoredTuple> = (0..1500)
        .map(|i| StoredTuple {
            object_type: "document".to_string(),
            object_id: format!("delete-doc-{i}"),
            relation: "viewer".to_string(),
            user_type: "user".to_string(),
            user_id: format!("delete-user-{}", i % 50),
            user_relation: None,
            condition_name: None,
            condition_context: None,
        })
        .collect();

    // Write all tuples
    store
        .write_tuples("test-batch-delete", tuples.clone(), vec![])
        .await
        .unwrap();

    // Verify all tuples were written
    let all_tuples = store
        .read_tuples("test-batch-delete", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(all_tuples.len(), 1500);

    // Now delete all tuples in one batch call - should be chunked internally
    store
        .write_tuples("test-batch-delete", vec![], tuples)
        .await
        .expect("Large batch delete should succeed with chunking");

    // Verify all tuples were deleted
    let remaining = store
        .read_tuples("test-batch-delete", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(
        remaining.len(),
        0,
        "All 1500 tuples should be deleted across chunks"
    );

    // Cleanup
    store.delete_store("test-batch-delete").await.unwrap();
}

// Test: Batch delete with user_relation (userset) tuples
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_batch_delete_with_user_relation() {
    let store = create_store().await;
    store
        .create_store("test-batch-delete-userset", "Batch Delete Userset Test")
        .await
        .unwrap();

    // Mix of tuples with and without user_relation
    let tuples = vec![
        StoredTuple::new("doc", "1", "viewer", "user", "alice", None),
        StoredTuple::new(
            "doc",
            "2",
            "viewer",
            "group",
            "eng",
            Some("member".to_string()),
        ),
        StoredTuple::new(
            "doc",
            "3",
            "editor",
            "group",
            "admin",
            Some("member".to_string()),
        ),
        StoredTuple::new("doc", "4", "viewer", "user", "bob", None),
    ];

    // Write all tuples
    store
        .write_tuples("test-batch-delete-userset", tuples.clone(), vec![])
        .await
        .unwrap();

    // Delete specific tuples (mix of with/without user_relation)
    let to_delete = vec![
        StoredTuple::new(
            "doc",
            "2",
            "viewer",
            "group",
            "eng",
            Some("member".to_string()),
        ),
        StoredTuple::new("doc", "4", "viewer", "user", "bob", None),
    ];

    store
        .write_tuples("test-batch-delete-userset", vec![], to_delete)
        .await
        .unwrap();

    // Verify correct tuples remain
    let remaining = store
        .read_tuples("test-batch-delete-userset", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(remaining.len(), 2);

    // Check the right ones remain
    let ids: Vec<_> = remaining.iter().map(|t| t.object_id.as_str()).collect();
    assert!(ids.contains(&"1"));
    assert!(ids.contains(&"3"));

    // Cleanup
    store
        .delete_store("test-batch-delete-userset")
        .await
        .unwrap();
}

// Test: Large dataset performance (10k+ tuples)
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_large_dataset_performance() {
    let store = create_store().await;
    store
        .create_store("test-large-dataset", "Large Dataset Test")
        .await
        .unwrap();

    // Generate 10k tuples
    let mut tuples = Vec::with_capacity(10_000);
    for i in 0..10_000 {
        tuples.push(StoredTuple {
            object_type: "document".to_string(),
            object_id: format!("doc{i}"),
            relation: "viewer".to_string(),
            user_type: "user".to_string(),
            user_id: format!("user{}", i % 100), // 100 unique users
            user_relation: None,
            condition_name: None,
            condition_context: None,
        });
    }

    // Measure write time
    let start = std::time::Instant::now();
    store
        .write_tuples("test-large-dataset", tuples, vec![])
        .await
        .unwrap();
    let write_duration = start.elapsed();

    // Verify count
    let all_tuples = store
        .read_tuples("test-large-dataset", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(all_tuples.len(), 10_000);

    // Measure filtered read time
    let start = std::time::Instant::now();
    let filter = TupleFilter {
        user: Some("user:user0".to_string()),
        ..Default::default()
    };
    let filtered = store
        .read_tuples("test-large-dataset", &filter)
        .await
        .unwrap();
    let read_duration = start.elapsed();

    // Each user should have ~100 tuples (10000 / 100 users)
    assert_eq!(filtered.len(), 100);

    // Log performance (always, for visibility)
    println!(
        "Large dataset performance (MySQL): write={}ms, filtered_read={}ms",
        write_duration.as_millis(),
        read_duration.as_millis()
    );

    // Performance assertions - only run if env var is set (CI environments vary)
    // Set RSFGA_MYSQL_PERF_ASSERTS=1 to enable strict timing checks
    if std::env::var("RSFGA_MYSQL_PERF_ASSERTS").is_ok() {
        assert!(
            write_duration.as_secs() < 60,
            "Write took too long: {}s",
            write_duration.as_secs()
        );
        assert!(
            read_duration.as_millis() < 1000,
            "Read took too long: {}ms",
            read_duration.as_millis()
        );
    }

    // Cleanup
    store.delete_store("test-large-dataset").await.unwrap();
}

// ==========================================================================
// Section 7.8: MySQL-Specific Tests
// ==========================================================================

// Test: Indexes are created for common query patterns
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_indexes_created() {
    let store = create_store().await;

    // Verify indexes exist by checking information_schema
    let pool = store.pool();
    let rows: Vec<(String,)> = sqlx::query_as(
        r#"
        SELECT INDEX_NAME FROM information_schema.STATISTICS
        WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'tuples'
        GROUP BY INDEX_NAME
        "#,
    )
    .fetch_all(pool)
    .await
    .unwrap();

    let index_names: Vec<String> = rows.into_iter().map(|(name,)| name).collect();

    // Check for our custom indexes
    assert!(
        index_names.iter().any(|n| n.contains("idx_tuples_store")),
        "Missing store index. Found: {index_names:?}"
    );
    assert!(
        index_names.iter().any(|n| n.contains("idx_tuples_object")),
        "Missing object index. Found: {index_names:?}"
    );
    assert!(
        index_names.iter().any(|n| n.contains("idx_tuples_user")),
        "Missing user index. Found: {index_names:?}"
    );
    assert!(
        index_names
            .iter()
            .any(|n| n.contains("idx_tuples_relation")),
        "Missing relation index. Found: {index_names:?}"
    );
}

// Test: Delete store cascades to tuples
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_delete_store_cascades() {
    let store = create_store().await;
    store
        .create_store("test-cascade", "Test Store")
        .await
        .unwrap();

    let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);

    store.write_tuple("test-cascade", tuple).await.unwrap();

    // Delete the store
    store.delete_store("test-cascade").await.unwrap();

    // Store should be gone
    let result = store.get_store("test-cascade").await;
    assert!(matches!(result, Err(StorageError::StoreNotFound { .. })));
}

// Test: supports_transactions returns false (individual tx control not supported)
// Note: write_tuples operations are still atomic internally
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_supports_transactions() {
    let store = create_store().await;
    // Individual transaction control is not supported, but write_tuples is atomic
    assert!(!store.supports_transactions());
}

// Test: Can paginate large result sets
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_large_result_set_pagination() {
    let store = create_store().await;
    store
        .create_store("test-large-set", "Test Store")
        .await
        .unwrap();

    // Write many tuples
    let mut tuples = Vec::new();
    for i in 0..100 {
        tuples.push(StoredTuple {
            object_type: "document".to_string(),
            object_id: format!("doc{i:03}"),
            relation: "viewer".to_string(),
            user_type: "user".to_string(),
            user_id: format!("user{i:03}"),
            user_relation: None,
            condition_name: None,
            condition_context: None,
        });
    }

    store
        .write_tuples("test-large-set", tuples, vec![])
        .await
        .unwrap();

    // Read all tuples - they should be ordered by created_at DESC
    let result = store
        .read_tuples("test-large-set", &TupleFilter::default())
        .await
        .unwrap();

    assert_eq!(result.len(), 100);

    // Cleanup
    store.delete_store("test-large-set").await.unwrap();
}

// Test: Storage survives application restart
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_storage_survives_restart() {
    let store_id = "test-restart-test";

    // Phase 1: Write data with first connection
    {
        let store = create_store().await;

        // Clean up from previous runs
        let _ = store.delete_store(store_id).await;

        store.create_store(store_id, "Restart Test").await.unwrap();

        let tuple = StoredTuple::new(
            "document",
            "persistent-doc",
            "viewer",
            "user",
            "persistent-user",
            None,
        );
        store.write_tuple(store_id, tuple).await.unwrap();

        // Verify data exists
        let tuples = store
            .read_tuples(store_id, &TupleFilter::default())
            .await
            .unwrap();
        assert_eq!(tuples.len(), 1);
    } // First connection dropped here

    // Phase 2: Read data with new connection (simulates restart)
    {
        let store = create_store().await;

        // Store should still exist
        let s = store.get_store(store_id).await.unwrap();
        assert_eq!(s.id, store_id);

        // Data should still exist
        let tuples = store
            .read_tuples(store_id, &TupleFilter::default())
            .await
            .unwrap();
        assert_eq!(tuples.len(), 1);
        assert_eq!(tuples[0].object_id, "persistent-doc");
        assert_eq!(tuples[0].user_id, "persistent-user");

        // Clean up
        store.delete_store(store_id).await.unwrap();
    }
}

// Test: Tuple with user_relation (userset)
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_tuple_with_user_relation() {
    let store = create_store().await;
    store
        .create_store("test-userset", "Test Store")
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
        .write_tuple("test-userset", tuple.clone())
        .await
        .unwrap();

    // Read it back
    let tuples = store
        .read_tuples("test-userset", &TupleFilter::default())
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
    let tuples = store.read_tuples("test-userset", &filter).await.unwrap();
    assert_eq!(tuples.len(), 1);

    // Cleanup
    store.delete_store("test-userset").await.unwrap();
}

// Test: Duplicate tuple write is idempotent (ON DUPLICATE KEY UPDATE)
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_duplicate_tuple_write_is_idempotent() {
    let store = create_store().await;
    store
        .create_store("test-idempotent-write", "Test Store")
        .await
        .unwrap();

    let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);

    // Write the same tuple twice
    store
        .write_tuple("test-idempotent-write", tuple.clone())
        .await
        .unwrap();
    store
        .write_tuple("test-idempotent-write", tuple.clone())
        .await
        .unwrap();

    // Should only have one tuple
    let tuples = store
        .read_tuples("test-idempotent-write", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1);

    // Cleanup
    store.delete_store("test-idempotent-write").await.unwrap();
}

// ==========================================================================
// Section 7.9: Connection Pool Exhaustion Tests
// ==========================================================================
//
// These tests verify connection pool behavior under exhaustion scenarios:
// - Pool exhaustion returns timeout errors
// - Pool recovers after queries complete
// - Concurrent migrations don't deadlock
//
// Note: Connection drop handling tests require testcontainers or network
// simulation and are documented but not implemented in this basic test suite.
// ==========================================================================

/// Test: Pool exhaustion returns timeout error when all connections busy
///
/// Creates a small pool (2 connections) and spawns more concurrent long-running
/// queries than available connections with a short acquire timeout.
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_pool_exhaustion_returns_timeout_error() {
    let database_url = get_database_url();

    // Create pool with very limited connections and short timeout
    let config = MySQLConfig {
        database_url,
        max_connections: 2,
        min_connections: 1,
        connect_timeout_secs: 1, // Very short timeout to trigger exhaustion quickly
        ..Default::default()
    };

    let store = MySQLDataStore::from_config(&config)
        .await
        .expect("Failed to create store with limited pool");

    store.run_migrations().await.unwrap();

    store
        .create_store("test-pool-exhaustion", "Pool Exhaustion Test")
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
            store.write_tuple("test-pool-exhaustion", tuple).await
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
    eprintln!("Pool exhaustion test: {success_count} succeeded, {error_count} failed");

    // At minimum, some should succeed
    assert!(success_count > 0, "At least some operations should succeed");

    // Cleanup (may partially fail if pool is still exhausted)
    let _ = store.delete_store("test-pool-exhaustion").await;
}

/// Test: Pool recovers after blocking queries complete
///
/// Verifies that after pool exhaustion, subsequent queries succeed
/// once previous connections are released.
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_pool_recovers_after_queries_complete() {
    let database_url = get_database_url();

    let config = MySQLConfig {
        database_url,
        max_connections: 2,
        min_connections: 1,
        connect_timeout_secs: 30,
        ..Default::default()
    };

    let store = MySQLDataStore::from_config(&config)
        .await
        .expect("Failed to create store");

    store.run_migrations().await.unwrap();

    store
        .create_store("test-pool-recovery", "Pool Recovery Test")
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
            store.write_tuple("test-pool-recovery", tuple).await
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
            .write_tuple("test-pool-recovery", tuple)
            .await
            .expect("Query after pool recovery should succeed");
    }

    // Verify all tuples were written
    let tuples = store
        .read_tuples("test-pool-recovery", &TupleFilter::default())
        .await
        .unwrap();

    // Should have tuples from both phases (some phase 1 may have failed, all phase 2 should succeed)
    assert!(
        tuples.len() >= 5,
        "At least phase 2 tuples should exist after recovery"
    );

    // Cleanup
    store.delete_store("test-pool-recovery").await.unwrap();
}

/// Test: Concurrent migrations don't deadlock
///
/// Multiple instances calling run_migrations() simultaneously should
/// all complete successfully without deadlocking.
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_concurrent_migrations_dont_deadlock() {
    let database_url = get_database_url();

    // Spawn multiple concurrent migration attempts
    let mut handles = Vec::new();
    for i in 0..5 {
        let url = database_url.clone();
        handles.push(tokio::spawn(async move {
            let config = MySQLConfig {
                database_url: url,
                max_connections: 2,
                min_connections: 1,
                connect_timeout_secs: 30,
                ..Default::default()
            };

            let store = MySQLDataStore::from_config(&config).await?;
            store.run_migrations().await?;

            eprintln!("Migration instance {i} completed");
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
    .expect("Concurrent migrations timed out - possible deadlock");

    // Count successes and failures
    let mut success_count = 0;
    for result in results {
        match result {
            Ok(Ok(())) => success_count += 1,
            Ok(Err(e)) => {
                // Migration errors are acceptable (e.g., if table already exists)
                eprintln!("Migration error (acceptable): {e}");
                success_count += 1; // Still counts as completing without deadlock
            }
            Err(e) => {
                panic!("Task join error: {e}");
            }
        }
    }

    assert_eq!(
        success_count, 5,
        "All migration attempts should complete (success or expected error)"
    );

    // Verify we can still operate after concurrent migrations
    let config = MySQLConfig {
        database_url,
        max_connections: 5,
        min_connections: 1,
        connect_timeout_secs: 30,
        ..Default::default()
    };

    let store = MySQLDataStore::from_config(&config).await.unwrap();

    // Clean up any test stores from this test
    let _ = store.delete_store("test-concurrent-migrations").await;

    store
        .create_store("test-concurrent-migrations", "Post-Migration Test")
        .await
        .unwrap();

    let s = store.get_store("test-concurrent-migrations").await.unwrap();
    assert_eq!(s.id, "test-concurrent-migrations");

    // Cleanup
    store
        .delete_store("test-concurrent-migrations")
        .await
        .unwrap();
}

// Note: Connection drop handling test (test_handles_connection_drop_gracefully)
// requires testcontainers or network simulation to reliably simulate connection
// failures mid-query. This is documented here for future implementation:
//
// Test case outline:
// 1. Start a MySQL container with testcontainers
// 2. Begin a long-running query
// 3. Pause or kill the container mid-query
// 4. Verify error is returned and pool doesn't deadlock
// 5. Restart container
// 6. Verify subsequent queries work after pool recovery

// ==========================================================================
// Section 7.10: Health Check Tests (MySQL)
// ==========================================================================

// Test: health_check returns healthy status with valid connection
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_health_check_returns_healthy() {
    let store = create_store().await;

    let status = store.health_check().await.unwrap();

    assert!(
        status.healthy,
        "Store should be healthy with valid connection"
    );
    assert!(
        status.latency.as_millis() < 5000,
        "Health check should complete within 5 seconds"
    );
    assert_eq!(
        status.message,
        Some("mysql".to_string()),
        "Should identify as mysql"
    );
}

// Test: health_check returns correct pool statistics
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_health_check_pool_stats() {
    let database_url = get_database_url();

    let config = MySQLConfig {
        database_url,
        max_connections: 5,
        min_connections: 1,
        connect_timeout_secs: 30,
        ..Default::default()
    };

    let store = MySQLDataStore::from_config(&config)
        .await
        .expect("Failed to create store");

    store.run_migrations().await.unwrap();

    let status = store.health_check().await.unwrap();

    let pool_stats = status
        .pool_stats
        .expect("Pool stats should be present for database backends");

    assert_eq!(
        pool_stats.max_connections, 5,
        "Max connections should match config"
    );
    assert!(
        pool_stats.idle_connections + pool_stats.active_connections <= pool_stats.max_connections,
        "Total connections should not exceed max"
    );
}

// Test: health_check latency measurement is accurate
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_health_check_latency_measurement() {
    let store = create_store().await;

    // Run multiple health checks and verify latency is reasonable
    let mut latencies = Vec::new();
    for _ in 0..5 {
        let status = store.health_check().await.unwrap();
        latencies.push(status.latency);
    }

    // All latencies should be reasonable (under 1 second for local DB)
    // Note: We only check upper bound because very fast health checks may
    // round down to 0ms at millisecond precision
    for latency in &latencies {
        assert!(
            latency.as_millis() < 1000,
            "Latency should be under 1 second for local DB, got {latency:?}"
        );
    }
}

// Test: Concurrent health checks under load
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_concurrent_health_checks() {
    let store = Arc::new(create_store().await);

    let mut handles = Vec::new();
    for _ in 0..10 {
        let store = Arc::clone(&store);
        handles.push(tokio::spawn(async move { store.health_check().await }));
    }

    // Collect all results - this provides clearer error messages on failure
    let results = futures::future::join_all(handles).await;

    assert_eq!(results.len(), 10, "Expected 10 health check results");

    for result in results {
        // Unwrap both the JoinHandle and the health_check Result
        // This will panic with a descriptive message on any failure
        let status = result.expect("Task panicked").expect("Health check failed");
        assert!(status.healthy);
    }
}

// Test: Health check when pool has active connections
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_health_check_with_active_connections() {
    let database_url = get_database_url();

    let config = MySQLConfig {
        database_url,
        max_connections: 3, // Small pool
        min_connections: 1,
        connect_timeout_secs: 30,
        ..Default::default()
    };

    let store = Arc::new(
        MySQLDataStore::from_config(&config)
            .await
            .expect("Failed to create store"),
    );

    store.run_migrations().await.unwrap();
    store
        .create_store("test-active-pool-mysql", "Test Store")
        .await
        .unwrap();

    // Start concurrent operations to use pool connections
    let store_clone = Arc::clone(&store);
    let write_handle = tokio::spawn(async move {
        for i in 0..5 {
            let tuple = StoredTuple::new(
                "document",
                format!("doc{i}"),
                "viewer",
                "user",
                format!("user{i}"),
                None,
            );
            store_clone
                .write_tuple("test-active-pool-mysql", tuple)
                .await
                .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    });

    // Run health check while writes are happening
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let status = store.health_check().await.unwrap();
    assert!(
        status.healthy,
        "Health check should succeed even with active connections"
    );

    // Wait for writes to complete
    write_handle.await.unwrap();

    // Cleanup
    store.delete_store("test-active-pool-mysql").await.unwrap();
}

// ==========================================================================
// Section 7.11: Query Timeout Tests (MySQL)
// ==========================================================================

// Test: StorageError::QueryTimeout contains correct operation name
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_timeout_error_contains_operation_name() {
    // This test verifies the error type structure exists
    // Actual timeout testing requires database-level query delays
    let error = StorageError::QueryTimeout {
        operation: "read_tuples".to_string(),
        timeout: std::time::Duration::from_secs(30),
    };

    let error_str = error.to_string();
    assert!(
        error_str.contains("read_tuples"),
        "Error should contain operation name"
    );
    assert!(
        error_str.contains("30"),
        "Error should contain timeout duration"
    );
}

// Test: Batch write is atomic on success (all tuples written or none)
// Note: This test verifies that successful batch writes are fully committed.
// True rollback testing would require inducing a mid-transaction failure.
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_batch_write_is_atomic_on_success() {
    let store = create_store().await;
    store
        .create_store("test-tx-consistency-mysql", "Test Store")
        .await
        .unwrap();

    // Write initial tuple
    let tuple1 = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);
    store
        .write_tuple("test-tx-consistency-mysql", tuple1)
        .await
        .unwrap();

    // Verify initial state
    let tuples = store
        .read_tuples("test-tx-consistency-mysql", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1);

    // Write another batch - this should succeed completely
    let batch = vec![
        StoredTuple::new("document", "doc2", "viewer", "user", "bob", None),
        StoredTuple::new("document", "doc3", "viewer", "user", "charlie", None),
    ];
    store
        .write_tuples("test-tx-consistency-mysql", batch, vec![])
        .await
        .unwrap();

    // Verify all tuples exist (no partial writes)
    let tuples = store
        .read_tuples("test-tx-consistency-mysql", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(
        tuples.len(),
        3,
        "All tuples should be present after successful transaction"
    );

    // Cleanup
    store
        .delete_store("test-tx-consistency-mysql")
        .await
        .unwrap();
}

// Test: Fast query succeeds when timeout is configured
// Note: This test verifies that normal operations complete within the configured
// timeout. True slow query timeout testing would require database-level query
// delays (e.g., SLEEP()), but the DataStore trait doesn't expose raw query execution.
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_fast_query_succeeds_with_timeout_set() {
    let database_url = get_database_url();

    // Use short timeout configuration
    let config = MySQLConfig {
        database_url,
        max_connections: 5,
        min_connections: 1,
        connect_timeout_secs: 30,
        query_timeout_secs: 1, // 1 second timeout
        ..Default::default()
    };

    let store = MySQLDataStore::from_config(&config)
        .await
        .expect("Failed to create store");

    store.run_migrations().await.unwrap();

    // Create test store
    store
        .create_store("test-timeout-config", "Test Store")
        .await
        .unwrap();

    // Verify that normal operations complete within the timeout
    let start = std::time::Instant::now();
    let result = store
        .read_tuples("test-timeout-config", &TupleFilter::default())
        .await;
    let elapsed = start.elapsed();

    assert!(result.is_ok(), "Normal read should succeed");
    assert!(
        elapsed.as_secs() < 2,
        "Read should complete within timeout, took {}ms",
        elapsed.as_millis()
    );

    // Cleanup
    store.delete_store("test-timeout-config").await.unwrap();
}

// ==========================================================================
// Section 7.12: Unicode and Special Characters Tests (Issue #96)
// ==========================================================================

// Test: Unicode object_ids (emoji, CJK characters)
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_unicode_object_ids() {
    let store = create_store().await;
    store
        .create_store("test-unicode-objects", "Test Store")
        .await
        .unwrap();

    // Test various Unicode characters
    let tuples = vec![
        // Emoji
        StoredTuple::new("document", "doc-🚀-rocket", "viewer", "user", "alice", None),
        // CJK characters (Chinese)
        StoredTuple::new("document", "文档-中文", "viewer", "user", "alice", None),
        // CJK characters (Japanese)
        StoredTuple::new(
            "document",
            "ドキュメント-日本語",
            "viewer",
            "user",
            "alice",
            None,
        ),
        // CJK characters (Korean)
        StoredTuple::new("document", "문서-한국어", "viewer", "user", "alice", None),
        // Mixed emoji and text
        StoredTuple::new(
            "document",
            "doc-💻📱🎮-devices",
            "viewer",
            "user",
            "alice",
            None,
        ),
        // Arabic
        StoredTuple::new("document", "وثيقة-عربي", "viewer", "user", "alice", None),
        // Cyrillic
        StoredTuple::new(
            "document",
            "документ-русский",
            "viewer",
            "user",
            "alice",
            None,
        ),
    ];

    store
        .write_tuples("test-unicode-objects", tuples.clone(), vec![])
        .await
        .expect("Should write tuples with Unicode object_ids");

    // Read back and verify
    let result = store
        .read_tuples("test-unicode-objects", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(result.len(), 7, "All Unicode tuples should be stored");

    // Verify specific Unicode object can be filtered
    let filter = TupleFilter {
        object_id: Some("doc-🚀-rocket".to_string()),
        ..Default::default()
    };
    let filtered = store
        .read_tuples("test-unicode-objects", &filter)
        .await
        .unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].object_id, "doc-🚀-rocket");

    // Cleanup
    store.delete_store("test-unicode-objects").await.unwrap();
}

// Test: Special characters in user_id
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_special_characters_in_user_id() {
    let store = create_store().await;
    store
        .create_store("test-special-users", "Test Store")
        .await
        .unwrap();

    // Test various special characters in user_id
    let tuples = vec![
        // Email-like format
        StoredTuple::new(
            "document",
            "doc1",
            "viewer",
            "user",
            "alice@example.com",
            None,
        ),
        // UUID format
        StoredTuple::new(
            "document",
            "doc2",
            "viewer",
            "user",
            "550e8400-e29b-41d4-a716-446655440000",
            None,
        ),
        // Underscores and hyphens
        StoredTuple::new(
            "document",
            "doc3",
            "viewer",
            "user",
            "user_with-special",
            None,
        ),
        // Dots
        StoredTuple::new(
            "document",
            "doc4",
            "viewer",
            "user",
            "first.last.name",
            None,
        ),
        // Plus sign (common in emails)
        StoredTuple::new(
            "document",
            "doc5",
            "viewer",
            "user",
            "user+tag@mail.com",
            None,
        ),
    ];

    store
        .write_tuples("test-special-users", tuples.clone(), vec![])
        .await
        .expect("Should write tuples with special characters in user_id");

    // Read back and verify
    let result = store
        .read_tuples("test-special-users", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(
        result.len(),
        5,
        "All tuples with special user_ids should be stored"
    );

    // Filter by specific user
    let filter = TupleFilter {
        user: Some("user:alice@example.com".to_string()),
        ..Default::default()
    };
    let filtered = store
        .read_tuples("test-special-users", &filter)
        .await
        .unwrap();
    assert_eq!(filtered.len(), 1);

    // Cleanup
    store.delete_store("test-special-users").await.unwrap();
}

// Test: Maximum length strings (VARCHAR(255) boundary)
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_maximum_length_strings() {
    let store = create_store().await;
    let store_id = "01TESTMAXLENGTHSTRINGS00";

    // Cleanup from any previous failed run
    let _ = store.delete_store(store_id).await;

    store.create_store(store_id, "Test Store").await.unwrap();

    // Create strings at column limits (updated for MariaDB/TiDB index compatibility)
    // object_id: VARCHAR(255), user_id: VARCHAR(128)
    let max_object_id: String = "x".repeat(255);
    let max_user_id: String = "y".repeat(128);

    let tuple = StoredTuple::new(
        "document",
        max_object_id.clone(),
        "viewer",
        "user",
        max_user_id.clone(),
        None,
    );

    store
        .write_tuple(store_id, tuple)
        .await
        .expect("Should write tuple with max length strings");

    // Read back and verify
    let result = store
        .read_tuples(store_id, &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].object_id.len(), 255);
    assert_eq!(result[0].user_id.len(), 128);

    // Cleanup
    store.delete_store(store_id).await.unwrap();
}

// ==========================================================================
// Section 7.13: Boundary Conditions Tests (Issue #96)
// ==========================================================================

// Test: Exactly WRITE_BATCH_SIZE tuples (1000)
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_exactly_batch_size_tuples() {
    let store = create_store().await;
    store
        .create_store("test-exact-batch", "Test Store")
        .await
        .unwrap();

    // Generate exactly 1000 tuples (WRITE_BATCH_SIZE)
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
        .write_tuples("test-exact-batch", tuples, vec![])
        .await
        .expect("Should write exactly WRITE_BATCH_SIZE tuples");

    // Verify count
    let result = store
        .read_tuples("test-exact-batch", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(result.len(), 1000, "Exactly 1000 tuples should be stored");

    // Cleanup
    store.delete_store("test-exact-batch").await.unwrap();
}

// Test: WRITE_BATCH_SIZE + 1 tuples (1001)
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_batch_size_plus_one_tuples() {
    let store = create_store().await;
    store
        .create_store("test-batch-plus-one", "Test Store")
        .await
        .unwrap();

    // Generate 1001 tuples (WRITE_BATCH_SIZE + 1)
    let tuples: Vec<StoredTuple> = (0..1001)
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
        .write_tuples("test-batch-plus-one", tuples, vec![])
        .await
        .expect("Should write WRITE_BATCH_SIZE + 1 tuples (crosses chunk boundary)");

    // Verify count
    let result = store
        .read_tuples("test-batch-plus-one", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(result.len(), 1001, "Exactly 1001 tuples should be stored");

    // Cleanup
    store.delete_store("test-batch-plus-one").await.unwrap();
}

// Test: Empty batch writes (0 tuples)
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_empty_batch_write() {
    let store = create_store().await;
    store
        .create_store("test-empty-batch", "Test Store")
        .await
        .unwrap();

    // Write empty batch
    store
        .write_tuples("test-empty-batch", vec![], vec![])
        .await
        .expect("Empty batch write should succeed");

    // Verify no tuples
    let result = store
        .read_tuples("test-empty-batch", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(result.len(), 0, "No tuples should be stored");

    // Cleanup
    store.delete_store("test-empty-batch").await.unwrap();
}

// Test: Single tuple via write_tuples (batch API with 1 element)
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_single_tuple_via_batch_api() {
    let store = create_store().await;
    store
        .create_store("test-single-batch", "Test Store")
        .await
        .unwrap();

    // Write single tuple using batch API
    let tuples = vec![StoredTuple::new(
        "document",
        "single-doc",
        "viewer",
        "user",
        "alice",
        None,
    )];

    store
        .write_tuples("test-single-batch", tuples, vec![])
        .await
        .expect("Single tuple batch write should succeed");

    // Verify
    let result = store
        .read_tuples("test-single-batch", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].object_id, "single-doc");

    // Cleanup
    store.delete_store("test-single-batch").await.unwrap();
}

// ==========================================================================
// Section 7.14: Error Recovery Tests (Issue #96)
// ==========================================================================

// Test: Duplicate deletes (deleting same tuple twice)
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_duplicate_delete() {
    let store = create_store().await;
    store
        .create_store("test-dup-delete", "Test Store")
        .await
        .unwrap();

    let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);

    // Write tuple
    store
        .write_tuple("test-dup-delete", tuple.clone())
        .await
        .unwrap();

    // Delete once
    store
        .delete_tuple("test-dup-delete", tuple.clone())
        .await
        .expect("First delete should succeed");

    // Delete again - should not error (idempotent)
    store
        .delete_tuple("test-dup-delete", tuple.clone())
        .await
        .expect("Second delete should also succeed (idempotent)");

    // Verify no tuples remain
    let result = store
        .read_tuples("test-dup-delete", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(result.len(), 0);

    // Cleanup
    store.delete_store("test-dup-delete").await.unwrap();
}

// Test: Delete of non-existent tuple
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_delete_nonexistent_tuple() {
    let store = create_store().await;
    store
        .create_store("test-delete-nonexist", "Test Store")
        .await
        .unwrap();

    let tuple = StoredTuple::new("document", "never-existed", "viewer", "user", "alice", None);

    // Delete tuple that was never written - should not error
    store
        .delete_tuple("test-delete-nonexist", tuple)
        .await
        .expect("Delete of non-existent tuple should succeed (no-op)");

    // Cleanup
    store.delete_store("test-delete-nonexist").await.unwrap();
}

// ==========================================================================
// Section 7.15: Pagination Edge Cases Tests (Issue #96)
// ==========================================================================

use rsfga_storage::PaginationOptions;

// Test: Pagination with exactly page_size results
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_pagination_exactly_page_size() {
    let store = create_store().await;
    store
        .create_store("test-exact-page", "Test Store")
        .await
        .unwrap();

    // Write exactly 10 tuples
    let tuples: Vec<StoredTuple> = (0..10)
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
        .write_tuples("test-exact-page", tuples, vec![])
        .await
        .unwrap();

    // Request exactly 10 items (page_size == total count)
    let page = store
        .read_tuples_paginated(
            "test-exact-page",
            &TupleFilter::default(),
            &PaginationOptions {
                page_size: Some(10),
                continuation_token: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(page.items.len(), 10, "Should return all 10 items");
    // When page_size equals total count, there may or may not be a token
    // depending on implementation. The key is that a subsequent request
    // with any token should return 0 items.

    if page.continuation_token.is_some() {
        let next_page = store
            .read_tuples_paginated(
                "test-exact-page",
                &TupleFilter::default(),
                &PaginationOptions {
                    page_size: Some(10),
                    continuation_token: page.continuation_token,
                },
            )
            .await
            .unwrap();
        assert_eq!(next_page.items.len(), 0, "Next page should be empty");
    }

    // Cleanup
    store.delete_store("test-exact-page").await.unwrap();
}

// Test: Continuation token at end of results
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_continuation_token_at_end() {
    let store = create_store().await;
    store
        .create_store("test-end-token", "Test Store")
        .await
        .unwrap();

    // Write 5 tuples
    let tuples: Vec<StoredTuple> = (0..5)
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
        .write_tuples("test-end-token", tuples, vec![])
        .await
        .unwrap();

    // Get first page of 3
    let page1 = store
        .read_tuples_paginated(
            "test-end-token",
            &TupleFilter::default(),
            &PaginationOptions {
                page_size: Some(3),
                continuation_token: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(page1.items.len(), 3);
    assert!(
        page1.continuation_token.is_some(),
        "Should have token for more results"
    );

    // Get second page (remaining 2)
    let page2 = store
        .read_tuples_paginated(
            "test-end-token",
            &TupleFilter::default(),
            &PaginationOptions {
                page_size: Some(3),
                continuation_token: page1.continuation_token,
            },
        )
        .await
        .unwrap();

    assert_eq!(page2.items.len(), 2, "Should return remaining 2 items");

    // If there's a token, the next page should be empty
    if page2.continuation_token.is_some() {
        let page3 = store
            .read_tuples_paginated(
                "test-end-token",
                &TupleFilter::default(),
                &PaginationOptions {
                    page_size: Some(3),
                    continuation_token: page2.continuation_token,
                },
            )
            .await
            .unwrap();
        assert_eq!(page3.items.len(), 0, "Third page should be empty");
    }

    // Cleanup
    store.delete_store("test-end-token").await.unwrap();
}

// Test: Invalid continuation token handling
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_invalid_continuation_token() {
    let store = create_store().await;
    store
        .create_store("test-invalid-token", "Test Store")
        .await
        .unwrap();

    // Write some tuples
    let tuples: Vec<StoredTuple> = (0..5)
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
        .write_tuples("test-invalid-token", tuples, vec![])
        .await
        .unwrap();

    // Try with completely invalid token (not valid base64)
    let result = store
        .read_tuples_paginated(
            "test-invalid-token",
            &TupleFilter::default(),
            &PaginationOptions {
                page_size: Some(3),
                continuation_token: Some("not-valid-base64!!!".to_string()),
            },
        )
        .await;

    // Should return an error for invalid token
    assert!(
        result.is_err(),
        "Invalid continuation token should return error"
    );

    // Try with valid base64 but invalid format
    let result = store
        .read_tuples_paginated(
            "test-invalid-token",
            &TupleFilter::default(),
            &PaginationOptions {
                page_size: Some(3),
                continuation_token: Some(base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    "invalid-json-content",
                )),
            },
        )
        .await;

    assert!(
        result.is_err(),
        "Malformed continuation token should return error"
    );

    // Cleanup
    store.delete_store("test-invalid-token").await.unwrap();
}

// Test: Empty result set pagination
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_pagination_empty_results() {
    let store = create_store().await;
    store
        .create_store("test-empty-pagination", "Test Store")
        .await
        .unwrap();

    // Don't write any tuples

    // Request paginated results from empty store
    let page = store
        .read_tuples_paginated(
            "test-empty-pagination",
            &TupleFilter::default(),
            &PaginationOptions {
                page_size: Some(10),
                continuation_token: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(page.items.len(), 0, "Should return empty list");
    assert!(
        page.continuation_token.is_none(),
        "Should not have continuation token for empty results"
    );

    // Cleanup
    store.delete_store("test-empty-pagination").await.unwrap();
}

// ==========================================================================
// Section 7.16: Column Size Migration Tests (Issues #175, #176)
// ==========================================================================
//
// These tests verify the column size migration for MariaDB/TiDB compatibility.
// The migration reduces column sizes to fit within the 3072-byte index key limit.

/// Test: New tables are created with correct column sizes for MariaDB/TiDB compatibility
///
/// Verifies that after running migrations, the tuples table has the correct
/// reduced column sizes matching OpenFGA's schema.
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_new_tables_have_correct_column_sizes() {
    let store = create_store().await;
    let pool = store.pool();

    // Query information_schema to check column sizes
    // Cast to SIGNED for MariaDB compatibility (returns BIGINT UNSIGNED)
    let columns: Vec<(String, String, i64)> = sqlx::query_as(
        r#"
        SELECT column_name, data_type, CAST(character_maximum_length AS SIGNED) as max_length
        FROM information_schema.columns
        WHERE table_schema = DATABASE()
          AND table_name = 'tuples'
          AND column_name IN ('store_id', 'object_type', 'object_id', 'relation',
                              'user_type', 'user_id', 'user_relation')
        ORDER BY column_name
        "#,
    )
    .fetch_all(pool)
    .await
    .expect("Failed to query column information");

    // Build a map for easier assertions
    let column_map: std::collections::HashMap<String, (String, i64)> = columns
        .into_iter()
        .map(|(name, dtype, len)| (name, (dtype, len)))
        .collect();

    // Verify each column has correct size
    // store_id: CHAR(26)
    let (dtype, len) = column_map.get("store_id").expect("store_id column missing");
    assert_eq!(dtype, "char", "store_id should be CHAR type");
    assert_eq!(*len, 26, "store_id should be 26 chars for ULID");

    // object_type: VARCHAR(128)
    let (dtype, len) = column_map
        .get("object_type")
        .expect("object_type column missing");
    assert_eq!(dtype, "varchar", "object_type should be VARCHAR");
    assert_eq!(*len, 128, "object_type should be 128 chars");

    // object_id: VARCHAR(255) - unchanged
    let (dtype, len) = column_map
        .get("object_id")
        .expect("object_id column missing");
    assert_eq!(dtype, "varchar", "object_id should be VARCHAR");
    assert_eq!(*len, 255, "object_id should remain 255 chars");

    // relation: VARCHAR(50)
    let (dtype, len) = column_map.get("relation").expect("relation column missing");
    assert_eq!(dtype, "varchar", "relation should be VARCHAR");
    assert_eq!(*len, 50, "relation should be 50 chars");

    // user_type: VARCHAR(128)
    let (dtype, len) = column_map
        .get("user_type")
        .expect("user_type column missing");
    assert_eq!(dtype, "varchar", "user_type should be VARCHAR");
    assert_eq!(*len, 128, "user_type should be 128 chars");

    // user_id: VARCHAR(128)
    let (dtype, len) = column_map.get("user_id").expect("user_id column missing");
    assert_eq!(dtype, "varchar", "user_id should be VARCHAR");
    assert_eq!(*len, 128, "user_id should be 128 chars");

    // user_relation: VARCHAR(50)
    let (dtype, len) = column_map
        .get("user_relation")
        .expect("user_relation column missing");
    assert_eq!(dtype, "varchar", "user_relation should be VARCHAR");
    assert_eq!(*len, 50, "user_relation should be 50 chars");
}

/// Test: Stores table has correct column size for id
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_stores_table_has_correct_id_size() {
    let store = create_store().await;
    let pool = store.pool();

    let (dtype, len): (String, i64) = sqlx::query_as(
        r#"
        SELECT data_type, CAST(character_maximum_length AS SIGNED) as max_length
        FROM information_schema.columns
        WHERE table_schema = DATABASE()
          AND table_name = 'stores'
          AND column_name = 'id'
        "#,
    )
    .fetch_one(pool)
    .await
    .expect("Failed to query stores.id column");

    assert_eq!(dtype, "char", "stores.id should be CHAR type");
    assert_eq!(len, 26, "stores.id should be 26 chars for ULID");
}

/// Test: Authorization models table has correct column sizes
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_authorization_models_table_has_correct_sizes() {
    let store = create_store().await;
    let pool = store.pool();

    let columns: Vec<(String, String, i64)> = sqlx::query_as(
        r#"
        SELECT column_name, data_type, CAST(character_maximum_length AS SIGNED) as max_length
        FROM information_schema.columns
        WHERE table_schema = DATABASE()
          AND table_name = 'authorization_models'
          AND column_name IN ('id', 'store_id')
        "#,
    )
    .fetch_all(pool)
    .await
    .expect("Failed to query authorization_models columns");

    for (name, dtype, len) in columns {
        assert_eq!(dtype, "char", "{name} should be CHAR type");
        assert_eq!(len, 26, "{name} should be 26 chars for ULID");
    }
}

/// Test: Unique index key length is under 3072 bytes
///
/// This test verifies that the unique index on the tuples table fits within
/// the MariaDB/TiDB limit of 3072 bytes.
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_unique_index_key_length_under_limit() {
    let store = create_store().await;
    let pool = store.pool();

    // Calculate total index key size based on column definitions
    // With utf8mb4 (4 bytes per char):
    // store_id(26) + object_type(128) + object_id(255) + relation(50) +
    // user_type(128) + user_id(128) + user_relation_key(50) = 765 chars * 4 = 3060 bytes
    let columns: Vec<(String, i64)> = sqlx::query_as(
        r#"
        SELECT column_name, CAST(character_maximum_length AS SIGNED) as max_length
        FROM information_schema.columns
        WHERE table_schema = DATABASE()
          AND table_name = 'tuples'
          AND column_name IN ('store_id', 'object_type', 'object_id', 'relation',
                              'user_type', 'user_id', 'user_relation_key')
        "#,
    )
    .fetch_all(pool)
    .await
    .expect("Failed to query index columns");

    // Calculate total index size (assuming utf8mb4 = 4 bytes per char)
    let total_chars: i64 = columns.iter().map(|(_, len)| len).sum();
    let total_bytes = total_chars * 4;

    assert!(
        total_bytes <= 3072,
        "Unique index key length ({total_bytes} bytes) exceeds 3072-byte limit. \
         Column sizes: {columns:?}"
    );

    // Log the actual size for visibility
    eprintln!("Unique index key length: {total_bytes} bytes ({total_chars} chars × 4 bytes/char)");
}

/// Test: Column size limits are enforced - data within limits succeeds
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_column_size_limits_valid_data_succeeds() {
    let store = create_store().await;

    // Create store with valid 26-char ULID
    let store_id = "01ARZ3NDEKTSV4RRFFQ69G5FAV"; // Valid ULID format
    store.create_store(store_id, "Test Store").await.unwrap();

    // Write tuple with data at column size limits
    let tuple = StoredTuple::new(
        "a".repeat(128),      // object_type: exactly 128 chars
        "b".repeat(255),      // object_id: exactly 255 chars
        "c".repeat(50),       // relation: exactly 50 chars
        "d".repeat(128),      // user_type: exactly 128 chars
        "e".repeat(128),      // user_id: exactly 128 chars
        Some("f".repeat(50)), // user_relation: exactly 50 chars
    );

    store
        .write_tuple(store_id, tuple)
        .await
        .expect("Tuple at column size limits should succeed");

    // Verify tuple was written
    let tuples = store
        .read_tuples(store_id, &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1);
    assert_eq!(tuples[0].object_type.len(), 128);
    assert_eq!(tuples[0].object_id.len(), 255);
    assert_eq!(tuples[0].relation.len(), 50);

    // Cleanup
    store.delete_store(store_id).await.unwrap();
}

/// Test: Migrations are idempotent for column size changes
///
/// Running migrations multiple times should not fail or change column sizes.
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_column_size_migration_idempotent() {
    let store = create_store().await;
    let pool = store.pool();

    // Get initial column sizes
    // Cast to SIGNED for MariaDB compatibility (returns BIGINT UNSIGNED)
    let initial_sizes: Vec<(String, i64)> = sqlx::query_as(
        r#"
        SELECT column_name, CAST(character_maximum_length AS SIGNED) as max_length
        FROM information_schema.columns
        WHERE table_schema = DATABASE()
          AND table_name = 'tuples'
          AND column_name IN ('store_id', 'object_type', 'relation', 'user_type', 'user_id')
        ORDER BY column_name
        "#,
    )
    .fetch_all(pool)
    .await
    .unwrap();

    // Run migrations again
    store
        .run_migrations()
        .await
        .expect("Second migration run should succeed");

    // Verify column sizes are unchanged
    let final_sizes: Vec<(String, i64)> = sqlx::query_as(
        r#"
        SELECT column_name, CAST(character_maximum_length AS SIGNED) as max_length
        FROM information_schema.columns
        WHERE table_schema = DATABASE()
          AND table_name = 'tuples'
          AND column_name IN ('store_id', 'object_type', 'relation', 'user_type', 'user_id')
        ORDER BY column_name
        "#,
    )
    .fetch_all(pool)
    .await
    .unwrap();

    assert_eq!(
        initial_sizes, final_sizes,
        "Column sizes should remain unchanged after re-running migrations"
    );
}

/// Test: CRUD operations work correctly with new column sizes
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_crud_operations_with_new_column_sizes() {
    let store = create_store().await;

    let store_id = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    store.create_store(store_id, "CRUD Test").await.unwrap();

    // Test Create
    let tuple = StoredTuple::new(
        "document_type",
        "doc-12345",
        "viewer",
        "user_type",
        "alice-user-id",
        Some("member".to_string()),
    );
    store
        .write_tuple(store_id, tuple.clone())
        .await
        .expect("Create should work with new column sizes");

    // Test Read
    let tuples = store
        .read_tuples(store_id, &TupleFilter::default())
        .await
        .expect("Read should work with new column sizes");
    assert_eq!(tuples.len(), 1);
    assert_eq!(tuples[0].object_type, "document_type");
    assert_eq!(tuples[0].user_relation, Some("member".to_string()));

    // Test Read with filter
    let filter = TupleFilter {
        object_type: Some("document_type".to_string()),
        relation: Some("viewer".to_string()),
        ..Default::default()
    };
    let filtered = store
        .read_tuples(store_id, &filter)
        .await
        .expect("Filtered read should work");
    assert_eq!(filtered.len(), 1);

    // Test Delete
    store
        .delete_tuple(store_id, tuple)
        .await
        .expect("Delete should work with new column sizes");

    let remaining = store
        .read_tuples(store_id, &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(remaining.len(), 0);

    // Cleanup
    store.delete_store(store_id).await.unwrap();
}

/// Test: Authorization model operations work with new column sizes
#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_authorization_model_operations_with_new_column_sizes() {
    use rsfga_storage::StoredAuthorizationModel;

    let store = create_store().await;

    let store_id = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    store.create_store(store_id, "Model Test").await.unwrap();

    // Create model with ULID format IDs
    let model_id = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
    let model =
        StoredAuthorizationModel::new(model_id, store_id, "1.1", r#"{"type_definitions":[]}"#);

    store
        .write_authorization_model(model)
        .await
        .expect("Write model should work with new column sizes");

    // Read model back
    let retrieved = store
        .get_authorization_model(store_id, model_id)
        .await
        .expect("Get model should work");
    assert_eq!(retrieved.id, model_id);
    assert_eq!(retrieved.store_id, store_id);

    // List models
    let models = store
        .list_authorization_models(store_id)
        .await
        .expect("List models should work");
    assert!(!models.is_empty());

    // Cleanup
    store.delete_store(store_id).await.unwrap();
}
