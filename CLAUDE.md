# CLAUDE.md - AI Assistant Guide for RSFGA

This file provides guidance for AI assistants (like Claude) working on the RSFGA project.

---

## Project Overview

**RSFGA** is a high-performance Rust implementation of OpenFGA, an authorization/permission engine inspired by Google Zanzibar. The project aims for 100% API compatibility with OpenFGA while achieving 2-5x performance improvements.

**Current Phase**: Architecture & Design ✅ Complete | Implementation ⏸️ Awaiting Approval

---

## Key Principles

### 1. **Architecture-First Approach**
- All major changes require architecture review
- Document decisions in ADRs (Architecture Decision Records)
- Update design documents before implementing
- Get explicit approval before starting implementation

### 2. **OpenFGA Compatibility**
- Must maintain 100% API compatibility
- Follow OpenFGA's data models and behavior
- Pass OpenFGA's official test suite
- Any deviation requires explicit justification

### 3. **Performance Focus**
- Every optimization must be benchmarked
- Document performance targets and actual results
- Profile before optimizing
- Validate all performance claims through testing

### 4. **Safety and Correctness**
- Authorization is security-critical
- Correctness > Performance
- Comprehensive testing required
- Security review for critical paths

---

## Project Structure

```
rsfga/
├── README.md                          # Project overview
├── CLAUDE.md                          # This file (AI assistant guide)
├── docs/
│   └── design/
│       ├── ARCHITECTURE.md            # System architecture (READ FIRST)
│       ├── ROADMAP.md                 # Implementation roadmap
│       ├── ARCHITECTURE_DECISIONS.md  # Key decisions (16 ADRs)
│       ├── RISKS.md                   # Risk register
│       ├── API_SPECIFICATION.md       # Complete API spec
│       ├── DATA_MODELS.md             # Data structures & schemas
│       ├── EDGE_ARCHITECTURE_NATS.md  # Edge deployment (Phase 3)
│       └── C4_DIAGRAMS.md             # Architecture diagrams
└── (implementation to be added)
```

---

## Before Making Changes

### ✅ Checklist for AI Assistants

1. **Read the Design Docs**
   - [ ] Read [ARCHITECTURE.md](docs/design/ARCHITECTURE.md) for system overview
   - [ ] Check [ROADMAP.md](docs/design/ROADMAP.md) for current milestone
   - [ ] Review relevant ADRs in [ARCHITECTURE_DECISIONS.md](docs/design/ARCHITECTURE_DECISIONS.md)
   - [ ] Check [RISKS.md](docs/design/RISKS.md) for related risks

2. **Understand the Context**
   - [ ] What phase are we in? (Design vs Implementation)
   - [ ] What milestone is active?
   - [ ] Are there blocking dependencies?

3. **Get Approval for Major Changes**
   - [ ] Discuss approach with user
   - [ ] Document decision if architectural
   - [ ] Update relevant design docs
   - [ ] Only then proceed with implementation

4. **Follow Conventions**
   - [ ] Rust style: `cargo fmt`, `cargo clippy`
   - [ ] Naming: Follow OpenFGA conventions
   - [ ] Testing: Unit + integration tests
   - [ ] Documentation: Doc comments for public items

---

## Architecture Quick Reference

### System Layers (Bottom-Up)

1. **Storage Layer** (`rsfga-storage/`)
   - Abstraction: `DataStore` trait
   - Implementations: PostgreSQL, MySQL, In-Memory
   - Future: SQLite, Valkey/Redis

2. **Domain Layer** (`rsfga-domain/`)
   - Authorization model parser (DSL → AST)
   - Graph resolver (async, parallel)
   - Type system and validation
   - Check cache (DashMap + Moka)

3. **Server Layer** (`rsfga-server/`)
   - Request handlers (Check, Batch, Write, Read)
   - Business logic
   - Request validation

4. **API Layer** (`rsfga-api/`)
   - HTTP REST (Axum)
   - gRPC (Tonic)
   - Middleware (auth, metrics, tracing)

### Key Technologies

| Component | Technology | Why |
|-----------|------------|-----|
| Async Runtime | Tokio | Industry standard, excellent I/O parallelism |
| HTTP | Axum | Fast, ergonomic, Tower ecosystem |
| gRPC | Tonic | Pure Rust, performant |
| Database | SQLx | Async, compile-time query checking |
| Concurrency | DashMap | Lock-free, 10-100x faster than Mutex |
| Cache | Moka | TTL support, async-friendly |
| Observability | tracing + metrics | Industry standard |

### Performance Targets

