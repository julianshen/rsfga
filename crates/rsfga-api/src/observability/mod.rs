//! Observability infrastructure for RSFGA.
//!
//! This module provides:
//! - Prometheus metrics endpoint
//! - Health and readiness checks
//! - Structured logging configuration
//! - OpenTelemetry tracing setup

mod metrics;

pub use metrics::{init_metrics, metrics_handler, MetricsState};
