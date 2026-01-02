# RSFGA Architecture Decision Records (ADR)

This document captures key architectural decisions made during the design of RSFGA.

## ADR Template

Each ADR follows this structure:
- **Status**: Proposed | Accepted | Deprecated | Superseded
- **Date**: Decision date
- **Deciders**: Who made the decision
- **Review Trigger**: When to revisit this decision
- **Context**: Why we need to make this decision
- **Decision**: What we decided
- **Rationale**: Why this decision
- **Consequences**: Impact of this decision
- **Alternatives**: What else we considered (with quantitative comparison where possible)
- **Validation**: How to verify this decision was correct
- **Related Risks**: Links to RISKS.md entries

## Decision Status

| ID | Title | Status | Date | Review Trigger |
|----|-------|--------|------|----------------|
| ADR-001 | Async Runtime Selection | ✅ Accepted | 2024-01-03 | Tokio performance issues |
| ADR-002 | Lock-Free Caching | ✅ Accepted | 2024-01-03 | DashMap scalability issues |
| ADR-003 | Parallel Graph Traversal | ✅ Accepted | 2024-01-03 | Performance target not met |
| ADR-004 | Storage Abstraction | ✅ Accepted | 2024-01-03 | New backend requirements |
| ADR-005 | Batch Deduplication | ✅ Accepted | 2024-01-03 | Complexity vs benefit |
| ADR-006 | gRPC Streaming | 🚧 Deferred | 2024-01-03 | User demand for streaming |
| ADR-007 | Async Cache Invalidation | ✅ Accepted | 2024-01-03 | Consistency issues |
| ADR-008 | Connection Pooling | ✅ Accepted | 2024-01-03 | Pool exhaustion |
| ADR-009 | Error Handling | ✅ Accepted | 2024-01-03 | N/A |
| ADR-010 | Observability Stack | ✅ Accepted | 2024-01-03 | Tool limitations |
| ADR-011 | Testing Strategy | ✅ Accepted | 2024-01-03 | Coverage drops <90% |
| ADR-012 | Storage Schema | ✅ Accepted | 2024-01-03 | Query performance issues |
| ADR-013 | Precomputation (Phase 2) | 📋 Proposed | 2024-01-03 | Before Phase 2 starts |
| ADR-014 | NATS for Edge (Phase 3) | 📋 Proposed | 2024-01-03 | Before Phase 3 starts |
| ADR-015 | Rust Edition/MSRV | ✅ Accepted | 2024-01-03 | Every 6 months |
| ADR-016 | Dependency Policy | ✅ Accepted | 2024-01-03 | Security audit |

---

## ADR-001: Asynchronous Runtime Selection

**Status**: ✅ Accepted
**Date**: 2024-01-03
**Deciders**: Architecture Team
**Review Trigger**: Tokio performance issues OR async overhead >10% in profiling

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
- **Sync Rust**: Simpler but misses parallelism opportunities. Estimated 30-50% slower on I/O-bound operations (unvalidated).
- **async-std**: Less ecosystem support than Tokio. Similar performance expected but smaller ecosystem.
- **Rayon (data parallelism)**: Not suitable for I/O-bound operations. Better for CPU-bound parallel tasks.

**Validation Criteria**:
- [ ] Async overhead <10% compared to theoretical sync baseline (Milestone 1.7)
- [ ] Check operation throughput >1000 req/s (Milestone 1.7)
- [ ] No tokio-related deadlocks or panics in testing

**Related Risks**: R-008 (Tokio Runtime Overhead)

---

## ADR-002: Lock-Free Caching Strategy

**Status**: ✅ Accepted
**Date**: 2024-01-03
**Deciders**: Performance Team
**Review Trigger**: Cache contention observed OR DashMap performance issues

**Context**:
OpenFGA uses mutex-based caching which creates contention under high concurrency. We need efficient concurrent access to cached check results.

**Decision**:
Use **DashMap** for the check cache instead of `Arc<Mutex<HashMap>>`.

**Rationale**:
- Lock-free reads with minimal contention
- Sharded internal structure distributes lock pressure
- **Claimed 10-100x faster** than mutex-based HashMap under high contention (requires validation)
- Native support for concurrent operations

**Consequences**:
- Slight memory overhead vs standard HashMap (~20-30% estimated)
- Expected excellent performance under high concurrency
- Predictable latency without lock contention

