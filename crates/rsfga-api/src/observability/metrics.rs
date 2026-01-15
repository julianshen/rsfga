//! Prometheus metrics infrastructure.
//!
//! This module provides Prometheus-compatible metrics using the `metrics` crate
//! with `metrics-exporter-prometheus` for exposition.
//!
//! # Metrics Exposed
//!
//! - `rsfga_http_requests_total` - Total HTTP requests by method, path, status
//! - `rsfga_http_request_duration_seconds` - Request duration histogram
//! - `rsfga_check_requests_total` - Total authorization check requests
//! - `rsfga_check_duration_seconds` - Check operation duration histogram
//! - `rsfga_cache_hits_total` - Cache hit count
//! - `rsfga_cache_misses_total` - Cache miss count

use std::sync::Arc;

use axum::{extract::State, http::header::CONTENT_TYPE, response::IntoResponse};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

/// Shared state containing the Prometheus handle for metrics rendering.
#[derive(Clone)]
pub struct MetricsState {
    handle: Arc<PrometheusHandle>,
}

impl MetricsState {
    /// Creates a new metrics state with the given Prometheus handle.
    pub fn new(handle: PrometheusHandle) -> Self {
        Self {
            handle: Arc::new(handle),
        }
    }

    /// Renders the current metrics in Prometheus text format.
    pub fn render(&self) -> String {
        self.handle.render()
    }
}

/// Error type for metrics initialization.
#[derive(Debug, thiserror::Error)]
pub enum MetricsError {
    #[error("failed to install Prometheus recorder: recorder already installed")]
    AlreadyInstalled,
}

/// Initializes the Prometheus metrics recorder.
///
/// This must be called once at application startup before any metrics are recorded.
/// Returns a handle that can be used to render metrics.
///
/// # Example
///
/// ```ignore
/// let metrics_state = init_metrics()?;
/// // Metrics are now being collected
/// let prometheus_output = metrics_state.render();
/// ```
///
/// # Errors
///
/// Returns an error if the recorder is already installed.
pub fn init_metrics() -> Result<MetricsState, MetricsError> {
    let builder = PrometheusBuilder::new();
    let handle = builder
        .install_recorder()
        .map_err(|_| MetricsError::AlreadyInstalled)?;

    // Register default metrics
    register_default_metrics();

    Ok(MetricsState::new(handle))
}

/// Registers default application metrics.
///
/// This function describes the metrics that will be collected. The actual
/// recording happens in the middleware and handlers.
fn register_default_metrics() {
    // Describe HTTP metrics
    metrics::describe_counter!("rsfga_http_requests_total", "Total number of HTTP requests");
    metrics::describe_histogram!(
        "rsfga_http_request_duration_seconds",
        "HTTP request duration in seconds"
    );

    // Describe authorization check metrics
    metrics::describe_counter!(
        "rsfga_check_requests_total",
        "Total number of authorization check requests"
    );
    metrics::describe_histogram!(
        "rsfga_check_duration_seconds",
        "Authorization check duration in seconds"
    );

    // Describe cache metrics
    metrics::describe_counter!("rsfga_cache_hits_total", "Total number of cache hits");
    metrics::describe_counter!("rsfga_cache_misses_total", "Total number of cache misses");

    // Describe storage metrics
    metrics::describe_counter!(
        "rsfga_storage_operations_total",
        "Total number of storage operations"
    );
    metrics::describe_histogram!(
        "rsfga_storage_operation_duration_seconds",
        "Storage operation duration in seconds"
    );

    // Describe storage query metrics (from rsfga-storage)
    metrics::describe_histogram!(
        "rsfga_storage_query_duration_seconds",
        "Storage query duration in seconds by operation, backend, and status"
    );
    metrics::describe_counter!(
        "rsfga_storage_query_timeout_total",
        "Total number of storage query timeouts by operation and backend"
    );

    // Describe storage health check metrics
    metrics::describe_histogram!(
        "rsfga_storage_health_check_duration_seconds",
        "Storage health check duration in seconds by backend and status"
    );
    metrics::describe_gauge!(
        "rsfga_storage_pool_connections",
        "Number of database pool connections by backend and state (active, idle, max)"
    );

    // Register CEL cache metrics
    rsfga_domain::cel::register_cel_cache_metrics();
}

/// Prometheus exposition format content type.
const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// Handler for the `/metrics` endpoint.
///
/// Returns Prometheus metrics in text format with proper content-type header.
pub async fn metrics_handler(State(state): State<MetricsState>) -> impl IntoResponse {
    ([(CONTENT_TYPE, PROMETHEUS_CONTENT_TYPE)], state.render())
}

/// Records an HTTP request metric.
///
/// # Arguments
///
/// * `method` - HTTP method (GET, POST, etc.)
/// * `path` - Request path
/// * `status` - HTTP status code
/// * `duration_seconds` - Request duration in seconds
#[allow(dead_code)] // Will be used when Prometheus middleware is integrated
pub fn record_http_request(method: &str, path: &str, status: u16, duration_seconds: f64) {
    let labels = [
        ("method", method.to_string()),
        ("path", path.to_string()),
        ("status", status.to_string()),
    ];

    metrics::counter!("rsfga_http_requests_total", &labels).increment(1);
    metrics::histogram!("rsfga_http_request_duration_seconds", &labels).record(duration_seconds);
}

/// Records an authorization check metric.
///
/// # Arguments
///
/// * `allowed` - Whether the check was allowed
/// * `cached` - Whether the result was from cache
/// * `duration_seconds` - Check duration in seconds
#[allow(dead_code)] // Will be used when check handler records metrics
pub fn record_check_request(allowed: bool, cached: bool, duration_seconds: f64) {
    let labels = [
        ("allowed", allowed.to_string()),
        ("cached", cached.to_string()),
    ];

    metrics::counter!("rsfga_check_requests_total", &labels).increment(1);
    metrics::histogram!("rsfga_check_duration_seconds", &labels).record(duration_seconds);
}

/// Records a cache hit.
#[allow(dead_code)] // Will be used when cache layer records metrics
pub fn record_cache_hit() {
    metrics::counter!("rsfga_cache_hits_total").increment(1);
}

/// Records a cache miss.
#[allow(dead_code)] // Will be used when cache layer records metrics
pub fn record_cache_miss() {
    metrics::counter!("rsfga_cache_misses_total").increment(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: These tests cannot run in parallel with other tests that install
    // a metrics recorder, as only one recorder can be installed per process.
    // The actual integration test for the /metrics endpoint is in routes tests.

    #[test]
    fn test_metrics_state_can_be_cloned() {
        // This test verifies the Clone derive works
        let builder = PrometheusBuilder::new();
        let handle = builder.build_recorder().handle();
        let state = MetricsState::new(handle);
        let _cloned = state.clone();
    }

    #[test]
    fn test_metrics_state_render_returns_string() {
        let builder = PrometheusBuilder::new();
        let handle = builder.build_recorder().handle();
        let state = MetricsState::new(handle);
        // render() should return a valid String without panicking
        // Empty metrics return an empty string, metrics with data contain '#' comments
        let _output = state.render();
        // If we get here without panic, the test passes
    }
}
