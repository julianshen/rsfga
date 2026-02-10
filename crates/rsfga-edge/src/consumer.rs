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
//! Messages that fail deserialization (malformed JSON) are also acked and
//! counted via the `events_failed` metric. This is intentional: since the
//! payload cannot be parsed, redelivery would produce the same failure
//! indefinitely. Operators should monitor `rsfga_edge_events_failed_total`.
//!
//! # Store Filtering
//!
//! The consumer can optionally filter events by store ID:
//! - `store_filter = None`: Receive all events (full replica)
//! - `store_filter = Some({"store-1", "store-2"})`: Only specific stores

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream::consumer::pull::Config as PullConfig;
use async_nats::jetstream::consumer::AckPolicy;
use async_nats::jetstream::Message as JsMessage;
use async_trait::async_trait;
use dashmap::DashMap;
use futures::StreamExt;
use tracing::{debug, error, info, warn};

use rsfga_nats::{CommittedEvent, NatsClient, STREAM_EVENTS, SUBJECT_EVENTS_PREFIX};
use rsfga_storage::{DataStore, StoredTuple};

use crate::error::{EdgeError, Result};
use crate::metrics::EdgeMetrics;
use crate::watermark_store::{WatermarkSnapshot, WatermarkStore};

/// Trait for invalidating check cache entries when tuple operations are applied.
///
/// This abstraction allows the edge consumer to invalidate cache entries
/// without depending on rsfga-domain directly. The `CheckCache` type
/// implements this trait when used in an integrated edge+API deployment.
#[async_trait]
pub trait CacheInvalidator: Send + Sync {
    /// Invalidate cache entries affected by a tuple write or delete.
    ///
    /// Called with the store ID, object (e.g., "document:readme"), and
    /// relation (e.g., "viewer") of the affected tuple.
    async fn invalidate_for_tuple(&self, store_id: &str, object: &str, relation: &str);
}

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

/// Maximum number of out-of-order sequences tracked per store before
/// force-advancing the contiguous watermark. Prevents unbounded memory
/// growth from persistent gaps (e.g., lost messages, sequence jumps).
const MAX_PENDING_SET_SIZE: usize = 10_000;

/// Per-store watermark state tracking both contiguous position and pending sequences.
#[derive(Debug, Clone)]
struct StoreWatermarkState {
    /// Highest sequence where all sequences 1..=contiguous are applied.
    contiguous: u64,
    /// Sequences applied above the contiguous point (gaps exist below these).
    /// Bounded to MAX_PENDING_SET_SIZE entries.
    pending: BTreeSet<u64>,
}

impl StoreWatermarkState {
    fn new() -> Self {
        Self {
            contiguous: 0,
            pending: BTreeSet::new(),
        }
    }

    /// Record that a sequence has been applied and advance contiguous if possible.
    ///
    /// If the pending set exceeds MAX_PENDING_SET_SIZE, the contiguous watermark
    /// is force-advanced to the lowest pending entry, accepting that the skipped
    /// gap will not be detected as a duplicate on replay.
    fn advance(&mut self, sequence: u64) {
        if sequence == 0 {
            return; // Sequence 0 is invalid (contiguous 0 means "no events")
        }

        if sequence <= self.contiguous {
            return; // Already covered by contiguous watermark
        }

        if sequence == self.contiguous + 1 {
            // Extends the contiguous range directly
            self.contiguous = sequence;
            // Drain any consecutive pending sequences
            while self.pending.first().copied() == Some(self.contiguous + 1) {
                self.pending.pop_first();
                self.contiguous += 1;
            }
        } else {
            // Gap exists — add to pending set
            self.pending.insert(sequence);

            // Enforce size bound: force-advance if pending set too large
            if self.pending.len() > MAX_PENDING_SET_SIZE {
                self.force_advance_to_lowest_pending();
            }
        }
    }

