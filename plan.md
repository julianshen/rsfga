# RSFGA Implementation Plan

**Test-Driven Development Plan following Kent Beck's TDD methodology**

This document outlines the complete implementation plan for RSFGA, broken down into specific testable increments. Each test represents a small, verifiable step towards the complete system.

---

## Terminology

To avoid confusion, we use these terms consistently:

- **Phase** = Major implementation stage (4 total: Compatibility Test Suite, MVP, Precomputation, Edge)
  - Example: "Phase 0: Compatibility Test Suite", "Phase 1: MVP", "Phase 2: Precomputation"

- **Milestone** = Time-boxed deliverable within a phase (~1-2 weeks)
  - Example: "Milestone 1.1: Project Foundation", "Milestone 1.2: Type System"

- **Section** = Logical grouping of related tests within a milestone
  - Example: "Section 1: Core Type Definitions", "Section 2: DSL Parser"

**Structure**: Phase → Milestone → Section → Individual Tests

---

## How to Use This Plan

1. User says **"go"**
2. Find the next unchecked test `[ ]`
3. Write the test (Red phase - watch it fail)
4. Implement minimum code to pass (Green phase)
5. Refactor if needed (keep tests green)
6. Mark test `[x]` and commit with `[BEHAVIORAL]` or `[STRUCTURAL]` prefix
7. Repeat

**Branch naming**: `feature/milestone-X.Y-description`
**PR after**: Each section completion (5-15 tests, <500 lines)

---

## Phase 0: Compatibility Test Suite

**Goal**: Create comprehensive test suite that validates OpenFGA behavior, which will be used to verify RSFGA compatibility at every phase

**Why First**: We claim "100% API compatibility" but OpenFGA doesn't provide a compatibility test suite. We must build our own validation framework before implementing RSFGA, so we have a ground truth to test against.

**Validation at Each Phase End**: Run this test suite against RSFGA to ensure no breaking changes

---

### Milestone 0.1: Test Harness Foundation (Week 1)

**Branch**: `feature/milestone-0.1-test-harness`

**Objective**: Set up infrastructure to run tests against OpenFGA and capture expected behavior

#### Section 1: OpenFGA Test Environment

- [x] Test: Can start OpenFGA via Docker Compose
- [x] Test: Can connect to OpenFGA HTTP API (health check)
- [x] Test: Can connect to OpenFGA gRPC API
- [x] Test: Can create a store via API
- [x] Test: Can clean up test stores after tests
- [x] Test: Test environment teardown is idempotent

#### Section 2: Test Data Generators

- [x] Test: Can generate valid User identifiers
- [x] Test: Can generate valid Object identifiers (type:id format)
- [ ] Test: Can generate valid Relation names
- [ ] Test: Can generate valid Tuples
- [ ] Test: Can generate authorization models with direct relations
- [ ] Test: Can generate authorization models with computed relations
- [ ] Test: Can generate authorization models with union relations
- [ ] Test: Can generate authorization models with intersection relations
- [ ] Test: Can generate models with deeply nested relations (10+ levels)

#### Section 3: Response Capture Framework

- [ ] Test: Can capture HTTP request/response pairs
- [ ] Test: Can capture gRPC request/response pairs
- [ ] Test: Can serialize captured data to JSON
- [ ] Test: Can load captured test cases from disk
- [ ] Test: Can compare two responses for equality
- [ ] Test: Can detect breaking changes in response format

**Validation Criteria**:
- [ ] OpenFGA running in Docker
- [ ] Test data generators produce valid inputs
- [ ] Can capture and replay requests
- [ ] Response comparison works correctly

**Deliverables**:
- `tests/compatibility/` directory with test harness
- Docker Compose file for OpenFGA
- Test data generators
- Response capture/comparison utilities

---

### Milestone 0.2: Store & Model API Tests (Week 2)

**Branch**: `feature/milestone-0.2-store-model-tests`

**Objective**: Validate Store and Authorization Model API behavior

#### Section 1: Store Management

- [ ] Test: POST /stores creates store with generated ID
- [ ] Test: POST /stores returns created store in response
- [ ] Test: GET /stores/{store_id} retrieves store by ID
- [ ] Test: GET /stores/{store_id} returns 404 for non-existent store
- [ ] Test: DELETE /stores/{store_id} deletes store
- [ ] Test: DELETE /stores/{store_id} returns 404 for non-existent store
- [ ] Test: LIST /stores returns paginated results
- [ ] Test: LIST /stores respects page_size parameter
- [ ] Test: LIST /stores continuation_token works correctly

#### Section 2: Authorization Model Write

- [ ] Test: POST /stores/{store_id}/authorization-models creates model
- [ ] Test: Model creation returns generated model_id
- [ ] Test: Can create model with only direct relations
- [ ] Test: Can create model with computed relations (union, intersection)
- [ ] Test: Can create model with this keyword
- [ ] Test: Can create model with wildcards
- [ ] Test: Invalid model syntax returns 400 error
- [ ] Test: Duplicate type definitions return error
- [ ] Test: Undefined relation references return error

