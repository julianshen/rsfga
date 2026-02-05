//! # rsfga-nats
//!
//! NATS JetStream integration for RSFGA async writes.
//!
//! This crate provides the core infrastructure for NATS-first write architecture:
//!
//! - **Publisher**: Publishes write requests to NATS JetStream
//! - **Consumer**: Consumes write requests and batches them to storage
//! - **Events**: Committed event publication for edge sync and cache invalidation
//! - **RYOW**: Read-your-own-writes consistency support via write tickets
//!
//! ## Architecture
//!
//! ```text
//! Client ──▶ NATS JetStream ──▶ Storage Consumer ──▶ Database
//!            (fast, durable)     (batched writes)
//!                  │
//!                  ├──▶ Edge Sync Consumer
//!                  ├──▶ Cache Invalidation Consumer
//!                  └──▶ Audit/Analytics Consumer
//! ```
//!
//! ## Streams
//!
//! - `RSFGA_WRITES`: WorkQueue retention for write requests
//! - `RSFGA_EVENTS`: Limits retention for committed events (edge sync, audit)
//!
//! ## Example
//!
//! ```rust,no_run
//! use rsfga_nats::{NatsConfig, NatsClient, JetStreamManager};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Create NATS client
//! let config = NatsConfig::default();
//! let client = NatsClient::connect(config).await?;
//!
//! // Setup JetStream streams using JetStreamManager
//! let manager = JetStreamManager::new(client);
//! manager.setup_streams().await?;
//! # Ok(())
//! # }
//! ```

pub mod config;
pub mod connection;
pub mod error;
pub mod events;
pub mod jetstream;
pub mod publisher;

// Re-export main types
pub use config::NatsConfig;
pub use connection::NatsClient;
pub use error::{NatsError, Result};
pub use events::{
    CommittedEvent, TupleCondition, TupleKey, TupleOperation, WriteRequest, WriteTicket,
};
pub use jetstream::JetStreamManager;
pub use publisher::{EventPublisher, PublisherConfig, PublisherStats};

/// Stream name for write requests (WorkQueue retention)
pub const STREAM_WRITES: &str = "RSFGA_WRITES";

/// Stream name for committed events (Limits retention)
pub const STREAM_EVENTS: &str = "RSFGA_EVENTS";

/// Subject prefix for write requests
pub const SUBJECT_WRITES_PREFIX: &str = "rsfga.writes";

/// Subject prefix for committed events
pub const SUBJECT_EVENTS_PREFIX: &str = "rsfga.events";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_constants() {
        assert_eq!(STREAM_WRITES, "RSFGA_WRITES");
        assert_eq!(STREAM_EVENTS, "RSFGA_EVENTS");
        assert_eq!(SUBJECT_WRITES_PREFIX, "rsfga.writes");
        assert_eq!(SUBJECT_EVENTS_PREFIX, "rsfga.events");
    }
}
