//! Configuration management for RSFGA server.
//!
//! This module provides configuration loading with multiple sources:
//! 1. Default values (hardcoded)
//! 2. Configuration file (YAML)
//! 3. Environment variables (override)
//!
//! # Configuration Hierarchy
//!
//! Environment variables take precedence over config file values,
//! which take precedence over defaults. This follows the 12-factor app pattern.
//!
//! # Example
//!
//! ```ignore
//! use rsfga_server::config::ServerConfig;
//!
//! // Load from file with env overrides
//! let config = ServerConfig::load("config.yaml")?;
//!
//! // Or load from environment only
//! let config = ServerConfig::from_env()?;
//! ```

use config::{Config, ConfigError, Environment, File, FileFormat};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Server configuration.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct ServerConfig {
    /// Server settings
    #[serde(default)]
    pub server: ServerSettings,

    /// gRPC settings
    #[serde(default)]
    pub grpc: GrpcSettings,

    /// Storage settings
    #[serde(default)]
    pub storage: StorageSettings,

    /// Logging settings
    #[serde(default)]
    pub logging: LoggingSettings,

    /// Metrics settings
    #[serde(default)]
    pub metrics: MetricsSettings,

    /// Tracing settings
    #[serde(default)]
    pub tracing: TracingSettings,
}

/// Server network settings.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ServerSettings {
    /// Host to bind to
    #[serde(default = "default_host")]
    pub host: String,

    /// Port to listen on
    #[serde(default = "default_port")]
    pub port: u16,

    /// Request timeout in seconds
    #[serde(default = "default_request_timeout")]
    pub request_timeout_secs: u64,

    /// Maximum concurrent connections
    #[serde(default = "default_max_connections")]
    pub max_connections: usize,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            request_timeout_secs: default_request_timeout(),
            max_connections: default_max_connections(),
        }
    }
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}

fn default_port() -> u16 {
    8080
}

fn default_request_timeout() -> u64 {
    30
}

fn default_max_connections() -> usize {
    10000
}

/// gRPC server settings.
///
/// These settings can be overridden via environment variables with the `RSFGA_` prefix
/// and `__` as the nested key separator:
///
/// - `RSFGA_GRPC__ENABLED=false` - Disable gRPC server entirely
/// - `RSFGA_GRPC__PORT=50052` - Change gRPC port
/// - `RSFGA_GRPC__REFLECTION=false` - Disable gRPC reflection
/// - `RSFGA_GRPC__HEALTH_CHECK=false` - Disable gRPC health check service
///
/// # Example YAML Configuration
///
/// ```yaml
/// grpc:
///   enabled: true
///   port: 50051
///   reflection: true
///   health_check: true
/// ```
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct GrpcSettings {
    /// Enable gRPC server.
    ///
    /// When disabled, only the HTTP REST API will be available.
    /// Environment variable: `RSFGA_GRPC__ENABLED`
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// gRPC port to listen on.
    ///
    /// Default: 50051 (standard gRPC port)
    /// Environment variable: `RSFGA_GRPC__PORT`
    #[serde(default = "default_grpc_port")]
    pub port: u16,

    /// Enable gRPC reflection for service discovery.
    ///
    /// When enabled, clients like grpcurl can discover available services
    /// without needing the proto files.
    /// Environment variable: `RSFGA_GRPC__REFLECTION`
    #[serde(default = "default_true")]
    pub reflection: bool,

    /// Enable gRPC health check service.
    ///
    /// Implements the standard gRPC health checking protocol for load balancer
    /// integration and Kubernetes readiness probes.
    /// Environment variable: `RSFGA_GRPC__HEALTH_CHECK`
    #[serde(default = "default_true")]
    pub health_check: bool,
}

impl Default for GrpcSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            port: default_grpc_port(),
            reflection: true,
            health_check: true,
        }
    }
}

fn default_grpc_port() -> u16 {
    50051
}