    /// Force-advance the contiguous watermark to the lowest pending entry,
    /// accepting the gap. This bounds memory usage at the cost of losing
    /// dedup protection for the skipped sequences.
    fn force_advance_to_lowest_pending(&mut self) {
        if let Some(&lowest) = self.pending.first() {
            warn!(
                old_contiguous = self.contiguous,
                new_contiguous = lowest,
                gap_size = lowest - self.contiguous - 1,
                pending_size = self.pending.len(),
                "Force-advancing watermark past gap (pending set exceeded limit)"
            );
            self.contiguous = lowest;
            self.pending.remove(&lowest);

            // Continue draining consecutive entries
            while self.pending.first().copied() == Some(self.contiguous + 1) {
                self.pending.pop_first();
                self.contiguous += 1;
            }
        }
    }

    /// Check if a sequence has been applied (either contiguous or pending).
    fn is_applied(&self, sequence: u64) -> bool {
        sequence <= self.contiguous || self.pending.contains(&sequence)
    }
}

/// Sync watermark tracking the last applied sequence per store.
///
/// Uses DashMap for lock-free concurrent access. Handles out-of-order
/// event delivery by maintaining a contiguous watermark plus a pending
/// set of applied-but-not-yet-contiguous sequences per store.
///
/// When gaps are filled, the contiguous watermark automatically advances
/// through any consecutive pending sequences. The pending set is bounded
/// to `MAX_PENDING_SET_SIZE` entries; if exceeded, the contiguous watermark
/// is force-advanced past the gap to prevent unbounded memory growth.
///
/// Persistence is handled by `WatermarkStore` (see `watermark_store.rs`).
/// On daemon restart, saved contiguous positions are restored via `restore()`.
/// Storage writes are idempotent (ON CONFLICT / upsert), so replayed events
/// from NATS redelivery are safe even if the watermark lags slightly.
#[derive(Debug, Default, Clone)]
pub struct SyncWatermark {
    /// Map of store_id -> watermark state (contiguous + pending).
    states: Arc<DashMap<String, StoreWatermarkState>>,
}

impl SyncWatermark {
    /// Create a new empty sync watermark.
    pub fn new() -> Self {
        Self {
            states: Arc::new(DashMap::new()),
        }
    }

    /// Get the highest applied sequence for a store (max of contiguous + pending).
    #[allow(dead_code)] // Used by tests
    pub fn get(&self, store_id: &str) -> Option<u64> {
        self.states.get(store_id).map(|state| {
            state
                .pending
                .last()
                .copied()
                .unwrap_or(state.contiguous)
                .max(state.contiguous)
        })
    }

    /// Get the contiguous position for a store.
    ///
    /// This is the highest sequence where all sequences 1..=N have been applied.
    /// Returns None if no events have been applied for this store.
    #[allow(dead_code)] // Used by tests
    pub fn contiguous_position(&self, store_id: &str) -> Option<u64> {
        self.states.get(store_id).and_then(|state| {
            if state.contiguous == 0 {
                None
            } else {
                Some(state.contiguous)
            }
        })
    }

    /// Record that a sequence has been applied for a store.
    ///
    /// Handles out-of-order delivery: if the sequence fills a gap,
    /// the contiguous watermark advances through any consecutive applied sequences.
    /// Sequence 0 is rejected (contiguous 0 means "no events applied").
    pub fn advance(&self, store_id: &str, sequence: u64) {
        if sequence == 0 {
            return;
        }
        self.states
            .entry(store_id.to_string())
            .and_modify(|state| {
                state.advance(sequence);
            })
            .or_insert_with(|| {
                let mut state = StoreWatermarkState::new();
                state.advance(sequence);
                state
            });
    }

    /// Check if a sequence has already been applied for a store.
    pub fn is_applied(&self, store_id: &str, sequence: u64) -> bool {
        self.states
            .get(store_id)
            .map(|state| state.is_applied(sequence))
            .unwrap_or(false)
    }

    /// Get all tracked store positions (contiguous watermark per store).
    pub fn positions(&self) -> HashMap<String, u64> {
        self.states
            .iter()
            .filter(|entry| entry.value().contiguous > 0)
            .map(|entry| (entry.key().clone(), entry.value().contiguous))
            .collect()
    }

