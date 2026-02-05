//! NATS JetStream consumer for write events.
//!
//! This module implements a pull consumer that:
//! - Fetches messages from RSFGA_WRITES stream
//! - Batches messages by store_id
//! - Writes to storage with idempotent upserts
//! - Publishes CommittedEvent to RSFGA_EVENTS
//! - Acks messages after successful commit and event publish
//!
//! # Delivery Semantics
//!
//! This consumer implements **at-least-once** delivery:
//!
//! - Messages are only acknowledged after BOTH storage write AND event publish succeed
//! - If event publishing fails after retries, messages are NOT acknowledged
//! - Unacknowledged messages will be redelivered by NATS after ack_wait timeout
//! - Storage writes use ON CONFLICT (upsert) for idempotency on redelivery
//!
//! This means:
//! - Writes may be applied more than once (but idempotently)
//! - CommittedEvents may be published more than once for the same batch
//! - Downstream consumers should handle duplicate events

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_nats::jetstream::consumer::pull::Config as PullConfig;
use async_nats::jetstream::consumer::AckPolicy;
use async_nats::jetstream::Message as JsMessage;
use futures::StreamExt;
use tracing::{debug, error, info, warn};

use rsfga_nats::{
    CommittedEvent, EventPublisher, NatsClient, TupleKey, TupleOperation, WriteRequest,
    STREAM_WRITES,
};
use rsfga_storage::{DataStore, StoredTuple};

use crate::error::{Result, WriterError};
use crate::metrics::WriterMetrics;

/// Minimum allowed batch size.
pub const MIN_BATCH_SIZE: usize = 1;
/// Maximum allowed batch size (prevents memory exhaustion).
pub const MAX_BATCH_SIZE: usize = 10_000;
/// Minimum batch timeout.
pub const MIN_BATCH_TIMEOUT: Duration = Duration::from_millis(10);
/// Maximum batch timeout.
pub const MAX_BATCH_TIMEOUT: Duration = Duration::from_secs(60);

/// Consumer configuration.
#[derive(Debug, Clone)]
pub struct ConsumerConfig {
    /// Consumer name (durable).
    pub consumer_name: String,
    /// Batch size (number of messages before flush).
    pub batch_size: usize,
    /// Batch timeout (flush if no messages for this duration).
    pub batch_timeout: Duration,
    /// Number of parallel workers for processing batches.
    ///
    /// **Note:** Currently unused. Reserved for future parallel batch processing.
    /// The consumer processes batches sequentially for simplicity and correctness.
    #[allow(dead_code)]
    pub workers: usize,
}

impl ConsumerConfig {
    /// Validate configuration values.
    pub fn validate(&self) -> Result<()> {
        if self.consumer_name.is_empty() {
            return Err(WriterError::Config("consumer_name cannot be empty".to_string()));
        }
        if self.batch_size < MIN_BATCH_SIZE || self.batch_size > MAX_BATCH_SIZE {
            return Err(WriterError::Config(format!(
                "batch_size must be between {} and {}, got {}",
                MIN_BATCH_SIZE, MAX_BATCH_SIZE, self.batch_size
            )));
        }
        if self.batch_timeout < MIN_BATCH_TIMEOUT || self.batch_timeout > MAX_BATCH_TIMEOUT {
            return Err(WriterError::Config(format!(
                "batch_timeout must be between {:?} and {:?}, got {:?}",
                MIN_BATCH_TIMEOUT, MAX_BATCH_TIMEOUT, self.batch_timeout
            )));
        }
        Ok(())
    }
}

/// Pending message with ack handle.
struct PendingMessage {
    /// The write request.
    request: WriteRequest,
    /// Ack handle for the message.
    ack: JsMessage,
}

/// Batch of writes grouped by store_id.
struct StoreBatch {
    store_id: String,
    writes: Vec<StoredTuple>,
    deletes: Vec<StoredTuple>,
    request_ids: Vec<String>,
    messages: Vec<JsMessage>,
}

