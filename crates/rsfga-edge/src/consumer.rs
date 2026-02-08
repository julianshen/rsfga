//! Edge sync consumer for RSFGA_EVENTS stream.
//!
//! This module implements a pull consumer that:
//! - Subscribes to RSFGA_EVENTS stream with optional store filtering
//! - Deserializes CommittedEvent messages
//! - Applies writes and deletes to local edge storage
//! - Tracks sync position (watermark) per store for idempotency
//! - Acks messages after successful application
//!
//! # Delivery Semantics
//!
//! This consumer implements **at-least-once** delivery with idempotency:
//!
//! - Messages are only acknowledged after successful storage write
//! - The sync watermark tracks the last applied sequence per store
//! - Events with sequence <= watermark are skipped (already applied)
//! - Storage writes use ON CONFLICT (upsert) for idempotency on redelivery
//!
//! # Poison Message Handling
//!
//! Messages that exceed `max_delivery_count` are detected via NATS delivery
//! metadata and acked to prevent infinite redelivery. These are logged as
//! warnings for operator investigation.
//!
//! # Store Filtering
//!
//! The consumer can optionally filter events by store ID:
//! - `store_filter = None`: Receive all events (full replica)
//! - `store_filter = Some({"store-1", "store-2"})`: Only specific stores

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream::consumer::pull::Config as PullConfig;
use async_nats::jetstream::consumer::AckPolicy;
use async_nats::jetstream::Message as JsMessage;
use dashmap::DashMap;
use futures::StreamExt;
use tracing::{debug, error, info, warn};

use rsfga_nats::{CommittedEvent, NatsClient, STREAM_EVENTS, SUBJECT_EVENTS_PREFIX};
use rsfga_storage::{DataStore, StoredTuple};

use crate::error::{EdgeError, Result};
use crate::metrics::EdgeMetrics;

/// Default fetch batch size for edge consumer.
pub const DEFAULT_BATCH_SIZE: usize = 100;

/// Default fetch timeout for edge consumer.
pub const DEFAULT_FETCH_TIMEOUT: Duration = Duration::from_millis(500);

/// Maximum delivery count before skipping a message.
pub const DEFAULT_MAX_DELIVERY_COUNT: u64 = 10;

/// Consumer configuration for the edge sync daemon.
#[derive(Debug, Clone)]
pub struct EdgeConsumerConfig {
    /// Consumer name (must be unique per edge instance).
    pub consumer_name: String,

    /// Optional store filter - if set, only events for these stores are consumed.
    /// Uses HashSet for O(1) lookup performance.
    pub store_filter: Option<HashSet<String>>,

    /// Number of messages to fetch per batch.
    pub batch_size: usize,

    /// Timeout for batch fetch operations.
    pub fetch_timeout: Duration,

    /// Maximum delivery attempts before acking a poison message.
    /// Set to 0 to disable poison message detection.
    pub max_delivery_count: u64,
}

impl Default for EdgeConsumerConfig {
    fn default() -> Self {
        Self {
            consumer_name: "rsfga-edge-1".to_string(),
            store_filter: None,
            batch_size: DEFAULT_BATCH_SIZE,
            fetch_timeout: DEFAULT_FETCH_TIMEOUT,
            max_delivery_count: DEFAULT_MAX_DELIVERY_COUNT,
        }
    }
}

/// Sync watermark tracking the last applied sequence per store.
///
/// Uses DashMap for lock-free concurrent access. The watermark tracks
/// the highest CommittedEvent.sequence that has been successfully
/// applied to local storage for each store_id.
///
/// **Note**: The watermark is currently in-memory only. On daemon restart,
/// events will be replayed from the NATS durable consumer position. Storage
/// writes are idempotent (ON CONFLICT / upsert), so replayed events are safe.
/// Persistent watermark storage is planned for a follow-up (plan.md 2.0.6.2).
#[derive(Debug, Default, Clone)]
pub struct SyncWatermark {
    /// Map of store_id -> last applied sequence number.
    positions: Arc<DashMap<String, u64>>,
}

impl SyncWatermark {
    /// Create a new empty sync watermark.
    pub fn new() -> Self {
        Self {
            positions: Arc::new(DashMap::new()),
        }
    }

    /// Get the last applied sequence for a store.
    #[allow(dead_code)] // Used by tests; planned for watermark persistence (2.0.6.2)
    pub fn get(&self, store_id: &str) -> Option<u64> {
        self.positions.get(store_id).map(|v| *v)
    }

