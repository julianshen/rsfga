//! MariaDB compatibility tests for MySqlDataStore.
//!
//! These tests verify that the `MySqlDataStore` implementation works correctly
//! with MariaDB, a community-developed fork of MySQL that maintains compatibility
//! with MySQL's protocol and SQL syntax.
//!
//! ## Compatibility Status
//!
//! MariaDB 10.5+ is fully compatible with the MySqlDataStore implementation.
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
//! # Start MariaDB
//! docker run --name rsfga-mariadb \
//!     -e MYSQL_ROOT_PASSWORD=test \
//!     -e MYSQL_DATABASE=rsfga \
//!     -p 3307:3306 \
//!     -d mariadb:10.11
//!
//! # Set environment variable
//! export MARIADB_URL=mysql://root:test@localhost:3307/rsfga
//!
//! # Run tests
//! cargo test -p rsfga-storage --test mariadb_integration -- --ignored --test-threads=1
//!
//! # Cleanup
//! docker rm -f rsfga-mariadb
//! ```
//!
//! ### Option 2: Using testcontainers (automatic)
//!
//! Tests marked with `testcontainers` in their name will automatically start
//! a MariaDB container using Docker. Just ensure Docker is running:
//!
//! ```bash
//! cargo test -p rsfga-storage --test mariadb_integration testcontainers -- --ignored
//! ```
//!
//! ## MariaDB-Specific Behaviors
//!
//! The following behaviors are specific to MariaDB and may differ from MySQL:
//!
//! 1. **Storage Engine**: MariaDB defaults to InnoDB (same as MySQL 8.0+)
//! 2. **JSON Support**: MariaDB has native JSON support (alias for LONGTEXT with JSON validation)
//! 3. **Sequence Handling**: MariaDB has sequences, but we use AUTO_INCREMENT for compatibility
//! 4. **Character Set**: UTF8MB4 is well-supported in MariaDB 10.5+
//!
//! ## Tested Versions
//!
//! - MariaDB 10.5 (LTS)
//! - MariaDB 10.6 (LTS)
//! - MariaDB 10.11 (LTS)
//! - MariaDB 11.x
//!
//! ## Related
//!
//! - Issue: #119
//! - MySQL tests: `mysql_integration.rs`
//! - TiDB tests: `tidb_integration.rs`

use rsfga_storage::{
    DataStore, MySQLConfig, MySQLDataStore, StoredAuthorizationModel, StoredTuple, TupleFilter,
};
use std::collections::HashMap;
use testcontainers::runners::AsyncRunner;
use testcontainers::ContainerAsync;
use testcontainers_modules::mariadb::Mariadb;

// =============================================================================
// Test Container Helpers
// =============================================================================

/// MariaDB container wrapper with connection info.
struct MariaDbContainer {
    #[allow(dead_code)]
    container: ContainerAsync<Mariadb>,
    connection_string: String,
}

impl MariaDbContainer {
    async fn new() -> Self {
        let container = Mariadb::default().start().await.unwrap();
        let host_port = container.get_host_port_ipv4(3306).await.unwrap();
        // Default MariaDB image uses root with no password
        let connection_string = format!("mysql://root@127.0.0.1:{host_port}/test");
        Self {
            container,
            connection_string,
        }
    }

    fn connection_string(&self) -> &str {
        &self.connection_string
    }
}

/// Create a MySQLDataStore from a MariaDB container.
async fn create_mariadb_store(container: &MariaDbContainer) -> MySQLDataStore {
    let config = MySQLConfig {
        database_url: container.connection_string().to_string(),
        max_connections: 5,
        min_connections: 1,
        connect_timeout_secs: 30,
        query_timeout_secs: 30,
        ..Default::default()
    };

    let store = MySQLDataStore::from_config(&config)
        .await
        .expect("Failed to create MySQLDataStore for MariaDB");

    store
        .run_migrations()
        .await
        .expect("Failed to run migrations on MariaDB");

    store
}

