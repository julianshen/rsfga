# Precomputed Check Design (v3.0.0)

**Date**: 2026-02-16
**Status**: Approved
**Phase**: 3 (Precomputed Check)

---

## Goal

Sub-millisecond check operations by precomputing hot-path authorization results and storing them in Valkey (Redis-compatible). On cache hit, checks return in <1ms instead of traversing the full permission graph.

## Key Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Deployment model | Multi-node with shared Valkey cache | Production HA, cross-instance result sharing |
| Trigger model | Event-driven from NATS JetStream | Low staleness, reuses existing Phase 2 infrastructure |
| Precomputation scope | Hot-path only | Bounded memory/CPU, precompute only actually-queried checks |
| Architecture | Dedicated `rsfga-precompute` binary | Independent scaling, follows rsfga-writer/rsfga-edge pattern |

## System Architecture

```
                          +-------------------+
                          |   Valkey/Redis    |
                          |   (Shared Cache)  |
                          +----+----------+---+
                     write     |          |  read
                  results      |          |  results
                               |          |
+----------+    +--------------+--+  +----+--------------+
|   NATS   |--->| rsfga-precompute|  |    rsfga API     |<-- Check requests
| JetStream|    |  (new binary)   |  |  (existing)      |
|          |    +-----------------+  +------------------+
+----------+           |                     |
     ^                 |                     |
     |           +-----+-----+         +----+------+
     |           |  Storage  |         |  Storage  |
     +-----------+  (read)   |         |  (read)   |
  tuple/model    +-----------+         +-----------+
  write events
```

### Data Flow

1. Client writes tuples/models -> RSFGA API publishes to NATS (existing Phase 2)
2. `rsfga-precompute` consumes events from NATS `RSFGA_EVENTS` stream
3. Classifies change -> identifies affected hot-path checks -> re-resolves via storage -> writes to Valkey
4. Client check request -> RSFGA API looks up Valkey first -> hit returns sub-ms -> miss falls back to graph resolver
5. RSFGA API records every check key in Valkey hot-path registry (lightweight ZADD, ~0.1ms)

### New Crate Structure

```
crates/
  rsfga-precompute/        # Precomputation library + binary
    src/
      lib.rs
      config.rs            # Configuration
      classifier.rs        # Change classification
      impact.rs            # Impact analysis (reverse expansion)
      registry.rs          # Hot-path registry client
      worker.rs            # Precomputation worker pool
      valkey.rs            # Valkey client wrapper
      main.rs              # Binary entry point
    Cargo.toml
  rsfga-valkey/            # Shared Valkey client (used by API + precompute)
    src/
      lib.rs
      client.rs            # Connection pooling
      keys.rs              # Key format conventions
      cache.rs             # Precomputed result read/write
    Cargo.toml
```

## Component Design

### Change Classifier (`classifier.rs`)

Consumes NATS events from `RSFGA_EVENTS.*` subjects. Categorizes each event:

| Event Type | Classification | Impact Scope |
|---|---|---|
| Tuple Write | `TupleAdded(object_type, relation, user)` | All checks involving this (relation, object_type) |
| Tuple Delete | `TupleRemoved(object_type, relation, user)` | Same as above |
| Model Change | `ModelChanged(store_id, model_id)` | ALL checks for this store (full invalidation) |

For tuple changes, extracts the "impact key" -- the `(store_id, object_type, relation)` triple.

### Hot-Path Registry (`registry.rs`)

Tracks which check queries are actually being made. Avoids precomputing the entire permission graph.

**Valkey storage**: Sorted set per store, scored by last-access timestamp.
```
Key:    hotpath:{store_id}
Member: {object_type}:{object_id}#{relation}@{user_type}:{user_id}
Score:  Unix timestamp of last check request
```

**API server side**: On every check request, `ZADD` with `GT` flag updates the hot-path entry (~0.1ms overhead).

**Precompute worker side**: On event processing, `ZSCAN` with pattern matching to find affected hot-path entries.

**Eviction**: Periodically trim entries older than configurable window (default: 1 hour).

### Impact Analyzer (`impact.rs`)

Given a tuple change and the hot-path registry, determines which check results need recomputation.

**Algorithm:**
1. Receive event, e.g. `TupleAdded(store_id, "document", "viewer", "user:alice")`
2. Query hot-path registry for entries matching `document:*#viewer@*` and any computed relations depending on `viewer`
3. For each match, schedule a recomputation job
4. For model changes: invalidate ALL precomputed results for the store, re-queue all hot-path entries

**Computed relation expansion**: If `viewer` is defined as `[user] or editor`, then a change to an `editor` tuple also impacts `viewer` checks. The analyzer reads the authorization model to traverse relation dependencies.

### Precomputation Worker Pool (`worker.rs`)

Async worker pool (configurable, default 4 workers):
1. Receives recomputation jobs from impact analyzer
2. Resolves check using existing `rsfga-domain` graph resolver (reads from storage)
3. Writes result to Valkey with TTL

**Valkey result format:**
```
Key:    check:{store_id}:{model_id}:{object_type}:{object_id}#{relation}@{user_type}:{user_id}
Value:  {"allowed": true/false, "computed_at": "...", "model_id": "..."}
TTL:    Configurable (default: 5 minutes)
```

Model ID in the key ensures stale results from old models are never returned.

**Backpressure**: Bounded job queue (default 10,000). Excess jobs dropped by lowest priority (oldest hot-path entries). Debounce: batch changes to same (object_type, relation) within 50ms window.

