//! Observability infrastructure for RSFGA.
//!
//! This module provides:
//! - Prometheus metrics endpoint
//! - Health and readiness checks
//! - Structured logging configuration
//! - OpenTelemetry tracing setup with Jaeger export

mod logging;
mod metrics;
mod tracing;

pub use logging::{create_json_layer, init_logging, LoggingConfig};
pub use metrics::{init_metrics, metrics_handler, MetricsState};
pub use tracing::{
    create_test_tracing_layer, init_tracing, shutdown_tracing, TracingConfig, TracingError,
};
