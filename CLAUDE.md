# CLAUDE.md - Senior Architect's Guide for RSFGA

**Purpose**: This document defines the architectural guardrails, invariants, and decision frameworks for working on RSFGA. It is written from a senior architect's perspective to ensure architectural integrity throughout the project lifecycle.

**Audience**: AI assistants, senior engineers, and architects contributing to RSFGA

**Current Phase**: Architecture & Design ✅ Complete | Implementation ⏸️ Awaiting Approval

---

## Architectural Philosophy

RSFGA is not just a Rust rewrite of OpenFGA. It is a rethinking of authorization engine architecture with three core pillars:

1. **Correctness as Invariant**: Authorization bugs have severe security implications. A slow authorization system is better than an incorrect one.

2. **Compatibility as Contract**: 100% OpenFGA API compatibility is a business requirement, not a technical nice-to-have. Breaking compatibility invalidates the project's value proposition.

3. **Performance through Architecture**: Performance comes from architectural decisions (async, lock-free, parallelism), not micro-optimizations. Measure everything, assume nothing.

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

## Architectural Constraints (Non-Negotiable)

See [ARCHITECTURE.md - Constraints](docs/design/ARCHITECTURE.md#constraints) for full list.

**Quick Reference**:
- **C1**: 100% API compatibility (see I2)
- **C8**: Correctness > Performance (see I1)
- **C5**: Rust stable only (no nightly)
- **C11**: Graph depth limit = 25 (matches OpenFGA)

**Before violating a constraint**: Must create ADR with explicit justification and get architect approval.

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

## Quality Gates (Do Not Merge Without)

### For All Code Changes

1. ✅ Passes `cargo clippy` (no warnings)
2. ✅ Passes `cargo fmt` (formatted)
3. ✅ Unit tests pass (>90% coverage target)
4. ✅ No new security vulnerabilities (`cargo audit`)

### For Authorization Logic Changes

5. ✅ Property-based tests added
6. ✅ OpenFGA compatibility tests pass
7. ✅ Manual security review completed
8. ✅ Graph traversal invariants documented

### For Performance Optimizations

9. ✅ Benchmark before/after included
10. ✅ Profiling data shows bottleneck
11. ✅ Validation criteria in ADR
12. ✅ No correctness trade-offs (see I1)

### For New Dependencies

13. ✅ License compatible (Apache/MIT/BSD)
14. ✅ Actively maintained (commits in last 6 months)
15. ✅ No known CVEs
16. ✅ Justification in ADR-016

---

## Critical Code Paths (Extra Scrutiny Required)

These components are architecturally critical. Changes require senior review:

### 1. Graph Resolver (`rsfga-domain/src/resolver/`)

**Why Critical**: Determines authorization outcomes
**Invariants**:
- Cycle detection MUST prevent infinite loops
- Depth limiting MUST prevent DoS
- Correctness MUST be preserved during parallelization

**Quality Requirements**:
- >95% test coverage
- Property-based tests for all relation types
- Fuzz testing for edge cases

**Related**: I1, I4, R-003, R-010

---

### 2. Type System (`rsfga-domain/src/model/`)

**Why Critical**: Validates authorization models, prevents invalid tuples
**Invariants**:
- Parser MUST reject invalid DSL (no silent failures)
- Validation MUST catch type mismatches
- Model evolution MUST be backwards compatible

**Quality Requirements**:
- 100% OpenFGA DSL feature parity
- Comprehensive parser error handling
- Model compatibility tests

**Related**: I2, R-001

---

### 3. Storage Abstraction (`rsfga-storage/src/traits.rs`)

**Why Critical**: All tuple operations flow through here
**Invariants**:
- Atomicity MUST be preserved for write operations
- Read consistency MUST be documented (strong vs eventual)
- Transaction semantics MUST match OpenFGA

**Quality Requirements**:
- Backend-agnostic tests
- Transaction rollback testing
- Concurrent access testing

**Related**: C6, R-004

---

### 4. Cache Invalidation (`rsfga-domain/src/cache/`)

**Why Critical**: Stale cache = incorrect authorization
**Invariants**:
- Invalidation MUST eventually complete
- Staleness window MUST be bounded
- Strong consistency mode MUST bypass cache

**Quality Requirements**:
- Document consistency guarantees
- Measure actual staleness in production
- Provide opt-out (strong consistency mode)

**Related**: I1, A9, R-005

---

## Common Architectural Anti-Patterns to Avoid

### AP1: Premature Optimization

**Pattern**: Optimizing before profiling/benchmarking
**Why Bad**: Wastes effort, adds complexity without proof of benefit
**Instead**: Profile → Benchmark → Optimize → Validate

**Example**:
- ❌ BAD: "Let's use unsafe Rust for this cache lookup to avoid bounds checks"
- ✅ GOOD: "Profile shows cache lookup is 2% of latency, not worth optimizing"

---

### AP2: Abstraction for Abstraction's Sake

**Pattern**: Creating generic abstractions "for future flexibility"
**Why Bad**: YAGNI (You Aren't Gonna Need It), adds maintenance burden
**Instead**: Solve current problem simply, refactor when second use case appears

**Example**:
- ❌ BAD: Generic graph traversal engine that supports multiple graph types
- ✅ GOOD: Authorization graph traversal specific to OpenFGA model

---

### AP3: Micro-Optimizations Before Macro-Architecture

**Pattern**: Optimizing low-level details before architectural decisions
**Why Bad**: Architecture has 10-100x impact, micro-opts have 1-10% impact
**Instead**: Get architecture right (async, lock-free, parallel) first

**Example**:
- ❌ BAD: Optimize HashMap hashing function
- ✅ GOOD: Use lock-free DashMap instead of Mutex<HashMap>

---

### AP4: Ignoring Operational Concerns

**Pattern**: "Works on my machine" without considering deployment
**Why Bad**: Production failures, operational burden, user frustration
**Instead**: Design for observability, failure modes, and operations from day 1

**Example**:
- ❌ BAD: No metrics, no traces, crashes on DB connection loss
- ✅ GOOD: Metrics, traces, graceful degradation, health checks

---

### AP5: Compatibility as Nice-to-Have

**Pattern**: "We'll fix compatibility later" or "Our API is better"
**Why Bad**: Violates I2 invariant, invalidates project value
**Instead**: Compatibility is requirement #1, design within those constraints

**Example**:
- ❌ BAD: "Let's add this required field, it's more correct"
- ✅ GOOD: "Optional field to maintain compatibility, document migration path"

---

## When to Escalate to Architect

Escalate (create GitHub issue, mention in PR) if:

1. ❓ **Constraint Violation**: Change would violate C1-C12
2. ❓ **Invariant Trade-off**: Considering trading correctness for performance
3. ❓ **Novel Risk**: Identified new high/critical risk
4. ❓ **Assumption Invalidated**: Testing shows assumption A1-A10 is false
5. ❓ **Architectural Change**: Modifying system layers or component responsibilities
6. ❓ **Performance Mystery**: Optimization attempts failing or making things worse
7. ❓ **Security Concern**: Potential vulnerability in authorization logic

---

## Key Resources (Read Before Contributing)

### Must Read (in order):
1. **[ARCHITECTURE.md](docs/design/ARCHITECTURE.md)** - System design, assumptions, constraints, non-goals
2. **[ARCHITECTURE_DECISIONS.md](docs/design/ARCHITECTURE_DECISIONS.md)** - ADRs with context and validation criteria
3. **[RISKS.md](docs/design/RISKS.md)** - Risk register with mitigation strategies
4. **[ROADMAP.md](docs/design/ROADMAP.md)** - Implementation phases and milestones

### Reference:
5. **[API_SPECIFICATION.md](docs/design/API_SPECIFICATION.md)** - Complete API spec
6. **[DATA_MODELS.md](docs/design/DATA_MODELS.md)** - Data structures
7. **[C4_DIAGRAMS.md](docs/design/C4_DIAGRAMS.md)** - Architecture diagrams
8. **[EDGE_ARCHITECTURE_NATS.md](docs/design/EDGE_ARCHITECTURE_NATS.md)** - Phase 3 edge design

### External:
9. **[OpenFGA Docs](https://openfga.dev/docs)** - What we're compatible with
10. **[Zanzibar Paper](https://research.google/pubs/pub48190/)** - Foundational model

---

## Status & Next Steps

**Current Phase**: Architecture & Design ✅ Complete

**Completed**:
- ✅ Comprehensive architecture with assumptions/constraints/non-goals
- ✅ 16 ADRs with validation criteria and risk links
- ✅ Risk register with 17 identified risks
- ✅ Complete API specification
- ✅ Data models and schemas
- ✅ C4 diagrams
- ✅ NATS edge architecture

**Next**: **STOP.** Awaiting user approval to begin implementation (Milestone 1.1)

**DO NOT**:
- ❌ Start implementing without explicit approval
- ❌ Create Rust project structure
- ❌ Write code beyond design documents

**DO**:
- ✅ Refine design documents based on feedback
- ✅ Answer architecture questions
- ✅ Update ADRs/risks based on new information

---

**Remember**: Architecture is about making the right trade-offs and documenting them so future contributors understand the "why" behind decisions. When in doubt, ask questions before proceeding.

---

*Last Updated: 2024-01-03*
*Phase: Architecture & Design ✅ Complete | Implementation ⏸️ Awaiting Approval*
*Document Owner: Chief Architect*

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

**Note**: All targets require validation through benchmarking.

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

**Next**: Awaiting user approval to begin MVP implementation (Milestone 1.1)

**Blocked**: None

**Questions**: None

---

## Version History

- **2024-01-03**: Initial CLAUDE.md created
  - Architecture & design phase complete
  - All design documents finalized
  - Awaiting implementation approval

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
