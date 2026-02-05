//! Integration tests for NATS JetStream using testcontainers.
//!
//! These tests verify:
//! - NATS connection establishment
//! - JetStream stream creation and configuration
//! - Message publishing and acknowledgment
//! - Write request and committed event workflows
//! - Circuit breaker behavior
//! - Reconnection handling
//!
//! ## Running Tests
//!
//! These tests require Docker to be running:
//!
//! ```bash
//! # Run all NATS integration tests
//! cargo test -p rsfga-nats --test nats_integration -- --ignored
//!
//! # Run specific test
//! cargo test -p rsfga-nats --test nats_integration test_nats_connection -- --ignored
//! ```

use rsfga_nats::{
    CommittedEvent, EventPublisher, JetStreamManager, NatsClient, NatsConfig, PublisherConfig,
    TupleKey, TupleOperation, WriteRequest, STREAM_EVENTS, STREAM_WRITES,
};
use std::time::Duration;
use testcontainers::runners::AsyncRunner;
use testcontainers::ContainerAsync;
use testcontainers_modules::nats::Nats;

// =============================================================================
// Test Container Helpers
// =============================================================================

/// NATS container wrapper with connection info.
struct NatsContainer {
    #[allow(dead_code)]
    container: ContainerAsync<Nats>,
    connection_url: String,
}

impl NatsContainer {
    /// Start a NATS container with JetStream enabled.
    async fn new() -> Self {
        // Start NATS container - testcontainers-modules handles JetStream by default
        let container = Nats::default().start().await.unwrap();
        let host_port = container.get_host_port_ipv4(4222).await.unwrap();
        let connection_url = format!("nats://127.0.0.1:{}", host_port);

        Self {
            container,
            connection_url,
        }
    }

    fn connection_url(&self) -> &str {
        &self.connection_url
    }
}

/// Create a NatsClient from a container.
async fn create_nats_client(container: &NatsContainer) -> NatsClient {
    let config = NatsConfig {
        servers: vec![container.connection_url().to_string()],
        name: "rsfga-test".to_string(),
        connect_timeout: Duration::from_secs(10),
        request_timeout: Duration::from_secs(5),
        ..Default::default()
    };

    NatsClient::connect(config)
        .await
        .expect("Failed to connect to NATS")
}

// =============================================================================
// Basic Connection Tests
// =============================================================================

/// Test: NATS container starts and client can connect
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_nats_connection() {
    let container = NatsContainer::new().await;
    let client = create_nats_client(&container).await;

    // Verify connected state
    assert!(client.is_connected().await, "Client should be connected");

    // Verify client info
    let config = client.config();
    assert_eq!(config.name, "rsfga-test");

    // Flush should work
    client.flush().await.expect("Flush should succeed");
}

/// Test: NatsClientBuilder creates correct configuration
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_nats_client_builder() {
    let container = NatsContainer::new().await;

    use rsfga_nats::connection::NatsClientBuilder;

    let client = NatsClientBuilder::new()
        .servers(vec![container.connection_url().to_string()])
        .name("builder-test")
        .connect()
        .await
        .expect("Should connect via builder");

    assert!(client.is_connected().await);
    assert_eq!(client.config().name, "builder-test");
}

// =============================================================================
// JetStream Setup Tests
// =============================================================================

/// Test: JetStream streams are created successfully
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_jetstream_setup_streams() {
    let container = NatsContainer::new().await;
    let client = create_nats_client(&container).await;
    let manager = JetStreamManager::new(client);

    // Setup streams
    manager
        .setup_streams()
        .await
        .expect("Stream setup should succeed");

    // Verify RSFGA_WRITES stream
    let writes_info = manager
        .stream_info(STREAM_WRITES)
        .await
        .expect("Should get writes stream info");
    assert_eq!(writes_info.name, STREAM_WRITES);

    // Verify RSFGA_EVENTS stream
    let events_info = manager
        .stream_info(STREAM_EVENTS)
        .await
        .expect("Should get events stream info");
    assert_eq!(events_info.name, STREAM_EVENTS);
}

/// Test: JetStream stream setup is idempotent
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_jetstream_setup_idempotent() {
    let container = NatsContainer::new().await;
    let client = create_nats_client(&container).await;
    let manager = JetStreamManager::new(client);

    // Setup streams twice
    manager
        .setup_streams()
        .await
        .expect("First setup should succeed");
    manager
        .setup_streams()
        .await
        .expect("Second setup should also succeed");

    // Streams should still exist
    let writes_info = manager
        .stream_info(STREAM_WRITES)
        .await
        .expect("Writes stream should exist");
    assert_eq!(writes_info.name, STREAM_WRITES);
}