| Operation | OpenFGA | RSFGA Target | Strategy |
|-----------|---------|--------------|----------|
| Check | 483 req/s | 1000+ req/s | Async graph, lock-free cache |
| Batch | 23 checks/s | 500+ checks/s | Parallel + dedup |
| Write | 59 req/s | 150+ req/s | Async invalidation |

**Note**: All targets require validation through benchmarking (60% confidence).

---

## Critical Architectural Invariants

**These MUST NOT be violated. Ever.**

### I1: Correctness Over Performance

**Rule**: Never trade authorization correctness for performance.

**Rationale**: A permission bug can:
- Grant unauthorized access (security breach)
- Deny legitimate access (service denial)
- Violate compliance requirements (legal liability)

**Examples**:
- ✅ GOOD: Cache timeout ensures eventual consistency
- ❌ BAD: Skip cycle detection to reduce latency
- ❌ BAD: Race condition in graph traversal to enable parallelism

**Quality Gate**: 100% OpenFGA test suite pass, property-based testing for all graph resolution paths

**Related**: Constraint C8, Risk R-003

---

### I2: 100% OpenFGA API Compatibility

**Rule**: All OpenFGA API endpoints, request/response formats, and behaviors must be identical.

**Rationale**: Users must be able to swap OpenFGA → RSFGA with zero code changes.

**Examples**:
- ✅ GOOD: Identical protobuf definitions
- ❌ BAD: "Better" API that breaks compatibility
- ❌ BAD: Additional required fields in requests

**Quality Gate**: Automated compatibility test suite in CI, no deviations without explicit ADR

**Related**: Constraint C1, Risk R-001

---

### I3: Performance Claims Require Validation

**Rule**: All performance targets are "unvalidated" until benchmarked. Document confidence levels.

**Rationale**: Unvalidated claims damage credibility and misguide development priorities.

**Examples**:
- ✅ GOOD: "Target: 1000 req/s (60% confidence, requires validation in M1.7)"
- ❌ BAD: "Will be 2-5x faster" (unqualified)
- ✅ GOOD: "Benchmark shows 3.2x speedup at p95"

**Quality Gate**: All performance ADRs must include validation criteria and risk links

**Related**: Assumption A1-A5, Risk R-002

---

### I4: Security-Critical Code Path Protection

**Rule**: Authorization resolution code (graph traversal, check logic) requires:
- Comprehensive unit tests (>95% coverage)
- Property-based tests for all relation types
- Manual security review
- Fuzz testing

**Rationale**: This code determines who can access what. Bugs here are catastrophic.

**Quality Gate**: No graph resolver changes merge without security review

**Related**: Risk R-003

---

## Decision Framework

When making architectural decisions, follow this framework:

### 1. Check Constraints First

**Question**: Does this violate any constraint (C1-C12)?
- **If yes**: Stop. Requires ADR and architect approval.
- **If no**: Continue to step 2.

### 2. Check Assumptions

**Question**: Does this rely on any assumption (A1-A10)?
- **If yes**: Document which assumptions, plan validation
- **If assumption false**: What's the fallback?

### 3. Identify Risks

**Question**: What could go wrong?
- Check [RISKS.md](docs/design/RISKS.md) for related risks
- Create new risk entry if novel risk identified
- Link decision to risk in ADR

### 4. Quantify Trade-offs

**Question**: What are we trading off?
- Performance vs Complexity?
- Memory vs Speed?
- Latency vs Throughput?

**Requirement**: Use numbers, not adjectives
- ✅ GOOD: "20% faster, 2x memory usage"
- ❌ BAD: "Much faster, slightly more memory"

### 5. Define Validation Criteria

**Question**: How will we know if this decision was correct?
- Define measurable success criteria
- Add validation checklist to ADR
- Schedule validation (which milestone?)

### 6. Document Decision

**Requirement**: Create ADR with:
- Status, Date, Deciders, Review Trigger
- Quantitative alternative comparison
- Validation criteria
- Risk links

---

## Development Phases

### Phase 1: MVP (Current - Awaiting Approval)
**Goal**: OpenFGA-compatible core with 2x performance

**Milestones**:
1. Project Foundation (weeks 1-2)
2. Type System & Parser (weeks 3-4)
3. Storage Layer (weeks 5-6)
4. Graph Resolver (weeks 7-8)
5. Batch Check Handler (week 9)
6. API Layer (week 10)
7. Testing & Benchmarking (weeks 11-12)

**Status**: Design complete, implementation not started

### Phase 2: Precomputation (Future)
- Precompute check results on writes
- Valkey/Redis for result storage
- Target: <1ms p99 check latency

### Phase 3: Distributed Edge (Future)
- NATS-based edge synchronization
- Product-based data partitioning
- Target: <10ms global latency

---

## Key Design Decisions (ADRs)

