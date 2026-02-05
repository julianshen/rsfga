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
//! - If event publishing fails after 3 retries, messages are NOT acknowledged
//! - Unacknowledged messages will be redelivered by NATS after ack_wait timeout
//! - Storage writes use ON CONFLICT (upsert) for idempotency on redelivery
//!
//! This means:
//! - Writes may be applied more than once (but idempotently)
//! - CommittedEvents may be published more than once for the same batch
//! - Downstream consumers should handle duplicate events
//!
//! # Error Handling
//!
//! ## Parse Failures
//! - Malformed messages are logged with details and acked to prevent queue blocking
//! - These are tracked via `messages_failed` metric
//! - Parse failures are published to Dead Letter Queue (DLQ) for analysis
//!
//! ## Event Publish Failures
//! - Retried 3 times with exponential backoff (100ms, 200ms, 400ms)
//! - If all retries fail, messages are NOT acked (will be redelivered)
//! - Tracked via `event_publish_failures` metric
//! - **Note**: This can cause duplicate storage writes on redelivery (idempotent)
//!
//! ## Consecutive Failures
//! - Consumer applies exponential backoff (100ms to 30s) on consecutive failures
//! - This prevents tight loop on persistent errors (e.g., storage unavailable)
//! - Backoff resets on successful batch processing

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_nats::jetstream::consumer::pull::Config as PullConfig;
use async_nats::jetstream::consumer::AckPolicy;
use async_nats::jetstream::Message as JsMessage;
use futures::StreamExt;
use tracing::{debug, error, info, warn};

use rsfga_nats::{
    CommittedEvent, EventPublisher, NatsClient, TupleKey, TupleOperation, WriteRequest,
    STREAM_WRITES, SUBJECT_DLQ_PREFIX,
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
/// Maximum pending queue size (prevents memory exhaustion under load).
/// When queue reaches this size, processing is forced before fetching more.
pub const MAX_PENDING_QUEUE_SIZE: usize = 50_000;

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
            return Err(WriterError::Config(
                "consumer_name cannot be empty".to_string(),
            ));
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
    /// NATS stream sequence number (persistent, unique).
    stream_sequence: u64,
}

/// Batch of writes grouped by store_id.
struct StoreBatch {
    store_id: String,
    writes: Vec<StoredTuple>,
    deletes: Vec<StoredTuple>,
    request_ids: Vec<String>,
    messages: Vec<JsMessage>,
    /// Maximum NATS stream sequence in this batch (used for CommittedEvent).
    max_stream_sequence: u64,
}

/// Write consumer for NATS JetStream.
pub struct WriteConsumer {
    client: NatsClient,
    storage: Arc<dyn DataStore>,
    publisher: EventPublisher,
    metrics: Arc<WriterMetrics>,
    config: ConsumerConfig,
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
        })
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
        let mut consecutive_failures: u32 = 0;
        const MAX_BACKOFF_MS: u64 = 30_000; // 30 seconds max backoff

        loop {
            // Apply backoff if we've had consecutive failures
            if consecutive_failures > 0 {
                let backoff_ms = std::cmp::min(
                    100 * (1u64 << std::cmp::min(consecutive_failures, 10)),
                    MAX_BACKOFF_MS,
                );
                warn!(
                    consecutive_failures,
                    backoff_ms, "Backing off due to consecutive failures"
                );
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
            }

            // Fetch a batch of messages
            let mut messages = consumer
                .fetch()
                .max_messages(self.config.batch_size)
                .expires(self.config.batch_timeout)
                .messages()
                .await
                .map_err(|e| WriterError::Consumer(format!("Failed to fetch messages: {}", e)))?;

            // Track if this batch had any failures
            let mut batch_had_failures = false;

            // Process fetched messages
            while let Some(msg_result) = messages.next().await {
                match msg_result {
                    Ok(msg) => {
                        self.metrics.record_message_consumed();

                        // Parse the write request
                        match serde_json::from_slice::<WriteRequest>(&msg.payload) {
                            Ok(request) => {
                                let store_id = request.store_id.clone();
                                // Get NATS stream sequence for persistent ordering
                                let stream_sequence =
                                    msg.info().map(|info| info.stream_sequence).unwrap_or(0);
                                pending.entry(store_id).or_default().push(PendingMessage {
                                    request,
                                    ack: msg,
                                    stream_sequence,
                                });
                            }
                            Err(e) => {
                                // Parse failures are typically permanent (malformed data)
                                // Publish to DLQ for analysis, then ack to prevent blocking
                                let stream_seq =
                                    msg.info().map(|info| info.stream_sequence).unwrap_or(0);

                                error!(
                                    error = %e,
                                    payload_size = msg.payload.len(),
                                    subject = %msg.subject,
                                    stream_sequence = stream_seq,
                                    "Failed to parse write request - sending to DLQ"
                                );

                                // Publish to Dead Letter Queue
                                let dlq_subject =
                                    format!("{}.parse_error.{}", SUBJECT_DLQ_PREFIX, stream_seq);
                                if let Err(dlq_err) = self
                                    .client
                                    .client()
                                    .publish(dlq_subject.clone(), msg.payload.clone())
                                    .await
                                {
                                    warn!(
                                        error = %dlq_err,
                                        subject = %dlq_subject,
                                        "Failed to publish to DLQ"
                                    );
                                } else {
                                    debug!(subject = %dlq_subject, "Message sent to DLQ");
                                }

                                batch_had_failures = true;

                                // Ack to prevent poison message blocking queue
                                if let Err(ack_err) = msg.ack().await {
                                    error!(error = %ack_err, "Failed to ack invalid message");
                                }
                                self.metrics.record_message_failed();
                            }
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Error receiving message");
                        batch_had_failures = true;
                    }
                }
            }

            // Process pending batches if we have any
            let total_pending: usize = pending.values().map(|v| v.len()).sum();
            let should_flush =
                !pending.is_empty() && batch_timer.elapsed() >= self.config.batch_timeout;
            let batch_full = total_pending >= self.config.batch_size;
            // Force processing if queue is at capacity to prevent memory exhaustion
            let queue_at_capacity = total_pending >= MAX_PENDING_QUEUE_SIZE;

            if queue_at_capacity {
                warn!(
                    total_pending,
                    max_pending = MAX_PENDING_QUEUE_SIZE,
                    "Pending queue at capacity, forcing batch processing"
                );
            }

            if should_flush || batch_full || queue_at_capacity {
                match self.process_pending_batches(&mut pending).await {
                    Ok(_) => {
                        // Reset backoff on successful batch processing
                        consecutive_failures = 0;
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to process batches");
                        batch_had_failures = true;
                    }
                }
                batch_timer = Instant::now();
            }

            // Update consecutive failures counter for backoff
            if batch_had_failures {
                consecutive_failures = consecutive_failures.saturating_add(1);
            } else if !pending.is_empty() || total_pending > 0 {
                // Only reset if we actually processed something successfully
                consecutive_failures = 0;
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
        // Use NATS stream sequence for persistent, unique ordering
        let event = CommittedEvent::new(&batch.store_id, batch.max_stream_sequence)
            .with_request_ids(batch.request_ids);

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
        let mut max_stream_sequence: u64 = 0;

        for msg in messages {
            request_ids.push(msg.request.request_id.clone());
            ack_handles.push(msg.ack);
            // Track maximum stream sequence for persistent ordering
            max_stream_sequence = max_stream_sequence.max(msg.stream_sequence);

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
            max_stream_sequence,
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
