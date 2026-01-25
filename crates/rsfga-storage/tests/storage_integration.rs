//! Storage Integration Tests.
//!
//! These tests verify that InMemory, PostgreSQL, and MySQL storage implementations
//! behave consistently and can be swapped at runtime.
//!
//! Tests marked with `#[ignore]` require a running database.
//! Run with:
//!   cargo test -p rsfga-storage --test storage_integration -- --ignored
//!
//! For PostgreSQL tests: export DATABASE_URL=postgres://postgres:test@localhost:5432/postgres
//! For MySQL tests: export MYSQL_URL=mysql://root:test@localhost:3306/rsfga

use rsfga_storage::{
    DataStore, MemoryDataStore, MySQLConfig, MySQLDataStore, PaginationOptions, PostgresConfig,
    PostgresDataStore, StoredTuple, TupleFilter,
};
use std::sync::Arc;

/// Get PostgreSQL database URL from environment, or use default for local testing.
fn get_postgres_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:test@localhost:5432/postgres".to_string())
}

/// Get MySQL database URL from environment, or use default for local testing.
fn get_mysql_url() -> String {
    std::env::var("MYSQL_URL")
        .unwrap_or_else(|_| "mysql://root:test@localhost:3306/rsfga".to_string())
}

/// Create an in-memory store for testing.
fn create_memory_store() -> MemoryDataStore {
    MemoryDataStore::new()
}

/// Create a PostgreSQL store for testing.
async fn create_postgres_store() -> PostgresDataStore {
    let database_url = get_postgres_url();

    let config = PostgresConfig {
        database_url,
        max_connections: 5,
        min_connections: 1,
        connect_timeout_secs: 30,
        ..Default::default()
    };

    let store = PostgresDataStore::from_config(&config)
        .await
        .expect("Failed to create PostgresDataStore - is PostgreSQL running?");

    store
        .run_migrations()
        .await
        .expect("Failed to run migrations");

    // Clean up any existing test data
    if let Ok(stores) = store.list_stores().await {
        for s in stores {
            if s.id.starts_with("integration-") {
                let _ = store.delete_store(&s.id).await;
            }
        }
    }

    store
}

/// Create a MySQL store for testing.
async fn create_mysql_store() -> MySQLDataStore {
    let database_url = get_mysql_url();

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

    // Clean up any existing test data
    if let Ok(stores) = store.list_stores().await {
        for s in stores {
            if s.id.starts_with("integration-") {
                let _ = store.delete_store(&s.id).await;
            }
        }
    }

    store
}

/// Helper function to run a test against any DataStore implementation.
async fn run_basic_crud_test<S: DataStore>(store: &S, store_id: &str) {
    // Create store
    store
        .create_store(store_id, "Integration Test Store")
        .await
        .unwrap();

    // Verify store exists
    let s = store.get_store(store_id).await.unwrap();
    assert_eq!(s.id, store_id);

    // Write a tuple
    let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);
    store.write_tuple(store_id, tuple.clone()).await.unwrap();

    // Read tuples
    let tuples = store
        .read_tuples(store_id, &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1);
    assert_eq!(tuples[0].object_id, "doc1");
    assert_eq!(tuples[0].user_id, "alice");

    // Delete tuple
    store.delete_tuple(store_id, tuple).await.unwrap();
    let tuples = store
        .read_tuples(store_id, &TupleFilter::default())
        .await
        .unwrap();
    assert!(tuples.is_empty());

    // Clean up
    store.delete_store(store_id).await.unwrap();
}

