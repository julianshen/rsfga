//! Request logging middleware.

use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Instant,
};

use axum::http::{Request, Response};
use tower::{Layer, Service};
use tracing::info;

/// Layer that logs HTTP requests and responses.
#[derive(Clone, Default)]
pub struct RequestLoggingLayer;

impl RequestLoggingLayer {
    pub fn new() -> Self {
        Self
    }
}

impl<S> Layer<S> for RequestLoggingLayer {
    type Service = RequestLoggingService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RequestLoggingService { inner }
    }
}

/// Service that logs request/response details.
#[derive(Clone)]
pub struct RequestLoggingService<S> {
    inner: S,
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for RequestLoggingService<S>
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
        let method = request.method().clone();
        let uri = request.uri().clone();
        let version = request.version();

        // Get request ID if present
        let request_id = request
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let start = Instant::now();
        let mut inner = self.inner.clone();

        Box::pin(async move {
            // Log request
            if let Some(ref id) = request_id {
                info!(
                    target: "rsfga::http",
                    request_id = %id,
                    method = %method,
                    uri = %uri,
                    version = ?version,
                    "request started"
                );
            } else {
                info!(
                    target: "rsfga::http",
                    method = %method,
                    uri = %uri,
                    version = ?version,
                    "request started"
                );
            }

            let response = inner.call(request).await?;
            let duration = start.elapsed();
            let status = response.status();

            // Log response
            if let Some(id) = request_id {
                info!(
                    target: "rsfga::http",
                    request_id = %id,
                    method = %method,
                    uri = %uri,
                    status = %status.as_u16(),
                    duration_ms = duration.as_millis() as u64,
                    "request completed"
                );
            } else {
                info!(
                    target: "rsfga::http",
                    method = %method,
                    uri = %uri,
                    status = %status.as_u16(),
                    duration_ms = duration.as_millis() as u64,
                    "request completed"
                );
            }

            Ok(response)
        })
    }
}
