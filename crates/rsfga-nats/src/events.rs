//! Event schemas for NATS messaging.
//!
//! This module defines the event types used for RSFGA async writes:
//!
//! - `WriteRequest`: Published to RSFGA_WRITES stream for async write requests
//! - `CommittedEvent`: Published to RSFGA_EVENTS stream after storage commit
//!
//! ## Wire Format
//!
//! Events are serialized as JSON for ease of debugging and interoperability.
//! A future version may add protobuf support for better performance.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Write request event published to RSFGA_WRITES stream.
///
/// This event represents an async write operation that will be consumed
/// and batched by the storage consumer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteRequest {
    /// Unique request ID (ULID for ordering)
    pub request_id: String,

    /// Store ID
    pub store_id: String,

    /// Authorization model ID to use for validation
    pub authorization_model_id: Option<String>,

    /// Tuples to write (add)
    #[serde(default)]
    pub writes: Vec<TupleOperation>,

    /// Tuples to delete
    #[serde(default)]
    pub deletes: Vec<TupleKey>,

    /// Request metadata
    #[serde(default)]
    pub metadata: RequestMetadata,

    /// Timestamp when the request was created
    pub created_at: DateTime<Utc>,
}

impl WriteRequest {
    /// Create a new write request with a generated request ID.
    pub fn new(store_id: impl Into<String>) -> Self {
        Self {
            request_id: ulid::Ulid::new().to_string(),
            store_id: store_id.into(),
            authorization_model_id: None,
            writes: Vec::new(),
            deletes: Vec::new(),
            metadata: RequestMetadata::default(),
            created_at: Utc::now(),
        }
    }

    /// Set the authorization model ID.
    pub fn with_model_id(mut self, model_id: impl Into<String>) -> Self {
        self.authorization_model_id = Some(model_id.into());
        self
    }

    /// Add a tuple to write.
    pub fn write(mut self, tuple: TupleOperation) -> Self {
        self.writes.push(tuple);
        self
    }

    /// Add tuples to write.
    pub fn writes(mut self, tuples: Vec<TupleOperation>) -> Self {
        self.writes.extend(tuples);
        self
    }

    /// Add a tuple key to delete.
    pub fn delete(mut self, key: TupleKey) -> Self {
        self.deletes.push(key);
        self
    }

    /// Add tuple keys to delete.
    pub fn deletes(mut self, keys: Vec<TupleKey>) -> Self {
        self.deletes.extend(keys);
        self
    }

    /// Set request metadata.
    pub fn with_metadata(mut self, metadata: RequestMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Get the NATS subject for this write request.
    pub fn subject(&self) -> String {
        format!("{}.{}", crate::SUBJECT_WRITES_PREFIX, self.store_id)
    }

    /// Serialize to JSON bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// Deserialize from JSON bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

/// Model write request for async model updates.
///
/// Published to the RSFGA_WRITES stream when a client calls the async
/// model update endpoint. Contains the full authorization model to be
/// written to storage.
///
/// # Model ID Generation ("Burning" Behavior)
///
/// The `model_id` is pre-generated when this request is created (either by the
/// caller or automatically via `new()`). This ID is returned to the client
/// immediately for RYOW (Read-Your-Own-Writes) consistency.
///
/// **Important**: If the async write fails after the request is created but
/// before it's committed to storage, the pre-generated `model_id` is "burned"
/// (consumed but never stored). This means:
///
/// - Model IDs are not guaranteed to be contiguous
/// - A client may receive a `model_id` that never becomes valid in storage
/// - Subsequent model writes will have higher/later IDs
///
/// This trade-off enables fast async acknowledgment while maintaining RYOW
/// semantics. Callers should verify model existence via `GetAuthorizationModel`
/// when strict consistency is required.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelWriteRequest {
    /// Unique request ID (ULID)
    pub request_id: String,

    /// Store ID
    pub store_id: String,

    /// Pre-generated model ID (ULID) for RYOW consistency.
    /// May be "burned" if async write fails - see struct docs.
    pub model_id: String,

    /// Schema version (e.g., "1.1", "1.2")
    pub schema_version: String,

    /// Type definitions for the authorization model
    pub type_definitions: serde_json::Value,

    /// Conditions defined in the model (optional)
    #[serde(default)]
    pub conditions: Option<serde_json::Value>,

    /// Request timestamp
    pub timestamp: DateTime<Utc>,

    /// Request metadata for tracing
    #[serde(default)]
    pub metadata: RequestMetadata,
}

impl ModelWriteRequest {
    /// Create a new model write request.
    pub fn new(store_id: impl Into<String>, schema_version: impl Into<String>) -> Self {
        Self {
            request_id: ulid::Ulid::new().to_string(),
            store_id: store_id.into(),
            model_id: ulid::Ulid::new().to_string(),
            schema_version: schema_version.into(),
            type_definitions: serde_json::Value::Array(vec![]),
            conditions: None,
            timestamp: Utc::now(),
            metadata: RequestMetadata::default(),
        }
    }