**Alternatives Considered** (with quantitative comparison):

| Alternative | Read Perf | Write Perf | Memory | Complexity |
|-------------|-----------|------------|--------|------------|
| **DashMap** (chosen) | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐ | ⭐⭐⭐ | ⭐⭐ |
| RwLock<HashMap> | ⭐⭐⭐ | ⭐⭐ | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐ |
| Evmap | ⭐⭐⭐⭐⭐ | ⭐⭐ | ⭐⭐ | ⭐ |
| Moka | ⭐⭐⭐⭐ | ⭐⭐⭐ | ⭐⭐⭐ | ⭐⭐⭐ |

- **RwLock<HashMap>**: Write lock blocks all readers. Expected 5-10x slower under contention.
- **Evmap**: Read-optimized but writes require coordination. Complex API.
- **Moka**: Excellent TTL support but higher overhead. Using as L2 cache.

**Implementation Note**:
We use `moka` for TTL-based eviction on top of DashMap for the best of both worlds.

**Validation Criteria**:
- [ ] Benchmark DashMap vs Mutex<HashMap> at 100, 500, 1000 concurrent threads (Milestone 1.7)
- [ ] Measure actual speedup (target: >5x at 100+ threads)
- [ ] Verify no scalability issues at 1000+ concurrent operations
- [ ] Memory overhead <50% vs Mutex<HashMap>

**Related Risks**: R-009 (DashMap Scalability Limits)

---

## ADR-003: Parallel Graph Traversal

**Status**: ✅ Accepted
**Date**: 2024-01-03
**Deciders**: Performance Team, Domain Team
**Review Trigger**: Performance target not met (<1000 req/s) OR deadlock issues

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

**Alternatives Considered**:
- **Sequential evaluation**: Simpler but 2-5x slower on models with many union relations (estimated)
- **Thread pool parallelism**: More overhead than async, doesn't work well with I/O-bound operations
- **Rayon data parallelism**: Not suitable for async I/O operations

**Validation Criteria**:
- [ ] Benchmark parallel vs sequential on complex models (Milestone 1.7)
- [ ] Measure speedup on union relations (target: 2-3x)
- [ ] Verify no race conditions or deadlocks in testing
- [ ] Timeout handling works correctly under load

**Related Risks**: R-002 (Performance Claims Unvalidated)

---

## ADR-004: Storage Abstraction with Async Trait

**Status**: ✅ Accepted
**Date**: 2024-01-03
**Deciders**: Architecture Team
**Review Trigger**: New backend requirements OR performance overhead >5%

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

**Alternatives Considered** (with trade-offs):

| Alternative | Performance | Flexibility | Complexity | Testability |
|-------------|-------------|-------------|------------|-------------|
| **async_trait** (chosen) | ⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ |
| Static generics | ⭐⭐⭐⭐⭐ | ⭐⭐ | ⭐⭐ | ⭐⭐⭐ |
| No abstraction | ⭐⭐⭐⭐⭐ | ⭐ | ⭐⭐⭐⭐⭐ | ⭐ |

- **Static dispatch with generics**: Slightly faster (~1-2%) but requires compile-time backend selection
- **No abstraction**: Fastest but impossible to switch backends or test with mocks

**Trade-off**:
Accept minor performance cost (~1-2%) for significantly better architecture and maintainability.

**Validation Criteria**:
- [ ] Benchmark trait overhead vs direct calls (Milestone 1.7)
- [ ] Verify overhead <2% in realistic workloads
- [ ] Successfully implement 3+ storage backends (PostgreSQL, MySQL, In-Memory)
- [ ] Mock implementation usable in unit tests

**Related Risks**: None (low-risk decision)

---

## ADR-005: Batch Check Deduplication Strategy

**Status**: ✅ Accepted
**Date**: 2024-01-03
**Deciders**: Performance Team
**Review Trigger**: Complexity outweighs benefits OR memory overhead issues

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
- 20-50x improvement in batch check performance (requires validation)
- Requires careful coordination of in-flight requests
- Small memory overhead for tracking in-flight requests (~1-5MB)

**Alternatives Considered**:
- **No deduplication**: Simple but wastes computation on duplicate checks (OpenFGA baseline: 23 checks/s)
- **Cache-only**: Doesn't help with concurrent requests for same check
- **Two-stage (no singleflight)**: Misses opportunity to deduplicate across concurrent requests

