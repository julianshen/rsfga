//! Integration tests using testcontainers for automatic database provisioning.
//!
//! These tests use testcontainers to automatically start and manage database
//! containers, eliminating the need for manual database setup.
//!
//! ## Running Tests
//!
//! These tests require Docker to be running:
//!
//! ```bash
//! # Run all testcontainer tests
//! cargo test -p rsfga-storage --test testcontainers_integration -- --ignored
//!
//! # Run specific test
//! cargo test -p rsfga-storage --test testcontainers_integration test_health_check_returns_unhealthy_when_db_unreachable -- --ignored
//! ```
//!
//! ## Test Categories
//!
//! 1. **Health Check Unhealthy States** (Issue #151)
//!    - Database unreachable
//!    - Pool exhaustion
//!    - Connection timeouts
//!
//! 2. **True Timeout Tests** (Issue #150)
//!    - Using pg_sleep() / SLEEP() to induce slow queries
//!    - Verifying StorageError::QueryTimeout is returned
//!
//! 3. **Recovery Tests**
//!    - Health check recovery after database restart

use rsfga_storage::{
    DataStore, MySQLConfig, MySQLDataStore, PostgresConfig, PostgresDataStore, StorageError,
    StoredTuple, TupleFilter,
};
use std::sync::Arc;
use std::time::Duration;
use testcontainers::runners::AsyncRunner;
use testcontainers::ContainerAsync;
use testcontainers_modules::{mysql::Mysql, postgres::Postgres};

// =============================================================================
// Test Container Helpers
// =============================================================================

/// PostgreSQL container wrapper with connection info.
struct PostgresContainer {
    #[allow(dead_code)]
    container: ContainerAsync<Postgres>,
    connection_string: String,
}

impl PostgresContainer {
    async fn new() -> Self {
        let container = Postgres::default().start().await.unwrap();
        let host_port = container.get_host_port_ipv4(5432).await.unwrap();
        let connection_string = format!(
            "postgres://postgres:postgres@127.0.0.1:{}/postgres",
            host_port
        );
        Self {
            container,
            connection_string,
        }
    }

    fn connection_string(&self) -> &str {
        &self.connection_string
    }
}

/// MySQL container wrapper with connection info.
struct MysqlContainer {
    #[allow(dead_code)]
    container: ContainerAsync<Mysql>,
    connection_string: String,
}

impl MysqlContainer {
    async fn new() -> Self {
        let container = Mysql::default().start().await.unwrap();
        let host_port = container.get_host_port_ipv4(3306).await.unwrap();
        // Default MySQL image uses root with no password
        let connection_string = format!("mysql://root@127.0.0.1:{}/test", host_port);
        Self {
            container,
            connection_string,
        }
    }

    fn connection_string(&self) -> &str {
        &self.connection_string
    }
}

/// Create a PostgresDataStore from a container.
async fn create_postgres_store(container: &PostgresContainer) -> PostgresDataStore {
    let config = PostgresConfig {
        database_url: container.connection_string().to_string(),
        max_connections: 5,
        min_connections: 1,
        connect_timeout_secs: 30,
        query_timeout_secs: 30,
        ..Default::default()
    };

    let store = PostgresDataStore::from_config(&config)
        .await
        .expect("Failed to create PostgresDataStore");

    store
        .run_migrations()
        .await
        .expect("Failed to run migrations");

    store
}

/// Create a PostgresDataStore with custom config from a container.
async fn create_postgres_store_with_config(
    container: &PostgresContainer,
    max_connections: u32,
    query_timeout_secs: u64,
) -> PostgresDataStore {
    let config = PostgresConfig {
        database_url: container.connection_string().to_string(),
        max_connections,
        min_connections: 1,
        connect_timeout_secs: 5,
        query_timeout_secs,
        ..Default::default()
    };

    let store = PostgresDataStore::from_config(&config)
        .await
        .expect("Failed to create PostgresDataStore");

    store
        .run_migrations()
        .await
        .expect("Failed to run migrations");

    store
}

