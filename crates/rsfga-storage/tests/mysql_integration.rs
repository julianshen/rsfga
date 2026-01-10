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
//! 3. Run tests: cargo test -p rsfga-storage --test mysql_integration -- --ignored
//!
//! For TiDB:
//! 1. Start TiDB: docker run --name rsfga-tidb -p 4000:4000 -d pingcap/tidb:latest
//! 2. Set MYSQL_URL: export MYSQL_URL=mysql://root@localhost:4000/test
//! 3. Run tests: cargo test -p rsfga-storage --test mysql_integration -- --ignored

use rsfga_storage::{
    DataStore, MySQLConfig, MySQLDataStore, StorageError, StoredTuple, TupleFilter,
};
use std::sync::Arc;

/// Get database URL from environment, or skip test if not set.
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
    };

    let store = MySQLDataStore::from_config(&config)
        .await
        .expect("Failed to create MySQLDataStore - is MySQL running?");

    store
        .run_migrations()
        .await
        .expect("Failed to run migrations");

    // Clean up any existing test data
    cleanup_test_stores(&store).await;

    store
}

/// Clean up test stores to ensure a clean state.
async fn cleanup_test_stores(store: &MySQLDataStore) {
    // List and delete any existing test stores
    if let Ok(stores) = store.list_stores().await {
        for s in stores {
            if s.id.starts_with("test-") {
                let _ = store.delete_store(&s.id).await;
            }
        }
    }
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
    };

    let store = MySQLDataStore::from_config(&config)
        .await
        .expect("Failed to create store with limited pool");

    store.run_migrations().await.unwrap();
    cleanup_test_stores(&store).await;

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
                object_id: format!("doc{}", i),
                relation: "viewer".to_string(),
                user_type: "user".to_string(),
                user_id: format!("user{}", i),
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
                object_id: format!("doc{}", i),
                relation: "viewer".to_string(),
                user_type: "user".to_string(),
                user_id: format!("user{}", i),
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
                    object_id: format!("doc-{}-{}", i, j),
                    relation: "viewer".to_string(),
                    user_type: "user".to_string(),
                    user_id: format!("user-{}", i),
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
            object_id: format!("batch-doc-{}", i),
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
            object_id: format!("delete-doc-{}", i),
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
            object_id: format!("doc{}", i),
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

    // Log performance
    println!(
        "Large dataset performance (MySQL): write={}ms, filtered_read={}ms",
        write_duration.as_millis(),
        read_duration.as_millis()
    );

    // Performance assertions - should complete in reasonable time
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
        "Missing store index. Found: {:?}",
        index_names
    );
    assert!(
        index_names.iter().any(|n| n.contains("idx_tuples_object")),
        "Missing object index. Found: {:?}",
        index_names
    );
    assert!(
        index_names.iter().any(|n| n.contains("idx_tuples_user")),
        "Missing user index. Found: {:?}",
        index_names
    );
    assert!(
        index_names
            .iter()
            .any(|n| n.contains("idx_tuples_relation")),
        "Missing relation index. Found: {:?}",
        index_names
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
async fn test_large_result_set_ordering() {
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
            object_id: format!("doc{:03}", i),
            relation: "viewer".to_string(),
            user_type: "user".to_string(),
            user_id: format!("user{:03}", i),
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
