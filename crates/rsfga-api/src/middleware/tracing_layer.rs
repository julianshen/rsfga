//! Tracing middleware for distributed tracing support.

use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use axum::http::{Request, Response};
use tower::{Layer, Service};
use tracing::{field::Empty, info_span, Instrument, Span};

/// Layer that creates tracing spans for requests.
#[derive(Clone, Default)]
pub struct TracingLayer;

impl TracingLayer {
    pub fn new() -> Self {
        Self
    }
}

impl<S> Layer<S> for TracingLayer {
    type Service = TracingService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        TracingService { inner }
    }
}

/// Service that wraps requests in tracing spans.
#[derive(Clone)]
pub struct TracingService<S> {
    inner: S,
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for TracingService<S>
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

        // Get request ID if present
        let request_id = request
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .unwrap_or_default();

        // Create span for this request
        // Note: http.status_code is declared with Empty and recorded later
        let span = info_span!(
            "http_request",
            method = %method,
            uri = %uri,
            request_id = %request_id,
            http.status_code = Empty,
            otel.kind = "server"
        );

        let mut inner = self.inner.clone();

        Box::pin(
            async move {
                let response = inner.call(request).await?;
                let status = response.status();

                // Record status in span
                Span::current().record("http.status_code", status.as_u16());

                Ok(response)
            }
            .instrument(span),
        )
    }
}