    /// Set model ID (for RYOW consistency when client provides pre-generated ID).
    pub fn with_model_id(mut self, model_id: impl Into<String>) -> Self {
        self.model_id = model_id.into();
        self
    }

    /// Set type definitions.
    pub fn with_type_definitions(mut self, type_definitions: serde_json::Value) -> Self {
        self.type_definitions = type_definitions;
        self
    }

    /// Set conditions.
    pub fn with_conditions(mut self, conditions: serde_json::Value) -> Self {
        self.conditions = Some(conditions);
        self
    }

    /// Set metadata.
    pub fn with_metadata(mut self, metadata: RequestMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Get the NATS subject for this request.
    pub fn subject(&self) -> String {
        format!("{}.models.{}", crate::SUBJECT_WRITES_PREFIX, self.store_id)
    }

    /// Serialize to JSON bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// Deserialize from JSON bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

/// Tuple operation for a write request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TupleOperation {
    /// Tuple key (user, relation, object)
    #[serde(flatten)]
    pub key: TupleKey,

    /// Optional condition for the tuple
    pub condition: Option<TupleCondition>,
}

impl TupleOperation {
    /// Create a new tuple operation.
    pub fn new(
        user: impl Into<String>,
        relation: impl Into<String>,
        object: impl Into<String>,
    ) -> Self {
        Self {
            key: TupleKey {
                user: user.into(),
                relation: relation.into(),
                object: object.into(),
            },
            condition: None,
        }
    }

    /// Add a condition to the tuple.
    pub fn with_condition(mut self, condition: TupleCondition) -> Self {
        self.condition = Some(condition);
        self
    }
}

/// Tuple key identifying a relationship.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct TupleKey {
    /// User (e.g., "user:alice", "group:eng#member")
    pub user: String,

    /// Relation (e.g., "viewer", "editor")
    pub relation: String,

    /// Object (e.g., "document:readme")
    pub object: String,
}

impl TupleKey {
    /// Create a new tuple key.
    pub fn new(
        user: impl Into<String>,
        relation: impl Into<String>,
        object: impl Into<String>,
    ) -> Self {
        Self {
            user: user.into(),
            relation: relation.into(),
            object: object.into(),
        }
    }
}

/// Condition attached to a tuple.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TupleCondition {
    /// Condition name (references condition defined in authorization model)
    pub name: String,

    /// Context values for condition parameters
    #[serde(default)]
    pub context: HashMap<String, serde_json::Value>,
}

impl TupleCondition {
    /// Create a new condition.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            context: HashMap::new(),
        }
    }

    /// Add a context value.
    pub fn with_context(
        mut self,
        key: impl Into<String>,
        value: impl Into<serde_json::Value>,
    ) -> Self {
        self.context.insert(key.into(), value.into());
        self
    }
}

/// Request metadata for tracing and audit.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RequestMetadata {
    /// Trace ID for distributed tracing
    pub trace_id: Option<String>,

    /// Span ID for distributed tracing
    pub span_id: Option<String>,

    /// Client ID for rate limiting and audit
    pub client_id: Option<String>,

    /// Source IP address
    pub source_ip: Option<String>,

    /// User agent
    pub user_agent: Option<String>,

    /// Additional custom metadata
    #[serde(default)]
    pub custom: HashMap<String, String>,
}

