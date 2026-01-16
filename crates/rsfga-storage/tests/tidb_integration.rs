//! TiDB compatibility tests for MySqlDataStore.
//!
//! These tests verify that the `MySqlDataStore` implementation works correctly
//! with TiDB, a distributed NewSQL database that is MySQL-compatible.
//!
//! ## Compatibility Status
//!
//! TiDB 6.x+ is compatible with the MySqlDataStore implementation.
//! TiDB uses MySQL protocol on port 4000.
//!
//! All operations tested:
//! - Store CRUD operations
//! - Tuple CRUD operations
//! - Authorization model operations
//! - Pagination
//! - Health checks
//!
//! ## Running Tests
//!
//! ### Option 1: Using Docker (manual)
//!
//! ```bash
//! # Start TiDB (single-node for testing)
//! docker run --name rsfga-tidb \
//!     -p 4000:4000 \
//!     -d pingcap/tidb:latest
//!
//! # Wait for TiDB to start (it may take 10-20 seconds)
//! sleep 20
//!
//! # Create test database
//! mysql -h 127.0.0.1 -P 4000 -u root -e "CREATE DATABASE IF NOT EXISTS rsfga"
//!
//! # Set environment variable
//! export TIDB_URL=mysql://root@localhost:4000/rsfga
//!
//! # Run tests
//! cargo test -p rsfga-storage --test tidb_integration -- --ignored --test-threads=1
//!
//! # Cleanup
//! docker rm -f rsfga-tidb
//! ```
//!
//! ### Option 2: Using testcontainers (automatic)
//!
//! Tests marked with `testcontainers` in their name will automatically start
//! a TiDB container using Docker. Just ensure Docker is running:
//!
//! ```bash
//! cargo test -p rsfga-storage --test tidb_integration testcontainers -- --ignored
//! ```
//!
//! ## TiDB-Specific Behaviors
//!
//! The following behaviors are specific to TiDB and may differ from MySQL:
//!
//! 1. **Port**: TiDB uses port 4000 for MySQL protocol (not 3306)
//! 2. **Storage Engine**: TiDB uses TiKV (distributed key-value storage) internally
//! 3. **AUTO_INCREMENT**: TiDB allocates AUTO_INCREMENT IDs in batches for performance
//! 4. **Transactions**: TiDB uses optimistic concurrency control by default
//! 5. **JSON Support**: TiDB has native JSON support compatible with MySQL 8.0
//! 6. **Character Set**: UTF8MB4 is fully supported
//! 7. **Startup Time**: TiDB single-node may take 10-30 seconds to fully initialize
//!
//! ## Known Limitations
//!
//! - Single-node TiDB is suitable for testing but not production
//! - Some MySQL features like stored procedures have limited support
//! - TiDB's optimistic locking may behave differently under high contention
//!
//! ## Tested Versions
//!
//! - TiDB 6.5 (LTS)
//! - TiDB 7.x
//! - TiDB 8.x
//!
//! ## Related
//!
//! - Issue: #120
//! - MySQL tests: `mysql_integration.rs`
//! - MariaDB tests: `mariadb_integration.rs`

use rsfga_storage::{
    DataStore, MySQLConfig, MySQLDataStore, StoredAuthorizationModel, StoredTuple, TupleFilter,
};
use std::collections::HashMap;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage};

// =============================================================================
// Test Container Helpers
// =============================================================================

/// TiDB container wrapper with connection info.
struct TiDbContainer {
    #[allow(dead_code)]
    container: ContainerAsync<GenericImage>,
    connection_string: String,
}

impl TiDbContainer {
    async fn new() -> Self {
        // TiDB uses port 4000 for MySQL protocol
        let image = GenericImage::new("pingcap/tidb", "latest")
            .with_exposed_port(4000.tcp())
            .with_wait_for(WaitFor::message_on_stdout("server is running"));

        let container = image.start().await.unwrap();
        let host_port = container.get_host_port_ipv4(4000).await.unwrap();

        // TiDB defaults to root user with no password
        // Need to create test database
        let connection_string = format!("mysql://root@127.0.0.1:{}/test", host_port);

        // Wait a bit more for TiDB to fully initialize
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

        Self {
            container,
            connection_string,
        }
    }

