//! Observability infrastructure for RSFGA.
//!
//! This module provides:
//! - Prometheus metrics endpoint
//! - Health and readiness checks
//! - Structured logging configuration
//! - OpenTelemetry tracing setup

mod logging;
mod metrics;

pub use logging::{create_json_layer, init_logging, LoggingConfig};
pub use metrics::{init_metrics, metrics_handler, MetricsState};