/// Committed event published to RSFGA_EVENTS stream after storage write.
///
/// This event is published by the storage consumer after successfully
/// committing a batch of writes to the database. It's used for:
///
/// - Edge synchronization
/// - Cache invalidation
/// - Audit logging
/// - Analytics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommittedEvent {
    /// Unique event ID (ULID)
    pub event_id: String,

    /// Store ID
    pub store_id: String,

    /// Sequence number within the store (monotonically increasing)
    pub sequence: u64,

    /// Original request IDs that were committed in this batch
    pub request_ids: Vec<String>,

    /// Tuples that were written
    #[serde(default)]
    pub writes: Vec<TupleOperation>,

    /// Tuples that were deleted
    #[serde(default)]
    pub deletes: Vec<TupleKey>,

    /// Timestamp when the commit occurred
    pub committed_at: DateTime<Utc>,

    /// Storage backend metadata (e.g., transaction ID)
    #[serde(default)]
    pub storage_metadata: HashMap<String, String>,
}

impl CommittedEvent {
    /// Create a new committed event.
    pub fn new(store_id: impl Into<String>, sequence: u64) -> Self {
        Self {
            event_id: ulid::Ulid::new().to_string(),
            store_id: store_id.into(),
            sequence,
            request_ids: Vec::new(),
            writes: Vec::new(),
            deletes: Vec::new(),
            committed_at: Utc::now(),
            storage_metadata: HashMap::new(),
        }
    }

    /// Add request IDs.
    pub fn with_request_ids(mut self, request_ids: Vec<String>) -> Self {
        self.request_ids = request_ids;
        self
    }

    /// Add written tuples.
    pub fn with_writes(mut self, writes: Vec<TupleOperation>) -> Self {
        self.writes = writes;
        self
    }

    /// Add deleted tuples.
    pub fn with_deletes(mut self, deletes: Vec<TupleKey>) -> Self {
        self.deletes = deletes;
        self
    }

    /// Get the NATS subject for this committed event.
    pub fn subject(&self) -> String {
        format!(
            "{}.{}.committed",
            crate::SUBJECT_EVENTS_PREFIX,
            self.store_id
        )
    }

    /// Serialize to JSON bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// Deserialize from JSON bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

/// Category of Dead Letter Queue message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DlqCategory {
    /// Message could not be parsed (malformed JSON, missing fields, etc.)
    ParseError,
    /// Event publish to RSFGA_EVENTS failed after retries
    EventPublishFailed,
    /// Message exceeded max delivery attempts (poison message)
    MaxRetriesExceeded,
}

impl std::fmt::Display for DlqCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DlqCategory::ParseError => write!(f, "parse_error"),
            DlqCategory::EventPublishFailed => write!(f, "event_publish_failed"),
            DlqCategory::MaxRetriesExceeded => write!(f, "max_retries_exceeded"),
        }
    }
}

/// Structured Dead Letter Queue message with error metadata.
///
/// Wraps the original payload with diagnostic information for analysis
/// and recovery. Published to the RSFGA_DLQ stream when messages cannot
/// be processed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DlqMessage {
    /// Error category
    pub category: DlqCategory,

    /// Human-readable error description
    pub error: String,

    /// NATS stream sequence of the original message
    pub stream_sequence: u64,

    /// Original NATS subject
    pub original_subject: String,

    /// Store ID (when extractable from the message)
    pub store_id: Option<String>,

    /// Number of delivery attempts before DLQ
    pub num_delivered: u64,

    /// Size of the original payload in bytes
    pub payload_size: usize,

    /// Original payload (base64-encoded for safe JSON transport)
    #[serde(with = "base64_bytes")]
    pub original_payload: Vec<u8>,

    /// Timestamp when the message was sent to DLQ
    pub dlq_timestamp: DateTime<Utc>,
}

impl DlqMessage {
    /// Create a new DLQ message for a parse error.
    pub fn parse_error(
        error: impl Into<String>,
        stream_sequence: u64,
        original_subject: impl Into<String>,
        payload: Vec<u8>,
        num_delivered: u64,
    ) -> Self {
        Self {
            category: DlqCategory::ParseError,
            error: error.into(),
            stream_sequence,
            original_subject: original_subject.into(),
            store_id: None,
            num_delivered,
            payload_size: payload.len(),
            original_payload: payload,
            dlq_timestamp: Utc::now(),
        }
    }