**Validation Criteria**:
- [ ] Benchmark batch throughput (target: 500+ checks/s, baseline: 23 checks/s) (Milestone 1.7)
- [ ] Measure deduplication effectiveness (% of eliminated checks)
- [ ] Verify memory overhead <10MB under high load
- [ ] No race conditions in singleflight coordination

**Related Risks**: R-002 (Performance Claims Unvalidated)

---

## ADR-006: gRPC Streaming for Batch Results

**Status**: 🚧 Deferred to Post-MVP
**Date**: 2024-01-03
**Deciders**: API Team, Product Team
**Review Trigger**: User demand for streaming OR batch sizes >1000 checks

**Context**:
Large batch requests currently block until all checks complete. Streaming results would allow clients to process partial results.

**Decision**:
**Defer to post-MVP**, but design batch handler to support streaming in the future.

**Rationale**:
- MVP focuses on correctness and basic performance
- Streaming adds complexity to client implementation
- Can be added later without breaking changes (new RPC method)
- OpenFGA doesn't support streaming currently

**Future Implementation**:
```rust
// Current: unary RPC
async fn batch_check(req) -> Response<BatchCheckResponse>

// Future: streaming RPC
async fn batch_check_stream(req) -> ResponseStream<CheckResponse>
```

**Alternatives Considered**:
- **Implement in MVP**: Adds complexity without proven user need
- **Never implement**: Limits scalability for very large batches (>10k checks)

**Validation Criteria** (for future consideration):
- [ ] User requests for streaming (track GitHub issues)
- [ ] Benchmark unary vs streaming on large batches (>1000 checks)
- [ ] Client library support for streaming responses

**Related Risks**: None (deferred feature)

---

## ADR-007: Write Operations with Async Cache Invalidation

**Status**: ✅ Accepted
**Date**: 2024-01-03
**Deciders**: Performance Team, Security Team
**Review Trigger**: Consistency issues reported OR cache staleness >100ms p99

**Context**:
OpenFGA blocks write operations on cache invalidation (1-second timeout). This adds latency to write operations.

**Decision**:
Implement **async cache invalidation** that doesn't block the write response.

**Rationale**:
- Write latency dominated by database commit, not cache invalidation
- Eventual consistency acceptable for authorization (reads can tolerate brief stale cache)
- 2-3x improvement in write throughput (requires validation)

**Consequences**:
- Small window (1-10ms estimated) where stale cached results may be returned
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

**Alternatives Considered**:
- **Synchronous invalidation**: OpenFGA approach, 1s timeout adds latency
- **No invalidation**: Stale cache indefinitely, unacceptable for security
- **Write-through cache**: Higher write latency, not needed for authorization

**Validation Criteria**:
- [ ] Measure actual staleness window (target: <100ms p99) (Milestone 1.7)
- [ ] Write throughput improvement >2x vs sync invalidation
- [ ] Document consistency guarantees clearly
- [ ] Strong consistency mode works correctly

**Related Risks**: R-005 (Cache Consistency Issues)

---

## ADR-008: Connection Pooling Strategy

**Status**: ✅ Accepted
**Date**: 2024-01-03
**Deciders**: Infrastructure Team
**Review Trigger**: Pool exhaustion (>80% utilization) OR connection errors

**Context**:
Database connections are expensive to create (~10-50ms). We need efficient connection management for high throughput.

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
- 10 connections always warm (slight resource overhead: ~10MB)
- Fast connection acquisition under normal load
- Graceful degradation under high load (blocking when pool exhausted)

**Alternatives Considered**:
- **No pooling**: Simple but 10-50ms overhead per request
- **Smaller pool (max 50)**: May saturate under high load
- **Larger pool (max 200)**: More resources, may overwhelm database

**Validation Criteria**:
- [ ] Monitor pool utilization (alert at >80%) (production)
- [ ] Connection acquisition time <1ms p95 under normal load
- [ ] Graceful degradation when pool exhausted (no crashes)
- [ ] Automatic recovery from database restarts

**Related Risks**: R-004 (Database Performance Bottleneck)

---

## ADR-009: Error Handling Strategy

