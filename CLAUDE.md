# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

---

# RSFGA - DEVELOPMENT GUIDE

**Project**: High-performance Rust implementation of OpenFGA
**Methodology**: Kent Beck's Test-Driven Development (TDD) + Tidy First principles
**Language**: Rust 1.75+ (Edition 2021)
**Test Coverage Target**: >90%
**Current Phase**: Architecture & Design ✅ Complete | Implementation ⏸️ Awaiting Approval

---

## PROJECT OVERVIEW

RSFGA is a high-performance authorization engine that provides 100% API compatibility with OpenFGA while achieving 2-5x performance improvements through Rust's zero-cost abstractions and async I/O.

- **OpenFGA Compatible**: Drop-in replacement with identical REST/gRPC APIs
- **High Performance**: 1000+ check req/s, 500+ batch checks/s (targets require validation)
- **Async-First**: Built on Tokio for I/O parallelism and concurrent graph traversal
- **Distributed Ready**: Designed for edge deployment with NATS synchronization (Phase 3)

### Quick Architecture

```
Client (HTTP/gRPC)
    ↓
API Layer (Axum/Tonic)
    ↓
Server Layer (Request Handlers)
    ↓
Domain Layer (Graph Resolver, Type System, Cache)
    ↓
Storage Abstraction (DataStore trait)
    ↓
Storage Backends (PostgreSQL, MySQL, In-Memory)
```

**Key Insight**: Authorization correctness is paramount. Graph traversal must be proven correct through property-based testing and 100% OpenFGA compatibility test suite compliance.

**Note**: OpenFGA doesn't provide a compatibility test suite, so Phase 0 builds one by testing OpenFGA behavior across all APIs. This test suite becomes our validation framework for RSFGA.

---

## CRITICAL ARCHITECTURAL INVARIANTS

**These MUST NOT be violated. Ever.**

### I1: Correctness Over Performance
- Never trade authorization correctness for performance
- A permission bug can grant unauthorized access or deny legitimate access
- Quality Gate: 100% compatibility test suite pass (Phase 0) + property-based testing

### I2: 100% OpenFGA API Compatibility
- All endpoints, request/response formats, and behaviors must be identical
- Users must swap OpenFGA → RSFGA with zero code changes
- Quality Gate: Automated compatibility test suite in CI (built in Phase 0)

### I3: Performance Claims Require Validation
- All performance targets are "unvalidated" until benchmarked (60% confidence)
- Document confidence levels explicitly
- Quality Gate: Validation criteria in ADRs, links to risks

### I4: Security-Critical Code Path Protection
- Graph resolver requires >95% coverage, property tests, security review, fuzz testing
- Quality Gate: No graph resolver changes merge without security review

See [docs/design/ARCHITECTURE.md](docs/design/ARCHITECTURE.md) for full constraints (C1-C12) and assumptions (A1-A10).

---

## DEVELOPMENT COMMANDS

### Build & Test

```bash
# Build the project
cargo build

# Build release binary
cargo build --release

# Run all tests
cargo test

# Run only unit tests
cargo test --lib

# Run only integration tests
cargo test --test '*'

# Run specific test by name
cargo test [test_name]

# Run tests with output visible
cargo test -- --nocapture

# Run tests with backtrace
RUST_BACKTRACE=1 cargo test

# Run benchmarks (requires nightly or criterion)
cargo bench
```

### Code Quality

```bash
# Lint (must pass before commit)
cargo clippy --all-targets --all-features -- -D warnings

# Format code (must pass before commit)
cargo fmt --all

# Check formatting without applying
cargo fmt --check

# Test coverage report (requires cargo-tarpaulin)
cargo tarpaulin --out Html --output-dir coverage

# Security audit (requires cargo-audit)
cargo audit

# Check for unused dependencies
cargo udeps
```

### Running the Application

```bash
# Run with default configuration
cargo run --bin rsfga-server

# Run with custom config file
cargo run --bin rsfga-server -- --config config.yaml

# Run with environment variables
DATABASE_URL=postgres://localhost/rsfga cargo run

# Run in release mode
cargo run --release --bin rsfga-server
```

---

## CODE ARCHITECTURE

### Module Structure