#### Section 3: Authorization Model Read

- [ ] Test: GET /stores/{store_id}/authorization-models/{model_id} retrieves model
- [ ] Test: GET returns 404 for non-existent model
- [ ] Test: GET /stores/{store_id}/authorization-models lists all models
- [ ] Test: Latest model can be retrieved
- [ ] Test: Model response includes schema_version
- [ ] Test: Model response includes type_definitions

**Validation Criteria**:
- [ ] All store CRUD operations captured
- [ ] All authorization model operations captured
- [ ] Error conditions documented
- [ ] Edge cases identified

**Deliverables**:
- 30+ test cases for Store API
- 15+ test cases for Authorization Model API
- Captured expected responses from OpenFGA

---

### Milestone 0.3: Tuple API Tests (Week 3)

**Branch**: `feature/milestone-0.3-tuple-tests`

**Objective**: Validate Tuple write and read operations

#### Section 1: Tuple Write Operations

- [ ] Test: POST /stores/{store_id}/write writes single tuple
- [ ] Test: Write returns empty response on success
- [ ] Test: Can write multiple tuples in single request
- [ ] Test: Writes are idempotent (writing same tuple twice succeeds)
- [ ] Test: Can delete tuple using Write API (deletes field)
- [ ] Test: Writing tuple without store returns 404
- [ ] Test: Writing tuple with invalid format returns 400
- [ ] Test: Writing tuple with non-existent type returns error
- [ ] Test: Writing tuple with non-existent relation returns error
- [ ] Test: Conditional writes work (condition field)

#### Section 2: Tuple Read Operations

- [ ] Test: POST /stores/{store_id}/read reads tuples by filter
- [ ] Test: Read with user filter returns matching tuples
- [ ] Test: Read with relation filter returns matching tuples
- [ ] Test: Read with object filter returns matching tuples
- [ ] Test: Read with multiple filters combines them (AND logic)
- [ ] Test: Read with empty filter returns all tuples
- [ ] Test: Read respects page_size parameter
- [ ] Test: Read continuation_token enables pagination
- [ ] Test: Read returns empty array when no matches

#### Section 3: Tuple Edge Cases

- [ ] Test: Writing tuple with user wildcard (user:*)
- [ ] Test: Reading tuple with userset relation
- [ ] Test: Tuple with contextual tuples in condition
- [ ] Test: Very long user/object IDs (1000+ characters)
- [ ] Test: Special characters in user/object IDs
- [ ] Test: Unicode characters in identifiers

**Validation Criteria**:
- [ ] All tuple write operations captured
- [ ] All tuple read operations captured
- [ ] Pagination behavior documented
- [ ] Error conditions captured

**Deliverables**:
- 25+ test cases for Tuple API
- Edge case documentation
- Expected error responses

---

### Milestone 0.4: Check API Tests (Week 4)

**Branch**: `feature/milestone-0.4-check-tests`

**Objective**: Validate Check and Batch Check operations (the core of authorization)

#### Section 1: Basic Check Operations

- [ ] Test: POST /stores/{store_id}/check performs direct relation check
- [ ] Test: Check returns {allowed: true} when tuple exists
- [ ] Test: Check returns {allowed: false} when tuple doesn't exist
- [ ] Test: Check follows computed relations (union)
- [ ] Test: Check follows computed relations (intersection)
- [ ] Test: Check follows computed relations (exclusion)
- [ ] Test: Check resolves this keyword correctly
- [ ] Test: Check with contextual_tuples considers them
- [ ] Test: Check with non-existent store returns 404
- [ ] Test: Check with invalid tuple format returns 400

#### Section 2: Complex Relation Resolution

- [ ] Test: Check resolves multi-hop relations (parent's viewer is child's viewer)
- [ ] Test: Check resolves deeply nested relations (5+ hops)
- [ ] Test: Check handles union of multiple relations
- [ ] Test: Check handles intersection correctly (must satisfy all)
- [ ] Test: Check handles but-not exclusion
- [ ] Test: Check with cycle detection doesn't infinite loop
- [ ] Test: Check with wildcards (user:*)
- [ ] Test: Check respects authorization model version

#### Section 3: Batch Check Operations

- [ ] Test: POST /stores/{store_id}/batch-check checks multiple tuples
- [ ] Test: Batch check returns array of results in same order
- [ ] Test: Batch check handles mix of allowed/denied
- [ ] Test: Batch check with empty array returns empty results
- [ ] Test: Batch check with 100+ items
- [ ] Test: Batch check deduplicates identical requests (verify via performance)

#### Section 4: Check Performance & Consistency

- [ ] Test: Measure check latency for direct relations (baseline)
- [ ] Test: Measure check latency for computed relations (union)
- [ ] Test: Measure check latency for deep nested relations
- [ ] Test: Check immediately after write reflects new tuple (consistency)
- [ ] Test: Check after delete reflects removed tuple

**Validation Criteria**:
- [ ] All check operations captured
- [ ] Complex relation types tested
- [ ] Batch check behavior documented
- [ ] Performance baselines established (for comparison in Phase 1)

