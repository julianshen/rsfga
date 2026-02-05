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

mod consumer;
mod error;
mod metrics;

use std::sync::Arc;

use clap::Parser;
use tracing::{error, info};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use rsfga_nats::{JetStreamManager, NatsClient, NatsConfig};
use rsfga_storage::DataStore;

use crate::consumer::WriteConsumer;
use crate::error::Result;
use crate::metrics::WriterMetrics;

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

    /// Number of parallel workers for processing batches
    #[arg(long, env = "WORKERS", default_value = "4")]
    workers: usize,

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
        nats_url = %args.nats_url,
        batch_size = args.batch_size,
        batch_timeout_ms = args.batch_timeout_ms,
        workers = args.workers,
        storage_backend = %args.storage_backend,
        "Starting RSFGA Writer daemon"
    );

    // Initialize metrics
    let metrics = Arc::new(WriterMetrics::new());
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

    // Create and run consumer
    let consumer = WriteConsumer::new(
        nats_client,
        storage,
        metrics,
        consumer::ConsumerConfig {
            consumer_name: args.consumer_name,
            batch_size: args.batch_size,
            batch_timeout: std::time::Duration::from_millis(args.batch_timeout_ms),
            workers: args.workers,
        },
    )
    .await?;

    // Run until shutdown signal
    tokio::select! {
        result = consumer.run() => {
            if let Err(e) = result {
                error!(error = %e, "Consumer error");
                return Err(e);
            }
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Received shutdown signal");
        }
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
                    .map_err(|e| crate::error::WriterError::Config(e.to_string()))?;
                Ok(Arc::new(storage))
            }
            #[cfg(not(feature = "rocksdb"))]
            {
                Err(crate::error::WriterError::Config(
                    "RocksDB support not compiled in. Rebuild with --features rocksdb".to_string(),
                ))
            }
        }
        "postgres" => {
            let url = args.database_url.as_ref().ok_or_else(|| {
                crate::error::WriterError::Config(
                    "DATABASE_URL required for postgres backend".to_string(),
                )
            })?;
            info!(url = %url, "Using PostgreSQL storage backend");
            let config = rsfga_storage::PostgresConfig {
                database_url: url.clone(),
                ..Default::default()
            };
            let storage = rsfga_storage::PostgresDataStore::from_config(&config).await?;
            Ok(Arc::new(storage))
        }
        "mysql" => {
            let url = args.database_url.as_ref().ok_or_else(|| {
                crate::error::WriterError::Config(
                    "DATABASE_URL required for mysql backend".to_string(),
                )
            })?;
            info!(url = %url, "Using MySQL storage backend");
            let config = rsfga_storage::MySQLConfig {
                database_url: url.clone(),
                ..Default::default()
            };
            let storage = rsfga_storage::MySQLDataStore::from_config(&config).await?;
            Ok(Arc::new(storage))
        }
        other => Err(crate::error::WriterError::Config(format!(
            "Unknown storage backend: {}",
            other
        ))),
    }
}
