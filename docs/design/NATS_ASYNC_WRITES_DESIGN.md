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
16. [Risk Identification](#16-risk-identification)
17. [Rollback Plan](#17-rollback-plan)
18. [Operational Runbook](#18-operational-runbook)
19. [Test Cases](#19-test-cases)
20. [Alternatives Considered](#20-alternatives-considered)
21. [Open Questions](#21-open-questions)

---

> **⚠️ SECURITY WARNING: Eventual Consistency and Permission Revocations**
>
> This design introduces an eventual consistency window where permission changes may not be immediately visible. **For permission REVOCATIONS (deletes), this means a revoked user may retain access for a brief window (typically 20-100ms).**
>
> **Critical Recommendations:**
> - **Use the sync `/write` endpoint for permission revocations** (DELETE operations)
> - **Use the async `/async/write` endpoint only for permission grants** (WRITE operations)
> - Consider the `async_allow_deletes: false` configuration to prevent accidental async revocations
> - For security-critical systems, always prefer the sync path
>
> See [Section 13: Security Considerations](#13-security-considerations) for detailed analysis.

---

## 1. Executive Summary

This document proposes a **NATS-first write architecture** for RSFGA where writes go to NATS JetStream first, then are consumed and batched into storage. This inverts the traditional pattern for significant benefits:

```text
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

| Metric | Storage-First | NATS-First | Improvement | Confidence |
|--------|---------------|------------|-------------|------------|
| Write Latency (p99) | 15-20ms | 2-5ms | **4-10x faster** | 60% - requires validation |
| Throughput (writes/sec) | 150-300 | 5,000-10,000 | **20-50x higher** | 60% - requires validation |
| Burst Handling | Limited by DB | JetStream buffer | **Much better** | 80% - based on NATS benchmarks |
| Multi-consumer | Complex (CDC) | Native | **Simpler** | 90% - architectural |

> **Note**: Performance claims marked with confidence levels per project standards (ADR methodology).
> Validation will occur in Milestone 2.0.3 with benchmarking against baseline.

### Security Consideration: Eventual Consistency Window

**⚠️ IMPORTANT**: The async path introduces an eventual consistency window where:

1. **Permission grants** are not immediately visible (~50-300ms typical)
2. **Permission revocations** continue to be honored during this window

**Security Analysis**:

| Scenario | Risk | Mitigation |
|----------|------|------------|
| Grant delay | User cannot access resource for ~300ms | Acceptable UX trade-off |
| Revoke delay | Revoked user may access for ~300ms | **Security concern** - see below |

**Revocation Window Mitigation Options**:

1. **Use sync path for revocations** (Recommended)
   - DELETE operations use original sync endpoint
   - Only writes (grants) use async path
   - Zero revocation delay

2. **RYOW for sensitive operations**
   - Client uses write ticket for critical checks
   - Adds latency but ensures consistency

3. **Accept the window** (Not recommended for sensitive systems)
   - Appropriate for non-critical authorization
   - Document in API guidelines

**Recommendation**: Applications should use the **sync `/write` endpoint for permission revocations** and the **async `/async/write` endpoint for permission grants**. This provides the performance benefits of async writes while maintaining security for revocations.

---

## 1.1 OpenFGA API Compatibility Statement

> **IMPORTANT: The async API is an EXTENSION, not a replacement for OpenFGA API**

This design maintains **100% OpenFGA API compatibility**:

| Endpoint | Behavior | OpenFGA Compatible |
|----------|----------|-------------------|
| `POST /stores/{id}/write` | Synchronous storage write, unchanged | ✅ Yes (identical) |
| `POST /stores/{id}/check` | Read from storage, unchanged | ✅ Yes (identical) |
| `POST /async/stores/{id}/write` | **NEW** - NATS-first write | ➕ Extension |

**Key Compatibility Guarantees**:

1. **Original endpoints unchanged**: The existing `/write`, `/check`, `/read`, and all other OpenFGA endpoints remain 100% compatible with identical request/response formats
2. **Drop-in replacement preserved**: Applications using only OpenFGA-compatible endpoints see no behavioral changes
3. **Extension, not modification**: The `/async/*` endpoints are **new additions** that clients explicitly opt into
4. **Backward compatible**: Clients unaware of async endpoints continue working exactly as before

**Why separate endpoints?**

```text
✅ Recommended: Separate /async endpoints
─────────────────────────────────────────
• Clear client opt-in to eventual consistency
• No changes to existing endpoint behavior
• Easy A/B testing and gradual migration
• Instant rollback by using original endpoint
• 100% OpenFGA test suite compatibility

❌ Rejected: Modify existing /write endpoint
─────────────────────────────────────────
• Breaks OpenFGA behavioral compatibility
• Clients unaware of consistency change
• Difficult to rollback
• Test suite failures
```

**Migration is Optional**: Organizations can use async endpoints for performance-critical write-heavy workloads while keeping sync endpoints for security-critical operations like permission revocations.

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

```text
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

```text
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
    Storage latency: ├────────────────── 20-100ms (batched) ─────────┤
```

### 4.2 Component Overview

```text
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

### 4.3 Deployment Architecture: Separate Daemons

The key architectural decision is that **consumers run as separate daemons**, not embedded in the API server:

```text
┌─────────────────────────────────────────────────────────────────────────────┐
│                        Deployment Architecture                               │
└─────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────┐
│                         Process: rsfga-server                                │
│                         (Existing API Server)                                │
│                                                                              │
│  • Minimal changes to existing code                                          │
│  • Add NATS publisher (optional, feature-flagged)                            │
│  • Keep original storage write path (fallback)                               │
│  • Config flag: nats.write_mode = "nats" | "direct" | "auto"                │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    │ Publish to NATS (when enabled)
                                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                           NATS JetStream                                     │
└─────────────────────────────────────────────────────────────────────────────┘
                                    │
              ┌─────────────────────┼─────────────────────┐
              ▼                     ▼                     ▼
┌──────────────────────┐ ┌──────────────────────┐ ┌──────────────────────┐
│ Process: rsfga-writer│ │ Process: rsfga-edge  │ │ Process: rsfga-sync  │
│ (Storage Consumer)   │ │ (Edge Sync Daemon)   │ │ (Region Replica)     │
│                      │ │                      │ │                      │
│ • Separate binary    │ │ • Separate binary    │ │ • Separate binary    │
│ • Stateless          │ │ • Runs at edge       │ │ • Runs in secondary  │
│ • Horizontally scale │ │ • Local storage      │ │   region             │
│ • Can run multiple   │ │                      │ │                      │
└──────────────────────┘ └──────────────────────┘ └──────────────────────┘
```

#### Benefits of Separate Daemons

| Benefit | Description |
|---------|-------------|
| **Minimal API changes** | API server only adds NATS publish; existing write path unchanged |
| **Easy fallback** | Config flag switches between NATS and direct write instantly |
| **Independent scaling** | Scale writers separately from API servers |
| **Isolated failures** | Writer crash doesn't affect API availability |
| **Gradual rollout** | Run both paths in parallel during migration |
| **Simpler testing** | Test writer daemon independently |

#### API Design: Separate Async Endpoints

Instead of modifying existing endpoints, we create **parallel async API endpoints** that mirror the original:

```text
┌─────────────────────────────────────────────────────────────────────────────┐
│                          API Endpoint Structure                              │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  Original API (client-visible behavior unchanged):                           │
│  ┌────────────────────────────────────────────────────────────────────────┐ │
│  │  POST /stores/{store_id}/write           → Storage + publish event      │ │
│  │  POST /stores/{store_id}/write-model     → Storage + publish event      │ │
│  │  POST /stores                            → Storage + publish event      │ │
│  │  DELETE /stores/{store_id}               → Storage + publish event      │ │
│  │                                                                        │ │
│  │  Note: Event publishing is internal (fire-and-forget after commit).    │ │
│  │  Request/response format is 100% OpenFGA compatible - no client change.│ │
│  └────────────────────────────────────────────────────────────────────────┘ │
│                                                                              │
│  Async API (NEW - NATS-first):                                               │
│  ┌────────────────────────────────────────────────────────────────────────┐ │
│  │  POST /async/stores/{store_id}/write     → Publish to NATS              │ │
│  │  POST /async/stores/{store_id}/write-model → Publish to NATS            │ │
│  │  POST /async/stores                      → Publish to NATS              │ │
│  │  DELETE /async/stores/{store_id}         → Publish to NATS              │ │
│  └────────────────────────────────────────────────────────────────────────┘ │
│                                                                              │
│  Read APIs (unchanged - always read from storage):                           │
│  ┌────────────────────────────────────────────────────────────────────────┐ │
│  │  POST /stores/{store_id}/check           → Read from storage            │ │
│  │  POST /stores/{store_id}/list-objects    → Read from storage            │ │
│  │  GET  /stores/{store_id}/read            → Read from storage            │ │
│  │  GET  /stores/{store_id}/changes         → Read from storage            │ │
│  └────────────────────────────────────────────────────────────────────────┘ │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

**Benefits of Separate `/async` Endpoints:**

| Benefit | Description |
|---------|-------------|
| **Client-visible API unchanged** | Same request/response format, 100% OpenFGA compatible |
| **Internal enhancement only** | Sync path adds event publishing (non-blocking, after commit) |
| **Client chooses** | Application decides sync vs async per request |
| **Easy A/B testing** | Route percentage of traffic to async |
| **Instant rollback** | Just use original endpoint |
| **Clear semantics** | `/async` signals eventual consistency |
| **OpenFGA compatible** | Original endpoints remain 100% compatible |

#### Async API Response

The async endpoint returns a **write ticket** for optional RYOW:

```rust
// Original endpoint response (unchanged)
// POST /stores/{store_id}/write
{
  // Empty response per OpenFGA spec
}

// Async endpoint response (NEW)
// POST /async/stores/{store_id}/write
{
  "request_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
  "sequence": 12345,
  "write_ticket": {
    "store_id": "store-abc",
    "sequence": 12345
  }
}
```

#### Using Write Ticket for RYOW

```rust
// Client can optionally wait for write to be committed
// POST /stores/{store_id}/check
{
  "tuple_key": { ... },
  "write_ticket": {           // Optional: wait for this write first
    "store_id": "store-abc",
    "sequence": 12345
  }
}
```

#### Event Publishing: Both Paths Need It

**Critical**: Both sync and async paths must publish to `RSFGA_EVENTS` for data consistency:

```text
┌─────────────────────────────────────────────────────────────────────────────┐
│                     Event Flow for Both Paths                                │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  SYNC PATH (Original API):                                                   │
│  ┌────────────────────────────────────────────────────────────────────────┐ │
│  │  Client → Storage (write) → RSFGA_EVENTS (publish) → Response          │ │
│  │                                    │                                    │ │
│  │                                    ├──▶ Edge Sync                       │ │
│  │                                    ├──▶ Cache Invalidation              │ │
│  │                                    └──▶ Region Replication              │ │
│  └────────────────────────────────────────────────────────────────────────┘ │
│                                                                              │
│  ASYNC PATH (New /async API):                                                │
│  ┌────────────────────────────────────────────────────────────────────────┐ │
│  │  Client → RSFGA_WRITES (publish) → Response                            │ │
│  │                  │                                                      │ │
│  │                  ▼                                                      │ │
│  │           rsfga-writer → Storage (batch) → RSFGA_EVENTS (publish)      │ │
│  │                                                  │                      │ │
│  │                                                  ├──▶ Edge Sync         │ │
│  │                                                  ├──▶ Cache Invalidation│ │
│  │                                                  └──▶ Region Replication│ │
│  └────────────────────────────────────────────────────────────────────────┘ │
│                                                                              │
│  Both paths publish to RSFGA_EVENTS after storage commit!                    │
│  This ensures all downstream consumers see ALL changes.                      │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

**Why sync path must also publish events:**

| Consumer | Needs Events Because |
|----------|---------------------|
| Edge nodes | Must see all writes to stay in sync |
| Cache invalidation | Distributed cache must invalidate on all nodes |
| Region replicas | Secondary regions need all changes |
| Audit/Analytics | Must capture all changes regardless of path |

#### Implementation

```rust
// routes.rs - Add async routes alongside existing ones

pub fn routes() -> Router {
    Router::new()
        // Original write endpoints (now also publishes events)
        .route("/stores/:store_id/write", post(write_tuples_handler))
        .route("/stores/:store_id/write-model", post(write_model_handler))

        // NEW: Async write endpoints (NATS-first)
        .route("/async/stores/:store_id/write", post(async_write_tuples_handler))
        .route("/async/stores/:store_id/write-model", post(async_write_model_handler))

        // Read endpoints (unchanged)
        .route("/stores/:store_id/check", post(check_handler))
        // ...
}

// Original handler - NOW publishes events after storage commit
pub async fn write_tuples_handler(/* ... */) -> Result<Json<()>, ApiError> {
    // Validation (unchanged)
    validate_write_request(&request)?;
    let model = get_cached_model(&store_id).await?;
    validate_tuples_against_model(&request, &model)?;

    // Write to storage (unchanged)
    let sequence = state.storage
        .write_tuples_with_sequence(&store_id, writes.clone(), deletes.clone())
        .await?;

    // NEW: Publish event to RSFGA_EVENTS for downstream consumers
    // This is fire-and-forget (spawn), doesn't block response
    let event_publisher = state.event_publisher.clone();
    let store_id_clone = store_id.clone();
    tokio::spawn(async move {
        if let Err(e) = event_publisher.publish_committed(
            &store_id_clone,
            sequence,
            &writes,
            &deletes,
        ).await {
            // Log but don't fail - storage write already succeeded
            tracing::warn!("Failed to publish event: {}", e);
            metrics::EVENT_PUBLISH_FAILURES.inc();
        }
    });

    // Original empty response (OpenFGA compatible)
    Ok(Json(()))
}

// NEW: Async handler - publishes to RSFGA_WRITES
pub async fn async_write_tuples_handler(/* ... */) -> Result<Json<AsyncWriteResponse>, ApiError> {
    // Same validation as original
    validate_write_request(&request)?;
    let model = get_cached_model(&store_id).await?;
    validate_tuples_against_model(&request, &model)?;

    // SECURITY: Block async deletes if configured (recommended for security-sensitive systems)
    // Revocations should use sync path to avoid eventual consistency window
    if !state.config.async_allow_deletes && request.has_deletes() {
        return Err(ApiError::BadRequest(
            "DELETE operations not allowed on async endpoint. Use /write for revocations.".into()
        ));
    }

    // Publish to RSFGA_WRITES (rsfga-writer will handle storage + events)
    let ack = state.nats.publish_write_request(&store_id, &request).await?;

    Ok(Json(AsyncWriteResponse {
        request_id: ack.request_id,
        sequence: ack.sequence,
        write_ticket: Some(WriteTicket {
            store_id: store_id.clone(),
            sequence: ack.sequence,
        }),
    }))
}
```

#### Event Publisher (Shared by Both Paths)

```rust
/// Publishes committed events to RSFGA_EVENTS stream
pub struct EventPublisher {
    js_context: Context,
    node_id: String,
}

impl EventPublisher {
    /// Publish a committed event (called by both sync handler and rsfga-writer)
    pub async fn publish_committed(
        &self,
        store_id: &str,
        sequence: u64,
        writes: &[StoredTuple],
        deletes: &[StoredTuple],
    ) -> Result<(), NatsError> {
        let event = CommittedEvent {
            event_id: ulid::Ulid::new().to_string(),
            store_id: store_id.to_string(),
            sequence,
            committed_at: Some(SystemTime::now().into()),
            source_node_id: self.node_id.clone(),
            payload: Some(Payload::TupleBatch(TupleBatchCommitted {
                operations: build_operations(writes, deletes),
            })),
        };

        let subject = format!("rsfga.events.{}.tuple", store_id);

        self.js_context
            .publish(subject, event.encode_to_vec().into())
            .await?
            .await?;

        Ok(())
    }
}
```

#### Migration Path

```
Phase 1: Deploy
  • Deploy rsfga-writer daemon
  • Add /async endpoints to rsfga-server
  • Original API unchanged

Phase 2: Test
  • Route internal/test traffic to /async
  • Monitor latency, throughput, consistency
  • Validate with RYOW tickets

Phase 3: Migrate
  • Update clients to use /async endpoints
  • Keep original endpoints for compatibility
  • Monitor fallback metrics

Phase 4: (Optional) Deprecate
  • If desired, deprecate original write endpoints
  • Or keep both forever for flexibility
```

#### Binary Structure

```
rsfga/
├── rsfga-server       # Existing API server (minimal changes)
├── rsfga-writer       # NEW: Storage consumer daemon
├── rsfga-edge         # NEW: Edge sync daemon
└── rsfga-sync         # NEW: Region replica daemon
```

Each daemon is:
- **Stateless**: All state in NATS/Database
- **Restartable**: Resume from last NATS consumer position
- **Horizontally scalable**: Run multiple instances with consumer groups
- **Independently deployable**: Update without touching API server

### 4.4 Two-Stream Design

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

```text
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

```text
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

### 6.4 Stream Capacity Limits and Behavior

The RSFGA_WRITES stream has explicit bounds to prevent unbounded resource usage:

| Limit | Default Value | Purpose |
|-------|---------------|---------|
| `max_messages` | 10,000,000 | Maximum messages in stream |
| `max_bytes` | 10 GB | Maximum total stream size |
| `max_age` | 24 hours | Maximum message retention |
| `max_msg_size` | 1 MB | Maximum single message size |

**What happens when limits are reached?**

The stream uses `DiscardPolicy::Old`, which means:

```text
┌─────────────────────────────────────────────────────────────────────────────┐
│                    Stream Capacity Behavior                                  │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  NORMAL OPERATION (stream below limits):                                     │
│  ┌────────────────────────────────────────────────────────────────────────┐ │
│  │  Client publish → Accept message → ACK to client                       │ │
│  │  Consumer pulls → Process → ACK → Message deleted (WorkQueue)          │ │
│  └────────────────────────────────────────────────────────────────────────┘ │
│                                                                              │
│  AT CAPACITY (max_messages or max_bytes reached):                            │
│  ┌────────────────────────────────────────────────────────────────────────┐ │
│  │  DiscardPolicy::Old behavior:                                          │ │
│  │  • OLDEST unprocessed messages are DISCARDED to make room              │ │
│  │  • New message is accepted                                              │ │
│  │  • ⚠️ DATA LOSS: Discarded writes are permanently lost                 │ │
│  └────────────────────────────────────────────────────────────────────────┘ │
│                                                                              │
│  ALTERNATIVE: DiscardPolicy::New (safer but blocks clients):                 │
│  ┌────────────────────────────────────────────────────────────────────────┐ │
│  │  • New messages are REJECTED with error                                 │ │
│  │  • Client receives 503 Service Unavailable                             │ │
│  │  • No data loss, but write requests fail                               │ │
│  └────────────────────────────────────────────────────────────────────────┘ │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

**Configuration Recommendations**:

| Deployment Size | max_messages | max_bytes | Rationale |
|-----------------|--------------|-----------|-----------|
| Small (dev/test) | 100,000 | 1 GB | Limited resources |
| Medium (production) | 10,000,000 | 10 GB | Default - good balance |
| Large (high-throughput) | 100,000,000 | 100 GB | Very high burst capacity |

**Capacity Planning Formula**:

```text
Required capacity = (peak_writes_per_sec × max_consumer_lag_seconds × avg_msg_size)

Example:
  Peak: 10,000 writes/sec
  Max acceptable lag: 60 seconds
  Avg message size: 500 bytes

  Capacity needed: 10,000 × 60 × 500 = 300 MB minimum
  Recommended: 10x headroom = 3 GB
```

**Monitoring and Alerts**:

```yaml
# Prometheus alerting rules
groups:
  - name: rsfga_nats_capacity
    rules:
      - alert: RSFGAWriteStreamNearCapacity
        expr: |
          jetstream_stream_messages{stream="RSFGA_WRITES"}
          / jetstream_stream_max_messages{stream="RSFGA_WRITES"} > 0.8
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "RSFGA_WRITES stream at {{ $value | humanizePercentage }} capacity"
          description: "Stream may start discarding old messages. Scale consumers or increase limits."

      - alert: RSFGAWriteStreamAtCapacity
        expr: |
          jetstream_stream_messages{stream="RSFGA_WRITES"}
          / jetstream_stream_max_messages{stream="RSFGA_WRITES"} > 0.95
        for: 1m
        labels:
          severity: critical
        annotations:
          summary: "RSFGA_WRITES stream at critical capacity"
          description: "Messages being discarded. Immediate action required."
```

**Discard Policy Selection**:

| Use Case | Recommended Policy | Trade-off |
|----------|-------------------|-----------|
| Best-effort writes (analytics) | `DiscardPolicy::Old` | Accept data loss to maintain throughput |
| **Authorization (default)** | `DiscardPolicy::New` | Reject writes to prevent silent data loss |
| Backpressure to clients | `DiscardPolicy::New` | Clients retry, no data loss |

**IMPORTANT**: For authorization systems where write loss is unacceptable, consider changing to `DiscardPolicy::New` and handling the resulting 503 errors in the API layer with fallback to synchronous writes:

```rust
// Recommended: Fallback to sync write on NATS capacity error
async fn async_write_with_fallback(/* ... */) -> Result<WriteResponse, ApiError> {
    match nats.publish_write_request(&store_id, &request).await {
        Ok(ack) => Ok(async_response(ack)),
        Err(NatsError::StreamFull) => {
            // Fallback to synchronous write
            metrics::NATS_FALLBACK_WRITES.inc();
            storage.write_tuples(&store_id, &writes, &deletes).await?;
            Ok(sync_response())
        }
        Err(e) => Err(ApiError::from(e)),
    }
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
            // Prioritize low latency over batch size for shorter consistency window
            batch_size: 100,                              // Smaller batches = faster processing
            batch_timeout: Duration::from_millis(20),     // 20ms max wait (was 100ms)
            max_per_store_batch: 100,                     // Smaller DB transactions
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

```text
┌─────────────────────────────────────────────────────────────────────┐
│                      Batching Strategy                               │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  Pull Configuration (optimized for low latency):                     │
│    • Batch size: 100 messages                                        │
│    • Batch timeout: 20ms                                             │
│    • Whichever comes first triggers processing                       │
│    • Prioritizes consistency window over throughput                  │
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
│  Throughput Math (with low-latency settings):                        │
│    • 100 msgs/batch × 50 batches/sec = 5,000 msgs/sec               │
│    • With 4 parallel stores: up to 20,000 writes/sec                │
│    • Actual limit: database write throughput                         │
│    • Trade-off: Slightly more DB transactions for shorter latency    │
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

#### Database-Specific Bulk Operations

The above example uses PostgreSQL's `UNNEST` syntax. Other backends require different approaches:

| Database | Bulk Insert Strategy | Bulk Delete Strategy |
|----------|---------------------|---------------------|
| **PostgreSQL** | `UNNEST` arrays | `IN (SELECT FROM UNNEST)` |
| **CockroachDB** | Same as PostgreSQL | Same as PostgreSQL |
| **MySQL/MariaDB** | Multi-value `INSERT` | `IN` with value list |
| **SQLite** | Multi-value `INSERT` | `IN` with value list |

**MySQL Example:**

```sql
-- Bulk insert (MySQL)
INSERT INTO tuples (store_id, object_type, object_id, relation, ...)
VALUES
  ('store-1', 'document', 'doc1', 'viewer', ...),
  ('store-1', 'document', 'doc2', 'viewer', ...),
  ...
ON DUPLICATE KEY UPDATE
  condition_name = VALUES(condition_name),
  condition_context = VALUES(condition_context);

-- Bulk delete (MySQL)
DELETE FROM tuples
WHERE store_id = 'store-1'
  AND (object_type, object_id, relation, user_type, user_id, user_relation) IN (
    ('document', 'doc1', 'viewer', 'user', 'alice', ''),
    ('document', 'doc2', 'viewer', 'user', 'bob', '')
  );
```

The storage consumer implementation should detect the backend type and use the appropriate bulk operation syntax.

### 7.6 Idempotency Guarantees and Edge Cases

The storage consumer must handle idempotency correctly to prevent data inconsistencies when messages are redelivered.

#### Idempotency Key Strategy

Idempotency is based on the **tuple key** `(user, relation, object)`, NOT the request_id:

```rust
// Tuple key for idempotency
pub struct TupleKey {
    pub user: String,      // e.g., "user:alice"
    pub relation: String,  // e.g., "viewer"
    pub object: String,    // e.g., "document:readme"
}
```

#### Edge Cases

| Scenario | Behavior | Rationale |
|----------|----------|-----------|
| Same tuple written twice | Second write is no-op (ON CONFLICT) | Idempotent by design |
| Tuple with different condition | **Update** the condition | Condition is metadata, not key |
| Write then delete same tuple | Delete wins (last-write-wins) | Ordering by NATS sequence |
| Delete then write same tuple | Write wins (last-write-wins) | Ordering by NATS sequence |
| Partial batch failure | Rollback entire store batch | Transactional consistency |
| Consumer processes before ack | Re-process on restart (idempotent) | At-least-once delivery |

#### Condition Changes

When the same tuple is written with a **different CEL condition**, the behavior is UPDATE:

```sql
-- PostgreSQL: Update condition on conflict
INSERT INTO tuples (user, relation, object, condition, condition_context)
VALUES ('user:alice', 'viewer', 'document:readme', 'context.ip_range == "10.0.0.0/8"', '{}')
ON CONFLICT (store_id, user, relation, object)
DO UPDATE SET
  condition = EXCLUDED.condition,
  condition_context = EXCLUDED.condition_context,
  updated_at = NOW();

-- MySQL: Same behavior with ON DUPLICATE KEY
INSERT INTO tuples (...)
VALUES (...)
ON DUPLICATE KEY UPDATE
  condition = VALUES(condition),
  condition_context = VALUES(condition_context),
  updated_at = NOW();
```

#### Last-Write-Wins Semantics (Detailed)

**Critical**: The storage consumer processes messages in **NATS sequence order**, ensuring deterministic last-write-wins behavior.

**Same Tuple, Different Conditions**:

```text
Scenario: Two writes for same tuple with different conditions
┌─────────────────────────────────────────────────────────────────────────┐
│ Time    │ Message              │ NATS Seq │ DB Result                   │
├─────────────────────────────────────────────────────────────────────────┤
│ T1      │ WRITE alice→doc:1    │ 100      │ INSERT (condition: none)    │
│         │ condition: none      │          │                             │
├─────────────────────────────────────────────────────────────────────────┤
│ T2      │ WRITE alice→doc:1    │ 101      │ UPDATE (condition: ip_check)│
│         │ condition: ip_check  │          │                             │
├─────────────────────────────────────────────────────────────────────────┤
│ Result  │ Final state: alice→doc:1 WITH condition: ip_check             │
└─────────────────────────────────────────────────────────────────────────┘

Scenario: Write then Delete then Write (same tuple)
┌─────────────────────────────────────────────────────────────────────────┐
│ NATS Seq │ Operation            │ DB Result                             │
├─────────────────────────────────────────────────────────────────────────┤
│ 100      │ WRITE alice→doc:1    │ INSERT                                │
│ 101      │ DELETE alice→doc:1   │ DELETE                                │
│ 102      │ WRITE alice→doc:1    │ INSERT (tuple exists again)           │
├─────────────────────────────────────────────────────────────────────────┤
│ Result   │ Final state: alice→doc:1 EXISTS (seq 102 is last write)      │
└─────────────────────────────────────────────────────────────────────────┘
```

**Important Guarantees**:

1. **Ordering**: Consumer processes in strict NATS sequence order
2. **Atomicity**: Each batch is a single DB transaction
3. **Determinism**: Replay produces identical results
4. **No lost updates**: Conditions are updated, not ignored

**Edge Case: Concurrent Batches for Same Tuple**:

```rust
// Consumer ensures ordering within batches
async fn process_batch(&self, messages: Vec<Message>) -> Result<()> {
    // Sort by sequence to ensure deterministic ordering
    let mut sorted = messages;
    sorted.sort_by_key(|m| m.info().unwrap().stream_sequence);

    // Group by store, maintaining order
    let grouped = group_by_store_ordered(&sorted);

    // Process each store's messages in sequence order
    for (store_id, store_messages) in grouped {
        self.process_store_batch_ordered(&store_id, store_messages).await?;
    }
}
```

#### Message Redelivery

If the consumer crashes after writing to storage but before acknowledging NATS:

1. NATS redelivers the message (at-least-once)
2. Consumer attempts to write again
3. `ON CONFLICT` / `ON DUPLICATE KEY` makes write idempotent
4. Event is published again (downstream consumers must also be idempotent)

```text
┌─────────────────────────────────────────────────────────────┐
│ Redelivery Scenario                                          │
│                                                              │
│ 1. Consumer pulls message                                    │
│ 2. Consumer writes to DB ✓                                   │
│ 3. Consumer crashes before ack ✗                             │
│ 4. NATS redelivers after ack_wait timeout                    │
│ 5. Consumer writes again (idempotent - no duplicate)         │
│ 6. Consumer acks message ✓                                   │
└─────────────────────────────────────────────────────────────┘
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
            .map_err(|_| {
                // IMPORTANT: Return 504 Gateway Timeout, NOT a false authorization result
                // Returning false would be a security issue (denying access incorrectly)
                // Client should retry or fall back to sync path
                ApiError::GatewayTimeout("RYOW timeout: write not yet committed".to_string())
            })?;
    }

    // Proceed with check (now guaranteed to see the write)
    // ...
}

// HTTP Status Codes for RYOW:
// - 200 OK: Write committed, check proceeded normally
// - 504 Gateway Timeout: Write not committed within timeout (client should retry)
// - NOT 403/false: Never return authorization denial due to timeout

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

### 8.4 WriteTracker Cleanup Strategy

The WriteTracker must manage memory efficiently to prevent leaks from indefinitely tracking sequences.

**Cleanup Triggers**:

```rust
pub struct WriteTracker {
    // Map of store_id -> last committed sequence
    committed: DashMap<String, AtomicU64>,
    // Waiters for specific sequences (potential memory leak source)
    waiters: DashMap<(String, u64), Vec<oneshot::Sender<()>>>,
    // Configuration
    config: WriteTrackerConfig,
}

#[derive(Clone)]
pub struct WriteTrackerConfig {
    /// Maximum sequences to track per store before cleanup
    pub max_tracked_sequences_per_store: usize,  // Default: 10,000
    /// Time-to-live for waiter entries
    pub waiter_ttl: Duration,                     // Default: 60s
    /// Cleanup interval
    pub cleanup_interval: Duration,               // Default: 30s
    /// Maximum total memory for waiters
    pub max_waiter_memory_mb: usize,              // Default: 100MB
}

impl Default for WriteTrackerConfig {
    fn default() -> Self {
        Self {
            max_tracked_sequences_per_store: 10_000,
            waiter_ttl: Duration::from_secs(60),
            cleanup_interval: Duration::from_secs(30),
            max_waiter_memory_mb: 100,
        }
    }
}
```

**Cleanup Logic**:

```rust
impl WriteTracker {
    /// Start background cleanup task
    pub fn start_cleanup_task(self: Arc<Self>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(self.config.cleanup_interval);
            loop {
                interval.tick().await;
                self.cleanup_expired_waiters().await;
                self.cleanup_old_sequences().await;
            }
        })
    }

    /// Remove waiters that have exceeded TTL
    async fn cleanup_expired_waiters(&self) {
        let now = Instant::now();
        let expired: Vec<_> = self.waiters
            .iter()
            .filter(|e| e.value().created_at + self.config.waiter_ttl < now)
            .map(|e| e.key().clone())
            .collect();

        for key in expired {
            if let Some((_, waiters)) = self.waiters.remove(&key) {
                // Notify waiters with timeout error
                for sender in waiters.senders {
                    let _ = sender.send(Err(WaitError::Timeout));
                }
                self.metrics.waiters_expired.inc_by(waiters.senders.len() as u64);
            }
        }
    }

    /// Prune old committed sequences (keep only recent window)
    async fn cleanup_old_sequences(&self) {
        for mut entry in self.committed.iter_mut() {
            let store_id = entry.key();
            let current_seq = entry.value().load(Ordering::Relaxed);

            // Remove waiters for sequences already committed
            let committed_waiters: Vec<_> = self.waiters
                .iter()
                .filter(|e| e.key().0 == *store_id && e.key().1 <= current_seq)
                .map(|e| e.key().clone())
                .collect();

            for key in committed_waiters {
                self.waiters.remove(&key);
            }
        }
    }

    /// Emergency cleanup if memory threshold exceeded
    fn check_memory_pressure(&self) -> bool {
        let estimated_memory = self.waiters.len() * std::mem::size_of::<WaiterEntry>();
        estimated_memory > self.config.max_waiter_memory_mb * 1024 * 1024
    }
}
```

**Metrics for Monitoring Cleanup**:

```rust
// WriteTracker cleanup metrics
pub static WAITERS_ACTIVE: Gauge = ...;          // Current active waiters
pub static WAITERS_EXPIRED: Counter = ...;        // Waiters expired by TTL
pub static WAITERS_SATISFIED: Counter = ...;      // Waiters satisfied by commit
pub static CLEANUP_RUNS: Counter = ...;           // Cleanup task runs
pub static MEMORY_PRESSURE_EVENTS: Counter = ...; // Emergency cleanups triggered
```

**Configuration Recommendations**:

| Deployment Size | max_tracked_sequences | waiter_ttl | cleanup_interval |
|-----------------|----------------------|------------|------------------|
| Small (< 100 writes/sec) | 1,000 | 30s | 60s |
| Medium (100-1000 writes/sec) | 10,000 | 60s | 30s |
| Large (> 1000 writes/sec) | 50,000 | 120s | 15s |

---

## 9. Edge Synchronization

### 9.1 Edge Consumer Architecture

Edge nodes consume from `RSFGA_EVENTS` (not `RSFGA_WRITES`) to receive committed changes:

```text
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

```text
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

```text
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
│    • Storage consumer batches: 100 msgs → 1 DB transaction                  │
│    • Consumer throughput: 5,000-10,000 tuples/sec                           │
│    • Consistency window: 20-100ms (optimized for low latency)               │
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

```text
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

### 13.3 Authorization Model Validation (Critical Security Guarantee)

> **⚠️ SECURITY INVARIANT**: Both sync and async write paths perform **IDENTICAL** authorization model validation before accepting any write operation.

**Validation Steps (Both Paths)**:

```rust
// IDENTICAL validation in both write_tuples_handler and async_write_tuples_handler
async fn validate_write_operation(
    store_id: &str,
    request: &WriteRequest,
    model_cache: &ModelCache,
) -> Result<(), ValidationError> {
    // 1. Load authorization model (from cache or storage)
    let model = model_cache.get_or_load(store_id).await?;

    // 2. Validate each tuple against the model
    for tuple in &request.writes {
        // Verify object type exists in model
        let type_def = model.get_type(&tuple.object_type)
            .ok_or(ValidationError::UndefinedType(tuple.object_type.clone()))?;

        // Verify relation exists for this type
        let _relation = type_def.get_relation(&tuple.relation)
            .ok_or(ValidationError::UndefinedRelation(
                tuple.object_type.clone(),
                tuple.relation.clone(),
            ))?;

        // Verify user type is allowed for this relation
        validate_user_type(&tuple.user, &type_def, &tuple.relation)?;

        // Validate condition syntax if present
        if let Some(condition) = &tuple.condition {
            validate_cel_condition(condition)?;
        }
    }

    // 3. Same validation for deletes
    for tuple in &request.deletes {
        // ... identical validation ...
    }

    Ok(())
}
```

**Validation Guarantees**:

| Check | Sync Path | Async Path | Storage Consumer |
|-------|-----------|------------|------------------|
| Model existence | ✅ | ✅ | N/A (pre-validated) |
| Type validation | ✅ | ✅ | N/A (pre-validated) |
| Relation validation | ✅ | ✅ | N/A (pre-validated) |
| User type validation | ✅ | ✅ | N/A (pre-validated) |
| CEL condition syntax | ✅ | ✅ | N/A (pre-validated) |
| Tuple format | ✅ | ✅ | N/A (pre-validated) |

**Why Storage Consumer Doesn't Re-validate**:
- All messages in `RSFGA_WRITES` have already passed validation
- Re-validation would add latency without security benefit
- Model changes don't invalidate existing tuples (OpenFGA behavior)
- Invalid messages can only come from compromised NATS credentials

**Audit Trail**:
```rust
// Both paths log validation for audit
tracing::info!(
    store_id = %store_id,
    path = "sync|async",
    tuples_validated = request.writes.len() + request.deletes.len(),
    model_version = %model.id,
    "Write request validated against authorization model"
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

// Write path selection metrics (audit logging)
pub static WRITES_BY_PATH: CounterVec = ...;     // Labels: path={sync,async}, reason={default,fallback,explicit}
pub static WRITES_BY_OPERATION: CounterVec = ...; // Labels: path={sync,async}, operation={write,delete}
pub static ASYNC_DELETES_BLOCKED: Counter = ...; // Deletes rejected by async_allow_deletes=false

// Consumer metrics
pub static CONSUMER_LAG: Gauge = ...;            // Messages pending processing
pub static BATCH_SIZE: Histogram = ...;          // Messages per batch
pub static STORAGE_WRITE_LATENCY: Histogram = ...; // DB write latency
pub static TUPLES_COMMITTED: Counter = ...;      // Tuples written to storage

// RYOW metrics
pub static RYOW_WAITS: Counter = ...;            // RYOW wait requests
pub static RYOW_WAIT_TIME: Histogram = ...;      // Time spent waiting for commit
pub static RYOW_TIMEOUTS: Counter = ...;         // RYOW timeout errors

// WriteTracker cleanup metrics
pub static WAITERS_ACTIVE: Gauge = ...;          // Current active waiters
pub static WAITERS_EXPIRED: Counter = ...;       // Waiters expired by TTL
pub static WAITERS_SATISFIED: Counter = ...;     // Waiters satisfied by commit
pub static CLEANUP_RUNS: Counter = ...;          // Cleanup task runs
```

**Write Path Selection Audit Logging**:

```rust
// Log every write for audit trail
fn log_write_path_selection(
    store_id: &str,
    path: WritePath,
    reason: WritePathReason,
    tuples_written: usize,
    tuples_deleted: usize,
) {
    // Increment metrics
    WRITES_BY_PATH
        .with_label_values(&[path.as_str(), reason.as_str()])
        .inc();

    WRITES_BY_OPERATION
        .with_label_values(&[path.as_str(), "write"])
        .inc_by(tuples_written as u64);

    WRITES_BY_OPERATION
        .with_label_values(&[path.as_str(), "delete"])
        .inc_by(tuples_deleted as u64);

    // Structured log for audit
    tracing::info!(
        store_id = %store_id,
        path = %path,
        reason = %reason,
        tuples_written = tuples_written,
        tuples_deleted = tuples_deleted,
        "Write operation completed"
    );
}

#[derive(Debug, Clone, Copy)]
pub enum WritePath {
    Sync,   // Original /write endpoint
    Async,  // New /async/write endpoint
}

#[derive(Debug, Clone, Copy)]
pub enum WritePathReason {
    Default,          // Normal operation
    Fallback,         // NATS unavailable, fell back to sync
    Explicit,         // Client explicitly requested sync
    DeletesBlocked,   // Delete redirected due to async_allow_deletes=false
}
```

**Grafana Dashboard Queries**:

```promql
# Write path distribution
sum by (path) (rate(rsfga_writes_by_path_total[5m]))

# Deletes blocked by security policy
rate(rsfga_async_deletes_blocked_total[5m])

# Fallback rate (indicates NATS issues)
rate(rsfga_writes_by_path_total{reason="fallback"}[5m])
  / rate(rsfga_writes_by_path_total[5m])
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

## 16. Risk Identification

### 16.1 Technical Risks

| Risk ID | Risk | Probability | Impact | Mitigation |
|---------|------|-------------|--------|------------|
| R1 | NATS becomes single point of failure | Medium | High | Circuit breaker + sync fallback |
| R2 | Consumer lag causes unacceptable staleness | Medium | Medium | Monitoring + auto-scaling |
| R3 | Batch processing causes data loss on crash | Low | High | Ack only after DB commit |
| R4 | RYOW adds complexity and latency | High | Low | Optional, off by default |
| R5 | Performance targets not met | Medium | Medium | Validate in Milestone 2.0.3 |
| R6 | NATS JetStream storage fills up | Low | High | Retention limits + monitoring |
| R7 | Event ordering issues across stores | Low | Medium | Per-store sequence numbers |

### 16.2 Security Risks

| Risk ID | Risk | Probability | Impact | Mitigation |
|---------|------|-------------|--------|------------|
| S1 | Revoked permissions honored during window | High | Medium | Use sync path for revocations |
| S2 | Unauthorized access to NATS stream | Low | High | NKey auth + TLS + ACLs |
| S3 | Event tampering in transit | Low | High | TLS encryption |
| S4 | DLQ exposes sensitive tuple data | Low | Medium | DLQ access controls |

### 16.3 Operational Risks

| Risk ID | Risk | Probability | Impact | Mitigation |
|---------|------|-------------|--------|------------|
| O1 | Increased operational complexity | High | Medium | Runbook + training |
| O2 | Debugging async issues is harder | Medium | Medium | Correlation IDs + tracing |
| O3 | Consumer daemon requires management | High | Low | Kubernetes deployment |
| O4 | NATS cluster management overhead | Medium | Medium | Managed NATS or simple setup |

---

## 17. Rollback Plan

### 17.1 Rollback Triggers

Rollback should be initiated if:

1. **Consumer lag exceeds 5 minutes** sustained for 15+ minutes
2. **NATS availability drops below 99%** for 10+ minutes
3. **Data inconsistency detected** between NATS and storage
4. **Security incident** related to async path
5. **Performance degradation** vs. sync baseline

### 17.2 Rollback Procedure

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           Rollback Procedure                                 │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  Phase 1: Immediate (< 5 minutes)                                            │
│  ┌────────────────────────────────────────────────────────────────────────┐ │
│  │  1. Route all traffic to sync endpoints                                │ │
│  │     - Update load balancer / ingress rules                             │ │
│  │     - OR clients switch from /async/* to /*                            │ │
│  │                                                                        │ │
│  │  2. Keep rsfga-writer running to drain queue                           │ │
│  │     - Prevents data loss from pending writes                           │ │
│  └────────────────────────────────────────────────────────────────────────┘ │
│                                                                              │
│  Phase 2: Drain (< 30 minutes)                                               │
│  ┌────────────────────────────────────────────────────────────────────────┐ │
│  │  3. Monitor RSFGA_WRITES queue until empty                             │ │
│  │     - nats stream info RSFGA_WRITES                                    │ │
│  │                                                                        │ │
│  │  4. Verify all messages processed                                      │ │
│  │     - Compare last sequence with consumer position                     │ │
│  └────────────────────────────────────────────────────────────────────────┘ │
│                                                                              │
│  Phase 3: Cleanup (< 1 hour)                                                 │
│  ┌────────────────────────────────────────────────────────────────────────┐ │
│  │  5. Stop rsfga-writer daemon                                           │ │
│  │                                                                        │ │
│  │  6. (Optional) Keep NATS streams for debugging                         │ │
│  │     - OR delete: nats stream delete RSFGA_WRITES                       │ │
│  │                                                                        │ │
│  │  7. Remove /async endpoints from routing (optional)                    │ │
│  └────────────────────────────────────────────────────────────────────────┘ │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 17.3 Rollback Validation

After rollback, verify:

- [ ] All writes going through sync path
- [ ] No messages pending in RSFGA_WRITES
- [ ] Latency/throughput metrics match pre-async baseline
- [ ] No data loss (compare tuple counts)
- [ ] Edge sync still works (via sync path events)

---

## 18. Operational Runbook

### 18.1 Deployment Checklist

```
Pre-deployment:
├── [ ] NATS cluster deployed and healthy
├── [ ] Streams created (RSFGA_WRITES, RSFGA_EVENTS)
├── [ ] Credentials configured (NKey or JWT)
├── [ ] TLS certificates in place
├── [ ] Monitoring dashboards ready
└── [ ] Alerting rules configured

Deployment:
├── [ ] Deploy rsfga-server with NATS config (events only)
├── [ ] Verify sync path publishes to RSFGA_EVENTS
├── [ ] Deploy rsfga-writer daemon
├── [ ] Verify consumer is processing
├── [ ] Enable /async endpoints (gradual rollout)
└── [ ] Monitor metrics for 24 hours

Post-deployment:
├── [ ] Validate performance targets
├── [ ] Verify edge sync receiving events
├── [ ] Test RYOW functionality
└── [ ] Document any issues
```

### 18.2 Common Operations

#### Check Consumer Status

```bash
# View consumer lag
nats consumer info RSFGA_WRITES storage-consumer

# View stream stats
nats stream info RSFGA_WRITES
nats stream info RSFGA_EVENTS

# View pending messages
nats stream view RSFGA_WRITES --last 10
```

#### Restart Consumer

```bash
# Consumer is stateless - just restart
kubectl rollout restart deployment/rsfga-writer

# Consumer will resume from last acked position
```

#### Handle DLQ Messages

```bash
# View DLQ messages
nats stream view RSFGA_DLQ --last 10

# Investigate specific message
nats stream get RSFGA_DLQ <sequence>

# Replay message (after fixing issue)
nats pub rsfga.writes.<store_id> --data '<fixed_payload>'

# Clear DLQ after resolution
nats stream purge RSFGA_DLQ
```

#### Force Drain Queue

```bash
# If consumer is stuck, manually drain
nats consumer next RSFGA_WRITES storage-consumer --count 100 --ack
```

### 18.3 Monitoring & Alerts

| Metric | Warning | Critical | Action |
|--------|---------|----------|--------|
| `rsfga_consumer_lag` | > 1000 | > 10000 | Scale consumer |
| `rsfga_nats_publish_errors` | > 1/min | > 10/min | Check NATS health |
| `rsfga_write_latency_p99` | > 10ms | > 50ms | Check NATS latency |
| `rsfga_fallback_writes` | > 0 | > 100/min | NATS may be down |
| `rsfga_dlq_messages` | > 0 | > 100 | Investigate failures |

### 18.4 Troubleshooting

| Symptom | Possible Cause | Resolution |
|---------|---------------|------------|
| High consumer lag | Consumer slow or down | Scale or restart consumer |
| Writes returning 503 | NATS unavailable | Check NATS, fallback active |
| Events not reaching edge | Consumer not publishing | Check RSFGA_EVENTS stream |
| RYOW timeouts | Consumer behind | Check lag, increase timeout |
| DLQ filling up | Poison messages | Check payloads, fix and replay |

---

## 19. Test Cases

### 19.1 Unit Tests

#### NATS Publisher Tests

```rust
#[cfg(test)]
mod nats_publisher_tests {
    // Connection & Configuration
    #[tokio::test]
    async fn test_publisher_connects_to_nats_server() { }

    #[tokio::test]
    async fn test_publisher_reconnects_on_connection_loss() { }

    #[tokio::test]
    async fn test_publisher_uses_configured_credentials() { }

    // Publishing
    #[tokio::test]
    async fn test_publish_write_request_to_correct_subject() { }

    #[tokio::test]
    async fn test_publish_includes_deduplication_id() { }

    #[tokio::test]
    async fn test_publish_serializes_protobuf_correctly() { }

    #[tokio::test]
    async fn test_publish_returns_sequence_number() { }

    #[tokio::test]
    async fn test_publish_timeout_returns_error() { }

    // Event Publisher
    #[tokio::test]
    async fn test_event_publisher_publishes_to_events_stream() { }

    #[tokio::test]
    async fn test_event_publisher_includes_all_tuple_operations() { }

    #[tokio::test]
    async fn test_event_publisher_generates_unique_event_id() { }
}
```

#### Storage Consumer Tests

```rust
#[cfg(test)]
mod storage_consumer_tests {
    // Batch Processing
    #[tokio::test]
    async fn test_consumer_pulls_batch_of_messages() { }

    #[tokio::test]
    async fn test_consumer_groups_messages_by_store_id() { }

    #[tokio::test]
    async fn test_consumer_respects_max_batch_size() { }

    #[tokio::test]
    async fn test_consumer_respects_batch_timeout() { }

    // Storage Writing
    #[tokio::test]
    async fn test_consumer_writes_tuples_to_storage() { }

    #[tokio::test]
    async fn test_consumer_deletes_tuples_from_storage() { }

    #[tokio::test]
    async fn test_consumer_handles_mixed_writes_and_deletes() { }

    #[tokio::test]
    async fn test_consumer_writes_in_single_transaction() { }

    // Idempotency
    #[tokio::test]
    async fn test_consumer_handles_duplicate_messages_idempotently() { }

    #[tokio::test]
    async fn test_consumer_uses_on_conflict_for_upsert() { }

    #[tokio::test]
    async fn test_consumer_skips_already_processed_sequence() { }

    // Acknowledgment
    #[tokio::test]
    async fn test_consumer_acks_after_successful_write() { }

    #[tokio::test]
    async fn test_consumer_nacks_on_storage_failure() { }

    #[tokio::test]
    async fn test_consumer_acks_individual_messages_in_batch() { }

    // Event Publishing
    #[tokio::test]
    async fn test_consumer_publishes_committed_event_after_storage() { }

    #[tokio::test]
    async fn test_consumer_includes_sequence_in_committed_event() { }

    // Error Handling
    #[tokio::test]
    async fn test_consumer_moves_to_dlq_after_max_retries() { }

    #[tokio::test]
    async fn test_consumer_continues_on_single_message_failure() { }

    #[tokio::test]
    async fn test_consumer_logs_decode_errors() { }
}
```

#### Write Tracker (RYOW) Tests

```rust
#[cfg(test)]
mod write_tracker_tests {
    #[tokio::test]
    async fn test_tracker_marks_sequence_as_committed() { }

    #[tokio::test]
    async fn test_tracker_wait_returns_immediately_if_committed() { }

    #[tokio::test]
    async fn test_tracker_wait_blocks_until_committed() { }

    #[tokio::test]
    async fn test_tracker_wait_times_out() { }

    #[tokio::test]
    async fn test_tracker_wakes_multiple_waiters() { }

    #[tokio::test]
    async fn test_tracker_handles_out_of_order_commits() { }

    #[tokio::test]
    async fn test_tracker_per_store_isolation() { }
}
```

#### Circuit Breaker Tests

```rust
#[cfg(test)]
mod circuit_breaker_tests {
    #[tokio::test]
    async fn test_circuit_closed_allows_calls() { }

    #[tokio::test]
    async fn test_circuit_opens_after_threshold_failures() { }

    #[tokio::test]
    async fn test_circuit_open_rejects_calls() { }

    #[tokio::test]
    async fn test_circuit_half_open_after_timeout() { }

    #[tokio::test]
    async fn test_circuit_closes_on_success_in_half_open() { }

    #[tokio::test]
    async fn test_circuit_reopens_on_failure_in_half_open() { }
}
```

### 19.2 Integration Tests

#### Async API Endpoint Tests

```rust
#[cfg(test)]
mod async_api_tests {
    // Basic Operations
    #[tokio::test]
    async fn test_async_write_returns_request_id() { }

    #[tokio::test]
    async fn test_async_write_returns_sequence_number() { }

    #[tokio::test]
    async fn test_async_write_returns_write_ticket() { }

    #[tokio::test]
    async fn test_async_write_validates_request() { }

    #[tokio::test]
    async fn test_async_write_validates_against_model() { }

    #[tokio::test]
    async fn test_async_write_rejects_invalid_tuples() { }

    // Consistency
    #[tokio::test]
    async fn test_async_write_eventually_visible_in_storage() { }

    #[tokio::test]
    async fn test_async_write_with_ryow_ticket_is_consistent() { }

    #[tokio::test]
    async fn test_check_waits_for_write_ticket_before_read() { }

    // Error Cases
    #[tokio::test]
    async fn test_async_write_returns_503_when_nats_unavailable() { }

    #[tokio::test]
    async fn test_async_write_returns_400_for_invalid_store() { }

    #[tokio::test]
    async fn test_async_write_returns_404_for_missing_model() { }
}
```

#### Sync API with Event Publishing Tests

```rust
#[cfg(test)]
mod sync_api_event_tests {
    #[tokio::test]
    async fn test_sync_write_publishes_event_to_nats() { }

    #[tokio::test]
    async fn test_sync_write_succeeds_even_if_event_publish_fails() { }

    #[tokio::test]
    async fn test_sync_write_event_contains_all_tuples() { }

    #[tokio::test]
    async fn test_sync_write_event_has_correct_sequence() { }

    #[tokio::test]
    async fn test_sync_delete_publishes_delete_event() { }

    #[tokio::test]
    async fn test_sync_model_update_publishes_model_event() { }
}
```

#### End-to-End Flow Tests

```rust
#[cfg(test)]
mod e2e_tests {
    // Full Write Path
    #[tokio::test]
    async fn test_async_write_flows_through_to_storage() { }

    #[tokio::test]
    async fn test_async_write_batch_processed_correctly() { }

    #[tokio::test]
    async fn test_multiple_stores_processed_in_parallel() { }

    // Edge Sync
    #[tokio::test]
    async fn test_edge_consumer_receives_committed_events() { }

    #[tokio::test]
    async fn test_edge_consumer_applies_writes_to_local_storage() { }

    #[tokio::test]
    async fn test_edge_consumer_handles_out_of_order_events() { }

    #[tokio::test]
    async fn test_edge_consumer_skips_duplicate_events() { }

    // Both Paths Consistency
    #[tokio::test]
    async fn test_sync_and_async_writes_both_trigger_edge_sync() { }

    #[tokio::test]
    async fn test_mixed_sync_async_writes_maintain_ordering() { }
}
```

### 19.3 Performance Tests

```rust
#[cfg(test)]
mod performance_tests {
    // Latency
    #[tokio::test]
    #[ignore] // Run manually
    async fn test_async_write_latency_under_5ms_p99() { }

    #[tokio::test]
    #[ignore]
    async fn test_sync_write_latency_under_25ms_p99() { }

    // Throughput
    #[tokio::test]
    #[ignore]
    async fn test_async_write_throughput_over_5000_per_second() { }

    #[tokio::test]
    #[ignore]
    async fn test_consumer_batch_throughput_over_10000_tuples_per_second() { }

    // Batching Efficiency
    #[tokio::test]
    #[ignore]
    async fn test_batching_reduces_db_transactions() { }

    #[tokio::test]
    #[ignore]
    async fn test_parallel_store_processing_improves_throughput() { }

    // Memory & Resources
    #[tokio::test]
    #[ignore]
    async fn test_consumer_memory_stable_under_load() { }

    #[tokio::test]
    #[ignore]
    async fn test_nats_buffer_handles_burst_traffic() { }
}
```

### 19.4 Failure & Recovery Tests

```rust
#[cfg(test)]
mod failure_tests {
    // NATS Failures
    #[tokio::test]
    async fn test_publisher_retries_on_transient_nats_error() { }

    #[tokio::test]
    async fn test_publisher_fails_after_max_retries() { }

    #[tokio::test]
    async fn test_consumer_reconnects_after_nats_restart() { }

    #[tokio::test]
    async fn test_consumer_resumes_from_last_acked_position() { }

    // Storage Failures
    #[tokio::test]
    async fn test_consumer_retries_on_transient_db_error() { }

    #[tokio::test]
    async fn test_consumer_nacks_on_persistent_db_error() { }

    #[tokio::test]
    async fn test_messages_redelivered_after_consumer_crash() { }

    // Circuit Breaker
    #[tokio::test]
    async fn test_fallback_to_sync_when_nats_unavailable() { }

    #[tokio::test]
    async fn test_circuit_breaker_prevents_cascade_failure() { }

    // DLQ
    #[tokio::test]
    async fn test_poison_message_moved_to_dlq() { }

    #[tokio::test]
    async fn test_dlq_contains_error_metadata() { }

    // Edge Failures
    #[tokio::test]
    async fn test_edge_consumer_recovers_after_disconnect() { }

    #[tokio::test]
    async fn test_edge_consumer_resyncs_missed_events() { }
}
```

#### Chaos Engineering Test Scenarios

These tests use chaos engineering tools (e.g., Chaos Mesh, Litmus) to simulate real-world failures:

```yaml
# Chaos Mesh experiments for RSFGA NATS integration

# 1. Network partition between API server and NATS
apiVersion: chaos-mesh.org/v1alpha1
kind: NetworkChaos
metadata:
  name: nats-network-partition
spec:
  action: partition
  mode: all
  selector:
    namespaces: [rsfga]
    labelSelectors:
      app: rsfga-server
  direction: both
  target:
    selector:
      namespaces: [rsfga]
      labelSelectors:
        app: nats
  duration: "60s"
---
# 2. NATS pod crash (test consumer recovery)
apiVersion: chaos-mesh.org/v1alpha1
kind: PodChaos
metadata:
  name: nats-pod-failure
spec:
  action: pod-kill
  mode: one
  selector:
    namespaces: [rsfga]
    labelSelectors:
      app: nats
  scheduler:
    cron: "@every 5m"  # Kill one NATS pod every 5 minutes
---
# 3. Storage consumer OOM (test redelivery)
apiVersion: chaos-mesh.org/v1alpha1
kind: StressChaos
metadata:
  name: consumer-memory-stress
spec:
  mode: all
  selector:
    namespaces: [rsfga]
    labelSelectors:
      app: rsfga-writer
  stressors:
    memory:
      workers: 4
      size: "256MB"
  duration: "30s"
---
# 4. Database latency injection
apiVersion: chaos-mesh.org/v1alpha1
kind: NetworkChaos
metadata:
  name: db-latency
spec:
  action: delay
  mode: all
  selector:
    namespaces: [rsfga]
    labelSelectors:
      app: rsfga-writer
  delay:
    latency: "500ms"
    jitter: "100ms"
  target:
    selector:
      namespaces: [rsfga]
      labelSelectors:
        app: postgresql
  duration: "2m"
```

**Expected Behaviors Under Chaos**:

| Scenario | Expected Behavior | Validation |
|----------|------------------|------------|
| NATS partition (60s) | Circuit breaker opens, sync fallback | `FALLBACK_WRITES` counter increases |
| NATS pod kill | Consumer reconnects, resumes from last ack | No message loss, lag recovers |
| Consumer OOM | Messages redelivered after ack_wait | Idempotent writes, no duplicates |
| DB latency spike | Batch processing slows, lag increases | Alert triggers, no data loss |
| JetStream disk full | Publish errors, sync fallback | `NATS_PUBLISH_ERRORS` increases |

**Chaos Test Checklist**:

- [ ] Circuit breaker transitions correctly under NATS failure
- [ ] Consumer resumes from correct position after crash
- [ ] No duplicate tuples after any failure scenario
- [ ] Metrics accurately reflect failure states
- [ ] Alerts fire within expected timeframes
- [ ] Recovery time < 60s for all failure modes

### 19.5 Compatibility Tests

```rust
#[cfg(test)]
mod compatibility_tests {
    // Original API Unchanged
    #[tokio::test]
    async fn test_original_write_api_response_format_unchanged() { }

    #[tokio::test]
    async fn test_original_write_api_still_works_without_nats() { }

    #[tokio::test]
    async fn test_original_read_apis_work_with_async_writes() { }

    // OpenFGA Compatibility
    #[tokio::test]
    async fn test_async_written_tuples_visible_via_check() { }

    #[tokio::test]
    async fn test_async_written_tuples_visible_via_read() { }

    #[tokio::test]
    async fn test_async_written_tuples_visible_via_list_objects() { }

    #[tokio::test]
    async fn test_changes_api_includes_async_writes() { }

    // Migration
    #[tokio::test]
    async fn test_can_run_sync_and_async_writes_simultaneously() { }

    #[tokio::test]
    async fn test_switch_from_sync_to_async_preserves_data() { }
}
```

### 19.6 Security Tests

```rust
#[cfg(test)]
mod security_tests {
    // Authentication
    #[tokio::test]
    async fn test_nats_connection_requires_valid_credentials() { }

    #[tokio::test]
    async fn test_nats_rejects_invalid_credentials() { }

    // Authorization
    #[tokio::test]
    async fn test_publisher_can_only_publish_to_writes_stream() { }

    #[tokio::test]
    async fn test_consumer_can_only_subscribe_to_allowed_subjects() { }

    #[tokio::test]
    async fn test_edge_consumer_filtered_to_assigned_stores() { }

    // TLS
    #[tokio::test]
    async fn test_nats_connection_uses_tls() { }

    #[tokio::test]
    async fn test_nats_rejects_invalid_certificates() { }

    // Input Validation
    #[tokio::test]
    async fn test_malformed_protobuf_rejected() { }

    #[tokio::test]
    async fn test_oversized_message_rejected() { }
}
```

### 19.7 Test Infrastructure

#### Embedded NATS for Unit Tests

```rust
use async_nats::jetstream;
use testcontainers::{clients::Cli, images::nats::Nats};

pub struct TestNats {
    _container: Container<'static, Nats>,
    pub client: async_nats::Client,
    pub js: jetstream::Context,
}

impl TestNats {
    pub async fn start() -> Self {
        let docker = Cli::default();
        let container = docker.run(Nats::default().with_jetstream());
        let port = container.get_host_port_ipv4(4222);

        let client = async_nats::connect(format!("localhost:{}", port)).await.unwrap();
        let js = jetstream::new(client.clone());

        // Create test streams
        js.get_or_create_stream(stream_config("RSFGA_WRITES")).await.unwrap();
        js.get_or_create_stream(stream_config("RSFGA_EVENTS")).await.unwrap();

        Self { _container: container, client, js }
    }
}

#[tokio::test]
async fn example_test_with_embedded_nats() {
    let nats = TestNats::start().await;

    // Use nats.js for publishing/consuming in tests
    let publisher = NatsEventPublisher::new(nats.js.clone());
    // ...
}
```

#### Test Fixtures

```rust
pub mod fixtures {
    pub fn sample_write_request(store_id: &str) -> WriteRequest {
        WriteRequest {
            request_id: ulid::Ulid::new().to_string(),
            store_id: store_id.to_string(),
            writes: vec![sample_tuple_write()],
            deletes: vec![],
            ..Default::default()
        }
    }

    pub fn sample_tuple_write() -> TupleWrite {
        TupleWrite {
            key: Some(TupleKey {
                object_type: "document".to_string(),
                object_id: "doc1".to_string(),
                relation: "viewer".to_string(),
                user_type: "user".to_string(),
                user_id: "alice".to_string(),
                ..Default::default()
            }),
            condition: None,
        }
    }

    pub fn sample_committed_event(store_id: &str, sequence: u64) -> CommittedEvent {
        CommittedEvent {
            event_id: ulid::Ulid::new().to_string(),
            store_id: store_id.to_string(),
            sequence,
            ..Default::default()
        }
    }
}
```

### 19.8 Test Coverage Requirements

| Component | Required Coverage | Critical Paths |
|-----------|------------------|----------------|
| NATS Publisher | >90% | Publish, retry, timeout |
| Storage Consumer | >95% | Batch, write, ack, idempotency |
| Write Tracker (RYOW) | >90% | Wait, notify, timeout |
| Event Publisher | >90% | Publish committed events |
| Circuit Breaker | >95% | State transitions |
| Async API Handlers | >90% | Validation, publish |
| Edge Consumer | >85% | Apply, dedupe, sync |

---

## 20. Alternatives Considered

### 20.1 Storage-First with Async Events (Original Proposal)

**Approach**: Write to storage first, publish events async after.

**Pros**:
- Simpler consistency model
- No RYOW complexity

**Cons**:
- Higher write latency (blocked by DB)
- Limited throughput
- No batching benefit

**Decision**: Rejected in favor of NATS-first for performance benefits.

### 20.2 Kafka Instead of NATS

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

### 20.3 Database-Based Queue (Transactional Outbox)

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

## 21. Open Questions

### 21.1 Resolved Questions

| Question | Options | Decision | Rationale |
|----------|---------|----------|-----------|
| Default consistency | Eventual / RYOW | **Eventual** | Performance over consistency; RYOW available when needed |
| Consumer scaling | Single / Sharded | **Single initially, shard per-store if needed** | Start simple; add per-store sharding when consumer lag exceeds 1000ms |
| DLQ retention | 7 days / 30 days | **30 days** | Sufficient for investigation; auto-purge after 30 days |
| Sync write threshold | Never / On request | **On request** | Flexibility for critical operations |
| DLQ retry strategy | Automatic / Manual | **Manual with tooling** | Automatic retry risks loops; provide CLI tools for replay |
| Event retention (RSFGA_EVENTS) | 7 days / 30 days / Unlimited | **7 days default, configurable** | Sufficient for edge reconnection; longer for compliance if needed |

### 21.2 Batch Size Tuning and Consumer Lag

**Question**: How does batch size relate to consumer lag?

**Answer**:

| Consumer Lag | Batch Size | Batch Timeout | Rationale |
|--------------|------------|---------------|-----------|
| < 100ms | 100 msgs | 20ms | Default: prioritize consistency |
| 100-500ms | 200 msgs | 50ms | Moderate backlog: increase throughput |
| 500ms-2s | 500 msgs | 100ms | Significant backlog: maximize throughput |
| > 2s | 500 msgs | 100ms + alert | Critical: scale consumers, investigate |

**Auto-tuning** (future enhancement):
```rust
// Dynamic batch sizing based on lag
fn calculate_batch_settings(lag_ms: u64) -> BatchSettings {
    match lag_ms {
        0..=100 => BatchSettings { size: 100, timeout_ms: 20 },
        101..=500 => BatchSettings { size: 200, timeout_ms: 50 },
        _ => BatchSettings { size: 500, timeout_ms: 100 },
    }
}
```

### 21.3 Edge Node Disconnection and Event Retention

**Question**: Is 7-day event retention sufficient for disconnected edge nodes?

**Answer**: **7 days is sufficient for typical scenarios, with bootstrap fallback for longer disconnections.**

| Disconnection Duration | Strategy |
|-----------------------|----------|
| < 7 days | Resume from last sequence in RSFGA_EVENTS |
| > 7 days | Full bootstrap sync from central storage |
| Permanent (new edge) | Full bootstrap sync |

**Configuration**:
```yaml
streams:
  events:
    max_age_hours: 168  # 7 days default
    # Increase for compliance requirements:
    # max_age_hours: 720  # 30 days
```

**Bootstrap procedure**: Edge consumer detects missing sequences and triggers full sync from central database via `GET /admin/bootstrap?store_id=X&format=batch`.

### 21.4 DLQ Retry Strategy

**Question**: Should DLQ messages be automatically retried or require manual intervention?

**Answer**: **Manual intervention with tooling support.**

**Rationale**:
- Automatic retry risks infinite loops for truly poisonous messages
- Manual review allows root cause analysis
- Tooling makes replay easy once issues are fixed

**DLQ Tooling**:
```bash
# View DLQ contents
nats stream view RSFGA_DLQ --last 20

# Inspect specific message
nats stream get RSFGA_DLQ 12345

# Replay single message after fix
rsfga-admin dlq replay --sequence 12345

# Replay all messages for a store
rsfga-admin dlq replay --store-id store-abc

# Purge old DLQ messages (after investigation)
nats stream purge RSFGA_DLQ --seq 10000
```

**Alert on DLQ growth**: Trigger alert when DLQ has > 10 messages in 1 hour.

### 21.5 Consumer Scaling Strategy

**Question**: Should consumer scaling be per-store or global?

**Answer**: **Start with global single consumer; scale per-store if needed.**

| Scenario | Consumer Strategy | When to Use |
|----------|------------------|-------------|
| Low volume (< 1000 writes/sec) | Single global consumer | Default |
| High volume (> 5000 writes/sec) | Multiple consumers with store-based routing | Scale when lag > 500ms |
| Hot store (one store dominates) | Dedicated consumer for hot store | When single store causes > 50% of lag |

**Scaling trigger**: Consumer lag consistently > 500ms for 5 minutes.

**Per-store consumer configuration**:
```yaml
# Consumer per store (advanced deployment)
consumers:
  - name: storage-consumer-store-hot
    filter: "rsfga.writes.store-hot-id"
    batch_size: 500
  - name: storage-consumer-default
    filter: "rsfga.writes.>"
    batch_size: 100
```

**Kubernetes Auto-Scaling with KEDA**:

KEDA (Kubernetes Event-Driven Autoscaling) can automatically scale consumers based on NATS JetStream lag:

```yaml
# KEDA ScaledObject for rsfga-writer
apiVersion: keda.sh/v1alpha1
kind: ScaledObject
metadata:
  name: rsfga-writer-scaler
  namespace: rsfga
spec:
  scaleTargetRef:
    name: rsfga-writer
    kind: Deployment
  pollingInterval: 15                    # Check lag every 15s
  cooldownPeriod: 60                     # Wait 60s before scaling down
  minReplicaCount: 1                     # Always run at least 1 consumer
  maxReplicaCount: 10                    # Maximum consumers
  triggers:
    - type: nats-jetstream
      metadata:
        # NATS server address
        natsServerMonitoringEndpoint: "nats.rsfga.svc.cluster.local:8222"
        # Consumer to monitor
        account: "$G"
        stream: "RSFGA_WRITES"
        consumer: "storage-consumer"
        # Scale threshold: lag in messages
        lagThreshold: "1000"             # Scale up when > 1000 messages pending
        # Or use activationLagThreshold for scale-from-zero
        activationLagThreshold: "10"
---
# TriggerAuthentication for NATS credentials
apiVersion: keda.sh/v1alpha1
kind: TriggerAuthentication
metadata:
  name: nats-auth
  namespace: rsfga
spec:
  secretTargetRef:
    - parameter: natsServerMonitoringEndpoint
      name: nats-credentials
      key: monitoring-url
```

**Consumer Lag Calculation Formula**:

```text
Consumer Lag = Stream Sequence (last message) - Consumer Delivered Sequence

Where:
- Stream Sequence: nats_jetstream_stream_state{stream="RSFGA_WRITES"} [last_seq]
- Delivered Sequence: nats_jetstream_consumer_info{consumer="storage-consumer"} [delivered.stream_seq]

Lag in milliseconds (approximate):
lag_ms = (lag_messages / consumer_throughput_per_sec) * 1000

Example:
- Lag: 500 messages
- Consumer throughput: 5000 msgs/sec
- lag_ms = (500 / 5000) * 1000 = 100ms
```

**Prometheus Alert for Consumer Lag**:

```yaml
# Alert when consumer is falling behind
groups:
  - name: rsfga-consumer-alerts
    rules:
      - alert: RSFGAConsumerLagHigh
        expr: |
          (
            nats_jetstream_stream_state_last_seq{stream="RSFGA_WRITES"}
            - on(cluster)
            nats_jetstream_consumer_ack_floor_stream_seq{consumer="storage-consumer"}
          ) > 1000
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "RSFGA storage consumer lag exceeds 1000 messages"
          description: "Consumer lag is {{ $value }} messages. Consider scaling up."

      - alert: RSFGAConsumerLagCritical
        expr: |
          (
            nats_jetstream_stream_state_last_seq{stream="RSFGA_WRITES"}
            - on(cluster)
            nats_jetstream_consumer_ack_floor_stream_seq{consumer="storage-consumer"}
          ) > 10000
        for: 2m
        labels:
          severity: critical
        annotations:
          summary: "RSFGA storage consumer lag critical"
          description: "Consumer lag is {{ $value }} messages. Immediate action required."
```

**HPA Alternative (Without KEDA)**:

```yaml
# Standard HPA using custom metrics from Prometheus
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: rsfga-writer-hpa
  namespace: rsfga
spec:
  scaleTargetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: rsfga-writer
  minReplicas: 1
  maxReplicas: 10
  metrics:
    - type: External
      external:
        metric:
          name: nats_consumer_lag
          selector:
            matchLabels:
              consumer: storage-consumer
        target:
          type: AverageValue
          averageValue: "500"  # Target 500 msgs lag per replica
```

### 21.6 Remaining Questions for Stakeholders

1. **Compliance requirements for event retention?**
   - Some industries require longer audit trails (90 days, 1 year)
   - Affects RSFGA_EVENTS max_age configuration

2. **Multi-region write conflict resolution?**
   - Current: Single write region, replicate to others
   - Future: Last-write-wins or custom resolution needed?

3. **Edge node autonomy during disconnection?**
   - Current: Read-only during disconnection
   - Future: Allow local writes with sync on reconnection?

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

  # Security: Block DELETE operations on /async endpoints
  # Recommended: false (default) - revocations should use sync path
  # Set to true only if eventual consistency is acceptable for revocations
  async_allow_deletes: false

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

# Storage consumer settings (optimized for low consistency window)
consumer:
  batch_size: 100              # Smaller batches for faster processing
  batch_timeout_ms: 20         # 20ms max wait (prioritizes latency)
  max_per_store_batch: 100     # Smaller DB transactions
  parallelism: 4
  ack_wait_seconds: 60         # See rationale below
  max_deliver: 5               # Move to DLQ after 5 failed attempts

# ack_wait_seconds Rationale:
# ────────────────────────────────────────────────────────────────────
# ack_wait = 60 seconds is chosen to balance:
#
# 1. Normal Processing Time:
#    - Batch fetch: ~20ms
#    - DB write (100 tuples): ~50-200ms
#    - Event publish: ~5-20ms
#    - Total: ~100-300ms typical, ~1-2s worst case
#
# 2. Transient Failure Recovery:
#    - DB connection timeout: 5-10s
#    - DB transaction retry: 3 attempts × 2s = 6s
#    - Network hiccup recovery: ~5s
#    - Total: ~20s for transient recovery
#
# 3. Safety Margin:
#    - 60s provides 3x safety margin over worst case
#    - Prevents premature redelivery causing duplicates
#    - Allows time for DB to recover from brief outages
#
# 4. Why NOT shorter (e.g., 30s):
#    - Risk of redelivery during slow DB operations
#    - Could cause duplicate processing overhead
#    - Less buffer for transient failures
#
# 5. Why NOT longer (e.g., 120s):
#    - Delays detection of crashed consumers
#    - Increases latency during consumer failures
#    - RYOW waiters blocked longer on consumer crash
#
# Recommendation: Adjust based on observed DB latency in production.
# Monitor: STORAGE_WRITE_LATENCY p99 should be << ack_wait_seconds

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
