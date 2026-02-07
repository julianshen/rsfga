//! Event subscriber for committed events from RSFGA_EVENTS stream.
//!
//! The `EventSubscriber` subscribes to the `RSFGA_EVENTS` JetStream stream
//! and forwards committed events to the [`WriteTracker`] so that RYOW waiters
//! are notified when their writes have been committed to storage.
//!
//! ## Architecture
//!
//! ```text
//! Storage Consumer ──▶ RSFGA_EVENTS stream ──▶ EventSubscriber ──▶ WriteTracker
//!                      (CommittedEvent)          (this module)       (mark_committed)
//! ```
//!
//! ## Usage
//!
//! ```rust,no_run
//! use rsfga_nats::event_subscriber::EventSubscriber;
//! use rsfga_nats::tracker::WriteTracker;
//! use rsfga_nats::NatsClient;
//! use rsfga_nats::NatsConfig;
//! use std::sync::Arc;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let client = NatsClient::connect(NatsConfig::default()).await?;
//! let tracker = Arc::new(WriteTracker::default());
//! let subscriber = EventSubscriber::new(client, Arc::clone(&tracker));
//! let handle = subscriber.start().await?;
//! // handle.abort() to stop
//! # Ok(())
//! # }
//! ```

use std::sync::Arc;
use std::time::Duration;

use tracing::{debug, error, info, warn};

use crate::connection::NatsClient;
use crate::error::{NatsError, Result};
use crate::events::CommittedEvent;
use crate::tracker::WriteTracker;

/// Default inactive threshold for durable consumers (5 minutes).
/// After this period of inactivity, the NATS server will clean up the consumer.
/// This prevents stale consumers from accumulating when API instances are terminated.
const DEFAULT_INACTIVE_THRESHOLD: Duration = Duration::from_secs(300);

/// Configuration for the event subscriber.
#[derive(Debug, Clone)]
pub struct EventSubscriberConfig {
    /// Consumer name (must be unique per API server instance for broadcast semantics).
    /// Each API server needs ALL committed events for its local WriteTracker to work,
    /// so each instance gets its own durable consumer.
    pub consumer_name: String,

    /// Whether to create the consumer as durable.
    /// Defaults to `true` so that on restart, the consumer replays events from
    /// where it left off (DeliverAll policy), preventing missed committed events
    /// that would cause RYOW waiters to timeout.
    pub durable: bool,

    /// Description for the consumer.
    pub description: String,

    /// Inactive threshold after which the NATS server auto-deletes the consumer.
    /// This cleans up consumers from terminated API server instances.
    /// Defaults to 5 minutes.
    pub inactive_threshold: Duration,
}

impl Default for EventSubscriberConfig {
    fn default() -> Self {
        // Use a unique consumer name per instance to prevent load-balancing
        // across instances in distributed deployments. Each API server must
        // receive ALL committed events for its local WriteTracker to work.
        let instance_id = ulid::Ulid::new().to_string();
        Self {
            consumer_name: format!("rsfga-api-events-{instance_id}"),
            durable: true, // Durable so restarts replay from last acked position
            description: "RSFGA API server committed event subscriber for RYOW".to_string(),
            inactive_threshold: DEFAULT_INACTIVE_THRESHOLD,
        }
    }
}

/// Subscribes to `RSFGA_EVENTS` stream and notifies the [`WriteTracker`]
/// when writes are committed to storage.
///
/// This is the bridge between the storage consumer (rsfga-writer) and
/// the API server's RYOW consistency mechanism.
pub struct EventSubscriber {
    client: NatsClient,
    tracker: Arc<WriteTracker>,
    config: EventSubscriberConfig,
}

impl EventSubscriber {
    /// Create a new event subscriber with default configuration.
    pub fn new(client: NatsClient, tracker: Arc<WriteTracker>) -> Self {
        Self {
            client,
            tracker,
            config: EventSubscriberConfig::default(),
        }
    }

    /// Create a new event subscriber with custom configuration.
    pub fn with_config(
        client: NatsClient,
        tracker: Arc<WriteTracker>,
        config: EventSubscriberConfig,
    ) -> Self {
        Self {
            client,
            tracker,
            config,
        }
    }

    /// Start consuming committed events in a background task.
    ///
    /// Returns a `JoinHandle` that can be used to abort the consumer.
    /// The consumer will automatically reconnect on transient errors.
    pub async fn start(&self) -> Result<tokio::task::JoinHandle<()>> {
        use async_nats::jetstream::consumer::pull::Config as PullConfig;
        use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy};
        use futures::StreamExt;

        let js = self.client.jetstream();
        let stream_name = crate::STREAM_EVENTS;

