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
//! ## Storage Failures and Circuit Breaker
//! - Consumer applies exponential backoff (100ms to 30s) on consecutive failures
//! - After STORAGE_CIRCUIT_BREAKER_THRESHOLD (5) consecutive storage failures:
//!   - Circuit opens and message fetching stops
//!   - Periodic health checks probe storage availability
//!   - Circuit closes when storage health check succeeds
//! - This prevents overwhelming a failing storage backend with retry traffic
//!
//! **Note**: Circuit breaker state is local to the consumer process. On restart,
//! state resets even if storage is still down. This is by design for simplicity:
//! - First health check after restart will detect ongoing storage issues
//! - Circuit will re-open quickly if storage is still unavailable
//! - Shared state (e.g., Redis) adds complexity and failure modes
//! - Metrics (rsfga_writer_circuit_breaker_open) provide visibility
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
use metrics::gauge;
use tracing::{debug, error, info, warn};

use rsfga_nats::{
    utils::{parse_object, parse_user},
    CommittedEvent, DlqMessage, EventPublisher, NatsClient, TupleKey, TupleOperation, WriteRequest,
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
/// Maximum pending queue size (prevents memory exhaustion under load).
/// When queue reaches this size, processing is forced before fetching more.
pub const MAX_PENDING_QUEUE_SIZE: usize = 50_000;
/// Number of consecutive storage failures before opening circuit breaker.
/// When reached, the consumer stops fetching and waits for storage recovery.
pub const STORAGE_CIRCUIT_BREAKER_THRESHOLD: u32 = 5;
/// How often to probe storage health when circuit is open.
pub const STORAGE_HEALTH_PROBE_INTERVAL: Duration = Duration::from_secs(10);
/// Timeout for DLQ publish operations. Prevents the consumer from blocking
/// indefinitely if NATS is slow or unresponsive during DLQ writes.
pub const DLQ_PUBLISH_TIMEOUT: Duration = Duration::from_secs(5);
/// Default maximum delivery attempts before a message is considered poison.
pub const DEFAULT_MAX_DELIVERY_COUNT: u64 = 5;

/// Consumer configuration.
#[derive(Debug, Clone)]
pub struct ConsumerConfig {
    /// Consumer name (durable).
    pub consumer_name: String,
    /// Batch size (number of messages before flush).
    pub batch_size: usize,
    /// Batch timeout (flush if no messages for this duration).
    pub batch_timeout: Duration,
    /// Maximum delivery attempts before a message is sent to DLQ as poison.
    /// Set to 0 to disable poison message detection.
    pub max_delivery_count: u64,
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

    /// Run the consumer loop until shutdown is signaled.
    ///
    /// # Graceful Shutdown
    ///
    /// When `shutdown` receives a value:
    /// 1. Stops fetching new messages from NATS
    /// 2. Processes all pending messages in the queue
    /// 3. Acknowledges all successfully processed messages
    /// 4. Returns gracefully
    ///
    /// This ensures no messages are lost during shutdown.
    pub async fn run_with_shutdown(
        &self,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Result<()> {
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
        let mut storage_failures: u32 = 0;
        let mut circuit_open = false;
        let mut last_health_probe = Instant::now();
        const MAX_BACKOFF_MS: u64 = 30_000; // 30 seconds max backoff

        loop {
            // Check for shutdown signal
            if *shutdown.borrow() {
                info!("Shutdown signal received, processing pending messages");
                break;
            }

            // Circuit breaker: if open, probe storage health before resuming
            if circuit_open {
                if last_health_probe.elapsed() >= STORAGE_HEALTH_PROBE_INTERVAL {
                    info!("Circuit open - probing storage health");
                    match self.storage.health_check().await {
                        Ok(status) if status.healthy => {
                            info!("Storage health check passed - closing circuit");
                            circuit_open = false;
                            storage_failures = 0;
                            consecutive_failures = 0;
                            self.metrics.update_circuit_breaker_state(false);
                        }
                        Ok(status) => {
                            warn!(
                                message = status.message,
                                "Storage health check failed - circuit remains open"
                            );
                            last_health_probe = Instant::now();
                        }
                        Err(e) => {
                            warn!(
                                error = %e,
                                "Storage health check error - circuit remains open"
                            );
                            last_health_probe = Instant::now();
                        }
                    }
                }

                if circuit_open {
                    // Sleep until next probe time
                    let remaining =
                        STORAGE_HEALTH_PROBE_INTERVAL.saturating_sub(last_health_probe.elapsed());
                    if !remaining.is_zero() {
                        tokio::time::sleep(remaining).await;
                    }
                    continue;
                }
            }

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

            // Check queue size BEFORE fetching to prevent memory exhaustion
            let total_pending: usize = pending.values().map(|v| v.len()).sum();
            if total_pending >= MAX_PENDING_QUEUE_SIZE {
                warn!(
                    total_pending,
                    max_pending = MAX_PENDING_QUEUE_SIZE,
                    "Pending queue at capacity - processing before fetching more"
                );
                // Process existing queue before fetching more
                match self.process_pending_batches(&mut pending).await {
                    Ok(_) => {
                        consecutive_failures = 0;
                        storage_failures = 0;
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to process batches at capacity");
                        if matches!(e, WriterError::Storage(_)) {
                            storage_failures = storage_failures.saturating_add(1);
                            self.metrics.record_storage_failure();
                            if storage_failures >= STORAGE_CIRCUIT_BREAKER_THRESHOLD {
                                error!(
                                    storage_failures,
                                    threshold = STORAGE_CIRCUIT_BREAKER_THRESHOLD,
                                    "Storage circuit breaker OPEN"
                                );
                                circuit_open = true;
                                last_health_probe = Instant::now();
                                self.metrics.update_circuit_breaker_state(true);
                            }
                        }
                        consecutive_failures = consecutive_failures.saturating_add(1);
                    }
                }
                batch_timer = Instant::now();
                continue; // Re-check queue size before fetching
            }

            // Update pending queue size metric
            gauge!("rsfga_writer_pending_queue_size").set(total_pending as f64);

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

                        // Get NATS stream sequence and delivery count
                        let (stream_sequence, num_delivered) = match msg.info() {
                            Ok(info) => (info.stream_sequence, info.delivered),
                            Err(e) => {
                                // This should never happen for JetStream messages
                                // If it does, don't ack - let message be redelivered
                                error!(
                                    error = %e,
                                    subject = %msg.subject,
                                    "Failed to get message info - not processing"
                                );
                                self.metrics.record_message_failed();
                                batch_had_failures = true;
                                continue;
                            }
                        };

                        // Poison message detection: if max_delivery_count is set
                        // and this message has been delivered too many times, send
                        // to DLQ and ack to prevent infinite redelivery.
                        // Uses >= so that max_delivery_count=5 triggers on the 5th attempt.
                        let num_delivered_u64 = num_delivered.max(0) as u64;
                        if self.config.max_delivery_count > 0
                            && num_delivered_u64 >= self.config.max_delivery_count
                        {
                            warn!(
                                stream_sequence,
                                num_delivered,
                                max_delivery_count = self.config.max_delivery_count,
                                subject = %msg.subject,
                                "Poison message detected - sending to DLQ"
                            );

                            // Try to extract store_id from payload for metadata
                            let store_id = serde_json::from_slice::<WriteRequest>(&msg.payload)
                                .ok()
                                .map(|r| r.store_id);

                            let dlq_msg = DlqMessage::max_retries_exceeded(
                                stream_sequence,
                                msg.subject.as_str(),
                                store_id,
                                msg.payload.to_vec(),
                                num_delivered_u64,
                            );

                            self.publish_to_dlq(&dlq_msg).await;

                            if let Err(ack_err) = msg.ack().await {
                                error!(error = %ack_err, "Failed to ack poison message");
                            }
                            self.metrics.record_message_failed();
                            batch_had_failures = true;
                            continue;
                        }

                        // Parse the write request
                        match serde_json::from_slice::<WriteRequest>(&msg.payload) {
                            Ok(request) => {
                                let store_id = request.store_id.clone();
                                pending.entry(store_id).or_default().push(PendingMessage {
                                    request,
                                    ack: msg,
                                    stream_sequence,
                                });
                            }
                            Err(e) => {
                                // Parse failures are typically permanent (malformed data)
                                // Publish structured DLQ message, then ack to prevent blocking

                                error!(
                                    error = %e,
                                    payload_size = msg.payload.len(),
                                    subject = %msg.subject,
                                    stream_sequence,
                                    num_delivered,
                                    "Failed to parse write request - sending to DLQ"
                                );

                                let dlq_msg = DlqMessage::parse_error(
                                    e.to_string(),
                                    stream_sequence,
                                    msg.subject.as_str(),
                                    msg.payload.to_vec(),
                                    num_delivered_u64,
                                );

                                self.publish_to_dlq(&dlq_msg).await;

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

            // Process pending batches based on timeout or batch size
            // Note: capacity check is done BEFORE fetching to prevent unbounded growth
            let current_pending: usize = pending.values().map(|v| v.len()).sum();
            let should_flush =
                !pending.is_empty() && batch_timer.elapsed() >= self.config.batch_timeout;
            let batch_full = current_pending >= self.config.batch_size;

            if should_flush || batch_full {
                match self.process_pending_batches(&mut pending).await {
                    Ok(_) => {
                        // Reset backoff on successful batch processing
                        consecutive_failures = 0;
                        storage_failures = 0;
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to process batches");
                        batch_had_failures = true;

                        // Track storage failures separately for circuit breaker
                        if matches!(e, WriterError::Storage(_)) {
                            storage_failures = storage_failures.saturating_add(1);
                            self.metrics.record_storage_failure();

                            // Open circuit breaker if storage failures exceed threshold
                            if storage_failures >= STORAGE_CIRCUIT_BREAKER_THRESHOLD {
                                error!(
                                    storage_failures,
                                    threshold = STORAGE_CIRCUIT_BREAKER_THRESHOLD,
                                    "Storage circuit breaker OPEN - stopping message fetch"
                                );
                                circuit_open = true;
                                last_health_probe = Instant::now();
                                self.metrics.update_circuit_breaker_state(true);
                            }
                        }
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

        // Process any remaining pending messages before shutdown
        if !pending.is_empty() {
            let remaining: usize = pending.values().map(|v| v.len()).sum();
            info!(
                remaining,
                "Processing remaining pending messages before shutdown"
            );
            if let Err(e) = self.process_pending_batches(&mut pending).await {
                error!(error = %e, "Failed to process remaining messages during shutdown");
                return Err(e);
            }
        }

        info!("Consumer shutdown complete");
        Ok(())
    }

    /// Process all pending batches.
    ///
    /// Returns the first error encountered, if any. All batches are attempted
    /// even if one fails, to maximize throughput.
    async fn process_pending_batches(
        &self,
        pending: &mut HashMap<String, Vec<PendingMessage>>,
    ) -> Result<()> {
        let mut first_error: Option<WriterError> = None;

        for (store_id, messages) in pending.drain() {
            if messages.is_empty() {
                continue;
            }

            let batch = self.create_batch(store_id, messages);
            self.metrics.record_batch_processed(batch.messages.len());

            if let Err(e) = self.process_batch(batch).await {
                error!(error = %e, "Failed to process batch");
                // Keep first error to return, but continue processing other batches
                if first_error.is_none() {
                    first_error = Some(e);
                }
            }
        }

        match first_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
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

        // Ack messages and handle event publish failures
        // Note: Acks are sequential because async-nats Message::ack() borrows self,
        // preventing parallel futures. This is typically fast (~1ms per ack) and
        // batches are bounded by batch_size (default 100, max 10000).
        if event_published {
            for msg in batch.messages {
                if let Err(e) = msg.ack().await {
                    error!(error = %e, "Failed to ack message");
                } else {
                    self.metrics.record_message_processed();
                }
            }
        } else {
            // Event publish failed after retries - storage is already committed!
            // Send structured DLQ message for recovery, then ack to prevent infinite redelivery
            let error_msg = last_error
                .as_ref()
                .map(|e| e.to_string())
                .unwrap_or_else(|| "unknown error".to_string());
            error!(
                store_id = %batch.store_id,
                error = %error_msg,
                "Failed to publish committed event after retries - sending to DLQ"
            );
            self.metrics.record_event_publish_failure();

            // Build structured DLQ message with the CommittedEvent as payload
            let event_payload = serde_json::to_vec(&event).unwrap_or_default();
            let dlq_msg = DlqMessage::event_publish_failed(
                error_msg,
                batch.max_stream_sequence,
                &batch.store_id,
                event_payload,
                1, // Event publish failures are always first delivery of the event
            );

            self.publish_to_dlq(&dlq_msg).await;

            // Ack messages since storage is committed (prevent duplicate writes)
            for msg in batch.messages {
                if let Err(e) = msg.ack().await {
                    error!(error = %e, "Failed to ack message after DLQ");
                } else {
                    self.metrics.record_message_processed();
                }
            }
        }

        debug!(
            store_id = %batch.store_id,
            duration_ms = start.elapsed().as_millis(),
            "Batch processed"
        );

        Ok(())
    }

    /// Publish a structured message to the Dead Letter Queue.
    ///
    /// Applies [`DLQ_PUBLISH_TIMEOUT`] to prevent blocking the consumer
    /// if NATS is slow or unresponsive.
    async fn publish_to_dlq(&self, dlq_msg: &DlqMessage) {
        let dlq_subject = dlq_msg.subject();
        let payload = match dlq_msg.to_bytes() {
            Ok(p) => p,
            Err(e) => {
                error!(
                    error = %e,
                    category = %dlq_msg.category,
                    "Failed to serialize DLQ message"
                );
                self.metrics.record_dlq_publish_failure();
                return;
            }
        };

        let publish_fut = self
            .client
            .client()
            .publish(dlq_subject.clone(), payload.into());

        match tokio::time::timeout(DLQ_PUBLISH_TIMEOUT, publish_fut).await {
            Ok(Ok(())) => {
                debug!(
                    subject = %dlq_subject,
                    category = %dlq_msg.category,
                    stream_sequence = dlq_msg.stream_sequence,
                    "Message sent to DLQ"
                );
                self.metrics.record_dlq_message_sent();
            }
            Ok(Err(dlq_err)) => {
                error!(
                    error = %dlq_err,
                    subject = %dlq_subject,
                    category = %dlq_msg.category,
                    "CRITICAL: Failed to publish to DLQ"
                );
                self.metrics.record_dlq_publish_failure();
            }
            Err(_elapsed) => {
                error!(
                    subject = %dlq_subject,
                    category = %dlq_msg.category,
                    timeout_secs = DLQ_PUBLISH_TIMEOUT.as_secs(),
                    "DLQ publish timed out"
                );
                self.metrics.record_dlq_publish_failure();
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_consumer_config_valid() {
        let config = ConsumerConfig {
            consumer_name: "test-consumer".to_string(),
            batch_size: 100,
            batch_timeout: Duration::from_millis(50),
            max_delivery_count: DEFAULT_MAX_DELIVERY_COUNT,
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_consumer_config_empty_name() {
        let config = ConsumerConfig {
            consumer_name: "".to_string(),
            batch_size: 100,
            batch_timeout: Duration::from_millis(50),
            max_delivery_count: DEFAULT_MAX_DELIVERY_COUNT,
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("consumer_name"));
    }

    #[test]
    fn test_consumer_config_batch_size_too_small() {
        let config = ConsumerConfig {
            consumer_name: "test".to_string(),
            batch_size: 0,
            batch_timeout: Duration::from_millis(50),
            max_delivery_count: DEFAULT_MAX_DELIVERY_COUNT,
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("batch_size"));
    }

    #[test]
    fn test_consumer_config_batch_size_too_large() {
        let config = ConsumerConfig {
            consumer_name: "test".to_string(),
            batch_size: MAX_BATCH_SIZE + 1,
            batch_timeout: Duration::from_millis(50),
            max_delivery_count: DEFAULT_MAX_DELIVERY_COUNT,
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("batch_size"));
    }

    #[test]
    fn test_consumer_config_timeout_too_small() {
        let config = ConsumerConfig {
            consumer_name: "test".to_string(),
            batch_size: 100,
            batch_timeout: Duration::from_millis(1),
            max_delivery_count: DEFAULT_MAX_DELIVERY_COUNT,
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("batch_timeout"));
    }

    #[test]
    fn test_consumer_config_timeout_too_large() {
        let config = ConsumerConfig {
            consumer_name: "test".to_string(),
            batch_size: 100,
            batch_timeout: MAX_BATCH_TIMEOUT + Duration::from_secs(1),
            max_delivery_count: DEFAULT_MAX_DELIVERY_COUNT,
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("batch_timeout"));
    }

    #[test]
    fn test_consumer_config_max_delivery_count_zero_disables() {
        let config = ConsumerConfig {
            consumer_name: "test".to_string(),
            batch_size: 100,
            batch_timeout: Duration::from_millis(50),
            max_delivery_count: 0,
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_consumer_config_default_max_delivery_count() {
        assert_eq!(DEFAULT_MAX_DELIVERY_COUNT, 5);
    }

    #[test]
    fn test_convert_operation_to_tuple_simple() {
        let op = TupleOperation::new("user:alice", "viewer", "document:readme");
        let tuple = convert_operation_to_tuple(&op).unwrap();

        assert_eq!(tuple.user_type, "user");
        assert_eq!(tuple.user_id, "alice");
        assert_eq!(tuple.relation, "viewer");
        assert_eq!(tuple.object_type, "document");
        assert_eq!(tuple.object_id, "readme");
        assert_eq!(tuple.user_relation, None);
        assert_eq!(tuple.condition_name, None);
    }

    #[test]
    fn test_convert_operation_to_tuple_with_userset() {
        let op = TupleOperation::new("team:eng#member", "viewer", "document:readme");
        let tuple = convert_operation_to_tuple(&op).unwrap();

        assert_eq!(tuple.user_type, "team");
        assert_eq!(tuple.user_id, "eng");
        assert_eq!(tuple.user_relation, Some("member".to_string()));
    }

    #[test]
    fn test_convert_operation_to_tuple_with_condition() {
        let op = TupleOperation::new("user:alice", "viewer", "document:readme")
            .with_condition(rsfga_nats::TupleCondition::new("ip_range"));
        let tuple = convert_operation_to_tuple(&op).unwrap();

        assert_eq!(tuple.condition_name, Some("ip_range".to_string()));
    }

    #[test]
    fn test_convert_key_to_tuple() {
        let key = TupleKey::new("user:bob", "editor", "folder:root");
        let tuple = convert_key_to_tuple(&key).unwrap();

        assert_eq!(tuple.user_type, "user");
        assert_eq!(tuple.user_id, "bob");
        assert_eq!(tuple.relation, "editor");
        assert_eq!(tuple.object_type, "folder");
        assert_eq!(tuple.object_id, "root");
    }

    #[test]
    fn test_convert_operation_invalid_user_returns_none() {
        let op = TupleOperation::new("invalid", "viewer", "document:readme");
        assert!(convert_operation_to_tuple(&op).is_none());
    }

    #[test]
    fn test_convert_operation_invalid_object_returns_none() {
        let op = TupleOperation::new("user:alice", "viewer", "invalid");
        assert!(convert_operation_to_tuple(&op).is_none());
    }
}