    /// Create a snapshot of the current contiguous positions for persistence.
    pub fn to_snapshot(&self) -> WatermarkSnapshot {
        WatermarkSnapshot {
            positions: self.positions(),
        }
    }

    /// Restore watermark from saved contiguous positions (e.g., on restart).
    pub fn restore(&self, positions: HashMap<String, u64>) {
        for (store_id, sequence) in positions {
            // Restore sets the contiguous position directly (all seqs up to it are assumed applied)
            self.states
                .entry(store_id)
                .and_modify(|state| {
                    if sequence > state.contiguous {
                        state.contiguous = sequence;
                        // Remove any pending entries now covered by the contiguous watermark
                        state.pending.retain(|&s| s > sequence);
                    }
                })
                .or_insert_with(|| StoreWatermarkState {
                    contiguous: sequence,
                    pending: BTreeSet::new(),
                });
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
    watermark_store: Option<Arc<dyn WatermarkStore>>,
    cache: Option<Arc<dyn CacheInvalidator>>,
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
            watermark_store: None,
            cache: None,
        }
    }

    /// Create a new edge consumer with persistent watermark storage.
    ///
    /// Loads the watermark from the store on creation and saves after each batch.
    #[allow(dead_code)] // Will be wired into main.rs CLI in follow-up
    pub async fn with_watermark_store(
        client: NatsClient,
        storage: Arc<dyn DataStore>,
        metrics: Arc<EdgeMetrics>,
        config: EdgeConsumerConfig,
        watermark_store: Arc<dyn WatermarkStore>,
    ) -> Result<Self> {
        let watermark = SyncWatermark::new();

        // Load saved positions from the store
        let snapshot = watermark_store.load().await?;
        if !snapshot.positions.is_empty() {
            info!(
                positions = ?snapshot.positions,
                "Restored watermark from persistent store"
            );
            watermark.restore(snapshot.positions);
        }

        Ok(Self {
            client,
            storage,
            metrics,
            config,
            watermark,
            watermark_store: Some(watermark_store),
            cache: None,
        })
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
            watermark_store: None,
            cache: None,
        }
    }

    /// Set the cache invalidator for this consumer.
    ///
    /// When set, the consumer will invalidate cache entries for each
    /// tuple affected by an applied event. This is used in edge+API
    /// deployments where the check cache is shared in-process.
    #[allow(dead_code)] // Public API for edge+API integration (wired in main.rs)
    pub fn with_cache(mut self, cache: Arc<dyn CacheInvalidator>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Get a reference to the sync watermark.
    #[allow(dead_code)] // Used by tests; will be used by health check endpoint
    pub fn watermark(&self) -> &SyncWatermark {
        &self.watermark
    }

    /// Save current watermark positions to persistent store.
    async fn save_watermark(&self) -> Result<()> {
        if let Some(store) = &self.watermark_store {
            let snapshot = self.watermark.to_snapshot();
            store.save(&snapshot).await?;
        }
        Ok(())
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
    /// and writes/deletes them via the DataStore trait. After a successful
    /// storage write, invalidates cache entries for all affected tuples.
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

        // Invalidate cache entries for affected tuples
        if let Some(cache) = &self.cache {
            invalidate_cache_for_event(cache.as_ref(), &self.metrics, event).await;
        }

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

                // Persist watermark after each batch
                if let Err(e) = self.save_watermark().await {
                    warn!(error = %e, "Failed to save watermark after batch");
                }
            }

            // Brief yield to prevent busy-loop when no messages
            tokio::task::yield_now().await;
        }

        // Final watermark save on shutdown
        if let Err(e) = self.save_watermark().await {
            warn!(error = %e, "Failed to save watermark on shutdown");
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

/// Invalidate cache entries for all tuples affected by a committed event.
///
/// For each written or deleted tuple, calls `invalidate_for_tuple()` on the
/// cache with the store ID, object, and relation. Records a metric for each
/// invalidation via `record_cache_invalidation()`.
async fn invalidate_cache_for_event(
    cache: &dyn CacheInvalidator,
    metrics: &EdgeMetrics,
    event: &CommittedEvent,
) {
    for write in &event.writes {
        cache
            .invalidate_for_tuple(&event.store_id, &write.key.object, &write.key.relation)
            .await;
        metrics.record_cache_invalidation();
    }
    for delete in &event.deletes {
        cache
            .invalidate_for_tuple(&event.store_id, &delete.object, &delete.relation)
            .await;
        metrics.record_cache_invalidation();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsfga_nats::{TupleCondition, TupleKey, TupleOperation};
    use std::sync::atomic::Ordering;
    use tokio::sync::Mutex;

    /// Mock cache invalidator that records all invalidation calls.
    struct MockCacheInvalidator {
        calls: Mutex<Vec<(String, String, String)>>,
    }

    impl MockCacheInvalidator {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
            }
        }

        async fn calls(&self) -> Vec<(String, String, String)> {
            self.calls.lock().await.clone()
        }
    }

    #[async_trait]
    impl CacheInvalidator for MockCacheInvalidator {
        async fn invalidate_for_tuple(&self, store_id: &str, object: &str, relation: &str) {
            self.calls.lock().await.push((
                store_id.to_string(),
                object.to_string(),
                relation.to_string(),
            ));
        }
    }

    /// Helper to create a CommittedEvent with writes and deletes.
    fn make_event(
        store_id: &str,
        sequence: u64,
        writes: Vec<TupleOperation>,
        deletes: Vec<TupleKey>,
    ) -> CommittedEvent {
        let mut event = CommittedEvent::new(store_id, sequence);
        event.writes = writes;
        event.deletes = deletes;
        event
    }

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
        wm.advance("store-1", 1);
        assert_eq!(wm.get("store-1"), Some(1));

        // With gap: get() returns the max of contiguous and pending
        wm.advance("store-1", 5);
        assert_eq!(wm.get("store-1"), Some(5));
    }

    #[test]
    fn test_sync_watermark_advance_only_moves_forward() {
        let wm = SyncWatermark::new();
        for seq in 1..=10 {
            wm.advance("store-1", seq);
        }
        wm.advance("store-1", 5); // Duplicate — should be no-op
        assert_eq!(wm.get("store-1"), Some(10));
        assert_eq!(wm.contiguous_position("store-1"), Some(10));

        // Advance to 15 sequentially
        for seq in 11..=15 {
            wm.advance("store-1", seq);
        }
        assert_eq!(wm.get("store-1"), Some(15));
    }

    #[test]
    fn test_sync_watermark_is_applied() {
        let wm = SyncWatermark::new();
        // Advance sequentially 1..=10 (contiguous watermark at 10)
        for seq in 1..=10 {
            wm.advance("store-1", seq);
        }

        assert!(wm.is_applied("store-1", 5)); // Before watermark
        assert!(wm.is_applied("store-1", 10)); // At watermark
        assert!(!wm.is_applied("store-1", 11)); // After watermark
        assert!(!wm.is_applied("store-2", 5)); // Unknown store
    }

    #[test]
    fn test_sync_watermark_multiple_stores() {
        let wm = SyncWatermark::new();
        // Advance each store sequentially
        for seq in 1..=10 {
            wm.advance("store-1", seq);
        }
        for seq in 1..=20 {
            wm.advance("store-2", seq);
        }
        for seq in 1..=5 {
            wm.advance("store-3", seq);
        }

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
    // Section 6: Out-of-Order Delivery Tests
    // ===========================================

    #[test]
    fn test_out_of_order_does_not_skip_unapplied_events() {
        // If we receive seq 5 before seq 3, seq 3 must NOT be considered "applied"
        let wm = SyncWatermark::new();
        wm.advance("store-1", 1);
        wm.advance("store-1", 2);
        wm.advance("store-1", 5); // Gap: 3, 4 not yet seen

        // Seq 3 was never applied — must return false
        assert!(!wm.is_applied("store-1", 3));
        assert!(!wm.is_applied("store-1", 4));

        // Seq 1, 2, 5 were applied
        assert!(wm.is_applied("store-1", 1));
        assert!(wm.is_applied("store-1", 2));
        assert!(wm.is_applied("store-1", 5));
    }

    #[test]
    fn test_contiguous_watermark_advances_when_gaps_fill() {
        let wm = SyncWatermark::new();
        wm.advance("store-1", 1);
        wm.advance("store-1", 3); // Gap at 2
        wm.advance("store-1", 5); // Gap at 4

        // Contiguous watermark should be at 1
        assert_eq!(wm.contiguous_position("store-1"), Some(1));

        // Fill gap at 2 — contiguous advances to 3
        wm.advance("store-1", 2);
        assert_eq!(wm.contiguous_position("store-1"), Some(3));

        // Fill gap at 4 — contiguous advances to 5
        wm.advance("store-1", 4);
        assert_eq!(wm.contiguous_position("store-1"), Some(5));
    }

    #[test]
    fn test_sequential_delivery_no_gaps() {
        let wm = SyncWatermark::new();
        for seq in 1..=10 {
            wm.advance("store-1", seq);
        }

        assert_eq!(wm.contiguous_position("store-1"), Some(10));
        for seq in 1..=10 {
            assert!(wm.is_applied("store-1", seq));
        }
        assert!(!wm.is_applied("store-1", 11));
    }

    #[test]
    fn test_reverse_delivery_order() {
        let wm = SyncWatermark::new();
        // Deliver in reverse: 5, 4, 3, 2, 1
        for seq in (1..=5).rev() {
            wm.advance("store-1", seq);
        }

        // All should be applied, contiguous watermark at 5
        assert_eq!(wm.contiguous_position("store-1"), Some(5));
        for seq in 1..=5 {
            assert!(wm.is_applied("store-1", seq));
        }
    }

    #[test]
    fn test_out_of_order_multi_store_independent() {
        let wm = SyncWatermark::new();
        wm.advance("store-a", 1);
        wm.advance("store-a", 3); // gap at 2 for store-a
        wm.advance("store-b", 1);
        wm.advance("store-b", 2); // no gap for store-b

        assert_eq!(wm.contiguous_position("store-a"), Some(1));
        assert_eq!(wm.contiguous_position("store-b"), Some(2));

        // store-a: 2 not applied, 3 applied
        assert!(!wm.is_applied("store-a", 2));
        assert!(wm.is_applied("store-a", 3));

        // store-b: both applied
        assert!(wm.is_applied("store-b", 1));
        assert!(wm.is_applied("store-b", 2));
    }

    #[test]
    fn test_duplicate_advance_is_idempotent() {
        let wm = SyncWatermark::new();
        wm.advance("store-1", 1);
        wm.advance("store-1", 3);
        wm.advance("store-1", 3); // duplicate
        wm.advance("store-1", 3); // duplicate

        assert_eq!(wm.contiguous_position("store-1"), Some(1));
        assert!(wm.is_applied("store-1", 3));
        assert!(!wm.is_applied("store-1", 2));
    }

    #[test]
    fn test_advance_sequence_zero_is_ignored() {
        let wm = SyncWatermark::new();
        wm.advance("store-1", 0);

        // Sequence 0 should be rejected — no state created
        assert!(wm.get("store-1").is_none());
        assert!(wm.contiguous_position("store-1").is_none());
        assert!(!wm.is_applied("store-1", 0));
    }

    #[test]
    fn test_pending_set_bounded_force_advances() {
        let wm = SyncWatermark::new();
        wm.advance("store-1", 1); // contiguous = 1

        // Insert MAX_PENDING_SET_SIZE + 1 entries with a gap at 2
        // Sequences: 3, 4, 5, ..., MAX_PENDING_SET_SIZE + 3
        for seq in 3..=(MAX_PENDING_SET_SIZE as u64 + 3) {
            wm.advance("store-1", seq);
        }

        // Pending set should have been force-trimmed.
        // After overflow, contiguous should have advanced past the gap.
        let contiguous = wm.contiguous_position("store-1").unwrap();
        assert!(
            contiguous > 1,
            "contiguous should have force-advanced past 1"
        );
        // The force-advance moves contiguous to the lowest pending (3), then drains
        // consecutive entries. Since all 3..=N are present, contiguous should be N.
        let expected = MAX_PENDING_SET_SIZE as u64 + 3;
        assert_eq!(contiguous, expected);
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

    // ===========================================
    // Section 7: Watermark Snapshot & Persistence Integration Tests
    // ===========================================

    #[test]
    fn test_watermark_to_snapshot_empty() {
        let wm = SyncWatermark::new();
        let snapshot = wm.to_snapshot();
        assert!(snapshot.positions.is_empty());
    }

    #[test]
    fn test_watermark_to_snapshot_contiguous_only() {
        let wm = SyncWatermark::new();
        for seq in 1..=5 {
            wm.advance("store-1", seq);
        }
        for seq in 1..=10 {
            wm.advance("store-2", seq);
        }

        let snapshot = wm.to_snapshot();
        assert_eq!(snapshot.positions.len(), 2);
        assert_eq!(snapshot.positions["store-1"], 5);
        assert_eq!(snapshot.positions["store-2"], 10);
    }

    #[test]
    fn test_watermark_to_snapshot_with_gaps_uses_contiguous() {
        let wm = SyncWatermark::new();
        wm.advance("store-1", 1);
        wm.advance("store-1", 2);
        wm.advance("store-1", 5); // gap at 3, 4
        wm.advance("store-1", 7); // gap at 6

        let snapshot = wm.to_snapshot();
        // Snapshot should only contain the contiguous position (2), not 5 or 7
        assert_eq!(snapshot.positions["store-1"], 2);
    }

    #[test]
    fn test_watermark_restore_from_snapshot() {
        let wm = SyncWatermark::new();
        let mut positions = HashMap::new();
        positions.insert("store-1".to_string(), 42);
        positions.insert("store-2".to_string(), 100);

        wm.restore(positions);

        // After restore, sequences 1..=42 are considered applied for store-1
        assert!(wm.is_applied("store-1", 1));
        assert!(wm.is_applied("store-1", 42));
        assert!(!wm.is_applied("store-1", 43));

        assert!(wm.is_applied("store-2", 100));
        assert!(!wm.is_applied("store-2", 101));
    }

    #[test]
    fn test_watermark_roundtrip_snapshot_restore() {
        // Build watermark with some state
        let wm1 = SyncWatermark::new();
        for seq in 1..=20 {
            wm1.advance("store-a", seq);
        }
        for seq in 1..=50 {
            wm1.advance("store-b", seq);
        }

        // Snapshot → Restore
        let snapshot = wm1.to_snapshot();
        let wm2 = SyncWatermark::new();
        wm2.restore(snapshot.positions);

        // wm2 should have the same contiguous positions
        assert_eq!(wm2.contiguous_position("store-a"), Some(20));
        assert_eq!(wm2.contiguous_position("store-b"), Some(50));
        assert!(wm2.is_applied("store-a", 20));
        assert!(!wm2.is_applied("store-a", 21));
    }

    #[tokio::test]
    async fn test_watermark_save_and_restore_via_store() {
        use crate::watermark_store::InMemoryWatermarkStore;

        let store = Arc::new(InMemoryWatermarkStore::new());

        // Build watermark, create snapshot, save
        let wm = SyncWatermark::new();
        for seq in 1..=15 {
            wm.advance("store-1", seq);
        }
        let snapshot = wm.to_snapshot();
        store.save(&snapshot).await.unwrap();

        // Load from store and restore into a new watermark
        let loaded = store.load().await.unwrap();
        let wm2 = SyncWatermark::new();
        wm2.restore(loaded.positions);

        assert_eq!(wm2.contiguous_position("store-1"), Some(15));
        assert!(wm2.is_applied("store-1", 15));
        assert!(!wm2.is_applied("store-1", 16));
    }

    // ===========================================
    // Section 8: Cache Invalidation Tests (2.0.6.3)
    // ===========================================

    #[tokio::test]
    async fn test_invalidate_cache_for_writes() {
        // When an event has tuple writes, cache entries for those tuples
        // should be invalidated.
        let cache = Arc::new(MockCacheInvalidator::new());
        let metrics = Arc::new(EdgeMetrics::new());

        let event = make_event(
            "store-1",
            1,
            vec![
                TupleOperation::new("user:alice", "viewer", "document:readme"),
                TupleOperation::new("user:bob", "editor", "document:design"),
            ],
            vec![],
        );

        invalidate_cache_for_event(cache.as_ref(), &metrics, &event).await;

        let calls = cache.calls().await;
        assert_eq!(calls.len(), 2, "Should invalidate for each written tuple");
        assert_eq!(
            calls[0],
            (
                "store-1".to_string(),
                "document:readme".to_string(),
                "viewer".to_string()
            )
        );
        assert_eq!(
            calls[1],
            (
                "store-1".to_string(),
                "document:design".to_string(),
                "editor".to_string()
            )
        );
    }

    #[tokio::test]
    async fn test_invalidate_cache_for_deletes() {
        // When an event has tuple deletes, cache entries for those tuples
        // should be invalidated.
        let cache = Arc::new(MockCacheInvalidator::new());
        let metrics = Arc::new(EdgeMetrics::new());

        let event = make_event(
            "store-2",
            5,
            vec![],
            vec![
                TupleKey::new("user:alice", "viewer", "document:readme"),
                TupleKey::new("team:eng#member", "editor", "folder:root"),
            ],
        );

        invalidate_cache_for_event(cache.as_ref(), &metrics, &event).await;

        let calls = cache.calls().await;
        assert_eq!(calls.len(), 2, "Should invalidate for each deleted tuple");
        assert_eq!(
            calls[0],
            (
                "store-2".to_string(),
                "document:readme".to_string(),
                "viewer".to_string()
            )
        );
        assert_eq!(
            calls[1],
            (
                "store-2".to_string(),
                "folder:root".to_string(),
                "editor".to_string()
            )
        );
    }

    #[tokio::test]
    async fn test_invalidate_cache_for_mixed_writes_and_deletes() {
        // Events with both writes and deletes should invalidate all affected tuples.
        let cache = Arc::new(MockCacheInvalidator::new());
        let metrics = Arc::new(EdgeMetrics::new());

        let event = make_event(
            "store-1",
            10,
            vec![TupleOperation::new("user:alice", "viewer", "document:new")],
            vec![TupleKey::new("user:bob", "editor", "document:old")],
        );

        invalidate_cache_for_event(cache.as_ref(), &metrics, &event).await;

        let calls = cache.calls().await;
        assert_eq!(
            calls.len(),
            2,
            "Should invalidate for both writes and deletes"
        );
        // Writes come first, then deletes
        assert_eq!(calls[0].1, "document:new");
        assert_eq!(calls[1].1, "document:old");
    }

    #[tokio::test]
    async fn test_invalidate_cache_records_metric() {
        // Each cache invalidation should increment the cache_invalidations metric.
        let cache = Arc::new(MockCacheInvalidator::new());
        let metrics = Arc::new(EdgeMetrics::new());

        let event = make_event(
            "store-1",
            1,
            vec![
                TupleOperation::new("user:alice", "viewer", "document:a"),
                TupleOperation::new("user:bob", "editor", "document:b"),
            ],
            vec![TupleKey::new("user:carol", "admin", "document:c")],
        );

        invalidate_cache_for_event(cache.as_ref(), &metrics, &event).await;

        assert_eq!(
            metrics.cache_invalidations.load(Ordering::Relaxed),
            3,
            "Should record one metric per invalidation (2 writes + 1 delete)"
        );
    }

    #[tokio::test]
    async fn test_invalidate_cache_empty_event_is_noop() {
        // An event with no writes or deletes should not trigger any invalidation.
        let cache = Arc::new(MockCacheInvalidator::new());
        let metrics = Arc::new(EdgeMetrics::new());

        let event = make_event("store-1", 1, vec![], vec![]);

        invalidate_cache_for_event(cache.as_ref(), &metrics, &event).await;

        let calls = cache.calls().await;
        assert!(calls.is_empty(), "No invalidation for empty events");
        assert_eq!(metrics.cache_invalidations.load(Ordering::Relaxed), 0);
    }
}
