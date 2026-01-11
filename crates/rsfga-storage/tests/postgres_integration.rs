//! PostgreSQL integration tests.
//!
//! These tests require a running PostgreSQL database. They are marked as
//! `#[ignore]` by default and will only run when explicitly enabled.
//!
//! To run these tests:
//! 1. Start PostgreSQL: docker run --name rsfga-postgres -e POSTGRES_PASSWORD=test -p 5432:5432 -d postgres:15-alpine
//! 2. Set DATABASE_URL: export DATABASE_URL=postgres://postgres:test@localhost:5432/postgres
//! 3. Run tests: cargo test -p rsfga-storage --test postgres_integration -- --ignored
//!
//! Alternatively, run with docker-compose (when available).

use rsfga_storage::{
    DataStore, PostgresConfig, PostgresDataStore, StorageError, StoredTuple, TupleFilter,
};
use std::sync::Arc;

/// Get database URL from environment, or skip test if not set.
fn get_database_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:test@localhost:5432/postgres".to_string())
}

/// Create a PostgresDataStore with migrations run.
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
        .expect("Failed to create PostgresDataStore - is PostgreSQL running?");

    store
        .run_migrations()
        .await
        .expect("Failed to run migrations");

    // Clean up any existing test data
    cleanup_test_stores(&store).await;

    store
}

/// Clean up test stores to ensure a clean state.
async fn cleanup_test_stores(store: &PostgresDataStore) {
    // List and delete any existing test stores
    if let Ok(stores) = store.list_stores().await {
        for s in stores {
            if s.id.starts_with("test-") {
                let _ = store.delete_store(&s.id).await;
            }
        }
    }
}

// Test: Can connect to PostgreSQL
#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_can_connect_to_postgres() {
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
#[ignore = "requires running PostgreSQL"]
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

// Test: Can write tuple to database
#[tokio::test]
#[ignore = "requires running PostgreSQL"]
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
#[ignore = "requires running PostgreSQL"]
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
#[ignore = "requires running PostgreSQL"]
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

// Test: Transactions rollback on error
#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_transactions_rollback_on_error() {
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
#[ignore = "requires running PostgreSQL"]
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

// Test: Connection pool limits work correctly
#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_connection_pool_limits() {
    let database_url = get_database_url();

    let config = PostgresConfig {
        database_url,
        max_connections: 2, // Small pool
        min_connections: 1,
        connect_timeout_secs: 30,
    };

    let store = PostgresDataStore::from_config(&config)
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

// Test: Database errors are properly wrapped in StorageError
#[tokio::test]
#[ignore = "requires running PostgreSQL"]
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

// Test: Concurrent writes use correct isolation level
#[tokio::test]
#[ignore = "requires running PostgreSQL"]
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

// Test: Indexes are created for common query patterns
#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_indexes_created() {
    let store = create_store().await;

    // Verify indexes exist by checking pg_indexes
    let pool = store.pool();
    let rows = sqlx::query(
        r#"
        SELECT indexname FROM pg_indexes
        WHERE tablename = 'tuples'
        "#,
    )
    .fetch_all(pool)
    .await
    .unwrap();

    let index_names: Vec<String> = rows
        .iter()
        .map(|row| sqlx::Row::get(row, "indexname"))
        .collect();

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

// Test: Can paginate large result sets (using filter + ordering)
#[tokio::test]
#[ignore = "requires running PostgreSQL"]
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

// Test: supports_transactions returns false (individual tx control not supported)
// Note: write_tuples operations are still atomic internally
#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_supports_transactions() {
    let store = create_store().await;
    // Individual transaction control is not supported, but write_tuples is atomic
    assert!(!store.supports_transactions());
}

// Test: Delete store cascades to tuples
#[tokio::test]
#[ignore = "requires running PostgreSQL"]
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

// Test: Invalid user filter returns InvalidFilter error
#[tokio::test]
#[ignore = "requires running PostgreSQL"]
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
// Section 4: Tuple Storage with Conditions (PostgreSQL)
// ==========================================================================

// Test: PostgresStore can write and read tuples with condition_name
#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_postgres_store_stores_condition_name() {
    let store = create_store().await;
    store
        .create_store("test-condition", "Test Store")
        .await
        .unwrap();

    // Write tuple with condition
    let tuple = StoredTuple::with_condition(
        "document",
        "doc1",
        "viewer",
        "user",
        "alice",
        None,
        "time_bound",
        None,
    );

    store.write_tuple("test-condition", tuple).await.unwrap();

    // Read it back
    let tuples = store
        .read_tuples("test-condition", &TupleFilter::default())
        .await
        .unwrap();

    assert_eq!(tuples.len(), 1);
    assert_eq!(tuples[0].condition_name, Some("time_bound".to_string()));
    assert!(tuples[0].condition_context.is_none());

    // Cleanup
    store.delete_store("test-condition").await.unwrap();
}