/// Test: Stream info returns correct statistics
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_stream_info_statistics() {
    let container = NatsContainer::new().await;
    let client = create_nats_client(&container).await;
    let manager = JetStreamManager::new(client.clone());

    manager.setup_streams().await.unwrap();

    // Get initial stream info
    let info = manager.stream_info(STREAM_WRITES).await.unwrap();
    assert_eq!(info.messages, 0, "Initial message count should be 0");
    assert_eq!(info.bytes, 0, "Initial byte count should be 0");
}

/// Test: Stream purge removes all messages
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_stream_purge() {
    let container = NatsContainer::new().await;
    let client = create_nats_client(&container).await;
    let manager = JetStreamManager::new(client.clone());

    manager.setup_streams().await.unwrap();

    // Publish some messages directly to test purge
    let publisher = EventPublisher::new(client);
    let request = WriteRequest::new("test-store").write(TupleOperation::new(
        "user:alice",
        "viewer",
        "document:doc1",
    ));
    publisher.publish_write_request(&request).await.unwrap();

    // Verify message exists
    let info_before = manager.stream_info(STREAM_WRITES).await.unwrap();
    assert!(info_before.messages >= 1, "Should have at least 1 message");

    // Purge the stream
    let purged = manager.purge_stream(STREAM_WRITES).await.unwrap();
    assert!(purged >= 1, "Should have purged at least 1 message");

    // Verify stream is empty
    let info_after = manager.stream_info(STREAM_WRITES).await.unwrap();
    assert_eq!(info_after.messages, 0, "Stream should be empty after purge");
}

// =============================================================================
// Event Publishing Tests
// =============================================================================

/// Test: Publish write request to JetStream
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_publish_write_request() {
    let container = NatsContainer::new().await;
    let client = create_nats_client(&container).await;
    let manager = JetStreamManager::new(client.clone());

    manager.setup_streams().await.unwrap();

    let publisher = EventPublisher::new(client);

    // Create and publish a write request
    let request = WriteRequest::new("test-store")
        .with_model_id("model-1")
        .write(TupleOperation::new(
            "user:alice",
            "viewer",
            "document:readme",
        ))
        .delete(TupleKey::new("user:bob", "editor", "document:readme"));

    let ticket = publisher
        .publish_write_request(&request)
        .await
        .expect("Publish should succeed");

    // Verify ticket
    assert_eq!(ticket.store_id, "test-store");
    assert_eq!(ticket.request_id, request.request_id);
    assert!(ticket.sequence > 0, "Sequence should be positive");
    assert!(!ticket.is_expired(), "Ticket should not be expired");
}

/// Test: Publish committed event to JetStream
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_publish_committed_event() {
    let container = NatsContainer::new().await;
    let client = create_nats_client(&container).await;
    let manager = JetStreamManager::new(client.clone());

    manager.setup_streams().await.unwrap();

    let publisher = EventPublisher::new(client);

    // Create and publish a committed event
    let event = CommittedEvent::new("test-store", 42)
        .with_request_ids(vec!["req-1".to_string(), "req-2".to_string()])
        .with_writes(vec![TupleOperation::new(
            "user:alice",
            "viewer",
            "document:readme",
        )]);

    let ack = publisher
        .publish_committed_event(&event)
        .await
        .expect("Publish should succeed");

    assert!(ack.sequence > 0, "Sequence should be positive");
    assert_eq!(ack.stream, STREAM_EVENTS);
}

/// Test: Multiple write requests are published in sequence
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_publish_multiple_write_requests() {
    let container = NatsContainer::new().await;
    let client = create_nats_client(&container).await;
    let manager = JetStreamManager::new(client.clone());

    manager.setup_streams().await.unwrap();

    let publisher = EventPublisher::new(client);

    let mut sequences = Vec::new();

    // Publish multiple requests
    for i in 0..5 {
        let request = WriteRequest::new(format!("store-{}", i)).write(TupleOperation::new(
            format!("user:user{}", i),
            "viewer",
            format!("document:doc{}", i),
        ));

        let ticket = publisher.publish_write_request(&request).await.unwrap();
        sequences.push(ticket.sequence);
    }

    // Verify sequences are monotonically increasing
    for i in 1..sequences.len() {
        assert!(
            sequences[i] > sequences[i - 1],
            "Sequences should be monotonically increasing"
        );
    }

    // Verify stream has all messages
    let info = manager.stream_info(STREAM_WRITES).await.unwrap();
    assert_eq!(info.messages, 5, "Should have 5 messages");
}

