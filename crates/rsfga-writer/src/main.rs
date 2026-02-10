//! RSFGA Writer Daemon
//!
//! This daemon consumes write events from NATS JetStream and batches them
//! to the storage backend for improved throughput.
//!
//! Architecture:
//! - Pull consumer on RSFGA_WRITES stream
//! - Message batching (500 msgs or 100ms timeout)
//! - Group by store_id for parallel processing
//! - Idempotent writes using ON CONFLICT
//! - Publish CommittedEvent to RSFGA_EVENTS after commit

use std::sync::Arc;

use clap::Parser;
use tracing::{error, info};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use rsfga_nats::{JetStreamManager, NatsClient, NatsConfig};
use rsfga_storage::DataStore;
use rsfga_writer::consumer::{ConsumerConfig, WriteConsumer, DEFAULT_MAX_DELIVERY_COUNT};
use rsfga_writer::error::{Result, WriterError};
use rsfga_writer::metrics::{setup_metrics_server, WriterMetrics};

/// Redact credentials from URLs for safe logging.
///
/// Uses proper URL parsing to handle edge cases like:
/// - Passwords containing `@` or `:` characters
/// - URLs without credentials (returned unchanged)
/// - Invalid URLs (returned unchanged with fallback parsing)
///
/// Examples:
/// - `postgres://user:password@host` → `postgres://user:***@host`
/// - `nats://user:p@ss:word@host:4222` → `nats://user:***@host:4222`
fn redact_url(url_str: &str) -> String {
    match url::Url::parse(url_str) {
        Ok(mut url) => {
            if url.password().is_some() {
                // Use expect here since we just confirmed password exists
                let _ = url.set_password(Some("***"));
            }
            url.to_string()
        }
        Err(_) => {
            // If URL parsing fails, fall back to simple string matching
            // This handles non-standard URL formats
            if let Some(scheme_end) = url_str.find("://") {
                let after_scheme = &url_str[scheme_end + 3..];
                // Find the last @ before any / (to handle @ in passwords)
                if let Some(slash_pos) = after_scheme.find('/') {
                    let userinfo_host = &after_scheme[..slash_pos];
                    if let Some(at_pos) = userinfo_host.rfind('@') {
                        let user_pass = &userinfo_host[..at_pos];
                        if let Some(colon_pos) = user_pass.find(':') {
                            let user = &user_pass[..colon_pos];
                            let host_and_rest = &after_scheme[at_pos..];
                            return format!(
                                "{}://{}:***{}",
                                &url_str[..scheme_end],
                                user,
                                host_and_rest
                            );
                        }
                    }
                } else if let Some(at_pos) = after_scheme.rfind('@') {
                    let user_pass = &after_scheme[..at_pos];
                    if let Some(colon_pos) = user_pass.find(':') {
                        let user = &user_pass[..colon_pos];
                        let rest = &after_scheme[at_pos..];
                        return format!("{}://{}:***{}", &url_str[..scheme_end], user, rest);
                    }
                }
            }
            url_str.to_string()
        }
    }
}

/// RSFGA Writer - Storage consumer daemon for async writes
#[derive(Parser, Debug)]
#[command(name = "rsfga-writer")]
#[command(version)]
#[command(about = "RSFGA storage consumer daemon for NATS async writes")]
struct Args {
    /// NATS server URL
    #[arg(long, env = "NATS_URL", default_value = "nats://localhost:4222")]
    nats_url: String,

    /// Consumer name (must be unique per instance)
    #[arg(long, env = "CONSUMER_NAME", default_value = "rsfga-writer-1")]
    consumer_name: String,

    /// Batch size (number of messages before flush)
    #[arg(long, env = "BATCH_SIZE", default_value = "500")]
    batch_size: usize,

    /// Batch timeout in milliseconds
    #[arg(long, env = "BATCH_TIMEOUT_MS", default_value = "100")]
    batch_timeout_ms: u64,

    /// Storage backend (memory, postgres, mysql, rocksdb)
    #[arg(long, env = "STORAGE_BACKEND", default_value = "memory")]
    storage_backend: String,

    /// Database URL (for postgres/mysql backends)
    #[arg(long, env = "DATABASE_URL")]
    database_url: Option<String>,

    /// RocksDB path (for rocksdb backend)
    #[arg(long, env = "ROCKSDB_PATH", default_value = "/var/lib/rsfga/rocksdb")]
    rocksdb_path: String,

    /// Prometheus metrics port
    #[arg(long, env = "METRICS_PORT", default_value = "9090")]
    metrics_port: u16,

