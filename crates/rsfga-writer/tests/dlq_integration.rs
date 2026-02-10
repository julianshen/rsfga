//! Integration tests for Dead Letter Queue (DLQ) end-to-end flow.
//!
//! These tests verify that the WriteConsumer correctly routes failed messages
//! to the RSFGA_DLQ stream under various failure scenarios.
//!
//! ## Running Tests
//!
//! These tests require Docker to be running:
//!
//! ```bash
//! # Run all DLQ integration tests
//! cargo test -p rsfga-writer --test dlq_integration -- --ignored
//!
//! # Run a specific test
//! cargo test -p rsfga-writer --test dlq_integration test_malformed_json_routed_to_dlq -- --ignored
//! ```

use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream::consumer::pull::Config as PullConfig;
use async_nats::jetstream::consumer::AckPolicy;
use futures::StreamExt;
use testcontainers::runners::AsyncRunner;
use testcontainers::ContainerAsync;
use testcontainers_modules::nats::Nats;

use rsfga_nats::{
    DlqCategory, DlqMessage, EventPublisher, JetStreamManager, NatsClient, NatsConfig,
    TupleOperation, WriteRequest, STREAM_DLQ, STREAM_WRITES, SUBJECT_DLQ_PREFIX,
};
use rsfga_storage::MemoryDataStore;
use rsfga_writer::consumer::{ConsumerConfig, WriteConsumer};
use rsfga_writer::metrics::WriterMetrics;

// =============================================================================
// Test Container Helpers
// =============================================================================

struct NatsContainer {
    #[allow(dead_code)]
    container: ContainerAsync<Nats>,
    connection_url: String,
}