/// Create a MySQLDataStore from a container.
async fn create_mysql_store(container: &MysqlContainer) -> MySQLDataStore {
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
        .expect("Failed to create MySQLDataStore");

    store
        .run_migrations()
        .await
        .expect("Failed to run migrations");

    store
}

/// Create a MySQLDataStore with custom config from a container.
async fn create_mysql_store_with_config(
    container: &MysqlContainer,
    max_connections: u32,
    query_timeout_secs: u64,
) -> MySQLDataStore {
    let config = MySQLConfig {
        database_url: container.connection_string().to_string(),
        max_connections,
        min_connections: 1,
        connect_timeout_secs: 5,
        query_timeout_secs,
        ..Default::default()
    };

    let store = MySQLDataStore::from_config(&config)
        .await
        .expect("Failed to create MySQLDataStore");

    store
        .run_migrations()
        .await
        .expect("Failed to run migrations");

    store
}

// =============================================================================
// Issue #71: Basic Testcontainers Integration
// =============================================================================

/// Test: PostgreSQL container starts and accepts connections
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_postgres_container_starts_and_connects() {
    let container = PostgresContainer::new().await;
    let store = create_postgres_store(&container).await;

    // Verify connection works
    let result = store.list_stores().await;
    assert!(result.is_ok(), "Should be able to list stores");
}

/// Test: MySQL container starts and accepts connections
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_mysql_container_starts_and_connects() {
    let container = MysqlContainer::new().await;
    let store = create_mysql_store(&container).await;

    // Verify connection works
    let result = store.list_stores().await;
    assert!(result.is_ok(), "Should be able to list stores");
}

/// Test: Basic CRUD operations work with testcontainer (PostgreSQL)
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_postgres_crud_with_testcontainer() {
    let container = PostgresContainer::new().await;
    let store = create_postgres_store(&container).await;

    // Create store
    store
        .create_store("test-crud", "Test CRUD Store")
        .await
        .unwrap();

    // Write tuple
    let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);
    store.write_tuple("test-crud", tuple.clone()).await.unwrap();

    // Read tuple
    let tuples = store
        .read_tuples("test-crud", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1);
    assert_eq!(tuples[0].user_id, "alice");

    // Delete tuple
    store.delete_tuple("test-crud", tuple).await.unwrap();
    let tuples = store
        .read_tuples("test-crud", &TupleFilter::default())
        .await
        .unwrap();
    assert!(tuples.is_empty());

    // Cleanup
    store.delete_store("test-crud").await.unwrap();
}

/// Test: Basic CRUD operations work with testcontainer (MySQL)
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_mysql_crud_with_testcontainer() {
    let container = MysqlContainer::new().await;
    let store = create_mysql_store(&container).await;

    // Create store
    store
        .create_store("test-crud", "Test CRUD Store")
        .await
        .unwrap();

    // Write tuple
    let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);
    store.write_tuple("test-crud", tuple.clone()).await.unwrap();

    // Read tuple
    let tuples = store
        .read_tuples("test-crud", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1);
    assert_eq!(tuples[0].user_id, "alice");

    // Delete tuple
    store.delete_tuple("test-crud", tuple).await.unwrap();
    let tuples = store
        .read_tuples("test-crud", &TupleFilter::default())
        .await
        .unwrap();
    assert!(tuples.is_empty());

    // Cleanup
    store.delete_store("test-crud").await.unwrap();
}

// =============================================================================
// Issue #151: Health Check Tests for Unhealthy States
// =============================================================================

/// Test: Health check returns healthy with valid container (PostgreSQL)
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_postgres_health_check_returns_healthy_with_container() {
    let container = PostgresContainer::new().await;
    let store = create_postgres_store(&container).await;

    let status = store.health_check().await.unwrap();

    assert!(status.healthy, "Store should be healthy");
    assert!(
        status.latency.as_millis() < 5000,
        "Health check should complete quickly"
    );
    assert_eq!(status.message, Some("postgresql".to_string()));
}