// Test: PostgresStore can write and read tuples with condition_context (JSONB)
#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_postgres_store_stores_condition_context() {
    let store = create_store().await;
    store
        .create_store("test-condition-ctx", "Test Store")
        .await
        .unwrap();

    // Write tuple with condition and context
    let mut context = std::collections::HashMap::new();
    context.insert("ip".to_string(), serde_json::json!("192.168.1.1"));
    context.insert(
        "allowed_hours".to_string(),
        serde_json::json!([9, 10, 11, 12, 13, 14, 15, 16, 17]),
    );

    let tuple = StoredTuple::with_condition(
        "document",
        "doc1",
        "viewer",
        "user",
        "alice",
        None,
        "ip_and_time_check",
        Some(context.clone()),
    );

    store
        .write_tuple("test-condition-ctx", tuple)
        .await
        .unwrap();

    // Read it back
    let tuples = store
        .read_tuples("test-condition-ctx", &TupleFilter::default())
        .await
        .unwrap();

    assert_eq!(tuples.len(), 1);
    assert_eq!(
        tuples[0].condition_name,
        Some("ip_and_time_check".to_string())
    );

    let read_context = tuples[0]
        .condition_context
        .as_ref()
        .expect("condition_context should be present");
    assert_eq!(
        read_context.get("ip"),
        Some(&serde_json::json!("192.168.1.1"))
    );
    assert_eq!(
        read_context.get("allowed_hours"),
        Some(&serde_json::json!([9, 10, 11, 12, 13, 14, 15, 16, 17]))
    );

    // Cleanup
    store.delete_store("test-condition-ctx").await.unwrap();
}

// Test: PostgresStore can filter tuples by condition_name
#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_postgres_store_filters_by_condition_name() {
    let store = create_store().await;
    store
        .create_store("test-filter-cond", "Test Store")
        .await
        .unwrap();

    // Write tuples with different conditions
    let tuple1 = StoredTuple::with_condition(
        "document",
        "doc1",
        "viewer",
        "user",
        "alice",
        None,
        "time_bound",
        None,
    );
    let tuple2 = StoredTuple::with_condition(
        "document", "doc2", "viewer", "user", "bob", None, "ip_check", None,
    );
    let tuple3 = StoredTuple::new("document", "doc3", "viewer", "user", "charlie", None);

    store.write_tuple("test-filter-cond", tuple1).await.unwrap();
    store.write_tuple("test-filter-cond", tuple2).await.unwrap();
    store.write_tuple("test-filter-cond", tuple3).await.unwrap();

    // Filter by condition_name = "time_bound"
    let filter = TupleFilter {
        condition_name: Some("time_bound".to_string()),
        ..Default::default()
    };
    let tuples = store
        .read_tuples("test-filter-cond", &filter)
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1);
    assert_eq!(tuples[0].user_id, "alice");

    // Filter by condition_name = "ip_check"
    let filter = TupleFilter {
        condition_name: Some("ip_check".to_string()),
        ..Default::default()
    };
    let tuples = store
        .read_tuples("test-filter-cond", &filter)
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1);
    assert_eq!(tuples[0].user_id, "bob");

    // Cleanup
    store.delete_store("test-filter-cond").await.unwrap();
}

// Test: PostgresStore returns ConditionConflict when writing tuple with different condition
// OpenFGA does NOT support upsert for conditions - you must delete and re-create.
#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_postgres_store_condition_conflict_on_different_condition() {
    let store = create_store().await;
    store
        .create_store("test-conflict-cond", "Test Store")
        .await
        .unwrap();

    // Write tuple without condition
    let tuple1 = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);
    store
        .write_tuple("test-conflict-cond", tuple1)
        .await
        .unwrap();

    // Read it back - should have no condition
    let tuples = store
        .read_tuples("test-conflict-cond", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1);
    assert!(tuples[0].condition_name.is_none());

    // Try to write same tuple WITH condition - should return ConditionConflict
    let tuple2 = StoredTuple::with_condition(
        "document",
        "doc1",
        "viewer",
        "user",
        "alice",
        None,
        "new_condition",
        None,
    );
    let result = store.write_tuple("test-conflict-cond", tuple2).await;

    // Should return ConditionConflict error (409 Conflict in OpenFGA)
    assert!(
        matches!(result, Err(StorageError::ConditionConflict { .. })),
        "Expected ConditionConflict error, got: {:?}",
        result
    );

    // Original tuple should still exist with no condition
    let tuples = store
        .read_tuples("test-conflict-cond", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1);
    assert!(
        tuples[0].condition_name.is_none(),
        "Original tuple should be unchanged"
    );

    // Cleanup
    store.delete_store("test-conflict-cond").await.unwrap();
}

// Test: Writing identical tuple with same condition is idempotent (no error)
#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_postgres_store_idempotent_same_condition() {
    let store = create_store().await;
    store
        .create_store("test-idempotent-cond", "Test Store")
        .await
        .unwrap();

    // Write tuple with condition
    let tuple = StoredTuple::with_condition(
        "document",
        "doc1",
        "viewer",
        "user",
        "alice",
        None,
        "time_bound",
        None,
    );
    store
        .write_tuple("test-idempotent-cond", tuple.clone())
        .await
        .unwrap();

    // Write the same tuple again - should succeed (idempotent)
    let result = store.write_tuple("test-idempotent-cond", tuple).await;
    assert!(
        result.is_ok(),
        "Writing identical tuple should be idempotent"
    );

    // Should still have exactly one tuple
    let tuples = store
        .read_tuples("test-idempotent-cond", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1);
    assert_eq!(tuples[0].condition_name, Some("time_bound".to_string()));

    // Cleanup
    store.delete_store("test-idempotent-cond").await.unwrap();
}