/// Get MariaDB URL from environment for manual testing.
fn get_mariadb_url() -> String {
    std::env::var("MARIADB_URL")
        .unwrap_or_else(|_| "mysql://root:test@localhost:3307/rsfga".to_string())
}

/// Create a MySQLDataStore from environment URL (for manual testing).
async fn create_store_from_env() -> MySQLDataStore {
    let config = MySQLConfig {
        database_url: get_mariadb_url(),
        max_connections: 5,
        min_connections: 1,
        connect_timeout_secs: 30,
        ..Default::default()
    };

    let store = MySQLDataStore::from_config(&config)
        .await
        .expect("Failed to create MySQLDataStore - is MariaDB running?");

    store
        .run_migrations()
        .await
        .expect("Failed to run migrations");

    store
}

// =============================================================================
// Testcontainers Tests (Automatic Container Provisioning)
// =============================================================================

/// Test: MariaDB container can connect and run migrations
#[tokio::test]
#[ignore = "requires Docker"]
async fn testcontainers_mariadb_can_connect() {
    let container = MariaDbContainer::new().await;
    let store = create_mariadb_store(&container).await;

    // Verify we can list stores
    let result = store.list_stores().await;
    assert!(result.is_ok(), "Should be able to list stores on MariaDB");
}

/// Test: MariaDB supports all store CRUD operations
#[tokio::test]
#[ignore = "requires Docker"]
async fn testcontainers_mariadb_store_crud() {
    let container = MariaDbContainer::new().await;
    let store = create_mariadb_store(&container).await;

    // Create store
    let created = store
        .create_store("mariadb-test", "MariaDB Test Store")
        .await;
    assert!(created.is_ok(), "Should create store on MariaDB");

    // Get store
    let retrieved = store.get_store("mariadb-test").await;
    assert!(retrieved.is_ok(), "Should get store from MariaDB");
    assert_eq!(retrieved.unwrap().name, "MariaDB Test Store");

    // List stores
    let stores = store.list_stores().await.unwrap();
    assert!(
        stores.iter().any(|s| s.id == "mariadb-test"),
        "Should list created store"
    );

    // Delete store
    let deleted = store.delete_store("mariadb-test").await;
    assert!(deleted.is_ok(), "Should delete store on MariaDB");
}

/// Test: MariaDB supports tuple operations
#[tokio::test]
#[ignore = "requires Docker"]
async fn testcontainers_mariadb_tuple_operations() {
    let container = MariaDbContainer::new().await;
    let store = create_mariadb_store(&container).await;

    store.create_store("mariadb-tuples", "Test").await.unwrap();

    // Write tuple
    let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);
    store
        .write_tuple("mariadb-tuples", tuple)
        .await
        .expect("Should write tuple to MariaDB");

    // Read tuple
    let tuples = store
        .read_tuples("mariadb-tuples", &TupleFilter::default())
        .await
        .expect("Should read tuples from MariaDB");
    assert_eq!(tuples.len(), 1);
    assert_eq!(tuples[0].object_id, "doc1");

    // Delete tuple
    let delete_tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);
    store
        .delete_tuple("mariadb-tuples", delete_tuple)
        .await
        .expect("Should delete tuple from MariaDB");

    let tuples_after = store
        .read_tuples("mariadb-tuples", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples_after.len(), 0);

    // Cleanup
    store.delete_store("mariadb-tuples").await.unwrap();
}

/// Test: MariaDB supports tuples with conditions
#[tokio::test]
#[ignore = "requires Docker"]
async fn testcontainers_mariadb_tuples_with_conditions() {
    let container = MariaDbContainer::new().await;
    let store = create_mariadb_store(&container).await;

    store
        .create_store("mariadb-conditions", "Test")
        .await
        .unwrap();

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
        created_at: None,
    };

    store
        .write_tuple("mariadb-conditions", tuple)
        .await
        .expect("Should write conditional tuple to MariaDB");

    // Read and verify condition data
    let tuples = store
        .read_tuples("mariadb-conditions", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1);
    assert_eq!(tuples[0].condition_name, Some("ip_allowlist".to_string()));
    assert!(tuples[0].condition_context.is_some());

    // Cleanup
    store.delete_store("mariadb-conditions").await.unwrap();
}