/// Test: Health check returns healthy with valid container (MySQL)
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_mysql_health_check_returns_healthy_with_container() {
    let container = MysqlContainer::new().await;
    let store = create_mysql_store(&container).await;

    let status = store.health_check().await.unwrap();

    assert!(status.healthy, "Store should be healthy");
    assert!(
        status.latency.as_millis() < 5000,
        "Health check should complete quickly"
    );
    assert_eq!(status.message, Some("mysql".to_string()));
}

/// Test: Health check returns unhealthy when database becomes unreachable (PostgreSQL)
///
/// This test verifies that the health check correctly reports unhealthy status
/// when the database container is stopped.
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_postgres_health_check_returns_unhealthy_when_db_unreachable() {
    let container = PostgresContainer::new().await;
    let store = create_postgres_store(&container).await;

    // Verify healthy first
    let status = store.health_check().await.unwrap();
    assert!(status.healthy, "Store should initially be healthy");

    // Stop the container to make DB unreachable
    container.container.stop().await.unwrap();

    // Give SQLx time to detect the connection is gone
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Health check should now return unhealthy or error
    let result = store.health_check().await;

    match result {
        Ok(status) => {
            assert!(
                !status.healthy,
                "Store should be unhealthy after container stopped"
            );
        }
        Err(_) => {
            // Connection error is also acceptable - the DB is unreachable
        }
    }
}

/// Test: Health check returns unhealthy when database becomes unreachable (MySQL)
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_mysql_health_check_returns_unhealthy_when_db_unreachable() {
    let container = MysqlContainer::new().await;
    let store = create_mysql_store(&container).await;

    // Verify healthy first
    let status = store.health_check().await.unwrap();
    assert!(status.healthy, "Store should initially be healthy");

    // Stop the container to make DB unreachable
    container.container.stop().await.unwrap();

    // Give SQLx time to detect the connection is gone
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Health check should now return unhealthy or error
    let result = store.health_check().await;

    match result {
        Ok(status) => {
            assert!(
                !status.healthy,
                "Store should be unhealthy after container stopped"
            );
        }
        Err(_) => {
            // Connection error is also acceptable - the DB is unreachable
        }
    }
}

/// Test: Health check behavior when pool is exhausted (PostgreSQL)
///
/// This test creates a small connection pool and exhausts it with long-running
/// queries, then verifies that health check still works (it should use its own
/// connection or timeout gracefully).
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_postgres_health_check_under_pool_exhaustion() {
    let container = PostgresContainer::new().await;
    // Create store with very small pool
    let store = Arc::new(create_postgres_store_with_config(&container, 2, 30).await);

    store.create_store("test-pool", "Test Pool").await.unwrap();

    // Start long-running queries to exhaust pool
    let store_clone = Arc::clone(&store);
    let handles: Vec<_> = (0..2)
        .map(|i| {
            let store = Arc::clone(&store_clone);
            tokio::spawn(async move {
                // Use pg_sleep to hold connections
                let pool = store.pool();
                let _ = sqlx::query("SELECT pg_sleep(5)").execute(pool).await;
                i
            })
        })
        .collect();

    // Give queries time to start
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Health check should still work (may timeout but shouldn't panic)
    let start = std::time::Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(10), store.health_check()).await;

    match result {
        Ok(Ok(status)) => {
            // If health check completed, it should report some status
            println!(
                "Health check completed: healthy={}, latency={:?}",
                status.healthy, status.latency
            );
        }
        Ok(Err(e)) => {
            // Error is acceptable under pool exhaustion
            println!("Health check error (acceptable): {:?}", e);
        }
        Err(_) => {
            // Timeout is acceptable under pool exhaustion
            let elapsed = start.elapsed();
            println!("Health check timed out after {:?} (acceptable)", elapsed);
        }
    }

    // Wait for long-running queries to complete
    for handle in handles {
        let _ = handle.await;
    }

    // Cleanup
    let _ = store.delete_store("test-pool").await;
}

