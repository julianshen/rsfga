#!/bin/bash
# Script to create remaining GitHub issues (4-23)
# Fixed: Uses only existing labels, fixed heredoc syntax
# Repository: julianshen/rsfga

set -e

REPO="julianshen/rsfga"

echo "Creating remaining GitHub issues..."
echo ""

echo "[4/23] Creating: Duplicate Store type..."
gh issue create --repo "$REPO" \
  --title "[High] Duplicate Store type with different timestamp representations" \
  --label "bug" \
  --body '## Source
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
3. If separation is intentional, rename one to avoid confusion'

echo "[5/23] Creating: Tuple comparison omits user_relation..."
gh issue create --repo "$REPO" \
  --title "[High] Tuple comparison omits user_relation field" \
  --label "bug" \
  --body '## Source
PR #13 review comments (coderabbitai, augmentcode)

## Description
Both delete matching (lines 89-95) and duplicate checking (lines 101-107) logic compare all fields except `user_relation`. Two tuples that differ only by `user_relation` would be incorrectly treated as duplicates.

## Impact
- Data integrity issues
- Incorrect deduplication could drop valid tuples
- Incorrect deletion could remove wrong tuples

## Location
`crates/rsfga-storage/src/memory.rs`

## Recommended Fix
Include `user_relation` in all tuple comparison operations.'

echo "[6/23] Creating: reqwest::Client inefficiency..."
gh issue create --repo "$REPO" \
  --title "[High] reqwest::Client created per call instead of being reused" \
  --label "enhancement" \
  --body '## Source
PR #10 review comments (gemini-code-assist)

## Description
Creating a new `reqwest::Client` for each function call is inefficient. The `reqwest::Client` holds a connection pool internally and is meant to be created once and reused.

## Impact
- Performance degradation in tests
- Resource waste (connection pool recreation)

## Location
`crates/compatibility-tests/tests/common/mod.rs`

## Recommended Fix
Create a shared `reqwest::Client` instance and reuse it across all test helper functions.'

echo "[7/23] Creating: Error handling masks failures..."
gh issue create --repo "$REPO" \
  --title "[High] Error handling masks failures in grpc_call" \
  --label "bug" \
  --body '## Source
PR #12 review comments (gemini-code-assist)

## Description
If `grpcurl` exits with a non-zero status code but prints a JSON body to stdout, the `grpc_call` function returns that JSON as success instead of failing.

## Impact
- Test failures could be masked
- False positives in test results

## Location
`crates/compatibility-tests/tests/common/mod.rs`

## Recommended Fix
Check the exit status of `grpcurl` before parsing the output.'

echo "[8/23] Creating: Incomplete batch check test..."
gh issue create --repo "$REPO" \
  --title "[High] Incomplete test: test_grpc_batch_check_matches_rest missing denied case" \
  --label "bug" \
  --body '## Source
PR #12 review comments (gemini-code-assist)

## Description
The comment mentions checking for both "allowed" and "denied" cases, but the test only includes assertions for the allowed case.

## Impact
- Test coverage gap
- Denied case behavior not being validated

## Location
`crates/compatibility-tests/tests/test_section_22_grpc_parity.rs`

## Recommended Fix
Add assertions for the denied case as indicated in the comment.'

echo "[9/23] Creating: StoredTuple should derive Hash..."
gh issue create --repo "$REPO" \
  --title "[Medium] StoredTuple should derive Hash for efficiency" \
  --label "enhancement" \
  --body '## Source
PR #13 review comments (gemini-code-assist)

## Description
The `StoredTuple` struct should derive `Hash` to allow storage in `HashSet`, which is more efficient for existence checks.

## Location
`crates/rsfga-storage/src/traits.rs`

## Recommended Fix
Add `#[derive(Hash)]` to `StoredTuple`.'

echo "[10/23] Creating: MemoryDataStore O(N) complexity..."
gh issue create --repo "$REPO" \
  --title "[Medium] MemoryDataStore has O(N) complexity issues" \
  --label "enhancement" \
  --body '## Source
PR #13 review comments (gemini-code-assist)

## Description
`MemoryDataStore` uses `Vec<StoredTuple>` resulting in O(N) complexity for write and delete operations.

## Location
`crates/rsfga-storage/src/memory.rs`

## Recommended Fix
Consider using `HashSet<StoredTuple>` for O(1) lookups.'

echo "[11/23] Creating: Parser drops type constraint information..."
gh issue create --repo "$REPO" \
  --title "[Medium] Parser drops type constraint information" \
  --label "bug" \
  --body '## Source
PR #13 review comments (augmentcode)

## Description
`parse_relation_definition` parses type constraints but does not preserve them in `RelationDefinition`.

## Location
`crates/rsfga-domain/src/model/parser.rs`

## Recommended Fix
Add a `type_constraints` field to `RelationDefinition` and store the parsed constraints.'

echo "[12/23] Creating: ValidationError has unused variants..."
gh issue create --repo "$REPO" \
  --title "[Medium] ValidationError has unused variants" \
  --label "bug" \
  --body '## Source
PR #13 review comments (gemini-code-assist)

## Description
`ValidationError` enum includes `UndefinedType` and `MissingRelationDefinition` variants that are never constructed.

## Location
`crates/rsfga-domain/src/validation/mod.rs`