**Deliverables**:
- 30+ test cases for Check API
- Batch check test cases
- Performance baseline measurements
- Consistency behavior documentation

---

### Milestone 0.5: Expand & ListObjects API Tests (Week 5)

**Branch**: `feature/milestone-0.5-expand-listobjects-tests`

**Objective**: Validate Expand (relation tree) and ListObjects operations

#### Section 1: Expand API

- [ ] Test: POST /stores/{store_id}/expand returns relation tree
- [ ] Test: Expand shows direct relations as leaf nodes
- [ ] Test: Expand shows computed relations as tree nodes
- [ ] Test: Expand includes union branches
- [ ] Test: Expand includes intersection branches
- [ ] Test: Expand respects max_depth parameter
- [ ] Test: Expand with depth=1 returns only immediate relations
- [ ] Test: Expand with non-existent object returns empty tree
- [ ] Test: Expand tree structure is deterministic (same result on repeat)

#### Section 2: ListObjects API

- [ ] Test: POST /stores/{store_id}/list-objects returns objects user can access
- [ ] Test: ListObjects with direct relations returns correct objects
- [ ] Test: ListObjects with computed relations returns correct objects
- [ ] Test: ListObjects with type filter limits results
- [ ] Test: ListObjects respects pagination (page_size)
- [ ] Test: ListObjects with continuation_token works
- [ ] Test: ListObjects with no accessible objects returns empty array
- [ ] Test: ListObjects with contextual_tuples considers them

#### Section 3: Edge Cases

- [ ] Test: Expand with very deep relation tree (20+ levels)
- [ ] Test: Expand with circular relation definition (cycle detection)
- [ ] Test: ListObjects with large result set (1000+ objects)
- [ ] Test: ListObjects with wildcards

**Validation Criteria**:
- [ ] Expand tree format documented
- [ ] ListObjects pagination behavior captured
- [ ] Edge cases identified

**Deliverables**:
- 20+ test cases for Expand/ListObjects APIs
- Relation tree format documentation
- Expected behavior for edge cases

---

### Milestone 0.6: Error Handling & Edge Cases (Week 6)

**Branch**: `feature/milestone-0.6-error-edge-cases`

**Objective**: Document all error conditions and edge cases systematically

#### Section 1: Error Response Format

- [ ] Test: 400 Bad Request format (error code, message, details)
- [ ] Test: 404 Not Found format
- [ ] Test: 500 Internal Server Error format
- [ ] Test: Error response is consistent across all endpoints
- [ ] Test: Error codes match OpenFGA error code enum
- [ ] Test: Validation errors include field-level details

#### Section 2: Rate Limiting & Throttling

- [ ] Test: API responds with 429 when rate limited (if applicable)
- [ ] Test: Rate limit headers present (if applicable)
- [ ] Test: Retry-After header on rate limit (if applicable)

#### Section 3: Consistency & Concurrency

- [ ] Test: Concurrent writes to same tuple
- [ ] Test: Read-after-write consistency
- [ ] Test: Check during model update
- [ ] Test: Check with stale model_id (if supported)

#### Section 4: Limits & Boundaries

- [ ] Test: Maximum tuple size (user + relation + object length)
- [ ] Test: Maximum number of tuples in single write
- [ ] Test: Maximum number of checks in batch
- [ ] Test: Maximum authorization model size
- [ ] Test: Maximum relation depth
- [ ] Test: Request timeout behavior

**Validation Criteria**:
- [ ] All error response formats documented
- [ ] Consistency guarantees understood
- [ ] Limits documented

**Deliverables**:
- Complete error response catalog
- Documented consistency model
- Limit boundary documentation
- Edge case test suite

---

### Milestone 0.7: gRPC API Compatibility (Week 7)

**Branch**: `feature/milestone-0.7-grpc-tests`

**Objective**: Validate gRPC API matches REST API behavior

#### Section 1: gRPC Service Setup

- [ ] Test: Can connect to OpenFGA gRPC service
- [ ] Test: gRPC reflection works (can discover services)
- [ ] Test: Can call Store service methods
- [ ] Test: Can call Authorization Model service methods
- [ ] Test: Can call Tuple service methods
- [ ] Test: Can call Check service methods

#### Section 2: gRPC vs REST Parity

- [ ] Test: gRPC Store.Create matches REST POST /stores
- [ ] Test: gRPC Store.Get matches REST GET /stores/{id}
- [ ] Test: gRPC Store.Delete matches REST DELETE /stores/{id}
- [ ] Test: gRPC Check matches REST Check
- [ ] Test: gRPC Batch Check matches REST Batch Check
- [ ] Test: gRPC Write matches REST Write
- [ ] Test: gRPC Read matches REST Read
- [ ] Test: gRPC Expand matches REST Expand
- [ ] Test: gRPC ListObjects matches REST ListObjects

#### Section 3: gRPC-Specific Features

- [ ] Test: gRPC streaming (if supported)
- [ ] Test: gRPC metadata/headers
- [ ] Test: gRPC error codes match HTTP status codes
- [ ] Test: gRPC error details format

