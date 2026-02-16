//! RSFGA Precompute Daemon
//!
//! Consumes CommittedEvents from NATS JetStream (RSFGA_EVENTS stream),
//! classifies changes, finds affected hot-path checks via the impact analyzer,
//! and submits recomputation jobs to a worker pool.
//!
//! Architecture:
//! - Pull consumer on RSFGA_EVENTS stream (filter: `rsfga.events.*.committed`)
//! - Classifies each event into tuple/model changes
//! - Queries Valkey hot-path sets to find affected checks
//! - Submits RecomputeJobs to the worker pool for async resolution
//! - Workers resolve checks via GraphResolver and write results to Valkey

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream::consumer::pull::Config as PullConfig;
use async_nats::jetstream::consumer::AckPolicy;
use clap::Parser;
use futures::StreamExt;
use tracing::{debug, error, info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use rsfga_nats::{JetStreamManager, NatsClient, NatsConfig, STREAM_EVENTS, SUBJECT_EVENTS_PREFIX};
use rsfga_storage::DataStore;
use rsfga_valkey::{ValkeyClient, ValkeyConfig};

use rsfga_precompute::adapters::{StoreModelIdProvider, StoreModelReader, StoreTupleReader};
use rsfga_precompute::classifier;
use rsfga_precompute::error::{PrecomputeError, Result};
use rsfga_precompute::impact;
use rsfga_precompute::metrics;
use rsfga_precompute::worker::WorkerPool;

use rsfga_domain::resolver::GraphResolver;
use rsfga_valkey::cache::CheckCache;

/// Type alias for per-store relation dependency map.
/// Outer key: store_id → inner: (object_type, relation) → set of dependent (object_type, relation) pairs.
type RelationDeps =
    Arc<tokio::sync::RwLock<HashMap<String, HashMap<(String, String), HashSet<(String, String)>>>>>;

/// RSFGA Precompute - Precomputation daemon for sub-millisecond check lookups
#[derive(Parser, Debug)]
#[command(name = "rsfga-precompute")]
#[command(version)]
#[command(about = "RSFGA precomputation daemon for sub-millisecond check lookups")]
struct Args {
    /// NATS server URL
    #[arg(long, env = "NATS_URL", default_value = "nats://localhost:4222")]
    nats_url: String,

    /// Consumer name (must be unique per instance)
    #[arg(long, env = "CONSUMER_NAME", default_value = "rsfga-precompute-1")]
    consumer_name: String,

    /// Number of resolver worker threads
    #[arg(long, env = "WORKERS", default_value = "4")]
    workers: usize,

    /// Maximum job queue depth
    #[arg(long, env = "MAX_QUEUE_DEPTH", default_value = "10000")]
    max_queue_depth: usize,

    /// Storage backend
    #[arg(long, env = "STORAGE_BACKEND", default_value = "memory")]
    storage_backend: String,

    /// Database URL
    #[arg(long, env = "DATABASE_URL")]
    database_url: Option<String>,

    /// RocksDB path
    #[arg(long, env = "ROCKSDB_PATH", default_value = "/var/lib/rsfga/rocksdb")]
    rocksdb_path: String,

    /// Valkey/Redis URL
    #[arg(long, env = "VALKEY_URL", default_value = "redis://localhost:6379")]
    valkey_url: String,

    /// Valkey result TTL in seconds
    #[arg(long, env = "VALKEY_RESULT_TTL", default_value = "300")]
    valkey_result_ttl: u64,

    /// Valkey hotpath TTL in seconds
    #[arg(long, env = "VALKEY_HOTPATH_TTL", default_value = "3600")]
    valkey_hotpath_ttl: u64,

    /// NATS fetch timeout in milliseconds
    #[arg(long, env = "FETCH_TIMEOUT_MS", default_value = "500")]
    fetch_timeout_ms: u64,

    /// Maximum messages per fetch batch
    #[arg(long, env = "MAX_MESSAGES", default_value = "100")]
    max_messages: usize,

    /// Prometheus metrics port
    #[arg(long, env = "METRICS_PORT", default_value = "9092")]
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
        workers = args.workers,
        max_queue_depth = args.max_queue_depth,
        storage_backend = %args.storage_backend,
        valkey_url = %redact_url(&args.valkey_url),
        metrics_port = args.metrics_port,
        "Starting RSFGA Precompute daemon"
    );

    // Initialize Prometheus metrics
    setup_metrics_server(args.metrics_port)?;

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

    // Connect to Valkey
    let valkey_config = ValkeyConfig {
        url: args.valkey_url.clone(),
        result_ttl_secs: args.valkey_result_ttl,
        hotpath_ttl_secs: args.valkey_hotpath_ttl,
    };
    let valkey_client = ValkeyClient::connect(valkey_config)
        .await
        .map_err(PrecomputeError::Valkey)?;
    let check_cache = Arc::new(CheckCache::new(valkey_client));

    // Create adapters bridging DataStore to domain traits.
    // The adapters use Arc<dyn DataStore> trait objects, which works because
    // create_storage returns a type-erased Arc<dyn DataStore>.
    let tuple_reader = Arc::new(StoreTupleReader::new(Arc::clone(&storage)));
    let model_reader = Arc::new(StoreModelReader::new(Arc::clone(&storage)));
    let model_id_provider = Arc::new(StoreModelIdProvider::new(Arc::clone(&storage)));

    // Create graph resolver with the concrete adapter types
    let resolver = Arc::new(GraphResolver::new(tuple_reader, model_reader));

    // Create worker pool
    let worker_pool = WorkerPool::new(
        args.workers,
        args.max_queue_depth,
        Arc::clone(&resolver),
        Arc::clone(&check_cache),
        model_id_provider,
    );

    // Create NATS pull consumer on RSFGA_EVENTS stream
    let js = nats_client.jetstream();
    let stream = js
        .get_stream(STREAM_EVENTS)
        .await
        .map_err(|e| PrecomputeError::Internal(format!("Failed to get events stream: {e}")))?;

    let consumer_config = PullConfig {
        durable_name: Some(args.consumer_name.clone()),
        name: Some(args.consumer_name.clone()),
        description: Some("RSFGA precompute event consumer".to_string()),
        filter_subject: format!("{SUBJECT_EVENTS_PREFIX}.*.committed"),
        ack_policy: AckPolicy::Explicit,
        ..Default::default()
    };

    let consumer = stream
        .get_or_create_consumer(&args.consumer_name, consumer_config)
        .await
        .map_err(|e| PrecomputeError::Internal(format!("Failed to create events consumer: {e}")))?;

    info!(
        consumer_name = %args.consumer_name,
        stream = STREAM_EVENTS,
        filter = %format!("{SUBJECT_EVENTS_PREFIX}.*.committed"),
        "Event consumer created"
    );

    // Set up graceful shutdown
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            info!("Received shutdown signal, initiating graceful shutdown");
            let _ = shutdown_tx.send(true);
        }
    });

    // Per-store relation dependency map, lazily populated on first event
    // for each store and rebuilt when model changes are detected.
    let relation_deps: RelationDeps = Arc::new(tokio::sync::RwLock::new(HashMap::new()));

    // Run consumer loop
    run_consumer_loop(
        consumer,
        &worker_pool,
        &check_cache,
        relation_deps,
        Arc::clone(&storage),
        shutdown_rx,
        Duration::from_millis(args.fetch_timeout_ms),
        args.max_messages,
    )
    .await?;

    info!("RSFGA Precompute daemon stopped");
    Ok(())
}