/// Helper function to run filter tests against any DataStore implementation.
async fn run_filter_test<S: DataStore>(store: &S, store_id: &str) {
    store
        .create_store(store_id, "Filter Test Store")
        .await
        .unwrap();

    // Write multiple tuples
    let tuples = vec![
        StoredTuple::new("document", "doc1", "viewer", "user", "alice", None),
        StoredTuple::new("document", "doc1", "editor", "user", "bob", None),
        StoredTuple::new("folder", "folder1", "owner", "user", "alice", None),
    ];

    store.write_tuples(store_id, tuples, vec![]).await.unwrap();

    // Filter by object type
    let filter = TupleFilter {
        object_type: Some("document".to_string()),
        ..Default::default()
    };
    let results = store.read_tuples(store_id, &filter).await.unwrap();
    assert_eq!(results.len(), 2);

    // Filter by relation
    let filter = TupleFilter {
        relation: Some("viewer".to_string()),
        ..Default::default()
    };
    let results = store.read_tuples(store_id, &filter).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].user_id, "alice");

    // Filter by user
    let filter = TupleFilter {
        user: Some("user:alice".to_string()),
        ..Default::default()
    };
    let results = store.read_tuples(store_id, &filter).await.unwrap();
    assert_eq!(results.len(), 2);

    // Clean up
    store.delete_store(store_id).await.unwrap();
}

// ============================================================================
// Test: Same behavior across InMemory, PostgreSQL, and MySQL stores
// ============================================================================

#[tokio::test]
async fn test_same_behavior_memory_basic_crud() {
    let store = create_memory_store();
    run_basic_crud_test(&store, "integration-memory-crud").await;
}

#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_same_behavior_postgres_basic_crud() {
    let store = create_postgres_store().await;
    run_basic_crud_test(&store, "integration-postgres-crud").await;
}

#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_same_behavior_mysql_basic_crud() {
    let store = create_mysql_store().await;
    run_basic_crud_test(&store, "integration-mysql-crud").await;
}

#[tokio::test]
async fn test_same_behavior_memory_filters() {
    let store = create_memory_store();
    run_filter_test(&store, "integration-memory-filters").await;
}

#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_same_behavior_postgres_filters() {
    let store = create_postgres_store().await;
    run_filter_test(&store, "integration-postgres-filters").await;
}

#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_same_behavior_mysql_filters() {
    let store = create_mysql_store().await;
    run_filter_test(&store, "integration-mysql-filters").await;
}

// ============================================================================
// Test: Can swap storage implementations
// ============================================================================

/// Demonstrates that DataStore trait enables runtime swapping of implementations.
#[tokio::test]
async fn test_can_swap_storage_implementations() {
    // Helper that works with any DataStore
    async fn use_store(store: Arc<dyn DataStore>, store_id: &str) -> usize {
        store.create_store(store_id, "Swap Test").await.unwrap();

        let tuple = StoredTuple::new("doc", "1", "viewer", "user", "test", None);
        store.write_tuple(store_id, tuple).await.unwrap();

        let tuples = store
            .read_tuples(store_id, &TupleFilter::default())
            .await
            .unwrap();

        store.delete_store(store_id).await.unwrap();

        tuples.len()
    }

    // Use with in-memory implementation
    let memory_store: Arc<dyn DataStore> = Arc::new(create_memory_store());
    let count = use_store(memory_store, "integration-swap-memory").await;
    assert_eq!(count, 1);

    // The same function can be used with PostgreSQL (when available)
    // This proves the implementations are interchangeable at runtime
}

#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_can_swap_to_postgres_implementation() {
    async fn use_store(store: Arc<dyn DataStore>, store_id: &str) -> usize {
        store.create_store(store_id, "Swap Test").await.unwrap();

        let tuple = StoredTuple::new("doc", "1", "viewer", "user", "test", None);
        store.write_tuple(store_id, tuple).await.unwrap();

        let tuples = store
            .read_tuples(store_id, &TupleFilter::default())
            .await
            .unwrap();

        store.delete_store(store_id).await.unwrap();

        tuples.len()
    }

    // Use with PostgreSQL implementation
    let postgres_store: Arc<dyn DataStore> = Arc::new(create_postgres_store().await);
    let count = use_store(postgres_store, "integration-swap-postgres").await;
    assert_eq!(count, 1);
}

