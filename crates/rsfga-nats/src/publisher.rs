//! Event publisher for NATS JetStream.
//!
//! This module provides the `EventPublisher` for publishing write requests
//! and committed events to JetStream streams.

use crate::connection::NatsClient;
use crate::error::{NatsError, Result};
use crate::events::{CommittedEvent, ModelWriteRequest, WriteRequest, WriteTicket};
use async_nats::jetstream::context::Publish;
use async_nats::jetstream::publish::PublishAck;
use bytes::Bytes;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
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
    /// Circuit breaker state (atomic for lock-free operation)
    circuit_breaker: AtomicCircuitBreaker,
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
    /// Write ticket TTL (how long tickets are valid for RYOW consistency)
    pub write_ticket_ttl: Duration,
}

impl Default for PublisherConfig {
    fn default() -> Self {
        Self {
            publish_timeout: Duration::from_secs(5),
            max_retries: 3,
            retry_delay: Duration::from_millis(100),
            circuit_breaker_threshold: 5,
            circuit_breaker_reset: Duration::from_secs(30),
            write_ticket_ttl: Duration::from_secs(60),
        }
    }
}

/// Atomic circuit breaker for lock-free operation.
///
/// Uses atomic operations to avoid race conditions between checking
/// the circuit state and updating it after failures/successes.
struct AtomicCircuitBreaker {
    /// Whether the circuit is open (rejecting requests)
    open: AtomicBool,
    /// Number of consecutive failures
    failure_count: AtomicU32,
    /// Timestamp when circuit was opened (milliseconds since process start, 0 = not set)
    opened_at_ms: AtomicU64,
    /// Reference instant for computing elapsed time
    epoch: std::time::Instant,
}

impl Default for AtomicCircuitBreaker {
    fn default() -> Self {
        Self {
            open: AtomicBool::new(false),
            failure_count: AtomicU32::new(0),
            opened_at_ms: AtomicU64::new(0),
            epoch: std::time::Instant::now(),
        }
    }
}

impl AtomicCircuitBreaker {
    /// Check if circuit allows requests, attempting reset if timeout elapsed.
    ///
    /// Performance: Fast path (circuit closed) is a single atomic load.
    /// When open, elapsed time calculation is O(1) on most systems.
    /// Caching reset time was considered but adds complexity for marginal gain
    /// since the circuit is typically closed during normal operation.
    fn check(&self, reset_timeout: Duration) -> Result<()> {
        // Fast path: circuit closed (most common case)
        if !self.open.load(Ordering::Acquire) {
            return Ok(());
        }

        // Circuit is open - check if we should attempt reset
        let opened_at_ms = self.opened_at_ms.load(Ordering::Acquire);
        if opened_at_ms == 0 {
            // Shouldn't happen, but handle gracefully
            return Ok(());
        }

        let elapsed_ms = self.epoch.elapsed().as_millis() as u64 - opened_at_ms;
        if elapsed_ms > reset_timeout.as_millis() as u64 {
            // Try to transition to half-open (reset)
            // Use compare_exchange to ensure only one thread resets
            if self
                .open
                .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                self.failure_count.store(0, Ordering::Release);
                self.opened_at_ms.store(0, Ordering::Release);
                debug!("Circuit breaker reset (half-open)");
            }
            return Ok(());
        }