```
rsfga/
├── rsfga-api/             # HTTP/gRPC API layer
│   ├── src/
│   │   ├── http/          # Axum REST endpoints
│   │   └── grpc/          # Tonic gRPC services
│   └── Cargo.toml
├── rsfga-server/          # Request handlers and business logic
│   ├── src/
│   │   ├── handlers/      # Check, Batch, Write, Read handlers
│   │   └── middleware/    # Auth, metrics, tracing
│   └── Cargo.toml
├── rsfga-domain/          # Core authorization logic
│   ├── src/
│   │   ├── model/         # Type system, DSL parser, AST
│   │   ├── resolver/      # Graph resolver (async, parallel)
│   │   ├── cache/         # Check cache (DashMap + Moka)
│   │   └── validation/    # Model validation
│   └── Cargo.toml
├── rsfga-storage/         # Storage abstraction layer
│   ├── src/
│   │   ├── traits.rs      # DataStore trait
│   │   ├── postgres.rs    # PostgreSQL implementation
│   │   ├── mysql.rs       # MySQL implementation
│   │   └── memory.rs      # In-memory implementation
│   └── Cargo.toml
└── docs/design/           # Architecture documentation
    ├── ARCHITECTURE.md
    ├── ARCHITECTURE_DECISIONS.md
    ├── RISKS.md
    └── ROADMAP.md
```

### Key Design Principles

**1. Async-First Architecture**
- Leverage Tokio for I/O parallelism and concurrent graph traversal
- All database operations are async (SQLx)
- Why: 2-3x performance improvement through parallelism (ADR-001)

**2. Lock-Free Concurrency**
- Use DashMap for cache instead of Mutex<HashMap>
- Sharded internal structure distributes lock pressure
- Why: 10-100x faster under high contention (ADR-002, requires validation)

**3. Fail Fast with Bounds**
- Depth limit: 25 (matches OpenFGA, prevents DoS)
- Cycle detection: MUST prevent infinite loops
- Timeouts on all I/O operations
- Why: Security and availability (Constraint C11)

**4. Strong Type Safety**
- Newtype pattern for validated data
- Thiserror for domain errors, anyhow for application errors
- Why: Prevent invalid states at compile time (ADR-009)

---

## TDD WORKFLOW WITH PLAN.MD

### Starting a New Phase

Before implementing tests for a new phase:

1. Ensure you're on updated `main`: `git checkout main && git pull`
2. Create a feature branch: `git checkout -b feature/phaseX-description`
3. **CRITICAL**: Review relevant ADRs and risks in [docs/design/](docs/design/)
4. Now you're ready to start the TDD cycle

### The "Go" Command

When you see "go" from the user:

1. Read `docs/design/ROADMAP.md` to find the next milestone/phase
2. Implement the test (Red phase - watch it fail)
3. Write minimum code to pass (Green phase)
4. Refactor if needed (keep tests green)
5. Mark test complete in ROADMAP.md
6. Commit with appropriate prefix
7. Wait for next "go"
8. **When phase is complete**: Push branch and create PR for review

### Test Execution Strategy

- **Red Phase**: Write failing test first (TDD)
- **Green Phase**: Minimum code to pass (simplicity)
- **Refactor Phase**: Improve structure while tests stay green
- **Commit**: Mark test complete, commit with prefix

### Decision Framework Integration

Before implementing any feature:

1. **Check Constraints** (C1-C12) - Does this violate any?
2. **Check Assumptions** (A1-A10) - Does this rely on unvalidated assumptions?
3. **Identify Risks** - Check [RISKS.md](docs/design/RISKS.md)
4. **Quantify Trade-offs** - Use numbers, not adjectives
5. **Define Validation Criteria** - How will we know it works?
6. **Document Decision** - Create ADR if architectural

---

## CORE DEVELOPMENT PRINCIPLES

- **Correctness > Performance** (Invariant I1)
- **Compatibility is non-negotiable** (Invariant I2)
- **Validate all performance claims** (Invariant I3)
- **Security-critical paths require extra scrutiny** (Invariant I4)
- Follow TDD cycle: **Red → Green → Refactor**
- Separate structural changes from behavioral changes (Tidy First)
- Maintain >90% test coverage
- Profile before optimizing
- Document architectural decisions in ADRs

---

## TDD METHODOLOGY GUIDANCE

- Start by writing a failing test that defines a small increment of functionality
- Use meaningful test names that describe behavior:
  - `test_check_returns_true_for_direct_tuple_assignment`
  - `test_batch_check_deduplicates_identical_requests`
  - `test_graph_resolver_detects_cycles_and_returns_error`