**Validation Criteria**:
- [ ] All gRPC services tested
- [ ] REST/gRPC parity verified
- [ ] gRPC-specific features documented

**Deliverables**:
- gRPC test suite (mirroring REST tests)
- Protobuf message examples
- gRPC/REST compatibility matrix

---

## Phase 0 Summary

**Total Duration**: 7 weeks

**Test Count**: ~150 compatibility tests

**Deliverables**:
1. ✅ OpenFGA test harness (Docker-based)
2. ✅ Complete REST API test suite with expected responses
3. ✅ Complete gRPC API test suite
4. ✅ Test data generators
5. ✅ Performance baselines for comparison
6. ✅ Error response catalog
7. ✅ Compatibility validation framework

**Success Criteria**:
- [ ] Can run test suite against OpenFGA (100% pass)
- [ ] All API endpoints covered
- [ ] All relation types tested
- [ ] Edge cases documented
- [ ] Performance baselines established

**Use at Each Phase End**:
After completing Phase 1, 2, or 3, run this test suite against RSFGA to ensure:
- All tests pass (100% compatibility)
- Performance meets or exceeds baselines
- No behavioral regressions

---

## Phase 1: MVP - OpenFGA Compatible Core

**Goal**: 100% API compatible drop-in replacement with 2x performance

---

### Milestone 1.1: Project Foundation (Weeks 1-2)

**Branch**: `feature/milestone-1.1-project-foundation`

**Objective**: Set up workspace, CI/CD, and verify basic compilation

#### Tests

- [ ] Test: Workspace compiles without errors
- [ ] Test: Workspace structure has all required crates
- [ ] Test: CI pipeline runs successfully on push
- [ ] Test: Cargo clippy passes with zero warnings
- [ ] Test: Cargo fmt check passes
- [ ] Test: Cargo audit passes (no vulnerabilities)
- [ ] Test: All crates have proper lib.rs with module exports
- [ ] Test: Development dependencies (proptest, mockall) are available
- [ ] Test: Benchmark harness compiles (criterion)
- [ ] Test: Can import core dependencies (tokio, serde, etc.)

**Validation Criteria**:
- [ ] All 4 crates compile successfully
- [ ] CI pipeline green
- [ ] Zero clippy warnings
- [ ] Code formatted correctly
- [ ] No security vulnerabilities

**Deliverables**:
- Cargo workspace with 4 crates: rsfga-api, rsfga-server, rsfga-domain, rsfga-storage
- GitHub Actions CI pipeline
- Pre-commit hooks for fmt/clippy
- README.md with quick start

---

### Milestone 1.2: Type System & Model Parser (Weeks 3-4)

**Branch**: `feature/milestone-1.2-type-system`

**Objective**: Parse OpenFGA authorization models and validate them

#### Section 1: Core Type Definitions

- [ ] Test: Can create a User type with validation
- [ ] Test: User type rejects empty string
- [ ] Test: User type rejects invalid format (missing "user:" prefix)
- [ ] Test: Can create Object type with type:id format
- [ ] Test: Can create Relation type
- [ ] Test: Can create Tuple struct with user, relation, object
- [ ] Test: Tuple validates all fields are non-empty
- [ ] Test: Can create Store with unique ID
- [ ] Test: Can create AuthorizationModel with schema version

#### Section 2: DSL Parser

- [ ] Test: Parser recognizes "type" keyword
- [ ] Test: Parser parses simple type definition
- [ ] Test: Parser parses type with single relation
- [ ] Test: Parser parses type with multiple relations
- [ ] Test: Parser handles "define" keyword for relations
- [ ] Test: Parser parses direct relation assignment
- [ ] Test: Parser parses "this" keyword
- [ ] Test: Parser parses union relation (relation1 or relation2)
- [ ] Test: Parser parses intersection relation (relation1 and relation2)
- [ ] Test: Parser parses exclusion relation (relation1 but not relation2)
- [ ] Test: Parser parses computed relation (relation from parent)
- [ ] Test: Parser rejects invalid syntax with clear error
- [ ] Test: Parser handles whitespace correctly
- [ ] Test: Parser handles comments

**Example DSL to parse**:
```
type user

type document
  relations
    define owner: [user]
    define editor: [user] or owner
    define viewer: [user] or editor
```

#### Section 3: Model Validation

- [ ] Test: Validator accepts valid model
- [ ] Test: Validator rejects cyclic relation definitions
- [ ] Test: Validator rejects undefined relation references
- [ ] Test: Validator rejects undefined type references
- [ ] Test: Validator checks relation type constraints
- [ ] Test: Validator ensures all relations have definitions

**Validation Criteria**:
- [ ] >90% test coverage on parser
- [ ] Parser handles all OpenFGA relation types
- [ ] Validation catches all invalid models
- [ ] Property-based tests for parser robustness

**Deliverables**:
- Complete type system (rsfga-domain/src/model/)
- DSL parser (rsfga-domain/src/parser/)
- Model validator (rsfga-domain/src/validation/)
- 50+ unit tests