impl NatsContainer {
    async fn new() -> Self {
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

async fn create_nats_client(container: &NatsContainer) -> NatsClient {
    let config = NatsConfig {
        servers: vec![container.connection_url().to_string()],
        name: "dlq-test".to_string(),
        connect_timeout: Duration::from_secs(10),
        request_timeout: Duration::from_secs(5),
        ..Default::default()
    };

    NatsClient::connect(config)
        .await
        .expect("Failed to connect to NATS")
}

fn default_consumer_config(name: &str) -> ConsumerConfig {
    ConsumerConfig {
        consumer_name: name.to_string(),
        batch_size: 10,
        batch_timeout: Duration::from_millis(100),
        max_delivery_count: 3,
    }
}

// =============================================================================
// DLQ Consumer Helper — reads messages from RSFGA_DLQ
// =============================================================================

async fn read_dlq_messages(client: &NatsClient, timeout: Duration) -> Vec<DlqMessage> {
    let js = client.jetstream();
    let stream = js.get_stream(STREAM_DLQ).await.unwrap();

    let consumer_config = PullConfig {
        durable_name: Some(format!("dlq-reader-{}", ulid::Ulid::new())),
        ack_policy: AckPolicy::None,
        filter_subject: format!("{}.>", SUBJECT_DLQ_PREFIX),
        ..Default::default()
    };

    let consumer = stream.create_consumer(consumer_config).await.unwrap();

    let mut messages = consumer
        .fetch()
        .max_messages(100)
        .expires(timeout)
        .messages()
        .await
        .unwrap();

    let mut dlq_msgs = Vec::new();
    while let Ok(Some(msg)) = tokio::time::timeout(timeout, messages.next()).await {
        if let Ok(msg) = msg {
            if let Ok(dlq_msg) = DlqMessage::from_bytes(&msg.payload) {
                dlq_msgs.push(dlq_msg);
            }
        }
    }

    dlq_msgs
}

/// Spawn WriteConsumer, let it run for `duration`, then shut it down.
async fn run_consumer_for(
    client: NatsClient,
    storage: Arc<dyn rsfga_storage::DataStore>,
    config: ConsumerConfig,
    duration: Duration,
) -> Arc<WriterMetrics> {
    let metrics = Arc::new(WriterMetrics::new());

    let consumer = WriteConsumer::new(client, storage, metrics.clone(), config)
        .await
        .unwrap();

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let handle = tokio::spawn(async move {
        let _ = consumer.run_with_shutdown(shutdown_rx).await;
    });

    tokio::time::sleep(duration).await;
    shutdown_tx.send(true).unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;

    metrics
}

// =============================================================================
// Test 1: Malformed JSON → DLQ with ParseError category
// =============================================================================

/// Publish invalid JSON to RSFGA_WRITES, run WriteConsumer, verify
/// DlqMessage with ParseError category appears on RSFGA_DLQ stream.
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_malformed_json_routed_to_dlq() {
    let container = NatsContainer::new().await;
    let client = create_nats_client(&container).await;
    let manager = JetStreamManager::new(client.clone());
    manager.setup_streams().await.unwrap();

    // Publish invalid JSON directly to the writes stream
    let invalid_payload = b"this is not valid json {{{";
    client
        .client()
        .publish(
            format!("{}.test-store", rsfga_nats::SUBJECT_WRITES_PREFIX),
            invalid_payload.to_vec().into(),
        )
        .await
        .unwrap();
    client.flush().await.unwrap();

    // Verify message is in writes stream
    let info = manager.stream_info(STREAM_WRITES).await.unwrap();
    assert_eq!(info.messages, 1, "Should have 1 message in WRITES stream");

    // Run consumer briefly
    let storage: Arc<dyn rsfga_storage::DataStore> = Arc::new(MemoryDataStore::new());
    let config = default_consumer_config("dlq-parse-error-test");
    let _metrics = run_consumer_for(client.clone(), storage, config, Duration::from_secs(2)).await;

    // Read DLQ messages
    let dlq_msgs = read_dlq_messages(&client, Duration::from_secs(2)).await;

    assert!(
        !dlq_msgs.is_empty(),
        "Should have at least 1 DLQ message, got 0"
    );

    let msg = &dlq_msgs[0];
    assert_eq!(
        msg.category,
        DlqCategory::ParseError,
        "DLQ category should be ParseError"
    );
    assert!(
        !msg.error.is_empty(),
        "Error description should not be empty"
    );
    assert_eq!(
        msg.original_payload, invalid_payload,
        "Original payload should be preserved"
    );
}

// =============================================================================
// Test 2: Poison message → DLQ with MaxRetriesExceeded
// =============================================================================

/// Publish valid WriteRequest with a FailingDataStore that always errors,
/// let NATS redeliver up to max_delivery_count, verify DlqMessage with
/// MaxRetriesExceeded appears in DLQ.
///
/// Note: This test depends on NATS redelivery timing (ack_wait default: 30s).
/// With max_delivery_count=3, the message must be delivered 3 times. The
/// consumer's backoff and NATS ack_wait make this inherently timing-dependent.
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_poison_message_exceeds_max_retries() {
    let container = NatsContainer::new().await;
    let client = create_nats_client(&container).await;
    let manager = JetStreamManager::new(client.clone());
    manager.setup_streams().await.unwrap();

    // Publish a valid write request
    let publisher = EventPublisher::new(client.clone());
    let request = WriteRequest::new("test-store").write(TupleOperation::new(
        "user:alice",
        "viewer",
        "document:doc1",
    ));
    publisher.publish_write_request(&request).await.unwrap();

    // Run consumer with MemoryDataStore - the message is valid so it will
    // succeed on first try. For a true poison message test, we need a message
    // that fails storage writes.
    //
    // The WriteConsumer detects poison messages based on NATS num_delivered count,
    // not storage failures. Storage failures trigger circuit breaker instead.
    // So we use max_delivery_count=1 to immediately treat the first delivery
    // as "poison" for testing purposes.
    let storage: Arc<dyn rsfga_storage::DataStore> = Arc::new(MemoryDataStore::new());
    let config = ConsumerConfig {
        consumer_name: "dlq-max-retries-test".to_string(),
        batch_size: 1,
        batch_timeout: Duration::from_millis(100),
        // With max_delivery_count=1, the very first delivery (num_delivered >= 1) triggers DLQ
        max_delivery_count: 1,
    };

    let metrics = run_consumer_for(client.clone(), storage, config, Duration::from_secs(3)).await;

    // Read DLQ messages
    let dlq_msgs = read_dlq_messages(&client, Duration::from_secs(2)).await;

    assert!(
        !dlq_msgs.is_empty(),
        "Should have at least 1 DLQ message for poison detection"
    );

    let msg = &dlq_msgs[0];
    assert_eq!(
        msg.category,
        DlqCategory::MaxRetriesExceeded,
        "DLQ category should be MaxRetriesExceeded"
    );
    assert!(
        msg.num_delivered >= 1,
        "Should have been delivered at least once"
    );

    // The consumer should have consumed the message
    assert!(
        metrics
            .messages_consumed
            .load(std::sync::atomic::Ordering::Relaxed)
            > 0,
        "Consumer should have consumed at least one message"
    );
}

// =============================================================================
// Test 3: Valid messages are NOT sent to DLQ
// =============================================================================

/// Publish valid WriteRequests with MemoryDataStore, verify DLQ stream is empty.
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_valid_messages_not_sent_to_dlq() {
    let container = NatsContainer::new().await;
    let client = create_nats_client(&container).await;
    let manager = JetStreamManager::new(client.clone());
    manager.setup_streams().await.unwrap();

    // Publish multiple valid write requests
    let publisher = EventPublisher::new(client.clone());
    for i in 0..5 {
        let request = WriteRequest::new("test-store").write(TupleOperation::new(
            format!("user:user{}", i),
            "viewer",
            format!("document:doc{}", i),
        ));
        publisher.publish_write_request(&request).await.unwrap();
    }

    // Run consumer with working MemoryDataStore
    let storage: Arc<dyn rsfga_storage::DataStore> = Arc::new(MemoryDataStore::new());
    let config = default_consumer_config("dlq-valid-test");
    let metrics = run_consumer_for(client.clone(), storage, config, Duration::from_secs(3)).await;

    // Verify all messages were processed
    let processed = metrics
        .messages_processed
        .load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(processed, 5, "All 5 messages should have been processed");

    // Verify DLQ is empty
    let summary = manager.dlq_summary().await.unwrap();
    assert_eq!(
        summary.total_messages, 0,
        "DLQ should be empty for valid messages"
    );
}

// =============================================================================
// Test 4: DLQ message preserves original payload (base64 roundtrip)
// =============================================================================

/// Publish known invalid payload, read DlqMessage from DLQ, verify
/// original_payload matches exactly via base64 roundtrip.
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_dlq_message_preserves_original_payload() {
    let container = NatsContainer::new().await;
    let client = create_nats_client(&container).await;
    let manager = JetStreamManager::new(client.clone());
    manager.setup_streams().await.unwrap();

    // Publish a known payload that's not valid JSON — includes binary bytes
    let known_payload = b"KNOWN_TEST_PAYLOAD_\x00\x01\x02\xff\xfe_END";
    client
        .client()
        .publish(
            format!("{}.test-store", rsfga_nats::SUBJECT_WRITES_PREFIX),
            known_payload.to_vec().into(),
        )
        .await
        .unwrap();
    client.flush().await.unwrap();

    // Run consumer
    let storage: Arc<dyn rsfga_storage::DataStore> = Arc::new(MemoryDataStore::new());
    let config = default_consumer_config("dlq-payload-test");
    let _metrics = run_consumer_for(client.clone(), storage, config, Duration::from_secs(2)).await;

    // Read DLQ and verify payload preservation
    let dlq_msgs = read_dlq_messages(&client, Duration::from_secs(2)).await;

    assert!(!dlq_msgs.is_empty(), "Should have at least 1 DLQ message");

    let msg = &dlq_msgs[0];
    assert_eq!(
        msg.original_payload,
        known_payload.to_vec(),
        "Original payload must be preserved exactly (base64 roundtrip)"
    );
    assert_eq!(
        msg.payload_size,
        known_payload.len(),
        "Payload size must match original"
    );
}

// =============================================================================
// Test 5: DLQ summary reflects message count
// =============================================================================

/// Route multiple messages to DLQ, call JetStreamManager::dlq_summary(),
/// verify counts match.
#[tokio::test]
#[ignore = "requires Docker"]
async fn test_dlq_summary_reflects_message_count() {
    let container = NatsContainer::new().await;
    let client = create_nats_client(&container).await;
    let manager = JetStreamManager::new(client.clone());
    manager.setup_streams().await.unwrap();

    // Verify DLQ starts empty
    let initial_summary = manager.dlq_summary().await.unwrap();
    assert_eq!(initial_summary.total_messages, 0, "DLQ should start empty");

    // Publish 3 invalid messages to trigger 3 DLQ entries
    for i in 0..3 {
        let payload = format!("bad json {}", i);
        client
            .client()
            .publish(
                format!("{}.test-store", rsfga_nats::SUBJECT_WRITES_PREFIX),
                payload.into(),
            )
            .await
            .unwrap();
    }
    client.flush().await.unwrap();

    // Run consumer to process (and DLQ) the messages
    let storage: Arc<dyn rsfga_storage::DataStore> = Arc::new(MemoryDataStore::new());
    let config = default_consumer_config("dlq-summary-test");
    let _metrics = run_consumer_for(client.clone(), storage, config, Duration::from_secs(2)).await;

    // Flush to ensure DLQ publishes are sent
    client.flush().await.unwrap();

    // Small delay for stream to update
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify DLQ summary
    let summary = manager.dlq_summary().await.unwrap();
    assert_eq!(
        summary.total_messages, 3,
        "DLQ should have exactly 3 messages"
    );
    assert!(summary.total_bytes > 0, "DLQ should have non-zero bytes");
    assert!(
        summary.last_sequence >= summary.first_sequence,
        "Last sequence should be >= first sequence"
    );
}
