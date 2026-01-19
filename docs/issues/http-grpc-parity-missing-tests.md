# HTTP/gRPC Parity Tests - Missing Scenarios

This document tracks missing test scenarios for HTTP/gRPC behavioral parity (related to #201).
Each section represents a GitHub issue to be created.

---

## Issue 1: Store Management API Parity Tests

**Title:** `[TEST] Add Store Management API parity tests (UpdateStore, ListStores)`

**Labels:** `area: api`, `enhancement`, `testing`

**Description:**

Add HTTP/gRPC parity tests for Store Management APIs that are currently missing from Section 22.

### Test Scenarios

- [ ] `test_grpc_update_store_matches_rest` - Modify store name via both APIs and verify matching response
- [ ] `test_grpc_list_stores_matches_rest` - List all stores via both APIs and verify consistent results
- [ ] `test_grpc_list_stores_pagination_parity` - Verify pagination with `page_size` and `continuation_token` across protocols
- [ ] `test_error_update_nonexistent_store_parity` - 404/NOT_FOUND for non-existent store update
- [ ] `test_error_create_store_empty_name_parity` - Validation behavior for empty store name
- [ ] `test_error_create_store_long_name_parity` - Validation for names >255 characters

### Acceptance Criteria

- All tests verify identical behavior between HTTP and gRPC
- Error responses map correctly (HTTP status codes to gRPC codes)
- Pagination tokens work consistently across protocols

---

## Issue 2: Authorization Model API Parity Tests

**Title:** `[TEST] Add Authorization Model API parity tests`

**Labels:** `area: api`, `enhancement`, `testing`, `priority: high`

**Description:**

Add HTTP/gRPC parity tests for Authorization Model APIs.

### Test Scenarios

- [ ] `test_grpc_write_authorization_model_matches_rest` - Create new model and verify identical ID/structure
- [ ] `test_grpc_read_authorization_model_matches_rest` - Retrieve specific model by ID
- [ ] `test_grpc_read_authorization_models_matches_rest` - List models via both APIs
- [ ] `test_grpc_read_authorization_models_pagination_parity` - Pagination with `page_size`, `continuation_token`
- [ ] `test_error_read_nonexistent_model_parity` - 404/NOT_FOUND for non-existent model
- [ ] `test_error_write_invalid_model_parity` - Invalid model structure error mapping
- [ ] `test_grpc_model_with_conditions_parity` - CEL condition definitions preserved
- [ ] `test_grpc_model_with_wildcards_parity` - Wildcard (`user:*`) handling
- [ ] `test_error_unsupported_schema_version_parity` - Schema version validation

### Acceptance Criteria

- Model creation returns identical authorization_model_id
- Model structure is preserved identically across protocols
- CEL conditions and metadata are handled consistently

---

## Issue 3: Assertions API Parity Tests

**Title:** `[TEST] Add Assertions API parity tests`

**Labels:** `area: api`, `enhancement`, `testing`

**Description:**

Add HTTP/gRPC parity tests for the Assertions API (ReadAssertions, WriteAssertions).

### Test Scenarios

- [ ] `test_grpc_write_assertions_matches_rest` - Write assertions and verify storage
- [ ] `test_grpc_read_assertions_matches_rest` - Retrieve assertions for a model
- [ ] `test_grpc_assertions_with_contextual_tuples_parity` - Assertions containing contextual tuple definitions
- [ ] `test_error_assertions_invalid_model_parity` - Error when model doesn't exist

### Acceptance Criteria

- Assertion structure is identical between protocols
- Contextual tuples in assertions handled consistently
- Error codes map correctly

---

## Issue 4: ReadChanges API Parity Tests

**Title:** `[TEST] Add ReadChanges (changelog) API parity tests`

**Labels:** `area: api`, `enhancement`, `testing`

**Description:**

Add HTTP/gRPC parity tests for the ReadChanges API (changelog functionality).

### Test Scenarios

- [ ] `test_grpc_read_changes_matches_rest` - Get changelog entries with matching structure
- [ ] `test_grpc_read_changes_pagination_parity` - Pagination with `page_size` and `continuation_token`
- [ ] `test_grpc_read_changes_type_filter_parity` - Filter by object type and verify consistent results
- [ ] `test_grpc_read_changes_timestamp_format_parity` - Timestamp format consistency (RFC 3339)

### Acceptance Criteria

- Change entries are structurally identical
- Pagination tokens work across protocols
- Type filtering returns same results
- Timestamps use consistent format

---

## Issue 5: Check API Advanced Parameters Parity Tests

**Title:** `[TEST] Add Check API advanced parameters parity tests`

**Labels:** `area: api`, `enhancement`, `testing`, `priority: high`

**Description:**

Add HTTP/gRPC parity tests for advanced Check API parameters that affect authorization resolution.

### Test Scenarios

- [ ] `test_grpc_check_with_contextual_tuples_parity` - Temporary tuples in check request
- [ ] `test_grpc_check_with_context_conditions_parity` - CEL context for conditional relations
- [ ] `test_grpc_check_with_consistency_preference_parity` - MINIMIZE_LATENCY vs HIGHER_CONSISTENCY
- [ ] `test_grpc_check_with_authorization_model_id_parity` - Explicit model ID parameter
- [ ] `test_error_check_invalid_contextual_tuple_parity` - Malformed contextual tuples
- [ ] `test_error_check_duplicate_contextual_tuple_parity` - Duplicate contextual tuples

### Acceptance Criteria

- Contextual tuples affect check results identically
- CEL conditions evaluate consistently
- Model ID parameter works the same way
- Error handling is consistent

---

## Issue 6: BatchCheck Advanced Features Parity Tests

**Title:** `[TEST] Add BatchCheck advanced features parity tests`

**Labels:** `area: api`, `enhancement`, `testing`

**Description:**

Add HTTP/gRPC parity tests for advanced BatchCheck features and error handling.

### Test Scenarios

- [ ] `test_grpc_batch_check_contextual_tuples_per_item_parity` - Different contextual tuples per check item
- [ ] `test_grpc_batch_check_with_context_conditions_parity` - Per-item CEL context
- [ ] `test_grpc_batch_check_with_authorization_model_id_parity` - Explicit model ID
- [ ] `test_grpc_batch_check_per_item_error_handling_parity` - One item succeeds, one errors
- [ ] `test_grpc_batch_check_large_batch_parity` - 500+ items batch handling

### Acceptance Criteria

- Per-item contextual tuples work consistently
- Error responses for individual items match
- Large batch handling is identical

---

## Issue 7: Write API Advanced Features Parity Tests

**Title:** `[TEST] Add Write API advanced features parity tests`

**Labels:** `area: api`, `enhancement`, `testing`

**Description:**

Add HTTP/gRPC parity tests for advanced Write API features.

### Test Scenarios

- [ ] `test_grpc_write_with_deletes_combined_parity` - Single request with both writes and deletes
- [ ] `test_grpc_write_with_authorization_model_id_parity` - Explicit model reference
- [ ] `test_error_write_duplicate_tuples_in_request_parity` - Same tuple twice in one request
- [ ] `test_error_write_invalid_model_id_parity` - Non-existent model ID
- [ ] `test_error_write_type_not_in_model_parity` - Undefined type
- [ ] `test_error_write_relation_not_in_model_parity` - Undefined relation

### Acceptance Criteria

- Combined write/delete operations work identically
- Error codes for validation failures match
- Model ID parameter handled consistently

---

## Issue 8: ListObjects/ListUsers Advanced Parity Tests

**Title:** `[TEST] Add ListObjects/ListUsers advanced parameters parity tests`

**Labels:** `area: api`, `enhancement`, `testing`

**Description:**

Add HTTP/gRPC parity tests for advanced ListObjects and ListUsers parameters.

### Test Scenarios

**ListObjects:**
- [ ] `test_grpc_list_objects_with_contextual_tuples_parity`
- [ ] `test_grpc_list_objects_with_context_conditions_parity`
- [ ] `test_grpc_list_objects_with_consistency_preference_parity`
- [ ] `test_grpc_list_objects_with_authorization_model_id_parity`

**ListUsers:**
- [ ] `test_grpc_list_users_with_contextual_tuples_parity`
- [ ] `test_grpc_list_users_with_context_conditions_parity`
- [ ] `test_grpc_list_users_with_consistency_preference_parity`
- [ ] `test_grpc_list_users_with_authorization_model_id_parity`
- [ ] `test_grpc_list_users_response_variations_parity` - Users, UsersetUsers, TypedWildcard types
- [ ] `test_grpc_list_users_excluded_users_parity` - excluded_users array handling

### Acceptance Criteria

- All parameter combinations work identically
- Response structures match exactly
- Excluded users handling is consistent

---

## Issue 9: Expand API Advanced Parity Tests

**Title:** `[TEST] Add Expand API advanced parity tests`

**Labels:** `area: api`, `enhancement`, `testing`

**Description:**

Add HTTP/gRPC parity tests for advanced Expand API scenarios.

### Test Scenarios

- [ ] `test_grpc_expand_tree_difference_parity` - Set difference operations
- [ ] `test_grpc_expand_tree_intersection_parity` - Intersection operations
- [ ] `test_grpc_expand_with_authorization_model_id_parity` - Explicit model ID
- [ ] `test_error_expand_nonexistent_type_parity` - Error for undefined type
- [ ] `test_error_expand_invalid_relation_parity` - Error for undefined relation

### Acceptance Criteria

- Tree structures for complex operations match exactly
- Error handling is consistent across protocols

---

## Issue 10: Error Code Mapping Comprehensive Tests

**Title:** `[TEST] Add comprehensive HTTP/gRPC error code mapping tests`

**Labels:** `area: api`, `enhancement`, `testing`, `priority: critical`

**Description:**

Add comprehensive tests to verify HTTP status codes map correctly to gRPC status codes.

### Error Mapping to Test

| HTTP Status | gRPC Code | Scenario |
|-------------|-----------|----------|
| 400 Bad Request | INVALID_ARGUMENT | Missing required fields, malformed JSON |
| 401 Unauthorized | UNAUTHENTICATED | Missing/invalid credentials |
| 403 Forbidden | PERMISSION_DENIED | Insufficient permissions |
| 404 Not Found | NOT_FOUND | Missing resource |
| 409 Conflict | ALREADY_EXISTS | Duplicate creation |
| 422 Unprocessable Entity | INVALID_ARGUMENT | Semantic validation failure |
| 429 Too Many Requests | RESOURCE_EXHAUSTED | Rate limiting |
| 500 Internal Server Error | INTERNAL | Unexpected errors |

### Test Scenarios

- [ ] `test_error_mapping_400_invalid_argument_parity` - Various 400 scenarios
- [ ] `test_error_mapping_404_not_found_comprehensive_parity` - All resource types
- [ ] `test_error_mapping_409_already_exists_parity` - Duplicate resources
- [ ] `test_error_mapping_invalid_continuation_token_parity` - Token validation
- [ ] `test_error_mapping_resolution_too_complex_parity` - Depth limit errors

### Acceptance Criteria

- Every HTTP 4xx/5xx status has corresponding gRPC code test
- Error messages contain equivalent information
- Error codes are semantically equivalent

---

## Issue 11: Response Format Edge Cases Parity Tests

**Title:** `[TEST] Add response format edge cases parity tests`

**Labels:** `area: api`, `enhancement`, `testing`

**Description:**

Add tests to verify response format consistency between HTTP and gRPC.

### Test Scenarios

- [ ] `test_timestamp_format_consistency_parity` - RFC 3339 format in both protocols
- [ ] `test_boolean_field_handling_parity` - Protobuf default (false) vs JSON null
- [ ] `test_empty_array_handling_parity` - Empty arrays vs omitted fields
- [ ] `test_null_empty_string_differentiation_parity` - Empty value representation
- [ ] `test_continuation_token_opaque_handling_parity` - Token structure consistency

### Acceptance Criteria

- All edge cases handled identically
- No semantic differences in response interpretation

---

## Issue 12: Boundary and Input Validation Parity Tests

**Title:** `[TEST] Add boundary and input validation parity tests`

**Labels:** `area: api`, `enhancement`, `testing`

**Description:**

Add tests for boundary conditions and input validation consistency.

### Test Scenarios

- [ ] `test_empty_strings_in_fields_parity` - Empty user/object/relation handling
- [ ] `test_very_long_strings_parity` - Strings >1000, >10000 characters
- [ ] `test_special_characters_in_ids_parity` - Unicode, emoji, symbols
- [ ] `test_numeric_only_ids_parity` - "123" as ID
- [ ] `test_ids_with_multiple_colons_parity` - "type:id:extra" handling
- [ ] `test_whitespace_handling_parity` - Leading/trailing spaces
- [ ] `test_case_sensitivity_parity` - Type/relation matching
- [ ] `test_large_batch_sizes_parity` - 500+, 1000+ item batches

### Acceptance Criteria

- Validation errors are consistent
- Boundary behavior matches exactly

---

## Summary

| Issue # | Title | Priority | Est. Tests |
|---------|-------|----------|------------|
| 1 | Store Management API | Medium | 6 |
| 2 | Authorization Model API | High | 9 |
| 3 | Assertions API | Medium | 4 |
| 4 | ReadChanges API | Medium | 4 |
| 5 | Check Advanced Parameters | High | 6 |
| 6 | BatchCheck Advanced | Medium | 5 |
| 7 | Write Advanced | Medium | 6 |
| 8 | ListObjects/ListUsers Advanced | High | 10 |
| 9 | Expand Advanced | Medium | 5 |
| 10 | Error Code Mapping | Critical | 5+ |
| 11 | Response Format Edge Cases | Medium | 5 |
| 12 | Boundary/Input Validation | Medium | 8 |

**Total: ~73 additional test scenarios across 12 issues**
