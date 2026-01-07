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

mod service;

pub use service::OpenFgaGrpcService;

// Re-export the generated server trait for service registration
pub use crate::proto::openfga::v1::open_fga_service_server::OpenFgaServiceServer;

#[cfg(test)]
mod tests;