Quick reference to important ADRs:

- **ADR-001**: Tokio async runtime (I/O parallelism)
- **ADR-002**: DashMap for caching (lock-free)
- **ADR-003**: Parallel graph traversal (race-to-first-success)
- **ADR-005**: Three-stage batch deduplication
- **ADR-007**: Async cache invalidation (don't block writes)
- **ADR-013**: Precomputation architecture (Phase 2)
- **ADR-014**: NATS for edge (Phase 3, replaced Kafka)

See [ARCHITECTURE_DECISIONS.md](docs/design/ARCHITECTURE_DECISIONS.md) for full details.

---

## Code Conventions

### Rust Style

```rust
// ✅ GOOD: Clear, documented, tested
/// Check if user has permission
#[instrument(skip(self))]
pub async fn check(
    &self,
    user: &str,
    relation: &str,
    object: &str,
) -> Result<bool> {
    // Implementation
}

// ❌ BAD: Undocumented, synchronous, blocking
pub fn check(user: &str, rel: &str, obj: &str) -> bool {
    // Implementation
}
```

### Error Handling

```rust
// ✅ GOOD: Use thiserror for domain errors
#[derive(Error, Debug)]
pub enum ResolverError {
    #[error("depth limit exceeded (max: {max})")]
    DepthLimitExceeded { max: u32 },

    #[error("timeout after {duration:?}")]
    Timeout { duration: Duration },
}

// ✅ GOOD: Use anyhow for application errors
pub type Result<T> = anyhow::Result<T>;
```

### Testing

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_direct_check() {
        // Arrange
        let resolver = setup_test_resolver().await;

        // Act
        let result = resolver.check(
            "user:alice",
            "viewer",
            "document:doc1"
        ).await;

        // Assert
        assert!(result.is_ok());
        assert!(result.unwrap());
    }
}
```

---

## Common Tasks

### Adding a New Relation Type

1. Update `Relation` enum in `rsfga-domain/src/model.rs`
2. Add parser support in DSL parser
3. Implement resolution logic in `GraphResolver`
4. Add validation in `TypeSystem`
5. Write unit tests
6. Update documentation

### Adding a Storage Backend

1. Implement `DataStore` trait
2. Add connection pooling
3. Write schema migrations
4. Add integration tests (use testcontainers)
5. Update documentation

### Adding an API Endpoint

1. Define protobuf message (if gRPC)
2. Implement handler in `rsfga-server/src/handlers/`
3. Wire up in HTTP router (Axum)
4. Wire up in gRPC service (Tonic)
5. Add middleware (auth, validation)
6. Write integration tests
7. Update API documentation

---

## Testing Strategy

### Test Pyramid

1. **Unit Tests** (90%+ coverage)
   - Individual components
   - Pure functions
   - Fast, isolated

2. **Integration Tests**
   - Storage backends (with testcontainers)
   - API endpoints
   - Component interactions

3. **Compatibility Tests**
   - Run against OpenFGA test suite
   - Ensure API parity

4. **Performance Tests**
   - Benchmarks (Criterion)
   - Load tests (k6)
   - Compare against OpenFGA baseline

---

## Performance Guidelines

### Before Optimizing

1. **Profile first**: Use flamegraph, perf, cargo-bench
2. **Measure baseline**: Document current performance
3. **Set target**: Clear, measurable goal
4. **Optimize**: Make changes
5. **Validate**: Benchmark improvement
6. **Document**: Update docs with results

### Common Optimizations

```rust
// ✅ GOOD: Parallel execution for independent operations
let futures: Vec<_> = items.iter()
    .map(|item| async move { process(item).await })
    .collect();
let results = futures::future::join_all(futures).await;

// ❌ BAD: Sequential execution
let mut results = Vec::new();
for item in items {
    results.push(process(item).await);
}
```

```rust
// ✅ GOOD: Lock-free concurrent access
let cache: Arc<DashMap<K, V>> = Arc::new(DashMap::new());

// ❌ BAD: Mutex contention
let cache: Arc<Mutex<HashMap<K, V>>> = Arc::new(Mutex::new(HashMap::new()));
```

---

## Documentation Standards

### Code Comments

```rust
// ✅ GOOD: Explain WHY, not WHAT
// Cache result to avoid redundant graph traversal
// on subsequent checks with same parameters
cache.insert(key, result);

