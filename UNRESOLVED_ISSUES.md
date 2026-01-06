# Unresolved Issues from Closed PRs

This document tracks unresolved review comments from closed PRs that should be converted to GitHub issues.

Generated: 2026-01-06

---

## Critical Priority

### 1. [CRITICAL] Parser operator precedence issue: or/and not handled correctly
- **Source**: PR #13 (gemini-code-assist, coderabbitai)
- **File**: `crates/rsfga-domain/src/model/parser.rs`
- **Description**: The current implementation of `parse_userset` does not correctly handle operator precedence between `or` and `and`. It attempts to parse all `or` operands first, and if none are found, it then attempts to parse `and` operands.
- **Impact**: Authorization decisions could be incorrect when using complex expressions with mixed `or` and `and` operators. This could grant unauthorized access or deny legitimate access.
- **Labels**: `bug`, `critical`, `parser`

### 2. [CRITICAL] Incorrect operator semantics when combining continuations
- **Source**: PR #13 (coderabbitai)
- **File**: `crates/rsfga-domain/src/model/parser.rs` (lines 339-342, 349-352)
- **Description**: Always combines continuations using `Userset::Union`, even when `parse_userset_continuation` returns `and` operands. This violates operator semantics.
- **Impact**: Same as above - could result in incorrect authorization decisions.
- **Labels**: `bug`, `critical`, `parser`

---

## High Priority

### 3. [HIGH] Missing type constraint validation in model validator
- **Source**: PR #13 (gemini-code-assist)
- **File**: `crates/rsfga-domain/src/validation/mod.rs`
- **Description**: When a relation is defined with a type constraint like `define owner: [user]`, the validator should verify that the type `user` actually exists in the model. Currently this check is missing.
- **Impact**: Invalid authorization models could be created without proper validation.
- **Labels**: `bug`, `validation`

### 4. [HIGH] Duplicate `Store` type with different timestamp representations
- **Source**: PR #13 (gemini-code-assist, coderabbitai)
- **Files**:
  - `crates/rsfga-domain/src/model/types.rs` (uses `Option<String>`)
  - `crates/rsfga-storage/src/traits.rs` (uses `chrono::DateTime<Utc>`)
- **Description**: Two different `Store` structs exist with different timestamp field types, which will cause confusion and mapping issues.
- **Impact**: Type confusion, potential bugs when mapping between layers.
- **Labels**: `bug`, `architecture`

### 5. [HIGH] Tuple comparison omits `user_relation` field
- **Source**: PR #13 (coderabbitai, augmentcode)
- **File**: `crates/rsfga-storage/src/memory.rs` (lines 89-95, 101-107)
- **Description**: Both delete matching and duplicate checking logic compare all fields except `user_relation`. Two tuples that differ only by `user_relation` (e.g., `group:eng#member` vs `group:eng#admin`) would be incorrectly treated as duplicates.
- **Impact**: Data integrity issues - incorrect deduplication and deletion.
- **Labels**: `bug`, `storage`

### 6. [HIGH] reqwest::Client created per call instead of being reused
- **Source**: PR #10 (gemini-code-assist)
- **File**: `crates/compatibility-tests/tests/common/mod.rs`
- **Description**: Creating a new `reqwest::Client` for each function call is inefficient. The `reqwest::Client` holds a connection pool internally and is meant to be created once and reused.
- **Impact**: Performance degradation, resource waste.
- **Labels**: `performance`, `tests`

### 7. [HIGH] Error handling masks failures in `grpc_call`
- **Source**: PR #12 (gemini-code-assist)
- **File**: `crates/compatibility-tests/tests/common/mod.rs`
- **Description**: If `grpcurl` exits with a non-zero status code (indicating an error) but prints a JSON body to stdout, this function returns that JSON as success instead of failing.
- **Impact**: Test failures could be masked, leading to false positives.
- **Labels**: `bug`, `tests`

### 8. [HIGH] Incomplete test - test_grpc_batch_check_matches_rest
- **Source**: PR #12 (gemini-code-assist)
- **File**: `crates/compatibility-tests/tests/test_section_22_grpc_parity.rs`
- **Description**: The comment on line 413 mentions checking for both an "allowed" (`check-1`) and a "denied" (`check-2`) case, but the test only includes assertions for `check-1`.
- **Impact**: Test coverage gap - denied case not being tested.
- **Labels**: `tests`, `incomplete`

---

## Medium Priority

### 9. [MEDIUM] `StoredTuple` should derive `Hash` for efficiency
- **Source**: PR #13 (gemini-code-assist)
- **File**: `crates/rsfga-storage/src/traits.rs`
- **Description**: The `StoredTuple` struct should derive `Hash` to allow storage in `HashSet`, which is more efficient for checking existence and performing set operations than a `Vec`.
- **Labels**: `enhancement`, `performance`

### 10. [MEDIUM] `MemoryDataStore` has O(N) complexity issues
- **Source**: PR #13 (gemini-code-assist)
- **File**: `crates/rsfga-storage/src/memory.rs`
- **Description**: Uses `Vec<StoredTuple>` to store tuples, resulting in O(N) complexity for write and delete operations due to linear scans.
- **Labels**: `performance`, `storage`

### 11. [MEDIUM] Parser drops type constraint information
- **Source**: PR #13 (augmentcode)
- **File**: `crates/rsfga-domain/src/model/parser.rs`
- **Description**: `parse_relation_definition` parses a `type_constraint`, but that information is not preserved in `RelationDefinition`, so type restriction data is effectively dropped.
- **Labels**: `bug`, `parser`