- Make test failures clear and informative with descriptive assertion messages
- Write just enough code to make the test pass - no more
- Once tests pass, consider if refactoring is needed
- Repeat the cycle for new functionality
- When fixing a defect:
  1. First write an API-level failing test that demonstrates the bug
  2. Then write the smallest possible test that replicates the problem
  3. Finally, implement the fix to make both tests pass

---

## TIDY FIRST APPROACH

Separate all changes into two distinct types:

### 1. STRUCTURAL CHANGES

Rearranging code without changing behavior:

- Renaming variables, methods, or modules for clarity
- Extracting methods or functions
- Moving code to more appropriate locations
- Reorganizing imports or dependencies
- Reformatting code

### 2. BEHAVIORAL CHANGES

Adding or modifying actual functionality:

- Implementing new features
- Fixing bugs that change program behavior
- Modifying algorithms or logic
- Adding new dependencies that change behavior

### Critical Rules:

- **Never mix structural and behavioral changes in the same commit**
- **Always make structural changes first** when both are needed
- Validate structural changes do not alter behavior by running tests before and after
- If a structural change breaks tests, revert and investigate

---

## BRANCHING STRATEGY

### Golden Rule: Never Commit Directly to Main

- **NEVER commit directly to the `main` branch** - No exceptions
- **Always create a feature branch** for new implementation
- Feature branches should be created from the latest `main`

### Branch Naming Convention

```
feature/milestone-[number]-[brief-description]
fix/[issue-number]-[brief-description]
refactor/[brief-description]
docs/[brief-description]
```

**Examples:**
```bash
feature/milestone-1.1-project-foundation
feature/milestone-1.4-graph-resolver
fix/42-cycle-detection-infinite-loop
refactor/extract-cache-module
docs/update-adr-001-tokio-validation
```

### Branch Workflow

```bash
# Start new feature (always from updated main)
git checkout main
git pull origin main
git checkout -b feature/milestone-1.1-project-foundation

# Work on the feature with regular commits
# ... make changes ...
git add .
git commit -m "[BEHAVIORAL] Add workspace structure"

# Push feature branch regularly
git push origin feature/milestone-1.1-project-foundation
```

### Pull Request Strategy - Small, Reviewable PRs

**Golden Rule**: Create PRs per section or small group of related sections (5-15 tests max)

After completing a **section** or logical group of tests:

1. **Ensure all quality gates pass** (tests, clippy, fmt, audit)
2. **Push your feature branch** to the remote repository
3. **Create a Pull Request (PR)** for review
4. **PR Title Format**: `[Milestone X.Y] Section N: Brief description`
   - Example: `[Milestone 1.2] Section 1: Core Type Definitions`
   - Example: `[Milestone 1.4] Sections 1-2: Direct & Computed Relations`
5. **PR Description** should include:
   - Summary of changes (what was implemented)
   - List of completed tests from plan.md (mark them [x])
   - Why decisions (if architectural choices made)
   - Test coverage metrics
   - Any dependencies or blockers for next section
6. **Wait for review and approval** before merging
7. **Merge to main** (prefer squash merge for clean history)
8. **Continue on same branch** for next section in milestone
9. **Delete branch only** when entire milestone is complete

**PR Size Guidelines**:
- ✅ **Good**: 5-15 tests, <500 lines changed, 1 focused area
- ⚠️ **Too Large**: 30+ tests, >1000 lines, multiple unrelated changes
- ❌ **Way Too Large**: Entire milestone in one PR

**Benefits of Small PRs**:
- Faster review cycles (minutes to hours, not days)
- Easier to spot issues
- Less risk of conflicts
- Simpler to revert if needed
- Better feedback loop

### Updating Status in plan.md at Each PR

**CRITICAL**: After each PR is merged, update `plan.md` to reflect the current implementation status:

1. **Mark completed tests**: Change `[ ]` to `[x]` for all tests that are now passing
2. **Update milestone status**: Add `✅ COMPLETE` to milestone header when all tests pass
3. **Update validation criteria**: Mark criteria as complete when met
4. **Update Current Status section**: Keep the bottom of plan.md accurate with:
   - Current phase and milestone in progress
   - What's been completed
   - What's next
5. **Commit status update**: Include plan.md updates in the PR or as a follow-up commit

