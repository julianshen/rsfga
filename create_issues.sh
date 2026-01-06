#!/bin/bash
# Script to create GitHub issues for unresolved PR review comments
# Generated: 2026-01-06
# Repository: julianshen/rsfga
#
# Usage: ./create_issues.sh
# Prerequisites: gh CLI installed and authenticated (gh auth login)

set -e

REPO="julianshen/rsfga"

echo "Creating GitHub issues for unresolved PR review comments..."
echo "Repository: $REPO"
echo ""

# =============================================================================
# CRITICAL PRIORITY
# =============================================================================

echo "[1/23] Creating: Parser operator precedence issue..."
gh issue create --repo "$REPO" \
  --title "[Critical] Parser operator precedence issue: or/and not handled correctly" \
  --label "bug" \
  --body "$(cat <<'EOF'
## Source
PR #13 review comments (gemini-code-assist, coderabbitai)

## Description
The current implementation of `parse_userset` does not correctly handle operator precedence between `or` and `and`. It attempts to parse all `or` operands first, and if none are found, it then attempts to parse `and` operands.

## Impact
- Authorization decisions could be incorrect when using complex expressions with mixed `or` and `and` operators
- This could grant unauthorized access or deny legitimate access

## Location
`crates/rsfga-domain/src/model/parser.rs`

## Recommended Fix
1. Properly track operator type when parsing continuations
2. Build `Union` or `Intersection` based on the actual operators used
3. Add test cases for mixed operator expressions
EOF
)"

echo "[2/23] Creating: Incorrect operator semantics..."
gh issue create --repo "$REPO" \
  --title "[Critical] Incorrect operator semantics when combining continuations" \
  --label "bug" \
  --body "$(cat <<'EOF'
## Source
PR #13 review comments (coderabbitai)

## Description
Lines 339-342 and 349-352 in `parse_userset` always combine continuations using `Userset::Union`, even when `parse_userset_continuation` returns `and` operands. This violates operator semantics.

## Impact
- Same as operator precedence issue - could result in incorrect authorization decisions
- `A and B` would incorrectly be evaluated as `A or B`

## Location
`crates/rsfga-domain/src/model/parser.rs` (lines 339-342, 349-352)

## Recommended Fix
Track the operator type returned by `parse_userset_continuation` and build the appropriate `Userset` variant.
EOF
)"

# =============================================================================
# HIGH PRIORITY
# =============================================================================

echo "[3/23] Creating: Missing type constraint validation..."
gh issue create --repo "$REPO" \
  --title "[High] Missing type constraint validation in model validator" \
  --label "bug" \
  --body "$(cat <<'EOF'
## Source
PR #13 review comments (gemini-code-assist)

## Description
When a relation is defined with a type constraint like `define owner: [user]`, the validator should verify that the type `user` actually exists in the model. Currently this check is missing.

## Impact
Invalid authorization models could be created without proper validation, leading to runtime errors or unexpected behavior.

## Location
`crates/rsfga-domain/src/validation/mod.rs`

## Recommended Fix
Add validation logic to check that all types referenced in type constraints exist in the model's type definitions.
EOF
)"

echo "[4/23] Creating: Duplicate Store type..."
gh issue create --repo "$REPO" \
  --title "[High] Duplicate Store type with different timestamp representations" \
  --label "bug" \
  --body "$(cat <<'EOF'
## Source
PR #13 review comments (gemini-code-assist, coderabbitai)

## Description
Two different `Store` structs exist with different timestamp field types:
- `crates/rsfga-domain/src/model/types.rs` uses `Option<String>`
- `crates/rsfga-storage/src/traits.rs` uses `chrono::DateTime<Utc>`

## Impact
- Type confusion between layers
- Potential bugs when mapping between domain and storage layers
- Maintenance burden with duplicate definitions

## Recommended Fix
1. Unify the `Store` type definition
2. Use a single canonical representation (preferably `DateTime<Utc>`)
3. If separation is intentional, rename one to avoid confusion (e.g., `StorageStore` vs `DomainStore`)
EOF
)"

echo "[5/23] Creating: Tuple comparison omits user_relation..."
gh issue create --repo "$REPO" \
  --title "[High] Tuple comparison omits user_relation field" \
  --label "bug" \
  --body "$(cat <<'EOF'
## Source
PR #13 review comments (coderabbitai, augmentcode)

