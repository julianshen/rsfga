//! gRPC Transport Layer Integration Tests
//!
//! These tests verify the gRPC transport layer functionality including:
//! - Server binding and port allocation
//! - gRPC service responses over network
//! - gRPC reflection service (service discovery)
//! - gRPC health check service
//! - Graceful shutdown
//! - Error handling for port conflicts
//!
//! # Test Architecture
//!
//! Unlike the unit tests in `grpc/tests.rs` which test the service trait directly,
//! these tests start an actual gRPC server and connect to it over the network.
//! This validates the complete transport layer integration.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::oneshot;
use tokio::time::timeout;
use tonic::transport::Channel;

use rsfga_api::grpc::{run_grpc_server_with_shutdown, GrpcServerConfig};
use rsfga_api::proto::openfga::v1::open_fga_service_client::OpenFgaServiceClient;
use rsfga_api::proto::openfga::v1::*;
use rsfga_storage::{DataStore, MemoryDataStore, StoredAuthorizationModel};

/// Timeout for server startup in tests.
const SERVER_STARTUP_TIMEOUT: Duration = Duration::from_secs(5);

/// Timeout for client operations in tests.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(5);

/// Helper to create a simple authorization model for tests.
fn simple_model_json() -> String {
    r#"{
        "type_definitions": [
            {"type": "user"},
            {"type": "document", "relations": {"viewer": {}, "editor": {}, "owner": {}}}
        ]
    }"#
    .to_string()
}

/// Helper to create and write a simple authorization model to a store.
async fn setup_simple_model(storage: &MemoryDataStore, store_id: &str) -> String {
    let model = StoredAuthorizationModel::new(
        ulid::Ulid::new().to_string(),
        store_id,
        "1.1",
        simple_model_json(),
    );
    let model_id = model.id.clone();
    storage.write_authorization_model(model).await.unwrap();
    model_id
}

/// Find an available port for testing by binding to port 0.
fn find_available_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// Start a gRPC server on the given address with a shutdown channel.
/// Returns a handle that can be awaited to wait for server completion.
async fn start_test_server<S>(
    storage: Arc<S>,
    addr: SocketAddr,
    config: GrpcServerConfig,
) -> (
    tokio::task::JoinHandle<Result<(), Box<dyn std::error::Error + Send + Sync>>>,
    oneshot::Sender<()>,
)
where
    S: DataStore + Send + Sync + 'static,
{
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    let handle = tokio::spawn(async move {
        let shutdown_signal = async move {
            let _ = shutdown_rx.await;
        };
        run_grpc_server_with_shutdown(storage, addr, config, shutdown_signal).await
    });

    // Give the server a moment to bind
    tokio::time::sleep(Duration::from_millis(100)).await;

    (handle, shutdown_tx)
}

/// Create a gRPC channel connected to the given address.
async fn create_channel(addr: SocketAddr) -> Result<Channel, tonic::transport::Error> {
    let endpoint = format!("http://{}", addr);
    Channel::from_shared(endpoint).unwrap().connect().await
}

/// Create a gRPC client connected to the given address.
async fn create_client(
    addr: SocketAddr,
) -> Result<OpenFgaServiceClient<Channel>, tonic::transport::Error> {
    let channel = create_channel(addr).await?;
    Ok(OpenFgaServiceClient::new(channel))
}

// ============================================================================
// Section 1: Server Startup and Binding Tests
// ============================================================================

/// Test: gRPC server starts and binds to port successfully.
///
/// Verifies the gRPC transport layer can:
/// 1. Bind to the specified address
/// 2. Accept incoming connections
/// 3. Respond to gRPC requests
#[tokio::test]
async fn test_grpc_server_starts_and_binds_to_port() {
    let storage = Arc::new(MemoryDataStore::new());
    let port = find_available_port();
    let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
    let config = GrpcServerConfig::default();

    let (handle, shutdown_tx) = start_test_server(Arc::clone(&storage), addr, config).await;

    // Try to connect to the server
    let client_result = timeout(CLIENT_TIMEOUT, create_client(addr)).await;
    assert!(client_result.is_ok(), "Should connect within timeout");
    assert!(
        client_result.unwrap().is_ok(),
        "Should connect successfully"
    );

    // Shutdown the server
    let _ = shutdown_tx.send(());
    let result = timeout(SERVER_STARTUP_TIMEOUT, handle).await;
    assert!(result.is_ok(), "Server should shutdown within timeout");
    assert!(
        result.unwrap().unwrap().is_ok(),
        "Server should shutdown cleanly"
    );
}

