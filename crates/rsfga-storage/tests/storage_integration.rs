//! Storage Integration Tests.
//!
//! These tests verify that InMemory and PostgreSQL storage implementations
//! behave consistently and can be swapped at runtime.
//!
//! Tests marked with `#[ignore]` require a running PostgreSQL database.
//! Run with: cargo test -p rsfga-storage --test storage_integration -- --ignored

use rsfga_storage::{
    DataStore, MemoryDataStore, PostgresConfig, PostgresDataStore, StoredTuple, TupleFilter,
};
use std::sync::Arc;

/// Get database URL from environment, or use default for local testing.
fn get_database_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:test@localhost:5432/postgres".to_string())
}

/// Create an in-memory store for testing.
fn create_memory_store() -> MemoryDataStore {
    MemoryDataStore::new()
}

/// Create a PostgreSQL store for testing.
async fn create_postgres_store() -> PostgresDataStore {
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
    let tuple = StoredTuple {
        object_type: "document".to_string(),
        object_id: "doc1".to_string(),
        relation: "viewer".to_string(),
        user_type: "user".to_string(),
        user_id: "alice".to_string(),
        user_relation: None,
    };
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
        StoredTuple {
            object_type: "document".to_string(),
            object_id: "doc1".to_string(),
            relation: "viewer".to_string(),
            user_type: "user".to_string(),
            user_id: "alice".to_string(),
            user_relation: None,
        },
        StoredTuple {
            object_type: "document".to_string(),
            object_id: "doc1".to_string(),
            relation: "editor".to_string(),
            user_type: "user".to_string(),
            user_id: "bob".to_string(),
            user_relation: None,
        },
        StoredTuple {
            object_type: "folder".to_string(),
            object_id: "folder1".to_string(),
            relation: "owner".to_string(),
            user_type: "user".to_string(),
            user_id: "alice".to_string(),
            user_relation: None,
        },
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
// Test: Same behavior across InMemory and PostgreSQL stores
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

// ============================================================================
// Test: Can swap storage implementations
// ============================================================================

/// Demonstrates that DataStore trait enables runtime swapping of implementations.
#[tokio::test]
async fn test_can_swap_storage_implementations() {
    // Helper that works with any DataStore
    async fn use_store(store: Arc<dyn DataStore>, store_id: &str) -> usize {
        store.create_store(store_id, "Swap Test").await.unwrap();

        let tuple = StoredTuple {
            object_type: "doc".to_string(),
            object_id: "1".to_string(),
            relation: "viewer".to_string(),
            user_type: "user".to_string(),
            user_id: "test".to_string(),
            user_relation: None,
        };
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

        let tuple = StoredTuple {
            object_type: "doc".to_string(),
            object_id: "1".to_string(),
            relation: "viewer".to_string(),
            user_type: "user".to_string(),
            user_id: "test".to_string(),
            user_relation: None,
        };
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
            object_id: format!("doc{}", i),
            relation: "viewer".to_string(),
            user_type: "user".to_string(),
            user_id: format!("user{}", i % 100), // 100 unique users
            user_relation: None,
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
            object_id: format!("doc{}", i),
            relation: "viewer".to_string(),
            user_type: "user".to_string(),
            user_id: format!("user{}", i % 100), // 100 unique users
            user_relation: None,
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

// ============================================================================
// Test: Storage survives application restart (PostgreSQL)
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

        let tuple = StoredTuple {
            object_type: "document".to_string(),
            object_id: "persistent-doc".to_string(),
            relation: "viewer".to_string(),
            user_type: "user".to_string(),
            user_id: "persistent-user".to_string(),
            user_relation: None,
        };
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
                    object_id: format!("doc-{}-{}", i, j),
                    relation: "viewer".to_string(),
                    user_type: "user".to_string(),
                    user_id: format!("user-{}", i),
                    user_relation: None,
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