## Description
Both delete matching (lines 89-95) and duplicate checking (lines 101-107) logic compare all fields except `user_relation`. Two tuples that differ only by `user_relation` (e.g., `group:eng#member` vs `group:eng#admin`) would be incorrectly treated as duplicates.

## Impact
- Data integrity issues
- Incorrect deduplication could drop valid tuples
- Incorrect deletion could remove wrong tuples

## Location
`crates/rsfga-storage/src/memory.rs` (lines 89-95, 101-107)

## Recommended Fix
Include `user_relation` in all tuple comparison operations.
EOF
)"

echo "[6/23] Creating: reqwest::Client inefficiency..."
gh issue create --repo "$REPO" \
  --title "[High] reqwest::Client created per call instead of being reused" \
  --label "performance" \
  --body "$(cat <<'EOF'
## Source
PR #10 review comments (gemini-code-assist)

## Description
Creating a new `reqwest::Client` for each function call is inefficient. The `reqwest::Client` holds a connection pool internally and is meant to be created once and reused for multiple requests.

## Impact
- Performance degradation in tests
- Resource waste (connection pool recreation)
- Slower test execution

## Location
`crates/compatibility-tests/tests/common/mod.rs`

## Recommended Fix
Create a shared `reqwest::Client` instance (e.g., using `lazy_static` or `once_cell`) and reuse it across all test helper functions.
EOF
)"

echo "[7/23] Creating: Error handling masks failures..."
gh issue create --repo "$REPO" \
  --title "[High] Error handling masks failures in grpc_call" \
  --label "bug" \
  --body "$(cat <<'EOF'
## Source
PR #12 review comments (gemini-code-assist)

## Description
If `grpcurl` exits with a non-zero status code (indicating an error) but prints a JSON body to stdout, the `grpc_call` function returns that JSON as success instead of failing.

## Impact
- Test failures could be masked
- False positives in test results
- Bugs may go undetected

## Location
`crates/compatibility-tests/tests/common/mod.rs`

## Recommended Fix
Check the exit status of `grpcurl` before parsing the output. Return an error if the command failed, even if it produced JSON output.
EOF
)"

echo "[8/23] Creating: Incomplete batch check test..."
gh issue create --repo "$REPO" \
  --title "[High] Incomplete test: test_grpc_batch_check_matches_rest missing denied case" \
  --label "bug" \
  --body "$(cat <<'EOF'
## Source
PR #12 review comments (gemini-code-assist)

## Description
The comment on line 413 mentions checking for both an "allowed" (`check-1`) and a "denied" (`check-2`) case, but the test only includes assertions for `check-1`.

## Impact
- Test coverage gap
- Denied case behavior not being validated
- Potential parity issues between gRPC and REST could go undetected

## Location
`crates/compatibility-tests/tests/test_section_22_grpc_parity.rs`

## Recommended Fix
Add assertions for the `check-2` (denied) case as indicated in the comment.
EOF
)"

# =============================================================================
# MEDIUM PRIORITY
# =============================================================================

echo "[9/23] Creating: StoredTuple should derive Hash..."
gh issue create --repo "$REPO" \
  --title "[Medium] StoredTuple should derive Hash for efficiency" \
  --label "enhancement" \
  --body "$(cat <<'EOF'
## Source
PR #13 review comments (gemini-code-assist)

## Description
The `StoredTuple` struct should derive `Hash` to allow storage in `HashSet`, which is more efficient for checking existence and performing set operations than a `Vec`.

## Location
`crates/rsfga-storage/src/traits.rs`

## Recommended Fix
Add `#[derive(Hash)]` to `StoredTuple` and consider using `HashSet` in storage implementations.
EOF
)"

echo "[10/23] Creating: MemoryDataStore O(N) complexity..."
gh issue create --repo "$REPO" \
  --title "[Medium] MemoryDataStore has O(N) complexity issues" \
  --label "performance" \
  --body "$(cat <<'EOF'
## Source
PR #13 review comments (gemini-code-assist)

## Description
`MemoryDataStore` uses `Vec<StoredTuple>` to store tuples, resulting in O(N) complexity for write and delete operations due to linear scans for duplicate checks and item removal.

## Location
`crates/rsfga-storage/src/memory.rs`

## Recommended Fix
Consider using `HashSet<StoredTuple>` or a more efficient data structure for tuple storage to achieve O(1) lookups.
EOF
)"

