//! Metrics for the RSFGA Edge sync daemon.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use metrics::{counter, gauge, histogram};
use tracing::info;

use crate::error::Result;

/// Edge sync daemon metrics.
#[derive(Debug, Default)]
pub struct EdgeMetrics {
    /// Total events consumed from RSFGA_EVENTS.
    pub events_consumed: AtomicU64,

    /// Total events successfully applied to local storage.
    pub events_applied: AtomicU64,

    /// Total events skipped (already processed / idempotency).
    pub events_skipped: AtomicU64,

    /// Total events that failed to apply.
    pub events_failed: AtomicU64,

    /// Total tuples written to local storage.
    pub tuples_written: AtomicU64,

    /// Total tuples deleted from local storage.
    pub tuples_deleted: AtomicU64,

    /// Total cache invalidations triggered.
    pub cache_invalidations: AtomicU64,

    /// Total storage failures.
    pub storage_failures: AtomicU64,
}

impl EdgeMetrics {
    /// Create a new metrics instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an event consumed from NATS.
    pub fn record_event_consumed(&self) {
        self.events_consumed.fetch_add(1, Ordering::Relaxed);
        counter!("rsfga_edge_events_consumed_total").increment(1);
    }

    /// Record an event successfully applied to local storage.
    pub fn record_event_applied(&self) {
        self.events_applied.fetch_add(1, Ordering::Relaxed);
        counter!("rsfga_edge_events_applied_total").increment(1);
    }

    /// Record an event skipped (already processed).
    pub fn record_event_skipped(&self) {
        self.events_skipped.fetch_add(1, Ordering::Relaxed);
        counter!("rsfga_edge_events_skipped_total").increment(1);
    }

    /// Record an event processing failure.
    pub fn record_event_failed(&self) {
        self.events_failed.fetch_add(1, Ordering::Relaxed);
        counter!("rsfga_edge_events_failed_total").increment(1);
    }

    /// Record tuples written to local storage.
    pub fn record_tuples_written(&self, count: usize) {
        self.tuples_written
            .fetch_add(count as u64, Ordering::Relaxed);
        counter!("rsfga_edge_tuples_written_total").increment(count as u64);
    }

    /// Record tuples deleted from local storage.
    pub fn record_tuples_deleted(&self, count: usize) {
        self.tuples_deleted
            .fetch_add(count as u64, Ordering::Relaxed);
        counter!("rsfga_edge_tuples_deleted_total").increment(count as u64);
    }

    /// Record a cache invalidation.
    pub fn record_cache_invalidation(&self) {
        self.cache_invalidations.fetch_add(1, Ordering::Relaxed);
        counter!("rsfga_edge_cache_invalidations_total").increment(1);
    }

    /// Record storage write latency.
    pub fn record_storage_latency(&self, start: Instant) {
        let duration = start.elapsed();
        histogram!("rsfga_edge_storage_write_duration_seconds").record(duration.as_secs_f64());
    }

    /// Record a storage failure.
    pub fn record_storage_failure(&self) {
        self.storage_failures.fetch_add(1, Ordering::Relaxed);
        counter!("rsfga_edge_storage_failures_total").increment(1);
    }

    /// Update sync lag gauge (events behind the central node).
    #[allow(dead_code)] // Will be wired into health check / monitoring endpoint
    pub fn update_sync_lag(&self, store_id: &str, lag: u64) {
        gauge!("rsfga_edge_sync_lag_events", "store_id" => store_id.to_string()).set(lag as f64);
    }

    /// Update the sync position gauge for a store.
    pub fn update_sync_position(&self, store_id: &str, sequence: u64) {
        gauge!("rsfga_edge_sync_position", "store_id" => store_id.to_string()).set(sequence as f64);
    }
}

/// Setup the Prometheus metrics server.
pub async fn setup_metrics_server(port: u16) -> Result<()> {
    use metrics_exporter_prometheus::PrometheusBuilder;

    PrometheusBuilder::new()
        .with_http_listener(([0, 0, 0, 0], port))
        .install()
        .map_err(|e| crate::error::EdgeError::Config(format!("Failed to setup metrics: {}", e)))?;

    info!(port = port, "Prometheus metrics server started");
    Ok(())
}
