//! Metrics collection middleware.
//!
//! This middleware collects HTTP request metrics using the `metrics` facade,
//! which integrates with Prometheus via the `metrics-exporter-prometheus` backend.
//!
//! # Metrics Emitted
//!
//! - `rsfga_http_requests_total` - Counter with labels: method, path, status_class
//! - `rsfga_http_request_duration_seconds` - Histogram with labels: method, path, status_class
//!
//! # Example
//!
//! ```ignore
//! use rsfga_api::middleware::{MetricsLayer, RequestMetrics};
//! use std::sync::Arc;
//!
//! let metrics = Arc::new(RequestMetrics::new());
//! let app = Router::new()
//!     .route("/", get(handler))
//!     .layer(MetricsLayer::new(metrics));
//! ```

use std::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicU64, Ordering},
    task::{Context, Poll},
    time::Instant,
};

use axum::{
    extract::MatchedPath,
    http::{Request, Response},
};
use tower::{Layer, Service};

/// Collected request metrics.
///
/// This struct tracks request counts and durations using both:
/// 1. Atomic counters for test introspection (reading counts back)
/// 2. The `metrics` facade for Prometheus export
///
/// The atomic counters solve the problem of testing middleware behavior,
/// while the metrics facade provides production observability.
#[derive(Debug, Default)]
pub struct RequestMetrics {
    /// Total request count (for test introspection).
    request_count: AtomicU64,
    /// Total request duration in microseconds (for test introspection).
    total_duration_us: AtomicU64,
    /// Count of successful requests (2xx status).
    success_count: AtomicU64,
    /// Count of client error requests (4xx status).
    client_error_count: AtomicU64,
    /// Count of server error requests (5xx status).
    server_error_count: AtomicU64,
}

impl RequestMetrics {
    /// Creates a new metrics collector.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a request with the given method, path, status, and duration.
    ///
    /// This method:
    /// 1. Updates internal atomic counters (for test introspection)
    /// 2. Emits metrics via the `metrics` facade (for Prometheus export)
    ///
    /// # Arguments
    ///
    /// * `method` - HTTP method (GET, POST, etc.)
    /// * `path` - Request path
    /// * `status` - HTTP status code
    /// * `duration_us` - Request duration in microseconds
    pub fn record(&self, method: &str, path: &str, status: u16, duration_us: u64) {
        // Update atomic counters for test introspection
        self.request_count.fetch_add(1, Ordering::Relaxed);
        self.total_duration_us
            .fetch_add(duration_us, Ordering::Relaxed);

        let status_class = match status {
            200..=299 => {
                self.success_count.fetch_add(1, Ordering::Relaxed);
                "2xx"
            }
            400..=499 => {
                self.client_error_count.fetch_add(1, Ordering::Relaxed);
                "4xx"
            }
            500..=599 => {
                self.server_error_count.fetch_add(1, Ordering::Relaxed);
                "5xx"
            }
            _ => "other",
        };

        // Emit to metrics facade for Prometheus export
        let labels = [
            ("method", method.to_string()),
            ("path", path.to_string()),
            ("status_class", status_class.to_string()),
        ];

        metrics::counter!("rsfga_http_requests_total", &labels).increment(1);
        metrics::histogram!("rsfga_http_request_duration_seconds", &labels)
            .record(duration_us as f64 / 1_000_000.0); // Convert to seconds
    }

    /// Gets the total request count.
    ///
    /// This is primarily used for testing to verify middleware behavior.
    pub fn get_request_count(&self) -> u64 {
        self.request_count.load(Ordering::Relaxed)
    }

    /// Gets the success request count (2xx status).
    pub fn get_success_count(&self) -> u64 {
        self.success_count.load(Ordering::Relaxed)
    }

    /// Gets the client error count (4xx status).
    pub fn get_client_error_count(&self) -> u64 {
        self.client_error_count.load(Ordering::Relaxed)
    }

    /// Gets the server error count (5xx status).
    pub fn get_server_error_count(&self) -> u64 {
        self.server_error_count.load(Ordering::Relaxed)
    }

    /// Gets the total duration in microseconds.
    ///
    /// This is primarily used for testing to verify middleware behavior.
    pub fn get_total_duration_us(&self) -> u64 {
        self.total_duration_us.load(Ordering::Relaxed)
    }

    /// Gets the average request duration in microseconds.
    ///
    /// Note: For production monitoring, use the histogram metrics from
    /// Prometheus which provide accurate percentiles. This method is
    /// primarily for testing purposes.
    pub fn get_avg_duration_us(&self) -> u64 {
        let count = self.request_count.load(Ordering::Relaxed);
        if count == 0 {
            0
        } else {
            self.total_duration_us.load(Ordering::Relaxed) / count
        }
    }
}

