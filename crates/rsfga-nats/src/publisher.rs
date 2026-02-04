//! Event publisher for NATS JetStream.
//!
//! This module provides the `EventPublisher` for publishing write requests
//! and committed events to JetStream streams.

use crate::connection::NatsClient;
use crate::error::{NatsError, Result};
use crate::events::{CommittedEvent, WriteRequest, WriteTicket};
use async_nats::jetstream::publish::PublishAck;
use bytes::Bytes;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::timeout;
use tracing::{debug, info, instrument, warn};

/// Event publisher for NATS JetStream.
///
/// Provides methods for publishing write requests and committed events
/// with automatic retry, circuit breaker, and metrics.
#[derive(Clone)]
pub struct EventPublisher {
    client: NatsClient,
    inner: Arc<PublisherInner>,
}

struct PublisherInner {
    /// Circuit breaker state
    circuit_breaker: RwLock<CircuitBreakerState>,
    /// Metrics
    metrics: PublisherMetrics,
    /// Configuration
    config: PublisherConfig,
}

/// Publisher configuration.
#[derive(Debug, Clone)]
pub struct PublisherConfig {
    /// Timeout for publish operations
    pub publish_timeout: Duration,
    /// Maximum retries for transient failures
    pub max_retries: u32,
    /// Initial retry delay (exponential backoff)
    pub retry_delay: Duration,
    /// Circuit breaker failure threshold
    pub circuit_breaker_threshold: u32,
    /// Circuit breaker reset timeout
    pub circuit_breaker_reset: Duration,
}

impl Default for PublisherConfig {
    fn default() -> Self {
        Self {
            publish_timeout: Duration::from_secs(5),
            max_retries: 3,
            retry_delay: Duration::from_millis(100),
            circuit_breaker_threshold: 5,
            circuit_breaker_reset: Duration::from_secs(30),
        }
    }
}

/// Circuit breaker state.
#[derive(Debug, Clone, Default)]
struct CircuitBreakerState {
    /// Whether the circuit is open (rejecting requests)
    open: bool,
    /// Number of consecutive failures
    failure_count: u32,
    /// When the circuit was opened
    opened_at: Option<std::time::Instant>,
}

/// Publisher metrics.
#[derive(Debug, Default)]
struct PublisherMetrics {
    /// Total publish attempts
    publish_attempts: AtomicU64,
    /// Successful publishes
    publish_success: AtomicU64,
    /// Failed publishes
    publish_failures: AtomicU64,
    /// Circuit breaker rejections
    circuit_breaker_rejections: AtomicU64,
    /// Retries
    retries: AtomicU64,
}

impl EventPublisher {
    /// Create a new event publisher.
    pub fn new(client: NatsClient) -> Self {
        Self::with_config(client, PublisherConfig::default())
    }

    /// Create a new event publisher with custom configuration.
    pub fn with_config(client: NatsClient, config: PublisherConfig) -> Self {
        Self {
            client,
            inner: Arc::new(PublisherInner {
                circuit_breaker: RwLock::new(CircuitBreakerState::default()),
                metrics: PublisherMetrics::default(),
                config,
            }),
        }
    }

    /// Publish a write request to RSFGA_WRITES stream.
    ///
    /// Returns a `WriteTicket` that can be used for read-your-own-writes.
    #[instrument(skip(self, request), fields(store_id = %request.store_id, request_id = %request.request_id))]
    pub async fn publish_write_request(&self, request: &WriteRequest) -> Result<WriteTicket> {
        // Check circuit breaker
        self.check_circuit_breaker().await?;

        self.inner
            .metrics
            .publish_attempts
            .fetch_add(1, Ordering::Relaxed);

        let subject = request.subject();
        let payload = request.to_bytes()?;

        // Publish with retry
        let ack = self
            .publish_with_retry(&subject, Bytes::from(payload))
            .await?;

        // Record success
        self.record_success().await;

        debug!(
            subject = %subject,
            sequence = ack.sequence,
            stream = %ack.stream,
            "Write request published"
        );

        // Create write ticket
        let ticket = WriteTicket::new(&request.store_id, ack.sequence, &request.request_id);

        Ok(ticket)
    }

    /// Publish a committed event to RSFGA_EVENTS stream.
    #[instrument(skip(self, event), fields(store_id = %event.store_id, sequence = event.sequence))]
    pub async fn publish_committed_event(&self, event: &CommittedEvent) -> Result<PublishAck> {
        // Check circuit breaker
        self.check_circuit_breaker().await?;

        self.inner
            .metrics
            .publish_attempts
            .fetch_add(1, Ordering::Relaxed);

        let subject = event.subject();
        let payload = event.to_bytes()?;

        // Publish with retry
        let ack = self
            .publish_with_retry(&subject, Bytes::from(payload))
            .await?;

        // Record success
        self.record_success().await;

        debug!(
            subject = %subject,
            sequence = ack.sequence,
            stream = %ack.stream,
            "Committed event published"
        );

        Ok(ack)
    }