**Example workflow after PR merge**:
```bash
# After PR is merged to main
git checkout main
git pull origin main

# Update plan.md with completed tests
# Mark tests [x], update milestone status, etc.

# Commit the status update
git add plan.md
git commit -m "[DOCS] Update plan.md status after Milestone X.Y completion"
git push origin main
```

**Status Indicators**:
- `[ ]` - Test not yet implemented
- `[x]` - Test implemented and passing
- `⏸️ Pending` - Work not yet started
- `🏗️ In Progress` - Currently being worked on
- `✅ COMPLETE` - Milestone/section fully complete

---

## COMMIT DISCIPLINE

Only commit when:

1. **ALL tests are passing** - No exceptions
2. **ALL compiler/linter warnings have been resolved** - Zero warnings policy
3. **Code is properly formatted** - Run `cargo fmt` before commit
4. **The change represents a single logical unit of work** - One concept per commit
5. **Commit messages clearly state** whether the commit contains structural or behavioral changes
6. **You are on a feature branch** - Never on main
7. **Security audit passes** - Run `cargo audit` for dependency changes

### Commit Message Format:

```
[STRUCTURAL] Extract graph traversal logic into separate module
[BEHAVIORAL] Implement parallel union relation resolution
[BEHAVIORAL] Fix cache invalidation race condition
[STRUCTURAL] Rename CheckContext to AuthorizationContext for clarity
[DOCS] Update ADR-003 with parallel traversal validation results
[TEST] Add property-based tests for cycle detection
[CHORE] Update SQLx to 0.7 for async improvements
```

Use small, frequent commits rather than large, infrequent ones. Each commit should tell a story.

---

## CODE QUALITY STANDARDS

### General Standards

- **Eliminate duplication ruthlessly** - DRY principle applied consistently
- **Express intent clearly** through naming and structure
- **Make dependencies explicit** - No hidden coupling
- **Keep methods small** and focused on a single responsibility
- **Minimize state and side effects** - Prefer pure functions when possible
- **Use the simplest solution** that could possibly work - YAGNI principle
- **Document why, not what** - The code shows what; comments explain why

### Rust-Specific Quality Standards

- Use `Result<T, E>` for error handling (no panics in production code)
- Prefer `&str` over `String` for function parameters when ownership not needed
- Use `impl Trait` or generics to avoid dynamic dispatch when possible
- Leverage Rust's type system for correctness (newtype pattern for validation)
- Write idiomatic Rust (follow clippy suggestions)
- Prefer `thiserror` for library errors and `anyhow` for application errors
- Use `#[must_use]` for functions where ignoring the return value is likely a bug
- Implement `Debug`, `Clone`, and other standard traits where appropriate
- Use `cfg(test)` modules for test-only code
- Prefer `const` and `static` for compile-time constants

### RSFGA-Specific Standards

**Authorization Logic (rsfga-domain/src/resolver/)**:
- >95% test coverage required
- Property-based tests for all relation types
- Fuzz testing for edge cases
- Manual security review for all changes

**API Compatibility (rsfga-api/)**:
- Identical protobuf definitions to OpenFGA
- Compatibility test suite must pass 100%
- Any deviation requires ADR and explicit justification

**Performance-Critical Code**:
- Profile before optimizing
- Benchmark before/after
- Document validation criteria in ADR
- Never trade correctness for performance

### Error Handling Best Practices

```rust
// Define custom error types with thiserror
#[derive(Debug, thiserror::Error)]
pub enum ResolverError {
    #[error("depth limit exceeded (max: {max})")]
    DepthLimitExceeded { max: u32 },

    #[error("timeout after {duration:?}")]
    Timeout { duration: Duration },

    #[error("cycle detected: {path}")]
    CycleDetected { path: String },

    #[error("storage error: {0}")]
    Storage(#[from] StorageError),
}

// Use Result for fallible operations
pub async fn resolve_check(
    &self,
    ctx: &CheckContext,
) -> Result<bool, ResolverError> {
    // Implementation
}
```

---

## REFACTORING GUIDELINES

- **Refactor only when tests are passing** (in the "Green" phase)
- **Use established refactoring patterns** with their proper names:
  - Extract Method/Function
  - Rename Variable/Function/Module
  - Move Method/Function
  - Extract Module
  - Inline Method/Function
  - Extract Trait
  - Introduce Newtype