echo "[11/23] Creating: Parser drops type constraint information..."
gh issue create --repo "$REPO" \
  --title "[Medium] Parser drops type constraint information" \
  --label "bug" \
  --body "$(cat <<'EOF'
## Source
PR #13 review comments (augmentcode)

## Description
`parse_relation_definition` parses a `type_constraint`, but that information is not preserved in `RelationDefinition`, so type restriction data is effectively dropped and can't be validated/enforced later.

## Location
`crates/rsfga-domain/src/model/parser.rs`

## Recommended Fix
1. Add a `type_constraints` field to `RelationDefinition`
2. Store the parsed type constraints in this field
3. Use type constraints during validation and authorization checks
EOF
)"

echo "[12/23] Creating: ValidationError has unused variants..."
gh issue create --repo "$REPO" \
  --title "[Medium] ValidationError has unused variants" \
  --label "code-quality" \
  --body "$(cat <<'EOF'
## Source
PR #13 review comments (gemini-code-assist)

## Description
The `ValidationError` enum includes `UndefinedType` and `MissingRelationDefinition` variants that are never constructed anywhere in the validation module.

## Location
`crates/rsfga-domain/src/validation/mod.rs`

## Recommended Fix
Either:
1. Implement the validations that would use these error variants, or
2. Remove the unused variants to reduce dead code
EOF
)"

echo "[13/23] Creating: create_store silently overwrites..."
gh issue create --repo "$REPO" \
  --title "[Medium] create_store silently overwrites existing stores" \
  --label "bug" \
  --body "$(cat <<'EOF'
## Source
PR #13 review comments (augmentcode)

## Description
`create_store` unconditionally inserts into `stores`/`tuples`; if the same `id` is reused this will overwrite the existing store and reset its tuple list, which is unexpected data loss for callers.

## Location
`crates/rsfga-storage/src/memory.rs`

## Recommended Fix
Check if the store already exists before inserting, and return an error if it does.
EOF
)"

echo "[14/23] Creating: Code duplication for gRPC helpers..."
gh issue create --repo "$REPO" \
  --title "[Medium] Code duplication for gRPC helpers across test files" \
  --label "code-quality" \
  --body "$(cat <<'EOF'
## Source
PR #12 review comments (gemini-code-assist)

## Description
Helper functions like `get_grpc_url`, `grpcurl_with_data`, `grpc_call` are duplicated across multiple test files:
- `test_section_21_grpc_setup.rs`
- `test_section_22_grpc_parity.rs`
- `test_section_23_grpc_features.rs`

## Recommended Fix
Move common gRPC helper functions to `common/mod.rs` and import them in each test file.
EOF
)"

echo "[15/23] Creating: get_grpc_url is brittle..."
gh issue create --repo "$REPO" \
  --title "[Medium] get_grpc_url is brittle with hardcoded port" \
  --label "bug" \
  --body "$(cat <<'EOF'
## Source
PR #12 review comments (gemini-code-assist)

## Description
`get_grpc_url` relies on string replacement of hardcoded port `:18080` and only handles `http://` scheme. This will not work correctly if `OPENFGA_URL` is configured differently.

## Location
`crates/compatibility-tests/tests/common/mod.rs`

## Recommended Fix
Use proper URL parsing (e.g., `url` crate) to extract and modify the port, and handle both `http://` and `https://` schemes.
EOF
)"

echo "[16/23] Creating: rest_call doesn't check status codes..."
gh issue create --repo "$REPO" \
  --title "[Medium] rest_call doesn't check HTTP status codes" \
  --label "bug" \
  --body "$(cat <<'EOF'
## Source
PR #12 review comments (augmentcode)

## Description
`rest_call` returns parsed JSON without checking `response.status()`. This can let a failing REST request look like success (e.g., tests that only assert `is_object()`).

## Location
`crates/compatibility-tests/tests/test_section_22_grpc_parity.rs`

## Recommended Fix
Check the response status code and fail early on non-2xx responses before parsing JSON.
EOF
)"

echo "[17/23] Creating: URL encoding issue..."
gh issue create --repo "$REPO" \
  --title "[Medium] URL encoding issue with # character in tests" \
  --label "bug" \
  --body "$(cat <<'EOF'
## Source
PR #12 review comments (augmentcode)

## Description
A test URL includes `#` in the path (`invalid-id!@#`), but `#` is treated as a fragment delimiter and won't be sent to the server. The HTTP request isn't actually testing the same invalid ID as the gRPC call.

## Location
`crates/compatibility-tests/tests/test_section_23_grpc_features.rs`

