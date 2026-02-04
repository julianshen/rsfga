//! NATS configuration for RSFGA.
//!
//! This module provides configuration structures for connecting to NATS,
//! including server addresses, authentication, TLS, and JetStream settings.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Configuration for NATS connection and JetStream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NatsConfig {
    /// NATS server URLs (e.g., "nats://localhost:4222")
    #[serde(default = "default_servers")]
    pub servers: Vec<String>,

    /// Connection name for identification
    #[serde(default = "default_name")]
    pub name: String,

    /// Connection timeout
    #[serde(default = "default_connect_timeout", with = "humantime_serde")]
    pub connect_timeout: Duration,

    /// Request timeout for JetStream operations
    #[serde(default = "default_request_timeout", with = "humantime_serde")]
    pub request_timeout: Duration,

    /// Reconnection settings
    #[serde(default)]
    pub reconnect: ReconnectConfig,

    /// Authentication settings
    #[serde(default)]
    pub auth: AuthConfig,

    /// TLS settings
    #[serde(default)]
    pub tls: TlsConfig,

    /// JetStream settings
    #[serde(default)]
    pub jetstream: JetStreamConfig,

    /// Write mode: "nats" (async-first), "direct" (sync), "auto" (fallback)
    #[serde(default = "default_write_mode")]
    pub write_mode: WriteMode,
}

impl Default for NatsConfig {
    fn default() -> Self {
        Self {
            servers: default_servers(),
            name: default_name(),
            connect_timeout: default_connect_timeout(),
            request_timeout: default_request_timeout(),
            reconnect: ReconnectConfig::default(),
            auth: AuthConfig::default(),
            tls: TlsConfig::default(),
            jetstream: JetStreamConfig::default(),
            write_mode: default_write_mode(),
        }
    }
}

/// Write mode configuration.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WriteMode {
    /// NATS-first writes (async, low latency)
    Nats,
    /// Direct storage writes (sync, strong consistency)
    /// Safe default: no change from current behavior
    #[default]
    Direct,
    /// Auto-fallback: try NATS, fallback to direct on failure
    Auto,
}

/// Reconnection configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconnectConfig {
    /// Enable automatic reconnection
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Maximum reconnection attempts (0 = unlimited)
    #[serde(default = "default_max_reconnects")]
    pub max_attempts: u32,

    /// Initial delay between reconnection attempts
    #[serde(default = "default_reconnect_delay", with = "humantime_serde")]
    pub initial_delay: Duration,

    /// Maximum delay between reconnection attempts
    #[serde(default = "default_max_reconnect_delay", with = "humantime_serde")]
    pub max_delay: Duration,

    /// Jitter factor for reconnection delay (0.0 - 1.0)
    #[serde(default = "default_jitter")]
    pub jitter: f64,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            max_attempts: default_max_reconnects(),
            initial_delay: default_reconnect_delay(),
            max_delay: default_max_reconnect_delay(),
            jitter: default_jitter(),
        }
    }
}

/// Authentication configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuthConfig {
    /// Username for basic auth
    pub username: Option<String>,

    /// Password for basic auth (sensitive - hidden in Debug)
    #[serde(skip_serializing)]
    pub password: Option<String>,

    /// Token for token auth (sensitive - hidden in Debug)
    #[serde(skip_serializing)]
    pub token: Option<String>,

    /// Path to credentials file (.creds)
    pub credentials_path: Option<String>,

    /// Path to NKey seed file
    pub nkey_seed_path: Option<String>,

    /// JWT token for JWT auth
    #[serde(skip_serializing)]
    pub jwt: Option<String>,
}

impl AuthConfig {
    /// Check if any authentication is configured.
    pub fn is_configured(&self) -> bool {
        self.username.is_some()
            || self.token.is_some()
            || self.credentials_path.is_some()
            || self.nkey_seed_path.is_some()
            || self.jwt.is_some()
    }
}

/// TLS configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TlsConfig {
    /// Enable TLS
    #[serde(default)]
    pub enabled: bool,

    /// Path to CA certificate file
    pub ca_cert_path: Option<String>,

    /// Path to client certificate file
    pub client_cert_path: Option<String>,

    /// Path to client key file
    pub client_key_path: Option<String>,

    /// Skip server certificate verification (INSECURE - for testing only)
    #[serde(default)]
    pub insecure_skip_verify: bool,
}

/// JetStream configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JetStreamConfig {
    /// Domain for multi-tenant JetStream
    pub domain: Option<String>,

    /// API prefix (usually empty or domain-specific)
    pub api_prefix: Option<String>,

    /// Deduplication window for message dedup
    #[serde(default = "default_dedup_window", with = "humantime_serde")]
    pub dedup_window: Duration,

    /// RSFGA_WRITES stream configuration
    #[serde(default)]
    pub writes_stream: StreamConfig,

    /// RSFGA_EVENTS stream configuration
    #[serde(default)]
    pub events_stream: EventsStreamConfig,
}

impl Default for JetStreamConfig {
    fn default() -> Self {
        Self {
            domain: None,
            api_prefix: None,
            dedup_window: default_dedup_window(),
            writes_stream: StreamConfig::default(),
            events_stream: EventsStreamConfig::default(),
        }
    }
}