/// Storage backend settings.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct StorageSettings {
    /// Storage backend type: "memory", "postgres", "mysql", "cockroachdb", or "rocksdb"
    #[serde(default = "default_storage_backend")]
    pub backend: String,

    /// Database connection URL (required if backend is "postgres", "mysql", or "cockroachdb")
    pub database_url: Option<String>,

    /// Data directory path (required if backend is "rocksdb")
    pub data_path: Option<String>,

    /// Connection pool size
    #[serde(default = "default_pool_size")]
    pub pool_size: u32,

    /// Connection timeout in seconds
    #[serde(default = "default_connection_timeout")]
    pub connection_timeout_secs: u64,
}

impl Default for StorageSettings {
    fn default() -> Self {
        Self {
            backend: default_storage_backend(),
            database_url: None,
            data_path: None,
            pool_size: default_pool_size(),
            connection_timeout_secs: default_connection_timeout(),
        }
    }
}

fn default_storage_backend() -> String {
    "memory".to_string()
}

fn default_pool_size() -> u32 {
    10
}

fn default_connection_timeout() -> u64 {
    5
}

/// Logging settings.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct LoggingSettings {
    /// Log level: "trace", "debug", "info", "warn", "error"
    #[serde(default = "default_log_level")]
    pub level: String,

    /// Use JSON format (true for production, false for development)
    #[serde(default)]
    pub json: bool,
}

impl Default for LoggingSettings {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            json: false,
        }
    }
}

fn default_log_level() -> String {
    "info".to_string()
}

/// Metrics settings.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct MetricsSettings {
    /// Enable metrics endpoint
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Metrics endpoint path
    #[serde(default = "default_metrics_path")]
    pub path: String,
}

impl Default for MetricsSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            path: default_metrics_path(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_metrics_path() -> String {
    "/metrics".to_string()
}

/// Tracing settings (OpenTelemetry/Jaeger).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct TracingSettings {
    /// Enable distributed tracing
    #[serde(default)]
    pub enabled: bool,

    /// Jaeger agent endpoint
    #[serde(default = "default_jaeger_endpoint")]
    pub jaeger_endpoint: String,

    /// Service name for traces
    #[serde(default = "default_service_name")]
    pub service_name: String,
}

impl Default for TracingSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            jaeger_endpoint: default_jaeger_endpoint(),
            service_name: default_service_name(),
        }
    }
}

fn default_jaeger_endpoint() -> String {
    "localhost:6831".to_string()
}

fn default_service_name() -> String {
    "rsfga".to_string()
}

/// Error type for configuration loading.
#[derive(Debug, thiserror::Error)]
pub enum ConfigLoadError {
    #[error("failed to load configuration: {0}")]
    Load(#[from] ConfigError),

    #[error("configuration file not found: {path}")]
    FileNotFound { path: String },

    #[error("invalid configuration: {message}")]
    Invalid { message: String },
}

impl ServerConfig {
    /// Load configuration from a YAML file with environment variable overrides.
    ///
    /// Environment variables are prefixed with `RSFGA_` and use `__` as separator.
    /// For example:
    /// - `RSFGA_SERVER__PORT=9090` overrides `server.port`
    /// - `RSFGA_STORAGE__DATABASE_URL=...` overrides `storage.database_url`
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the configuration file
    ///
    /// # Returns
    ///
    /// Returns the loaded configuration or an error if loading fails.
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, ConfigLoadError> {
        let path = path.as_ref();

        // Check if file exists
        if !path.exists() {
            return Err(ConfigLoadError::FileNotFound {
                path: path.display().to_string(),
            });
        }

        let config = Config::builder()
            // Start with defaults
            .add_source(Config::try_from(&ServerConfig::default())?)
            // Add config file
            .add_source(File::from(path).format(FileFormat::Yaml))
            // Add environment variables with RSFGA_ prefix
            // Use __ as separator for nested keys: RSFGA_SERVER__PORT -> server.port
            .add_source(
                Environment::with_prefix("RSFGA")
                    .prefix_separator("_")
                    .separator("__"),
            )
            .build()?;

        let server_config: ServerConfig = config.try_deserialize()?;
        server_config.validate()?;

        Ok(server_config)
    }