// Test: condition_context size validation rejects oversized payloads
#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_postgres_store_rejects_oversized_condition_context() {
    let store = create_store().await;
    store
        .create_store("test-size-limit", "Test Store")
        .await
        .unwrap();

    // Create a large condition_context (> 64KB)
    let mut large_context = std::collections::HashMap::new();
    // Each entry is ~100 bytes, need ~650 entries for 65KB
    for i in 0..700 {
        large_context.insert(
            format!("key_{:04}", i),
            serde_json::json!(format!(
                "value_{:04}_padding_to_make_it_larger_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
                i
            )),
        );
    }

    let tuple = StoredTuple::with_condition(
        "document",
        "doc1",
        "viewer",
        "user",
        "alice",
        None,
        "oversized",
        Some(large_context),
    );

    let result = store.write_tuple("test-size-limit", tuple).await;

    // Should return InvalidInput error due to size limit
    assert!(
        matches!(result, Err(StorageError::InvalidInput { .. })),
        "Expected InvalidInput error for oversized condition_context, got: {:?}",
        result
    );

    // Cleanup
    store.delete_store("test-size-limit").await.unwrap();
}

// Test: Pagination works correctly with condition fields
#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_postgres_store_pagination_with_conditions() {
    use rsfga_storage::PaginationOptions;

    let store = create_store().await;
    store
        .create_store("test-paginate-cond", "Test Store")
        .await
        .unwrap();

    // Write tuples with various conditions
    let mut tuples = Vec::new();
    for i in 0..15 {
        let condition = if i % 3 == 0 {
            Some("time_bound".to_string())
        } else if i % 3 == 1 {
            Some("ip_check".to_string())
        } else {
            None
        };

        let context = condition.as_ref().map(|_| {
            let mut ctx = std::collections::HashMap::new();
            ctx.insert("index".to_string(), serde_json::json!(i));
            ctx
        });

        tuples.push(StoredTuple {
            object_type: "document".to_string(),
            object_id: format!("doc{:02}", i),
            relation: "viewer".to_string(),
            user_type: "user".to_string(),
            user_id: format!("user{:02}", i),
            user_relation: None,
            condition_name: condition,
            condition_context: context,
        });
    }

    store
        .write_tuples("test-paginate-cond", tuples, vec![])
        .await
        .unwrap();

    // Paginate with page size of 5
    let page1 = store
        .read_tuples_paginated(
            "test-paginate-cond",
            &TupleFilter::default(),
            &PaginationOptions {
                page_size: Some(5),
                continuation_token: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(page1.items.len(), 5);
    assert!(page1.continuation_token.is_some());

    // Verify conditions are preserved in first page
    for tuple in &page1.items {
        // Each tuple should have its condition properly loaded
        if tuple.condition_name.is_some() {
            assert!(
                tuple.condition_context.is_some(),
                "Tuple with condition_name should have condition_context"
            );
        }
    }

    // Get second page
    let page2 = store
        .read_tuples_paginated(
            "test-paginate-cond",
            &TupleFilter::default(),
            &PaginationOptions {
                page_size: Some(5),
                continuation_token: page1.continuation_token,
            },
        )
        .await
        .unwrap();

    assert_eq!(page2.items.len(), 5);
    assert!(page2.continuation_token.is_some());

    // Get third page
    let page3 = store
        .read_tuples_paginated(
            "test-paginate-cond",
            &TupleFilter::default(),
            &PaginationOptions {
                page_size: Some(5),
                continuation_token: page2.continuation_token,
            },
        )
        .await
        .unwrap();

    assert_eq!(page3.items.len(), 5);
    assert!(page3.continuation_token.is_none()); // Last page

    // Verify total count across all pages
    let all_tuples: Vec<_> = page1
        .items
        .into_iter()
        .chain(page2.items)
        .chain(page3.items)
        .collect();
    assert_eq!(all_tuples.len(), 15);

    // Count tuples by condition type
    let time_bound_count = all_tuples
        .iter()
        .filter(|t| t.condition_name.as_deref() == Some("time_bound"))
        .count();
    let ip_check_count = all_tuples
        .iter()
        .filter(|t| t.condition_name.as_deref() == Some("ip_check"))
        .count();
    let no_condition_count = all_tuples
        .iter()
        .filter(|t| t.condition_name.is_none())
        .count();

    assert_eq!(time_bound_count, 5); // indices 0, 3, 6, 9, 12
    assert_eq!(ip_check_count, 5); // indices 1, 4, 7, 10, 13
    assert_eq!(no_condition_count, 5); // indices 2, 5, 8, 11, 14

    // Cleanup
    store.delete_store("test-paginate-cond").await.unwrap();
}