## Recommended Fix
Percent-encode special characters in the path segment (e.g., `#` becomes `%23`).
EOF
)"

echo "[18/23] Creating: Rate limiting test is flaky..."
gh issue create --repo "$REPO" \
  --title "[Medium] Rate limiting test timing assertion is flaky" \
  --label "bug" \
  --body "$(cat <<'EOF'
## Source
PR #11 review comments (augmentcode)

## Description
The wall-clock assertion (`duration.as_secs() < 10`) can be flaky on slower CI/loaded runners and isn't directly tied to validating "no rate limiting".

## Location
`crates/compatibility-tests/tests/test_section_18_rate_limiting.rs`

## Recommended Fix
Consider relaxing or removing this timing check so the test only fails on behavioral differences rather than timing.
EOF
)"

echo "[19/23] Creating: Error message checking is brittle..."
gh issue create --repo "$REPO" \
  --title "[Medium] Error message checking is brittle" \
  --label "bug" \
  --body "$(cat <<'EOF'
## Source
PR #11 review comments (augmentcode)

## Description
`error.message.contains("StoreId")` is case/wording-dependent and may break across OpenFGA versions even if the structured error contract stays the same.

## Location
`crates/compatibility-tests/tests/test_section_17_error_format.rs`

## Recommended Fix
Make the check more tolerant (e.g., case-insensitive) or focus on structured error fields rather than message text.
EOF
)"

echo "[20/23] Creating: HTTP 400 handling could mask errors..."
gh issue create --repo "$REPO" \
  --title "[Medium] HTTP 400 handling could mask other errors" \
  --label "bug" \
  --body "$(cat <<'EOF'
## Source
PR #11 review comments (augmentcode)

## Description
In the concurrent-write analysis, any HTTP 400 is treated as a "duplicate tuple" outcome. This could mask other unexpected 400s (e.g., validation or model errors).

## Location
`crates/compatibility-tests/tests/test_section_19_consistency.rs`

## Recommended Fix
Parse the 400 response body and assert the expected error `code` for the duplicate tuple case specifically.
EOF
)"

echo "[21/23] Creating: unwrap_or(false) could mask regressions..."
gh issue create --repo "$REPO" \
  --title "[Medium] unwrap_or(false) could let API contract regressions pass" \
  --label "bug" \
  --body "$(cat <<'EOF'
## Source
PR #10 review comments (augmentcode)

## Description
Using `unwrap_or(false)` means a malformed `/check` response (missing/non-boolean `allowed`) gets treated as `false`, which can let "expected false" tests pass even if the API contract regresses.

## Location
`crates/compatibility-tests/tests/common/mod.rs`

## Recommended Fix
Fail the helper when `allowed` field is missing or has an unexpected type, rather than defaulting to `false`.
EOF
)"

echo "[22/23] Creating: Module docs don't match implementation..."
gh issue create --repo "$REPO" \
  --title "[Medium] Validation module docs don't match implementation" \
  --label "documentation" \
  --body "$(cat <<'EOF'
## Source
PR #13 review comments (augmentcode)

## Description
The module docs list "All referenced types exist" / "Type constraints are satisfied", but the current validator only checks relation references + cycles. The documentation is misleading.

## Location
`crates/rsfga-domain/src/validation/mod.rs`

## Recommended Fix
Either:
1. Update the docs to accurately reflect what the validator currently checks, or
2. Implement the missing validations mentioned in the docs
EOF
)"

echo "[23/23] Creating: Race condition in store creation..."
gh issue create --repo "$REPO" \
  --title "[Medium] Race condition between existence check and insertion in create_store" \
  --label "bug" \
  --body "$(cat <<'EOF'
## Source
PR #13 review comments (coderabbitai)

## Description
The `contains_key` check (line 36) and `insert` (line 50) are not atomic. Under concurrent access, two threads could both pass the check and one would silently overwrite the other's store.

## Location
`crates/rsfga-storage/src/memory.rs`

## Recommended Fix
Use DashMap's `entry` API or similar atomic operations to ensure check-and-insert is atomic.
EOF
)"

echo ""
echo "=========================================="
echo "All 23 issues created successfully!"
echo "=========================================="
echo ""
echo "Summary:"
echo "  - Critical: 2 issues"
echo "  - High: 6 issues"
echo "  - Medium: 15 issues"
echo ""
echo "View issues at: https://github.com/$REPO/issues"