- **Make one refactoring change at a time** - Small, safe steps
- **Run tests after each refactoring step** - Continuous validation
- **Prioritize refactorings that**:
  - Remove duplication
  - Improve clarity and readability
  - Reduce complexity
  - Make future changes easier
  - Improve type safety

---

## TESTING STRATEGY

### Test Pyramid

```
     /\
    /  \     E2E Tests (5%) - Full API tests
   /____\
  /      \   Integration Tests (15%) - Component interactions
 /________\
/          \ Unit Tests (80%) - Individual functions
```

### Test Levels

**Unit Tests** (>90% coverage target)

- Individual functions and methods
- Fast execution (<1s total for all unit tests)
- No external dependencies (use mocks/stubs)
- Test behavior, not implementation
- Property-based tests for graph resolver

**Integration Tests** (all critical paths)

- Component interactions (storage + domain, server + API)
- May use testcontainers for databases
- Moderate execution time (<10s total)
- OpenFGA compatibility test suite

**End-to-End Tests** (main workflows)

- Full application stack (HTTP/gRPC → storage)
- Performance benchmarks
- Mark with `#[ignore]` for normal test runs
- Run before releases

### Test File Organization

```
tests/
├── unit/                    # Fast unit tests
│   ├── model_test.rs       # Type system tests
│   ├── resolver_test.rs    # Graph resolver tests
│   └── cache_test.rs       # Cache behavior tests
├── integration/             # Component integration
│   ├── storage_backends_test.rs
│   ├── api_handlers_test.rs
│   └── compatibility/      # OpenFGA test suite
│       └── ...
└── e2e/                    # Full stack tests
    └── authorization_flows_test.rs
```

### Testing Best Practices

- Keep tests fast - slow tests kill TDD rhythm
- Make tests independent - no test should depend on another
- Use test doubles (mocks, stubs, fakes) judiciously
- Test behavior, not implementation details
- Use descriptive test names that explain the scenario
- Arrange-Act-Assert (AAA) pattern for test structure
- Property-based tests for complex logic (use proptest)

### Example Test Structure

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_check_returns_true_for_direct_tuple_assignment() {
        // Arrange
        let storage = InMemoryStorage::new();
        storage.write_tuple(Tuple {
            user: "user:alice",
            relation: "viewer",
            object: "document:readme",
        }).await.unwrap();

        let resolver = GraphResolver::new(storage);

        // Act
        let result = resolver.check(
            "user:alice",
            "viewer",
            "document:readme",
        ).await;

        // Assert
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), true);
    }

    #[tokio::test]
    async fn test_check_returns_error_when_depth_limit_exceeded() {
        // Arrange
        let storage = create_deep_hierarchy(30); // Depth limit is 25
        let resolver = GraphResolver::new(storage);

        // Act
        let result = resolver.check("user:alice", "viewer", "object:deep").await;

        // Assert
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ResolverError::DepthLimitExceeded { max: 25 }));
    }

    #[test]
    fn proptest_cycle_detection_always_prevents_infinite_loops() {
        use proptest::prelude::*;

        proptest!(|(tuples in arbitrary_tuple_set())| {
            let storage = InMemoryStorage::from(tuples);
            let resolver = GraphResolver::new(storage);

            // Should either succeed or return CycleDetected error
            // but NEVER hang or panic
            let result = timeout(
                Duration::from_secs(1),
                resolver.check("user:alice", "viewer", "object:test")
            ).await;

            prop_assert!(result.is_ok()); // Timeout didn't fire
        });
    }
}
```

---

## QUALITY GATES (Must Pass Before Commit)

```bash
# All must succeed:
git branch --show-current | grep -v '^main$'     # ✅ Not on main branch
cargo test                                        # ✅ All tests passing
cargo clippy --all-targets --all-features -- -D warnings  # ✅ No warnings
cargo fmt --check                                 # ✅ Code formatted
cargo audit                                       # ✅ No security vulnerabilities
# Coverage >90% (check with cargo tarpaulin)
```

**Never commit when:**

- You are on the `main` branch
- Any test is failing
- Clippy shows any warnings
- Code is not formatted
- Cargo audit shows vulnerabilities
- Mixing structural and behavioral changes
- Performance claims are unvalidated (use `// TODO: Validate in M1.7`)

**Never merge to main without:**