/// Write consumer for NATS JetStream.
pub struct WriteConsumer {
    client: NatsClient,
    storage: Arc<dyn DataStore>,
    publisher: EventPublisher,
    metrics: Arc<WriterMetrics>,
    config: ConsumerConfig,
    /// Global sequence counter for CommittedEvent ordering.
    ///
    /// # Known Limitation
    ///
    /// This counter is global (not per-store) and resets on daemon restart.
    /// This means:
    /// - Sequence numbers may reset after restart, breaking ordering guarantees
    /// - Multiple stores share the same counter (not isolated)
    ///
    /// For production deployments requiring strict ordering:
    /// - Consider persisting sequence per store_id in storage
    /// - Or use NATS stream sequence numbers directly
    ///
    /// TODO: Implement per-store persistent sequence tracking (see ADR-017)
    sequence_counter: AtomicU64,
}

impl WriteConsumer {
    /// Create a new write consumer.
    pub async fn new(
        client: NatsClient,
        storage: Arc<dyn DataStore>,
        metrics: Arc<WriterMetrics>,
        config: ConsumerConfig,
    ) -> Result<Self> {
        // Validate configuration
        config.validate()?;

        let publisher = EventPublisher::new(client.clone());

        Ok(Self {
            client,
            storage,
            publisher,
            metrics,
            config,
            sequence_counter: AtomicU64::new(0),
        })
    }

