//! rsfga-api: HTTP and gRPC API layer
//!
//! This crate provides the API layer including:
//! - HTTP REST endpoints via Axum
//! - gRPC services via Tonic
//! - Middleware (auth, metrics, tracing)
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │                 rsfga-api                    │
//! ├─────────────────────────────────────────────┤
//! │  http/       - HTTP REST endpoints          │
//! │  grpc/       - gRPC service implementations │
//! │  middleware/ - Auth, metrics, tracing       │
//! └─────────────────────────────────────────────┘
//! ```

pub mod grpc;
pub mod http;
pub mod middleware;
