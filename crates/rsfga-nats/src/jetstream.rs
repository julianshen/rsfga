//! JetStream setup and management.
//!
//! This module handles creation and configuration of JetStream streams:
//!
//! - `RSFGA_WRITES`: WorkQueue retention for write requests
//! - `RSFGA_EVENTS`: Limits retention for committed events

use crate::config::{EventsStreamConfig, StorageType, StreamConfig};
use crate::connection::NatsClient;
use crate::error::{NatsError, Result};
use crate::{STREAM_EVENTS, STREAM_WRITES, SUBJECT_EVENTS_PREFIX, SUBJECT_WRITES_PREFIX};
use async_nats::jetstream::stream::{
    Config as StreamConfigNats, RetentionPolicy, StorageType as NatsStorageType,
};
use tracing::{info, warn};

/// JetStream manager for creating and configuring streams.
pub struct JetStreamManager {
    client: NatsClient,
}

impl JetStreamManager {
    /// Create a new JetStream manager.
    pub fn new(client: NatsClient) -> Self {
        Self { client }
    }

    /// Setup all RSFGA streams.
    ///
    /// This is idempotent - existing streams will be updated if configuration differs.
    pub async fn setup_streams(&self) -> Result<()> {
        let config = &self.client.config().jetstream;

        info!("Setting up JetStream streams");

        // Create RSFGA_WRITES stream
        self.setup_writes_stream(&config.writes_stream, config.dedup_window)
            .await?;

        // Create RSFGA_EVENTS stream
        self.setup_events_stream(&config.events_stream).await?;

        info!("JetStream streams setup complete");
        Ok(())
    }

    /// Setup the RSFGA_WRITES stream.
    ///
    /// This stream uses WorkQueue retention - messages are deleted after
    /// being acknowledged by a consumer.
    async fn setup_writes_stream(
        &self,
        config: &StreamConfig,
        dedup_window: std::time::Duration,
    ) -> Result<()> {
        let js = self.client.jetstream();

        let stream_config = StreamConfigNats {
            name: STREAM_WRITES.to_string(),
            description: Some("RSFGA async write requests".to_string()),
            subjects: vec![format!("{}.*", SUBJECT_WRITES_PREFIX)],
            retention: RetentionPolicy::WorkQueue,
            max_consumers: -1,
            max_messages: config.max_messages,
            max_bytes: config.max_bytes,
            max_age: config.max_age,
            max_message_size: config.max_message_size,
            storage: convert_storage_type(config.storage),
            num_replicas: config.replicas as usize,
            duplicate_window: dedup_window,
            ..Default::default()
        };

        match js.get_or_create_stream(stream_config.clone()).await {
            Ok(mut stream) => {
                let info = stream.info().await.map_err(|e| NatsError::Stream {
                    stream: STREAM_WRITES.to_string(),
                    reason: e.to_string(),
                })?;
                info!(
                    stream = %STREAM_WRITES,
                    messages = info.state.messages,
                    bytes = info.state.bytes,
                    consumers = info.state.consumer_count,
                    "Stream ready"
                );
                Ok(())
            }
            Err(e) => {
                warn!(stream = %STREAM_WRITES, error = %e, "Failed to create stream, attempting update");
                // Stream might exist with different config, try to update it
                // First verify stream exists
                js.get_stream(STREAM_WRITES)
                    .await
                    .map_err(|get_err| NatsError::Stream {
                        stream: STREAM_WRITES.to_string(),
                        reason: format!("create failed: {}, get failed: {}", e, get_err),
                    })?;

                // Attempt to update stream configuration via context
                // Note: Some fields cannot be updated after creation (e.g., storage type)
                match js.update_stream(&stream_config).await {
                    Ok(info) => {
                        info!(
                            stream = %STREAM_WRITES,
                            messages = info.state.messages,
                            "Stream configuration updated"
                        );
                    }
                    Err(update_err) => {
                        warn!(
                            stream = %STREAM_WRITES,
                            error = %update_err,
                            "Failed to update stream config, using existing configuration"
                        );
                    }
                }
                Ok(())
            }
        }
    }