    /// Create a new DLQ message for an event publish failure.
    pub fn event_publish_failed(
        error: impl Into<String>,
        stream_sequence: u64,
        store_id: impl Into<String>,
        payload: Vec<u8>,
        num_delivered: u64,
    ) -> Self {
        let store = store_id.into();
        Self {
            category: DlqCategory::EventPublishFailed,
            error: error.into(),
            stream_sequence,
            original_subject: format!("{}.{}", crate::SUBJECT_WRITES_PREFIX, store),
            store_id: Some(store),
            num_delivered,
            payload_size: payload.len(),
            original_payload: payload,
            dlq_timestamp: Utc::now(),
        }
    }

    /// Create a new DLQ message for a poison message (max retries exceeded).
    pub fn max_retries_exceeded(
        stream_sequence: u64,
        original_subject: impl Into<String>,
        store_id: Option<String>,
        payload: Vec<u8>,
        num_delivered: u64,
    ) -> Self {
        Self {
            category: DlqCategory::MaxRetriesExceeded,
            error: format!("Message exceeded max delivery attempts ({})", num_delivered),
            stream_sequence,
            original_subject: original_subject.into(),
            store_id,
            num_delivered,
            payload_size: payload.len(),
            original_payload: payload,
            dlq_timestamp: Utc::now(),
        }
    }

    /// Get the NATS DLQ subject for this message.
    pub fn subject(&self) -> String {
        format!(
            "{}.{}.{}",
            crate::SUBJECT_DLQ_PREFIX,
            self.category,
            self.stream_sequence
        )
    }

    /// Serialize to JSON bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// Deserialize from JSON bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

/// Base64 serialization for binary payloads in JSON.
mod base64_bytes {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(deserializer)?;
        STANDARD.decode(s).map_err(serde::de::Error::custom)
    }
}

/// Write ticket returned by async write endpoint.
///
/// The client can use this ticket to wait for the write to be committed
/// (read-your-own-writes consistency).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteTicket {
    /// Store ID
    pub store_id: String,

    /// Expected sequence number after commit
    pub sequence: u64,

    /// Request ID for correlation
    pub request_id: String,

    /// Expiration time for the ticket
    pub expires_at: DateTime<Utc>,
}

impl WriteTicket {
    /// Default TTL for write tickets (60 seconds).
    pub const DEFAULT_TTL_SECS: i64 = 60;

    /// Create a new write ticket with default TTL (60 seconds).
    pub fn new(store_id: impl Into<String>, sequence: u64, request_id: impl Into<String>) -> Self {
        Self::with_ttl(
            store_id,
            sequence,
            request_id,
            std::time::Duration::from_secs(Self::DEFAULT_TTL_SECS as u64),
        )
    }

    /// Create a new write ticket with custom TTL.
    pub fn with_ttl(
        store_id: impl Into<String>,
        sequence: u64,
        request_id: impl Into<String>,
        ttl: std::time::Duration,
    ) -> Self {
        Self {
            store_id: store_id.into(),
            sequence,
            request_id: request_id.into(),
            expires_at: Utc::now()
                + chrono::Duration::from_std(ttl)
                    .unwrap_or(chrono::Duration::seconds(Self::DEFAULT_TTL_SECS)),
        }
    }

    /// Check if the ticket has expired.
    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_request_creation() {
        let request = WriteRequest::new("store-1")
            .with_model_id("model-1")
            .write(TupleOperation::new(
                "user:alice",
                "viewer",
                "document:readme",
            ))
            .delete(TupleKey::new("user:bob", "editor", "document:readme"));

        assert_eq!(request.store_id, "store-1");
        assert_eq!(request.authorization_model_id, Some("model-1".to_string()));
        assert_eq!(request.writes.len(), 1);
        assert_eq!(request.deletes.len(), 1);
        assert!(!request.request_id.is_empty());
    }

    #[test]
    fn test_write_request_subject() {
        let request = WriteRequest::new("store-abc");
        assert_eq!(request.subject(), "rsfga.writes.store-abc");
    }