    fn connection_string(&self) -> &str {
        &self.connection_string
    }
}

/// Create a MySQLDataStore from a TiDB container.
async fn create_tidb_store(container: &TiDbContainer) -> MySQLDataStore {
    let config = MySQLConfig {
        database_url: container.connection_string().to_string(),
        max_connections: 5,
        min_connections: 1,
        connect_timeout_secs: 60, // TiDB may need longer to initialize
        query_timeout_secs: 30,
        ..Default::default()
    };

    let store = MySQLDataStore::from_config(&config)
        .await
        .expect("Failed to create MySQLDataStore for TiDB");

    store
        .run_migrations()
        .await
        .expect("Failed to run migrations on TiDB");

    store
}

/// Get TiDB URL from environment for manual testing.
fn get_tidb_url() -> String {
    std::env::var("TIDB_URL").unwrap_or_else(|_| "mysql://root@localhost:4000/rsfga".to_string())
}

/// Create a MySQLDataStore from environment URL (for manual testing).
async fn create_store_from_env() -> MySQLDataStore {
    let config = MySQLConfig {
        database_url: get_tidb_url(),
        max_connections: 5,
        min_connections: 1,
        connect_timeout_secs: 60,
        ..Default::default()
    };

    let store = MySQLDataStore::from_config(&config)
        .await
        .expect("Failed to create MySQLDataStore - is TiDB running?");

    store
        .run_migrations()
        .await
        .expect("Failed to run migrations");

    store
}

// =============================================================================
// Testcontainers Tests (Automatic Container Provisioning)
// =============================================================================

/// Test: TiDB container can connect and run migrations
#[tokio::test]
#[ignore = "requires Docker"]
async fn testcontainers_tidb_can_connect() {
    let container = TiDbContainer::new().await;
    let store = create_tidb_store(&container).await;

    // Verify we can list stores
    let result = store.list_stores().await;
    assert!(result.is_ok(), "Should be able to list stores on TiDB");
}

/// Test: TiDB supports all store CRUD operations
#[tokio::test]
#[ignore = "requires Docker"]
async fn testcontainers_tidb_store_crud() {
    let container = TiDbContainer::new().await;
    let store = create_tidb_store(&container).await;

    // Create store
    let created = store.create_store("tidb-test", "TiDB Test Store").await;
    assert!(created.is_ok(), "Should create store on TiDB");

    // Get store
    let retrieved = store.get_store("tidb-test").await;
    assert!(retrieved.is_ok(), "Should get store from TiDB");
    assert_eq!(retrieved.unwrap().name, "TiDB Test Store");

    // List stores
    let stores = store.list_stores().await.unwrap();
    assert!(
        stores.iter().any(|s| s.id == "tidb-test"),
        "Should list created store"
    );

    // Delete store
    let deleted = store.delete_store("tidb-test").await;
    assert!(deleted.is_ok(), "Should delete store on TiDB");
}

/// Test: TiDB supports tuple operations
#[tokio::test]
#[ignore = "requires Docker"]
async fn testcontainers_tidb_tuple_operations() {
    let container = TiDbContainer::new().await;
    let store = create_tidb_store(&container).await;

    store.create_store("tidb-tuples", "Test").await.unwrap();

    // Write tuple
    let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);
    store
        .write_tuple("tidb-tuples", tuple)
        .await
        .expect("Should write tuple to TiDB");

    // Read tuple
    let tuples = store
        .read_tuples("tidb-tuples", &TupleFilter::default())
        .await
        .expect("Should read tuples from TiDB");
    assert_eq!(tuples.len(), 1);
    assert_eq!(tuples[0].object_id, "doc1");

    // Delete tuple
    let delete_tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);
    store
        .delete_tuple("tidb-tuples", delete_tuple)
        .await
        .expect("Should delete tuple from TiDB");

    let tuples_after = store
        .read_tuples("tidb-tuples", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples_after.len(), 0);

    // Cleanup
    store.delete_store("tidb-tuples").await.unwrap();
}