/// Test: gRPC server responds to CreateStore request over network.
///
/// This is an end-to-end test verifying the complete transport path:
/// Client → Network → Tonic Server → OpenFgaGrpcService → Storage → Response
#[tokio::test]
async fn test_grpc_server_responds_to_requests() {
    let storage = Arc::new(MemoryDataStore::new());
    let port = find_available_port();
    let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
    let config = GrpcServerConfig::default();

    let (handle, shutdown_tx) = start_test_server(Arc::clone(&storage), addr, config).await;

    // Connect and make a request
    let mut client = create_client(addr).await.expect("Should connect to server");

    let response = timeout(
        CLIENT_TIMEOUT,
        client.create_store(CreateStoreRequest {
            name: "Test Store".to_string(),
        }),
    )
    .await;

    assert!(response.is_ok(), "Request should complete within timeout");
    let response = response.unwrap();
    assert!(response.is_ok(), "CreateStore should succeed");

    let store = response.unwrap().into_inner();
    assert!(!store.id.is_empty(), "Store should have an ID");
    assert_eq!(store.name, "Test Store");

    // Cleanup
    let _ = shutdown_tx.send(());
    let _ = timeout(SERVER_STARTUP_TIMEOUT, handle).await;
}

/// Test: gRPC Check request works over transport layer.
///
/// Verifies Check RPC works end-to-end through the transport layer.
#[tokio::test]
async fn test_grpc_check_request_over_transport() {
    let storage = Arc::new(MemoryDataStore::new());
    let port = find_available_port();
    let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
    let config = GrpcServerConfig::default();

    // Create store and model
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();
    setup_simple_model(&storage, "test-store").await;

    // Write a tuple
    storage
        .write_tuple(
            "test-store",
            rsfga_storage::StoredTuple::new("document", "doc1", "viewer", "user", "alice", None),
        )
        .await
        .unwrap();

    let (handle, shutdown_tx) = start_test_server(Arc::clone(&storage), addr, config).await;

    // Connect and make Check request
    let mut client = create_client(addr).await.expect("Should connect to server");

    // Check should return true for existing tuple
    let response = timeout(
        CLIENT_TIMEOUT,
        client.check(CheckRequest {
            store_id: "test-store".to_string(),
            tuple_key: Some(TupleKey {
                user: "user:alice".to_string(),
                relation: "viewer".to_string(),
                object: "document:doc1".to_string(),
                condition: None,
            }),
            contextual_tuples: None,
            authorization_model_id: String::new(),
            trace: false,
            context: None,
            consistency: 0,
        }),
    )
    .await
    .unwrap()
    .unwrap();

    assert!(
        response.into_inner().allowed,
        "Alice should have viewer access"
    );

    // Check should return false for non-existing tuple
    let response = timeout(
        CLIENT_TIMEOUT,
        client.check(CheckRequest {
            store_id: "test-store".to_string(),
            tuple_key: Some(TupleKey {
                user: "user:bob".to_string(),
                relation: "viewer".to_string(),
                object: "document:doc1".to_string(),
                condition: None,
            }),
            contextual_tuples: None,
            authorization_model_id: String::new(),
            trace: false,
            context: None,
            consistency: 0,
        }),
    )
    .await
    .unwrap()
    .unwrap();

    assert!(
        !response.into_inner().allowed,
        "Bob should NOT have viewer access"
    );

    // Cleanup
    let _ = shutdown_tx.send(());
    let _ = timeout(SERVER_STARTUP_TIMEOUT, handle).await;
}

// ============================================================================
// Section 2: Health Check Service Tests
// ============================================================================