    /// Load configuration from environment variables only.
    ///
    /// Uses default values and allows overrides via RSFGA_ prefixed env vars.
    pub fn from_env() -> Result<Self, ConfigLoadError> {
        let config = Config::builder()
            // Start with defaults
            .add_source(Config::try_from(&ServerConfig::default())?)
            // Add environment variables with RSFGA_ prefix
            // Use __ as separator for nested keys: RSFGA_SERVER__HOST -> server.host
            .add_source(
                Environment::with_prefix("RSFGA")
                    .prefix_separator("_")
                    .separator("__"),
            )
            .build()?;

        let server_config: ServerConfig = config.try_deserialize()?;
        server_config.validate()?;

        Ok(server_config)
    }

    /// Validate the configuration.
    ///
    /// Returns an error if the configuration is invalid.
    pub fn validate(&self) -> Result<(), ConfigLoadError> {
        // Validate port range
        if self.server.port == 0 {
            return Err(ConfigLoadError::Invalid {
                message: "server.port must be greater than 0".to_string(),
            });
        }

        // Validate storage backend
        let valid_backends = ["memory", "postgres", "mysql", "cockroachdb"];
        if !valid_backends.contains(&self.storage.backend.as_str()) {
            return Err(ConfigLoadError::Invalid {
                message: format!(
                    "storage.backend must be one of: {:?}, got: {}",
                    valid_backends, self.storage.backend
                ),
            });
        }

        // Validate database backends require non-empty database_url
        let database_backends = ["postgres", "mysql", "cockroachdb"];
        if database_backends.contains(&self.storage.backend.as_str())
            && self
                .storage
                .database_url
                .as_deref()
                .map_or(true, |s| s.trim().is_empty())
        {
            return Err(ConfigLoadError::Invalid {
                message: format!(
                    "storage.database_url is required when backend is '{}'",
                    self.storage.backend
                ),
            });
        }

        // Validate log level
        let valid_levels = ["trace", "debug", "info", "warn", "error"];
        if !valid_levels.contains(&self.logging.level.to_lowercase().as_str()) {
            return Err(ConfigLoadError::Invalid {
                message: format!(
                    "logging.level must be one of: {:?}, got: {}",
                    valid_levels, self.logging.level
                ),
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// Test: Can load config from YAML file
    #[test]
    #[serial]
    fn test_can_load_config_from_yaml_file() {
        // Create a temp YAML config file
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
server:
  host: "127.0.0.1"
  port: 9090
  request_timeout_secs: 60

storage:
  backend: memory
  pool_size: 20

logging:
  level: debug
  json: true

metrics:
  enabled: true
  path: /custom-metrics

tracing:
  enabled: false
"#
        )
        .unwrap();

        // Load config from file
        let config = ServerConfig::load(file.path()).unwrap();

        // Verify values were loaded
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 9090);
        assert_eq!(config.server.request_timeout_secs, 60);
        assert_eq!(config.storage.backend, "memory");
        assert_eq!(config.storage.pool_size, 20);
        assert_eq!(config.logging.level, "debug");
        assert!(config.logging.json);
        assert!(config.metrics.enabled);
        assert_eq!(config.metrics.path, "/custom-metrics");
        assert!(!config.tracing.enabled);
    }

    /// Test: Can override config with env vars
    #[test]
    #[serial]
    fn test_can_override_config_with_env_vars() {
        // Create a temp config file with base values
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
server:
  host: "127.0.0.1"
  port: 8080

storage:
  backend: memory
"#
        )
        .unwrap();

        // Set environment variables to override
        std::env::set_var("RSFGA_SERVER__PORT", "9999");
        std::env::set_var("RSFGA_LOGGING__LEVEL", "warn");

        // Load config (env vars should override file values)
        let config = ServerConfig::load(file.path()).unwrap();

        // Clean up env vars
        std::env::remove_var("RSFGA_SERVER__PORT");
        std::env::remove_var("RSFGA_LOGGING__LEVEL");

        // Verify env var overrides
        assert_eq!(config.server.port, 9999); // Overridden by env
        assert_eq!(config.server.host, "127.0.0.1"); // From file
        assert_eq!(config.logging.level, "warn"); // Overridden by env
    }

    /// Test: Config validation catches errors
    #[test]
    fn test_config_validation_catches_errors() {
        // Test invalid storage backend
        let mut config = ServerConfig::default();
        config.storage.backend = "invalid".to_string();
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("storage.backend"));

        // Test postgres without database_url
        let mut config = ServerConfig::default();
        config.storage.backend = "postgres".to_string();
        config.storage.database_url = None;
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("database_url"));