    /// Setup the RSFGA_EVENTS stream.
    ///
    /// This stream uses Limits retention - messages are kept until
    /// the configured max_age or max_bytes limit is reached.
    async fn setup_events_stream(&self, config: &EventsStreamConfig) -> Result<()> {
        let js = self.client.jetstream();

        let stream_config = StreamConfigNats {
            name: STREAM_EVENTS.to_string(),
            description: Some("RSFGA committed events for edge sync and audit".to_string()),
            subjects: vec![format!("{}.>", SUBJECT_EVENTS_PREFIX)],
            retention: RetentionPolicy::Limits,
            max_consumers: -1,
            max_messages: config.max_messages,
            max_bytes: config.max_bytes,
            max_age: config.max_age,
            max_message_size: config.max_message_size,
            storage: convert_storage_type(config.storage),
            num_replicas: config.replicas as usize,
            ..Default::default()
        };

        match js.get_or_create_stream(stream_config.clone()).await {
            Ok(mut stream) => {
                let info = stream.info().await.map_err(|e| NatsError::Stream {
                    stream: STREAM_EVENTS.to_string(),
                    reason: e.to_string(),
                })?;
                info!(
                    stream = %STREAM_EVENTS,
                    messages = info.state.messages,
                    bytes = info.state.bytes,
                    consumers = info.state.consumer_count,
                    "Stream ready"
                );
                Ok(())
            }
            Err(e) => {
                warn!(stream = %STREAM_EVENTS, error = %e, "Failed to create stream, attempting update");
                // Stream might exist with different config, try to update it
                // First verify stream exists
                js.get_stream(STREAM_EVENTS)
                    .await
                    .map_err(|get_err| NatsError::Stream {
                        stream: STREAM_EVENTS.to_string(),
                        reason: format!("create failed: {}, get failed: {}", e, get_err),
                    })?;

                // Attempt to update stream configuration via context
                // Note: Some fields cannot be updated after creation (e.g., storage type)
                match js.update_stream(&stream_config).await {
                    Ok(info) => {
                        info!(
                            stream = %STREAM_EVENTS,
                            messages = info.state.messages,
                            "Stream configuration updated"
                        );
                    }
                    Err(update_err) => {
                        warn!(
                            stream = %STREAM_EVENTS,
                            error = %update_err,
                            "Failed to update stream config, using existing configuration"
                        );
                    }
                }
                Ok(())
            }
        }
    }

    /// Get information about a stream.
    pub async fn stream_info(&self, stream_name: &str) -> Result<StreamInfo> {
        let js = self.client.jetstream();
        let mut stream = js
            .get_stream(stream_name)
            .await
            .map_err(|e| NatsError::Stream {
                stream: stream_name.to_string(),
                reason: e.to_string(),
            })?;

        let info = stream.info().await.map_err(|e| NatsError::Stream {
            stream: stream_name.to_string(),
            reason: e.to_string(),
        })?;

        Ok(StreamInfo {
            name: info.config.name.clone(),
            messages: info.state.messages,
            bytes: info.state.bytes,
            consumer_count: info.state.consumer_count,
            first_sequence: info.state.first_sequence,
            last_sequence: info.state.last_sequence,
        })
    }

    /// Delete a stream (USE WITH CAUTION).
    pub async fn delete_stream(&self, stream_name: &str) -> Result<()> {
        let js = self.client.jetstream();
        js.delete_stream(stream_name)
            .await
            .map_err(|e| NatsError::Stream {
                stream: stream_name.to_string(),
                reason: e.to_string(),
            })?;
        info!(stream = %stream_name, "Stream deleted");
        Ok(())
    }

    /// Purge all messages from a stream.
    pub async fn purge_stream(&self, stream_name: &str) -> Result<u64> {
        let js = self.client.jetstream();
        let stream = js
            .get_stream(stream_name)
            .await
            .map_err(|e| NatsError::Stream {
                stream: stream_name.to_string(),
                reason: e.to_string(),
            })?;

        let purge_response = stream.purge().await.map_err(|e| NatsError::Stream {
            stream: stream_name.to_string(),
            reason: e.to_string(),
        })?;

        info!(
            stream = %stream_name,
            purged = purge_response.purged,
            "Stream purged"
        );
        Ok(purge_response.purged)
    }
}

/// Stream information.
#[derive(Debug, Clone)]
pub struct StreamInfo {
    /// Stream name
    pub name: String,
    /// Number of messages in the stream
    pub messages: u64,
    /// Total bytes in the stream
    pub bytes: u64,
    /// Number of consumers
    pub consumer_count: usize,
    /// First sequence number
    pub first_sequence: u64,
    /// Last sequence number
    pub last_sequence: u64,
}

/// Convert storage type from config to NATS type.
fn convert_storage_type(storage: StorageType) -> NatsStorageType {
    match storage {
        StorageType::File => NatsStorageType::File,
        StorageType::Memory => NatsStorageType::Memory,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_storage_type() {
        assert!(matches!(
            convert_storage_type(StorageType::File),
            NatsStorageType::File
        ));
        assert!(matches!(
            convert_storage_type(StorageType::Memory),
            NatsStorageType::Memory
        ));
    }

    #[test]
    fn test_stream_info() {
        let info = StreamInfo {
            name: "TEST".to_string(),
            messages: 100,
            bytes: 1024,
            consumer_count: 2,
            first_sequence: 1,
            last_sequence: 100,
        };

        assert_eq!(info.name, "TEST");
        assert_eq!(info.messages, 100);
        assert_eq!(info.consumer_count, 2);
    }
}