// ❌ BAD: Obvious comment
// Insert into cache
cache.insert(key, result);
```

### Doc Comments

```rust
/// Check if a user has a specific relation to an object.
///
/// This performs graph traversal through the authorization model,
/// following relation definitions until a direct assignment is found
/// or all paths are exhausted.
///
/// # Arguments
///
/// * `user` - User identifier (e.g., "user:alice")
/// * `relation` - Relation to check (e.g., "viewer")
/// * `object` - Object identifier (e.g., "document:readme")
///
/// # Returns
///
/// `Ok(true)` if user has the relation, `Ok(false)` otherwise.
/// Returns `Err` on timeout, cycle detection, or depth limit.
///
/// # Examples
///
/// ```
/// let allowed = resolver.check("user:alice", "viewer", "document:doc1").await?;
/// ```
pub async fn check(&self, user: &str, relation: &str, object: &str) -> Result<bool> {
    // Implementation
}
```

---

## Common Pitfalls

### ❌ Don't Do This

1. **Blocking I/O in async context**
   ```rust
   // ❌ BAD
   async fn query() {
       let result = std::fs::read_to_string("file.txt"); // Blocks!
   }

   // ✅ GOOD
   async fn query() {
       let result = tokio::fs::read_to_string("file.txt").await;
   }
   ```

2. **Holding locks across await points**
   ```rust
   // ❌ BAD
   let mut data = mutex.lock().await;
   expensive_async_operation().await; // Holding lock!
   data.insert(key, value);

   // ✅ GOOD
   let value = expensive_async_operation().await;
   let mut data = mutex.lock().await;
   data.insert(key, value);
   ```

3. **Ignoring errors**
   ```rust
   // ❌ BAD
   let _ = dangerous_operation().await;

   // ✅ GOOD
   dangerous_operation().await?;
   ```

4. **Premature optimization**
   ```rust
   // ❌ BAD: Optimizing without profiling
   // Added complex caching for something called once

   // ✅ GOOD: Profile first, then optimize hot paths
   ```

---

## Resources

### Design Documents (Read These First!)
- [ARCHITECTURE.md](docs/design/ARCHITECTURE.md) - System architecture
- [ROADMAP.md](docs/design/ROADMAP.md) - Implementation plan
- [ARCHITECTURE_DECISIONS.md](docs/design/ARCHITECTURE_DECISIONS.md) - Key decisions
- [RISKS.md](docs/design/RISKS.md) - Risk register
- [API_SPECIFICATION.md](docs/design/API_SPECIFICATION.md) - API reference
- [DATA_MODELS.md](docs/design/DATA_MODELS.md) - Data structures

### External References
- [OpenFGA Documentation](https://openfga.dev/docs)
- [Google Zanzibar Paper](https://research.google/pubs/pub48190/)
- [OpenFGA GitHub](https://github.com/openfga/openfga)

### Rust Resources
- [Tokio Documentation](https://tokio.rs)
- [Async Book](https://rust-lang.github.io/async-book/)
- [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)

---

## Communication with User

### Always Ask Before:
- Starting implementation (we're in design phase!)
- Making architectural changes
- Adding new dependencies
- Changing API contracts
- Modifying performance characteristics

### Provide Context:
- Explain your reasoning
- Reference design docs
- Show trade-offs
- Estimate impact

### Be Transparent:
- Acknowledge uncertainty
- Flag potential issues
- Suggest alternatives
- Ask clarifying questions

---

## Quick Command Reference

```bash
# Check compilation
cargo check

# Run tests
cargo test

# Run clippy
cargo clippy --all-targets --all-features

# Format code
cargo fmt --all

# Run benchmarks
cargo bench

# Build release
cargo build --release

# Run server (future)
cargo run --bin rsfga-server
```

---

## Status Tracking

**Current Status**: Architecture & Design Phase Complete ✅

**Completed**:
- ✅ Research & Analysis
- ✅ Architecture Design
- ✅ API Specification
- ✅ Data Models
- ✅ C4 Diagrams
- ✅ Edge Architecture (NATS)
- ✅ Implementation Roadmap
- ✅ Risk Register
- ✅ Senior Architect Documentation

**Next**: Awaiting user approval to begin MVP implementation (Milestone 1.1)

**Blocked**: None

**Questions**: None

---

## Version History

- **2024-01-03**: CLAUDE.md recreated
  - Streamlined structure
  - Senior architect perspective maintained
  - Clear invariants and decision framework
  - Comprehensive quality gates

---

## Final Notes for AI Assistants

1. **This is a design phase project** - Do NOT implement without explicit approval
2. **OpenFGA compatibility is critical** - Any deviation must be justified
3. **Performance claims require validation** - Always benchmark
4. **Security matters** - This is authorization infrastructure
5. **Document everything** - Architecture decisions, trade-offs, benchmarks

**When in doubt, ask the user before proceeding.**

---

*Last Updated: 2024-01-03*
*Phase: Architecture & Design ✅ Complete | Implementation ⏸️ Awaiting Approval*
