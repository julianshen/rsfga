//! Metrics collection middleware.

use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    task::{Context, Poll},
    time::Instant,
};

use axum::http::{Request, Response};
use tower::{Layer, Service};

/// Collected request metrics.
#[derive(Debug, Default)]
pub struct RequestMetrics {
    /// Total request count.
    pub request_count: AtomicU64,
    /// Total request duration in microseconds.
    pub total_duration_us: AtomicU64,
    /// Count of successful requests (2xx status).
    pub success_count: AtomicU64,
    /// Count of client error requests (4xx status).
    pub client_error_count: AtomicU64,
    /// Count of server error requests (5xx status).
    pub server_error_count: AtomicU64,
}

impl RequestMetrics {
    /// Creates a new metrics collector.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a request with the given status and duration.
    pub fn record(&self, status: u16, duration_us: u64) {
        self.request_count.fetch_add(1, Ordering::Relaxed);
        self.total_duration_us.fetch_add(duration_us, Ordering::Relaxed);

        match status {
            200..=299 => {
                self.success_count.fetch_add(1, Ordering::Relaxed);
            }
            400..=499 => {
                self.client_error_count.fetch_add(1, Ordering::Relaxed);
            }
            500..=599 => {
                self.server_error_count.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
    }

    /// Gets the total request count.
    pub fn get_request_count(&self) -> u64 {
        self.request_count.load(Ordering::Relaxed)
    }

    /// Gets the average request duration in microseconds.
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
    metrics: Arc<RequestMetrics>,
}

impl MetricsLayer {
    /// Creates a new metrics layer with shared metrics.
    pub fn new(metrics: Arc<RequestMetrics>) -> Self {
        Self { metrics }
    }

    /// Gets the metrics collector.
    pub fn metrics(&self) -> Arc<RequestMetrics> {
        Arc::clone(&self.metrics)
    }
}

impl<S> Layer<S> for MetricsLayer {
    type Service = MetricsService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        MetricsService {
            inner,
            metrics: Arc::clone(&self.metrics),
        }
    }
}

/// Service that records metrics for each request.
#[derive(Clone)]
pub struct MetricsService<S> {
    inner: S,
    metrics: Arc<RequestMetrics>,
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
        let metrics = Arc::clone(&self.metrics);
        let mut inner = self.inner.clone();

        Box::pin(async move {
            let response = inner.call(request).await?;
            let duration = start.elapsed();
            let status = response.status().as_u16();

            metrics.record(status, duration.as_micros() as u64);

            Ok(response)
        })
    }
}