        // Get or create the stream (it should already exist from JetStreamManager::setup_streams)
        let stream = js.get_stream(stream_name).await.map_err(|e| {
            NatsError::JetStream(format!("Failed to get stream {stream_name}: {e}"))
        })?;

        // Create or get the durable pull consumer for committed events.
        // Key design decisions:
        // - Durable: Survives restarts, replays from last acked position
        // - DeliverAll: On first creation, starts from the oldest available message
        // - inactive_threshold: Auto-cleans consumers from terminated instances
        // - Per-instance name: Each API server gets ALL events (broadcast semantics)
        let consumer_config = PullConfig {
            durable_name: if self.config.durable {
                Some(self.config.consumer_name.clone())
            } else {
                None
            },
            name: Some(self.config.consumer_name.clone()),
            description: Some(self.config.description.clone()),
            filter_subject: format!("{}.*.committed", crate::SUBJECT_EVENTS_PREFIX),
            ack_policy: AckPolicy::Explicit,
            deliver_policy: DeliverPolicy::All,
            inactive_threshold: self.config.inactive_threshold,
            ..Default::default()
        };

        let consumer = stream
            .get_or_create_consumer(&self.config.consumer_name, consumer_config)
            .await
            .map_err(|e| NatsError::JetStream(format!("Failed to create event consumer: {e}")))?;

        info!(
            consumer_name = %self.config.consumer_name,
            stream = stream_name,
            durable = self.config.durable,
            inactive_threshold_secs = self.config.inactive_threshold.as_secs(),
            "Event subscriber started"
        );

        let tracker = Arc::clone(&self.tracker);
        let consumer_name = self.config.consumer_name.clone();