    #[test]
    fn test_write_request_serialization() {
        let request = WriteRequest::new("store-1").write(TupleOperation::new(
            "user:alice",
            "viewer",
            "document:readme",
        ));

        let bytes = request.to_bytes().unwrap();
        let parsed = WriteRequest::from_bytes(&bytes).unwrap();

        assert_eq!(parsed.store_id, request.store_id);
        assert_eq!(parsed.writes.len(), 1);
        assert_eq!(parsed.writes[0].key.user, "user:alice");
    }

    #[test]
    fn test_tuple_operation_with_condition() {
        let tuple = TupleOperation::new("user:alice", "viewer", "document:readme").with_condition(
            TupleCondition::new("time_bound").with_context("expires_at", "2026-12-31T23:59:59Z"),
        );

        assert!(tuple.condition.is_some());
        let condition = tuple.condition.unwrap();
        assert_eq!(condition.name, "time_bound");
        assert!(condition.context.contains_key("expires_at"));
    }

    #[test]
    fn test_committed_event_creation() {
        let event = CommittedEvent::new("store-1", 42)
            .with_request_ids(vec!["req-1".to_string(), "req-2".to_string()])
            .with_writes(vec![TupleOperation::new(
                "user:alice",
                "viewer",
                "document:readme",
            )]);

        assert_eq!(event.store_id, "store-1");
        assert_eq!(event.sequence, 42);
        assert_eq!(event.request_ids.len(), 2);
        assert_eq!(event.writes.len(), 1);
    }

    #[test]
    fn test_committed_event_subject() {
        let event = CommittedEvent::new("store-xyz", 1);
        assert_eq!(event.subject(), "rsfga.events.store-xyz.committed");
    }

    #[test]
    fn test_committed_event_serialization() {
        let event = CommittedEvent::new("store-1", 100).with_writes(vec![TupleOperation::new(
            "user:alice",
            "viewer",
            "document:readme",
        )]);

        let bytes = event.to_bytes().unwrap();
        let parsed = CommittedEvent::from_bytes(&bytes).unwrap();

        assert_eq!(parsed.store_id, event.store_id);
        assert_eq!(parsed.sequence, event.sequence);
        assert_eq!(parsed.writes.len(), 1);
    }

    #[test]
    fn test_write_ticket_expiration() {
        let ticket = WriteTicket::new("store-1", 42, "req-1");
        assert!(!ticket.is_expired());

        let expired_ticket = WriteTicket {
            store_id: "store-1".to_string(),
            sequence: 42,
            request_id: "req-1".to_string(),
            expires_at: Utc::now() - chrono::Duration::seconds(60),
        };
        assert!(expired_ticket.is_expired());
    }

    #[test]
    fn test_tuple_key_equality() {
        let key1 = TupleKey::new("user:alice", "viewer", "document:readme");
        let key2 = TupleKey::new("user:alice", "viewer", "document:readme");
        let key3 = TupleKey::new("user:bob", "viewer", "document:readme");

        assert_eq!(key1, key2);
        assert_ne!(key1, key3);
    }

    #[test]
    fn test_request_metadata() {
        let metadata = RequestMetadata {
            trace_id: Some("trace-123".to_string()),
            client_id: Some("client-1".to_string()),
            ..Default::default()
        };

        let request = WriteRequest::new("store-1").with_metadata(metadata);

        assert_eq!(request.metadata.trace_id, Some("trace-123".to_string()));
        assert_eq!(request.metadata.client_id, Some("client-1".to_string()));
    }

    // DLQ Message tests

    #[test]
    fn test_dlq_message_parse_error_creation() {
        let payload = b"not valid json".to_vec();
        let msg = DlqMessage::parse_error(
            "expected value at line 1",
            42,
            "rsfga.writes.store1",
            payload.clone(),
            1,
        );

        assert_eq!(msg.category, DlqCategory::ParseError);
        assert_eq!(msg.error, "expected value at line 1");
        assert_eq!(msg.stream_sequence, 42);
        assert_eq!(msg.original_subject, "rsfga.writes.store1");
        assert_eq!(msg.store_id, None);
        assert_eq!(msg.num_delivered, 1);
        assert_eq!(msg.payload_size, payload.len());
        assert_eq!(msg.original_payload, payload);
    }