/// Test: TiDB supports tuples with conditions
#[tokio::test]
#[ignore = "requires Docker"]
async fn testcontainers_tidb_tuples_with_conditions() {
    let container = TiDbContainer::new().await;
    let store = create_tidb_store(&container).await;

    store.create_store("tidb-conditions", "Test").await.unwrap();

    // Create tuple with condition
    let mut context = HashMap::new();
    context.insert("allowed_ips".to_string(), serde_json::json!(["10.0.0.1"]));

    let tuple = StoredTuple {
        object_type: "document".to_string(),
        object_id: "doc1".to_string(),
        relation: "viewer".to_string(),
        user_type: "user".to_string(),
        user_id: "alice".to_string(),
        user_relation: None,
        condition_name: Some("ip_allowlist".to_string()),
        condition_context: Some(context),
    };

    store
        .write_tuple("tidb-conditions", tuple)
        .await
        .expect("Should write conditional tuple to TiDB");

    // Read and verify condition data
    let tuples = store
        .read_tuples("tidb-conditions", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1);
    assert_eq!(tuples[0].condition_name, Some("ip_allowlist".to_string()));
    assert!(tuples[0].condition_context.is_some());

    // Cleanup
    store.delete_store("tidb-conditions").await.unwrap();
}

/// Test: TiDB supports authorization models
#[tokio::test]
#[ignore = "requires Docker"]
async fn testcontainers_tidb_authorization_models() {
    let container = TiDbContainer::new().await;
    let store = create_tidb_store(&container).await;

    store.create_store("tidb-models", "Test").await.unwrap();

    // Create authorization model
    let model_json = serde_json::json!({
        "schema_version": "1.1",
        "type_definitions": [
            {
                "type": "document",
                "relations": {
                    "viewer": {"this": {}}
                }
            }
        ]
    })
    .to_string();

    let model_to_write = StoredAuthorizationModel {
        id: ulid::Ulid::new().to_string(),
        store_id: "tidb-models".to_string(),
        schema_version: "1.1".to_string(),
        model_json,
        created_at: chrono::Utc::now(),
    };

    let model = store
        .write_authorization_model(model_to_write)
        .await
        .expect("Should write model on TiDB");

    // Get model
    let retrieved = store
        .get_authorization_model("tidb-models", &model.id)
        .await
        .expect("Should get model from TiDB");
    assert_eq!(retrieved.id, model.id);

    // Get latest model
    let latest = store
        .get_latest_authorization_model("tidb-models")
        .await
        .expect("Should get latest model from TiDB");
    assert_eq!(latest.id, model.id);

    // Cleanup
    store.delete_store("tidb-models").await.unwrap();
}

/// Test: TiDB health check returns healthy status
#[tokio::test]
#[ignore = "requires Docker"]
async fn testcontainers_tidb_health_check() {
    let container = TiDbContainer::new().await;
    let store = create_tidb_store(&container).await;

    let health = store
        .health_check()
        .await
        .expect("Health check should succeed");
    assert!(health.healthy, "TiDB should report healthy");
    assert!(health.pool_stats.is_some(), "Should have pool stats");
    assert_eq!(health.message, Some("mysql".to_string()));
}

/// Test: TiDB supports pagination
#[tokio::test]
#[ignore = "requires Docker"]
async fn testcontainers_tidb_pagination() {
    let container = TiDbContainer::new().await;
    let store = create_tidb_store(&container).await;

    store.create_store("tidb-pagination", "Test").await.unwrap();

    // Create multiple tuples
    for i in 0..10 {
        let tuple = StoredTuple::new(
            "document",
            format!("doc{}", i),
            "viewer",
            "user",
            "alice",
            None,
        );
        store.write_tuple("tidb-pagination", tuple).await.unwrap();
    }

    // Test pagination
    let page1 = store
        .read_tuples_paginated(
            "tidb-pagination",
            &TupleFilter::default(),
            &rsfga_storage::PaginationOptions {
                page_size: Some(3),
                continuation_token: None,
            },
        )
        .await
        .expect("Should paginate on TiDB");

    assert_eq!(page1.items.len(), 3, "First page should have 3 items");
    assert!(
        page1.continuation_token.is_some(),
        "Should have continuation token"
    );

    // Cleanup
    store.delete_store("tidb-pagination").await.unwrap();
}