#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_can_swap_to_mysql_implementation() {
    async fn use_store(store: Arc<dyn DataStore>, store_id: &str) -> usize {
        store.create_store(store_id, "Swap Test").await.unwrap();

        let tuple = StoredTuple::new("doc", "1", "viewer", "user", "test", None);
        store.write_tuple(store_id, tuple).await.unwrap();

        let tuples = store
            .read_tuples(store_id, &TupleFilter::default())
            .await
            .unwrap();

        store.delete_store(store_id).await.unwrap();

        tuples.len()
    }

    // Use with MySQL implementation
    let mysql_store: Arc<dyn DataStore> = Arc::new(create_mysql_store().await);
    let count = use_store(mysql_store, "integration-swap-mysql").await;
    assert_eq!(count, 1);
}

// ============================================================================
// Test: Large dataset performance (10k+ tuples)
// ============================================================================

#[tokio::test]
async fn test_large_dataset_performance_memory() {
    let store = create_memory_store();
    store
        .create_store("integration-large-memory", "Large Dataset Test")
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
            created_at: None,
        });
    }

    // Measure write time
    let start = std::time::Instant::now();
    store
        .write_tuples("integration-large-memory", tuples, vec![])
        .await
        .unwrap();
    let write_duration = start.elapsed();

    // Verify count
    let all_tuples = store
        .read_tuples("integration-large-memory", &TupleFilter::default())
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
        .read_tuples("integration-large-memory", &filter)
        .await
        .unwrap();
    let read_duration = start.elapsed();

    // Each user should have ~100 tuples (10000 / 100 users)
    assert_eq!(filtered.len(), 100);

    // Log performance (for informational purposes)
    println!(
        "Large dataset performance (Memory): write={}ms, filtered_read={}ms",
        write_duration.as_millis(),
        read_duration.as_millis()
    );

    // Performance assertions (should be fast for in-memory)
    assert!(
        write_duration.as_millis() < 5000,
        "Write took too long: {}ms",
        write_duration.as_millis()
    );
    assert!(
        read_duration.as_millis() < 100,
        "Read took too long: {}ms",
        read_duration.as_millis()
    );

    // Clean up
    store
        .delete_store("integration-large-memory")
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_large_dataset_performance_postgres() {
    let store = create_postgres_store().await;
    store
        .create_store("integration-large-postgres", "Large Dataset Test")
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
            created_at: None,
        });
    }

    // Measure write time
    let start = std::time::Instant::now();
    store
        .write_tuples("integration-large-postgres", tuples, vec![])
        .await
        .unwrap();
    let write_duration = start.elapsed();

    // Verify count
    let all_tuples = store
        .read_tuples("integration-large-postgres", &TupleFilter::default())
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
        .read_tuples("integration-large-postgres", &filter)
        .await
        .unwrap();
    let read_duration = start.elapsed();

    // Each user should have ~100 tuples
    assert_eq!(filtered.len(), 100);

    // Log performance
    println!(
        "Large dataset performance (PostgreSQL): write={}ms, filtered_read={}ms",
        write_duration.as_millis(),
        read_duration.as_millis()
    );

    // PostgreSQL may be slower, but should still complete in reasonable time
    // Target: <5ms p95 for queries (per plan.md validation criteria)
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

    // Clean up
    store
        .delete_store("integration-large-postgres")
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_large_dataset_performance_mysql() {
    let store = create_mysql_store().await;
    store
        .create_store("integration-large-mysql", "Large Dataset Test")
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
            created_at: None,
        });
    }

    // Measure write time
    let start = std::time::Instant::now();
    store
        .write_tuples("integration-large-mysql", tuples, vec![])
        .await
        .unwrap();
    let write_duration = start.elapsed();

    // Verify count
    let all_tuples = store
        .read_tuples("integration-large-mysql", &TupleFilter::default())
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
        .read_tuples("integration-large-mysql", &filter)
        .await
        .unwrap();
    let read_duration = start.elapsed();

    // Each user should have ~100 tuples
    assert_eq!(filtered.len(), 100);

    // Log performance
    println!(
        "Large dataset performance (MySQL): write={}ms, filtered_read={}ms",
        write_duration.as_millis(),
        read_duration.as_millis()
    );

    // MySQL may be slower, but should still complete in reasonable time
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

    // Clean up
    store.delete_store("integration-large-mysql").await.unwrap();
}

