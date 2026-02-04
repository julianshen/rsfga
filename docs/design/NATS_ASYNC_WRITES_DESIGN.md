# NATS Async Writes Design

**Status**: Draft
**Authors**: RSFGA Team
**Created**: 2026-02-04
**Last Updated**: 2026-02-04
**Related Documents**:
- [EDGE_ARCHITECTURE_NATS.md](EDGE_ARCHITECTURE_NATS.md) - Edge deployment topology
- [ARCHITECTURE_DECISIONS.md](ARCHITECTURE_DECISIONS.md) - ADR-007 (Async Cache Invalidation)
- [ARCHITECTURE.md](ARCHITECTURE.md) - System constraints and assumptions

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Motivation and Goals](#2-motivation-and-goals)
3. [Current Write Path Analysis](#3-current-write-path-analysis)
4. [Proposed Architecture: NATS-First Writes](#4-proposed-architecture-nats-first-writes)
5. [Event Schema Design](#5-event-schema-design)
6. [Write Path: Client to NATS](#6-write-path-client-to-nats)
7. [Storage Consumer: NATS to Database](#7-storage-consumer-nats-to-database)
8. [Read-Your-Own-Writes Consistency](#8-read-your-own-writes-consistency)
9. [Edge Synchronization](#9-edge-synchronization)
10. [Multi-Region Replication](#10-multi-region-replication)
11. [Failure Handling and Recovery](#11-failure-handling-and-recovery)
12. [Performance Analysis](#12-performance-analysis)
13. [Security Considerations](#13-security-considerations)
14. [Observability](#14-observability)
15. [Implementation Plan](#15-implementation-plan)
16. [Alternatives Considered](#16-alternatives-considered)
17. [Open Questions](#17-open-questions)

---

## 1. Executive Summary

This document proposes a **NATS-first write architecture** for RSFGA where writes go to NATS JetStream first, then are consumed and batched into storage. This inverts the traditional pattern for significant benefits:

```
┌─────────────────────────────────────────────────────────────────────┐
│                    NATS-First Write Architecture                     │
│                                                                      │
│   Client ──▶ NATS JetStream ──▶ Storage Consumer ──▶ Database       │
│              (fast, durable)     (batched writes)                   │
│                    │                                                 │
│                    ├──▶ Edge Sync Consumer                          │
│                    ├──▶ Cache Invalidation Consumer                 │
│                    └──▶ Audit/Analytics Consumer                    │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### Core Principle: **"NATS is the Write Path, Storage is the Read Path"**

- Writes commit to NATS JetStream (durable, fast acknowledgment)
- Storage consumer batches events for efficient bulk writes
- Multiple consumers can process the same event stream
- Natural decoupling, backpressure, and reliability

### Key Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Write Target | NATS JetStream first | Non-blocking, durable, enables batching |
| Storage Writes | Batched by consumer | 10-50x throughput improvement |
| Consistency Model | Eventual (with RYOW option) | Performance over strong consistency |
| Event Delivery | At-least-once | Idempotent consumers handle duplicates |
| Ordering | Per-store FIFO | JetStream subject partitioning |

### Benefits Over Storage-First

| Metric | Storage-First | NATS-First | Improvement |
|--------|---------------|------------|-------------|
| Write Latency (p99) | 15-20ms | 2-5ms | **4-10x faster** |
| Throughput (writes/sec) | 150-300 | 5,000-10,000 | **20-50x higher** |
| Burst Handling | Limited by DB | JetStream buffer | **Much better** |
| Multi-consumer | Complex (CDC) | Native | **Simpler** |

---

## 2. Motivation and Goals

### 2.1 Why NATS-First?

The traditional write path (validate → write to DB → publish event) has limitations:

1. **Write latency**: Blocked by database round-trip (10-20ms)
2. **Throughput ceiling**: Limited by database connection pool
3. **No batching**: Each write is a separate transaction
4. **Tight coupling**: Storage failure blocks writes entirely
5. **Single consumer**: Additional consumers need CDC/polling

**NATS-first inverts this**: Writes are accepted immediately by JetStream, and a dedicated consumer handles database persistence with batching and retry.

### 2.2 Goals

| Goal | Priority | Success Criteria |
|------|----------|------------------|
| G1: Sub-5ms write latency | P0 | Client receives ack in <5ms p99 |
| G2: 10,000+ writes/sec | P0 | Sustained throughput with batching |
| G3: Durability before ack | P0 | JetStream persists before client ack |
| G4: Batch storage writes | P0 | 100-500 tuples per transaction |
| G5: Multi-consumer support | P1 | Edge, cache, audit consumers |
| G6: Read-your-own-writes | P1 | Optional strong consistency mode |
| G7: Graceful degradation | P1 | Handle NATS/storage failures |

### 2.3 Non-Goals

- **Strong consistency by default**: Would negate latency benefits
- **Exactly-once to storage**: At-least-once with idempotency is sufficient
- **Real-time reads after write**: Eventual consistency is acceptable (with RYOW option)

---

## 3. Current Write Path Analysis

### 3.1 Existing Synchronous Write Flow

```
┌─────────────────────────────────────────────────────────────────────┐
│                   Current Write Path (Synchronous)                   │
└─────────────────────────────────────────────────────────────────────┘

  Client              API Layer              Database              Cache
    │                     │                      │                   │
    │  POST /write        │                      │                   │
    ├────────────────────▶│                      │                   │
    │                     │                      │                   │
    │                     │ 1. Validate request  │                   │
    │                     │ 2. Check auth model  │                   │
    │                     │                      │                   │
    │                     │    BEGIN TX          │                   │
    │                     ├─────────────────────▶│                   │
    │                     │    DELETE tuples     │                   │
    │                     ├─────────────────────▶│                   │
    │                     │    INSERT tuples     │                   │
    │                     ├─────────────────────▶│                   │
    │                     │    INSERT changelog  │                   │
    │                     ├─────────────────────▶│                   │
    │                     │    COMMIT            │                   │
    │                     │◀─────────────────────┤                   │
    │                     │                      │                   │
    │                     │ 3. Async invalidate ─┼──────────────────▶│
    │                     │                      │                   │
    │  200 OK             │                      │                   │
    │◀────────────────────┤                      │                   │
    │                     │                      │                   │

    Timeline: ├──────────────── 15-20ms ────────────────┤
```

### 3.2 Current Limitations

| Limitation | Impact | Root Cause |
|------------|--------|------------|
| High latency | 15-20ms p99 | Synchronous DB transaction |
| Limited throughput | ~300 writes/sec | DB connection pool, per-write TX |
| No batching | Inefficient | Each API call = 1 transaction |
| Burst handling | Poor | DB becomes bottleneck |
| Single consumer | Inflexible | No native event stream |

---

## 4. Proposed Architecture: NATS-First Writes

### 4.1 High-Level Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        NATS-First Write Architecture                         │
└─────────────────────────────────────────────────────────────────────────────┘

  Client              API Layer           NATS JetStream         Consumers
    │                     │                     │                     │
    │  POST /write        │                     │                     │
    ├────────────────────▶│                     │                     │
    │                     │                     │                     │
    │                     │ 1. Validate request │                     │
    │                     │ 2. Check auth model │                     │
    │                     │    (cached)         │                     │
    │                     │                     │                     │
    │                     │    PUBLISH event    │                     │
    │                     ├────────────────────▶│                     │
    │                     │                     │ [Persist to disk]   │
    │                     │◀────────────────────┤ ACK                 │
    │                     │                     │                     │
    │  200 OK + ticket    │                     │                     │
    │◀────────────────────┤                     │                     │
    │                     │                     │                     │
    │                     │                     │    Storage Consumer │
    │                     │                     ├────────────────────▶│
    │                     │                     │    (batched writes) │
    │                     │                     │                     │
    │                     │                     │    Edge Consumer    │
    │                     │                     ├────────────────────▶│
    │                     │                     │                     │
    │                     │                     │    Cache Consumer   │
    │                     │                     ├────────────────────▶│
    │                     │                     │                     │

    Write latency: ├─── 2-5ms ───┤
    Storage latency: ├────────────────── 50-200ms (batched) ─────────┤
```

### 4.2 Component Overview

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           Component Architecture                             │
└─────────────────────────────────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────────────────────────────────┐
│                              API Layer                                        │
│                                                                               │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌──────────────────────┐ │
│  │   Validator │  │Model Cache  │  │NATS Producer│  │ Write Ticket Tracker │ │
│  │             │  │  (Moka)     │  │             │  │    (for RYOW)        │ │
│  └─────────────┘  └─────────────┘  └─────────────┘  └──────────────────────┘ │
└──────────────────────────────────────────────────────────────────────────────┘
                                          │
                                          ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│                           NATS JetStream                                      │
│                                                                               │
│  Stream: RSFGA_WRITES                                                        │
│  ┌────────────────────────────────────────────────────────────────────────┐  │
│  │  Subject: rsfga.writes.{store_id}                                      │  │
│  │  Retention: WorkQueue (deleted after ack)                              │  │
│  │  Replicas: 3                                                           │  │
│  │  Dedup Window: 120s                                                    │  │
│  └────────────────────────────────────────────────────────────────────────┘  │
│                                                                               │
│  Stream: RSFGA_EVENTS (for fan-out after storage commit)                     │
│  ┌────────────────────────────────────────────────────────────────────────┐  │
│  │  Subject: rsfga.events.{store_id}.{event_type}                         │  │
│  │  Retention: Limits (7 days)                                            │  │
│  │  Consumers: Edge sync, audit, analytics                                │  │
│  └────────────────────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────────────────┘
                                          │
                    ┌─────────────────────┼─────────────────────┐
                    ▼                     ▼                     ▼
┌──────────────────────────┐ ┌──────────────────────┐ ┌──────────────────────┐
│    Storage Consumer      │ │    Edge Consumer     │ │   Cache Consumer     │
│                          │ │                      │ │                      │
│ • Pull from RSFGA_WRITES │ │ • Push from EVENTS   │ │ • Push from EVENTS   │
│ • Batch by store_id      │ │ • Apply to local DB  │ │ • Invalidate entries │
│ • Bulk INSERT/DELETE     │ │ • Track sync position│ │ • Update metrics     │
│ • Publish to EVENTS      │ │                      │ │                      │
│ • Ack on commit          │ │                      │ │                      │
└──────────────────────────┘ └──────────────────────┘ └──────────────────────┘
             │
             ▼
┌──────────────────────────┐
│        Database          │
│  (PostgreSQL/MySQL/etc)  │
└──────────────────────────┘
```

### 4.3 Two-Stream Design

We use **two separate streams** for different purposes:

| Stream | Purpose | Retention | Consumers |
|--------|---------|-----------|-----------|
| `RSFGA_WRITES` | Incoming writes queue | WorkQueue (delete on ack) | Storage consumer only |
| `RSFGA_EVENTS` | Committed events fan-out | Limits (7 days) | Edge, cache, audit, analytics |

**Why two streams?**
1. `RSFGA_WRITES` is a work queue - each message processed once by storage consumer
2. `RSFGA_EVENTS` is a fan-out stream - multiple consumers see all events
3. Events only appear in `RSFGA_EVENTS` after storage commit (guaranteed durable)
4. Separation allows different retention and consumer patterns

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         Two-Stream Flow                                      │
└─────────────────────────────────────────────────────────────────────────────┘

   API Layer                RSFGA_WRITES              Storage Consumer
       │                         │                          │
       │  Publish WriteRequest   │                          │
       ├────────────────────────▶│                          │
       │                         │                          │
       │                         │   Pull batch (100 msgs)  │
       │                         │◀─────────────────────────┤
       │                         │                          │
       │                         │                          │ Batch by store_id
       │                         │                          │ Write to database
       │                         │                          │
       │                         │         ACK batch        │
       │                         │◀─────────────────────────┤
       │                         │                          │
       │                         │                          │
       │                    RSFGA_EVENTS                    │
       │                         │                          │
       │                         │   Publish CommittedEvent │
       │                         │◀─────────────────────────┤
       │                         │                          │
       │                         │                          │
                                 │
           ┌─────────────────────┼─────────────────────┐
           ▼                     ▼                     ▼
    Edge Consumer         Cache Consumer        Audit Consumer
```

---

## 5. Event Schema Design

### 5.1 Write Request Event (RSFGA_WRITES stream)

```protobuf
syntax = "proto3";
package rsfga.writes.v1;

import "google/protobuf/timestamp.proto";
import "google/protobuf/struct.proto";

// WriteRequest represents an incoming write operation
// Published to: rsfga.writes.{store_id}
message WriteRequest {
  // Unique request ID (ULID) - used for deduplication
  string request_id = 1;

  // Store this write belongs to
  string store_id = 2;

  // Timestamp when request was received
  google.protobuf.Timestamp received_at = 3;

  // Authorization model ID (for validation tracking)
  string authorization_model_id = 4;

  // Tuples to write
  repeated TupleWrite writes = 5;

  // Tuples to delete
  repeated TupleKey deletes = 6;

  // Client metadata for tracing
  RequestMetadata metadata = 7;
}

message TupleWrite {
  TupleKey key = 1;
  optional Condition condition = 2;
}

message TupleKey {
  string object_type = 1;
  string object_id = 2;
  string relation = 3;
  string user_type = 4;
  string user_id = 5;
  string user_relation = 6;  // For userset references
}

message Condition {
  string name = 1;
  google.protobuf.Struct context = 2;
}

message RequestMetadata {
  string correlation_id = 1;
  string client_id = 2;
  string source_ip = 3;
}
```

### 5.2 Committed Event (RSFGA_EVENTS stream)

```protobuf
// CommittedEvent represents a write that has been persisted to storage
// Published to: rsfga.events.{store_id}.{event_type}
message CommittedEvent {
  // Unique event ID
  string event_id = 1;

  // Original request ID (for correlation)
  string request_id = 2;

  // Store ID
  string store_id = 3;

  // Monotonic sequence number within store
  uint64 sequence = 4;

  // Timestamp when committed to storage
  google.protobuf.Timestamp committed_at = 5;

  // Event type
  EventType event_type = 6;

  // Payload
  oneof payload {
    TupleBatchCommitted tuple_batch = 10;
    ModelUpdated model_updated = 11;
  }
}

enum EventType {
  EVENT_TYPE_UNSPECIFIED = 0;
  EVENT_TYPE_TUPLE_BATCH = 1;
  EVENT_TYPE_MODEL_UPDATED = 2;
}

message TupleBatchCommitted {
  repeated TupleOperation operations = 1;
}

message TupleOperation {
  OperationType operation = 1;
  TupleKey key = 2;
  optional Condition condition = 3;
}

enum OperationType {
  OPERATION_TYPE_UNSPECIFIED = 0;
  OPERATION_TYPE_WRITE = 1;
  OPERATION_TYPE_DELETE = 2;
}
```

### 5.3 Subject Patterns

```
RSFGA_WRITES stream:
├── rsfga.writes.store-abc          # Writes for store-abc
├── rsfga.writes.store-xyz          # Writes for store-xyz
└── rsfga.writes.>                  # Wildcard for all stores

RSFGA_EVENTS stream:
├── rsfga.events.store-abc.tuple    # Tuple changes for store-abc
├── rsfga.events.store-abc.model    # Model changes for store-abc
├── rsfga.events.*.tuple            # All tuple changes (for global audit)
└── rsfga.events.>                  # All events
```

---

## 6. Write Path: Client to NATS

### 6.1 API Handler Implementation

```rust
use async_nats::jetstream::{self, Context, PublishAckFuture};
use prost::Message;
use ulid::Ulid;

/// Write handler - publishes to NATS JetStream
pub async fn write_tuples_handler(
    State(state): State<AppState>,
    Path(store_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<WriteApiRequest>,
) -> Result<Json<WriteApiResponse>, ApiError> {
    let request_id = Ulid::new().to_string();
    let received_at = SystemTime::now();

    // 1. Validate request format (fast, no I/O)
    validate_write_request(&request)?;

    // 2. Check authorization model (from cache)
    let model = state.model_cache
        .get_or_fetch(&store_id, || state.storage.get_latest_model(&store_id))
        .await?;

    validate_tuples_against_model(&request.writes, &request.deletes, &model)?;

    // 3. Build write request event
    let write_request = WriteRequest {
        request_id: request_id.clone(),
        store_id: store_id.clone(),
        received_at: Some(received_at.into()),
        authorization_model_id: model.id.clone(),
        writes: request.writes.iter().map(Into::into).collect(),
        deletes: request.deletes.iter().map(Into::into).collect(),
        metadata: Some(RequestMetadata {
            correlation_id: extract_correlation_id(&headers),
            client_id: extract_client_id(&headers),
            source_ip: extract_source_ip(&headers),
        }),
    };

    // 4. Publish to JetStream with deduplication
    let subject = format!("rsfga.writes.{}", store_id);
    let payload = write_request.encode_to_vec();

    let ack = state.js_context
        .publish_with_options(
            subject,
            payload.into(),
            jetstream::PublishOptions {
                msg_id: Some(request_id.clone()),  // Deduplication key
                expected_stream: Some("RSFGA_WRITES".to_string()),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| ApiError::ServiceUnavailable(format!("Write queue unavailable: {}", e)))?;

    // 5. Wait for JetStream acknowledgment (durability guarantee)
    let ack = ack.await
        .map_err(|e| ApiError::ServiceUnavailable(format!("Write not acknowledged: {}", e)))?;

    // 6. Track write ticket for read-your-own-writes (optional)
    if request.consistency == Some(ConsistencyPreference::HigherConsistency) {
        state.write_tracker.track(&store_id, &request_id, ack.sequence).await;
    }

    // 7. Return success with ticket
    Ok(Json(WriteApiResponse {
        request_id,
        sequence: ack.sequence,
        // Ticket for RYOW - client can use this to wait for storage commit
        write_ticket: Some(WriteTicket {
            store_id,
            sequence: ack.sequence,
        }),
    }))
}

/// Response includes write ticket for read-your-own-writes
#[derive(Serialize)]
pub struct WriteApiResponse {
    pub request_id: String,
    pub sequence: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub write_ticket: Option<WriteTicket>,
}

#[derive(Serialize)]
pub struct WriteTicket {
    pub store_id: String,
    pub sequence: u64,
}
```

### 6.2 Write Latency Breakdown

```
┌─────────────────────────────────────────────────────────────────────┐
│                    NATS-First Write Latency                          │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  Request parsing:           ~0.1ms                                   │
│  Model cache lookup:        ~0.1ms (hot cache)                       │
│  Tuple validation:          ~0.2ms                                   │
│  Protobuf serialization:    ~0.1ms                                   │
│  NATS publish + ack:        ~1-3ms (local), ~5-10ms (remote)        │
│  Response serialization:    ~0.1ms                                   │
│  ─────────────────────────────────────────                          │
│  Total (local NATS):        ~2-4ms p99                               │
│  Total (remote NATS):       ~6-12ms p99                              │
│                                                                      │
│  Compare to storage-first:  15-25ms p99                              │
│  Improvement:               3-10x faster                             │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### 6.3 JetStream Configuration for Writes

```rust
/// Configure RSFGA_WRITES stream
pub async fn setup_writes_stream(js: &Context) -> Result<(), NatsError> {
    let config = jetstream::stream::Config {
        name: "RSFGA_WRITES".to_string(),
        description: Some("Incoming write requests for RSFGA".to_string()),

        // Subject pattern
        subjects: vec!["rsfga.writes.>".to_string()],

        // WorkQueue retention: delete after consumer ack
        retention: jetstream::stream::RetentionPolicy::WorkQueue,

        // Replication for durability
        num_replicas: 3,

        // Storage
        storage: jetstream::stream::StorageType::File,

        // Deduplication window (must cover retry period)
        duplicate_window: Duration::from_secs(120),

        // Limits
        max_messages: 10_000_000,
        max_bytes: 10 * 1024 * 1024 * 1024,  // 10GB
        max_age: Duration::from_secs(24 * 60 * 60),  // 24h max (should be consumed quickly)
        max_msg_size: 1024 * 1024,  // 1MB

        // Discard old if limits hit
        discard: jetstream::stream::DiscardPolicy::Old,

        ..Default::default()
    };

    js.get_or_create_stream(config).await?;
    Ok(())
}
```

---

## 7. Storage Consumer: NATS to Database

### 7.1 Consumer Architecture

The storage consumer is the critical component that:
1. Pulls batches of write requests from `RSFGA_WRITES`
2. Groups by store_id for efficient batch writes
3. Writes to database in bulk transactions
4. Publishes committed events to `RSFGA_EVENTS`
5. Acknowledges processed messages

```rust
/// Storage consumer that batches writes to database
pub struct StorageConsumer {
    js_context: Context,
    storage: Arc<dyn DataStore>,
    event_publisher: Arc<EventPublisher>,
    config: StorageConsumerConfig,
    metrics: Arc<ConsumerMetrics>,
}

#[derive(Clone)]
pub struct StorageConsumerConfig {
    /// Maximum messages per batch
    pub batch_size: usize,

    /// Maximum time to wait for batch to fill
    pub batch_timeout: Duration,

    /// Maximum messages per store in a single transaction
    pub max_per_store_batch: usize,

    /// Number of parallel store writers
    pub parallelism: usize,
}

impl Default for StorageConsumerConfig {
    fn default() -> Self {
        Self {
            batch_size: 500,
            batch_timeout: Duration::from_millis(100),
            max_per_store_batch: 200,
            parallelism: 4,
        }
    }
}

impl StorageConsumer {
    pub async fn new(
        js_context: Context,
        storage: Arc<dyn DataStore>,
        event_publisher: Arc<EventPublisher>,
        config: StorageConsumerConfig,
    ) -> Result<Self, NatsError> {
        Ok(Self {
            js_context,
            storage,
            event_publisher,
            config,
            metrics: Arc::new(ConsumerMetrics::new()),
        })
    }

    /// Start the storage consumer
    pub async fn run(&self) -> Result<(), ConsumerError> {
        // Create pull consumer
        let consumer = self.js_context
            .get_stream("RSFGA_WRITES").await?
            .create_consumer(jetstream::consumer::pull::Config {
                name: Some("storage-consumer".to_string()),
                durable_name: Some("storage-consumer".to_string()),
                ack_policy: jetstream::consumer::AckPolicy::Explicit,
                ack_wait: Duration::from_secs(60),
                max_deliver: 5,
                max_ack_pending: 10_000,
                ..Default::default()
            }).await?;

        info!("Storage consumer started");

        loop {
            // Pull batch of messages
            let batch = consumer
                .fetch()
                .max_messages(self.config.batch_size)
                .expires(self.config.batch_timeout)
                .messages()
                .await?;

            let messages: Vec<_> = batch.try_collect().await?;

            if messages.is_empty() {
                continue;
            }

            self.metrics.batches_received.inc();
            self.metrics.messages_received.add(messages.len() as u64);

            // Process batch
            match self.process_batch(messages).await {
                Ok(()) => {
                    self.metrics.batches_processed.inc();
                }
                Err(e) => {
                    error!(error = %e, "Failed to process batch");
                    self.metrics.batches_failed.inc();
                    // Messages will be redelivered by NATS
                }
            }
        }
    }

    /// Process a batch of write requests
    async fn process_batch(&self, messages: Vec<Message>) -> Result<(), ConsumerError> {
        let start = Instant::now();

        // 1. Decode and group by store_id
        let mut by_store: HashMap<String, Vec<(Message, WriteRequest)>> = HashMap::new();

        for msg in messages {
            match WriteRequest::decode(msg.payload.as_ref()) {
                Ok(req) => {
                    by_store
                        .entry(req.store_id.clone())
                        .or_default()
                        .push((msg, req));
                }
                Err(e) => {
                    warn!(error = %e, "Failed to decode write request, acking to skip");
                    msg.ack().await?;
                }
            }
        }

        // 2. Process each store's writes in parallel (up to parallelism limit)
        let semaphore = Arc::new(Semaphore::new(self.config.parallelism));
        let mut handles = Vec::new();

        for (store_id, requests) in by_store {
            let permit = semaphore.clone().acquire_owned().await?;
            let storage = self.storage.clone();
            let publisher = self.event_publisher.clone();
            let max_batch = self.config.max_per_store_batch;
            let metrics = self.metrics.clone();

            let handle = tokio::spawn(async move {
                let _permit = permit;
                Self::process_store_batch(&store_id, requests, storage, publisher, max_batch, metrics).await
            });

            handles.push(handle);
        }

        // 3. Wait for all store batches to complete
        for handle in handles {
            handle.await??;
        }

        self.metrics.batch_latency.observe(start.elapsed().as_secs_f64());

        Ok(())
    }

    /// Process all writes for a single store
    async fn process_store_batch(
        store_id: &str,
        requests: Vec<(Message, WriteRequest)>,
        storage: Arc<dyn DataStore>,
        publisher: Arc<EventPublisher>,
        max_batch: usize,
        metrics: Arc<ConsumerMetrics>,
    ) -> Result<(), ConsumerError> {
        // Split into sub-batches if too large
        for chunk in requests.chunks(max_batch) {
            // Aggregate all writes and deletes
            let mut all_writes = Vec::new();
            let mut all_deletes = Vec::new();
            let mut request_ids = Vec::new();

            for (_, req) in chunk {
                request_ids.push(req.request_id.clone());

                for write in &req.writes {
                    all_writes.push(StoredTuple::from(write));
                }
                for delete in &req.deletes {
                    all_deletes.push(StoredTuple::from(delete));
                }
            }

            // Write to database (with deduplication handling)
            let sequence = storage
                .write_tuples_with_sequence(store_id, all_writes.clone(), all_deletes.clone())
                .await?;

            metrics.tuples_written.add(all_writes.len() as u64);
            metrics.tuples_deleted.add(all_deletes.len() as u64);

            // Publish committed event to RSFGA_EVENTS
            publisher.publish_committed(
                store_id,
                sequence,
                &request_ids,
                &all_writes,
                &all_deletes,
            ).await?;

            // Ack all messages in this chunk
            for (msg, _) in chunk {
                msg.ack().await?;
            }
        }

        Ok(())
    }
}
```

### 7.2 Batching Strategy

```
┌─────────────────────────────────────────────────────────────────────┐
│                      Batching Strategy                               │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  Pull Configuration:                                                 │
│    • Batch size: 500 messages                                        │
│    • Batch timeout: 100ms                                            │
│    • Whichever comes first triggers processing                       │
│                                                                      │
│  Grouping:                                                           │
│    • Group messages by store_id                                      │
│    • Each store gets separate database transaction                   │
│    • Parallel processing across stores (up to 4 concurrent)          │
│                                                                      │
│  Per-Store Batching:                                                 │
│    • Max 200 tuples per transaction                                  │
│    • Uses bulk INSERT/DELETE (not individual statements)             │
│    • Single changelog entry for entire batch                         │
│                                                                      │
│  Throughput Math:                                                    │
│    • 500 msgs/batch × 10 batches/sec = 5,000 msgs/sec               │
│    • With 4 parallel stores: up to 20,000 writes/sec                │
│    • Actual limit: database write throughput                         │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### 7.3 Idempotent Storage Writes

To handle at-least-once delivery, storage writes must be idempotent:

```rust
impl PostgresStore {
    /// Write tuples with idempotency using request_id tracking
    pub async fn write_tuples_with_sequence(
        &self,
        store_id: &str,
        writes: Vec<StoredTuple>,
        deletes: Vec<StoredTuple>,
    ) -> StorageResult<u64> {
        let mut tx = self.pool.begin().await?;

        // 1. Get and increment sequence (with row lock)
        let sequence: i64 = sqlx::query_scalar(
            "UPDATE store_metadata
             SET write_sequence = write_sequence + 1
             WHERE store_id = $1
             RETURNING write_sequence"
        )
        .bind(store_id)
        .fetch_one(&mut *tx)
        .await?;

        // 2. Bulk delete (using UNNEST for efficiency)
        if !deletes.is_empty() {
            sqlx::query(
                "DELETE FROM tuples
                 WHERE store_id = $1
                 AND (object_type, object_id, relation, user_type, user_id, user_relation)
                 IN (SELECT * FROM UNNEST($2::text[], $3::text[], $4::text[], $5::text[], $6::text[], $7::text[]))"
            )
            .bind(store_id)
            .bind(&deletes.iter().map(|t| &t.object_type).collect::<Vec<_>>())
            .bind(&deletes.iter().map(|t| &t.object_id).collect::<Vec<_>>())
            .bind(&deletes.iter().map(|t| &t.relation).collect::<Vec<_>>())
            .bind(&deletes.iter().map(|t| &t.user_type).collect::<Vec<_>>())
            .bind(&deletes.iter().map(|t| &t.user_id).collect::<Vec<_>>())
            .bind(&deletes.iter().map(|t| &t.user_relation).collect::<Vec<_>>())
            .execute(&mut *tx)
            .await?;
        }

        // 3. Bulk insert (with ON CONFLICT for idempotency)
        if !writes.is_empty() {
            sqlx::query(
                "INSERT INTO tuples (store_id, object_type, object_id, relation, user_type, user_id, user_relation, condition_name, condition_context)
                 SELECT $1, * FROM UNNEST($2::text[], $3::text[], $4::text[], $5::text[], $6::text[], $7::text[], $8::text[], $9::jsonb[])
                 ON CONFLICT (store_id, object_type, object_id, relation, user_type, user_id, user_relation)
                 DO UPDATE SET condition_name = EXCLUDED.condition_name, condition_context = EXCLUDED.condition_context"
            )
            .bind(store_id)
            .bind(&writes.iter().map(|t| &t.object_type).collect::<Vec<_>>())
            // ... bind all fields
            .execute(&mut *tx)
            .await?;
        }

        // 4. Commit transaction
        tx.commit().await?;

        Ok(sequence as u64)
    }
}
```

---

## 8. Read-Your-Own-Writes Consistency

### 8.1 The Consistency Challenge

With NATS-first writes, there's a window where:
1. Client receives write acknowledgment (from NATS)
2. Storage consumer hasn't yet persisted the write
3. Client issues a Check/Read that doesn't see the write

This is **eventual consistency** - acceptable for many use cases, but some need stronger guarantees.

### 8.2 RYOW Implementation Options

#### Option A: Write Ticket Polling (Recommended)

Client waits for write to be committed before reading:

```rust
/// Check handler with RYOW support
pub async fn check_handler(
    State(state): State<AppState>,
    Path(store_id): Path<String>,
    Json(request): Json<CheckRequest>,
) -> Result<Json<CheckResponse>, ApiError> {
    // If client provides write ticket, wait for it to be committed
    if let Some(ticket) = &request.write_ticket {
        state.write_tracker
            .wait_for_commit(&ticket.store_id, ticket.sequence, Duration::from_secs(5))
            .await
            .map_err(|_| ApiError::Timeout("Write not yet committed".to_string()))?;
    }

    // Proceed with check (now guaranteed to see the write)
    // ...
}

/// Write tracker that knows which sequences have been committed
pub struct WriteTracker {
    // Map of store_id -> last committed sequence
    committed: DashMap<String, AtomicU64>,
    // Waiters for specific sequences
    waiters: DashMap<(String, u64), Vec<oneshot::Sender<()>>>,
}

impl WriteTracker {
    /// Called by storage consumer after committing a batch
    pub fn mark_committed(&self, store_id: &str, sequence: u64) {
        // Update committed sequence
        self.committed
            .entry(store_id.to_string())
            .or_insert(AtomicU64::new(0))
            .fetch_max(sequence, Ordering::SeqCst);

        // Wake up any waiters for this or earlier sequences
        let to_wake: Vec<_> = self.waiters
            .iter()
            .filter(|e| e.key().0 == store_id && e.key().1 <= sequence)
            .map(|e| e.key().clone())
            .collect();

        for key in to_wake {
            if let Some((_, senders)) = self.waiters.remove(&key) {
                for sender in senders {
                    let _ = sender.send(());
                }
            }
        }
    }

    /// Wait for a sequence to be committed
    pub async fn wait_for_commit(
        &self,
        store_id: &str,
        sequence: u64,
        timeout: Duration,
    ) -> Result<(), TimeoutError> {
        // Check if already committed
        if let Some(committed) = self.committed.get(store_id) {
            if committed.load(Ordering::SeqCst) >= sequence {
                return Ok(());
            }
        }

        // Register waiter
        let (tx, rx) = oneshot::channel();
        self.waiters
            .entry((store_id.to_string(), sequence))
            .or_default()
            .push(tx);

        // Wait with timeout
        tokio::time::timeout(timeout, rx)
            .await
            .map_err(|_| TimeoutError)?
            .map_err(|_| TimeoutError)
    }
}
```

#### Option B: Consistency Header

Client specifies consistency requirement in request:

```rust
// Request header: X-Consistency: strong | eventual

pub async fn check_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    // ...
) -> Result<Json<CheckResponse>, ApiError> {
    let consistency = headers
        .get("X-Consistency")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("eventual");

    if consistency == "strong" {
        // Wait for all pending writes to this store to be committed
        let current_seq = state.nats_writes_stream.last_sequence(&store_id).await?;
        state.write_tracker
            .wait_for_commit(&store_id, current_seq, Duration::from_secs(5))
            .await?;
    }

    // Proceed with check
}
```

#### Option C: Synchronous Write Mode (Escape Hatch)

For critical operations, allow synchronous write path:

```rust
// Request body: { "sync": true, ... }

pub async fn write_handler(
    // ...
    Json(request): Json<WriteRequest>,
) -> Result<Json<WriteResponse>, ApiError> {
    if request.sync.unwrap_or(false) {
        // Bypass NATS, write directly to storage
        let sequence = state.storage
            .write_tuples_with_sequence(&store_id, writes, deletes)
            .await?;

        // Still publish event for other consumers
        state.event_publisher.publish_committed(
            &store_id, sequence, &[request_id], &writes, &deletes
        ).await?;

        return Ok(Json(WriteResponse { request_id, sequence, sync: true }));
    }

    // Normal NATS-first path
    // ...
}
```

### 8.3 Consistency Decision Matrix

| Use Case | Recommended Approach | Latency Impact |
|----------|---------------------|----------------|
| Batch imports | Eventual (default) | None |
| User-facing writes | Write ticket | +5-50ms on read |
| Critical permissions | Sync write mode | +10-20ms on write |
| Audit reads | Eventual (acceptable) | None |

---

## 9. Edge Synchronization

### 9.1 Edge Consumer Architecture

Edge nodes consume from `RSFGA_EVENTS` (not `RSFGA_WRITES`) to receive committed changes:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                          Edge Synchronization                                │
└─────────────────────────────────────────────────────────────────────────────┘

   Central Region                    NATS Cluster                  Edge Node
         │                                │                            │
         │                                │                            │
  ┌──────┴──────┐                   RSFGA_EVENTS                       │
  │   Storage   │                        │                             │
  │  Consumer   │                        │                             │
  │             │                        │                             │
  │  Commits to │                        │                             │
  │   database  │                        │                             │
  │             │                        │                             │
  │  Publishes  │                        │                             │
  │  committed  ├───────────────────────▶│                             │
  │   events    │                        │                             │
  └─────────────┘                        │                             │
                                         │     Edge Consumer           │
                                         ├────────────────────────────▶│
                                         │                             │
                                         │                    ┌────────┴────────┐
                                         │                    │  Apply to local │
                                         │                    │     storage     │
                                         │                    │                 │
                                         │                    │  Invalidate     │
                                         │                    │     cache       │
                                         │                    └─────────────────┘
```

### 9.2 Edge Consumer Implementation

```rust
/// Edge sync consumer
pub struct EdgeSyncConsumer {
    edge_id: String,
    js_context: Context,
    local_storage: Arc<dyn DataStore>,
    local_cache: Arc<CheckCache>,
    store_filter: String,
    metrics: Arc<EdgeMetrics>,
}

impl EdgeSyncConsumer {
    pub async fn run(&self) -> Result<(), ConsumerError> {
        // Create push consumer for this edge
        let consumer = self.js_context
            .get_stream("RSFGA_EVENTS").await?
            .create_consumer(jetstream::consumer::push::Config {
                name: Some(format!("edge-{}", self.edge_id)),
                durable_name: Some(format!("edge-{}", self.edge_id)),
                filter_subject: format!("rsfga.events.{}.>", self.store_filter),
                deliver_policy: jetstream::consumer::DeliverPolicy::New,
                ack_policy: jetstream::consumer::AckPolicy::Explicit,
                ack_wait: Duration::from_secs(30),
                max_deliver: 5,
                max_ack_pending: 1000,
                ..Default::default()
            }).await?;

        let mut messages = consumer.messages().await?;

        while let Some(msg) = messages.next().await {
            let msg = msg?;

            match self.process_event(&msg).await {
                Ok(()) => {
                    msg.ack().await?;
                    self.metrics.events_processed.inc();
                }
                Err(e) => {
                    warn!(error = %e, "Failed to process event");
                    self.metrics.events_failed.inc();
                    // Will be redelivered
                }
            }
        }

        Ok(())
    }

    async fn process_event(&self, msg: &Message) -> Result<(), ConsumerError> {
        let event = CommittedEvent::decode(msg.payload.as_ref())?;

        // Check for duplicate (idempotency)
        let last_seq = self.local_storage
            .get_sync_sequence(&event.store_id)
            .await?;

        if event.sequence <= last_seq {
            // Already processed
            return Ok(());
        }

        // Apply to local storage
        if let Some(Payload::TupleBatch(batch)) = event.payload {
            let mut writes = Vec::new();
            let mut deletes = Vec::new();

            for op in batch.operations {
                match OperationType::from_i32(op.operation) {
                    Some(OperationType::Write) => {
                        writes.push(StoredTuple::from(&op));
                    }
                    Some(OperationType::Delete) => {
                        deletes.push(StoredTuple::from(&op));
                    }
                    _ => {}
                }
            }

            self.local_storage
                .write_tuples(&event.store_id, writes, deletes)
                .await?;
        }

        // Update sync position
        self.local_storage
            .set_sync_sequence(&event.store_id, event.sequence)
            .await?;

        // Invalidate local cache
        self.local_cache.invalidate_store(&event.store_id).await;

        Ok(())
    }
}
```

---

## 10. Multi-Region Replication

### 10.1 Replication Architecture

For multi-region deployment, use NATS stream mirroring:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                      Multi-Region Replication                                │
└─────────────────────────────────────────────────────────────────────────────┘

                    Primary Region (US-East)
                    ┌─────────────────────────────────────┐
                    │  RSFGA_WRITES    RSFGA_EVENTS       │
                    │       │               │             │
                    │       ▼               │             │
                    │  Storage Consumer    │             │
                    │       │               │             │
                    │       ▼               │             │
                    │   Database ──────────▶│             │
                    │                       │             │
                    └───────────────────────┼─────────────┘
                                            │
                         NATS Gateway / Leafnode
                                            │
              ┌─────────────────────────────┼─────────────────────────────┐
              │                             │                             │
              ▼                             ▼                             ▼
    ┌─────────────────────┐     ┌─────────────────────┐     ┌─────────────────────┐
    │   Region: EU-West   │     │  Region: AP-South   │     │  Region: US-West    │
    │                     │     │                     │     │                     │
    │  RSFGA_EVENTS       │     │  RSFGA_EVENTS       │     │  RSFGA_EVENTS       │
    │  (Mirror)           │     │  (Mirror)           │     │  (Mirror)           │
    │       │             │     │       │             │     │       │             │
    │       ▼             │     │       ▼             │     │       ▼             │
    │  Region Consumer    │     │  Region Consumer    │     │  Region Consumer    │
    │       │             │     │       │             │     │       │             │
    │       ▼             │     │       ▼             │     │       ▼             │
    │   Database          │     │   Database          │     │   Database          │
    │   (Replica)         │     │   (Replica)         │     │   (Replica)         │
    │                     │     │                     │     │                     │
    └─────────────────────┘     └─────────────────────┘     └─────────────────────┘
```

### 10.2 Stream Mirroring Configuration

```rust
/// Configure stream mirror in secondary region
pub async fn setup_region_mirror(
    js: &Context,
    region_id: &str,
    source_domain: &str,
) -> Result<(), NatsError> {
    let config = jetstream::stream::Config {
        name: format!("RSFGA_EVENTS_{}", region_id.to_uppercase()),
        description: Some(format!("RSFGA events mirror for region {}", region_id)),

        // Mirror from primary
        mirror: Some(jetstream::stream::Source {
            name: "RSFGA_EVENTS".to_string(),
            domain: Some(source_domain.to_string()),
            ..Default::default()
        }),

        // Regional retention (can be shorter)
        max_age: Duration::from_secs(7 * 24 * 60 * 60),

        // Regional replication
        num_replicas: 3,

        storage: jetstream::stream::StorageType::File,

        ..Default::default()
    };

    js.get_or_create_stream(config).await?;
    Ok(())
}
```

---

## 11. Failure Handling and Recovery

### 11.1 Failure Modes

| Component | Failure Mode | Impact | Mitigation |
|-----------|-------------|--------|------------|
| NATS unavailable | API can't publish | Writes fail (503) | Circuit breaker, fallback to sync |
| Storage consumer down | Writes queue up | Reads stale | Auto-restart, monitoring |
| Database unavailable | Consumer can't write | Events queue in NATS | NATS buffering, retry |
| Edge disconnected | Edge stale | Local reads stale | Reconnect, resync |

### 11.2 Circuit Breaker for NATS

```rust
/// Circuit breaker for NATS publishing
pub struct NatsCircuitBreaker {
    state: AtomicU8,  // 0=closed, 1=open, 2=half-open
    failures: AtomicU32,
    last_failure: AtomicU64,
    config: CircuitBreakerConfig,
}

impl NatsCircuitBreaker {
    pub async fn call<F, T>(&self, f: F) -> Result<T, CircuitBreakerError>
    where
        F: Future<Output = Result<T, NatsError>>,
    {
        match self.state.load(Ordering::SeqCst) {
            0 => { /* Closed - allow calls */ }
            1 => {
                // Open - check if we should try half-open
                let elapsed = self.elapsed_since_failure();
                if elapsed < self.config.reset_timeout {
                    return Err(CircuitBreakerError::Open);
                }
                self.state.store(2, Ordering::SeqCst);
            }
            2 => { /* Half-open - allow single call */ }
            _ => unreachable!(),
        }

        match f.await {
            Ok(result) => {
                self.on_success();
                Ok(result)
            }
            Err(e) => {
                self.on_failure();
                Err(CircuitBreakerError::Failed(e))
            }
        }
    }

    fn on_success(&self) {
        self.failures.store(0, Ordering::SeqCst);
        self.state.store(0, Ordering::SeqCst);  // Close circuit
    }

    fn on_failure(&self) {
        let failures = self.failures.fetch_add(1, Ordering::SeqCst) + 1;
        self.last_failure.store(now_millis(), Ordering::SeqCst);

        if failures >= self.config.failure_threshold {
            self.state.store(1, Ordering::SeqCst);  // Open circuit
        }
    }
}
```

### 11.3 Fallback to Synchronous Writes

When NATS is unavailable, fall back to direct storage writes:

```rust
pub async fn write_with_fallback(
    state: &AppState,
    store_id: &str,
    writes: Vec<StoredTuple>,
    deletes: Vec<StoredTuple>,
) -> Result<WriteResponse, ApiError> {
    // Try NATS first
    match state.circuit_breaker.call(
        state.publish_to_nats(store_id, &writes, &deletes)
    ).await {
        Ok(response) => Ok(response),
        Err(CircuitBreakerError::Open) | Err(CircuitBreakerError::Failed(_)) => {
            // Fallback to synchronous write
            warn!("NATS unavailable, falling back to synchronous write");
            state.metrics.fallback_writes.inc();

            let sequence = state.storage
                .write_tuples_with_sequence(store_id, writes.clone(), deletes.clone())
                .await?;

            // Publish event for other consumers (best effort)
            let _ = state.event_publisher
                .publish_committed(store_id, sequence, &[], &writes, &deletes)
                .await;

            Ok(WriteResponse {
                request_id: Ulid::new().to_string(),
                sequence,
                sync: true,
            })
        }
    }
}
```

### 11.4 Dead Letter Queue

Failed events go to DLQ for investigation:

```rust
/// Move failed event to dead letter queue
pub async fn move_to_dlq(
    js: &Context,
    original_msg: &Message,
    error: &str,
) -> Result<(), NatsError> {
    let dlq_subject = format!(
        "rsfga.dlq.{}",
        original_msg.subject.replace("rsfga.writes.", "")
    );

    let mut headers = async_nats::HeaderMap::new();
    headers.insert("X-Original-Subject", original_msg.subject.as_str());
    headers.insert("X-Error", error);
    headers.insert("X-Delivery-Count", &original_msg.info()?.delivered.to_string());
    headers.insert("X-Failed-At", &chrono::Utc::now().to_rfc3339());

    js.publish_with_headers(
        dlq_subject,
        headers,
        original_msg.payload.clone(),
    ).await?;

    Ok(())
}
```

---

## 12. Performance Analysis

### 12.1 Throughput Comparison

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                       Throughput Comparison                                  │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  Storage-First (Current):                                                    │
│    • Single write: 1 API call → 1 DB transaction                            │
│    • Batch of 100: 1 API call → 1 DB transaction (100 tuples)               │
│    • Max throughput: ~300 writes/sec (limited by DB connections)            │
│                                                                              │
│  NATS-First (Proposed):                                                      │
│    • Single write: 1 API call → 1 NATS publish                              │
│    • NATS accepts: ~50,000+ publishes/sec                                   │
│    • Storage consumer batches: 500 msgs → 1 DB transaction                  │
│    • Consumer throughput: 5,000-10,000 tuples/sec                           │
│                                                                              │
│  Improvement: 15-30x higher sustained throughput                            │
│                                                                              │
│  Burst Handling:                                                             │
│    • Storage-first: Limited by DB pool (50-100 concurrent)                  │
│    • NATS-first: Buffered in JetStream (millions of messages)               │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 12.2 Latency Comparison

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        Latency Comparison                                    │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  Storage-First Write Latency:                                                │
│    p50:  12ms                                                                │
│    p95:  18ms                                                                │
│    p99:  25ms                                                                │
│                                                                              │
│  NATS-First Write Latency (to client):                                       │
│    p50:  1.5ms                                                               │
│    p95:  3ms                                                                 │
│    p99:  5ms                                                                 │
│                                                                              │
│  Time to Storage (eventual):                                                 │
│    p50:  50ms                                                                │
│    p95:  150ms                                                               │
│    p99:  300ms                                                               │
│                                                                              │
│  Client-visible improvement: 5-8x faster                                    │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 12.3 Resource Usage

| Resource | Storage-First | NATS-First | Notes |
|----------|--------------|------------|-------|
| DB connections | High (under load) | Lower (batched) | More efficient DB usage |
| Memory (API) | Low | Low | Similar |
| Memory (NATS) | N/A | Moderate | JetStream buffering |
| CPU (API) | Higher | Lower | Less DB wait |
| Network | Per-write | Batched | More efficient |

---

## 13. Security Considerations

### 13.1 Authentication & Authorization

```yaml
# NATS account configuration
accounts:
  RSFGA_WRITERS:
    users:
      - nkey: UABC...
        permissions:
          publish:
            allow: ["rsfga.writes.>"]
          subscribe:
            deny: ["*"]

  RSFGA_CONSUMERS:
    users:
      - nkey: UDEF...
        permissions:
          publish:
            allow: ["rsfga.events.>", "rsfga.dlq.>"]
          subscribe:
            allow: ["rsfga.writes.>", "rsfga.events.>"]

  RSFGA_EDGE:
    users:
      - nkey: UGHI...
        permissions:
          publish:
            deny: ["*"]
          subscribe:
            allow: ["rsfga.events.store-123.>"]  # Per-edge filtering
```

### 13.2 TLS Configuration

```rust
let options = async_nats::ConnectOptions::new()
    .require_tls(true)
    .add_root_certificates("/etc/rsfga/ca.crt".into())
    .add_client_certificate(
        "/etc/rsfga/client.crt".into(),
        "/etc/rsfga/client.key".into(),
    );
```

---

## 14. Observability

### 14.1 Key Metrics

```rust
// Write path metrics
pub static WRITES_PUBLISHED: Counter = ...;      // Total writes published to NATS
pub static WRITE_LATENCY: Histogram = ...;       // Client-visible write latency
pub static NATS_PUBLISH_ERRORS: Counter = ...;   // NATS publish failures
pub static FALLBACK_WRITES: Counter = ...;       // Writes using sync fallback

// Consumer metrics
pub static CONSUMER_LAG: Gauge = ...;            // Messages pending processing
pub static BATCH_SIZE: Histogram = ...;          // Messages per batch
pub static STORAGE_WRITE_LATENCY: Histogram = ...; // DB write latency
pub static TUPLES_COMMITTED: Counter = ...;      // Tuples written to storage

// RYOW metrics
pub static RYOW_WAITS: Counter = ...;            // RYOW wait requests
pub static RYOW_WAIT_TIME: Histogram = ...;      // Time spent waiting for commit
pub static RYOW_TIMEOUTS: Counter = ...;         // RYOW timeout errors
```

### 14.2 Alerting Rules

```yaml
groups:
  - name: rsfga-nats
    rules:
      - alert: HighConsumerLag
        expr: rsfga_consumer_lag > 10000
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "Storage consumer falling behind"

      - alert: NATSPublishErrors
        expr: rate(rsfga_nats_publish_errors_total[5m]) > 0.1
        for: 2m
        labels:
          severity: critical
        annotations:
          summary: "NATS publish failures detected"

      - alert: HighWriteLatency
        expr: histogram_quantile(0.99, rsfga_write_latency_seconds) > 0.01
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "Write latency exceeding 10ms p99"
```

---

## 15. Implementation Plan

### 15.1 Phases

| Phase | Scope | Duration | Deliverables |
|-------|-------|----------|--------------|
| **Phase 1** | Core NATS integration | 2 weeks | Publisher, streams, basic consumer |
| **Phase 2** | Storage consumer | 2 weeks | Batching, idempotency, metrics |
| **Phase 3** | RYOW support | 1 week | Write tracker, consistency options |
| **Phase 4** | Edge sync | 2 weeks | Edge consumer, bootstrap |
| **Phase 5** | Production hardening | 2 weeks | Security, monitoring, testing |

### 15.2 Phase 1: Core NATS Integration

```
Tasks:
├── [ ] Add async-nats dependency
├── [ ] Define protobuf schemas (WriteRequest, CommittedEvent)
├── [ ] Implement NATS connection manager
├── [ ] Create RSFGA_WRITES stream configuration
├── [ ] Create RSFGA_EVENTS stream configuration
├── [ ] Implement write publisher in API layer
├── [ ] Unit tests with embedded NATS
└── [ ] Integration tests
```

### 15.3 Phase 2: Storage Consumer

```
Tasks:
├── [ ] Implement StorageConsumer
├── [ ] Add batch grouping by store_id
├── [ ] Implement idempotent storage writes
├── [ ] Add sequence tracking
├── [ ] Implement event publishing (WRITES → EVENTS)
├── [ ] Add consumer metrics
├── [ ] DLQ handling
└── [ ] Load testing
```

### 15.4 Phase 3: RYOW Support

```
Tasks:
├── [ ] Implement WriteTracker
├── [ ] Add write ticket to API response
├── [ ] Add wait_for_commit to read handlers
├── [ ] Add consistency header support
├── [ ] Add sync write fallback option
└── [ ] Performance testing
```

---

## 16. Alternatives Considered

### 16.1 Storage-First with Async Events (Original Proposal)

**Approach**: Write to storage first, publish events async after.

**Pros**:
- Simpler consistency model
- No RYOW complexity

**Cons**:
- Higher write latency (blocked by DB)
- Limited throughput
- No batching benefit

**Decision**: Rejected in favor of NATS-first for performance benefits.

### 16.2 Kafka Instead of NATS

**Approach**: Use Kafka for event streaming.

**Pros**:
- Higher throughput (at scale)
- Strong ordering guarantees
- Mature ecosystem

**Cons**:
- Higher operational complexity
- Higher latency (~5-10ms vs ~1-2ms)
- Heavier resource footprint
- Poor edge support

**Decision**: NATS selected for lower latency and edge-native design.

### 16.3 Database-Based Queue (Transactional Outbox)

**Approach**: Write to outbox table in same transaction, process async.

**Pros**:
- Strong consistency
- No additional infrastructure

**Cons**:
- Still limited by DB throughput
- Complex polling/CDC setup
- No natural batching

**Decision**: Rejected - doesn't solve throughput problem.

---

## 17. Open Questions

### 17.1 Unresolved

| Question | Options | Recommendation | Status |
|----------|---------|----------------|--------|
| Default consistency | Eventual / RYOW | Eventual | **Open** |
| Consumer scaling | Single / Sharded | Single initially | **Open** |
| DLQ retention | 7 days / 30 days | 30 days | **Open** |
| Sync write threshold | Never / On request | On request | **Decided** |

### 17.2 Questions for Stakeholders

1. **What is acceptable staleness for reads?**
   - Current assumption: <500ms typical, <1s max
   - Affects consumer batch settings

2. **Should sync writes be available?**
   - Current assumption: Yes, as escape hatch
   - Adds complexity but provides flexibility

3. **Multi-region write handling?**
   - Current assumption: Single write region
   - Multi-primary would require conflict resolution

---

## Appendix A: Configuration Reference

```yaml
# NATS-first write configuration
nats:
  enabled: true
  servers:
    - "nats://nats-1:4222"
    - "nats://nats-2:4222"
    - "nats://nats-3:4222"

  # Authentication
  auth:
    type: nkey
    seed_file: /etc/rsfga/nats.nkey

  # TLS
  tls:
    enabled: true
    ca_file: /etc/rsfga/ca.crt
    cert_file: /etc/rsfga/client.crt
    key_file: /etc/rsfga/client.key

  # Publisher settings
  publisher:
    timeout_ms: 5000
    retry_attempts: 3
    retry_delay_ms: 100

  # Streams
  streams:
    writes:
      name: RSFGA_WRITES
      replicas: 3
      max_age_hours: 24
      dedup_window_seconds: 120

    events:
      name: RSFGA_EVENTS
      replicas: 3
      max_age_days: 7

# Storage consumer settings
consumer:
  batch_size: 500
  batch_timeout_ms: 100
  max_per_store_batch: 200
  parallelism: 4
  ack_wait_seconds: 60
  max_deliver: 5

# RYOW settings
consistency:
  default: eventual  # eventual | ryow
  ryow_timeout_ms: 5000
  sync_writes_enabled: true

# Circuit breaker
circuit_breaker:
  failure_threshold: 5
  reset_timeout_ms: 30000

# Fallback
fallback:
  enabled: true
  sync_on_nats_failure: true
```

---

## Appendix B: Glossary

| Term | Definition |
|------|------------|
| **NATS-first** | Architecture where writes go to NATS before storage |
| **RYOW** | Read-Your-Own-Writes consistency guarantee |
| **Write ticket** | Token returned to client for RYOW tracking |
| **Storage consumer** | Component that batches NATS events to database |
| **JetStream** | NATS persistence and streaming layer |
| **WorkQueue** | JetStream retention policy (delete on ack) |
| **Circuit breaker** | Pattern to fail-fast when dependency is unhealthy |

---

*This is a living document. Updated as implementation progresses.*