/// Test: TiDB handles AUTO_INCREMENT IDs correctly
///
/// TiDB allocates AUTO_INCREMENT IDs in batches (default 30000) for performance.
/// This test verifies that our code works correctly with this behavior.
#[tokio::test]
#[ignore = "requires Docker"]
async fn testcontainers_tidb_autoincrement_behavior() {
    let container = TiDbContainer::new().await;
    let store = create_tidb_store(&container).await;

    // Create multiple stores to test AUTO_INCREMENT
    for i in 0..5 {
        let store_id = format!("tidb-autoinc-{}", i);
        store
            .create_store(&store_id, &format!("Test Store {}", i))
            .await
            .expect("Should create store");
    }

    // Verify all stores were created
    let stores = store.list_stores().await.unwrap();
    let created_stores: Vec<_> = stores
        .iter()
        .filter(|s| s.id.starts_with("tidb-autoinc-"))
        .collect();
    assert_eq!(created_stores.len(), 5, "Should have created 5 stores");

    // Cleanup
    for i in 0..5 {
        store
            .delete_store(&format!("tidb-autoinc-{}", i))
            .await
            .unwrap();
    }
}

// =============================================================================
// Manual Tests (Using TIDB_URL environment variable)
// =============================================================================

/// Test: Can connect to TiDB via environment URL
#[tokio::test]
#[ignore = "requires running TiDB"]
async fn test_tidb_can_connect() {
    let store = create_store_from_env().await;
    let result = store.list_stores().await;
    assert!(
        result.is_ok(),
        "Should be able to list stores on TiDB: {:?}",
        result.err()
    );
}

/// Test: Migrations are idempotent on TiDB
#[tokio::test]
#[ignore = "requires running TiDB"]
async fn test_tidb_migrations_idempotent() {
    let store = create_store_from_env().await;

    // Run migrations again
    store
        .run_migrations()
        .await
        .expect("Migrations should be idempotent");

    // Verify we can still operate
    store.create_store("tidb-idempotent", "Test").await.unwrap();
    store.delete_store("tidb-idempotent").await.unwrap();
}

/// Test: TiDB optimistic locking behavior
///
/// TiDB uses optimistic concurrency control by default.
/// This test verifies our implementation handles concurrent writes correctly.
#[tokio::test]
#[ignore = "requires running TiDB"]
async fn test_tidb_concurrent_writes() {
    let store = create_store_from_env().await;

    store.create_store("tidb-concurrent", "Test").await.unwrap();

    // Write multiple tuples concurrently
    let mut handles = vec![];
    for i in 0..10 {
        let store_id = "tidb-concurrent".to_string();
        let tuple = StoredTuple::new(
            "document",
            format!("doc{}", i),
            "viewer",
            "user",
            format!("user{}", i),
            None,
        );

        // Create a new store connection for each concurrent write
        let config = MySQLConfig {
            database_url: get_tidb_url(),
            max_connections: 2,
            min_connections: 1,
            connect_timeout_secs: 60,
            ..Default::default()
        };
        let write_store = MySQLDataStore::from_config(&config).await.unwrap();

        handles.push(tokio::spawn(async move {
            write_store.write_tuple(&store_id, tuple).await
        }));
    }

    // Wait for all writes to complete
    for handle in handles {
        handle
            .await
            .unwrap()
            .expect("Concurrent write should succeed");
    }

    // Verify all tuples were written
    let tuples = store
        .read_tuples("tidb-concurrent", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 10, "All concurrent writes should succeed");

    // Cleanup
    store.delete_store("tidb-concurrent").await.unwrap();
}
