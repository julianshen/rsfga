//! gRPC service implementations.
//!
//! Implements OpenFGA-compatible gRPC services using Tonic.
//!
//! # Architecture
//!
//! The gRPC layer provides a Tonic-based service implementing the OpenFGA
//! service interface. The service delegates to the storage layer for data
//! operations.
//!
//! ```text
//! gRPC Request → OpenFgaGrpcService → Storage → Response
//! ```
//!
//! # Server Setup
//!
//! Use the `server` module to start the gRPC transport layer:
//!
//! ```ignore
//! use rsfga_api::grpc::server::{run_grpc_server, GrpcServerConfig};
//!
//! let config = GrpcServerConfig::default();
//! run_grpc_server(storage, addr, config).await?;
//! ```

pub mod conversion;
pub mod server;
mod service;

pub use server::{run_grpc_server, run_grpc_server_with_shutdown, GrpcServerConfig};
pub use service::OpenFgaGrpcService;

// Re-export the generated server trait for service registration
pub use crate::proto::openfga::v1::open_fga_service_server::OpenFgaServiceServer;

#[cfg(test)]
mod tests;