/// Main consumer loop: fetch events, classify, find affected checks, submit jobs.
#[allow(clippy::too_many_arguments)]
async fn run_consumer_loop(
    consumer: async_nats::jetstream::consumer::Consumer<PullConfig>,
    worker_pool: &WorkerPool,
    check_cache: &Arc<CheckCache>,
    relation_deps: RelationDeps,
    storage: Arc<dyn DataStore>,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
    fetch_timeout: Duration,
    max_messages: usize,
) -> Result<()> {
    loop {
        // Check for shutdown signal
        if *shutdown_rx.borrow() {
            info!("Shutdown signal received, stopping consumer loop");
            break;
        }

        // Fetch a batch of messages
        let mut messages = consumer
            .fetch()
            .max_messages(max_messages)
            .expires(fetch_timeout)
            .messages()
            .await
            .map_err(|e| PrecomputeError::Internal(format!("Failed to fetch messages: {e}")))?;

        while let Some(msg_result) = messages.next().await {
            // Check shutdown between messages
            if *shutdown_rx.borrow() {
                info!("Shutdown during message processing");
                break;
            }

            match msg_result {
                Ok(msg) => {
                    metrics::record_event_received("committed");

                    // Parse CommittedEvent from payload
                    let event: rsfga_nats::CommittedEvent =
                        match serde_json::from_slice(&msg.payload) {
                            Ok(event) => event,
                            Err(e) => {
                                warn!(
                                    error = %e,
                                    subject = %msg.subject,
                                    "Failed to parse CommittedEvent, skipping"
                                );
                                // Ack to avoid redelivery of unparseable messages
                                if let Err(ack_err) = msg.ack().await {
                                    error!(error = %ack_err, "Failed to ack unparseable message");
                                }
                                continue;
                            }
                        };

                    debug!(
                        store_id = %event.store_id,
                        sequence = event.sequence,
                        writes = event.writes.len(),
                        deletes = event.deletes.len(),
                        "Processing committed event"
                    );

                    // Classify the event into change types
                    let changes = classifier::classify(&event);

                    if !changes.is_empty() {
                        let store_id = &event.store_id;

                        // Build or rebuild per-store relation deps as needed:
                        // - First event for a store: lazy initialization
                        // - Model change: rebuild since relations may have changed
                        let has_model_change = changes
                            .iter()
                            .any(|c| matches!(c, classifier::ChangeType::ModelChange { .. }));

                        {
                            let needs_build = has_model_change || {
                                let deps_read = relation_deps.read().await;
                                !deps_read.contains_key(store_id)
                            };

                            if needs_build {
                                if let Some(type_refs) =
                                    rsfga_precompute::adapters::load_store_relation_refs(
                                        &*storage, store_id,
                                    )
                                    .await
                                {
                                    let store_deps =
                                        impact::build_relation_dependencies(&type_refs);
                                    let mut deps_write = relation_deps.write().await;
                                    deps_write.insert(store_id.clone(), store_deps);
                                    debug!(store_id, "Built relation dependencies for store");
                                }
                            }
                        }

                        // Look up this store's deps for impact analysis
                        let deps_read = relation_deps.read().await;
                        let empty_deps = HashMap::new();
                        let store_deps = deps_read.get(store_id).unwrap_or(&empty_deps);

                        match impact::find_affected_checks(&changes, check_cache, store_deps).await
                        {
                            Ok(jobs) => {
                                debug!(
                                    store_id = %event.store_id,
                                    job_count = jobs.len(),
                                    "Submitting recomputation jobs"
                                );

                                for job in jobs {
                                    if !worker_pool.submit(job) {
                                        warn!("Worker pool queue full, dropping job");
                                    }
                                }

                                // Update queue depth metric
                                metrics::update_queue_depth(worker_pool.queue_depth() as f64);
                            }
                            Err(e) => {
                                warn!(
                                    error = %e,
                                    store_id = %event.store_id,
                                    "Failed to find affected checks"
                                );
                            }
                        }
                    }

                    // Ack after processing
                    if let Err(ack_err) = msg.ack().await {
                        error!(error = %ack_err, "Failed to ack message");
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Error receiving message from NATS");
                }
            }
        }
    }

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
                    .map_err(|e| PrecomputeError::Internal(format!("RocksDB init failed: {e}")))?;
                Ok(Arc::new(storage))
            }
            #[cfg(not(feature = "rocksdb"))]
            {
                Err(PrecomputeError::Internal(
                    "RocksDB support not compiled in. Rebuild with --features rocksdb".to_string(),
                ))
            }
        }
        "postgres" => {
            let url = args.database_url.as_ref().ok_or_else(|| {
                PrecomputeError::Internal("DATABASE_URL required for postgres backend".to_string())
            })?;
            info!(url = %redact_url(url), "Using PostgreSQL storage backend");
            let config = rsfga_storage::PostgresConfig {
                database_url: url.clone(),
                ..Default::default()
            };
            let storage = rsfga_storage::PostgresDataStore::from_config(&config)
                .await
                .map_err(PrecomputeError::Storage)?;
            Ok(Arc::new(storage))
        }
        "mysql" => {
            let url = args.database_url.as_ref().ok_or_else(|| {
                PrecomputeError::Internal("DATABASE_URL required for mysql backend".to_string())
            })?;
            info!(url = %redact_url(url), "Using MySQL storage backend");
            let config = rsfga_storage::MySQLConfig {
                database_url: url.clone(),
                ..Default::default()
            };
            let storage = rsfga_storage::MySQLDataStore::from_config(&config)
                .await
                .map_err(PrecomputeError::Storage)?;
            Ok(Arc::new(storage))
        }
        other => Err(PrecomputeError::Internal(format!(
            "Unknown storage backend: {other}"
        ))),
    }
}

