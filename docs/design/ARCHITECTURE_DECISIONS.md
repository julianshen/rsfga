# RSFGA Architecture Decision Records (ADR)

This document captures key architectural decisions made during the design of RSFGA.

---

## ADR-001: Asynchronous Runtime Selection

**Status**: Accepted

**Context**:
OpenFGA uses Go with synchronous blocking I/O. Rust offers both sync and async options. We need to choose an async runtime for handling concurrent operations efficiently.

**Decision**:
Use **Tokio** as the async runtime with async/await throughout the codebase.

**Rationale**:
- Industry standard with mature ecosystem
- Excellent performance benchmarks
- Native async I/O for database operations (SQLx)
- Work-stealing scheduler for efficient CPU utilization
- Built-in support for timeouts, cancellation, and bounded concurrency

**Consequences**:
- All I/O operations must be async
- Learning curve for contributors unfamiliar with async Rust
- Enables significant performance improvements through concurrent graph traversal

**Alternatives Considered**:
- **Sync Rust**: Simpler but misses parallelism opportunities
- **async-std**: Less ecosystem support than Tokio
- **Rayon (data parallelism)**: Not suitable for I/O-bound operations

---

## ADR-002: Lock-Free Caching Strategy

**Status**: Accepted

**Context**:
OpenFGA uses mutex-based caching which creates contention under high concurrency. We need efficient concurrent access to cached check results.

**Decision**:
Use **DashMap** for the check cache instead of `Arc<Mutex<HashMap>>`.

**Rationale**:
- Lock-free reads with minimal contention
- Sharded internal structure distributes lock pressure
- 10-100x faster than mutex-based HashMap under contention
- Native support for concurrent operations

**Consequences**:
- Slight memory overhead vs standard HashMap
- Excellent performance under high concurrency
- Predictable latency without lock contention

**Alternatives Considered**:
- **RwLock<HashMap>**: Still has lock contention on writes
- **Evmap**: More complex, overkill for our use case
- **Moka**: Good for TTL-based eviction but adds overhead

**Implementation Note**:
We use `moka` for TTL-based eviction on top of DashMap for the best of both worlds.

---

## ADR-003: Parallel Graph Traversal

**Status**: Accepted

**Context**:
OpenFGA's graph resolver is synchronous and single-threaded. Union relations (A or B or C) are evaluated sequentially, missing parallelism opportunities.

**Decision**:
Implement **parallel graph traversal** using Tokio's `select_ok` for union relations and `join_all` for intersections.

**Rationale**:
- Union relations can race to first success (early termination)
- Intersection relations must check all branches anyway (parallel execution)
- Exploits I/O parallelism during database queries
- Can terminate early when first successful path found

**Consequences**:
- Significantly faster resolution for complex permission models
- Requires careful timeout and resource management
- More complex implementation than sequential traversal

**Example**:
```rust
// Instead of:
for relation in union_relations {
    if resolve(relation).await? { return Ok(true); }
}

// Use:
futures::future::select_ok(
    union_relations.map(|r| resolve(r))
).await
```

---

## ADR-004: Storage Abstraction with Async Trait

**Status**: Accepted

**Context**:
We need to support multiple storage backends (PostgreSQL, MySQL, in-memory) with a clean abstraction.

**Decision**:
Define `DataStore` trait with **async_trait** for async method signatures.

**Rationale**:
- Clean abstraction over different storage backends
- Allows runtime backend selection
- Testability with mock implementations
- Future extensibility (Redis, Cassandra, etc.)

**Consequences**:
- Slight performance overhead from dynamic dispatch
- Small allocation overhead from `async_trait` (Box<Future>)
- Enables clean architecture and testing

**Alternatives Considered**:
- **Static dispatch with generics**: More complex API, compile-time backend selection only
- **Trait objects without async**: Can't use async methods

**Trade-off**:
Accept minor performance cost (~1-2%) for significantly better architecture and maintainability.

---

## ADR-005: Batch Check Deduplication Strategy