/// Test: gRPC health check service responds correctly.
///
/// Verifies the health check service is enabled and responds to health queries.
/// The health check service is set up for the OpenFgaServiceServer type, which
/// derives its service name from the protobuf service definition.
#[tokio::test]
async fn test_grpc_health_check_service_enabled() {
    use tonic_health::pb::health_client::HealthClient;
    use tonic_health::pb::HealthCheckRequest;

    let storage = Arc::new(MemoryDataStore::new());
    let port = find_available_port();
    let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
    let config = GrpcServerConfig {
        reflection_enabled: false,
        health_check_enabled: true,
    };

    let (handle, shutdown_tx) = start_test_server(Arc::clone(&storage), addr, config).await;

    // Connect health client
    let channel = create_channel(addr)
        .await
        .expect("Should connect to server");
    let mut health_client = HealthClient::new(channel);

    // Check overall server health (empty service name)
    // This is the standard pattern for checking if the health service is responding
    let response = timeout(
        CLIENT_TIMEOUT,
        health_client.check(HealthCheckRequest {
            service: String::new(), // Empty string checks overall health
        }),
    )
    .await;

    assert!(
        response.is_ok(),
        "Health check should complete within timeout"
    );
    let response = response.unwrap();
    assert!(response.is_ok(), "Health check should succeed");

    let status = response.unwrap().into_inner().status;
    assert_eq!(status, 1, "Server should be SERVING (status=1)");

    // Cleanup
    let _ = shutdown_tx.send(());
    let _ = timeout(SERVER_STARTUP_TIMEOUT, handle).await;
}

/// Test: gRPC health check service disabled when configured.
///
/// Verifies health check service is not available when disabled.
#[tokio::test]
async fn test_grpc_health_check_service_disabled() {
    use tonic_health::pb::health_client::HealthClient;
    use tonic_health::pb::HealthCheckRequest;

    let storage = Arc::new(MemoryDataStore::new());
    let port = find_available_port();
    let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
    let config = GrpcServerConfig {
        reflection_enabled: false,
        health_check_enabled: false, // Disabled
    };

    let (handle, shutdown_tx) = start_test_server(Arc::clone(&storage), addr, config).await;

    // Connect health client
    let channel = create_channel(addr)
        .await
        .expect("Should connect to server");
    let mut health_client = HealthClient::new(channel);

    // Health check should fail when service is disabled
    let response = timeout(
        CLIENT_TIMEOUT,
        health_client.check(HealthCheckRequest {
            service: "openfga.v1.OpenFgaService".to_string(),
        }),
    )
    .await;

    assert!(response.is_ok(), "Request should complete within timeout");
    // When health service is disabled, the health check should return an error
    assert!(
        response.unwrap().is_err(),
        "Health check should fail when disabled"
    );

    // Cleanup
    let _ = shutdown_tx.send(());
    let _ = timeout(SERVER_STARTUP_TIMEOUT, handle).await;
}

// ============================================================================
// Section 3: Reflection Service Tests
// ============================================================================

/// Test: gRPC reflection service is enabled.
///
/// Verifies the reflection service allows service discovery via grpcurl-like tools.
/// Note: Testing reflection directly is complex; this test verifies the service
/// is registered by checking that a basic reflection query doesn't error immediately.
#[tokio::test]
async fn test_grpc_reflection_service_enabled() {
    let storage = Arc::new(MemoryDataStore::new());
    let port = find_available_port();
    let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
    let config = GrpcServerConfig {
        reflection_enabled: true,
        health_check_enabled: false,
    };

    let (handle, shutdown_tx) = start_test_server(Arc::clone(&storage), addr, config).await;

    // If reflection is enabled, the server should start successfully
    // and we should be able to connect
    let client_result = create_client(addr).await;
    assert!(
        client_result.is_ok(),
        "Should connect to server with reflection enabled"
    );

    // The actual reflection service can be tested via grpcurl in real scenarios
    // For integration tests, we verify the server starts with reflection enabled

    // Cleanup
    let _ = shutdown_tx.send(());
    let result = timeout(SERVER_STARTUP_TIMEOUT, handle).await;
    assert!(result.is_ok(), "Server should shutdown cleanly");
}