/// Set up Prometheus metrics exporter.
fn setup_metrics_server(port: u16) -> Result<()> {
    use metrics_exporter_prometheus::PrometheusBuilder;

    PrometheusBuilder::new()
        .with_http_listener(([0, 0, 0, 0], port))
        .install()
        .map_err(|e| PrecomputeError::Internal(format!("Failed to setup metrics: {e}")))?;

    info!(port = port, "Prometheus metrics server started");
    Ok(())
}

/// Redact credentials from URLs for safe logging.
fn redact_url(url_str: &str) -> String {
    match url::Url::parse(url_str) {
        Ok(mut url) => {
            if !url.password().unwrap_or_default().is_empty() {
                let _ = url.set_password(Some("REDACTED"));
            }
            url.to_string()
        }
        Err(_) => "[invalid URL]".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact_url_with_password() {
        let url = "redis://user:secret@localhost:6379";
        let redacted = redact_url(url);
        assert!(redacted.contains("REDACTED"));
        assert!(!redacted.contains("secret"));
    }

    #[test]
    fn test_redact_url_without_password() {
        let url = "redis://localhost:6379";
        let redacted = redact_url(url);
        assert!(redacted.starts_with("redis://localhost:6379"));
        assert!(!redacted.contains("REDACTED"));
    }

    #[test]
    fn test_redact_url_nats() {
        let url = "nats://admin:password@localhost:4222";
        let redacted = redact_url(url);
        assert!(redacted.contains("REDACTED"));
        assert!(!redacted.contains("password"));
    }

    #[test]
    fn test_redact_url_invalid() {
        let url = "not a url";
        let redacted = redact_url(url);
        assert_eq!(redacted, "[invalid URL]");
    }
}