**Status**: ✅ Accepted
**Date**: 2024-01-03
**Deciders**: Architecture Team
**Review Trigger**: N/A (stable pattern)

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
- Follows Rust best practices

**Alternatives Considered**:
- **anyhow everywhere**: Loses type safety at domain boundaries
- **thiserror everywhere**: Too verbose for application-level code
- **std::error::Error only**: Lacks ergonomic derive macros

**Validation Criteria**:
- [ ] All domain modules have custom error types
- [ ] Error messages are clear and actionable
- [ ] Error context propagates correctly through layers
- [ ] No panic!() in production code paths

**Related Risks**: None (established best practice)

---

## ADR-010: Observability Stack

**Status**: ✅ Accepted
**Date**: 2024-01-03
**Deciders**: Infrastructure Team, Operations Team
**Review Trigger**: Tool limitations OR performance overhead >1%

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
- Low overhead when disabled (<1% estimated)

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

**Alternatives Considered**:
- **Prometheus client only**: No distributed tracing capability
- **OpenTelemetry only**: Overkill for metrics, better with tracing crate
- **Custom metrics**: Reinventing the wheel, poor ecosystem integration

**Validation Criteria**:
- [ ] Observability overhead <1% in production (Milestone 1.7)
- [ ] Metrics exported successfully to Prometheus
- [ ] Distributed traces viewable in Jaeger/Zipkin
- [ ] Structured logs parseable by log aggregators

**Related Risks**: None (established tooling)

---

## ADR-011: Testing Strategy

**Status**: ✅ Accepted
**Date**: 2024-01-03
**Deciders**: Architecture Team, QA Team
**Review Trigger**: Coverage drops <90% OR compatibility tests fail

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

**Alternatives Considered**:
- **Unit tests only**: Misses integration issues
- **E2E tests only**: Too slow for CI, hard to debug
- **Manual testing**: Not scalable, regression-prone

**Validation Criteria**:
- [ ] Unit test coverage >90% (Milestone 1.7)
- [ ] 100% OpenFGA compatibility test pass rate
- [ ] Integration tests cover all storage backends
- [ ] Performance tests establish baseline (Milestone 1.7)

**Related Risks**: R-003 (Authorization Correctness Bug), R-011 (Testing Strategy), R-016 (Insufficient Testing Resources)

---

## ADR-012: Storage Schema Design

**Status**: ✅ Accepted
**Date**: 2024-01-03
**Deciders**: Infrastructure Team, Domain Team
**Review Trigger**: Query performance issues OR write throughput <100 req/s

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
- Fast queries for graph traversal (expected <5ms p95)
- Write performance remains good (tuple writes are append-heavy)
- Index maintenance overhead acceptable (~10-20% write overhead)

**Alternatives Considered**:
- **Normalized schema**: More tables, complex joins, slower queries
- **Single index**: Fast for one pattern, slow for others
- **No indexes**: Unacceptable query performance (table scans)

**Validation Criteria**:
- [ ] Query performance <5ms p95 for tuple lookups (Milestone 1.7)
- [ ] Write throughput >100 req/s (Milestone 1.7)
- [ ] Index size <2x table size
- [ ] All query patterns use indexes (verify with EXPLAIN)

**Related Risks**: R-004 (Database Performance Bottleneck)

---

## ADR-013: Precomputation Architecture (Phase 2)

**Status**: 📋 Proposed (Phase 2)
**Date**: 2024-01-03
**Deciders**: Architecture Team, Performance Team
**Review Trigger**: Before Phase 2 starts OR Phase 1 performance insufficient

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

**Performance Target** (unvalidated):
- Check: <1ms p99 for cached results (vs ~20ms graph traversal)
- Batch: 500+ checks/second
- Write: May increase to 100-200ms (acceptable)

**Alternatives Considered**:
- **No precomputation**: Simpler but limited to ~1000 req/s check throughput
- **Full materialization**: Precompute everything, storage explosion (TB scale)
- **Query-time caching only**: Doesn't help first request, cold cache issues

**Validation Criteria** (Phase 2):
- [ ] Prototype validates <1ms p99 check latency
- [ ] Storage overhead acceptable (<10GB for 1M tuples)
- [ ] Write latency increase acceptable (<200ms p95)
- [ ] TTL eviction prevents unbounded storage growth

**Related Risks**: R-012 (Precomputation Storage Explosion)

---