---

### Milestone 1.3: Storage Layer (Weeks 5-6)

**Branch**: `feature/milestone-1.3-storage-layer`

**Objective**: Abstract storage interface with PostgreSQL and in-memory implementations

#### Section 1: Storage Trait

- [ ] Test: DataStore trait compiles
- [ ] Test: DataStore trait is Send + Sync
- [ ] Test: Can define write_tuple method signature
- [ ] Test: Can define read_tuples method signature
- [ ] Test: Can define delete_tuple method signature
- [ ] Test: Can define transaction support methods

#### Section 2: In-Memory Storage

- [ ] Test: InMemoryStore can be created
- [ ] Test: Can write a single tuple
- [ ] Test: Can read back written tuple
- [ ] Test: Read returns empty vec when no tuples match
- [ ] Test: Can write multiple tuples
- [ ] Test: Can filter tuples by user
- [ ] Test: Can filter tuples by object
- [ ] Test: Can filter tuples by relation
- [ ] Test: Can delete tuple
- [ ] Test: Delete is idempotent (deleting non-existent tuple is ok)
- [ ] Test: Concurrent writes don't lose data
- [ ] Test: Concurrent reads while writing return consistent data

#### Section 3: PostgreSQL Storage

- [ ] Test: Can connect to PostgreSQL
- [ ] Test: Can create tables with migrations
- [ ] Test: Can write tuple to database
- [ ] Test: Can read tuple from database
- [ ] Test: Can delete tuple from database
- [ ] Test: Transactions rollback on error
- [ ] Test: Transactions commit on success
- [ ] Test: Connection pool limits work correctly
- [ ] Test: Database errors are properly wrapped in StorageError
- [ ] Test: Concurrent writes use correct isolation level
- [ ] Test: Indexes are created for common query patterns
- [ ] Test: Can paginate large result sets

**Schema**:
```sql
CREATE TABLE tuples (
    store_id UUID NOT NULL,
    object_type VARCHAR(255) NOT NULL,
    object_id VARCHAR(255) NOT NULL,
    relation VARCHAR(255) NOT NULL,
    user_type VARCHAR(255) NOT NULL,
    user_id VARCHAR(255) NOT NULL,
    user_relation VARCHAR(255),
    created_at TIMESTAMP DEFAULT NOW(),
    PRIMARY KEY (store_id, object_type, object_id, relation, user_type, user_id, COALESCE(user_relation, ''))
);

CREATE INDEX idx_tuples_object ON tuples(store_id, object_type, object_id);
CREATE INDEX idx_tuples_user ON tuples(store_id, user_type, user_id);
```

#### Section 4: Storage Integration Tests

- [ ] Test: Same behavior across InMemory and PostgreSQL stores
- [ ] Test: Can swap storage implementations
- [ ] Test: Large dataset performance (10k+ tuples)
- [ ] Test: Storage survives application restart (PostgreSQL)

**Validation Criteria**:
- [ ] Both implementations pass identical test suite
- [ ] >90% test coverage
- [ ] Integration tests use testcontainers
- [ ] Query performance <5ms p95

**Deliverables**:
- DataStore trait (rsfga-storage/src/traits.rs)
- InMemoryStore implementation
- PostgresStore implementation
- Database migrations
- 40+ unit + integration tests

---

### Milestone 1.4: Graph Resolver (Weeks 7-8)

**Branch**: `feature/milestone-1.4-graph-resolver`

**Objective**: Implement async graph traversal with cycle detection and depth limiting

#### Section 1: Direct Tuple Resolution

- [ ] Test: Check returns true for direct tuple assignment
- [ ] Test: Check returns false when no tuple exists
- [ ] Test: Check handles multiple stores independently
- [ ] Test: Check validates input parameters
- [ ] Test: Check rejects invalid user format
- [ ] Test: Check rejects invalid object format

#### Section 2: Computed Relations

- [ ] Test: Can resolve "this" relation
- [ ] Test: Can resolve relation from parent object
- [ ] Test: Resolves nested parent relationships
- [ ] Test: Handles missing parent gracefully

#### Section 3: Union Relations (A or B)

- [ ] Test: Returns true if ANY union branch is true
- [ ] Test: Returns false if ALL union branches are false
- [ ] Test: Short-circuits on first true branch (performance)
- [ ] Test: Executes union branches in parallel
- [ ] Test: Handles errors in union branches

#### Section 4: Intersection Relations (A and B)

- [ ] Test: Returns true only if ALL intersection branches are true
- [ ] Test: Returns false if ANY intersection branch is false
- [ ] Test: Short-circuits on first false branch
- [ ] Test: Executes intersection branches in parallel

#### Section 5: Exclusion Relations (A but not B)

- [ ] Test: Returns true if A is true and B is false
- [ ] Test: Returns false if A is false
- [ ] Test: Returns false if B is true

#### Section 6: Safety Mechanisms