- A Pull Request
- Passing CI checks (tests, clippy, fmt, audit)
- OpenFGA compatibility tests passing (if API changes)
- Code review approval
- ADRs updated (if architectural changes)
- ROADMAP.md milestone marked complete

---

## DEPENDENCIES (Cargo.toml)

### Core Dependencies

```toml
[workspace]
members = ["rsfga-api", "rsfga-server", "rsfga-domain", "rsfga-storage"]

[workspace.dependencies]
# Async runtime (ADR-001)
tokio = { version = "1", features = ["full"] }

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Error handling (ADR-009)
thiserror = "1"
anyhow = "1"

# Logging and tracing (ADR-010)
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
metrics = "0.21"

# HTTP/gRPC
axum = "0.7"
tonic = "0.11"
prost = "0.12"

# Database (ADR-008)
sqlx = { version = "0.7", features = ["postgres", "mysql", "runtime-tokio"] }

# Concurrency (ADR-002)
dashmap = "5"
moka = { version = "0.12", features = ["future"] }

[workspace.dev-dependencies]
# Testing
tokio-test = "0.4"
proptest = "1"
mockall = "0.12"
testcontainers = "0.15"
criterion = "0.5"  # Benchmarking
```

### Dependency Policy (ADR-016)

- Minimize dependencies
- Prefer tier-1 crates (tokio, serde, sqlx)
- No nightly-only features
- Run `cargo audit` weekly
- Document rationale for major dependencies in ADRs

---

## COMMON PATTERNS

### Newtype Pattern for Validation

```rust
#[derive(Debug, Clone)]
pub struct UserId(String);

impl UserId {
    pub fn new(id: impl Into<String>) -> Result<Self, ValidationError> {
        let id = id.into();
        if id.is_empty() || !id.starts_with("user:") {
            return Err(ValidationError::InvalidUserId(id));
        }
        Ok(Self(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for UserId {
    type Err = ValidationError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}
```

### Async Trait Pattern (ADR-004)

```rust
#[async_trait]
pub trait DataStore: Send + Sync {
    async fn write_tuple(&self, tuple: &Tuple) -> Result<(), StorageError>;
    async fn read_tuples(
        &self,
        filter: &TupleFilter,
    ) -> Result<Vec<Tuple>, StorageError>;
    async fn delete_tuple(&self, tuple: &Tuple) -> Result<(), StorageError>;
}

pub struct PostgresStore {
    pool: PgPool,
}

#[async_trait]
impl DataStore for PostgresStore {
    async fn write_tuple(&self, tuple: &Tuple) -> Result<(), StorageError> {
        sqlx::query!(
            "INSERT INTO tuples (user, relation, object) VALUES ($1, $2, $3)",
            tuple.user,
            tuple.relation,
            tuple.object
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    // ...
}
```

### Parallel Graph Traversal (ADR-003)

```rust
// Union relations: race to first success
let futures: Vec<_> = union_relations
    .iter()
    .map(|rel| self.resolve_relation(ctx, rel))
    .collect();

// Return as soon as ANY relation resolves to true
match futures::future::select_ok(futures).await {
    Ok((true, _)) => return Ok(true),
    Ok((false, _)) | Err(_) => return Ok(false),
}

// Intersection relations: all must succeed
let results = futures::future::join_all(
    intersection_relations
        .iter()
        .map(|rel| self.resolve_relation(ctx, rel))
).await;

Ok(results.iter().all(|r| r.as_ref().map_or(false, |&b| b)))
```

---

## PERFORMANCE TARGETS

| Metric                       | OpenFGA Baseline | RSFGA Target    | Validation Status   |
| ---------------------------- | ---------------- | --------------- | ------------------- |
| Check throughput             | 483 req/s        | 1000+ req/s     | ⏸️ M1.7 (60% conf) |
| Batch check throughput       | 23 checks/s      | 500+ checks/s   | ⏸️ M1.7 (60% conf) |
| Write throughput             | 59 req/s         | 150+ req/s      | ⏸️ M1.7 (60% conf) |
| Check p95 latency            | 22ms             | <20ms           | ⏸️ M1.7 (60% conf) |
| Cache staleness window       | 1s timeout       | <100ms p99      | ⏸️ M1.7 (A9)       |
| Memory usage (baseline)      | TBD              | <500MB          | ⏸️ M1.7            |
| Startup time                 | TBD              | <2s             | ⏸️ M1.1            |

