//! Metrics for the RSFGA Writer daemon.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use metrics::{counter, gauge, histogram};
use tracing::info;

use crate::error::Result;

/// Writer daemon metrics.
#[derive(Debug, Default)]
pub struct WriterMetrics {
    /// Total messages consumed from NATS.
    pub messages_consumed: AtomicU64,

    /// Total messages successfully processed.
    pub messages_processed: AtomicU64,

    /// Total messages failed.
    pub messages_failed: AtomicU64,

    /// Total batches processed.
    pub batches_processed: AtomicU64,

    /// Total tuples written to storage.
    pub tuples_written: AtomicU64,

    /// Total tuples deleted from storage.
    pub tuples_deleted: AtomicU64,

    /// Total committed events published.
    pub events_published: AtomicU64,

    /// Total event publish failures (after retries).
    pub event_publish_failures: AtomicU64,
}

impl WriterMetrics {
    /// Create a new metrics instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a message consumed from NATS.
    pub fn record_message_consumed(&self) {
        self.messages_consumed.fetch_add(1, Ordering::Relaxed);
        counter!("rsfga_writer_messages_consumed_total").increment(1);
    }

    /// Record a message successfully processed.
    pub fn record_message_processed(&self) {
        self.messages_processed.fetch_add(1, Ordering::Relaxed);
        counter!("rsfga_writer_messages_processed_total").increment(1);
    }

    /// Record a message processing failure.
    pub fn record_message_failed(&self) {
        self.messages_failed.fetch_add(1, Ordering::Relaxed);
        counter!("rsfga_writer_messages_failed_total").increment(1);
    }

    /// Record a batch processed.
    pub fn record_batch_processed(&self, batch_size: usize) {
        self.batches_processed.fetch_add(1, Ordering::Relaxed);
        counter!("rsfga_writer_batches_processed_total").increment(1);
        histogram!("rsfga_writer_batch_size").record(batch_size as f64);
    }

    /// Record tuples written.
    pub fn record_tuples_written(&self, count: usize) {
        self.tuples_written
            .fetch_add(count as u64, Ordering::Relaxed);
        counter!("rsfga_writer_tuples_written_total").increment(count as u64);
    }

    /// Record tuples deleted.
    pub fn record_tuples_deleted(&self, count: usize) {
        self.tuples_deleted
            .fetch_add(count as u64, Ordering::Relaxed);
        counter!("rsfga_writer_tuples_deleted_total").increment(count as u64);
    }

    /// Record a committed event published.
    pub fn record_event_published(&self) {
        self.events_published.fetch_add(1, Ordering::Relaxed);
        counter!("rsfga_writer_events_published_total").increment(1);
    }

    /// Record an event publish failure (after retries exhausted).
    pub fn record_event_publish_failure(&self) {
        self.event_publish_failures.fetch_add(1, Ordering::Relaxed);
        counter!("rsfga_writer_event_publish_failures_total").increment(1);
    }

    /// Record storage write latency.
    pub fn record_storage_latency(&self, start: Instant) {
        let duration = start.elapsed();
        histogram!("rsfga_writer_storage_write_duration_seconds").record(duration.as_secs_f64());
    }

    /// Update consumer lag gauge (reserved for future use).
    #[allow(dead_code)]
    pub fn update_consumer_lag(&self, lag: u64) {
        gauge!("rsfga_writer_consumer_lag_messages").set(lag as f64);
    }
}

/// Setup the Prometheus metrics server.
pub async fn setup_metrics_server(port: u16) -> Result<()> {
    use metrics_exporter_prometheus::PrometheusBuilder;

    PrometheusBuilder::new()
        .with_http_listener(([0, 0, 0, 0], port))
        .install()
        .map_err(|e| {
            crate::error::WriterError::Config(format!("Failed to setup metrics: {}", e))
        })?;

    info!(port = port, "Prometheus metrics server started");
    Ok(())
}
