//! OpenTelemetry tracing configuration for Jaeger export.
//!
//! This module provides functions for configuring distributed tracing
//! using OpenTelemetry with Jaeger as the backend.
//!
//! # Architecture
//!
//! The tracing setup bridges the `tracing` crate (used throughout the codebase)
//! with OpenTelemetry, which exports spans to Jaeger:
//!
//! ```text
//! tracing::span!()  -->  tracing-opentelemetry  -->  OpenTelemetry SDK  -->  Jaeger
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use rsfga_api::observability::tracing::{init_tracing, TracingConfig};
//!
//! // Initialize with default Jaeger endpoint (localhost:6831)
//! init_tracing(TracingConfig::default())?;
//!
//! // Or specify a custom endpoint
//! init_tracing(TracingConfig::new("jaeger.example.com:6831"))?;
//! ```

use opentelemetry::trace::TracerProvider;
use opentelemetry_jaeger::config::agent::AgentPipeline;
use opentelemetry_sdk::trace::Tracer;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Registry};

/// Configuration for OpenTelemetry tracing with Jaeger export.
#[derive(Debug, Clone)]
pub struct TracingConfig {
    /// Service name for the traces
    pub service_name: String,
    /// Jaeger agent endpoint (host:port for UDP agent)
    pub jaeger_endpoint: String,
    /// Whether tracing is enabled
    pub enabled: bool,
}

impl Default for TracingConfig {
    fn default() -> Self {
        Self {
            service_name: "rsfga".to_string(),
            jaeger_endpoint: "localhost:6831".to_string(),
            enabled: true,
        }
    }
}

impl TracingConfig {
    /// Create a new tracing configuration with a custom Jaeger endpoint.
    pub fn new(jaeger_endpoint: impl Into<String>) -> Self {
        Self {
            jaeger_endpoint: jaeger_endpoint.into(),
            ..Default::default()
        }
    }

    /// Set the service name for traces.
    pub fn with_service_name(mut self, name: impl Into<String>) -> Self {
        self.service_name = name.into();
        self
    }

    /// Disable tracing (useful for testing).
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Default::default()
        }
    }
}

/// Error type for tracing initialization failures.
#[derive(Debug, thiserror::Error)]
pub enum TracingError {
    #[error("failed to initialize Jaeger exporter: {0}")]
    JaegerInit(String),

    #[error("failed to install tracing subscriber: {0}")]
    SubscriberInit(String),
}

/// Initialize OpenTelemetry tracing with Jaeger export.
///
/// This function sets up the tracing subscriber to export spans to Jaeger.
/// It should be called once at application startup.
///
/// # Arguments
///
/// * `config` - Tracing configuration specifying the Jaeger endpoint
///
/// # Returns
///
/// Returns `Ok(())` on success, or an error if initialization fails.
///
/// # Example
///
/// ```ignore
/// use rsfga_api::observability::tracing::{init_tracing, TracingConfig};
///
/// init_tracing(TracingConfig::default())?;
/// ```
pub fn init_tracing(config: TracingConfig) -> Result<(), TracingError> {
    if !config.enabled {
        return Ok(());
    }

    // Create the Jaeger exporter pipeline
    let tracer = create_jaeger_tracer(&config)?;

    // Create the OpenTelemetry layer for tracing
    let telemetry_layer = OpenTelemetryLayer::new(tracer);

    // Build the subscriber with the OpenTelemetry layer
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    Registry::default()
        .with(filter)
        .with(telemetry_layer)
        .try_init()
        .map_err(|e| TracingError::SubscriberInit(e.to_string()))?;

    Ok(())
}

/// Create a Jaeger tracer with the given configuration.
fn create_jaeger_tracer(config: &TracingConfig) -> Result<Tracer, TracingError> {
    let pipeline = opentelemetry_jaeger::new_agent_pipeline()
        .with_service_name(&config.service_name)
        .with_endpoint(&config.jaeger_endpoint);

    build_tracer(pipeline)
}