**Status**: Accepted

**Context**:
OpenFGA processes batch checks synchronously with no deduplication. Multiple concurrent requests checking the same permission compute independently.

**Decision**:
Implement **three-stage deduplication**:
1. Intra-batch dedup (within single request)
2. Cross-request singleflight (across concurrent requests)
3. Cache lookup (across all requests)

**Rationale**:
- Eliminates redundant computation
- Reduces database load
- Significantly improves batch throughput
- Compatible with async execution

**Implementation**:
```rust
// Stage 1: Dedup within batch
let (unique, dedup_map) = deduplicate_checks(&batch);

// Stage 2: Singleflight across requests
let results = execute_with_singleflight(unique).await;

// Stage 3: Cache lookup happens within execute
```

**Consequences**:
- 20-50x improvement in batch check performance
- Requires careful coordination of in-flight requests
- Small memory overhead for tracking in-flight requests

---

## ADR-006: gRPC Streaming for Batch Results

**Status**: Deferred to Post-MVP

**Context**:
Large batch requests currently block until all checks complete. Streaming results would allow clients to process partial results.

**Decision**:
**Defer to post-MVP**, but design batch handler to support streaming in the future.

**Rationale**:
- MVP focuses on correctness and basic performance
- Streaming adds complexity to client implementation
- Can be added later without breaking changes

**Future Implementation**:
```rust
// Current: unary RPC
async fn batch_check(req) -> Response<BatchCheckResponse>

// Future: streaming RPC
async fn batch_check_stream(req) -> ResponseStream<CheckResponse>
```

---

## ADR-007: Write Operations with Async Cache Invalidation

**Status**: Accepted

**Context**:
OpenFGA blocks write operations on cache invalidation (1-second timeout). This adds latency to write operations.

**Decision**:
Implement **async cache invalidation** that doesn't block the write response.

**Rationale**:
- Write latency dominated by database commit, not cache invalidation
- Eventual consistency acceptable for authorization (reads can tolerate brief stale cache)
- 2-3x improvement in write throughput

**Consequences**:
- Small window (1-10ms) where stale cached results may be returned
- For critical security, clients can use `consistency=strong` flag (bypasses cache)
- Significantly better write performance

**Safety Mechanism**:
```rust
async fn write_tuples(tuples) -> Result<()> {
    // Write to database (blocking)
    let write_result = storage.write(tuples).await?;

    // Invalidate cache (non-blocking)
    tokio::spawn(async move {
        cache.invalidate_pattern(tuples).await
    });

    Ok(write_result)
}
```

---

## ADR-008: Connection Pooling Strategy

**Status**: Accepted

**Context**:
Database connections are expensive to create. We need efficient connection management for high throughput.

**Decision**:
Use **deadpool** (for PostgreSQL) with the following configuration:
- Min connections: 10
- Max connections: 100
- Acquire timeout: 5s
- Idle timeout: 10 minutes
- Max lifetime: 30 minutes

**Rationale**:
- Proven connection pool with excellent performance
- Automatic recovery from connection failures
- Configurable limits to prevent resource exhaustion
- Integration with SQLx

**Consequences**:
- 10 connections always warm (slight resource overhead)
- Fast connection acquisition under normal load
- Graceful degradation under high load

---

## ADR-009: Error Handling Strategy

**Status**: Accepted

**Context**:
Rust requires explicit error handling. We need a consistent approach across the codebase.

**Decision**:
Use **thiserror** for domain errors with custom error types per module, and **anyhow** for application-level error propagation.

**Example**:
```rust
// Domain errors (thiserror)
#[derive(thiserror::Error, Debug)]
pub enum ResolverError {
    #[error("depth limit exceeded (max: {max})")]
    DepthLimitExceeded { max: u32 },

    #[error("timeout after {duration:?}")]
    Timeout { duration: Duration },

    #[error("cycle detected: {path}")]
    CycleDetected { path: String },
}

// Application errors (anyhow)
pub type Result<T> = anyhow::Result<T>;
```