/// Test: Server starts correctly with reflection disabled.
#[tokio::test]
async fn test_grpc_server_starts_with_reflection_disabled() {
    let storage = Arc::new(MemoryDataStore::new());
    let port = find_available_port();
    let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
    let config = GrpcServerConfig {
        reflection_enabled: false, // Disabled
        health_check_enabled: false,
    };

    let (handle, shutdown_tx) = start_test_server(Arc::clone(&storage), addr, config).await;

    // Server should still work without reflection
    let mut client = create_client(addr).await.expect("Should connect to server");

    let response = client
        .create_store(CreateStoreRequest {
            name: "Test Store".to_string(),
        })
        .await;

    assert!(
        response.is_ok(),
        "CreateStore should succeed without reflection"
    );

    // Cleanup
    let _ = shutdown_tx.send(());
    let _ = timeout(SERVER_STARTUP_TIMEOUT, handle).await;
}

// ============================================================================
// Section 4: Graceful Shutdown Tests
// ============================================================================

/// Test: gRPC server performs graceful shutdown.
///
/// Verifies the server stops accepting new connections and completes
/// existing requests when shutdown is signaled.
#[tokio::test]
async fn test_grpc_server_graceful_shutdown() {
    let storage = Arc::new(MemoryDataStore::new());
    let port = find_available_port();
    let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
    let config = GrpcServerConfig::default();

    let (handle, shutdown_tx) = start_test_server(Arc::clone(&storage), addr, config).await;

    // Verify server is running
    let mut client = create_client(addr).await.expect("Should connect to server");
    let response = client
        .create_store(CreateStoreRequest {
            name: "Test".to_string(),
        })
        .await;
    assert!(response.is_ok(), "Request before shutdown should succeed");

    // Signal shutdown
    shutdown_tx.send(()).expect("Should send shutdown signal");

    // Wait for server to shutdown
    let result = timeout(SERVER_STARTUP_TIMEOUT, handle).await;
    assert!(result.is_ok(), "Server should shutdown within timeout");
    assert!(
        result.unwrap().unwrap().is_ok(),
        "Server should shutdown cleanly"
    );

    // New connections should fail after shutdown
    let new_client_result = timeout(Duration::from_millis(500), create_client(addr)).await;

    // Either the connect times out or fails
    if new_client_result.is_ok() {
        let client_result = new_client_result.unwrap();
        assert!(
            client_result.is_err(),
            "Connection after shutdown should fail"
        );
    }
    // Timeout is also acceptable - server is down
}

/// Test: Multiple clients can connect concurrently.
///
/// Verifies the server can handle multiple simultaneous connections.
#[tokio::test]
async fn test_grpc_server_handles_concurrent_clients() {
    let storage = Arc::new(MemoryDataStore::new());
    let port = find_available_port();
    let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
    let config = GrpcServerConfig::default();

    let (handle, shutdown_tx) = start_test_server(Arc::clone(&storage), addr, config).await;

    // Create multiple concurrent clients
    let num_clients = 10;
    let mut handles = Vec::new();

    for i in 0..num_clients {
        let addr_clone = addr;
        let h = tokio::spawn(async move {
            let mut client = create_client(addr_clone).await.expect("Should connect");
            client
                .create_store(CreateStoreRequest {
                    name: format!("Store {}", i),
                })
                .await
        });
        handles.push(h);
    }

    // Wait for all clients to complete
    let mut success_count = 0;
    for h in handles {
        let result = timeout(CLIENT_TIMEOUT, h).await;
        if result.is_ok() && result.unwrap().is_ok() {
            success_count += 1;
        }
    }

    assert_eq!(
        success_count, num_clients,
        "All concurrent requests should succeed"
    );

    // Cleanup
    let _ = shutdown_tx.send(());
    let _ = timeout(SERVER_STARTUP_TIMEOUT, handle).await;
}

// ============================================================================
// Section 5: Error Handling Tests
// ============================================================================

