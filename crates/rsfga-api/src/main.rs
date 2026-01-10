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
use tracing::{error, info, Level};

use rsfga_api::http::{create_router_with_observability, AppState};
use rsfga_api::observability::{init_metrics, init_observability, LoggingConfig, TracingConfig};
use rsfga_server::ServerConfig;
use rsfga_storage::{MemoryDataStore, PostgresConfig, PostgresDataStore};

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

    // Initialize metrics
    let metrics_state = if config.metrics.enabled {
        info!("Metrics enabled at {}", config.metrics.path);
        init_metrics()?
    } else {
        init_metrics()?
    };

    // Create storage backend based on configuration
    let addr: SocketAddr = format!("{}:{}", config.server.host, config.server.port).parse()?;

    match config.storage.backend.as_str() {
        "memory" => {
            info!("Using in-memory storage backend");
            let storage = Arc::new(MemoryDataStore::new());
            let state = AppState { storage };
            let router = create_router_with_observability(state, metrics_state);
            run_server(router, addr).await
        }
        "postgres" => {
            let database_url = config
                .storage
                .database_url
                .as_ref()
                .expect("database_url required for postgres backend");

            info!("Connecting to PostgreSQL database");
            let pg_config = PostgresConfig {
                database_url: database_url.clone(),
                max_connections: config.storage.pool_size,
                min_connections: 1,
                connect_timeout_secs: config.storage.connection_timeout_secs,
            };

            let storage = PostgresDataStore::from_config(&pg_config).await?;
            info!("PostgreSQL connection established");

            // Run database migrations
            info!("Running database migrations");
            storage.run_migrations().await?;
            info!("Database migrations complete");

            let storage = Arc::new(storage);

            let state = AppState { storage };
            let router = create_router_with_observability(state, metrics_state);
            run_server(router, addr).await
        }
        _ => {
            error!("Unknown storage backend: {}", config.storage.backend);
            anyhow::bail!("Unknown storage backend: {}", config.storage.backend);
        }
    }
}

/// Run the HTTP server with graceful shutdown.
async fn run_server(router: axum::Router, addr: SocketAddr) -> anyhow::Result<()> {
    info!(%addr, "Server listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("Server shutdown complete");
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