- [ ] Test: Depth limit prevents stack overflow
- [ ] Test: Depth limit matches OpenFGA (25)
- [ ] Test: Returns DepthLimitExceeded error when limit hit
- [ ] Test: Cycle detection prevents infinite loops
- [ ] Test: Cycle detection doesn't false-positive on valid DAGs
- [ ] Test: Timeout prevents hanging on pathological graphs
- [ ] Test: Timeout is configurable
- [ ] Test: Returns timeout error with context

#### Section 7: Property-Based Tests

- [ ] Test: Property: Resolver never panics on any input
- [ ] Test: Property: Resolver always terminates (no infinite loops)
- [ ] Test: Property: Resolver returns correct result for known graphs
- [ ] Test: Property: Adding a tuple never makes check false→true incorrectly
- [ ] Test: Property: Deleting a tuple never makes check true→false incorrectly

**Validation Criteria**:
- [ ] >95% test coverage (security-critical code)
- [ ] Property-based tests cover all relation types
- [ ] Fuzz testing for edge cases
- [ ] Manual security review completed

**Deliverables**:
- GraphResolver (rsfga-domain/src/resolver/)
- Cycle detection
- Depth limiting
- Parallel traversal
- 60+ unit tests
- 10+ property-based tests

---

### Milestone 1.5: Check Cache (Week 9)

**Branch**: `feature/milestone-1.5-check-cache`

**Objective**: Lock-free caching with TTL and async invalidation

#### Section 1: Cache Structure

- [ ] Test: Can create cache with DashMap
- [ ] Test: Can insert check result into cache
- [ ] Test: Can retrieve cached check result
- [ ] Test: Cache miss returns None
- [ ] Test: Cache key includes store, user, relation, object
- [ ] Test: Different stores have separate cache entries

#### Section 2: TTL and Eviction

- [ ] Test: Cached entry expires after TTL
- [ ] Test: TTL is configurable
- [ ] Test: Can manually invalidate cache entry
- [ ] Test: Can invalidate all entries for a store
- [ ] Test: Can invalidate entries matching pattern
- [ ] Test: Eviction doesn't block reads

#### Section 3: Cache Invalidation