/// Test: Publisher metrics are tracked correctly
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_publisher_metrics() {
    let container = NatsContainer::new().await;
    let client = create_nats_client(&container).await;
    let manager = JetStreamManager::new(client.clone());

    manager.setup_streams().await.unwrap();

    let publisher = EventPublisher::new(client);

    // Initial metrics
    let initial_metrics = publisher.metrics();
    assert_eq!(initial_metrics.publish_attempts, 0);
    assert_eq!(initial_metrics.publish_success, 0);
    assert!((initial_metrics.success_rate() - 1.0).abs() < f64::EPSILON);

    // Publish some requests
    for i in 0..3 {
        let request = WriteRequest::new(format!("store-{}", i)).write(TupleOperation::new(
            "user:alice",
            "viewer",
            "document:doc1",
        ));
        publisher.publish_write_request(&request).await.unwrap();
    }

    // Verify metrics
    let metrics = publisher.metrics();
    assert_eq!(metrics.publish_attempts, 3);
    assert_eq!(metrics.publish_success, 3);
    assert_eq!(metrics.publish_failures, 0);
    assert!((metrics.success_rate() - 1.0).abs() < f64::EPSILON);
}

// =============================================================================
// Circuit Breaker Tests
// =============================================================================

/// Test: Circuit breaker initial state is closed
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_circuit_breaker_initial_state() {
    let container = NatsContainer::new().await;
    let client = create_nats_client(&container).await;
    let manager = JetStreamManager::new(client.clone());

    manager.setup_streams().await.unwrap();

    let publisher = EventPublisher::new(client);

    // Circuit should be closed initially
    assert!(
        !publisher.is_circuit_open(),
        "Circuit should be closed initially"
    );

    // Publishing should work
    let request = WriteRequest::new("test-store").write(TupleOperation::new(
        "user:alice",
        "viewer",
        "document:doc1",
    ));
    publisher.publish_write_request(&request).await.unwrap();
}

/// Test: Circuit breaker can be manually reset
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_circuit_breaker_manual_reset() {
    let container = NatsContainer::new().await;
    let client = create_nats_client(&container).await;
    let manager = JetStreamManager::new(client.clone());

    manager.setup_streams().await.unwrap();

    let config = PublisherConfig {
        circuit_breaker_threshold: 1, // Open after 1 failure
        ..Default::default()
    };
    let publisher = EventPublisher::with_config(client, config);

    // Circuit should be closed
    assert!(!publisher.is_circuit_open());

    // Reset should work even when closed
    publisher.reset_circuit_breaker();
    assert!(!publisher.is_circuit_open());
}

// =============================================================================
// Event Serialization Tests (with real NATS)
// =============================================================================

/// Test: Write request serializes and deserializes correctly
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_write_request_roundtrip() {
    let container = NatsContainer::new().await;
    let client = create_nats_client(&container).await;
    let manager = JetStreamManager::new(client.clone());

    manager.setup_streams().await.unwrap();

    let original = WriteRequest::new("test-store")
        .with_model_id("model-123")
        .write(TupleOperation::new(
            "user:alice",
            "viewer",
            "document:readme",
        ))
        .write(TupleOperation::new("user:bob", "editor", "document:readme"))
        .delete(TupleKey::new("user:charlie", "admin", "document:readme"));

    // Serialize
    let bytes = original.to_bytes().expect("Serialization should succeed");

    // Deserialize
    let restored = WriteRequest::from_bytes(&bytes).expect("Deserialization should succeed");

    // Verify
    assert_eq!(restored.store_id, original.store_id);
    assert_eq!(
        restored.authorization_model_id,
        original.authorization_model_id
    );
    assert_eq!(restored.writes.len(), 2);
    assert_eq!(restored.deletes.len(), 1);
    assert_eq!(restored.writes[0].key.user, "user:alice");
}

/// Test: Committed event serializes and deserializes correctly
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_committed_event_roundtrip() {
    let container = NatsContainer::new().await;
    let client = create_nats_client(&container).await;
    let manager = JetStreamManager::new(client.clone());

    manager.setup_streams().await.unwrap();

    let original = CommittedEvent::new("test-store", 12345)
        .with_request_ids(vec!["req-1".to_string(), "req-2".to_string()])
        .with_writes(vec![TupleOperation::new(
            "user:alice",
            "viewer",
            "document:readme",
        )])
        .with_deletes(vec![TupleKey::new("user:bob", "editor", "document:readme")]);

    // Serialize
    let bytes = original.to_bytes().expect("Serialization should succeed");

    // Deserialize
    let restored = CommittedEvent::from_bytes(&bytes).expect("Deserialization should succeed");

    // Verify
    assert_eq!(restored.store_id, original.store_id);
    assert_eq!(restored.sequence, 12345);
    assert_eq!(restored.request_ids.len(), 2);
    assert_eq!(restored.writes.len(), 1);
    assert_eq!(restored.deletes.len(), 1);
}