    /// Log level
    #[arg(long, env = "RUST_LOG", default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize tracing
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&args.log_level)))
        .init();

    info!(
        version = env!("CARGO_PKG_VERSION"),
        consumer_name = %args.consumer_name,
        nats_url = %redact_url(&args.nats_url),
        batch_size = args.batch_size,
        batch_timeout_ms = args.batch_timeout_ms,
        storage_backend = %args.storage_backend,
        "Starting RSFGA Writer daemon"
    );

    // Initialize metrics
    let metrics = Arc::new(WriterMetrics::new());
    setup_metrics_server(args.metrics_port).await?;

    // Connect to NATS
    let nats_config = NatsConfig {
        servers: vec![args.nats_url.clone()],
        ..Default::default()
    };
    let nats_client = NatsClient::connect(nats_config).await?;

    // Ensure JetStream streams exist
    let js_manager = JetStreamManager::new(nats_client.clone());
    js_manager.setup_streams().await?;

    // Create storage backend
    let storage: Arc<dyn DataStore> = create_storage(&args).await?;

    // Create and run consumer
    let consumer = WriteConsumer::new(
        nats_client,
        storage,
        metrics,
        ConsumerConfig {
            consumer_name: args.consumer_name,
            batch_size: args.batch_size,
            batch_timeout: std::time::Duration::from_millis(args.batch_timeout_ms),
            max_delivery_count: DEFAULT_MAX_DELIVERY_COUNT,
        },
    )
    .await?;

    // Set up graceful shutdown signal
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // Spawn shutdown signal handler
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            info!("Received shutdown signal, initiating graceful shutdown");
            let _ = shutdown_tx.send(true);
        }
    });

    // Run consumer with graceful shutdown
    if let Err(e) = consumer.run_with_shutdown(shutdown_rx).await {
        error!(error = %e, "Consumer error");
        return Err(e);
    }

    info!("RSFGA Writer daemon stopped");
    Ok(())
}

/// Create storage backend based on configuration.
async fn create_storage(args: &Args) -> Result<Arc<dyn DataStore>> {
    match args.storage_backend.as_str() {
        "memory" => {
            info!("Using in-memory storage backend");
            Ok(Arc::new(rsfga_storage::MemoryDataStore::new()))
        }
        "rocksdb" => {
            #[cfg(feature = "rocksdb")]
            {
                use rsfga_storage::{RocksDBConfig, RocksDBDataStore};
                info!(path = %args.rocksdb_path, "Using RocksDB storage backend");
                let config = RocksDBConfig {
                    path: args.rocksdb_path.clone(),
                    ..Default::default()
                };
                let storage = RocksDBDataStore::new(config)
                    .map_err(|e| WriterError::Config(e.to_string()))?;
                Ok(Arc::new(storage))
            }
            #[cfg(not(feature = "rocksdb"))]
            {
                Err(WriterError::Config(
                    "RocksDB support not compiled in. Rebuild with --features rocksdb".to_string(),
                ))
            }
        }
        "postgres" => {
            let url = args.database_url.as_ref().ok_or_else(|| {
                WriterError::Config("DATABASE_URL required for postgres backend".to_string())
            })?;
            info!(url = %redact_url(url), "Using PostgreSQL storage backend");
            let config = rsfga_storage::PostgresConfig {
                database_url: url.clone(),
                ..Default::default()
            };
            let storage = rsfga_storage::PostgresDataStore::from_config(&config).await?;
            Ok(Arc::new(storage))
        }
        "mysql" => {
            let url = args.database_url.as_ref().ok_or_else(|| {
                WriterError::Config("DATABASE_URL required for mysql backend".to_string())
            })?;
            info!(url = %redact_url(url), "Using MySQL storage backend");
            let config = rsfga_storage::MySQLConfig {
                database_url: url.clone(),
                ..Default::default()
            };
            let storage = rsfga_storage::MySQLDataStore::from_config(&config).await?;
            Ok(Arc::new(storage))
        }
        other => Err(WriterError::Config(format!(
            "Unknown storage backend: {}",
            other
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact_url_postgres_simple() {
        let url = "postgres://user:password@localhost:5432/db";
        let redacted = redact_url(url);
        assert!(redacted.contains("user:***@"));
        assert!(!redacted.contains("password"));
    }

    #[test]
    fn test_redact_url_nats_simple() {
        let url = "nats://admin:secret@localhost:4222";
        let redacted = redact_url(url);
        assert!(redacted.contains("admin:***@"));
        assert!(!redacted.contains("secret"));
    }

    #[test]
    fn test_redact_url_with_at_in_password() {
        // Passwords containing @ are URL-encoded in valid URLs
        let url = "postgres://user:p%40ssword@localhost/db";
        let redacted = redact_url(url);
        assert!(redacted.contains("user:***@"));
        assert!(!redacted.contains("p%40ssword"));
    }

    #[test]
    fn test_redact_url_no_password() {
        let url = "postgres://localhost:5432/db";
        let redacted = redact_url(url);
        assert_eq!(redacted, "postgres://localhost:5432/db");
    }

    #[test]
    fn test_redact_url_no_credentials() {
        let url = "nats://localhost:4222";
        let redacted = redact_url(url);
        assert_eq!(redacted, "nats://localhost:4222");
    }

    #[test]
    fn test_redact_url_mysql() {
        let url = "mysql://root:mysecret@127.0.0.1:3306/rsfga";
        let redacted = redact_url(url);
        assert!(redacted.contains("root:***@"));
        assert!(!redacted.contains("mysecret"));
    }

    #[test]
    fn test_redact_url_with_path() {
        let url = "postgres://user:pass@host/database?ssl=true";
        let redacted = redact_url(url);
        assert!(redacted.contains("user:***@"));
        assert!(!redacted.contains(":pass@"));
        assert!(redacted.contains("/database"));
    }
}