/// Test: Health check behavior when pool is exhausted (MySQL)
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_mysql_health_check_under_pool_exhaustion() {
    let container = MysqlContainer::new().await;
    // Create store with very small pool
    let store = Arc::new(create_mysql_store_with_config(&container, 2, 30).await);

    store.create_store("test-pool", "Test Pool").await.unwrap();

    // Start long-running queries to exhaust pool
    let store_clone = Arc::clone(&store);
    let handles: Vec<_> = (0..2)
        .map(|i| {
            let store = Arc::clone(&store_clone);
            tokio::spawn(async move {
                // Use SLEEP to hold connections
                let pool = store.pool();
                let _ = sqlx::query("SELECT SLEEP(5)").execute(pool).await;
                i
            })
        })
        .collect();

    // Give queries time to start
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Health check should still work (may timeout but shouldn't panic)
    let start = std::time::Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(10), store.health_check()).await;

    match result {
        Ok(Ok(status)) => {
            // If health check completed, it should report some status
            println!(
                "Health check completed: healthy={}, latency={:?}",
                status.healthy, status.latency
            );
        }
        Ok(Err(e)) => {
            // Error is acceptable under pool exhaustion
            println!("Health check error (acceptable): {:?}", e);
        }
        Err(_) => {
            // Timeout is acceptable under pool exhaustion
            let elapsed = start.elapsed();
            println!("Health check timed out after {:?} (acceptable)", elapsed);
        }
    }

    // Wait for long-running queries to complete
    for handle in handles {
        let _ = handle.await;
    }

    // Cleanup
    let _ = store.delete_store("test-pool").await;
}