    #[test]
    fn test_dlq_message_event_publish_failed_creation() {
        let payload = b"{\"store_id\":\"store1\"}".to_vec();
        let msg = DlqMessage::event_publish_failed(
            "NATS connection lost",
            100,
            "store1",
            payload.clone(),
            3,
        );

        assert_eq!(msg.category, DlqCategory::EventPublishFailed);
        assert_eq!(msg.error, "NATS connection lost");
        assert_eq!(msg.stream_sequence, 100);
        assert_eq!(msg.store_id, Some("store1".to_string()));
        assert_eq!(msg.num_delivered, 3);
        assert_eq!(msg.original_payload, payload);
    }

    #[test]
    fn test_dlq_message_max_retries_exceeded_creation() {
        let payload = b"some message".to_vec();
        let msg = DlqMessage::max_retries_exceeded(
            55,
            "rsfga.writes.store2",
            Some("store2".to_string()),
            payload.clone(),
            10,
        );

        assert_eq!(msg.category, DlqCategory::MaxRetriesExceeded);
        assert!(msg.error.contains("10"));
        assert_eq!(msg.stream_sequence, 55);
        assert_eq!(msg.store_id, Some("store2".to_string()));
        assert_eq!(msg.num_delivered, 10);
    }

    #[test]
    fn test_dlq_message_serialization_roundtrip() {
        let original_payload = b"bad data \x00\xff binary".to_vec();
        let msg = DlqMessage::parse_error(
            "invalid utf-8",
            7,
            "rsfga.writes.store1",
            original_payload.clone(),
            2,
        );

        let bytes = msg.to_bytes().unwrap();
        let parsed = DlqMessage::from_bytes(&bytes).unwrap();

        assert_eq!(parsed.category, DlqCategory::ParseError);
        assert_eq!(parsed.error, "invalid utf-8");
        assert_eq!(parsed.stream_sequence, 7);
        assert_eq!(parsed.original_subject, "rsfga.writes.store1");
        assert_eq!(parsed.num_delivered, 2);
        assert_eq!(parsed.payload_size, original_payload.len());
        assert_eq!(parsed.original_payload, original_payload);
    }

    #[test]
    fn test_dlq_message_subject_format() {
        let msg = DlqMessage::parse_error("err", 42, "sub", vec![], 1);
        assert_eq!(msg.subject(), "rsfga.dlq.parse_error.42");

        let msg = DlqMessage::event_publish_failed("err", 100, "s1", vec![], 1);
        assert_eq!(msg.subject(), "rsfga.dlq.event_publish_failed.100");

        let msg = DlqMessage::max_retries_exceeded(55, "sub", None, vec![], 10);
        assert_eq!(msg.subject(), "rsfga.dlq.max_retries_exceeded.55");
    }

    #[test]
    fn test_dlq_category_display() {
        assert_eq!(DlqCategory::ParseError.to_string(), "parse_error");
        assert_eq!(
            DlqCategory::EventPublishFailed.to_string(),
            "event_publish_failed"
        );
        assert_eq!(
            DlqCategory::MaxRetriesExceeded.to_string(),
            "max_retries_exceeded"
        );
    }

    #[test]
    fn test_dlq_message_binary_payload_base64_in_json() {
        let payload = vec![0u8, 1, 2, 255, 254, 253];
        let msg = DlqMessage::parse_error("binary", 1, "sub", payload.clone(), 1);

        let json_str = serde_json::to_string(&msg).unwrap();
        // Verify it's valid JSON (binary is base64-encoded, not raw bytes)
        assert!(!json_str.contains("\\u0000"));

        let parsed: DlqMessage = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed.original_payload, payload);
    }

    #[test]
    fn test_dlq_category_serde_roundtrip() {
        let categories = vec![
            DlqCategory::ParseError,
            DlqCategory::EventPublishFailed,
            DlqCategory::MaxRetriesExceeded,
        ];

        for cat in categories {
            let json = serde_json::to_string(&cat).unwrap();
            let parsed: DlqCategory = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, cat);
        }
    }
}