// ============================================================================
// Test: Storage survives application restart (PostgreSQL/MySQL)
// ============================================================================

#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_storage_survives_restart_postgres() {
    let store_id = "integration-restart-test";

    // Phase 1: Write data with first connection
    {
        let store = create_postgres_store().await;

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
        let store = create_postgres_store().await;

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

#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_storage_survives_restart_mysql() {
    let store_id = "int-restart-test-mysql";

    // Phase 1: Write data with first connection
    {
        let store = create_mysql_store().await;

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
        let store = create_mysql_store().await;

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

// ============================================================================
// Additional: Thread safety test
// ============================================================================

#[tokio::test]
async fn test_concurrent_access_across_threads() {
    let store = Arc::new(create_memory_store());
    store
        .create_store("integration-concurrent", "Concurrent Test")
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
                    created_at: None,
                };
                store
                    .write_tuple("integration-concurrent", tuple)
                    .await
                    .unwrap();
            }
        }));
    }

    // Wait for all writes
    for handle in handles {
        handle.await.unwrap();
    }

    // Verify all tuples were written
    let tuples = store
        .read_tuples("integration-concurrent", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1000); // 10 tasks * 100 tuples each

    // Clean up
    store.delete_store("integration-concurrent").await.unwrap();
}

// ============================================================================
// Test: Pagination consistency across implementations
// ============================================================================

/// Helper function to run pagination test against any DataStore implementation.
async fn run_pagination_test<S: DataStore>(store: &S, store_id: &str) {
    store
        .create_store(store_id, "Pagination Test Store")
        .await
        .unwrap();

    // Write 25 tuples
    let tuples: Vec<StoredTuple> = (0..25)
        .map(|i| StoredTuple {
            object_type: "document".to_string(),
            object_id: format!("doc{i}"),
            relation: "viewer".to_string(),
            user_type: "user".to_string(),
            user_id: format!("user{i}"),
            user_relation: None,
            condition_name: None,
            condition_context: None,
            created_at: None,
        })
        .collect();

    store.write_tuples(store_id, tuples, vec![]).await.unwrap();

    // First page of 10
    let pagination = PaginationOptions {
        page_size: Some(10),
        continuation_token: None,
    };
    let result = store
        .read_tuples_paginated(store_id, &TupleFilter::default(), &pagination)
        .await
        .unwrap();
    assert_eq!(result.items.len(), 10);
    assert!(result.continuation_token.is_some());

    // Second page of 10
    let pagination = PaginationOptions {
        page_size: Some(10),
        continuation_token: result.continuation_token,
    };
    let result = store
        .read_tuples_paginated(store_id, &TupleFilter::default(), &pagination)
        .await
        .unwrap();
    assert_eq!(result.items.len(), 10);
    assert!(result.continuation_token.is_some());

    // Third page (only 5 remaining)
    let pagination = PaginationOptions {
        page_size: Some(10),
        continuation_token: result.continuation_token,
    };
    let result = store
        .read_tuples_paginated(store_id, &TupleFilter::default(), &pagination)
        .await
        .unwrap();
    assert_eq!(result.items.len(), 5);
    assert!(result.continuation_token.is_none()); // No more pages

    // Clean up
    store.delete_store(store_id).await.unwrap();
}

#[tokio::test]
async fn test_pagination_memory() {
    let store = create_memory_store();
    run_pagination_test(&store, "int-pagination-mem").await;
}

#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_pagination_postgres() {
    let store = create_postgres_store().await;
    run_pagination_test(&store, "int-pagination-pg").await;
}

#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_pagination_mysql() {
    let store = create_mysql_store().await;
    run_pagination_test(&store, "int-pagination-sql").await;
}

// ============================================================================
// Test: UpdateStore functionality across implementations
// ============================================================================