**Rationale**:
- Type-safe error handling at domain boundaries
- Flexible error propagation in application code
- Good error messages for debugging
- Compatible with `?` operator

---

## ADR-010: Observability Stack

**Status**: Accepted

**Context**:
Production deployments require comprehensive observability. We need metrics, tracing, and logging.

**Decision**:
Use the following stack:
- **Metrics**: `metrics` crate + Prometheus exporter
- **Tracing**: `tracing` crate + OpenTelemetry exporter
- **Logging**: `tracing` (structured logging)

**Rationale**:
- Industry-standard observability stack
- Rich ecosystem and tooling
- Distributed tracing for complex operations
- Low overhead when disabled

**Key Metrics**:
```rust
// RED metrics
metrics::counter!("requests_total", "operation" => "check");
metrics::histogram!("request_duration_seconds");
metrics::counter!("request_errors_total");

// Business metrics
metrics::gauge!("cache_hit_ratio");
metrics::histogram!("graph_depth");
metrics::gauge!("tuple_count");
```

**Tracing**:
```rust
#[instrument(skip(self), fields(store=%store))]
async fn check(&self, store: &str, user: &str, ...) -> Result<bool> {
    // Automatic span creation with attributes
}
```

---

## ADR-011: Testing Strategy

**Status**: Accepted

**Context**:
We need comprehensive testing to ensure correctness and prevent regressions.

**Decision**:
Implement **four-tier testing pyramid**:
1. **Unit tests**: Individual components (90%+ coverage)
2. **Integration tests**: Storage backends, API endpoints
3. **Compatibility tests**: OpenFGA test suite
4. **Performance tests**: Benchmarks, load tests (k6)

**Rationale**:
- Unit tests catch logic errors early
- Integration tests verify component interactions
- Compatibility tests ensure API parity with OpenFGA
- Performance tests prevent regressions

**CI Pipeline**:
```yaml
# .github/workflows/ci.yml
- Run unit tests (cargo test)
- Run integration tests (testcontainers)
- Run compatibility tests (vs OpenFGA)
- Run benchmarks (criterion, compare to baseline)
- Run k6 load tests
```

---

## ADR-012: Storage Schema Design

**Status**: Accepted

**Context**:
Need efficient storage schema for tuples with support for common query patterns.

**Decision**:
Use **denormalized tuple table** with multiple indexes:

```sql
CREATE TABLE tuples (
    store_id UUID NOT NULL,
    object_type VARCHAR(255) NOT NULL,
    object_id VARCHAR(255) NOT NULL,
    relation VARCHAR(255) NOT NULL,
    user_type VARCHAR(255) NOT NULL,
    user_id VARCHAR(255) NOT NULL,
    user_relation VARCHAR(255),
    PRIMARY KEY (store_id, object_type, object_id, relation,
                 user_type, user_id, COALESCE(user_relation, ''))
);

CREATE INDEX idx_tuples_object ON tuples(store_id, object_type, object_id);
CREATE INDEX idx_tuples_user ON tuples(store_id, user_type, user_id);
CREATE INDEX idx_tuples_userset ON tuples(store_id, user_type, user_id, user_relation)
    WHERE user_relation IS NOT NULL;
```

**Rationale**:
- Composite primary key prevents duplicates
- Indexes optimize common query patterns:
  - Find tuples by object (forward expansion)
  - Find tuples by user (reverse expansion)
  - Find tuples by userset (group membership)

**Consequences**:
- Fast queries for graph traversal
- Write performance remains good (tuple writes are append-heavy)
- Index maintenance overhead acceptable

---

## ADR-013: Precomputation Architecture (Phase 2)

**Status**: Approved for Phase 2

**Context**:
Check operations require graph traversal which is computationally expensive. Precomputing results can dramatically improve performance.

**Decision**:
Implement **on-write precomputation** with Valkey (Redis) storage:

**Architecture**:
```
Write → Change Classification → Impact Analysis → Compute Affected Checks → Store in Valkey
                                                                                    ↓
                                                              Check → Valkey Lookup → Return
                                                                    ↓ (on miss)
                                                              Graph Resolve → Cache
```

**Rationale**:
- Move computation cost from read path (check) to write path
- Amortize cost across multiple reads
- Sub-millisecond check operations from cache lookup

**Trade-offs**:
- Increased write latency (acceptable for better read performance)
- Storage overhead for precomputed results
- Eventual consistency (TTL-based expiration)

**Performance Target**:
- Check: <1ms (99th percentile) for cached results
- Batch: 500+ checks/second

---

## ADR-014: Edge Architecture Design (Phase 3)

**Status**: Approved for Phase 3

**Context**:
Global deployments require low latency worldwide. Centralized architecture has inherent latency limits.

**Decision**:
Implement **product-based data partitioning** with edge nodes:

**Topology**:
```
Edge Nodes (per region)
    ↓ (selective sync via Kafka)
Regional Nodes (multi-region)
    ↓ (full replication)
Central Nodes (primary)
```

**Rationale**:
- Data locality reduces latency (edge <10ms, regional <50ms)
- Product-based partitioning reduces storage by 85-95% vs full replication
- Selective sync reduces bandwidth and storage costs
- Fallback to regional ensures correctness

**Data Tiers**:
- **Tier 1 (Critical)**: Models, metadata → replicate everywhere
- **Tier 2 (Hot)**: Recently accessed data → primary/secondary regions
- **Tier 3 (Warm)**: Cache-only → fetch on demand
- **Tier 4 (Cold)**: Audit logs → never replicate to edge

**Performance Target**:
- Edge: <10ms (95th percentile) globally
- Regional: <50ms (95th percentile)
- Cluster: 100K+ req/s throughput

---

## ADR-015: Rust Edition and MSRV

**Status**: Accepted

**Context**:
Need to choose Rust edition and minimum supported Rust version (MSRV).

**Decision**:
- **Rust Edition**: 2021
- **MSRV**: 1.75.0 (or latest stable at project start)

**Rationale**:
- Edition 2021 provides latest language features
- Recent MSRV ensures access to modern crate ecosystem
- Will update MSRV every 6 months following Rust stable

**Consequences**:
- Contributors need relatively recent Rust version
- Access to latest async/await improvements
- Better compile times and error messages

---

## ADR-016: Dependency Management Policy

**Status**: Accepted

**Context**:
External dependencies introduce security risks and maintenance burden. Need clear policy.

**Decision**:
**Minimize dependencies**, prefer:
1. Tier-1 crates (tokio, serde, sqlx, etc.)
2. Crates with strong maintenance and security track record
3. No unmaintained dependencies
4. Regular `cargo audit` in CI

**Allowed Dependency Categories**:
- Async runtime: tokio
- HTTP/gRPC: axum, tonic
- Database: sqlx
- Serialization: serde, prost
- Observability: tracing, metrics, opentelemetry

**Forbidden**:
- Nightly-only features
- Deprecated crates
- Crates with known security issues

---

## Summary: Key Design Principles

1. **Async-first**: Leverage Tokio for I/O parallelism
2. **Lock-free where possible**: DashMap, arc-swap for concurrent access
3. **Fail fast**: Depth limits, timeouts, circuit breakers
4. **Observable**: Comprehensive metrics, tracing, logging
5. **Compatible**: 100% OpenFGA API compatibility
6. **Performant**: 2-5x improvement through Rust's zero-cost abstractions
7. **Scalable**: Design for distributed deployment from the start
8. **Tested**: High coverage, compatibility suite, performance benchmarks

---

## Next: Start Implementation

With these architectural decisions documented, proceed to **Milestone 1.1** in the roadmap:

1. Initialize Rust workspace
2. Set up CI/CD pipeline
3. Copy OpenFGA proto definitions
4. Begin type system implementation

See `ROADMAP.md` for detailed implementation plan.
