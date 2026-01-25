//! RSFGA Server Binary
//!
//! High-performance OpenFGA-compatible authorization server.
//!
//! # Usage
//!
//! ```bash
//! # With config file
//! rsfga --config config.yaml
//!
//! # With environment variables only
//! RSFGA_STORAGE__BACKEND=memory rsfga
//! ```

use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use tokio::signal;
use tokio::sync::broadcast;
use tracing::{error, info, Level};

use rsfga_api::grpc::{run_grpc_server_with_shutdown, GrpcServerConfig};
use rsfga_api::http::{create_router_with_observability, AppState};
use rsfga_api::observability::{init_metrics, init_observability, LoggingConfig, TracingConfig};
use rsfga_server::ServerConfig;
use rsfga_storage::{
    DataStore, MemoryDataStore, MySQLConfig, MySQLDataStore, PostgresConfig, PostgresDataStore,
};

/// RSFGA - High-Performance OpenFGA-Compatible Authorization Server
#[derive(Parser, Debug)]
#[command(name = "rsfga")]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to configuration file (YAML)
    #[arg(short, long)]
    config: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Load configuration
    let config = if let Some(config_path) = args.config {
        ServerConfig::load(&config_path)?
    } else {
        ServerConfig::from_env()?
    };

    // Initialize observability (logging + tracing)
    let log_config = LoggingConfig {
        json_format: config.logging.json,
        default_level: parse_log_level(&config.logging.level),
        include_spans: false,
    };

    let trace_config = if config.tracing.enabled {
        Some(TracingConfig {
            enabled: true,
            service_name: config.tracing.service_name.clone(),
            jaeger_endpoint: config.tracing.jaeger_endpoint.clone(),
        })
    } else {
        None
    };

    init_observability(log_config, trace_config)?;

    info!(version = env!("CARGO_PKG_VERSION"), "Starting RSFGA server");

    // Initialize metrics (always enabled - config.metrics.enabled reserved for future use)
    // Note: metrics.path is currently hardcoded to /metrics in the router
    let metrics_state = init_metrics()?;
    if config.metrics.enabled {
        info!("Metrics enabled at /metrics");
    }

    // Create storage backend based on configuration
    let http_addr: SocketAddr = format!("{}:{}", config.server.host, config.server.port).parse()?;
    let grpc_addr: SocketAddr = format!("{}:{}", config.server.host, config.grpc.port).parse()?;

    match config.storage.backend.as_str() {
        "memory" => {
            info!("Using in-memory storage backend");
            let storage = Arc::new(MemoryDataStore::new());
            run_servers(storage, http_addr, grpc_addr, &config, metrics_state).await
        }
        "postgres" | "cockroachdb" => {
            let database_url = config.storage.database_url.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "storage.database_url is required for {} backend",
                    config.storage.backend
                )
            })?;

            let backend_name = if config.storage.backend == "cockroachdb" {
                "CockroachDB"
            } else {
                "PostgreSQL"
            };

            info!("Connecting to {} database", backend_name);
            let pg_config = PostgresConfig {
                database_url: database_url.clone(),
                max_connections: config.storage.pool_size,
                min_connections: 1,
                connect_timeout_secs: config.storage.connection_timeout_secs,
                ..Default::default()
            };

            let storage = PostgresDataStore::from_config(&pg_config).await?;
            info!("{} connection established", backend_name);

            // Run database migrations
            info!("Running database migrations");
            storage.run_migrations().await?;
            info!("Database migrations complete");

            let storage = Arc::new(storage);
            run_servers(storage, http_addr, grpc_addr, &config, metrics_state).await
        }
        "mysql" => {
            let database_url = config.storage.database_url.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "storage.database_url is required for {} backend",
                    config.storage.backend
                )
            })?;

            info!("Connecting to MySQL database");
            let mysql_config = MySQLConfig {
                database_url: database_url.clone(),
                max_connections: config.storage.pool_size,
                min_connections: 1,
                connect_timeout_secs: config.storage.connection_timeout_secs,
                ..Default::default()
            };

            let storage = MySQLDataStore::from_config(&mysql_config).await?;
            info!("MySQL connection established");

            // Run database migrations
            info!("Running database migrations");
            storage.run_migrations().await?;
            info!("Database migrations complete");

            let storage = Arc::new(storage);
            run_servers(storage, http_addr, grpc_addr, &config, metrics_state).await
        }
        _ => {
            error!("Unknown storage backend: {}", config.storage.backend);
            anyhow::bail!("Unknown storage backend: {}", config.storage.backend);
        }
    }
}