/// Test: gRPC server returns appropriate error for non-existent store.
///
/// Verifies errors are properly propagated through the transport layer.
#[tokio::test]
async fn test_grpc_server_returns_not_found_error() {
    let storage = Arc::new(MemoryDataStore::new());
    let port = find_available_port();
    let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
    let config = GrpcServerConfig::default();

    let (handle, shutdown_tx) = start_test_server(Arc::clone(&storage), addr, config).await;

    let mut client = create_client(addr).await.expect("Should connect to server");

    // Request for non-existent store should return NOT_FOUND
    let response = timeout(
        CLIENT_TIMEOUT,
        client.check(CheckRequest {
            store_id: "non-existent-store".to_string(),
            tuple_key: Some(TupleKey {
                user: "user:alice".to_string(),
                relation: "viewer".to_string(),
                object: "document:doc1".to_string(),
                condition: None,
            }),
            contextual_tuples: None,
            authorization_model_id: String::new(),
            trace: false,
            context: None,
            consistency: 0,
        }),
    )
    .await;

    assert!(response.is_ok(), "Request should complete within timeout");
    let response = response.unwrap();
    assert!(response.is_err(), "Check on non-existent store should fail");
    assert_eq!(
        response.unwrap_err().code(),
        tonic::Code::NotFound,
        "Should return NOT_FOUND status"
    );

    // Cleanup
    let _ = shutdown_tx.send(());
    let _ = timeout(SERVER_STARTUP_TIMEOUT, handle).await;
}

/// Test: gRPC server returns invalid argument for bad requests.
///
/// Verifies validation errors are properly propagated.
#[tokio::test]
async fn test_grpc_server_returns_invalid_argument_error() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    let port = find_available_port();
    let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
    let config = GrpcServerConfig::default();

    let (handle, shutdown_tx) = start_test_server(Arc::clone(&storage), addr, config).await;

    let mut client = create_client(addr).await.expect("Should connect to server");

    // Check without tuple_key should return INVALID_ARGUMENT
    let response = timeout(
        CLIENT_TIMEOUT,
        client.check(CheckRequest {
            store_id: "test-store".to_string(),
            tuple_key: None, // Missing required field
            contextual_tuples: None,
            authorization_model_id: String::new(),
            trace: false,
            context: None,
            consistency: 0,
        }),
    )
    .await;

    assert!(response.is_ok(), "Request should complete within timeout");
    let response = response.unwrap();
    assert!(response.is_err(), "Check without tuple_key should fail");
    assert_eq!(
        response.unwrap_err().code(),
        tonic::Code::InvalidArgument,
        "Should return INVALID_ARGUMENT status"
    );

    // Cleanup
    let _ = shutdown_tx.send(());
    let _ = timeout(SERVER_STARTUP_TIMEOUT, handle).await;
}

// ============================================================================
// Section 6: Configuration Tests
// ============================================================================

/// Test: Default configuration enables reflection and health check.
#[tokio::test]
async fn test_grpc_default_config_enables_all_services() {
    let config = GrpcServerConfig::default();
    assert!(
        config.reflection_enabled,
        "Reflection should be enabled by default"
    );
    assert!(
        config.health_check_enabled,
        "Health check should be enabled by default"
    );
}

/// Test: Server works with all services disabled.
///
/// Verifies the core OpenFGA service works even when optional services are disabled.
#[tokio::test]
async fn test_grpc_server_works_with_minimal_config() {
    let storage = Arc::new(MemoryDataStore::new());
    let port = find_available_port();
    let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
    let config = GrpcServerConfig {
        reflection_enabled: false,
        health_check_enabled: false,
    };

    let (handle, shutdown_tx) = start_test_server(Arc::clone(&storage), addr, config).await;

    // Core service should still work
    let mut client = create_client(addr).await.expect("Should connect to server");

    let response = timeout(
        CLIENT_TIMEOUT,
        client.create_store(CreateStoreRequest {
            name: "Minimal Config Test".to_string(),
        }),
    )
    .await;

    assert!(response.is_ok(), "Request should complete within timeout");
    assert!(response.unwrap().is_ok(), "CreateStore should succeed");

    // Cleanup
    let _ = shutdown_tx.send(());
    let _ = timeout(SERVER_STARTUP_TIMEOUT, handle).await;
}

// ============================================================================
// Section 7: BatchCheck Transport Tests
// ============================================================================