/// Layer that collects request metrics.
#[derive(Clone)]
pub struct MetricsLayer {
    metrics: std::sync::Arc<RequestMetrics>,
}

impl MetricsLayer {
    /// Creates a new metrics layer with shared metrics.
    pub fn new(metrics: std::sync::Arc<RequestMetrics>) -> Self {
        Self { metrics }
    }

    /// Gets the metrics collector.
    pub fn metrics(&self) -> std::sync::Arc<RequestMetrics> {
        std::sync::Arc::clone(&self.metrics)
    }
}

impl<S> Layer<S> for MetricsLayer {
    type Service = MetricsService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        MetricsService {
            inner,
            metrics: std::sync::Arc::clone(&self.metrics),
        }
    }
}

/// Service that records metrics for each request.
#[derive(Clone)]
pub struct MetricsService<S> {
    inner: S,
    metrics: std::sync::Arc<RequestMetrics>,
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for MetricsService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>> + Clone + Send + 'static,
    S::Future: Send,
    ReqBody: Send + 'static,
    ResBody: Default + Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<ReqBody>) -> Self::Future {
        let start = Instant::now();
        let method = request.method().to_string();
        // Use matched route pattern to avoid cardinality explosion in Prometheus.
        // Falls back to raw path if MatchedPath is not available (e.g., 404 routes).
        let path = request
            .extensions()
            .get::<MatchedPath>()
            .map(|p| p.as_str().to_string())
            .unwrap_or_else(|| request.uri().path().to_string());
        let metrics = std::sync::Arc::clone(&self.metrics);
        let mut inner = self.inner.clone();

        Box::pin(async move {
            let response = inner.call(request).await?;
            let duration = start.elapsed();
            let status = response.status().as_u16();

            metrics.record(&method, &path, status, duration.as_micros() as u64);

            Ok(response)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_metrics_creation() {
        let metrics = RequestMetrics::new();
        assert_eq!(metrics.get_request_count(), 0);
        assert_eq!(metrics.get_success_count(), 0);
        assert_eq!(metrics.get_client_error_count(), 0);
        assert_eq!(metrics.get_server_error_count(), 0);
    }

    #[test]
    fn test_request_metrics_record_success() {
        let metrics = RequestMetrics::new();
        metrics.record("GET", "/test", 200, 1000);

        assert_eq!(metrics.get_request_count(), 1);
        assert_eq!(metrics.get_success_count(), 1);
        assert_eq!(metrics.get_client_error_count(), 0);
        assert_eq!(metrics.get_server_error_count(), 0);
        assert_eq!(metrics.get_total_duration_us(), 1000);
    }

    #[test]
    fn test_request_metrics_record_client_error() {
        let metrics = RequestMetrics::new();
        metrics.record("POST", "/api", 404, 500);

        assert_eq!(metrics.get_request_count(), 1);
        assert_eq!(metrics.get_success_count(), 0);
        assert_eq!(metrics.get_client_error_count(), 1);
        assert_eq!(metrics.get_server_error_count(), 0);
    }

    #[test]
    fn test_request_metrics_record_server_error() {
        let metrics = RequestMetrics::new();
        metrics.record("PUT", "/resource", 500, 2000);

        assert_eq!(metrics.get_request_count(), 1);
        assert_eq!(metrics.get_success_count(), 0);
        assert_eq!(metrics.get_client_error_count(), 0);
        assert_eq!(metrics.get_server_error_count(), 1);
    }

    #[test]
    fn test_request_metrics_multiple_records() {
        let metrics = RequestMetrics::new();
        metrics.record("GET", "/", 200, 100);
        metrics.record("POST", "/api", 201, 200);
        metrics.record("GET", "/notfound", 404, 50);
        metrics.record("GET", "/error", 500, 150);

        assert_eq!(metrics.get_request_count(), 4);
        assert_eq!(metrics.get_success_count(), 2);
        assert_eq!(metrics.get_client_error_count(), 1);
        assert_eq!(metrics.get_server_error_count(), 1);
        assert_eq!(metrics.get_total_duration_us(), 500);
        assert_eq!(metrics.get_avg_duration_us(), 125);
    }

    #[test]
    fn test_request_metrics_avg_duration_zero_requests() {
        let metrics = RequestMetrics::new();
        assert_eq!(metrics.get_avg_duration_us(), 0);
    }

    #[test]
    fn test_metrics_emitted_without_panic() {
        // This test verifies that metrics are emitted without panicking
        // even without a recorder installed (metrics crate uses no-op)
        let metrics = RequestMetrics::new();
        metrics.record("GET", "/test", 200, 1000);
        metrics.record("POST", "/api", 400, 500);
        metrics.record("DELETE", "/resource", 500, 2000);
        // If we get here without panic, metrics emission works
    }
}
