//! Structured logging configuration.
//!
//! This module provides functions for configuring structured JSON logging
//! using `tracing-subscriber`.
//!
//! # Log Format
//!
//! When JSON formatting is enabled, log entries are output as JSON objects:
//!
//! ```json
//! {"timestamp":"2024-01-15T10:30:00.000Z","level":"INFO","target":"rsfga","message":"Server started","fields":{}}
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use rsfga_api::observability::logging::init_logging;
//!
//! // Initialize with JSON logging for production
//! init_logging(true);
//!
//! // Or use text logging for development
//! init_logging(false);
//! ```

use tracing::Level;
use tracing_subscriber::{
    fmt::{self, format::FmtSpan},
    prelude::*,
    EnvFilter,
};

/// Configuration for structured logging.
#[derive(Debug, Clone)]
pub struct LoggingConfig {
    /// Whether to use JSON format (true) or text format (false)
    pub json_format: bool,
    /// The default log level if RUST_LOG is not set
    pub default_level: Level,
    /// Whether to include span events (enter/exit)
    pub include_spans: bool,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            json_format: false,
            default_level: Level::INFO,
            include_spans: false,
        }
    }
}

impl LoggingConfig {
    /// Create a new logging configuration for JSON output.
    pub fn json() -> Self {
        Self {
            json_format: true,
            ..Default::default()
        }
    }

    /// Create a new logging configuration for text output (development).
    pub fn text() -> Self {
        Self {
            json_format: false,
            ..Default::default()
        }
    }

    /// Set the default log level.
    pub fn with_level(mut self, level: Level) -> Self {
        self.default_level = level;
        self
    }

    /// Include span events in the output.
    pub fn with_spans(mut self) -> Self {
        self.include_spans = true;
        self
    }
}

/// Initialize the logging subsystem with the given configuration.
///
/// This should be called once at application startup. If called multiple times,
/// subsequent calls will have no effect (the subscriber is global).
///
/// # Arguments
///
/// * `config` - Logging configuration specifying format and level
///
/// # Example
///
/// ```ignore
/// use rsfga_api::observability::logging::{init_logging, LoggingConfig};
///
/// // Production: JSON format with INFO level
/// init_logging(LoggingConfig::json());
///
/// // Development: Text format with DEBUG level
/// init_logging(LoggingConfig::text().with_level(tracing::Level::DEBUG));
/// ```
pub fn init_logging(config: LoggingConfig) {
    // Build the filter from RUST_LOG env var or use default level
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(config.default_level.to_string()));

    let span_events = if config.include_spans {
        FmtSpan::ENTER | FmtSpan::EXIT
    } else {
        FmtSpan::NONE
    };

    if config.json_format {
        // JSON formatted logging for production
        let subscriber = tracing_subscriber::registry().with(filter).with(
            fmt::layer()
                .json()
                .with_span_events(span_events)
                .with_current_span(true)
                .with_target(true)
                .with_file(false)
                .with_line_number(false),
        );

        // Try to set as global default, ignore if already set
        let _ = tracing::subscriber::set_global_default(subscriber);
    } else {
        // Pretty text logging for development
        let subscriber = tracing_subscriber::registry().with(filter).with(
            fmt::layer()
                .pretty()
                .with_span_events(span_events)
                .with_target(true),
        );

        // Try to set as global default, ignore if already set
        let _ = tracing::subscriber::set_global_default(subscriber);
    }
}

/// Creates a JSON-formatted log output for testing purposes.
///
/// This function creates a logging layer that writes to a provided writer,
/// allowing tests to capture and verify JSON log output.
///
/// # Arguments
///
/// * `writer` - The writer to output logs to
///
/// # Returns
///
/// A configured logging layer
pub fn create_json_layer<W>(writer: W) -> impl tracing::Subscriber + Send + Sync
where
    W: for<'writer> tracing_subscriber::fmt::MakeWriter<'writer> + Send + Sync + 'static,
{
    tracing_subscriber::registry()
        .with(EnvFilter::new("trace"))
        .with(
            fmt::layer()
                .json()
                .with_writer(writer)
                .with_target(true)
                .with_current_span(true),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A writer that captures output to a shared buffer.
    #[derive(Clone)]
    struct CaptureWriter {
        buffer: Arc<Mutex<Vec<u8>>>,
    }

    impl CaptureWriter {
        fn new() -> Self {
            Self {
                buffer: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn get_output(&self) -> String {
            let buffer = self.buffer.lock().unwrap();
            String::from_utf8_lossy(&buffer).to_string()
        }
    }

    impl std::io::Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let mut buffer = self.buffer.lock().unwrap();
            buffer.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureWriter {
        type Writer = CaptureWriter;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[test]
    fn test_logging_config_default() {
        let config = LoggingConfig::default();
        assert!(!config.json_format);
        assert_eq!(config.default_level, Level::INFO);
        assert!(!config.include_spans);
    }

    #[test]
    fn test_logging_config_json() {
        let config = LoggingConfig::json();
        assert!(config.json_format);
    }

    #[test]
    fn test_logging_config_text() {
        let config = LoggingConfig::text();
        assert!(!config.json_format);
    }

    #[test]
    fn test_logging_config_with_level() {
        let config = LoggingConfig::default().with_level(Level::DEBUG);
        assert_eq!(config.default_level, Level::DEBUG);
    }

    #[test]
    fn test_logging_config_with_spans() {
        let config = LoggingConfig::default().with_spans();
        assert!(config.include_spans);
    }

    /// Test: Structured logs are JSON formatted
    ///
    /// Verifies that when JSON logging is configured, log entries are valid JSON.
    #[test]
    fn test_structured_logs_are_json_formatted() {
        use tracing::info;

        // Create a capture writer to collect log output
        let writer = CaptureWriter::new();
        let writer_clone = writer.clone();

        // Create a JSON subscriber with our capture writer
        let subscriber = create_json_layer(writer_clone);

        // Use the subscriber for this test
        tracing::subscriber::with_default(subscriber, || {
            info!(user = "alice", action = "check", "Permission check performed");
        });

        // Get the captured output
        let output = writer.get_output();

        // Verify we got some output
        assert!(!output.is_empty(), "Should have captured log output");

        // Each line should be valid JSON
        for line in output.lines() {
            if line.is_empty() {
                continue;
            }
            let parsed: Result<serde_json::Value, _> = serde_json::from_str(line);
            assert!(
                parsed.is_ok(),
                "Log line should be valid JSON: {} (error: {:?})",
                line,
                parsed.err()
            );

            let json = parsed.unwrap();

            // JSON logs should have standard fields
            assert!(
                json.get("level").is_some(),
                "JSON log should have 'level' field"
            );
            assert!(
                json.get("target").is_some(),
                "JSON log should have 'target' field"
            );
        }
    }
}