/// Configuration for RSFGA_WRITES stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamConfig {
    /// Number of replicas for durability
    #[serde(default = "default_replicas")]
    pub replicas: u32,

    /// Maximum age of messages (0 = unlimited)
    #[serde(default, with = "humantime_serde")]
    pub max_age: Duration,

    /// Maximum number of messages (0 = unlimited)
    #[serde(default)]
    pub max_messages: i64,

    /// Maximum bytes (0 = unlimited)
    #[serde(default)]
    pub max_bytes: i64,

    /// Maximum message size in bytes
    #[serde(default = "default_max_message_size")]
    pub max_message_size: i32,

    /// Storage type: "file" or "memory"
    #[serde(default = "default_storage")]
    pub storage: StorageType,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            replicas: default_replicas(),
            max_age: Duration::ZERO,
            max_messages: 0,
            max_bytes: 0,
            max_message_size: default_max_message_size(),
            storage: default_storage(),
        }
    }
}

/// Configuration for RSFGA_EVENTS stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventsStreamConfig {
    /// Number of replicas for durability
    #[serde(default = "default_replicas")]
    pub replicas: u32,

    /// Maximum age of messages
    #[serde(default = "default_events_max_age", with = "humantime_serde")]
    pub max_age: Duration,

    /// Maximum number of messages (0 = unlimited)
    #[serde(default)]
    pub max_messages: i64,

    /// Maximum bytes (0 = unlimited)
    #[serde(default)]
    pub max_bytes: i64,

    /// Maximum message size in bytes
    #[serde(default = "default_max_message_size")]
    pub max_message_size: i32,

    /// Storage type: "file" or "memory"
    #[serde(default = "default_storage")]
    pub storage: StorageType,
}

impl Default for EventsStreamConfig {
    fn default() -> Self {
        Self {
            replicas: default_replicas(),
            max_age: default_events_max_age(),
            max_messages: 0,
            max_bytes: 0,
            max_message_size: default_max_message_size(),
            storage: default_storage(),
        }
    }
}

/// Storage type for JetStream streams.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageType {
    #[default]
    File,
    Memory,
}

// Default value functions

fn default_servers() -> Vec<String> {
    vec!["nats://localhost:4222".to_string()]
}

fn default_name() -> String {
    "rsfga".to_string()
}

fn default_connect_timeout() -> Duration {
    Duration::from_secs(10)
}

fn default_request_timeout() -> Duration {
    Duration::from_secs(5)
}

fn default_write_mode() -> WriteMode {
    WriteMode::Direct
}

fn default_true() -> bool {
    true
}

fn default_max_reconnects() -> u32 {
    60 // Try for ~10 minutes with default delays
}

fn default_reconnect_delay() -> Duration {
    Duration::from_millis(100)
}

fn default_max_reconnect_delay() -> Duration {
    Duration::from_secs(10)
}

fn default_jitter() -> f64 {
    0.1
}

fn default_dedup_window() -> Duration {
    Duration::from_secs(120)
}

fn default_replicas() -> u32 {
    1 // Single replica for development; set to 3 for production
}

fn default_max_message_size() -> i32 {
    1024 * 1024 // 1MB
}

fn default_storage() -> StorageType {
    StorageType::File
}

fn default_events_max_age() -> Duration {
    Duration::from_secs(7 * 24 * 60 * 60) // 7 days
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = NatsConfig::default();
        assert_eq!(config.servers, vec!["nats://localhost:4222"]);
        assert_eq!(config.name, "rsfga");
        assert_eq!(config.connect_timeout, Duration::from_secs(10));
        assert_eq!(config.write_mode, WriteMode::Direct);
    }

    #[test]
    fn test_write_mode_serde() {
        assert_eq!(serde_json::to_string(&WriteMode::Nats).unwrap(), "\"nats\"");
        assert_eq!(
            serde_json::to_string(&WriteMode::Direct).unwrap(),
            "\"direct\""
        );
        assert_eq!(serde_json::to_string(&WriteMode::Auto).unwrap(), "\"auto\"");
    }

    #[test]
    fn test_auth_is_configured() {
        let auth = AuthConfig::default();
        assert!(!auth.is_configured());

        let auth = AuthConfig {
            token: Some("secret".to_string()),
            ..Default::default()
        };
        assert!(auth.is_configured());
    }

    #[test]
    fn test_reconnect_config_defaults() {
        let config = ReconnectConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_attempts, 60);
        assert_eq!(config.initial_delay, Duration::from_millis(100));
        assert_eq!(config.max_delay, Duration::from_secs(10));
        assert!((config.jitter - 0.1).abs() < f64::EPSILON);
    }

    #[test]
    fn test_stream_config_defaults() {
        let config = StreamConfig::default();
        assert_eq!(config.replicas, 1);
        assert_eq!(config.max_message_size, 1024 * 1024);
        assert_eq!(config.storage, StorageType::File);
    }

    #[test]
    fn test_events_stream_config_defaults() {
        let config = EventsStreamConfig::default();
        assert_eq!(config.replicas, 1);
        assert_eq!(config.max_age, Duration::from_secs(7 * 24 * 60 * 60)); // 7 days
    }

    #[test]
    fn test_jetstream_config_defaults() {
        let config = JetStreamConfig::default();
        assert_eq!(config.dedup_window, Duration::from_secs(120));
        assert!(config.domain.is_none());
    }

    #[test]
    fn test_config_serialization() {
        let config = NatsConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: NatsConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.servers, config.servers);
        assert_eq!(parsed.name, config.name);
    }

    #[test]
    fn test_auth_password_not_serialized() {
        let auth = AuthConfig {
            username: Some("user".to_string()),
            password: Some("secret".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&auth).unwrap();
        assert!(json.contains("user"));
        assert!(!json.contains("secret"));
    }
}