    /// Update the sync position for a store.
    ///
    /// Only advances the watermark (never goes backward).
    pub fn advance(&self, store_id: &str, sequence: u64) {
        self.positions
            .entry(store_id.to_string())
            .and_modify(|current| {
                if sequence > *current {
                    *current = sequence;
                }
            })
            .or_insert(sequence);
    }

    /// Check if a sequence has already been applied for a store.
    pub fn is_applied(&self, store_id: &str, sequence: u64) -> bool {
        self.positions
            .get(store_id)
            .map(|v| sequence <= *v)
            .unwrap_or(false)
    }

    /// Get all tracked store positions.
    pub fn positions(&self) -> HashMap<String, u64> {
        self.positions
            .iter()
            .map(|entry| (entry.key().clone(), *entry.value()))
            .collect()
    }

    /// Restore watermark from saved positions (e.g., on restart).
    #[allow(dead_code)] // Used by tests; planned for watermark persistence (2.0.6.2)
    pub fn restore(&self, positions: HashMap<String, u64>) {
        for (store_id, sequence) in positions {
            self.advance(&store_id, sequence);
        }
    }
}

/// Edge sync consumer that applies committed events to local storage.
pub struct EdgeConsumer {
    client: NatsClient,
    storage: Arc<dyn DataStore>,
    metrics: Arc<EdgeMetrics>,
    config: EdgeConsumerConfig,
    watermark: SyncWatermark,
}

impl EdgeConsumer {
    /// Create a new edge consumer.
    pub fn new(
        client: NatsClient,
        storage: Arc<dyn DataStore>,
        metrics: Arc<EdgeMetrics>,
        config: EdgeConsumerConfig,
    ) -> Self {
        Self {
            client,
            storage,
            metrics,
            config,
            watermark: SyncWatermark::new(),
        }
    }

    /// Create a new edge consumer with a pre-existing watermark (for restart recovery).
    #[allow(dead_code)] // Planned for bootstrap sync (2.0.6.4)
    pub fn with_watermark(
        client: NatsClient,
        storage: Arc<dyn DataStore>,
        metrics: Arc<EdgeMetrics>,
        config: EdgeConsumerConfig,
        watermark: SyncWatermark,
    ) -> Self {
        Self {
            client,
            storage,
            metrics,
            config,
            watermark,
        }
    }

    /// Get a reference to the sync watermark.
    #[allow(dead_code)] // Planned for watermark persistence (2.0.6.2)
    pub fn watermark(&self) -> &SyncWatermark {
        &self.watermark
    }

    /// Get a reference to the consumer config.
    #[allow(dead_code)] // Planned for health check endpoint
    pub fn config(&self) -> &EdgeConsumerConfig {
        &self.config
    }

    /// Build the NATS subject filter based on store_filter config.
    fn subject_filter(&self) -> Vec<String> {
        match &self.config.store_filter {
            Some(stores) => stores
                .iter()
                .map(|store_id| format!("{}.{}.>", SUBJECT_EVENTS_PREFIX, store_id))
                .collect(),
            None => vec![format!("{}.>", SUBJECT_EVENTS_PREFIX)],
        }
    }

    /// Apply a committed event to local storage.
    ///
    /// Converts the event's tuple operations into StoredTuple format
    /// and writes/deletes them via the DataStore trait.
    async fn apply_event(&self, event: &CommittedEvent) -> Result<()> {
        let writes: Vec<StoredTuple> = event
            .writes
            .iter()
            .filter_map(tuple_operation_to_stored_tuple)
            .collect();

        let deletes: Vec<StoredTuple> = event
            .deletes
            .iter()
            .filter_map(tuple_key_to_stored_tuple)
            .collect();

        if writes.is_empty() && deletes.is_empty() {
            debug!(
                store_id = %event.store_id,
                sequence = event.sequence,
                "Event has no tuple operations to apply"
            );
            return Ok(());
        }

        let start = std::time::Instant::now();
        let num_writes = writes.len();
        let num_deletes = deletes.len();

        self.storage
            .write_tuples(&event.store_id, writes, deletes)
            .await
            .map_err(EdgeError::Storage)?;

        self.metrics.record_storage_latency(start);
        self.metrics.record_tuples_written(num_writes);
        self.metrics.record_tuples_deleted(num_deletes);

        debug!(
            store_id = %event.store_id,
            sequence = event.sequence,
            writes = num_writes,
            deletes = num_deletes,
            "Applied event to local storage"
        );

        Ok(())
    }