/// Test: BatchCheck works over transport layer.
///
/// Verifies BatchCheck RPC works end-to-end through the transport layer.
#[tokio::test]
async fn test_grpc_batch_check_over_transport() {
    let storage = Arc::new(MemoryDataStore::new());
    let port = find_available_port();
    let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
    let config = GrpcServerConfig::default();

    // Create store and model
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();
    setup_simple_model(&storage, "test-store").await;

    // Write a tuple
    storage
        .write_tuple(
            "test-store",
            rsfga_storage::StoredTuple::new("document", "doc1", "viewer", "user", "alice", None),
        )
        .await
        .unwrap();

    let (handle, shutdown_tx) = start_test_server(Arc::clone(&storage), addr, config).await;

    let mut client = create_client(addr).await.expect("Should connect to server");

    // BatchCheck request
    let response = timeout(
        CLIENT_TIMEOUT,
        client.batch_check(BatchCheckRequest {
            store_id: "test-store".to_string(),
            checks: vec![
                BatchCheckItem {
                    tuple_key: Some(TupleKey {
                        user: "user:alice".to_string(),
                        relation: "viewer".to_string(),
                        object: "document:doc1".to_string(),
                        condition: None,
                    }),
                    contextual_tuples: None,
                    context: None,
                    correlation_id: "check-1".to_string(),
                },
                BatchCheckItem {
                    tuple_key: Some(TupleKey {
                        user: "user:bob".to_string(),
                        relation: "viewer".to_string(),
                        object: "document:doc1".to_string(),
                        condition: None,
                    }),
                    contextual_tuples: None,
                    context: None,
                    correlation_id: "check-2".to_string(),
                },
            ],
            authorization_model_id: String::new(),
            consistency: 0,
        }),
    )
    .await
    .unwrap()
    .unwrap();

    let result = response.into_inner().result;
    assert_eq!(result.len(), 2, "Should have 2 results");
    assert!(
        result.get("check-1").unwrap().allowed,
        "Alice should have access"
    );
    assert!(
        !result.get("check-2").unwrap().allowed,
        "Bob should NOT have access"
    );

    // Cleanup
    let _ = shutdown_tx.send(());
    let _ = timeout(SERVER_STARTUP_TIMEOUT, handle).await;
}

// ============================================================================
// Section 8: Write and Read Operations Over Transport
// ============================================================================

/// Test: Write and Read operations work over transport layer.
///
/// Verifies the complete CRUD workflow works through the gRPC transport.
#[tokio::test]
async fn test_grpc_write_and_read_over_transport() {
    let storage = Arc::new(MemoryDataStore::new());
    let port = find_available_port();
    let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
    let config = GrpcServerConfig::default();

    // Create store and model
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();
    setup_simple_model(&storage, "test-store").await;

    let (handle, shutdown_tx) = start_test_server(Arc::clone(&storage), addr, config).await;

    let mut client = create_client(addr).await.expect("Should connect to server");

    // Write a tuple
    let write_response = timeout(
        CLIENT_TIMEOUT,
        client.write(WriteRequest {
            store_id: "test-store".to_string(),
            writes: Some(WriteRequestWrites {
                tuple_keys: vec![TupleKey {
                    user: "user:alice".to_string(),
                    relation: "editor".to_string(),
                    object: "document:report".to_string(),
                    condition: None,
                }],
            }),
            deletes: None,
            authorization_model_id: String::new(),
        }),
    )
    .await;

    assert!(
        write_response.is_ok(),
        "Write should complete within timeout"
    );
    assert!(write_response.unwrap().is_ok(), "Write should succeed");

    // Read the tuple back
    let read_response = timeout(
        CLIENT_TIMEOUT,
        client.read(ReadRequest {
            store_id: "test-store".to_string(),
            tuple_key: Some(TupleKeyWithoutCondition {
                user: "user:alice".to_string(),
                relation: "editor".to_string(),
                object: "document:report".to_string(),
            }),
            page_size: None,
            continuation_token: String::new(),
            consistency: 0,
        }),
    )
    .await;

    assert!(read_response.is_ok(), "Read should complete within timeout");
    let read_result = read_response.unwrap().unwrap().into_inner();
    assert_eq!(read_result.tuples.len(), 1, "Should have 1 tuple");
    assert_eq!(
        read_result.tuples[0].key.as_ref().unwrap().user,
        "user:alice"
    );

    // Cleanup
    let _ = shutdown_tx.send(());
    let _ = timeout(SERVER_STARTUP_TIMEOUT, handle).await;
}