/// Test: Health check recovers after database restart (PostgreSQL)
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_postgres_health_check_recovers_after_db_restart() {
    let container = PostgresContainer::new().await;
    let store = create_postgres_store(&container).await;

    // Verify healthy
    let status = store.health_check().await.unwrap();
    assert!(status.healthy, "Store should initially be healthy");

    // Stop the container
    container.container.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify unhealthy - either unhealthy status or error is acceptable
    if let Ok(status) = store.health_check().await {
        assert!(!status.healthy, "Should be unhealthy after stop");
    }

    // Restart the container
    container.container.start().await.unwrap();

    // Wait for database to be ready (PostgreSQL typically takes a few seconds)
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Health check should recover
    // Note: SQLx connection pool may need time to reconnect
    let mut recovered = false;
    for _ in 0..10 {
        if let Ok(status) = store.health_check().await {
            if status.healthy {
                recovered = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    assert!(
        recovered,
        "Health check should recover after database restart"
    );
}

// =============================================================================
// Issue #150: True Timeout Integration Tests
// =============================================================================

/// Test: Query timeout triggers on slow query (PostgreSQL)
///
/// Uses table locking to induce a blocked query that exceeds the configured
/// timeout, verifying that StorageError::QueryTimeout is returned.
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_postgres_query_timeout_triggers_on_slow_query() {
    let container = PostgresContainer::new().await;
    // Create store with a 2-second timeout
    let store = Arc::new(create_postgres_store_with_config(&container, 5, 2).await);
    store.create_store("test-timeout", "test").await.unwrap();

    // Acquire a connection and lock the tuples table to cause a timeout
    let mut conn = store.pool().acquire().await.unwrap();
    sqlx::query("BEGIN; LOCK TABLE tuples IN ACCESS EXCLUSIVE MODE;")
        .execute(&mut *conn)
        .await
        .unwrap();

    // This write operation should time out because the table is locked.
    let result = store
        .write_tuple(
            "test-timeout",
            StoredTuple::new("doc", "1", "viewer", "user", "alice", None),
        )
        .await;

    // Assert that we received a query timeout error
    assert!(
        matches!(result, Err(StorageError::QueryTimeout { .. })),
        "Expected a QueryTimeout error, but got {:?}",
        result
    );

    // Cleanup: rollback the transaction to release the lock
    sqlx::query("ROLLBACK").execute(&mut *conn).await.unwrap();
    let _ = store.delete_store("test-timeout").await;
}

/// Test: Query timeout triggers on slow query (MySQL)
///
/// Uses table locking to induce a blocked query that exceeds the configured
/// timeout, verifying that StorageError::QueryTimeout is returned.
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_mysql_query_timeout_triggers_on_slow_query() {
    let container = MysqlContainer::new().await;
    // Create store with a 2-second timeout
    let store = Arc::new(create_mysql_store_with_config(&container, 5, 2).await);
    store.create_store("test-timeout", "test").await.unwrap();

    // Acquire a connection and lock the tuples table to cause a timeout
    let mut conn = store.pool().acquire().await.unwrap();
    sqlx::query("LOCK TABLES tuples WRITE")
        .execute(&mut *conn)
        .await
        .unwrap();

    // This write operation should time out because the table is locked.
    let result = store
        .write_tuple(
            "test-timeout",
            StoredTuple::new("doc", "1", "viewer", "user", "alice", None),
        )
        .await;

    // Assert that we received a query timeout error
    assert!(
        matches!(result, Err(StorageError::QueryTimeout { .. })),
        "Expected a QueryTimeout error, but got {:?}",
        result
    );

    // Cleanup: unlock tables
    sqlx::query("UNLOCK TABLES")
        .execute(&mut *conn)
        .await
        .unwrap();
    let _ = store.delete_store("test-timeout").await;
}

/// Test: Multiple timeouts don't exhaust the connection pool (PostgreSQL)
///
/// This test verifies that after multiple query timeouts, the connection pool
/// recovers and subsequent operations succeed.
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_postgres_multiple_timeouts_dont_exhaust_pool() {
    let container = PostgresContainer::new().await;
    // Create store with a short timeout and a small pool
    let store = Arc::new(create_postgres_store_with_config(&container, 3, 2).await);

    store
        .create_store("test-timeout-recovery", "Test Store")
        .await
        .unwrap();

    // Lock the table to make all subsequent writes time out
    let mut conn_locker = store.pool().acquire().await.unwrap();
    sqlx::query("BEGIN; LOCK TABLE tuples IN ACCESS EXCLUSIVE MODE;")
        .execute(&mut *conn_locker)
        .await
        .unwrap();

    // Trigger several timeouts via concurrent writes
    let handles: Vec<_> = (0..3)
        .map(|i| {
            let store = Arc::clone(&store);
            tokio::spawn(async move {
                let tuple =
                    StoredTuple::new("doc", format!("doc{}", i), "viewer", "user", "alice", None);
                // This should time out
                store.write_tuple("test-timeout-recovery", tuple).await
            })
        })
        .collect();

    // Wait for all tasks to complete
    for handle in handles {
        let result = handle.await.unwrap();
        assert!(
            matches!(result, Err(StorageError::QueryTimeout { .. })),
            "Expected QueryTimeout, got {:?}",
            result
        );
    }

    // Release the lock
    sqlx::query("ROLLBACK")
        .execute(&mut *conn_locker)
        .await
        .unwrap();

    // Give the pool a moment to recover if needed
    tokio::time::sleep(Duration::from_secs(1)).await;

    // A normal operation should now succeed, proving the pool is healthy
    let tuple = StoredTuple::new("document", "doc-final", "viewer", "user", "alice", None);
    let result = store.write_tuple("test-timeout-recovery", tuple).await;

    assert!(
        result.is_ok(),
        "Write should succeed after timeout recovery: {:?}",
        result
    );

    // Cleanup
    store.delete_store("test-timeout-recovery").await.unwrap();
}

/// Test: Multiple timeouts don't exhaust the connection pool (MySQL)
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_mysql_multiple_timeouts_dont_exhaust_pool() {
    let container = MysqlContainer::new().await;
    // Create store with a short timeout and a small pool
    let store = Arc::new(create_mysql_store_with_config(&container, 3, 2).await);

    store
        .create_store("test-timeout-recovery", "Test Store")
        .await
        .unwrap();

    // Lock the table to make all subsequent writes time out
    let mut conn_locker = store.pool().acquire().await.unwrap();
    sqlx::query("LOCK TABLES tuples WRITE")
        .execute(&mut *conn_locker)
        .await
        .unwrap();

    // Trigger several timeouts via concurrent writes
    let handles: Vec<_> = (0..3)
        .map(|i| {
            let store = Arc::clone(&store);
            tokio::spawn(async move {
                let tuple =
                    StoredTuple::new("doc", format!("doc{}", i), "viewer", "user", "alice", None);
                // This should time out
                store.write_tuple("test-timeout-recovery", tuple).await
            })
        })
        .collect();

    // Wait for all tasks to complete
    for handle in handles {
        let result = handle.await.unwrap();
        assert!(
            matches!(result, Err(StorageError::QueryTimeout { .. })),
            "Expected QueryTimeout, got {:?}",
            result
        );
    }

    // Release the lock
    sqlx::query("UNLOCK TABLES")
        .execute(&mut *conn_locker)
        .await
        .unwrap();

    // Give the pool a moment to recover if needed
    tokio::time::sleep(Duration::from_secs(1)).await;

    // A normal operation should now succeed, proving the pool is healthy
    let tuple = StoredTuple::new("document", "doc-final", "viewer", "user", "alice", None);
    let result = store.write_tuple("test-timeout-recovery", tuple).await;

    assert!(
        result.is_ok(),
        "Write should succeed after timeout recovery: {:?}",
        result
    );

    // Cleanup
    store.delete_store("test-timeout-recovery").await.unwrap();
}

/// Test: Timeout cancels in-progress query (PostgreSQL)
///
/// Verifies that when a timeout occurs, the operation returns promptly
/// rather than waiting for the full query duration.
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_postgres_timeout_returns_promptly() {
    let container = PostgresContainer::new().await;
    // Create store with a 1-second timeout
    let store = Arc::new(create_postgres_store_with_config(&container, 5, 1).await);
    store
        .create_store("test-prompt-timeout", "Test Store")
        .await
        .unwrap();

    // Lock the table to cause a timeout on write operations
    let mut conn = store.pool().acquire().await.unwrap();
    sqlx::query("BEGIN; LOCK TABLE tuples IN ACCESS EXCLUSIVE MODE;")
        .execute(&mut *conn)
        .await
        .unwrap();

    // Time how long the blocking operation takes
    let start = std::time::Instant::now();
    let result = store
        .write_tuple(
            "test-prompt-timeout",
            StoredTuple::new("doc", "1", "viewer", "user", "alice", None),
        )
        .await;
    let elapsed = start.elapsed();

    // Assert that the operation returned promptly (around 1s), not indefinitely.
    // We allow a small buffer for test environment variance.
    assert!(
        elapsed.as_millis() < 3000,
        "Operation should have returned promptly after the 1s timeout, but it took {:?}",
        elapsed
    );

    // Assert that the result is a timeout error
    assert!(
        matches!(result, Err(StorageError::QueryTimeout { .. })),
        "Expected a QueryTimeout error, but got {:?}",
        result
    );

    // Cleanup
    sqlx::query("ROLLBACK").execute(&mut *conn).await.unwrap();
    let _ = store.delete_store("test-prompt-timeout").await;
}

/// Test: Timeout returns promptly (MySQL)
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_mysql_timeout_returns_promptly() {
    let container = MysqlContainer::new().await;
    // Create store with a 1-second timeout
    let store = Arc::new(create_mysql_store_with_config(&container, 5, 1).await);
    store
        .create_store("test-prompt-timeout", "Test Store")
        .await
        .unwrap();

    // Lock the table to cause a timeout on write operations
    let mut conn = store.pool().acquire().await.unwrap();
    sqlx::query("LOCK TABLES tuples WRITE")
        .execute(&mut *conn)
        .await
        .unwrap();

    // Time how long the blocking operation takes
    let start = std::time::Instant::now();
    let result = store
        .write_tuple(
            "test-prompt-timeout",
            StoredTuple::new("doc", "1", "viewer", "user", "alice", None),
        )
        .await;
    let elapsed = start.elapsed();

    // Assert that the operation returned promptly (around 1s), not indefinitely.
    // We allow a small buffer for test environment variance.
    assert!(
        elapsed.as_millis() < 3000,
        "Operation should have returned promptly after the 1s timeout, but it took {:?}",
        elapsed
    );

    // Assert that the result is a timeout error
    assert!(
        matches!(result, Err(StorageError::QueryTimeout { .. })),
        "Expected a QueryTimeout error, but got {:?}",
        result
    );

    // Cleanup
    sqlx::query("UNLOCK TABLES")
        .execute(&mut *conn)
        .await
        .unwrap();
    let _ = store.delete_store("test-prompt-timeout").await;
}

// =============================================================================
// Additional Integration Tests
// =============================================================================

/// Test: Concurrent operations with testcontainer (PostgreSQL)
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_postgres_concurrent_operations_with_container() {
    let container = PostgresContainer::new().await;
    let store = Arc::new(create_postgres_store(&container).await);

    store
        .create_store("test-concurrent", "Test Concurrent")
        .await
        .unwrap();

    // Run concurrent writes
    let handles: Vec<_> = (0..20)
        .map(|i| {
            let store = Arc::clone(&store);
            tokio::spawn(async move {
                let tuple = StoredTuple::new(
                    "document",
                    format!("doc{}", i),
                    "viewer",
                    "user",
                    format!("user{}", i),
                    None,
                );
                store.write_tuple("test-concurrent", tuple).await
            })
        })
        .collect();

    // Collect results
    for handle in handles {
        handle.await.unwrap().unwrap();
    }

    // Verify all writes succeeded
    let tuples = store
        .read_tuples("test-concurrent", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 20);

    // Cleanup
    store.delete_store("test-concurrent").await.unwrap();
}

/// Test: Concurrent operations with testcontainer (MySQL)
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_mysql_concurrent_operations_with_container() {
    let container = MysqlContainer::new().await;
    let store = Arc::new(create_mysql_store(&container).await);

    store
        .create_store("test-concurrent", "Test Concurrent")
        .await
        .unwrap();

    // Run concurrent writes
    let handles: Vec<_> = (0..20)
        .map(|i| {
            let store = Arc::clone(&store);
            tokio::spawn(async move {
                let tuple = StoredTuple::new(
                    "document",
                    format!("doc{}", i),
                    "viewer",
                    "user",
                    format!("user{}", i),
                    None,
                );
                store.write_tuple("test-concurrent", tuple).await
            })
        })
        .collect();

    // Collect results
    for handle in handles {
        handle.await.unwrap().unwrap();
    }

    // Verify all writes succeeded
    let tuples = store
        .read_tuples("test-concurrent", &TupleFilter::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 20);

    // Cleanup
    store.delete_store("test-concurrent").await.unwrap();
}

/// Test: Connection pool statistics are accurate (PostgreSQL)
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_postgres_pool_stats_accuracy() {
    let container = PostgresContainer::new().await;
    let store = create_postgres_store_with_config(&container, 10, 30).await;

    let status = store.health_check().await.unwrap();
    let pool_stats = status.pool_stats.expect("Should have pool stats");

    assert_eq!(pool_stats.max_connections, 10);
    assert!(
        pool_stats.idle_connections + pool_stats.active_connections <= 10,
        "Total connections should not exceed max"
    );
}

/// Test: Connection pool statistics are accurate (MySQL)
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_mysql_pool_stats_accuracy() {
    let container = MysqlContainer::new().await;
    let store = create_mysql_store_with_config(&container, 10, 30).await;

    let status = store.health_check().await.unwrap();
    let pool_stats = status.pool_stats.expect("Should have pool stats");

    assert_eq!(pool_stats.max_connections, 10);
    assert!(
        pool_stats.idle_connections + pool_stats.active_connections <= 10,
        "Total connections should not exceed max"
    );
}
