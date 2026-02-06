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

use tracing::{debug, error, info, warn};

use crate::connection::NatsClient;
use crate::error::{NatsError, Result};
use crate::events::CommittedEvent;
use crate::tracker::WriteTracker;

/// Configuration for the event subscriber.
#[derive(Debug, Clone)]
pub struct EventSubscriberConfig {
    /// Consumer name (must be unique per API server instance for push-based,
    /// or shared for pull-based load balancing).
    pub consumer_name: String,

    /// Whether to create the consumer as durable.
    pub durable: bool,

    /// Description for the consumer.
    pub description: String,
}

impl Default for EventSubscriberConfig {
    fn default() -> Self {
        Self {
            consumer_name: "rsfga-api-events".to_string(),
            durable: true,
            description: "RSFGA API server committed event subscriber for RYOW".to_string(),
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
        use async_nats::jetstream::consumer::AckPolicy;
        use futures::StreamExt;

        let js = self.client.jetstream();
        let stream_name = crate::STREAM_EVENTS;

        // Get or create the stream (it should already exist from JetStreamManager::setup_streams)
        let stream = js.get_stream(stream_name).await.map_err(|e| {
            NatsError::JetStream(format!("Failed to get stream {stream_name}: {e}"))
        })?;

        // Create or get the pull consumer for committed events
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
            ..Default::default()
        };

        let consumer = stream
            .get_or_create_consumer(&self.config.consumer_name, consumer_config)
            .await
            .map_err(|e| NatsError::JetStream(format!("Failed to create event consumer: {e}")))?;

        info!(
            consumer_name = %self.config.consumer_name,
            stream = stream_name,
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
        assert_eq!(config.consumer_name, "rsfga-api-events");
        assert!(config.durable);
        assert!(!config.description.is_empty());
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
}
