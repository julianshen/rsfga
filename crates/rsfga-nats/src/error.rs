//! Error types for NATS integration.

use thiserror::Error;

/// Result type for NATS operations.
pub type Result<T> = std::result::Result<T, NatsError>;

/// Errors that can occur during NATS operations.
#[derive(Debug, Error)]
pub enum NatsError {
    /// Failed to connect to NATS server.
    #[error("connection error: {0}")]
    Connection(String),

    /// Failed to reconnect after disconnection.
    #[error("reconnection failed after {attempts} attempts: {reason}")]
    ReconnectionFailed { attempts: u32, reason: String },

    /// Authentication failed.
    #[error("authentication failed: {0}")]
    Authentication(String),

    /// TLS configuration error.
    #[error("TLS error: {0}")]
    Tls(String),

    /// JetStream operation failed.
    #[error("JetStream error: {0}")]
    JetStream(String),

    /// Stream creation or configuration failed.
    #[error("stream error: {stream}: {reason}")]
    Stream { stream: String, reason: String },

    /// Consumer creation or operation failed.
    #[error("consumer error: {consumer}: {reason}")]
    Consumer { consumer: String, reason: String },

    /// Failed to publish message.
    #[error("publish error: {subject}: {reason}")]
    Publish { subject: String, reason: String },

    /// Message acknowledgment failed.
    #[error("ack error: {0}")]
    Ack(String),

    /// Serialization or deserialization failed.
    #[error("serialization error: {0}")]
    Serialization(String),

    /// Timeout waiting for operation.
    #[error("timeout: {operation} after {duration_ms}ms")]
    Timeout { operation: String, duration_ms: u64 },

    /// Invalid configuration.
    #[error("configuration error: {0}")]
    Config(String),

    /// Circuit breaker is open.
    #[error("circuit breaker open: NATS unavailable")]
    CircuitBreakerOpen,

    /// Write ticket not found or expired.
    #[error("write ticket not found: {ticket_id}")]
    WriteTicketNotFound { ticket_id: String },

    /// Write not yet committed (for RYOW).
    #[error("write not yet committed: sequence {sequence} for store {store_id}")]
    WriteNotCommitted { store_id: String, sequence: u64 },

    /// Internal error.
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<async_nats::Error> for NatsError {
    fn from(err: async_nats::Error) -> Self {
        NatsError::Connection(err.to_string())
    }
}

impl From<async_nats::ConnectError> for NatsError {
    fn from(err: async_nats::ConnectError) -> Self {
        NatsError::Connection(err.to_string())
    }
}

impl From<async_nats::jetstream::context::CreateStreamError> for NatsError {
    fn from(err: async_nats::jetstream::context::CreateStreamError) -> Self {
        NatsError::Stream {
            stream: "unknown".to_string(),
            reason: err.to_string(),
        }
    }
}

impl From<async_nats::jetstream::stream::ConsumerError> for NatsError {
    fn from(err: async_nats::jetstream::stream::ConsumerError) -> Self {
        NatsError::Consumer {
            consumer: "unknown".to_string(),
            reason: err.to_string(),
        }
    }
}

impl From<serde_json::Error> for NatsError {
    fn from(err: serde_json::Error) -> Self {
        NatsError::Serialization(err.to_string())
    }
}

impl From<prost::DecodeError> for NatsError {
    fn from(err: prost::DecodeError) -> Self {
        NatsError::Serialization(err.to_string())
    }
}

impl From<prost::EncodeError> for NatsError {
    fn from(err: prost::EncodeError) -> Self {
        NatsError::Serialization(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = NatsError::Connection("failed to connect".to_string());
        assert_eq!(err.to_string(), "connection error: failed to connect");

        let err = NatsError::ReconnectionFailed {
            attempts: 5,
            reason: "server unavailable".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "reconnection failed after 5 attempts: server unavailable"
        );

        let err = NatsError::Stream {
            stream: "RSFGA_WRITES".to_string(),
            reason: "already exists".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "stream error: RSFGA_WRITES: already exists"
        );

        let err = NatsError::Timeout {
            operation: "publish".to_string(),
            duration_ms: 5000,
        };
        assert_eq!(err.to_string(), "timeout: publish after 5000ms");
    }

    #[test]
    fn test_write_ticket_errors() {
        let err = NatsError::WriteTicketNotFound {
            ticket_id: "abc123".to_string(),
        };
        assert_eq!(err.to_string(), "write ticket not found: abc123");

        let err = NatsError::WriteNotCommitted {
            store_id: "store-1".to_string(),
            sequence: 42,
        };
        assert_eq!(
            err.to_string(),
            "write not yet committed: sequence 42 for store store-1"
        );
    }
}
