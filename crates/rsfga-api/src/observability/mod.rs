//! Observability infrastructure for RSFGA.
//!
//! This module provides:
//! - Prometheus metrics endpoint
//! - Health and readiness checks
//! - Structured logging configuration
//! - OpenTelemetry tracing setup with Jaeger export
//!
//! # Unified Initialization
//!
//! The `tracing` ecosystem only allows one global subscriber. Use `init_observability`
//! to initialize both logging and distributed tracing together:
//!
//! ```ignore
//! use rsfga_api::observability::{init_observability, LoggingConfig, TracingConfig};
//!
//! // Initialize with both logging and tracing
//! init_observability(
//!     LoggingConfig::json(),
//!     Some(TracingConfig::default()),
//! )?;
//! ```

mod logging;
mod metrics;
mod tracing;

pub use logging::{create_json_layer, LoggingConfig};
pub use metrics::{init_metrics, metrics_handler, MetricsError, MetricsState};
pub use tracing::{shutdown_tracing, TracingConfig, TracingError};

use tracing_subscriber::{
    fmt::{self, format::FmtSpan},
    layer::SubscriberExt,
    util::SubscriberInitExt,
    EnvFilter, Layer, Registry,
};

/// Unified observability initialization error.
#[derive(Debug, thiserror::Error)]
pub enum ObservabilityError {
    #[error("failed to initialize tracing: {0}")]
    Tracing(#[from] TracingError),

    #[error("failed to install subscriber: {0}")]
    SubscriberInit(String),
}

/// Initialize the complete observability stack.
///
/// This function sets up both structured logging and distributed tracing in a single
/// subscriber stack. The `tracing` ecosystem only allows one global subscriber, so
/// this unified function ensures both features work together correctly.
///
/// # Arguments
///
/// * `log_config` - Configuration for structured logging (format, level, spans)
/// * `trace_config` - Optional configuration for OpenTelemetry/Jaeger tracing.
///   Pass `None` to disable distributed tracing.
///
/// # Returns
///
/// Returns `Ok(())` on success, or an error if initialization fails.
///
/// # Example
///
/// ```ignore
/// use rsfga_api::observability::{init_observability, LoggingConfig, TracingConfig};
///
/// // Production: JSON logs + Jaeger tracing
/// init_observability(
///     LoggingConfig::json(),
///     Some(TracingConfig::default()),
/// )?;
///
/// // Development: Pretty text logs, no tracing
/// init_observability(
///     LoggingConfig::text().with_level(tracing::Level::DEBUG),
///     None,
/// )?;
/// ```
pub fn init_observability(
    log_config: LoggingConfig,
    trace_config: Option<TracingConfig>,
) -> Result<(), ObservabilityError> {
    // Build the filter from RUST_LOG env var or use default level
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(log_config.default_level.to_string()));

    let span_events = if log_config.include_spans {
        FmtSpan::ENTER | FmtSpan::EXIT
    } else {
        FmtSpan::NONE
    };

    // Create the logging layer based on format
    let fmt_layer = if log_config.json_format {
        fmt::layer()
            .json()
            .with_span_events(span_events)
            .with_current_span(true)
            .with_target(true)
            .with_file(false)
            .with_line_number(false)
            .boxed()
    } else {
        fmt::layer()
            .pretty()
            .with_span_events(span_events)
            .with_target(true)
            .boxed()
    };

    // Optionally create the OpenTelemetry layer
    if let Some(trace_cfg) = trace_config {
        if trace_cfg.enabled {
            let tracer = tracing::create_jaeger_tracer(&trace_cfg)?;
            let otel_layer = tracing_opentelemetry::OpenTelemetryLayer::new(tracer);

            Registry::default()
                .with(filter)
                .with(fmt_layer)
                .with(otel_layer)
                .try_init()
                .map_err(|e| ObservabilityError::SubscriberInit(e.to_string()))?;
        } else {
            // Tracing disabled, just use logging
            Registry::default()
                .with(filter)
                .with(fmt_layer)
                .try_init()
                .map_err(|e| ObservabilityError::SubscriberInit(e.to_string()))?;
        }
    } else {
        // No tracing config, just use logging
        Registry::default()
            .with(filter)
            .with(fmt_layer)
            .try_init()
            .map_err(|e| ObservabilityError::SubscriberInit(e.to_string()))?;
    }

    Ok(())
}

/// Initialize logging only (for backward compatibility and testing).
///
/// **Note**: Prefer `init_observability` for production use to ensure
/// logging and tracing are properly composed.
///
/// This function sets up the logging subscriber without distributed tracing.
/// If called after `init_observability`, subsequent calls will have no effect.
pub fn init_logging(config: LoggingConfig) {
    // Delegate to init_observability with no tracing
    let _ = init_observability(config, None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::tracing::Level;

    #[test]
    fn test_logging_config_default() {
        let config = LoggingConfig::default();
        assert!(!config.json_format);
        assert_eq!(config.default_level, Level::INFO);
    }

    #[test]
    fn test_tracing_config_default() {
        let config = TracingConfig::default();
        assert_eq!(config.service_name, "rsfga");
        assert!(config.enabled);
    }

    #[test]
    fn test_tracing_config_disabled() {
        let config = TracingConfig::disabled();
        assert!(!config.enabled);
    }
}
