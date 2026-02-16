use thiserror::Error;

#[derive(Debug, Error)]
pub enum ValkeyError {
    #[error("Valkey connection error: {0}")]
    Connection(#[from] redis::RedisError),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Valkey unavailable")]
    Unavailable,
}

pub type Result<T> = std::result::Result<T, ValkeyError>;