## Recommended Fix
Either implement the missing validations or remove unused variants.'

echo "[13/23] Creating: create_store silently overwrites..."
gh issue create --repo "$REPO" \
  --title "[Medium] create_store silently overwrites existing stores" \
  --label "bug" \
  --body '## Source
PR #13 review comments (augmentcode)

## Description
`create_store` unconditionally inserts, overwriting existing stores if the same ID is reused.

## Location
`crates/rsfga-storage/src/memory.rs`

## Recommended Fix
Check if store exists before inserting and return an error if it does.'

echo "[14/23] Creating: Code duplication for gRPC helpers..."
gh issue create --repo "$REPO" \
  --title "[Medium] Code duplication for gRPC helpers across test files" \
  --label "enhancement" \
  --body '## Source
PR #12 review comments (gemini-code-assist)

## Description
Helper functions like `get_grpc_url`, `grpcurl_with_data` are duplicated across multiple test files.

## Recommended Fix
Move common gRPC helpers to `common/mod.rs`.'

echo "[15/23] Creating: get_grpc_url is brittle..."
gh issue create --repo "$REPO" \
  --title "[Medium] get_grpc_url is brittle with hardcoded port" \
  --label "bug" \
  --body '## Source
PR #12 review comments (gemini-code-assist)

## Description
`get_grpc_url` relies on string replacement of hardcoded port `:18080` and only handles `http://` scheme.

## Location
`crates/compatibility-tests/tests/common/mod.rs`

## Recommended Fix
Use proper URL parsing to extract and modify the port.'

echo "[16/23] Creating: rest_call does not check status codes..."
gh issue create --repo "$REPO" \
  --title "[Medium] rest_call does not check HTTP status codes" \
  --label "bug" \
  --body '## Source
PR #12 review comments (augmentcode)

## Description
`rest_call` returns parsed JSON without checking `response.status()`.

## Location
`crates/compatibility-tests/tests/test_section_22_grpc_parity.rs`

## Recommended Fix
Check response status and fail early on non-2xx responses.'

echo "[17/23] Creating: URL encoding issue..."
gh issue create --repo "$REPO" \
  --title "[Medium] URL encoding issue with # character in tests" \
  --label "bug" \
  --body '## Source
PR #12 review comments (augmentcode)

## Description
Test URL includes `#` which is treated as fragment delimiter and not sent to server.

## Location
`crates/compatibility-tests/tests/test_section_23_grpc_features.rs`

## Recommended Fix
Percent-encode special characters in path.'

echo "[18/23] Creating: Rate limiting test is flaky..."
gh issue create --repo "$REPO" \
  --title "[Medium] Rate limiting test timing assertion is flaky" \
  --label "bug" \
  --body '## Source
PR #11 review comments (augmentcode)

## Description
Wall-clock assertion can be flaky on slower CI runners.

## Location
`crates/compatibility-tests/tests/test_section_18_rate_limiting.rs`

## Recommended Fix
Relax or remove timing check.'

echo "[19/23] Creating: Error message checking is brittle..."
gh issue create --repo "$REPO" \
  --title "[Medium] Error message checking is brittle" \
  --label "bug" \
  --body '## Source
PR #11 review comments (augmentcode)

## Description
`error.message.contains("StoreId")` is case/wording-dependent.

## Location
`crates/compatibility-tests/tests/test_section_17_error_format.rs`

## Recommended Fix
Make check case-insensitive or focus on structured error fields.'

echo "[20/23] Creating: HTTP 400 handling could mask errors..."
gh issue create --repo "$REPO" \
  --title "[Medium] HTTP 400 handling could mask other errors" \
  --label "bug" \
  --body '## Source
PR #11 review comments (augmentcode)

## Description
Any HTTP 400 is treated as "duplicate tuple" which could mask other errors.

## Location
`crates/compatibility-tests/tests/test_section_19_consistency.rs`

## Recommended Fix
Parse 400 body and assert expected error code.'

echo "[21/23] Creating: unwrap_or(false) could mask regressions..."
gh issue create --repo "$REPO" \
  --title "[Medium] unwrap_or(false) could let API regressions pass" \
  --label "bug" \
  --body '## Source
PR #10 review comments (augmentcode)

## Description
Malformed `/check` response with missing `allowed` field defaults to `false`.

## Location
`crates/compatibility-tests/tests/common/mod.rs`

## Recommended Fix
Fail when `allowed` field is missing.'

echo "[22/23] Creating: Module docs do not match implementation..."
gh issue create --repo "$REPO" \
  --title "[Medium] Validation module docs do not match implementation" \
  --label "documentation" \
  --body '## Source
PR #13 review comments (augmentcode)

## Description
Module docs claim validations that are not implemented.

## Location
`crates/rsfga-domain/src/validation/mod.rs`

## Recommended Fix
Update docs or implement missing validations.'

echo "[23/23] Creating: Race condition in store creation..."
gh issue create --repo "$REPO" \
  --title "[Medium] Race condition in create_store" \
  --label "bug" \
  --body '## Source
PR #13 review comments (coderabbitai)

## Description
`contains_key` check and `insert` are not atomic, allowing race conditions.

## Location
`crates/rsfga-storage/src/memory.rs`

## Recommended Fix
Use atomic entry API.'

echo ""
echo "=========================================="
echo "All remaining issues created!"
echo "=========================================="
