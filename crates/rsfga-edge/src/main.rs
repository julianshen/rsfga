//! RSFGA Edge Sync Daemon
//!
//! This daemon subscribes to RSFGA_EVENTS from NATS JetStream and applies
//! committed events to local edge storage, enabling distributed authorization
//! with eventual consistency.
//!
//! Architecture:
//! - Pull consumer on RSFGA_EVENTS stream
//! - Optional store filtering (replicate only specific stores)
//! - Idempotent application via sync watermark per store
//! - Graceful shutdown with position persistence

mod consumer;
mod error;
mod metrics;
pub mod watermark_store;

use std::collections::HashSet;
use std::sync::Arc;

use clap::Parser;
use tracing::{error, info};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use rsfga_nats::{JetStreamManager, NatsClient, NatsConfig};
use rsfga_storage::DataStore;

use crate::consumer::{EdgeConsumer, EdgeConsumerConfig};
use crate::error::Result;
use crate::metrics::EdgeMetrics;

/// Redact credentials from URLs for safe logging.
///
/// Redacts both the username and password fields of the userinfo section
/// to prevent token/credential leakage. NATS tokens often appear in the
/// username field (e.g., `nats://token@localhost:4222`).
fn redact_url(url_str: &str) -> String {
    match url::Url::parse(url_str) {
        Ok(mut url) => {
            let has_username = !url.username().is_empty();
            let has_password = url.password().is_some();
            if has_username || has_password {
                let _ = url.set_username("***");
                let _ = url.set_password(None);
            }
            url.to_string()
        }
        Err(_) => url_str.to_string(),
    }
}

/// RSFGA Edge - Distributed edge sync daemon
#[derive(Parser, Debug)]
#[command(name = "rsfga-edge")]
#[command(version)]
#[command(about = "RSFGA edge sync daemon for distributed authorization")]
struct Args {
    /// NATS server URL
    #[arg(long, env = "NATS_URL", default_value = "nats://localhost:4222")]
    nats_url: String,

    /// Consumer name (must be unique per edge instance)
    #[arg(long, env = "CONSUMER_NAME", default_value = "rsfga-edge-1")]
    consumer_name: String,

    /// Store filter (comma-separated list of store IDs to replicate)
    #[arg(long, env = "STORE_FILTER", value_delimiter = ',')]
    store_filter: Option<Vec<String>>,

    /// Batch size for message fetching
    #[arg(long, env = "BATCH_SIZE", default_value = "100")]
    batch_size: usize,

    /// Fetch timeout in milliseconds
    #[arg(long, env = "FETCH_TIMEOUT_MS", default_value = "500")]
    fetch_timeout_ms: u64,

    /// Storage backend (memory, postgres, mysql, rocksdb)
    #[arg(long, env = "STORAGE_BACKEND", default_value = "memory")]
    storage_backend: String,

    /// Database URL (for postgres/mysql backends)
    #[arg(long, env = "DATABASE_URL")]
    database_url: Option<String>,

    /// RocksDB path (for rocksdb backend)
    #[arg(
        long,
        env = "ROCKSDB_PATH",
        default_value = "/var/lib/rsfga-edge/rocksdb"
    )]
    rocksdb_path: String,

    /// Prometheus metrics port
    #[arg(long, env = "METRICS_PORT", default_value = "9091")]
    metrics_port: u16,

    /// Log level (used when RUST_LOG env var is not set)
    #[arg(long, env = "LOG_LEVEL", default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize tracing: RUST_LOG env var takes precedence over --log-level flag
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&args.log_level));
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(env_filter)
        .init();

    info!(
        version = env!("CARGO_PKG_VERSION"),
        consumer_name = %args.consumer_name,
        nats_url = %redact_url(&args.nats_url),
        store_filter = ?args.store_filter,
        batch_size = args.batch_size,
        fetch_timeout_ms = args.fetch_timeout_ms,
        storage_backend = %args.storage_backend,
        "Starting RSFGA Edge sync daemon"
    );

    // Initialize metrics
    let metrics = Arc::new(EdgeMetrics::new());
    metrics::setup_metrics_server(args.metrics_port).await?;

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

    // Create edge consumer config
    let store_filter: Option<HashSet<String>> = args.store_filter.map(|v| v.into_iter().collect());

    let consumer_config = EdgeConsumerConfig {
        consumer_name: args.consumer_name,
        store_filter,
        batch_size: args.batch_size,
        fetch_timeout: std::time::Duration::from_millis(args.fetch_timeout_ms),
        max_delivery_count: consumer::DEFAULT_MAX_DELIVERY_COUNT,
    };

    // Create consumer (synchronous - no async work needed)
    let consumer = EdgeConsumer::new(nats_client, storage, metrics, consumer_config);

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
        error!(error = %e, "Edge consumer error");
        return Err(e);
    }

    info!("RSFGA Edge sync daemon stopped");
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
                let storage = RocksDBDataStore::new(config)?;
                Ok(Arc::new(storage))
            }
            #[cfg(not(feature = "rocksdb"))]
            {
                Err(crate::error::EdgeError::Config(
                    "RocksDB support not compiled in. Rebuild with --features rocksdb".to_string(),
                ))
            }
        }
        "postgres" => {
            let url = args.database_url.as_ref().ok_or_else(|| {
                crate::error::EdgeError::Config(
                    "DATABASE_URL required for postgres backend".to_string(),
                )
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
                crate::error::EdgeError::Config(
                    "DATABASE_URL required for mysql backend".to_string(),
                )
            })?;
            info!(url = %redact_url(url), "Using MySQL storage backend");
            let config = rsfga_storage::MySQLConfig {
                database_url: url.clone(),
                ..Default::default()
            };
            let storage = rsfga_storage::MySQLDataStore::from_config(&config).await?;
            Ok(Arc::new(storage))
        }
        other => Err(crate::error::EdgeError::Config(format!(
            "Unknown storage backend: {}",
            other
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact_url_with_user_and_password() {
        let url = "nats://admin:secret@localhost:4222";
        let redacted = redact_url(url);
        assert!(!redacted.contains("admin"));
        assert!(!redacted.contains("secret"));
        assert!(redacted.contains("***@"));
    }

    #[test]
    fn test_redact_url_no_credentials() {
        let url = "nats://localhost:4222";
        let redacted = redact_url(url);
        assert_eq!(redacted, "nats://localhost:4222");
    }

    #[test]
    fn test_redact_url_token_in_username() {
        // NATS tokens often appear as just the username: nats://token@host
        let url = "nats://my-secret-token@localhost:4222";
        let redacted = redact_url(url);
        assert!(!redacted.contains("my-secret-token"));
        assert!(redacted.contains("***@"));
    }

    #[test]
    fn test_redact_url_postgres() {
        let url = "postgres://user:password@localhost:5432/db";
        let redacted = redact_url(url);
        assert!(!redacted.contains("user"));
        assert!(!redacted.contains("password"));
        assert!(redacted.contains("***@"));
    }
}