- [ ] Test: Writing tuple invalidates affected cache entries
- [ ] Test: Deleting tuple invalidates affected cache entries
- [ ] Test: Invalidation is async (doesn't block write response)
- [ ] Test: Invalidation completes eventually
- [ ] Test: Invalidation handles errors gracefully
- [ ] Test: Can measure staleness window (<100ms p99)

#### Section 4: Concurrent Access

- [ ] Test: Concurrent reads don't block each other
- [ ] Test: Concurrent writes don't lose data
- [ ] Test: Read during write returns valid result
- [ ] Test: Cache scales to 1000+ concurrent operations
- [ ] Test: No deadlocks under high contention

**Validation Criteria**:
- [ ] Lock-free reads verified
- [ ] Staleness window <100ms p99
- [ ] >90% test coverage
- [ ] Benchmark shows >5x speedup vs Mutex<HashMap>

**Deliverables**:
- CheckCache (rsfga-domain/src/cache/)
- DashMap + Moka integration
- Async invalidation
- 30+ tests

---

### Milestone 1.6: Batch Check Handler (Week 10)

**Branch**: `feature/milestone-1.6-batch-handler`

**Objective**: Parallel batch checking with three-stage deduplication

#### Section 1: Batch Request Parsing

- [ ] Test: Can parse batch check request
- [ ] Test: Validates each check in batch
- [ ] Test: Rejects empty batch
- [ ] Test: Accepts batch with single check
- [ ] Test: Accepts batch with 100+ checks

#### Section 2: Intra-Batch Deduplication

- [ ] Test: Identifies duplicate checks in batch
- [ ] Test: Executes unique checks only once
- [ ] Test: Maps results back to original positions
- [ ] Test: Preserves request order in response

#### Section 3: Singleflight (Cross-Request Dedup)

- [ ] Test: Concurrent requests for same check share result
- [ ] Test: Singleflight groups expire after completion
- [ ] Test: Errors don't poison singleflight group
- [ ] Test: Singleflight handles timeouts correctly

#### Section 4: Parallel Execution

- [ ] Test: Unique checks execute in parallel
- [ ] Test: Batch processes faster than sequential checks
- [ ] Test: Respects concurrency limits
- [ ] Test: Handles partial failures gracefully

#### Section 5: Performance

- [ ] Test: Batch of 100 identical checks executes ~1 check
- [ ] Test: Batch throughput >500 checks/s (target)
- [ ] Test: Memory usage scales linearly with unique checks

**Validation Criteria**:
- [ ] Batch throughput >500 checks/s
- [ ] Deduplication effectiveness >90% on typical workloads
- [ ] >90% test coverage

**Deliverables**:
- BatchCheckHandler (rsfga-server/src/handlers/batch.rs)
- Three-stage deduplication
- Singleflight implementation
- 25+ tests

---

### Milestone 1.7: API Layer (Week 11)

**Branch**: `feature/milestone-1.7-api-layer`

**Objective**: HTTP REST and gRPC APIs with OpenFGA compatibility

#### Section 1: Protobuf Definitions

- [ ] Test: Protobuf files compile
- [ ] Test: Generated Rust code is identical to OpenFGA types
- [ ] Test: Can serialize/deserialize Check request
- [ ] Test: Can serialize/deserialize Check response
- [ ] Test: Can serialize/deserialize all request types

#### Section 2: HTTP REST API

- [ ] Test: Server starts on configured port
- [ ] Test: POST /stores/{store_id}/check returns 200
- [ ] Test: Check endpoint validates request body
- [ ] Test: Check endpoint returns correct response format
- [ ] Test: POST /stores/{store_id}/expand returns 200
- [ ] Test: POST /stores/{store_id}/write returns 200
- [ ] Test: POST /stores/{store_id}/read returns 200
- [ ] Test: Invalid JSON returns 400
- [ ] Test: Non-existent store returns 404
- [ ] Test: Server errors return 500 with details

#### Section 3: gRPC API

- [ ] Test: gRPC server starts
- [ ] Test: Check RPC works correctly
- [ ] Test: BatchCheck RPC works correctly
- [ ] Test: Write RPC works correctly
- [ ] Test: Read RPC works correctly
- [ ] Test: gRPC errors map correctly to status codes

#### Section 4: Middleware

- [ ] Test: Request logging works
- [ ] Test: Metrics are collected (request count, duration)
- [ ] Test: Tracing spans are created
- [ ] Test: CORS headers are set correctly
- [ ] Test: Request ID is generated and propagated

#### Section 5: OpenFGA Compatibility

- [ ] Test: All OpenFGA API endpoints present
- [ ] Test: Request/response schemas match exactly
- [ ] Test: Error codes match OpenFGA
- [ ] Test: Run OpenFGA compatibility test suite (100% pass)

**Validation Criteria**:
- [ ] 100% OpenFGA API compatibility
- [ ] OpenFGA test suite passes 100%
- [ ] API documentation generated
- [ ] >90% test coverage

**Deliverables**:
- HTTP REST API (rsfga-api/src/http/)
- gRPC API (rsfga-api/src/grpc/)
- Middleware (auth, metrics, tracing)
- OpenFGA compatibility verified
- 50+ integration tests

---

### Milestone 1.8: Testing & Benchmarking (Week 12)

**Branch**: `feature/milestone-1.8-testing-benchmarking`

**Objective**: Comprehensive testing and performance validation

#### Section 1: Test Coverage

- [ ] Test: Overall coverage >90%
- [ ] Test: Domain layer coverage >95%
- [ ] Test: Graph resolver coverage >95%
- [ ] Test: All public APIs have tests
- [ ] Test: All error paths have tests

#### Section 2: Integration Tests

- [ ] Test: End-to-end authorization flow works
- [ ] Test: Multiple concurrent clients work correctly
- [ ] Test: Database connection loss is handled
- [ ] Test: Server restart preserves data (PostgreSQL)
- [ ] Test: Large authorization models work (1000+ types)
- [ ] Test: Deep hierarchies work (depth 20+)

#### Section 3: Performance Benchmarks

- [ ] Test: Benchmark check operation throughput
- [ ] Test: Benchmark batch check throughput
- [ ] Test: Benchmark write operation throughput
- [ ] Test: Benchmark cache hit ratio
- [ ] Test: Benchmark memory usage
- [ ] Test: Benchmark startup time
- [ ] Test: Compare against OpenFGA baseline

**Baseline Comparison**:
```
OpenFGA Baseline:
- Check: 483 req/s
- Batch: 23 checks/s
- Write: 59 req/s
- Check p95: 22ms

RSFGA Target (60% confidence):
- Check: 1000+ req/s (2x improvement)
- Batch: 500+ checks/s (20x improvement)
- Write: 150+ req/s (2.5x improvement)
- Check p95: <20ms
```

#### Section 4: Stress Testing

- [ ] Test: Server handles 1000 concurrent connections
- [ ] Test: No memory leaks under sustained load
- [ ] Test: Graceful degradation under overload
- [ ] Test: Recovery after database failure

#### Section 5: Security Testing

- [ ] Test: SQL injection attempts fail
- [ ] Test: Input validation prevents XSS
- [ ] Test: Rate limiting works
- [ ] Test: Authentication required for sensitive endpoints
- [ ] Test: Authorization model cannot be bypassed

**Validation Criteria**:
- [ ] Check throughput >1000 req/s (validates ADR-001, ADR-002)
- [ ] Batch throughput >500 checks/s (validates ADR-005)
- [ ] Cache staleness <100ms p99 (validates ADR-007)
- [ ] No critical security vulnerabilities
- [ ] 100% OpenFGA compatibility

**Deliverables**:
- Comprehensive test suite (100+ tests)
- Benchmark suite (criterion)
- Load testing scripts (k6)
- Performance report
- Security audit report
- Updated ADRs with validation results

---

### Milestone 1.9: Production Readiness (Week 13)

**Branch**: `feature/milestone-1.9-production-ready`

**Objective**: Observability, documentation, and deployment

#### Section 1: Observability

- [ ] Test: Metrics endpoint exposes Prometheus metrics
- [ ] Test: Tracing spans are exported to Jaeger
- [ ] Test: Structured logs are JSON formatted
- [ ] Test: Health check endpoint returns 200
- [ ] Test: Readiness check validates dependencies

**Key Metrics**:
- Request rate (requests/sec)
- Request duration (p50, p95, p99)
- Error rate
- Cache hit ratio
- Database connection pool usage
- Active connections

#### Section 2: Configuration

- [ ] Test: Can load config from YAML file
- [ ] Test: Can override config with env vars
- [ ] Test: Config validation catches errors
- [ ] Test: Invalid config returns clear error
- [ ] Test: Config hot-reload works (if supported)

#### Section 3: Documentation

- [ ] Test: API documentation is generated
- [ ] Test: README has quick start guide
- [ ] Test: Architecture docs are up to date
- [ ] Test: All ADRs have validation results
- [ ] Test: Migration guide from OpenFGA exists

#### Section 4: Deployment

- [ ] Test: Docker image builds successfully
- [ ] Test: Docker image runs correctly
- [ ] Test: Kubernetes manifests are valid
- [ ] Test: Helm chart installs successfully
- [ ] Test: Database migrations run automatically

**Validation Criteria**:
- [ ] Complete observability stack
- [ ] Production-grade configuration
- [ ] Comprehensive documentation
- [ ] One-command deployment

**Deliverables**:
- Prometheus metrics
- OpenTelemetry tracing
- Structured logging
- Configuration management
- Docker image
- Kubernetes/Helm charts
- Complete documentation

---

## Phase 1 Summary

**When Phase 1 is complete, RSFGA will have:**

✅ 100% OpenFGA API compatibility
✅ PostgreSQL and in-memory storage backends
✅ Async graph resolution with parallel traversal
✅ Lock-free caching with TTL
✅ Batch checking with deduplication
✅ REST and gRPC APIs
✅ >90% test coverage
✅ 2x performance improvement (validated)
✅ Production-ready observability
✅ Complete documentation

**Performance Validation Status**: ⏸️ Awaiting M1.8 benchmarking

**OpenFGA Compatibility**: ✅ Verified in M1.7

**Next**: Phase 2 - Precomputation Engine (Optional)

---

## Phase 2: Precomputation Engine (Future)

**Goal**: Sub-millisecond check latency through on-write precomputation

**Status**: 📋 Proposed (awaits Phase 1 completion)

**Key Features**:
- Valkey/Redis for precomputed results
- Change impact analysis
- Selective precomputation (hot paths)
- TTL-based eviction

**Target Performance**:
- Check: <1ms p99 (vs ~20ms graph traversal)
- Write: 100-200ms p95 (acceptable trade-off)

See [ARCHITECTURE_DECISIONS.md - ADR-013](docs/design/ARCHITECTURE_DECISIONS.md#adr-013-precomputation-architecture-phase-2) for details.

---

## Phase 3: Distributed Edge (Future)

**Goal**: Global <10ms check latency through edge deployment

**Status**: 📋 Proposed (awaits Phase 2 completion)

**Key Features**:
- NATS JetStream for sync
- Product-based data partitioning
- Edge, Regional, Central topology
- Automatic fallback

**Target Performance**:
- Edge: <10ms p95 globally
- Cluster: 100K+ req/s

See [EDGE_ARCHITECTURE_NATS.md](docs/design/EDGE_ARCHITECTURE_NATS.md) for details.

---

## Risk Mitigation Tracking

As we implement, track risk mitigation:

**Critical Risks**:
- [ ] R-001: API Compatibility verified in M1.7
- [ ] R-002: Performance claims validated in M1.8
- [ ] R-003: Security review completed in M1.4

**High Risks**:
- [ ] R-004: Database performance monitored in M1.3
- [ ] R-005: Cache consistency measured in M1.5
- [ ] R-007: Dependencies audited in M1.1

See [RISKS.md](docs/design/RISKS.md) for complete list.

---

## Validation Gates

Before marking Phase 1 complete:

**Functional**:
- [ ] All Phase 1 tests passing (500+ tests)
- [ ] OpenFGA compatibility suite 100% pass
- [ ] Zero critical bugs

**Performance**:
- [ ] Check: >1000 req/s
- [ ] Batch: >500 checks/s
- [ ] Write: >150 req/s
- [ ] All within error budget

**Quality**:
- [ ] >90% test coverage
- [ ] Zero clippy warnings
- [ ] Zero security vulnerabilities
- [ ] All ADRs validated

**Production**:
- [ ] Complete documentation
- [ ] Deployment automation
- [ ] Observability stack
- [ ] Runbook for operations

---

## Current Status

**Phase**: Architecture & Design ✅ Complete
**Next Milestone**: 1.1 - Project Foundation
**Status**: ⏸️ Awaiting user approval to begin implementation

**Ready to start? Say "go" and let's begin with Milestone 1.1!**