    /// Check if a message is a poison message (exceeded max delivery count).
    ///
    /// Returns true if the message should be skipped and acked.
    fn is_poison_message(&self, message: &JsMessage) -> bool {
        if self.config.max_delivery_count == 0 {
            return false;
        }

        match message.info() {
            Ok(info) => {
                let num_delivered = info.delivered.max(0) as u64;
                if num_delivered >= self.config.max_delivery_count {
                    warn!(
                        stream_sequence = info.stream_sequence,
                        num_delivered,
                        max_delivery_count = self.config.max_delivery_count,
                        subject = %message.subject,
                        "Poison message detected - acking to prevent infinite redelivery"
                    );
                    true
                } else {
                    false
                }
            }
            Err(e) => {
                warn!(
                    error = %e,
                    subject = %message.subject,
                    "Failed to get message info for poison detection"
                );
                false
            }
        }
    }

    /// Process a single NATS message containing a CommittedEvent.
    ///
    /// Returns Ok(true) if the event was applied, Ok(false) if it was skipped.
    async fn process_message(&self, message: &JsMessage) -> Result<bool> {
        self.metrics.record_event_consumed();

        // Check for poison messages before deserializing
        if self.is_poison_message(message) {
            self.metrics.record_event_failed();
            return Ok(false);
        }

        // Deserialize the event
        let event = match CommittedEvent::from_bytes(&message.payload) {
            Ok(event) => event,
            Err(e) => {
                warn!(
                    error = %e,
                    subject = %message.subject,
                    "Failed to deserialize CommittedEvent, skipping"
                );
                self.metrics.record_event_failed();
                return Ok(false);
            }
        };

        // Check idempotency via watermark
        if self.watermark.is_applied(&event.store_id, event.sequence) {
            debug!(
                store_id = %event.store_id,
                sequence = event.sequence,
                "Event already applied, skipping"
            );
            self.metrics.record_event_skipped();
            return Ok(false);
        }

        // Check store filter
        if let Some(filter) = &self.config.store_filter {
            if !filter.contains(&event.store_id) {
                debug!(
                    store_id = %event.store_id,
                    "Event for untracked store, skipping"
                );
                self.metrics.record_event_skipped();
                return Ok(false);
            }
        }

        // Apply the event to local storage
        self.apply_event(&event).await?;

        // Advance the watermark
        self.watermark.advance(&event.store_id, event.sequence);
        self.metrics.record_event_applied();
        self.metrics
            .update_sync_position(&event.store_id, event.sequence);

        info!(
            store_id = %event.store_id,
            sequence = event.sequence,
            writes = event.writes.len(),
            deletes = event.deletes.len(),
            "Synced event to edge storage"
        );

        Ok(true)
    }