    /// Get next sequence number.
    fn next_sequence(&self) -> u64 {
        self.sequence_counter.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Run the consumer loop.
    pub async fn run(&self) -> Result<()> {
        // Create or get the durable pull consumer
        let consumer = self.create_consumer().await?;

        info!(
            consumer_name = %self.config.consumer_name,
            batch_size = self.config.batch_size,
            batch_timeout_ms = self.config.batch_timeout.as_millis(),
            "Consumer started"
        );

        // Main consumer loop - fetch messages in batches
        let mut pending: HashMap<String, Vec<PendingMessage>> = HashMap::new();
        let mut batch_timer = Instant::now();

        loop {
            // Fetch a batch of messages
            let mut messages = consumer
                .fetch()
                .max_messages(self.config.batch_size)
                .expires(self.config.batch_timeout)
                .messages()
                .await
                .map_err(|e| WriterError::Consumer(format!("Failed to fetch messages: {}", e)))?;

            // Process fetched messages
            while let Some(msg_result) = messages.next().await {
                match msg_result {
                    Ok(msg) => {
                        self.metrics.record_message_consumed();

                        // Parse the write request
                        match serde_json::from_slice::<WriteRequest>(&msg.payload) {
                            Ok(request) => {
                                let store_id = request.store_id.clone();
                                pending
                                    .entry(store_id)
                                    .or_default()
                                    .push(PendingMessage { request, ack: msg });
                            }
                            Err(e) => {
                                warn!(error = %e, "Failed to parse write request, acking to skip");
                                if let Err(e) = msg.ack().await {
                                    error!(error = %e, "Failed to ack invalid message");
                                }
                                self.metrics.record_message_failed();
                            }
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Error receiving message");
                    }
                }
            }

            // Process pending batches if we have any
            let should_flush =
                !pending.is_empty() && batch_timer.elapsed() >= self.config.batch_timeout;
            let total_pending: usize = pending.values().map(|v| v.len()).sum();
            let batch_full = total_pending >= self.config.batch_size;

            if should_flush || batch_full {
                self.process_pending_batches(&mut pending).await?;
                batch_timer = Instant::now();
            }
        }
    }

    /// Process all pending batches.
    async fn process_pending_batches(
        &self,
        pending: &mut HashMap<String, Vec<PendingMessage>>,
    ) -> Result<()> {
        for (store_id, messages) in pending.drain() {
            if messages.is_empty() {
                continue;
            }

            let batch = self.create_batch(store_id, messages);
            self.metrics.record_batch_processed(batch.messages.len());

            if let Err(e) = self.process_batch(batch).await {
                error!(error = %e, "Failed to process batch");
            }
        }
        Ok(())
    }

    /// Process a single batch.
    async fn process_batch(&self, batch: StoreBatch) -> Result<()> {
        let start = Instant::now();

        debug!(
            store_id = %batch.store_id,
            writes = batch.writes.len(),
            deletes = batch.deletes.len(),
            messages = batch.messages.len(),
            "Processing batch"
        );

        // Write to storage (idempotent using ON CONFLICT)
        let write_count = batch.writes.len();
        let delete_count = batch.deletes.len();

        self.storage
            .write_tuples(&batch.store_id, batch.writes, batch.deletes)
            .await?;

        self.metrics.record_storage_latency(start);
        self.metrics.record_tuples_written(write_count);
        self.metrics.record_tuples_deleted(delete_count);

        // Publish CommittedEvent to RSFGA_EVENTS with retry
        let sequence = self.next_sequence();
        let event =
            CommittedEvent::new(&batch.store_id, sequence).with_request_ids(batch.request_ids);

        let mut event_published = false;
        let mut last_error = None;

        // Retry event publishing up to 3 times with exponential backoff
        for attempt in 0..3 {
            match self.publisher.publish_committed_event(&event).await {
                Ok(_) => {
                    self.metrics.record_event_published();
                    event_published = true;
                    break;
                }
                Err(e) => {
                    last_error = Some(e);
                    if attempt < 2 {
                        let delay = Duration::from_millis(100 * (1 << attempt));
                        warn!(
                            store_id = %batch.store_id,
                            attempt = attempt + 1,
                            delay_ms = delay.as_millis(),
                            "Failed to publish committed event, retrying"
                        );
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }

        // Only ack messages if event was successfully published
        // If event publishing fails after retries, messages will be redelivered
        if event_published {
            for msg in batch.messages {
                if let Err(e) = msg.ack().await {
                    error!(error = %e, "Failed to ack message");
                } else {
                    self.metrics.record_message_processed();
                }
            }
        } else {
            error!(
                store_id = %batch.store_id,
                error = ?last_error,
                "Failed to publish committed event after retries, not acking messages"
            );
            self.metrics.record_event_publish_failure();
        }

        debug!(
            store_id = %batch.store_id,
            duration_ms = start.elapsed().as_millis(),
            "Batch processed"
        );

        Ok(())
    }

    /// Create or get the durable pull consumer.
    async fn create_consumer(
        &self,
    ) -> Result<
        async_nats::jetstream::consumer::Consumer<async_nats::jetstream::consumer::pull::Config>,
    > {
        let js = self.client.jetstream();
        let stream = js
            .get_stream(STREAM_WRITES)
            .await
            .map_err(|e| WriterError::Consumer(format!("Failed to get stream: {}", e)))?;

        // Try to get existing consumer, or create new one
        match stream.get_consumer(&self.config.consumer_name).await {
            Ok(consumer) => {
                info!(
                    consumer_name = %self.config.consumer_name,
                    "Using existing consumer"
                );
                Ok(consumer)
            }
            Err(_) => {
                info!(
                    consumer_name = %self.config.consumer_name,
                    "Creating new consumer"
                );
                let config = PullConfig {
                    durable_name: Some(self.config.consumer_name.clone()),
                    ack_policy: AckPolicy::Explicit,
                    max_ack_pending: 10000,
                    ..Default::default()
                };
                stream
                    .create_consumer(config)
                    .await
                    .map_err(|e| WriterError::Consumer(format!("Failed to create consumer: {}", e)))
            }
        }
    }

    /// Create a StoreBatch from pending messages.
    fn create_batch(&self, store_id: String, messages: Vec<PendingMessage>) -> StoreBatch {
        let mut writes = Vec::new();
        let mut deletes = Vec::new();
        let mut request_ids = Vec::new();
        let mut ack_handles = Vec::new();

        for msg in messages {
            request_ids.push(msg.request.request_id.clone());
            ack_handles.push(msg.ack);

            // Convert writes
            for op in msg.request.writes {
                if let Some(tuple) = convert_operation_to_tuple(&op) {
                    writes.push(tuple);
                }
            }

            // Convert deletes
            for key in msg.request.deletes {
                if let Some(tuple) = convert_key_to_tuple(&key) {
                    deletes.push(tuple);
                }
            }
        }

        StoreBatch {
            store_id,
            writes,
            deletes,
            request_ids,
            messages: ack_handles,
        }
    }
}

/// Convert a TupleOperation to StoredTuple.
fn convert_operation_to_tuple(op: &TupleOperation) -> Option<StoredTuple> {
    let key = &op.key;

    // Parse user: "user:alice" or "team:eng#member"
    let (user_type, user_id, user_relation) = parse_user(&key.user)?;

    // Parse object: "document:readme"
    let (object_type, object_id) = parse_object(&key.object)?;

    // Extract condition if present
    let (condition_name, condition_context) = if let Some(ref cond) = op.condition {
        let context = if cond.context.is_empty() {
            None
        } else {
            Some(cond.context.clone())
        };
        (Some(cond.name.clone()), context)
    } else {
        (None, None)
    };

    Some(StoredTuple {
        object_type: object_type.to_string(),
        object_id: object_id.to_string(),
        relation: key.relation.clone(),
        user_type: user_type.to_string(),
        user_id: user_id.to_string(),
        user_relation: user_relation.map(|s| s.to_string()),
        condition_name,
        condition_context,
        created_at: Some(chrono::Utc::now()),
    })
}

/// Convert a TupleKey to StoredTuple.
fn convert_key_to_tuple(key: &TupleKey) -> Option<StoredTuple> {
    // Parse user: "user:alice" or "team:eng#member"
    let (user_type, user_id, user_relation) = parse_user(&key.user)?;

    // Parse object: "document:readme"
    let (object_type, object_id) = parse_object(&key.object)?;

    Some(StoredTuple {
        object_type: object_type.to_string(),
        object_id: object_id.to_string(),
        relation: key.relation.clone(),
        user_type: user_type.to_string(),
        user_id: user_id.to_string(),
        user_relation: user_relation.map(|s| s.to_string()),
        condition_name: None,
        condition_context: None,
        created_at: None,
    })
}

/// Parse a user string into (type, id, optional_relation).
///
/// Formats:
/// - "user:alice" -> ("user", "alice", None)
/// - "team:eng#member" -> ("team", "eng", Some("member"))
///
/// TODO: Extract to shared crate (rsfga-domain?) to avoid duplication with rsfga-api/src/utils.rs
fn parse_user(user: &str) -> Option<(&str, &str, Option<&str>)> {
    let (type_part, rest) = user.split_once(':')?;

    if let Some((id, relation)) = rest.split_once('#') {
        Some((type_part, id, Some(relation)))
    } else {
        Some((type_part, rest, None))
    }
}

/// Parse an object string into (type, id).
///
/// Format: "document:readme" -> ("document", "readme")
///
/// TODO: Extract to shared crate (rsfga-domain?) to avoid duplication with rsfga-api/src/utils.rs
fn parse_object(object: &str) -> Option<(&str, &str)> {
    object.split_once(':')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_user_simple() {
        let result = parse_user("user:alice");
        assert_eq!(result, Some(("user", "alice", None)));
    }

    #[test]
    fn test_parse_user_with_relation() {
        let result = parse_user("team:eng#member");
        assert_eq!(result, Some(("team", "eng", Some("member"))));
    }

    #[test]
    fn test_parse_user_invalid() {
        let result = parse_user("invalid");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_object() {
        let result = parse_object("document:readme");
        assert_eq!(result, Some(("document", "readme")));
    }

    #[test]
    fn test_parse_object_invalid() {
        let result = parse_object("invalid");
        assert_eq!(result, None);
    }
}
