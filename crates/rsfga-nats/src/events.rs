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
    /// Create a new write ticket.
    pub fn new(store_id: impl Into<String>, sequence: u64, request_id: impl Into<String>) -> Self {
        Self {
            store_id: store_id.into(),
            sequence,
            request_id: request_id.into(),
            expires_at: Utc::now() + chrono::Duration::seconds(60),
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
}
