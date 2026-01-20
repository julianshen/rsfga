# Follow-up Issues from PR #233

Create these issues on GitHub after the PR is merged.

---

## Issue 1: Extract proto-JSON conversion to separate testable module

**Title:** `Refactor: Extract proto-JSON conversion to separate testable module`

**Labels:** `refactor`, `testing`

### Description

The proto-to-JSON and JSON-to-proto conversion code in `service.rs` is currently inline and difficult to test independently. This code handles critical data transformations for authorization models and should be extracted into a separate module with dedicated unit tests.

### Current State

- Conversion functions like `stored_model_to_proto`, `json_to_type_definition`, `json_to_userset`, etc. are embedded in `service.rs`
- These functions use complex pattern matching and error handling
- Testing requires full gRPC service setup even for simple conversion tests

### Proposed Changes

1. Create a new module `crates/rsfga-api/src/grpc/conversion.rs`
2. Move all proto-JSON conversion functions to this module
3. Add comprehensive unit tests for each conversion function
4. Test edge cases: malformed JSON, missing fields, type mismatches

### Benefits

- Easier to test conversion logic in isolation
- Better separation of concerns
- Enables property-based testing for conversion roundtrips
- Simplifies debugging when conversion failures occur

### Related

- PR #233 review comment about testability

---

## Issue 2: Standardize error message formatting across API layers

**Title:** `Refactor: Standardize error message formatting across API layers`

**Labels:** `refactor`, `developer-experience`

### Description

Error messages across gRPC and HTTP endpoints have inconsistent formatting, making it harder to parse errors programmatically and provide consistent user experience.

### Current State

Examples of inconsistent error messages:
- `"store not found: {store_id}"` vs `"Store not found"`
- `"invalid tuple_key at writes index {i}: ..."` vs `"assertion at index {}: condition '{}' has invalid context"`
- Some errors include field names, others don't
- Capitalization varies

### Proposed Changes

1. Define a consistent error message format:
   - Use lowercase for consistency with OpenFGA
   - Always include relevant identifiers (store_id, model_id, index)
   - Use structured format: `"{entity} {action}: {details}"`

2. Create error message constants or builder functions

3. Update all error messages across:
   - `crates/rsfga-api/src/grpc/service.rs`
   - `crates/rsfga-api/src/http/routes.rs`

4. Add tests to verify error message format consistency

### Example Format

```
store not found: store_id={id}
invalid tuple at index {i}: {reason}
condition context invalid: condition={name}, error={msg}
```

### Related

- PR #233 review comment about error handling consistency

---

## Issue 3: Add assertion cleanup for obsolete model versions

**Title:** `Enhancement: Add assertion cleanup for obsolete model versions`

**Labels:** `enhancement`, `memory-management`

**Priority:** Low

### Description

Assertions are only cleaned up when stores are deleted, not when individual authorization models become obsolete. In long-running production environments with frequent model updates, this can lead to orphaned assertion data consuming memory.

### Current State

- Assertions are stored in-memory keyed by `(store_id, model_id)`
- Cleanup only occurs when `delete_store` is called
- OpenFGA doesn't support model deletion (models are immutable)
- Current limit of 10,000 entries provides some protection

### Proposed Enhancement

Add optional assertion cleanup mechanisms:

#### Option 1: TTL-based Expiration
- Track last access time for each assertion entry
- Periodically evict entries not accessed within a configurable TTL
- Default: 24 hours for testing assertions

#### Option 2: Model Version Cleanup
- When a new model is written, offer option to clean up assertions for older model versions
- Could be a configuration flag or API parameter

#### Option 3: Manual Cleanup API
- Add `DELETE /stores/{store_id}/assertions/{model_id}` endpoint
- Allows explicit cleanup of assertions for specific models

### Considerations

- This is a lower-priority enhancement
- Current 10,000 entry limit provides adequate protection
- OpenFGA also stores assertions in-memory without cleanup
- May require metrics to understand real-world usage patterns first

### Related

- PR #233 review comment about memory management
- Current implementation: `service.rs:84-96`
