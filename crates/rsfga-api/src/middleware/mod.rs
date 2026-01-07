//! API middleware.
//!
//! Includes:
//! - Request logging
//! - Metrics collection
//! - Request tracing
//! - CORS configuration
//! - Request ID generation

mod logging;
mod metrics;
mod request_id;
mod tracing_layer;

pub use logging::RequestLoggingLayer;
pub use metrics::{MetricsLayer, RequestMetrics};
pub use request_id::{RequestIdLayer, REQUEST_ID_HEADER};
pub use tracing_layer::TracingLayer;

use tower_http::cors::{Any, CorsLayer};

/// Creates a CORS layer with permissive settings for development.
///
/// In production, you should restrict origins, methods, and headers.
pub fn cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any)
        .expose_headers(Any)
}

#[cfg(test)]
mod tests;