## ADR-014: Edge Architecture Design with NATS (Phase 3)

**Status**: 📋 Proposed (Phase 3)
**Date**: 2024-01-03
**Deciders**: Architecture Team, Edge Team
**Review Trigger**: Before Phase 3 starts OR NATS proves insufficient in prototyping

**Context**:
Global deployments require low latency worldwide. Centralized architecture has inherent latency limits.

**Decision**:
Implement **product-based data partitioning** with edge nodes using **NATS** for synchronization:

**Topology**:
```
Edge Nodes (per region)
    ↓ (selective sync via NATS JetStream)
Regional Nodes (multi-region)
    ↓ (full replication via NATS)
Central Nodes (primary)
```

**Rationale**:
- Data locality reduces latency (edge <10ms, regional <50ms)
- Product-based partitioning reduces storage by 85-95% vs full replication
- NATS JetStream provides persistent messaging with minimal footprint
- NATS Leaf Nodes enable lightweight edge deployment
- Fallback to regional ensures correctness

**Data Tiers**:
- **Tier 1 (Critical)**: Models, metadata → replicate everywhere
- **Tier 2 (Hot)**: Recently accessed data → primary/secondary regions
- **Tier 3 (Warm)**: Cache-only → fetch on demand
- **Tier 4 (Cold)**: Audit logs → never replicate to edge

**Performance Target** (unvalidated):
- Edge: <10ms p95 globally
- Regional: <50ms p95
- Cluster: 100K+ req/s throughput

**Alternatives Considered**:
- **Kafka for edge**: Heavier (500MB+ vs 10-20MB NATS), more complex operations
- **Full replication**: Simple but 10-20x storage overhead, unacceptable at scale
- **No edge nodes**: Cannot achieve <10ms global latency

**Validation Criteria** (Phase 3):
- [ ] NATS throughput >100K msgs/s in production
- [ ] NATS Leaf Node stability in edge deployment
- [ ] Sync lag <5s p99 between edge and central
- [ ] Automatic fallback works on partition

**Related Risks**: R-006 (NATS Edge Sync Lag), R-014 (NATS vs Kafka Regret)

---

## ADR-015: Rust Edition and MSRV

**Status**: ✅ Accepted
**Date**: 2024-01-03
**Deciders**: Architecture Team
**Review Trigger**: Every 6 months OR new Rust edition released

**Context**:
Need to choose Rust edition and minimum supported Rust version (MSRV).

**Decision**:
- **Rust Edition**: 2021
- **MSRV**: 1.75.0 (or latest stable at project start)

**Rationale**:
- Edition 2021 provides latest language features
- Recent MSRV ensures access to modern crate ecosystem
- Will update MSRV every 6 months following Rust stable
- Balances stability with access to modern features

**Consequences**:
- Contributors need relatively recent Rust version
- Access to latest async/await improvements
- Better compile times and error messages

**Alternatives Considered**:
- **Rust 2018**: Older edition, missing modern async features
- **Older MSRV (1.60)**: Broader compatibility but missing ecosystem improvements
- **Bleeding edge (nightly)**: Unstable, not suitable for production

**Validation Criteria**:
- [ ] Document MSRV in README and CI
- [ ] CI tests against MSRV (not just latest)
- [ ] Review and update MSRV every 6 months

**Related Risks**: R-015 (Rust Compiler Bugs - low probability)

---

## ADR-016: Dependency Management Policy

**Status**: ✅ Accepted
**Date**: 2024-01-03
**Deciders**: Security Team, Architecture Team
**Review Trigger**: Security audit OR new dependency addition

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
- Concurrency: dashmap, arc-swap
- Caching: moka

**Forbidden**:
- Nightly-only features
- Deprecated crates
- Crates with known security issues
- Unmaintained crates (no updates in 12+ months)

**Alternatives Considered**:
- **Accept all dependencies**: Faster development but security/maintenance risk
- **Zero dependencies**: Unrealistic, reinventing wheels
- **Vendor all dependencies**: High maintenance burden

**Validation Criteria**:
- [ ] cargo audit passes in CI (weekly)
- [ ] All dependencies reviewed for security/maintenance
- [ ] Dependabot enabled for security updates
- [ ] Document rationale for each major dependency

**Related Risks**: R-007 (Dependency Vulnerabilities)

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