/// Helper function to run update_store test against any DataStore implementation.
async fn run_update_store_test<S: DataStore>(store: &S, store_id: &str) {
    // Create a store
    let created = store.create_store(store_id, "Original Name").await.unwrap();
    assert_eq!(created.name, "Original Name");
    let original_created_at = created.created_at;

    // Update the store name
    let updated = store.update_store(store_id, "Updated Name").await.unwrap();

    // Verify the update
    assert_eq!(updated.id, store_id);
    assert_eq!(updated.name, "Updated Name");
    assert_eq!(updated.created_at, original_created_at); // created_at should not change
    assert!(updated.updated_at >= created.updated_at); // updated_at should be >= original

    // Verify the update persisted by fetching the store again
    let fetched = store.get_store(store_id).await.unwrap();
    assert_eq!(fetched.name, "Updated Name");
    assert_eq!(fetched.created_at, original_created_at);

    // Clean up
    store.delete_store(store_id).await.unwrap();
}

/// Helper function to test update_store on non-existent store.
async fn run_update_store_not_found_test<S: DataStore>(store: &S) {
    use rsfga_storage::StorageError;

    let result = store
        .update_store("non-existent-store-id", "New Name")
        .await;

    assert!(result.is_err());
    match result.unwrap_err() {
        StorageError::StoreNotFound { store_id } => {
            assert_eq!(store_id, "non-existent-store-id");
        }
        e => panic!("Expected StoreNotFound, got {e:?}"),
    }
}

/// Helper function to test multiple sequential updates.
async fn run_update_store_multiple_test<S: DataStore>(store: &S, store_id: &str) {
    store.create_store(store_id, "Name v1").await.unwrap();

    // Update multiple times
    let v2 = store.update_store(store_id, "Name v2").await.unwrap();
    assert_eq!(v2.name, "Name v2");

    let v3 = store.update_store(store_id, "Name v3").await.unwrap();
    assert_eq!(v3.name, "Name v3");
    assert!(v3.updated_at >= v2.updated_at);

    let v4 = store.update_store(store_id, "Name v4").await.unwrap();
    assert_eq!(v4.name, "Name v4");
    assert!(v4.updated_at >= v3.updated_at);

    // Verify final state
    let final_store = store.get_store(store_id).await.unwrap();
    assert_eq!(final_store.name, "Name v4");

    // Clean up
    store.delete_store(store_id).await.unwrap();
}

#[tokio::test]
async fn test_update_store_memory() {
    let store = create_memory_store();
    run_update_store_test(&store, "integration-update-memory").await;
}

#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_update_store_postgres() {
    let store = create_postgres_store().await;
    run_update_store_test(&store, "integration-update-postgres").await;
}

#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_update_store_mysql() {
    let store = create_mysql_store().await;
    run_update_store_test(&store, "integration-update-mysql").await;
}

#[tokio::test]
async fn test_update_store_not_found_memory() {
    let store = create_memory_store();
    run_update_store_not_found_test(&store).await;
}

#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_update_store_not_found_postgres() {
    let store = create_postgres_store().await;
    run_update_store_not_found_test(&store).await;
}

#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_update_store_not_found_mysql() {
    let store = create_mysql_store().await;
    run_update_store_not_found_test(&store).await;
}

#[tokio::test]
async fn test_update_store_multiple_memory() {
    let store = create_memory_store();
    run_update_store_multiple_test(&store, "int-update-multi-mem").await;
}

#[tokio::test]
#[ignore = "requires running PostgreSQL"]
async fn test_update_store_multiple_postgres() {
    let store = create_postgres_store().await;
    run_update_store_multiple_test(&store, "int-update-multi-pg").await;
}

#[tokio::test]
#[ignore = "requires running MySQL"]
async fn test_update_store_multiple_mysql() {
    let store = create_mysql_store().await;
    run_update_store_multiple_test(&store, "int-update-multi-sql").await;
}