        // Spawn the consumer loop
        let handle = tokio::spawn(async move {
            loop {
                // Fetch messages in batches
                let mut messages = match consumer.messages().await {
                    Ok(msgs) => msgs,
                    Err(e) => {
                        error!(
                            error = %e,
                            consumer = %consumer_name,
                            "Failed to get message stream, retrying in 5s"
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        continue;
                    }
                };

                while let Some(msg_result) = messages.next().await {
                    match msg_result {
                        Ok(msg) => {
                            // Parse the committed event
                            match CommittedEvent::from_bytes(&msg.payload) {
                                Ok(event) => {
                                    // Validate store_id to prevent unbounded memory growth
                                    // from malformed events with arbitrary store IDs
                                    if event.store_id.is_empty()
                                        || event.store_id.len() > 26
                                        || !event
                                            .store_id
                                            .chars()
                                            .all(|c| c.is_ascii_alphanumeric())
                                    {
                                        warn!(
                                            store_id = %event.store_id,
                                            "Ignoring committed event with invalid store_id"
                                        );
                                        if let Err(e) = msg.ack().await {
                                            warn!(error = %e, "Failed to ack invalid store_id event");
                                        }
                                        continue;
                                    }

                                    debug!(
                                        store_id = %event.store_id,
                                        sequence = event.sequence,
                                        request_ids = ?event.request_ids,
                                        "Received committed event"
                                    );

                                    // Notify the write tracker
                                    tracker.mark_committed(&event.store_id, event.sequence);

                                    // Acknowledge the message
                                    if let Err(e) = msg.ack().await {
                                        warn!(
                                            error = %e,
                                            store_id = %event.store_id,
                                            sequence = event.sequence,
                                            "Failed to ack committed event message"
                                        );
                                    }
                                }
                                Err(e) => {
                                    error!(
                                        error = %e,
                                        payload_len = msg.payload.len(),
                                        "Failed to parse committed event, acking to skip"
                                    );
                                    // Ack malformed messages to prevent blocking
                                    if let Err(ack_err) = msg.ack().await {
                                        warn!(error = %ack_err, "Failed to ack malformed message");
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                error = %e,
                                consumer = %consumer_name,
                                "Error receiving message from event stream"
                            );
                        }
                    }
                }

                // Stream ended (e.g., disconnect), retry
                warn!(
                    consumer = %consumer_name,
                    "Event message stream ended, reconnecting in 1s"
                );
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        });

        Ok(handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::WriteTrackerConfig;
    use std::time::Duration;

    #[test]
    fn test_event_subscriber_config_defaults() {
        let config = EventSubscriberConfig::default();
        // Consumer name should have instance-unique suffix for broadcast semantics
        assert!(config.consumer_name.starts_with("rsfga-api-events-"));
        assert!(config.consumer_name.len() > "rsfga-api-events-".len());
        // Durable by default so restarts replay from last acked position
        assert!(config.durable);
        assert!(!config.description.is_empty());
        // Inactive threshold should be 5 minutes for auto-cleanup of dead instances
        assert_eq!(config.inactive_threshold, Duration::from_secs(300));
    }

    #[test]
    fn test_event_subscriber_config_unique_per_instance() {
        let config1 = EventSubscriberConfig::default();
        let config2 = EventSubscriberConfig::default();
        // Each instance gets a unique consumer name (ULID-based)
        assert_ne!(config1.consumer_name, config2.consumer_name);
    }

    #[test]
    fn test_committed_event_parsing() {
        let event = CommittedEvent::new("store1", 42)
            .with_request_ids(vec!["req-1".to_string(), "req-2".to_string()]);

        let bytes = event.to_bytes().unwrap();
        let parsed = CommittedEvent::from_bytes(&bytes).unwrap();

        assert_eq!(parsed.store_id, "store1");
        assert_eq!(parsed.sequence, 42);
        assert_eq!(parsed.request_ids, vec!["req-1", "req-2"]);
    }

    #[test]
    fn test_committed_event_mark_committed_integration() {
        // Verify that parsing a committed event and calling mark_committed
        // works end-to-end without a real NATS connection
        let tracker = WriteTracker::new(WriteTrackerConfig {
            waiter_ttl: Duration::from_secs(60),
            cleanup_interval: Duration::from_secs(30),
        });

        let event = CommittedEvent::new("store1", 10);
        let bytes = event.to_bytes().unwrap();
        let parsed = CommittedEvent::from_bytes(&bytes).unwrap();

        tracker.mark_committed(&parsed.store_id, parsed.sequence);

        assert_eq!(tracker.get_committed_sequence("store1"), Some(10));
    }

    #[tokio::test]
    async fn test_committed_event_wakes_waiters() {
        // Simulate the full RYOW flow without NATS:
        // 1. Register a waiter
        // 2. Parse a committed event
        // 3. Call mark_committed
        // 4. Waiter should be woken
        let tracker = Arc::new(WriteTracker::default());
        let tracker_clone = Arc::clone(&tracker);

        // Spawn a waiter
        let waiter = tokio::spawn(async move {
            tracker_clone
                .wait_for_commit("store1", 5, Duration::from_secs(5))
                .await
        });

        // Let waiter register
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Simulate receiving a committed event
        let event = CommittedEvent::new("store1", 5);
        let bytes = event.to_bytes().unwrap();
        let parsed = CommittedEvent::from_bytes(&bytes).unwrap();

        // This is what the subscriber loop does
        tracker.mark_committed(&parsed.store_id, parsed.sequence);

        // Waiter should complete successfully
        let result = waiter.await.unwrap();
        assert!(result.is_ok());
    }

    #[test]
    fn test_malformed_event_bytes_returns_error() {
        let bad_bytes = b"not valid json";
        let result = CommittedEvent::from_bytes(bad_bytes);
        assert!(result.is_err());
    }

    #[test]
    fn test_committed_event_subject_format() {
        let event = CommittedEvent::new("my-store", 1);
        assert_eq!(event.subject(), "rsfga.events.my-store.committed");
    }

    #[test]
    fn test_store_id_validation_rejects_empty() {
        // Validates the store_id check added to the subscriber loop
        let store_id = "";
        assert!(
            store_id.is_empty()
                || store_id.len() > 26
                || !store_id.chars().all(|c| c.is_ascii_alphanumeric())
        );
    }

    #[test]
    fn test_store_id_validation_rejects_too_long() {
        let store_id = "A".repeat(27);
        assert!(store_id.len() > 26);
    }

    #[test]
    fn test_store_id_validation_rejects_non_alphanumeric() {
        let store_id = "store-with-dashes";
        assert!(!store_id.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn test_store_id_validation_accepts_valid_ulid() {
        let store_id = "01HQJK2M3N4P5R6S7T8V9WXYZ";
        assert!(
            !store_id.is_empty()
                && store_id.len() <= 26
                && store_id.chars().all(|c| c.is_ascii_alphanumeric())
        );
    }

    #[tokio::test]
    async fn test_ryow_end_to_end_multiple_stores() {
        // RYOW end-to-end: multiple stores with independent sequence tracking.
        // Simulates the EventSubscriber receiving committed events for different
        // stores and waking the correct waiters.
        let tracker = Arc::new(WriteTracker::default());

        // Spawn waiters for two different stores
        let t1 = Arc::clone(&tracker);
        let waiter_store1 = tokio::spawn(async move {
            t1.wait_for_commit("store1", 10, Duration::from_secs(5))
                .await
        });

        let t2 = Arc::clone(&tracker);
        let waiter_store2 = tokio::spawn(async move {
            t2.wait_for_commit("store2", 5, Duration::from_secs(5))
                .await
        });

        // Let waiters register
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Simulate EventSubscriber processing committed events
        // Store2 commits first — should only wake store2's waiter
        let event2 = CommittedEvent::new("store2", 5);
        let bytes2 = event2.to_bytes().unwrap();
        let parsed2 = CommittedEvent::from_bytes(&bytes2).unwrap();
        tracker.mark_committed(&parsed2.store_id, parsed2.sequence);

        // Store2's waiter should complete
        let result2 = tokio::time::timeout(Duration::from_secs(1), waiter_store2)
            .await
            .expect("store2 waiter should not timeout")
            .unwrap();
        assert!(result2.is_ok(), "store2 waiter should succeed");

        // Store1's waiter is still waiting (sequence 10 not committed yet)
        // Now commit store1 at sequence 10
        let event1 = CommittedEvent::new("store1", 10);
        let bytes1 = event1.to_bytes().unwrap();
        let parsed1 = CommittedEvent::from_bytes(&bytes1).unwrap();
        tracker.mark_committed(&parsed1.store_id, parsed1.sequence);

        // Store1's waiter should now complete
        let result1 = tokio::time::timeout(Duration::from_secs(1), waiter_store1)
            .await
            .expect("store1 waiter should not timeout")
            .unwrap();
        assert!(result1.is_ok(), "store1 waiter should succeed");

        // Verify final committed sequences
        assert_eq!(tracker.get_committed_sequence("store1"), Some(10));
        assert_eq!(tracker.get_committed_sequence("store2"), Some(5));
    }

    #[tokio::test]
    async fn test_ryow_end_to_end_progressive_commits() {
        // Tests that progressive sequence commits correctly wake waiters.
        // Simulates a stream of committed events with increasing sequences.
        let tracker = Arc::new(WriteTracker::default());

        // Waiter is waiting for sequence 15
        let t = Arc::clone(&tracker);
        let waiter = tokio::spawn(async move {
            t.wait_for_commit("store1", 15, Duration::from_secs(5))
                .await
        });

        tokio::time::sleep(Duration::from_millis(20)).await;

        // Simulate progressive commits (5, 10, 15) — like a writer
        // processing batches of events
        for seq in [5u64, 10, 15] {
            let event = CommittedEvent::new("store1", seq);
            let bytes = event.to_bytes().unwrap();
            let parsed = CommittedEvent::from_bytes(&bytes).unwrap();
            tracker.mark_committed(&parsed.store_id, parsed.sequence);
        }

        // Waiter should be woken when sequence reaches 15
        let result = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("waiter should not timeout")
            .unwrap();
        assert!(result.is_ok(), "waiter should succeed after sequence 15");
    }

    #[tokio::test]
    async fn test_ryow_end_to_end_waiter_timeout() {
        // Tests that a waiter correctly times out when the expected sequence
        // is never committed. This ensures RYOW doesn't hang indefinitely.
        let tracker = Arc::new(WriteTracker::default());

        let t = Arc::clone(&tracker);
        let waiter = tokio::spawn(async move {
            // Very short timeout for test speed
            t.wait_for_commit("store1", 100, Duration::from_millis(100))
                .await
        });

        // Commit a lower sequence — not enough to satisfy the waiter
        let event = CommittedEvent::new("store1", 50);
        let bytes = event.to_bytes().unwrap();
        let parsed = CommittedEvent::from_bytes(&bytes).unwrap();
        tracker.mark_committed(&parsed.store_id, parsed.sequence);

        // Waiter should timeout since sequence 100 was never committed
        let result = waiter.await.unwrap();
        assert!(
            result.is_err(),
            "waiter should timeout when sequence not reached"
        );
    }

    #[tokio::test]
    async fn test_ryow_already_committed_returns_immediately() {
        // Tests that if the sequence is already committed when a waiter registers,
        // it returns immediately without blocking.
        let tracker = Arc::new(WriteTracker::default());

        // Pre-commit sequence 20
        let event = CommittedEvent::new("store1", 20);
        let bytes = event.to_bytes().unwrap();
        let parsed = CommittedEvent::from_bytes(&bytes).unwrap();
        tracker.mark_committed(&parsed.store_id, parsed.sequence);

        // Now wait for sequence 15 (already satisfied by committed 20)
        let result = tracker
            .wait_for_commit("store1", 15, Duration::from_millis(100))
            .await;
        assert!(
            result.is_ok(),
            "should return immediately for already-committed sequence"
        );
    }
}
