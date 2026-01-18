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

use rsfga_api::grpc::{OpenFgaGrpcService, OpenFgaServiceServer};
use rsfga_api::http::{create_router_with_observability, AppState};
use rsfga_api::observability::{init_metrics, init_observability, LoggingConfig, TracingConfig};
use rsfga_server::ServerConfig;
use rsfga_storage::{
    DataStore, MemoryDataStore, MySQLConfig, MySQLDataStore, PostgresConfig, PostgresDataStore,
};
use tonic::transport::Server;

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
    let addr: SocketAddr = format!("{}:{}", config.server.host, config.server.port).parse()?;

    match config.storage.backend.as_str() {
        "memory" => {
            info!("Using in-memory storage backend");
            let storage = Arc::new(MemoryDataStore::new());
            let state = AppState::new(storage.clone());
            let router = create_router_with_observability(state, metrics_state);
            run_server(router, addr, Some((config.server.host, config.server.grpc_port)), storage).await
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

            let state = AppState::new(storage.clone());
            let router = create_router_with_observability(state, metrics_state);
            run_server(router, addr, Some((config.server.host, config.server.grpc_port)), storage).await
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

            let state = AppState::new(storage.clone());
            let router = create_router_with_observability(state, metrics_state);
            run_server(router, addr, Some((config.server.host, config.server.grpc_port)), storage).await
        }
        _ => {
            error!("Unknown storage backend: {}", config.storage.backend);
            anyhow::bail!("Unknown storage backend: {}", config.storage.backend);
        }
    }
}

/// Run the HTTP and gRPC servers with graceful shutdown.
async fn run_server<S: DataStore>(
    router: axum::Router,
    addr: SocketAddr,
    grpc_config: Option<(String, u16)>,
    storage: Arc<S>,
) -> anyhow::Result<()> {
    info!(%addr, "HTTP Server listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;

    // Spawn gRPC server if configured
    if let Some((host, port)) = grpc_config {
        let grpc_addr: SocketAddr = format!("{}:{}", host, port).parse()?;
        let grpc_service = OpenFgaGrpcService::new(storage);

        // Enable gRPC Reflection
        let reflection_service = tonic_reflection::server::Builder::configure()
            .register_encoded_file_descriptor_set(
                rsfga_api::proto::openfga::v1::FILE_DESCRIPTOR_SET,
            )
            .register_encoded_file_descriptor_set(tonic_health::pb::FILE_DESCRIPTOR_SET)
            .build()
            .expect("Failed to build gRPC reflection service");

        // Enable gRPC Health
        let (mut health_reporter, health_service) = tonic_health::server::health_reporter();
        health_reporter
            .set_serving::<OpenFgaServiceServer<OpenFgaGrpcService<S>>>()
            .await;

        info!(addr = %grpc_addr, "gRPC Server listening");

        tokio::spawn(async move {
            if let Err(e) = Server::builder()
                .add_service(reflection_service)
                .add_service(health_service)
                .add_service(OpenFgaServiceServer::new(grpc_service))
                .serve(grpc_addr)
                .await
            {
                error!("gRPC server error: {}", e);
            }
        });
    }

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