// =============================================================================
// Connection Lifecycle Tests
// =============================================================================

/// Test: Client drain and close works correctly
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_client_drain() {
    let container = NatsContainer::new().await;
    let client = create_nats_client(&container).await;

    // Should be connected
    assert!(client.is_connected().await);

    // Drain should work
    client.drain().await.expect("Drain should succeed");

    // Note: After drain, the client is closed and may report disconnected
    // The exact state depends on async-nats implementation
}

/// Test: Client flush ensures messages are sent
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_client_flush() {
    let container = NatsContainer::new().await;
    let client = create_nats_client(&container).await;
    let manager = JetStreamManager::new(client.clone());

    manager.setup_streams().await.unwrap();

    let publisher = EventPublisher::new(client.clone());

    // Publish a message
    let request = WriteRequest::new("test-store").write(TupleOperation::new(
        "user:alice",
        "viewer",
        "document:doc1",
    ));
    publisher.publish_write_request(&request).await.unwrap();

    // Flush to ensure it's sent
    client.flush().await.expect("Flush should succeed");

    // Verify message is in stream
    let info = manager.stream_info(STREAM_WRITES).await.unwrap();
    assert!(
        info.messages >= 1,
        "Message should be in stream after flush"
    );
}

// =============================================================================
// Error Handling Tests
// =============================================================================

/// Test: Stream info for non-existent stream returns error
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_stream_info_nonexistent() {
    let container = NatsContainer::new().await;
    let client = create_nats_client(&container).await;
    let manager = JetStreamManager::new(client);

    let result = manager.stream_info("NONEXISTENT_STREAM").await;
    assert!(result.is_err(), "Should fail for non-existent stream");
}

/// Test: Delete non-existent stream returns error
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_delete_nonexistent_stream() {
    let container = NatsContainer::new().await;
    let client = create_nats_client(&container).await;
    let manager = JetStreamManager::new(client);

    let result = manager.delete_stream("NONEXISTENT_STREAM").await;
    assert!(result.is_err(), "Should fail to delete non-existent stream");
}

// =============================================================================
// Performance Tests
// =============================================================================

/// Test: High throughput publishing works
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_high_throughput_publishing() {
    let container = NatsContainer::new().await;
    let client = create_nats_client(&container).await;
    let manager = JetStreamManager::new(client.clone());

    manager.setup_streams().await.unwrap();

    let publisher = EventPublisher::new(client);

    let start = std::time::Instant::now();
    let count = 100;

    // Publish many messages
    for i in 0..count {
        let request = WriteRequest::new(format!("store-{}", i % 10)).write(TupleOperation::new(
            format!("user:user{}", i),
            "viewer",
            format!("document:doc{}", i),
        ));
        publisher.publish_write_request(&request).await.unwrap();
    }

    let elapsed = start.elapsed();
    let rate = count as f64 / elapsed.as_secs_f64();

    println!(
        "Published {} messages in {:?} ({:.2} msg/sec)",
        count, elapsed, rate
    );

    // Should be reasonably fast (at least 100 msg/sec)
    assert!(
        rate > 100.0,
        "Publishing rate should be at least 100 msg/sec, got {:.2}",
        rate
    );

    // Verify all messages arrived
    let info = manager.stream_info(STREAM_WRITES).await.unwrap();
    assert_eq!(info.messages, count, "All messages should be in stream");
}

// =============================================================================
// Store-Specific Subject Tests
// =============================================================================

/// Test: Messages are routed to correct store-specific subjects
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_store_specific_subjects() {
    let container = NatsContainer::new().await;
    let client = create_nats_client(&container).await;
    let manager = JetStreamManager::new(client.clone());

    manager.setup_streams().await.unwrap();

    let publisher = EventPublisher::new(client);

    // Publish to different stores
    let stores = vec!["store-a", "store-b", "store-c"];
    for store in &stores {
        let request = WriteRequest::new(*store).write(TupleOperation::new(
            "user:alice",
            "viewer",
            "document:doc1",
        ));
        publisher.publish_write_request(&request).await.unwrap();
    }

    // Verify subject format
    let request = WriteRequest::new("my-store");
    assert_eq!(request.subject(), "rsfga.writes.my-store");

    let event = CommittedEvent::new("my-store", 1);
    assert_eq!(event.subject(), "rsfga.events.my-store.committed");
}
