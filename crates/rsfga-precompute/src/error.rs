use thiserror::Error;

#[derive(Debug, Error)]
pub enum PrecomputeError {
    #[error("NATS error: {0}")]
    Nats(#[from] rsfga_nats::NatsError),

    #[error("Valkey error: {0}")]
    Valkey(#[from] rsfga_valkey::ValkeyError),

    #[error("Storage error: {0}")]
    Storage(#[from] rsfga_storage::StorageError),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("{0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, PrecomputeError>;