### Hybrid Check Resolution (API modification)

Modify existing check handler to try Valkey first:

```
Check Request -> Parse & Validate
    |
Construct Valkey key from (store_id, model_id, tuple_key)
    |
Valkey GET
    +-- Hit -> Return result (sub-ms), header: X-Precomputed: hit
    +-- Miss -> Graph resolver (existing), header: X-Precomputed: miss
                Record in hot-path registry (ZADD)
```

Valkey lookup is optional. If Valkey unavailable, falls back gracefully. Controlled by `precompute` feature flag (like `nats` feature flag).

## Error Handling

| Failure | Behavior |
|---|---|
| Valkey unavailable (API) | Graceful fallback to graph resolver. Log warning. |
| Valkey unavailable (precompute) | Pause, retry with exponential backoff. Events stay in NATS. |
| NATS unavailable | Auto-reconnect. JetStream persists events, no data loss. |
| Storage unavailable (precompute) | Retry with backoff, skip after max retries. Stale Valkey entry expires via TTL. |
| Model change during recomputation | Model ID in key ensures correctness. Old-model results never returned. |
| Write burst (thundering herd) | Bounded queue, deduplication, 50ms debounce window. |

## Metrics (Prometheus)

```
# Precompute worker
precompute_events_received_total{event_type}
precompute_jobs_queued_total
precompute_jobs_completed_total{result="success|error"}
precompute_resolution_duration_seconds{quantile}
precompute_valkey_write_duration_seconds{quantile}
precompute_queue_depth

# API-side
check_precomputed_hit_total
check_precomputed_miss_total
check_precomputed_lookup_duration_seconds{quantile}
hotpath_registry_size{store_id}
```

## Configuration

### rsfga-precompute binary
```yaml
nats:
  url: nats://localhost:4222
  name: rsfga-precompute-1

valkey:
  url: redis://localhost:6379
  pool_size: 10
  result_ttl_secs: 300        # 5 minutes
  hotpath_ttl_secs: 3600      # 1 hour

workers: 4
batch_window_ms: 50
max_queue_depth: 10000

storage:
  backend: postgres
  database_url: postgres://rsfga:rsfga@localhost:5432/rsfga
```

### rsfga API additions
```yaml
precompute:
  enabled: true
  valkey_url: redis://localhost:6379
  valkey_pool_size: 5
  hotpath_recording: true
```

## Implementation Milestones

| Milestone | Description | Est. Tests |
|---|---|---|
| 3.0.1 | Valkey client crate (`rsfga-valkey`) | ~15 |
| 3.0.2 | Change classifier (NATS event parsing, impact keys) | ~20 |
| 3.0.3 | Hot-path registry (ZADD, ZSCAN, TTL eviction) | ~15 |
| 3.0.4 | Impact analyzer (relation dependencies, affected check enumeration) | ~25 |
| 3.0.5 | Precomputation worker pool (job queue, resolution, Valkey write) | ~20 |
| 3.0.6 | `rsfga-precompute` binary (main loop, config, graceful shutdown) | ~10 |
| 3.0.7 | API-side hybrid resolution (Valkey lookup, fallback, hot-path recording) | ~20 |
| 3.0.8 | Docker & integration (Dockerfile, docker-compose, e2e tests) | ~15 |
| **Total** | | **~140** |

## Valkey Security Considerations

Valkey stores precomputed authorization results (allowed/denied) keyed by check parameters. While it does not store credentials or PII, it holds security-sensitive data that could be exploited to infer permission structures.

### Network Isolation
- Deploy Valkey in a private network segment, not exposed to the public internet.
- Use firewall rules to restrict access to only `rsfga` and `rsfga-precompute` instances.

### Authentication
- Enable Valkey AUTH (`requirepass` or ACL users) in production. Pass credentials via `VALKEY_URL` (e.g., `redis://user:password@host:6379`).
- The `redact_url()` helper in both `rsfga-precompute` and `rsfga-valkey` ensures passwords are never logged.

### Encryption in Transit
- Use TLS (`rediss://` URL scheme) for all Valkey connections in production.
- Valkey 7.2+ supports TLS natively; earlier versions require stunnel or a TLS proxy.

### Key Design
- Cache keys use percent-encoding for delimiter characters (`#`, `@`, `%`) to prevent key injection via crafted object IDs.
- Key format: `check:{store_id}:{model_id}:{type}:{encoded_id}#{relation}@{encoded_user}`
- Hot-path members use the same encoding: `{type}:{encoded_id}#{relation}@{encoded_user}`

### Data Sensitivity
- Cached values are booleans (allowed/denied). They do not contain tuple data or user attributes.
- TTL-based expiration (default 300s for results, 3600s for hot-path sets) limits the window of stale data.
- Model changes trigger full store invalidation, preventing stale results from persisting after permission model updates.

### Operational
- Monitor Valkey memory usage; use `maxmemory` with `allkeys-lru` eviction policy to bound memory.
- `get_all_hotpath` uses paginated ZSCAN (cursor-based, COUNT 100) to avoid blocking Valkey with unbounded ZRANGE operations.

## Testing Strategy

- **Unit tests**: Classifier, impact analyzer, registry operations, Valkey client
- **Integration tests**: End-to-end with NATS + Valkey + storage
- **Correctness**: Precomputed results must match graph resolver exactly (property-based tests)
- **Performance**: Benchmark precomputation throughput and Valkey lookup latency
- **Failure modes**: Valkey down, NATS down, storage down (verify graceful degradation)