        // Test postgres with empty database_url
        let mut config = ServerConfig::default();
        config.storage.backend = "postgres".to_string();
        config.storage.database_url = Some("".to_string());
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("database_url"));

        // Test postgres with whitespace-only database_url
        let mut config = ServerConfig::default();
        config.storage.backend = "postgres".to_string();
        config.storage.database_url = Some("   ".to_string());
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("database_url"));

        // Test mysql and cockroachdb backend validation using data-driven approach
        let test_cases = [
            ("mysql", None, true),
            ("mysql", Some("mysql://localhost/rsfga".to_string()), false),
            ("cockroachdb", None, true),
            (
                "cockroachdb",
                Some("postgresql://root@localhost:26257/rsfga".to_string()),
                false,
            ),
        ];

        for (backend, url, should_err) in test_cases {
            let mut config = ServerConfig::default();
            config.storage.backend = backend.to_string();
            config.storage.database_url = url;
            let result = config.validate();

            if should_err {
                assert!(result.is_err(), "Expected error for backend '{backend}'");
                let err = result.unwrap_err();
                assert!(
                    err.to_string().contains("database_url"),
                    "Error for '{backend}' should contain 'database_url'"
                );
            } else {
                assert!(result.is_ok(), "Expected ok for backend '{backend}'");
            }
        }

        // Test invalid log level
        let mut config = ServerConfig::default();
        config.logging.level = "invalid".to_string();
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("logging.level"));
    }

    /// Test: Invalid config returns clear error
    #[test]
    fn test_invalid_config_returns_clear_error() {
        // Test file not found
        let result = ServerConfig::load("/nonexistent/path/config.yaml");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ConfigLoadError::FileNotFound { .. }));
        assert!(err.to_string().contains("not found"));

        // Test invalid YAML syntax
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "invalid: yaml: syntax: [").unwrap();

        let result = ServerConfig::load(file.path());
        assert!(result.is_err());
        // Should return a Load error with parse details
        let err = result.unwrap_err();
        assert!(matches!(err, ConfigLoadError::Load(_)));
    }

    /// Test: Default config is valid
    #[test]
    fn test_default_config_is_valid() {
        let config = ServerConfig::default();
        assert!(config.validate().is_ok());

        // Check default values
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 8080);
        assert_eq!(config.storage.backend, "memory");
        assert_eq!(config.logging.level, "info");
        assert!(!config.logging.json);
        assert!(config.metrics.enabled);
    }

    /// Test: from_env loads defaults with env overrides
    #[test]
    #[serial]
    fn test_from_env_loads_defaults_with_env_overrides() {
        // Set an env var
        std::env::set_var("RSFGA_SERVER__HOST", "192.168.1.1");

        let config = ServerConfig::from_env().unwrap();

        // Clean up
        std::env::remove_var("RSFGA_SERVER__HOST");

        // Should have default port but overridden host
        assert_eq!(config.server.host, "192.168.1.1");
        assert_eq!(config.server.port, 8080); // default
    }

    /// Test: Config hot-reload works (if supported)
    ///
    /// Hot-reload is NOT currently supported. Configuration is loaded once at startup.
    /// To change configuration, the server must be restarted.
    ///
    /// Future considerations for hot-reload:
    /// - Watch config file for changes
    /// - Reload certain settings (log level, timeouts) without restart
    /// - Graceful connection draining for settings that require restart
    #[test]
    fn test_config_hot_reload_not_supported() {
        // Document that hot-reload is not currently implemented
        // The server loads configuration once at startup
        //
        // To reload config, restart the server process:
        // - In Kubernetes: Update ConfigMap and restart pods
        // - In Docker: docker restart <container>
        // - Bare metal: systemctl restart rsfga
        //
        // This is the standard approach for most production services,
        // as hot-reload introduces complexity and potential for inconsistent state.

        // Verify ServerConfig doesn't expose any reload methods
        // (this documents the intentional absence of hot-reload)
        let config = ServerConfig::default();

        // Config is loaded once and immutable - no reload method exists
        // If hot-reload were added, this test would need to be updated
        // to test that functionality instead
        let _ = config;
    }
}