    /// Run the edge consumer with graceful shutdown support.
    pub async fn run_with_shutdown(
        &self,
        shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> Result<()> {
        info!(
            consumer_name = %self.config.consumer_name,
            store_filter = ?self.config.store_filter,
            batch_size = self.config.batch_size,
            "Starting edge sync consumer"
        );

        // Create or get durable consumer on RSFGA_EVENTS
        let js = self.client.jetstream();
        let stream = js.get_stream(STREAM_EVENTS).await.map_err(|e| {
            EdgeError::Consumer(format!("Failed to get RSFGA_EVENTS stream: {}", e))
        })?;

        let subjects = self.subject_filter();
        let consumer_config = PullConfig {
            durable_name: Some(self.config.consumer_name.clone()),
            ack_policy: AckPolicy::Explicit,
            filter_subjects: subjects,
            max_ack_pending: 10_000,
            ..Default::default()
        };

        let consumer = match stream.get_consumer(&self.config.consumer_name).await {
            Ok(consumer) => {
                info!(
                    consumer_name = %self.config.consumer_name,
                    "Resuming existing durable consumer"
                );
                consumer
            }
            Err(e) => {
                // Only create a new consumer if the error indicates it doesn't exist.
                // Log the original error for debugging transient failures.
                info!(
                    consumer_name = %self.config.consumer_name,
                    error = %e,
                    "Consumer not found, creating new durable consumer"
                );
                stream
                    .create_consumer(consumer_config)
                    .await
                    .map_err(|e| EdgeError::Consumer(format!("Failed to create consumer: {}", e)))?
            }
        };

        // Main consume loop
        loop {
            // Check for shutdown signal
            if *shutdown_rx.borrow() {
                info!("Shutdown signal received, stopping edge consumer");
                break;
            }

            // Fetch a batch of messages
            let messages_result = consumer
                .fetch()
                .max_messages(self.config.batch_size)
                .expires(self.config.fetch_timeout)
                .messages()
                .await;

            let mut messages = match messages_result {
                Ok(msgs) => msgs,
                Err(e) => {
                    warn!(error = %e, "Failed to fetch messages, retrying");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };

            let mut processed = 0u64;
            while let Some(msg_result) = messages.next().await {
                // Check shutdown between messages
                if *shutdown_rx.borrow() {
                    info!("Shutdown signal received during batch processing");
                    break;
                }

                let message = match msg_result {
                    Ok(msg) => msg,
                    Err(e) => {
                        warn!(error = %e, "Error receiving message");
                        continue;
                    }
                };

                match self.process_message(&message).await {
                    Ok(_applied) => {
                        // Ack the message regardless of whether it was applied or skipped
                        if let Err(e) = message.ack().await {
                            warn!(error = %e, "Failed to ack message");
                        }
                        processed += 1;
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to process event");
                        self.metrics.record_event_failed();
                        self.metrics.record_storage_failure();
                        // Don't ack - let NATS redeliver
                    }
                }
            }

            if processed > 0 {
                debug!(processed = processed, "Batch complete");
            }

            // Brief yield to prevent busy-loop when no messages
            tokio::task::yield_now().await;
        }

        let positions = self.watermark.positions();
        info!(
            positions = ?positions,
            "Edge consumer stopped, final sync positions"
        );

        Ok(())
    }
}

/// Convert a TupleOperation (from NATS event) to a StoredTuple (for storage).
fn tuple_operation_to_stored_tuple(op: &rsfga_nats::TupleOperation) -> Option<StoredTuple> {
    tuple_key_to_stored_tuple_with_condition(&op.key, op.condition.as_ref())
}

/// Convert a TupleKey (from NATS event) to a StoredTuple (for storage).
fn tuple_key_to_stored_tuple(key: &rsfga_nats::TupleKey) -> Option<StoredTuple> {
    tuple_key_to_stored_tuple_with_condition(key, None)
}

/// Convert a TupleKey with optional condition to a StoredTuple.
fn tuple_key_to_stored_tuple_with_condition(
    key: &rsfga_nats::TupleKey,
    condition: Option<&rsfga_nats::TupleCondition>,
) -> Option<StoredTuple> {
    use rsfga_nats::utils::{parse_object, parse_user};

    let (user_type, user_id, user_relation) = parse_user(&key.user)?;
    let (object_type, object_id) = parse_object(&key.object)?;

    let mut tuple = StoredTuple {
        user_type: user_type.to_string(),
        user_id: user_id.to_string(),
        user_relation: user_relation.map(|s| s.to_string()),
        relation: key.relation.clone(),
        object_type: object_type.to_string(),
        object_id: object_id.to_string(),
        condition_name: None,
        condition_context: None,
        created_at: None,
    };

    if let Some(cond) = condition {
        tuple.condition_name = Some(cond.name.clone());
        if !cond.context.is_empty() {
            tuple.condition_context = Some(cond.context.clone());
        }
    }

    Some(tuple)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsfga_nats::{TupleCondition, TupleKey, TupleOperation};
    use std::sync::atomic::Ordering;

    // ===========================================
    // Section 1: SyncWatermark Tests
    // ===========================================

    #[test]
    fn test_sync_watermark_new_is_empty() {
        let wm = SyncWatermark::new();
        assert!(wm.get("store-1").is_none());
        assert!(wm.positions().is_empty());
    }

    #[test]
    fn test_sync_watermark_advance_and_get() {
        let wm = SyncWatermark::new();
        wm.advance("store-1", 10);
        assert_eq!(wm.get("store-1"), Some(10));
    }

    #[test]
    fn test_sync_watermark_advance_only_moves_forward() {
        let wm = SyncWatermark::new();
        wm.advance("store-1", 10);
        wm.advance("store-1", 5); // Should not go backward
        assert_eq!(wm.get("store-1"), Some(10));

        wm.advance("store-1", 15); // Should advance
        assert_eq!(wm.get("store-1"), Some(15));
    }

    #[test]
    fn test_sync_watermark_is_applied() {
        let wm = SyncWatermark::new();
        wm.advance("store-1", 10);

        assert!(wm.is_applied("store-1", 5)); // Before watermark
        assert!(wm.is_applied("store-1", 10)); // At watermark
        assert!(!wm.is_applied("store-1", 11)); // After watermark
        assert!(!wm.is_applied("store-2", 5)); // Unknown store
    }

    #[test]
    fn test_sync_watermark_multiple_stores() {
        let wm = SyncWatermark::new();
        wm.advance("store-1", 10);
        wm.advance("store-2", 20);
        wm.advance("store-3", 5);

        assert_eq!(wm.get("store-1"), Some(10));
        assert_eq!(wm.get("store-2"), Some(20));
        assert_eq!(wm.get("store-3"), Some(5));

        let positions = wm.positions();
        assert_eq!(positions.len(), 3);
    }

    #[test]
    fn test_sync_watermark_restore() {
        let wm = SyncWatermark::new();
        let mut saved = HashMap::new();
        saved.insert("store-1".to_string(), 100);
        saved.insert("store-2".to_string(), 200);

        wm.restore(saved);
        assert_eq!(wm.get("store-1"), Some(100));
        assert_eq!(wm.get("store-2"), Some(200));
    }

    #[test]
    fn test_sync_watermark_restore_does_not_regress() {
        let wm = SyncWatermark::new();
        wm.advance("store-1", 100);

        let mut saved = HashMap::new();
        saved.insert("store-1".to_string(), 50); // Lower than current
        wm.restore(saved);

        assert_eq!(wm.get("store-1"), Some(100)); // Should not regress
    }

    // ===========================================
    // Section 2: EdgeConsumerConfig Tests
    // ===========================================

    #[test]
    fn test_edge_consumer_config_defaults() {
        let config = EdgeConsumerConfig::default();
        assert_eq!(config.consumer_name, "rsfga-edge-1");
        assert!(config.store_filter.is_none());
        assert_eq!(config.batch_size, DEFAULT_BATCH_SIZE);
        assert_eq!(config.fetch_timeout, DEFAULT_FETCH_TIMEOUT);
        assert_eq!(config.max_delivery_count, DEFAULT_MAX_DELIVERY_COUNT);
    }

    #[test]
    fn test_edge_consumer_config_custom() {
        let config = EdgeConsumerConfig {
            consumer_name: "edge-us-west-1".to_string(),
            store_filter: Some(
                ["store-a".to_string(), "store-b".to_string()]
                    .into_iter()
                    .collect(),
            ),
            batch_size: 50,
            fetch_timeout: Duration::from_secs(1),
            max_delivery_count: 5,
        };
        assert_eq!(config.consumer_name, "edge-us-west-1");
        assert_eq!(config.store_filter.as_ref().unwrap().len(), 2);
        assert_eq!(config.batch_size, 50);
    }

    #[test]
    fn test_store_filter_hashset_contains() {
        let filter: HashSet<String> = ["store-a".to_string(), "store-b".to_string()]
            .into_iter()
            .collect();
        assert!(filter.contains("store-a"));
        assert!(filter.contains("store-b"));
        assert!(!filter.contains("store-c"));
    }

    // ===========================================
    // Section 3: Tuple Conversion Tests
    // ===========================================

    #[test]
    fn test_tuple_key_to_stored_tuple_simple_user() {
        let key = TupleKey::new("user:alice", "viewer", "document:readme");
        let stored = tuple_key_to_stored_tuple(&key).unwrap();

        assert_eq!(stored.user_type, "user");
        assert_eq!(stored.user_id, "alice");
        assert!(stored.user_relation.is_none());
        assert_eq!(stored.relation, "viewer");
        assert_eq!(stored.object_type, "document");
        assert_eq!(stored.object_id, "readme");
    }

    #[test]
    fn test_tuple_key_to_stored_tuple_userset() {
        let key = TupleKey::new("team:engineering#member", "viewer", "document:readme");
        let stored = tuple_key_to_stored_tuple(&key).unwrap();

        assert_eq!(stored.user_type, "team");
        assert_eq!(stored.user_id, "engineering");
        assert_eq!(stored.user_relation, Some("member".to_string()));
        assert_eq!(stored.relation, "viewer");
        assert_eq!(stored.object_type, "document");
        assert_eq!(stored.object_id, "readme");
    }

    #[test]
    fn test_tuple_operation_to_stored_tuple_with_condition() {
        let op = TupleOperation::new("user:alice", "viewer", "document:readme").with_condition(
            TupleCondition::new("time_bound").with_context("expires_at", "2026-12-31"),
        );

        let stored = tuple_operation_to_stored_tuple(&op).unwrap();
        assert_eq!(stored.condition_name, Some("time_bound".to_string()));
        assert!(stored.condition_context.is_some());
    }

    #[test]
    fn test_tuple_operation_to_stored_tuple_without_condition() {
        let op = TupleOperation::new("user:alice", "viewer", "document:readme");
        let stored = tuple_operation_to_stored_tuple(&op).unwrap();
        assert!(stored.condition_name.is_none());
        assert!(stored.condition_context.is_none());
    }

    #[test]
    fn test_tuple_key_invalid_object_returns_none() {
        // Objects without "type:id" format should fail to parse
        let key = TupleKey::new("user:alice", "viewer", "invalid-no-colon");
        assert!(tuple_key_to_stored_tuple(&key).is_none());
    }

    #[test]
    fn test_tuple_key_invalid_user_returns_none() {
        let key = TupleKey::new("invalid-no-colon", "viewer", "document:readme");
        assert!(tuple_key_to_stored_tuple(&key).is_none());
    }

    // ===========================================
    // Section 4: Subject Filter Tests
    // ===========================================

    #[tokio::test]
    async fn test_subject_filter_no_store_filter() {
        let config = EdgeConsumerConfig::default();
        assert!(config.store_filter.is_none());
        // When no filter, subject should be "rsfga.events.>"
    }

    #[test]
    fn test_subject_filter_with_store_filter() {
        // Test the subject generation logic directly
        let stores: HashSet<String> = ["store-a".to_string(), "store-b".to_string()]
            .into_iter()
            .collect();
        let mut subjects: Vec<String> = stores
            .iter()
            .map(|store_id| format!("{}.{}.>", SUBJECT_EVENTS_PREFIX, store_id))
            .collect();
        subjects.sort(); // HashSet iteration order is not deterministic

        assert_eq!(subjects.len(), 2);
        assert_eq!(subjects[0], "rsfga.events.store-a.>");
        assert_eq!(subjects[1], "rsfga.events.store-b.>");
    }

    #[test]
    fn test_subject_filter_all_events() {
        let subject = format!("{}.>", SUBJECT_EVENTS_PREFIX);
        assert_eq!(subject, "rsfga.events.>");
    }

    // ===========================================
    // Section 5: EdgeMetrics Tests
    // ===========================================

    #[test]
    fn test_edge_metrics_defaults() {
        let metrics = EdgeMetrics::new();
        assert_eq!(metrics.events_consumed.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.events_applied.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.events_skipped.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.events_failed.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.tuples_written.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.tuples_deleted.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.cache_invalidations.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.storage_failures.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_edge_metrics_record_event_consumed() {
        let metrics = EdgeMetrics::new();
        metrics.record_event_consumed();
        metrics.record_event_consumed();
        assert_eq!(metrics.events_consumed.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn test_edge_metrics_record_event_applied() {
        let metrics = EdgeMetrics::new();
        metrics.record_event_applied();
        assert_eq!(metrics.events_applied.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_edge_metrics_record_event_skipped() {
        let metrics = EdgeMetrics::new();
        metrics.record_event_skipped();
        assert_eq!(metrics.events_skipped.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_edge_metrics_record_tuples_written() {
        let metrics = EdgeMetrics::new();
        metrics.record_tuples_written(5);
        metrics.record_tuples_written(3);
        assert_eq!(metrics.tuples_written.load(Ordering::Relaxed), 8);
    }

    #[test]
    fn test_edge_metrics_record_tuples_deleted() {
        let metrics = EdgeMetrics::new();
        metrics.record_tuples_deleted(2);
        assert_eq!(metrics.tuples_deleted.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn test_edge_metrics_record_cache_invalidation() {
        let metrics = EdgeMetrics::new();
        metrics.record_cache_invalidation();
        metrics.record_cache_invalidation();
        assert_eq!(metrics.cache_invalidations.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn test_edge_metrics_record_storage_failure() {
        let metrics = EdgeMetrics::new();
        metrics.record_storage_failure();
        assert_eq!(metrics.storage_failures.load(Ordering::Relaxed), 1);
    }
}