    /// Publish with retry and timeout.
    async fn publish_with_retry(&self, subject: &str, payload: Bytes) -> Result<PublishAck> {
        let mut last_error = None;
        let config = &self.inner.config;

        for attempt in 0..=config.max_retries {
            if attempt > 0 {
                self.inner.metrics.retries.fetch_add(1, Ordering::Relaxed);
                let delay = config.retry_delay * 2u32.pow(attempt - 1);
                debug!(attempt, delay_ms = delay.as_millis(), "Retrying publish");
                tokio::time::sleep(delay).await;
            }

            match timeout(
                config.publish_timeout,
                self.client
                    .jetstream()
                    .publish(subject.to_string(), payload.clone()),
            )
            .await
            {
                Ok(Ok(ack_future)) => {
                    // Wait for the ack
                    match timeout(config.publish_timeout, ack_future).await {
                        Ok(Ok(ack)) => return Ok(ack),
                        Ok(Err(e)) => {
                            warn!(attempt, error = %e, "Publish ack failed");
                            last_error = Some(NatsError::Publish {
                                subject: subject.to_string(),
                                reason: e.to_string(),
                            });
                        }
                        Err(_) => {
                            warn!(attempt, "Publish ack timeout");
                            last_error = Some(NatsError::Timeout {
                                operation: "publish_ack".to_string(),
                                duration_ms: config.publish_timeout.as_millis() as u64,
                            });
                        }
                    }
                }
                Ok(Err(e)) => {
                    warn!(attempt, error = %e, "Publish failed");
                    last_error = Some(NatsError::Publish {
                        subject: subject.to_string(),
                        reason: e.to_string(),
                    });
                }
                Err(_) => {
                    warn!(attempt, "Publish timeout");
                    last_error = Some(NatsError::Timeout {
                        operation: "publish".to_string(),
                        duration_ms: config.publish_timeout.as_millis() as u64,
                    });
                }
            }
        }

        // All retries failed
        self.record_failure().await;
        Err(last_error.unwrap_or_else(|| NatsError::Internal("Unknown publish error".to_string())))
    }

    /// Check if circuit breaker allows the request.
    async fn check_circuit_breaker(&self) -> Result<()> {
        let mut state = self.inner.circuit_breaker.write().await;
        let config = &self.inner.config;

        if state.open {
            // Check if we should try to reset
            if let Some(opened_at) = state.opened_at {
                if opened_at.elapsed() > config.circuit_breaker_reset {
                    debug!("Circuit breaker attempting reset (half-open)");
                    state.open = false;
                    state.failure_count = 0;
                    state.opened_at = None;
                } else {
                    self.inner
                        .metrics
                        .circuit_breaker_rejections
                        .fetch_add(1, Ordering::Relaxed);
                    return Err(NatsError::CircuitBreakerOpen);
                }
            }
        }

        Ok(())
    }

    /// Record a successful publish.
    async fn record_success(&self) {
        self.inner
            .metrics
            .publish_success
            .fetch_add(1, Ordering::Relaxed);

        let mut state = self.inner.circuit_breaker.write().await;
        state.failure_count = 0;
    }

    /// Record a failed publish.
    async fn record_failure(&self) {
        self.inner
            .metrics
            .publish_failures
            .fetch_add(1, Ordering::Relaxed);

        let mut state = self.inner.circuit_breaker.write().await;
        state.failure_count += 1;

        if state.failure_count >= self.inner.config.circuit_breaker_threshold && !state.open {
            warn!(failures = state.failure_count, "Circuit breaker opened");
            state.open = true;
            state.opened_at = Some(std::time::Instant::now());
        }
    }

    /// Get publisher metrics.
    pub fn metrics(&self) -> PublisherStats {
        PublisherStats {
            publish_attempts: self.inner.metrics.publish_attempts.load(Ordering::Relaxed),
            publish_success: self.inner.metrics.publish_success.load(Ordering::Relaxed),
            publish_failures: self.inner.metrics.publish_failures.load(Ordering::Relaxed),
            circuit_breaker_rejections: self
                .inner
                .metrics
                .circuit_breaker_rejections
                .load(Ordering::Relaxed),
            retries: self.inner.metrics.retries.load(Ordering::Relaxed),
        }
    }

    /// Check if the circuit breaker is currently open.
    pub async fn is_circuit_open(&self) -> bool {
        self.inner.circuit_breaker.read().await.open
    }

    /// Manually reset the circuit breaker.
    pub async fn reset_circuit_breaker(&self) {
        let mut state = self.inner.circuit_breaker.write().await;
        state.open = false;
        state.failure_count = 0;
        state.opened_at = None;
        info!("Circuit breaker manually reset");
    }
}

/// Publisher statistics.
#[derive(Debug, Clone)]
pub struct PublisherStats {
    pub publish_attempts: u64,
    pub publish_success: u64,
    pub publish_failures: u64,
    pub circuit_breaker_rejections: u64,
    pub retries: u64,
}

impl PublisherStats {
    /// Calculate success rate (0.0 - 1.0).
    pub fn success_rate(&self) -> f64 {
        if self.publish_attempts == 0 {
            1.0
        } else {
            self.publish_success as f64 / self.publish_attempts as f64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::TupleOperation;

    #[test]
    fn test_publisher_config_defaults() {
        let config = PublisherConfig::default();
        assert_eq!(config.publish_timeout, Duration::from_secs(5));
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.circuit_breaker_threshold, 5);
    }

    #[test]
    fn test_circuit_breaker_state_default() {
        let state = CircuitBreakerState::default();
        assert!(!state.open);
        assert_eq!(state.failure_count, 0);
        assert!(state.opened_at.is_none());
    }

    #[test]
    fn test_publisher_stats_success_rate() {
        let stats = PublisherStats {
            publish_attempts: 100,
            publish_success: 95,
            publish_failures: 5,
            circuit_breaker_rejections: 0,
            retries: 10,
        };

        assert!((stats.success_rate() - 0.95).abs() < f64::EPSILON);
    }

    #[test]
    fn test_publisher_stats_zero_attempts() {
        let stats = PublisherStats {
            publish_attempts: 0,
            publish_success: 0,
            publish_failures: 0,
            circuit_breaker_rejections: 0,
            retries: 0,
        };

        assert!((stats.success_rate() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_write_request_subject() {
        let request = WriteRequest::new("my-store").write(TupleOperation::new(
            "user:alice",
            "viewer",
            "document:readme",
        ));

        assert_eq!(request.subject(), "rsfga.writes.my-store");
    }
}