/// Build a tracer from an agent pipeline.
fn build_tracer(pipeline: AgentPipeline) -> Result<Tracer, TracingError> {
    let provider = pipeline
        .build_batch(opentelemetry_sdk::runtime::Tokio)
        .map_err(|e| TracingError::JaegerInit(e.to_string()))?;

    let tracer = provider.tracer("rsfga");
    Ok(tracer)
}

/// Create an OpenTelemetry layer for use in test scenarios.
///
/// This creates a layer that can be composed with other tracing layers
/// for testing purposes, without affecting the global subscriber.
///
/// # Arguments
///
/// * `config` - Tracing configuration
///
/// # Returns
///
/// Returns the OpenTelemetry layer wrapped in an Option (None if disabled).
pub fn create_test_tracing_layer(
    config: &TracingConfig,
) -> Result<Option<OpenTelemetryLayer<Registry, Tracer>>, TracingError> {
    if !config.enabled {
        return Ok(None);
    }

    let tracer = create_jaeger_tracer(config)?;
    Ok(Some(OpenTelemetryLayer::new(tracer)))
}

/// Shutdown OpenTelemetry and flush any pending spans.
///
/// This should be called before application shutdown to ensure
/// all spans are exported to Jaeger.
pub fn shutdown_tracing() {
    opentelemetry::global::shutdown_tracer_provider();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tracing_config_default() {
        let config = TracingConfig::default();
        assert_eq!(config.service_name, "rsfga");
        assert_eq!(config.jaeger_endpoint, "localhost:6831");
        assert!(config.enabled);
    }

    #[test]
    fn test_tracing_config_custom_endpoint() {
        let config = TracingConfig::new("jaeger.example.com:6831");
        assert_eq!(config.jaeger_endpoint, "jaeger.example.com:6831");
    }

    #[test]
    fn test_tracing_config_with_service_name() {
        let config = TracingConfig::default().with_service_name("my-service");
        assert_eq!(config.service_name, "my-service");
    }

    #[test]
    fn test_tracing_config_disabled() {
        let config = TracingConfig::disabled();
        assert!(!config.enabled);
    }

    #[test]
    fn test_init_tracing_when_disabled() {
        // When tracing is disabled, init should succeed without doing anything
        let config = TracingConfig::disabled();
        let result = init_tracing(config);
        assert!(result.is_ok());
    }

    // Note: Testing actual Jaeger export requires a running Jaeger instance.
    // The integration test below is marked as ignored and should be run
    // manually with a Jaeger instance available.

    /// Test: Tracing spans are exported to Jaeger
    ///
    /// This test verifies that spans created with the tracing crate
    /// are properly exported to a Jaeger backend.
    ///
    /// To run this test:
    /// 1. Start Jaeger: docker run -d -p 6831:6831/udp -p 16686:16686 jaegertracing/all-in-one
    /// 2. Run: cargo test test_tracing_spans_exported_to_jaeger -- --ignored
    /// 3. Check Jaeger UI at http://localhost:16686
    #[tokio::test]
    #[ignore = "requires running Jaeger instance"]
    async fn test_tracing_spans_exported_to_jaeger() {
        use tracing::{info, info_span};

        // Note: This test uses a fresh config each time, but since the global
        // subscriber can only be set once, this test must run in isolation.
        let config = TracingConfig::default();

        // Create the tracer
        let tracer = create_jaeger_tracer(&config).expect("Failed to create tracer");
        let telemetry_layer = OpenTelemetryLayer::new(tracer);

        // Create a subscriber for this test
        let subscriber = Registry::default().with(telemetry_layer);

        // Use the subscriber for this scope
        tracing::subscriber::with_default(subscriber, || {
            // Create a span and log within it
            let span = info_span!("test_operation", user = "alice", action = "check");
            let _guard = span.enter();

            info!("Performing test operation");

            // Simulate some work
            std::thread::sleep(std::time::Duration::from_millis(10));
        });

        // Give time for the span to be exported
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Shutdown to flush spans
        shutdown_tracing();

        // At this point, the span should be visible in Jaeger UI
        // Manual verification is required
    }
}
