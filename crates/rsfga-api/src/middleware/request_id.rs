//! Request ID middleware for request tracing and correlation.

use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use axum::http::{HeaderValue, Request, Response};
use tower::{Layer, Service};
use uuid::Uuid;

/// HTTP header name for request ID.
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// Layer that adds request ID to requests and responses.
#[derive(Clone, Default)]
pub struct RequestIdLayer;

impl RequestIdLayer {
    pub fn new() -> Self {
        Self
    }
}

impl<S> Layer<S> for RequestIdLayer {
    type Service = RequestIdService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RequestIdService { inner }
    }
}

/// Service that generates and propagates request IDs.
#[derive(Clone)]
pub struct RequestIdService<S> {
    inner: S,
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for RequestIdService<S>
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

    fn call(&mut self, mut request: Request<ReqBody>) -> Self::Future {
        // Get or generate request ID
        let request_id = request
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        // Add request ID to request headers
        if let Ok(value) = HeaderValue::from_str(&request_id) {
            request.headers_mut().insert(REQUEST_ID_HEADER, value);
        }

        let mut inner = self.inner.clone();
        Box::pin(async move {
            let mut response = inner.call(request).await?;

            // Add request ID to response headers
            if let Ok(value) = HeaderValue::from_str(&request_id) {
                response.headers_mut().insert(REQUEST_ID_HEADER, value);
            }

            Ok(response)
        })
    }
}
