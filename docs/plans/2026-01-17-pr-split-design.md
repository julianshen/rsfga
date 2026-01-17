# PR Split Design: CockroachDB, Expand API, ListObjects API

**Date**: 2026-01-17
**Status**: Approved
**Original PR**: #181

---

## Overview

Split PR #181 into three independent PRs for cleaner review and independent merging.

## PR Structure

### PR #1: CockroachDB Batch Fix

**Branch**: `feature/cockroachdb-batch-fix`

**Files**:
- `crates/rsfga-storage/src/postgres.rs` - UNNEST helper functions
- `crates/rsfga-storage/tests/cockroachdb_integration.rs` - New testcontainer tests

**Scope**:
- `build_delete_unnest()` and `build_insert_unnest()` helpers
- CockroachDB uses `ROWS FROM (UNNEST(...), UNNEST(...))` syntax
- PostgreSQL uses `UNNEST(..., ...) AS t(...)` syntax

**Tests**:
- `test_batch_write_tuples_cockroachdb`
- `test_batch_delete_tuples_cockroachdb`
- `test_batch_operations_with_conditions_cockroachdb`

### PR #2: Expand API

**Branch**: `feature/expand-api`

**Files**:
- `crates/rsfga-domain/src/resolver/graph_resolver.rs` - expand(), expand_userset()
- `crates/rsfga-domain/src/resolver/types.rs` - ExpandRequest, ExpandResult, ExpandNode
- `crates/rsfga-api/src/http/routes.rs` - POST /stores/{id}/expand
- `crates/rsfga-api/src/grpc/service.rs` - Expand RPC
- `crates/rsfga-api/src/adapters.rs` - expand_node_to_body()

### PR #3: ListObjects API

**Branch**: `feature/list-objects-api`

**Files**:
- `crates/rsfga-domain/src/resolver/graph_resolver.rs` - list_objects() with DoS limit
- `crates/rsfga-domain/src/resolver/types.rs` - ListObjectsRequest, ListObjectsResult
- `crates/rsfga-domain/src/resolver/traits.rs` - list_objects_by_type()
- `crates/rsfga-api/src/validation.rs` - New shared validation module
- `crates/rsfga-api/src/http/routes.rs` - POST /stores/{id}/list-objects
- `crates/rsfga-api/src/grpc/service.rs` - ListObjects RPC
- `crates/rsfga-storage/src/*.rs` - list_objects_by_type() implementations

---

## Validation Module

**Location**: `crates/rsfga-api/src/validation.rs`

**Constants**:
```rust
pub const MAX_JSON_DEPTH: usize = 10;
pub const MAX_CONDITION_CONTEXT_SIZE: usize = 10 * 1024; // 10KB
pub const MAX_LIST_OBJECTS_RESULTS: usize = 1_000;
```

**Functions**:
- `validate_json_depth(value, depth) -> Result<(), String>`
- `validate_context_size(context) -> Result<(), String>`
- `validate_contextual_tuples(tuples) -> Result<(), String>`

---

## DoS Protection for ListObjects

**Limit**: 1,000 objects (matches OpenFGA)

**Strategy**: Limit candidates before permission checks (not results after)

**Rationale**:
- Bounds memory: O(1000) vs O(N)
- Bounds CPU: 1,000 checks max vs unlimited
- Prevents attacker-controlled resource exhaustion

**Response**:
```rust
pub struct ListObjectsResult {
    pub objects: Vec<String>,
    pub truncated: bool,  // true if more objects exist
}
```

---

## CockroachDB Integration Tests

**File**: `crates/rsfga-storage/tests/cockroachdb_integration.rs`

**Container**: `testcontainers_modules::cockroachdb::CockroachDb`

**Tests** (all `#[ignore]` - require Docker):
1. `test_batch_write_tuples_cockroachdb`
2. `test_batch_delete_tuples_cockroachdb`
3. `test_batch_operations_with_conditions_cockroachdb`

**Run**:
```bash
cargo test -p rsfga-storage --test cockroachdb_integration -- --ignored
```

---

## Implementation Order

1. **PR #1 (CockroachDB)** - Smallest, urgent fix
2. **PR #2 (Expand)** - Medium complexity
3. **PR #3 (ListObjects)** - Largest, includes validation refactor

All PRs are independent and can be reviewed/merged in parallel.