### 12. [MEDIUM] `ValidationError` has unused variants
- **Source**: PR #13 (gemini-code-assist)
- **File**: `crates/rsfga-domain/src/validation/mod.rs`
- **Description**: The `ValidationError` enum includes `UndefinedType` and `MissingRelationDefinition` variants that are never constructed anywhere in the module.
- **Labels**: `code-quality`

### 13. [MEDIUM] `create_store` silently overwrites existing stores
- **Source**: PR #13 (augmentcode)
- **File**: `crates/rsfga-storage/src/memory.rs`
- **Description**: `create_store` unconditionally inserts into `stores`/`tuples`; if the same `id` is reused this will overwrite the existing store and reset its tuple list.
- **Labels**: `bug`, `storage`

### 14. [MEDIUM] Code duplication for gRPC helpers across test files
- **Source**: PR #12 (gemini-code-assist)
- **Files**:
  - `crates/compatibility-tests/tests/test_section_21_grpc_setup.rs`
  - `crates/compatibility-tests/tests/test_section_22_grpc_parity.rs`
  - `crates/compatibility-tests/tests/test_section_23_grpc_features.rs`
- **Description**: Helper functions like `get_grpc_url`, `grpcurl_with_data` are duplicated across multiple test files.
- **Labels**: `code-quality`, `tests`

### 15. [MEDIUM] `get_grpc_url` is brittle
- **Source**: PR #12 (gemini-code-assist)
- **File**: `crates/compatibility-tests/tests/common/mod.rs`
- **Description**: Relies on string replacement of hardcoded port `:18080` and only handles `http://` scheme.
- **Labels**: `bug`, `tests`

### 16. [MEDIUM] `rest_call` doesn't check status codes
- **Source**: PR #12 (augmentcode)
- **File**: `crates/compatibility-tests/tests/test_section_22_grpc_parity.rs`
- **Description**: Returns parsed JSON without checking `response.status()`, which can let a failing REST request look like success.
- **Labels**: `bug`, `tests`

### 17. [MEDIUM] URL encoding issue with `#` character in tests
- **Source**: PR #12 (augmentcode)
- **File**: `crates/compatibility-tests/tests/test_section_23_grpc_features.rs`
- **Description**: URL includes `#` in the path (`invalid-id!@#`), but `#` is treated as a fragment delimiter and won't be sent to the server.
- **Labels**: `bug`, `tests`

### 18. [MEDIUM] Rate limiting test timing assertion is flaky
- **Source**: PR #11 (augmentcode)
- **File**: `crates/compatibility-tests/tests/test_section_18_rate_limiting.rs`
- **Description**: The wall-clock assertion (`duration.as_secs() < 10`) can be flaky on slower CI/loaded runners.
- **Labels**: `tests`, `flaky`

### 19. [MEDIUM] Error message checking is brittle
- **Source**: PR #11 (augmentcode)
- **File**: `crates/compatibility-tests/tests/test_section_17_error_format.rs`
- **Description**: `error.message.contains("StoreId")` is case/wording-dependent and may break across OpenFGA versions.
- **Labels**: `tests`

### 20. [MEDIUM] HTTP 400 handling could mask other errors
- **Source**: PR #11 (augmentcode)
- **File**: `crates/compatibility-tests/tests/test_section_19_consistency.rs`
- **Description**: In concurrent-write analysis, any HTTP 400 is treated as "duplicate tuple" outcome; that could mask other unexpected 400s.
- **Labels**: `tests`

### 21. [MEDIUM] `unwrap_or(false)` could let API contract regressions pass
- **Source**: PR #10 (augmentcode)
- **File**: `crates/compatibility-tests/tests/common/mod.rs`
- **Description**: A malformed `/check` response (missing/non-boolean `allowed`) gets treated as `false`, which can let "expected false" tests pass even if the API contract regresses.
- **Labels**: `tests`

### 22. [MEDIUM] Module docs don't match implementation
- **Source**: PR #13 (augmentcode)
- **File**: `crates/rsfga-domain/src/validation/mod.rs`
- **Description**: The module docs list "All referenced types exist" / "Type constraints are satisfied", but the current validator only checks relation references + cycles.
- **Labels**: `documentation`

### 23. [MEDIUM] Race condition in store creation
- **Source**: PR #13 (coderabbitai)
- **File**: `crates/rsfga-storage/src/memory.rs`
- **Description**: The `contains_key` check and `insert` are not atomic. Under concurrent access, two threads could both pass the check and one would silently overwrite the other's store.
- **Labels**: `bug`, `concurrency`

---

## Already Fixed

These issues were identified but have been resolved:

- **CI audit job missing Rust toolchain** (PR #15) - Fixed: Added `dtolnay/rust-toolchain@stable` step
- **Status inconsistency in plan.md** (PR #14) - Fixed: Now shows "10/10 tests complete"

---

## How to Create GitHub Issues

To convert these items to GitHub issues:

1. Go to https://github.com/julianshen/rsfga/issues/new
2. Copy the title (without the priority tag)
3. Copy the description and format it as needed
4. Add appropriate labels

Or use GitHub CLI:
```bash
gh issue create --title "Parser operator precedence issue" --body "..." --label "bug,critical"
```