/// Test concurrent updates to the same store (race condition test).
#[tokio::test]
async fn test_update_store_concurrent_memory() {
    let store = Arc::new(create_memory_store());
    let store_id = "int-update-concurrent";

    store.create_store(store_id, "Initial Name").await.unwrap();

    let mut handles = Vec::new();

    // Spawn 10 concurrent update tasks
    for i in 0..10 {
        let store = Arc::clone(&store);
        let store_id = store_id.to_string();
        handles.push(tokio::spawn(async move {
            store
                .update_store(&store_id, &format!("Name from task {i}"))
                .await
        }));
    }

    // All updates should succeed (no panics or errors)
    for handle in handles {
        let result = handle.await.unwrap();
        assert!(result.is_ok(), "Concurrent update failed: {result:?}");
    }

    // Verify the store has some valid name (one of the updates won)
    let final_store = store.get_store(store_id).await.unwrap();
    assert!(final_store.name.starts_with("Name from task "));

    // Clean up
    store.delete_store(store_id).await.unwrap();
}

/// Test that invalid store_id is rejected.
#[tokio::test]
async fn test_update_store_validates_store_id() {
    use rsfga_storage::StorageError;

    let store = create_memory_store();

    // Empty store_id should fail validation
    let result = store.update_store("", "New Name").await;
    assert!(result.is_err());
    match result.unwrap_err() {
        StorageError::InvalidInput { message } => {
            assert!(
                message.contains("store_id"),
                "Error message should mention store_id: {message}"
            );
        }
        e => panic!("Expected InvalidInput, got {e:?}"),
    }

    // Store id with only whitespace should fail
    let result = store.update_store("   ", "New Name").await;
    assert!(result.is_err());
}

/// Test that invalid store name is rejected.
#[tokio::test]
async fn test_update_store_validates_name() {
    use rsfga_storage::StorageError;

    let store = create_memory_store();
    let store_id = "integration-validate-name";

    store.create_store(store_id, "Valid Name").await.unwrap();

    // Empty name should fail validation
    let result = store.update_store(store_id, "").await;
    assert!(result.is_err());
    match result.unwrap_err() {
        StorageError::InvalidInput { message } => {
            assert!(
                message.contains("name"),
                "Error message should mention name: {message}"
            );
        }
        e => panic!("Expected InvalidInput, got {e:?}"),
    }

    // Clean up
    store.delete_store(store_id).await.unwrap();
}

/// Test that store_id exceeding max length (26 chars for ULID) is rejected.
#[tokio::test]
async fn test_update_store_rejects_long_store_id() {
    use rsfga_storage::StorageError;

    let store = create_memory_store();
    let long_store_id = "x".repeat(27); // Exceeds 26 char ULID limit

    let result = store.update_store(&long_store_id, "Valid Name").await;
    assert!(result.is_err());
    match result.unwrap_err() {
        StorageError::InvalidInput { message } => {
            assert!(
                message.contains("store_id") && message.contains("maximum length"),
                "Error message should mention store_id and maximum length: {message}"
            );
        }
        e => panic!("Expected InvalidInput, got {e:?}"),
    }
}

/// Test that store name exceeding max length (256 chars) is rejected.
#[tokio::test]
async fn test_update_store_rejects_long_name() {
    use rsfga_storage::StorageError;

    let store = create_memory_store();
    let store_id = "integration-long-name-test";
    let long_name = "x".repeat(257); // Exceeds 256 char limit

    store.create_store(store_id, "Valid Name").await.unwrap();

    let result = store.update_store(store_id, &long_name).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        StorageError::InvalidInput { message } => {
            assert!(
                message.contains("name") && message.contains("maximum length"),
                "Error message should mention name and maximum length: {message}"
            );
        }
        e => panic!("Expected InvalidInput, got {e:?}"),
    }

    // Clean up
    store.delete_store(store_id).await.unwrap();
}

/// Test that exactly 256 character values are accepted (boundary test).
#[tokio::test]
async fn test_update_store_accepts_max_length_values() {
    let store = create_memory_store();
    let store_id = "integration-max-len-test";
    let max_length_name = "x".repeat(256); // Exactly 256 chars - should work

    store.create_store(store_id, "Initial Name").await.unwrap();

    // Update with max length name should succeed
    let result = store.update_store(store_id, &max_length_name).await;
    assert!(result.is_ok(), "256 char name should be accepted");

    let updated = result.unwrap();
    assert_eq!(updated.name.len(), 256);

    // Clean up
    store.delete_store(store_id).await.unwrap();
}