        Err(NatsError::CircuitBreakerOpen)
    }

    /// Record a successful operation (resets failure count).
    fn record_success(&self) {
        self.failure_count.store(0, Ordering::Release);
    }

    /// Record a failed operation, potentially opening the circuit.
    fn record_failure(&self, threshold: u32) {
        // Increment failure count atomically
        let failures = self.failure_count.fetch_add(1, Ordering::AcqRel) + 1;

        if failures >= threshold {
            // Try to open the circuit (only one thread will succeed)
            if self
                .open
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                let now_ms = self.epoch.elapsed().as_millis() as u64;
                self.opened_at_ms.store(now_ms, Ordering::Release);
                warn!(failures, "Circuit breaker opened");
            }
        }
    }

    /// Check if circuit is currently open.
    fn is_open(&self) -> bool {
        self.open.load(Ordering::Acquire)
    }

    /// Manually reset the circuit.
    fn reset(&self) {
        self.open.store(false, Ordering::Release);
        self.failure_count.store(0, Ordering::Release);
        self.opened_at_ms.store(0, Ordering::Release);
    }
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
                circuit_breaker: AtomicCircuitBreaker::default(),
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
        self.check_circuit_breaker()?;

        self.inner
            .metrics
            .publish_attempts
            .fetch_add(1, Ordering::Relaxed);

        let subject = request.subject();
        let payload = request.to_bytes()?;

        // Use request_id as message ID for deduplication during retries
        let msg_id = &request.request_id;

        // Publish with retry and deduplication
        let ack = self
            .publish_with_retry(&subject, Bytes::from(payload), Some(msg_id))
            .await?;

        // Record success
        self.record_success();

        debug!(
            subject = %subject,
            sequence = ack.sequence,
            stream = %ack.stream,
            "Write request published"
        );

        // Create write ticket with configured TTL
        let ticket = WriteTicket::with_ttl(
            &request.store_id,
            ack.sequence,
            &request.request_id,
            self.inner.config.write_ticket_ttl,
        );

        Ok(ticket)
    }

    /// Publish a model write request to RSFGA_WRITES stream.
    ///
    /// Returns a `WriteTicket` that can be used for read-your-own-writes.
    #[instrument(skip(self, request), fields(store_id = %request.store_id, request_id = %request.request_id))]
    pub async fn publish_model_write_request(
        &self,
        request: &ModelWriteRequest,
    ) -> Result<WriteTicket> {
        // Check circuit breaker
        self.check_circuit_breaker()?;

        self.inner
            .metrics
            .publish_attempts
            .fetch_add(1, Ordering::Relaxed);

        let subject = request.subject();
        let payload = request.to_bytes()?;

        // Use request_id as message ID for deduplication during retries
        let msg_id = &request.request_id;

        // Publish with retry and deduplication
        let ack = self
            .publish_with_retry(&subject, Bytes::from(payload), Some(msg_id))
            .await?;

        // Record success
        self.record_success();

        debug!(
            subject = %subject,
            sequence = ack.sequence,
            stream = %ack.stream,
            "Model write request published"
        );

        // Create write ticket with configured TTL
        let ticket = WriteTicket::with_ttl(
            &request.store_id,
            ack.sequence,
            &request.request_id,
            self.inner.config.write_ticket_ttl,
        );

        Ok(ticket)
    }

    /// Publish a committed event to RSFGA_EVENTS stream.
    #[instrument(skip(self, event), fields(store_id = %event.store_id, sequence = event.sequence))]
    pub async fn publish_committed_event(&self, event: &CommittedEvent) -> Result<PublishAck> {
        // Check circuit breaker
        self.check_circuit_breaker()?;

        self.inner
            .metrics
            .publish_attempts
            .fetch_add(1, Ordering::Relaxed);

        let subject = event.subject();
        let payload = event.to_bytes()?;

        // Use store_id + sequence as message ID for deduplication during retries
        let msg_id = format!("{}-{}", event.store_id, event.sequence);

        // Publish with retry and deduplication
        let ack = self
            .publish_with_retry(&subject, Bytes::from(payload), Some(&msg_id))
            .await?;

        // Record success
        self.record_success();

        debug!(
            subject = %subject,
            sequence = ack.sequence,
            stream = %ack.stream,
            "Committed event published"
        );

        Ok(ack)
    }

    /// Publish with retry, timeout, and deduplication.
    ///
    /// If `msg_id` is provided, it's used as the Nats-Msg-Id header for
    /// JetStream deduplication, preventing duplicate messages during retries.
    async fn publish_with_retry(
        &self,
        subject: &str,
        payload: Bytes,
        msg_id: Option<&str>,
    ) -> Result<PublishAck> {
        let mut last_error = None;
        let config = &self.inner.config;

        for attempt in 0..=config.max_retries {
            if attempt > 0 {
                self.inner.metrics.retries.fetch_add(1, Ordering::Relaxed);
                let delay = config.retry_delay * 2u32.pow(attempt - 1);
                debug!(attempt, delay_ms = delay.as_millis(), "Retrying publish");
                tokio::time::sleep(delay).await;
            }

            // Build publish request with message ID for deduplication
            // Only set message_id when provided and non-empty for proper NATS deduplication
            let mut publish = Publish::build().payload(payload.clone());
            if let Some(id) = msg_id {
                if !id.is_empty() {
                    publish = publish.message_id(id);
                }
            }
            let publish = publish;

            match timeout(
                config.publish_timeout,
                self.client
                    .jetstream()
                    .send_publish(subject.to_string(), publish),
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
        self.record_failure();
        Err(last_error.unwrap_or_else(|| NatsError::Internal("Unknown publish error".to_string())))
    }

    /// Check if circuit breaker allows the request.
    fn check_circuit_breaker(&self) -> Result<()> {
        let result = self
            .inner
            .circuit_breaker
            .check(self.inner.config.circuit_breaker_reset);

        if result.is_err() {
            self.inner
                .metrics
                .circuit_breaker_rejections
                .fetch_add(1, Ordering::Relaxed);
        }

        result
    }

    /// Record a successful publish.
    fn record_success(&self) {
        self.inner
            .metrics
            .publish_success
            .fetch_add(1, Ordering::Relaxed);

        self.inner.circuit_breaker.record_success();
    }

    /// Record a failed publish.
    fn record_failure(&self) {
        self.inner
            .metrics
            .publish_failures
            .fetch_add(1, Ordering::Relaxed);

        self.inner
            .circuit_breaker
            .record_failure(self.inner.config.circuit_breaker_threshold);
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
    pub fn is_circuit_open(&self) -> bool {
        self.inner.circuit_breaker.is_open()
    }

    /// Manually reset the circuit breaker.
    pub fn reset_circuit_breaker(&self) {
        self.inner.circuit_breaker.reset();
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
        let cb = AtomicCircuitBreaker::default();
        assert!(!cb.is_open());
        assert_eq!(cb.failure_count.load(Ordering::Relaxed), 0);
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

    #[test]
    fn test_circuit_breaker_opens_after_threshold() {
        let cb = AtomicCircuitBreaker::default();
        let threshold = 3;

        // Record failures up to threshold
        for _ in 0..threshold {
            cb.record_failure(threshold);
        }

        // Circuit should now be open
        assert!(cb.is_open());
    }

    #[test]
    fn test_circuit_breaker_success_resets_failure_count() {
        let cb = AtomicCircuitBreaker::default();
        let threshold = 5;

        // Record some failures (below threshold)
        cb.record_failure(threshold);
        cb.record_failure(threshold);
        assert_eq!(cb.failure_count.load(Ordering::Relaxed), 2);

        // Record success should reset count
        cb.record_success();
        assert_eq!(cb.failure_count.load(Ordering::Relaxed), 0);
        assert!(!cb.is_open());
    }

    #[test]
    fn test_circuit_breaker_manual_reset() {
        let cb = AtomicCircuitBreaker::default();
        let threshold = 2;

        // Open the circuit
        cb.record_failure(threshold);
        cb.record_failure(threshold);
        assert!(cb.is_open());

        // Manual reset
        cb.reset();
        assert!(!cb.is_open());
        assert_eq!(cb.failure_count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_circuit_breaker_check_rejects_when_open() {
        let cb = AtomicCircuitBreaker::default();
        let threshold = 1;
        let reset_duration = Duration::from_secs(3600); // Long reset

        // Small delay to ensure elapsed time is non-zero when circuit opens
        std::thread::sleep(Duration::from_millis(1));

        // Open the circuit
        cb.record_failure(threshold);
        assert!(cb.is_open());

        // Verify opened_at_ms was set (non-zero)
        assert!(cb.opened_at_ms.load(Ordering::Relaxed) > 0);

        // Check should fail
        let result = cb.check(reset_duration);
        assert!(result.is_err());
    }

    #[test]
    fn test_circuit_breaker_check_allows_when_closed() {
        let cb = AtomicCircuitBreaker::default();
        let reset_duration = Duration::from_secs(30);

        // Check should succeed when circuit is closed
        let result = cb.check(reset_duration);
        assert!(result.is_ok());
    }
}