/// Run both HTTP and gRPC servers with shared storage.
///
/// Uses `tokio::select!` to race server futures against shutdown signal,
/// propagating errors immediately if a server fails to start or encounters an error.
async fn run_servers<S>(
    storage: Arc<S>,
    http_addr: SocketAddr,
    grpc_addr: SocketAddr,
    config: &ServerConfig,
    metrics_state: rsfga_api::observability::MetricsState,
) -> anyhow::Result<()>
where
    S: DataStore + Send + Sync + 'static,
{
    // Create shutdown broadcast channel
    let (shutdown_tx, _) = broadcast::channel::<()>(1);

    // Prepare HTTP server future
    let http_storage = Arc::clone(&storage);
    let http_shutdown_rx = shutdown_tx.subscribe();
    let http_future = async move {
        let state = AppState::new(http_storage);
        let router = create_router_with_observability(state, metrics_state);
        run_http_server(router, http_addr, http_shutdown_rx).await
    };

    // Prepare gRPC server future if enabled
    let grpc_enabled = config.grpc.enabled;
    let grpc_future = if grpc_enabled {
        let grpc_storage = Arc::clone(&storage);
        let grpc_config = GrpcServerConfig {
            reflection_enabled: config.grpc.reflection,
            health_check_enabled: config.grpc.health_check,
        };
        let grpc_shutdown_rx = shutdown_tx.subscribe();

        info!(%grpc_addr, "gRPC server enabled");

        Some(async move {
            let shutdown_future = async move {
                let mut rx = grpc_shutdown_rx;
                let _ = rx.recv().await;
            };
            run_grpc_server_with_shutdown(grpc_storage, grpc_addr, grpc_config, shutdown_future)
                .await
                .map_err(|e| anyhow::anyhow!("gRPC server error: {}", e))
        })
    } else {
        info!("gRPC server disabled");
        None
    };

    // Use select! to race servers against shutdown signal, propagating errors immediately
    let result = if let Some(grpc_fut) = grpc_future {
        tokio::select! {
            result = http_future => {
                // HTTP server completed (either error or shutdown)
                if let Err(ref e) = result {
                    error!("HTTP server error: {}", e);
                }
                // Signal gRPC to shutdown
                let _ = shutdown_tx.send(());
                result
            }
            result = grpc_fut => {
                // gRPC server completed (either error or shutdown)
                if let Err(ref e) = result {
                    error!("gRPC server error: {}", e);
                }
                // Signal HTTP to shutdown
                let _ = shutdown_tx.send(());
                result
            }
            _ = shutdown_signal() => {
                // Shutdown signal received
                info!("Shutdown signal received, stopping servers");
                let _ = shutdown_tx.send(());
                Ok(())
            }
        }
    } else {
        // Only HTTP server
        tokio::select! {
            result = http_future => {
                if let Err(ref e) = result {
                    error!("HTTP server error: {}", e);
                }
                result
            }
            _ = shutdown_signal() => {
                info!("Shutdown signal received, stopping servers");
                let _ = shutdown_tx.send(());
                Ok(())
            }
        }
    };

    info!("All servers shutdown complete");
    result
}

/// Run the HTTP server with graceful shutdown.
async fn run_http_server(
    router: axum::Router,
    addr: SocketAddr,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> anyhow::Result<()> {
    info!(%addr, "HTTP server listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;

    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.recv().await;
            info!("HTTP server received shutdown signal");
        })
        .await?;

    info!("HTTP server shutdown complete");
    Ok(())
}

/// Wait for shutdown signal (Ctrl+C or SIGTERM).
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            info!("Received Ctrl+C, initiating graceful shutdown");
        }
        _ = terminate => {
            info!("Received SIGTERM, initiating graceful shutdown");
        }
    }
}

/// Parse log level from string.
fn parse_log_level(level: &str) -> Level {
    match level.to_lowercase().as_str() {
        "trace" => Level::TRACE,
        "debug" => Level::DEBUG,
        "info" => Level::INFO,
        "warn" => Level::WARN,
        "error" => Level::ERROR,
        _ => Level::INFO,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_log_level() {
        assert_eq!(parse_log_level("trace"), Level::TRACE);
        assert_eq!(parse_log_level("DEBUG"), Level::DEBUG);
        assert_eq!(parse_log_level("Info"), Level::INFO);
        assert_eq!(parse_log_level("WARN"), Level::WARN);
        assert_eq!(parse_log_level("error"), Level::ERROR);
        assert_eq!(parse_log_level("unknown"), Level::INFO);
    }

    #[test]
    fn test_cli_args_parsing() {
        // Test with no args
        let args = Args::try_parse_from(["rsfga"]).unwrap();
        assert!(args.config.is_none());

        // Test with config
        let args = Args::try_parse_from(["rsfga", "--config", "config.yaml"]).unwrap();
        assert_eq!(args.config, Some("config.yaml".to_string()));

        // Test with short flag
        let args = Args::try_parse_from(["rsfga", "-c", "test.yaml"]).unwrap();
        assert_eq!(args.config, Some("test.yaml".to_string()));
    }
}