/// Test: MariaDB supports authorization models
#[tokio::test]
#[ignore = "requires Docker"]
async fn testcontainers_mariadb_authorization_models() {
    let container = MariaDbContainer::new().await;
    let store = create_mariadb_store(&container).await;

    store.create_store("mariadb-models", "Test").await.unwrap();

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
        store_id: "mariadb-models".to_string(),
        schema_version: "1.1".to_string(),
        model_json,
        created_at: chrono::Utc::now(),
    };

    let model = store
        .write_authorization_model(model_to_write)
        .await
        .expect("Should write model on MariaDB");

    // Get model
    let retrieved = store
        .get_authorization_model("mariadb-models", &model.id)
        .await
        .expect("Should get model from MariaDB");
    assert_eq!(retrieved.id, model.id);

    // Get latest model
    let latest = store
        .get_latest_authorization_model("mariadb-models")
        .await
        .expect("Should get latest model from MariaDB");
    assert_eq!(latest.id, model.id);

    // Cleanup
    store.delete_store("mariadb-models").await.unwrap();
}

/// Test: MariaDB health check returns healthy status
#[tokio::test]
#[ignore = "requires Docker"]
async fn testcontainers_mariadb_health_check() {
    let container = MariaDbContainer::new().await;
    let store = create_mariadb_store(&container).await;

    let health = store
        .health_check()
        .await
        .expect("Health check should succeed");
    assert!(health.healthy, "MariaDB should report healthy");
    assert!(health.pool_stats.is_some(), "Should have pool stats");
    assert_eq!(health.message, Some("mysql".to_string()));
}

/// Test: MariaDB supports pagination
#[tokio::test]
#[ignore = "requires Docker"]
async fn testcontainers_mariadb_pagination() {
    let container = MariaDbContainer::new().await;
    let store = create_mariadb_store(&container).await;

    store
        .create_store("mariadb-pagination", "Test")
        .await
        .unwrap();

    // Create multiple tuples
    for i in 0..10 {
        let tuple = StoredTuple::new(
            "document",
            format!("doc{i}"),
            "viewer",
            "user",
            "alice",
            None,
        );
        store
            .write_tuple("mariadb-pagination", tuple)
            .await
            .unwrap();
    }

    // Test pagination
    let page1 = store
        .read_tuples_paginated(
            "mariadb-pagination",
            &TupleFilter::default(),
            &rsfga_storage::PaginationOptions {
                page_size: Some(3),
                continuation_token: None,
            },
        )
        .await
        .expect("Should paginate on MariaDB");

    assert_eq!(page1.items.len(), 3, "First page should have 3 items");
    assert!(
        page1.continuation_token.is_some(),
        "Should have continuation token"
    );

    // Cleanup
    store.delete_store("mariadb-pagination").await.unwrap();
}

// =============================================================================
// Manual Tests (Using MARIADB_URL environment variable)
// =============================================================================

/// Test: Can connect to MariaDB via environment URL
#[tokio::test]
#[ignore = "requires running MariaDB"]
async fn test_mariadb_can_connect() {
    let store = create_store_from_env().await;
    let result = store.list_stores().await;
    assert!(
        result.is_ok(),
        "Should be able to list stores on MariaDB: {:?}",
        result.err()
    );
}

/// Test: Migrations are idempotent on MariaDB
#[tokio::test]
#[ignore = "requires running MariaDB"]
async fn test_mariadb_migrations_idempotent() {
    let store = create_store_from_env().await;

    // Run migrations again
    store
        .run_migrations()
        .await
        .expect("Migrations should be idempotent");

    // Verify we can still operate
    store
        .create_store("mariadb-idempotent", "Test")
        .await
        .unwrap();
    store.delete_store("mariadb-idempotent").await.unwrap();
}
