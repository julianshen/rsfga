//! Error types for the RSFGA Writer daemon.

use thiserror::Error;

/// Writer daemon errors.
#[derive(Debug, Error)]
pub enum WriterError {
    /// NATS connection/operation error.
    #[error("NATS error: {0}")]
    Nats(#[from] rsfga_nats::NatsError),

    /// Storage error.
    #[error("Storage error: {0}")]
    Storage(#[from] rsfga_storage::StorageError),

    /// Configuration error.
    #[error("Configuration error: {0}")]
    Config(String),

    /// Serialization error.
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Consumer error.
    #[error("Consumer error: {0}")]
    Consumer(String),

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result type for writer operations.
pub type Result<T> = std::result::Result<T, WriterError>;