**All targets require validation through benchmarking. See ADRs for validation criteria.**

---

## DOCUMENTATION RESOURCES

- **README.md** - Project overview, quick start, status
- **docs/design/ARCHITECTURE.md** - Complete system architecture, assumptions, constraints
- **docs/design/ARCHITECTURE_DECISIONS.md** - 16 ADRs with validation criteria
- **docs/design/RISKS.md** - Risk register with 17 identified risks
- **docs/design/ROADMAP.md** - Implementation plan (7 milestones)
- **docs/design/API_SPECIFICATION.md** - Complete REST/gRPC API spec
- **docs/design/DATA_MODELS.md** - Data structures and schemas
- **docs/design/C4_DIAGRAMS.md** - Architecture diagrams
- **docs/design/EDGE_ARCHITECTURE_NATS.md** - Phase 3 edge deployment

---

## WORKFLOW SUMMARY

```
┌─────────────────────────────────────────────────────┐
│ MILESTONE START:                                    │
│ 1. Create feature branch from main                  │
│    git checkout -b feature/milestone-X.Y-name       │
│ 2. Review relevant ADRs and risks                   │
│                                                     │
│ SECTION CYCLE (repeat for each section):            │
│ 3. User says "go"                                   │
│ 4. Check plan.md for next test in section           │
│ 5. Write failing test (Red)                         │
│ 6. Write minimum code to pass (Green)               │
│ 7. Refactor if needed (keep green)                  │
│ 8. Run quality gates (test, clippy, fmt, audit)     │
│ 9. Mark test [x] in plan.md                         │
│ 10. Commit with [BEHAVIORAL] or [STRUCTURAL] prefix │
│ 11. Repeat until section complete (5-15 tests)      │
│                                                     │
│ SECTION END (Create PR):                            │
│ 12. Push feature branch                             │
│ 13. Create PR: [Milestone X.Y] Section N: Title     │
│ 14. Wait for review and approval                    │
│ 15. Merge to main (squash merge)                    │
│ 16. Continue on same branch for next section        │
│                                                     │
│ MILESTONE END:                                      │
│ 17. Verify all milestone validation criteria met    │
│ 18. Update ADRs if architectural changes             │
│ 19. Delete feature branch                           │
│ 20. Start next milestone on new branch              │
└─────────────────────────────────────────────────────┘
```

Follow this process precisely, always prioritizing:
1. **Correctness** over performance (I1)
2. **Compatibility** over innovation (I2)
3. **Validation** over claims (I3)
4. **Security** over convenience (I4)

**Quality is not negotiable.**

---

## WHEN IN DOUBT

- **Consult the invariants** (I1-I4) - These are non-negotiable
- **Check constraints** (C1-C12 in ARCHITECTURE.md) - May require ADR to violate
- **Review assumptions** (A1-A10 in ARCHITECTURE.md) - Plan validation if relying on them
- **Check risks** (RISKS.md) - Related to what you're implementing?
- **Read relevant ADRs** (ARCHITECTURE_DECISIONS.md) - Context for decisions
- **Write a test first** - When uncertain, TDD guides you
- **Ask the user** - Better to clarify than assume

---

**Current Status**:
- Phase 0 (Compatibility Test Suite): ✅ Complete (194 tests)
- Phase 1 (MVP Implementation): ✅ Complete (~918 tests total)
  - Milestone 1.1 (Project Foundation): ✅ Complete
  - Milestone 1.2 (Type System & Parser): ✅ Complete
  - Milestone 1.3 (Storage Layer): ✅ Complete
  - Milestone 1.4 (Graph Resolver): ✅ Complete
  - Milestone 1.5 (Check Cache): ✅ Complete
  - Milestone 1.6 (Batch Check Handler): ✅ Complete
  - Milestone 1.7 (API Layer): ✅ Complete
  - Milestone 1.8 (Testing & Benchmarking): ✅ Complete
  - Milestone 1.9 (Production Readiness): ✅ Complete
  - Milestone 1.10 (CEL Condition Evaluation): ✅ Complete
  - Milestone 1.11 (MySQL/MariaDB/TiDB Storage): ✅ Complete
  - Milestone 1.12 (CockroachDB Storage): ✅ Complete

**Next Step**: Phase 2 - Precomputation Engine (Optional)

**To continue**: Check `plan.md` for detailed test status and say "go" to proceed!
