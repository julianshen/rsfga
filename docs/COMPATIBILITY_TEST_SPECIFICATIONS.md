# OpenFGA Compatibility Test Specifications

**Methodology**: Specification by Example (SBE)
**Total Test Cases**: ~240 tests across 35 sections
**Purpose**: Validate RSFGA implementation against OpenFGA behavior

---

## Table of Contents

1. [Test Infrastructure](#1-test-infrastructure-sections-1-3)
2. [Store Management API](#2-store-management-api-section-4)
3. [Authorization Model API](#3-authorization-model-api-sections-5-6)
4. [Tuple Management API](#4-tuple-management-api-sections-7-9)
5. [Check API](#5-check-api-sections-10-13)
6. [Expand API](#6-expand-api-section-14)
7. [ListObjects API](#7-listobjects-api-section-15)
8. [Edge Cases & Error Handling](#8-edge-cases--error-handling-sections-16-20)
9. [gRPC API](#9-grpc-api-sections-21-23)
10. [CEL Conditions](#10-cel-conditions-sections-24-29)
11. [ListUsers API](#11-listusers-api-section-30)
12. [ReadChanges API](#12-readchanges-api-section-31)
13. [Assertions API](#13-assertions-api-section-32)
14. [Consistency Preference](#14-consistency-preference-section-33)
15. [StreamedListObjects API](#15-streamedlistobjects-api-section-34)
16. [Modular Models / Schema 1.2](#16-modular-models--schema-12-section-35)

---

## 1. Test Infrastructure (Sections 1-3)

### Section 1: Environment Setup

| Specification | Example |
|--------------|---------|
| **Can start OpenFGA via Docker** | `docker-compose up -d` starts OpenFGA on port 8080 (HTTP) and 8081 (gRPC) |
| **Can connect to HTTP API** | `GET http://localhost:8080/stores` returns 200 OK |
| **Can connect to gRPC API** | `grpcurl -plaintext localhost:8081 list` returns service list |
| **Environment teardown is idempotent** | `docker-compose down` can be called multiple times safely |

### Section 2: Test Generators

| Specification | Example |
|--------------|---------|
| **Can generate valid user identifiers** | `user:alice`, `user:bob123`, `user:test_user` |
| **Can generate valid object identifiers** | `document:doc1`, `folder:projects/readme` |
| **Can generate valid relation names** | `viewer`, `editor`, `owner`, `can_read` |
| **Can generate valid tuples** | `{user: "user:alice", relation: "viewer", object: "document:doc1"}` |
| **Can generate models with direct relations** | Model with `viewer: this` relation |
| **Can generate models with computed relations** | Model with `editor: union(owner, viewer)` |
| **Can generate deeply nested relations** | Relations with depth up to 25 levels |

### Section 3: Response Capture & Comparison

| Specification | Example |
|--------------|---------|
| **Can capture HTTP request/response pairs** | Stores method, URL, headers, body, status, response |
| **Can capture gRPC request/response pairs** | Stores method, request proto, response proto |
| **Can serialize captured data to JSON** | Test fixtures saved as `.json` files |
| **Can detect breaking changes in response format** | Field removed → test fails |
| **Can detect breaking changes in arrays** | Array order changed → detected |
| **Can detect breaking changes in nested objects** | Nested field type changed → detected |

---

## 2. Store Management API (Section 4)

### Create Store

```text
Given: No existing store
When:  POST /stores with {"name": "test-store"}
Then:  Response 201 with {"id": "<generated-ulid>", "name": "test-store", "created_at": "..."}
```

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_post_stores_creates_store_with_generated_id` | Empty system | POST /stores | Returns store with ULID format ID |
| `test_post_stores_returns_created_store_in_response` | Empty system | POST /stores | Response contains name and created_at |

### Get Store

```text
Given: Store "store-123" exists
When:  GET /stores/store-123
Then:  Response 200 with store details
```

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_get_store_retrieves_store_by_id` | Existing store | GET /stores/{id} | Returns store details |
| `test_get_store_returns_400_for_nonexistent_store` | No such store | GET /stores/{invalid-id} | Returns 400/404 error |

### Delete Store

```text
Given: Store "store-123" exists
When:  DELETE /stores/store-123
Then:  Response 204 No Content
```

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_delete_store_deletes_store` | Existing store | DELETE /stores/{id} | Store is removed |
| `test_delete_store_returns_400_for_nonexistent_store` | No such store | DELETE /stores/{id} | Returns error |
| `test_can_clean_up_test_stores` | Multiple stores | DELETE each | All stores removed |

### List Stores

```text
Given: 10 stores exist
When:  GET /stores?page_size=5
Then:  Response with 5 stores and continuation_token
```

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_list_stores_returns_paginated_results` | Many stores | GET /stores | Returns paginated list |
| `test_list_stores_respects_page_size_parameter` | 10 stores | GET /stores?page_size=3 | Returns 3 stores |
| `test_list_stores_continuation_token_works` | Many stores | GET with token | Returns next page |

---

## 3. Authorization Model API (Sections 5-6)

### Create Model

```text
Given: Store exists
When:  POST /stores/{id}/authorization-models with type definitions
Then:  Response 201 with authorization_model_id
```

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_post_authorization_model_creates_model` | Store exists | POST model | Model created |
| `test_model_creation_returns_generated_model_id` | Store exists | POST model | Returns ULID model ID |
| `test_can_create_model_with_direct_relations` | Store exists | Model with `this` | Success |
| `test_can_create_model_with_computed_relations` | Store exists | Model with `union` | Success |
| `test_can_create_model_with_wildcards` | Store exists | Model with `user:*` | Success |
| `test_can_create_model_with_this_keyword` | Store exists | Model with `define viewer: this` | Success |

### Example Model Structure

```json
{
  "schema_version": "1.1",
  "type_definitions": [
    { "type": "user" },
    {
      "type": "document",
      "relations": {
        "owner": { "this": {} },
        "editor": { "union": { "child": [{"this": {}}, {"computedUserset": {"relation": "owner"}}] } },
        "viewer": { "union": { "child": [{"this": {}}, {"computedUserset": {"relation": "editor"}}] } }
      },
      "metadata": {
        "relations": {
          "owner": { "directly_related_user_types": [{"type": "user"}] },
          "editor": { "directly_related_user_types": [{"type": "user"}] },
          "viewer": { "directly_related_user_types": [{"type": "user"}, {"type": "user", "wildcard": {}}] }
        }
      }
    }
  ]
}
```

### Get/List Models

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_get_authorization_model_retrieves_model` | Model exists | GET model by ID | Returns full model |
| `test_get_nonexistent_model_returns_error` | No such model | GET invalid ID | Returns error |
| `test_list_authorization_models` | Multiple models | GET models | Returns list |
| `test_can_retrieve_latest_model` | Multiple versions | GET latest | Returns newest |
| `test_model_response_includes_schema_version` | Model exists | GET model | Has schema_version |
| `test_model_response_includes_type_definitions` | Model exists | GET model | Has type_definitions |

### Validation Errors

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_invalid_model_syntax_returns_400` | Store exists | POST invalid JSON | 400 Bad Request |
| `test_duplicate_type_definitions_return_error` | Store exists | POST duplicate types | Error |
| `test_undefined_relation_references_return_error` | Store exists | Reference undefined relation | Error |

---

## 4. Tuple Management API (Sections 7-9)

### Write Tuples

```text
Given: Store with model exists
When:  POST /stores/{id}/write with tuple
Then:  Response 200 with empty object {}
```

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_write_single_tuple` | Store + model | Write 1 tuple | Success, {} response |
| `test_write_multiple_tuples` | Store + model | Write array of tuples | All written |
| `test_write_tuple_with_wildcard` | Model allows wildcards | Write `user:*` tuple | Success |
| `test_writes_are_idempotent` | Tuple exists | Write same tuple again | Success (no error) |
| `test_write_returns_empty_response` | Store + model | Write tuple | Returns `{}` |

### Write Tuple Example

```json
// Request: POST /stores/{store_id}/write
{
  "writes": {
    "tuple_keys": [
      {"user": "user:alice", "relation": "viewer", "object": "document:doc1"}
    ]
  }
}

// Response: 200 OK
{}
```

### Delete Tuples

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_can_delete_tuple` | Tuple exists | DELETE tuple | Tuple removed |
| `test_concurrent_writes_to_same_tuple` | Concurrent requests | Multiple writes | Last write wins |

### Read Tuples

```text
Given: Tuples exist in store
When:  POST /stores/{id}/read with filter
Then:  Response with matching tuples
```

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_read_tuples_by_filter` | Multiple tuples | Read with filter | Returns matches |
| `test_read_with_empty_filter` | Multiple tuples | Read without filter | Returns all |
| `test_read_with_user_filter` | Tuples for various users | Filter by user | Returns user's tuples |
| `test_read_with_object_filter` | Tuples for various objects | Filter by object | Returns object's tuples |
| `test_read_with_relation_filter` | Tuples with various relations | Filter by relation | Returns matching |
| `test_read_returns_empty_when_no_matches` | No matching tuples | Read with filter | Returns empty array |
| `test_read_continuation_token` | Many tuples | Paginated read | Token works |
| `test_read_respects_page_size` | Many tuples | Read with page_size | Respects limit |
| `test_read_tuple_with_userset` | Userset tuple | Read | Returns userset format |

### Edge Cases

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_write_nonexistent_type_returns_error` | Model without type | Write tuple | Error |
| `test_write_nonexistent_relation_returns_error` | Model without relation | Write tuple | Error |
| `test_write_invalid_format_returns_400` | Store exists | Invalid JSON | 400 error |
| `test_write_without_store_returns_error` | No store | Write tuple | Error |

---

## 5. Check API (Sections 10-13)

### Basic Check

```text
Given: Tuple (user:alice, viewer, document:doc1) exists
When:  POST /stores/{id}/check with {user: alice, relation: viewer, object: doc1}
Then:  Response {"allowed": true}
```

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_check_direct_relation` | Direct tuple exists | Check | `{allowed: true}` |
| `test_check_returns_true_when_tuple_exists` | Tuple exists | Check | `{allowed: true}` |
| `test_check_returns_false_when_tuple_does_not_exist` | No tuple | Check | `{allowed: false}` |
| `test_check_resolves_this_keyword` | Model with `this` | Check | Resolves correctly |
| `test_check_with_wildcards` | Wildcard tuple `user:*` | Check any user | `{allowed: true}` |

### Computed Relations

```text
Given: Model where editor = owner OR direct_editor
       Tuple (user:alice, owner, document:doc1) exists
When:  Check if alice is editor of doc1
Then:  {"allowed": true} (inherited from owner)
```

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_check_follows_computed_union` | Union relation | Check | Follows union logic |
| `test_check_follows_computed_intersection` | Intersection relation | Check | All branches must pass |
| `test_check_follows_computed_exclusion` | Exclusion (but not) | Check | Excludes specified |
| `test_check_intersection_all_required` | Intersection | Check | Requires all relations |
| `test_check_union_multiple_relations` | Union of 3+ | Check | Any relation suffices |
| `test_check_exclusion_but_not` | `but not` exclusion | Check | Correctly excludes |

### Complex Scenarios

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_check_multi_hop_relations` | A→B→C chain | Check A→C | Follows chain |
| `test_check_deeply_nested_relations` | 10+ levels deep | Check | Works to depth 25 |
| `test_check_cycle_detection` | Circular reference | Check | Detects cycle, doesn't hang |
| `test_complex_nested_computed_relations` | Complex model | Check | Correct evaluation |
| `test_check_with_contextual_tuples` | No stored tuples | Check with context | Uses contextual tuples |
| `test_check_respects_model_version` | Multiple model versions | Check with model_id | Uses specified version |

### Error Cases

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_check_with_invalid_format` | N/A | Invalid JSON | 400 error |
| `test_check_with_nonexistent_store` | No such store | Check | Error |
| `test_check_with_stale_model_id` | Deleted model | Check | Error |

### Batch Check

```text
Given: Multiple tuples to check
When:  POST /stores/{id}/batch-check with array of checks
Then:  Response with array of results in same order
```

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_batch_check_multiple_tuples` | Multiple tuples | Batch check | All results returned |
| `test_batch_check_preserves_order` | Ordered requests | Batch check | Results in same order |
| `test_batch_check_mixed_results` | Some allowed, some not | Batch check | Correct per-item results |
| `test_batch_check_deduplication` | Duplicate requests | Batch check | Deduplicates internally |
| `test_batch_check_empty_array` | Empty array | Batch check | Returns empty array |
| `test_batch_check_large_batch` | 100 checks | Batch check | All processed |

### Performance

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_check_latency_direct_relation` | Direct tuple | Time check | < 50ms |
| `test_check_latency_computed_union` | Union relation | Time check | < 100ms |
| `test_check_latency_deep_nested` | 10 levels | Time check | < 500ms |

---

## 6. Expand API (Section 14)

### Basic Expand

```text
Given: Model with viewer = editor OR direct_viewer
       Tuples for user:alice as editor, user:bob as viewer
When:  POST /stores/{id}/expand {relation: viewer, object: doc1}
Then:  Response with relation tree showing both paths
```

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_expand_returns_relation_tree` | Tuples exist | Expand | Returns tree structure |
| `test_expand_shows_direct_relations_as_leaf` | Direct tuples | Expand | Shows in leaf nodes |
| `test_expand_shows_computed_relations` | Computed relations | Expand | Shows computation path |
| `test_expand_includes_union_branches` | Union relation | Expand | All branches shown |
| `test_expand_includes_intersection_branches` | Intersection | Expand | All required branches |
| `test_expand_includes_difference_branches` | Difference | Expand | Shows exclusion |
| `test_expand_tree_is_deterministic` | Same data | Expand twice | Same result |
| `test_expand_with_deep_relation_tree` | Deep nesting | Expand | Full tree returned |
| `test_expand_with_circular_relations` | Cycle in model | Expand | Handles gracefully |

### Error Cases

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_expand_with_invalid_store` | No such store | Expand | Error |
| `test_expand_with_nonexistent_object` | No tuples for object | Expand | Empty tree |

---

## 7. ListObjects API (Section 15)

### Basic ListObjects

```text
Given: user:alice has viewer on doc1, doc2, doc3
When:  POST /stores/{id}/list-objects {user: alice, relation: viewer, type: document}
Then:  Response {"objects": ["document:doc1", "document:doc2", "document:doc3"]}
```

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_listobjects_returns_accessible_objects` | Multiple accessible | List | All returned |
| `test_listobjects_with_no_accessible_objects` | No access | List | Empty array |
| `test_listobjects_with_direct_relations` | Direct tuples | List | Returns objects |
| `test_listobjects_with_computed_relations` | Computed access | List | Includes inherited |
| `test_listobjects_with_wildcards` | Wildcard tuple | List | Includes wildcard access |
| `test_listobjects_with_contextual_tuples` | Contextual only | List | Uses context |
| `test_listobjects_with_type_filter` | Multiple types | List with type | Filters by type |
| `test_listobjects_with_large_result_set` | 100+ objects | List | All returned |

---

## 8. Edge Cases & Error Handling (Sections 16-20)

### Section 16: Edge Cases

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_special_characters_in_identifiers` | ID with `_-@.` | Operations | Works correctly |
| `test_unicode_in_identifiers` | Unicode characters | Operations | Supported |
| `test_identifiers_at_size_limit` | Max length ID | Operations | Accepted |
| `test_identifiers_over_size_limit` | Too long ID | Operations | Rejected |

### Section 17: Error Format

```text
Given: Invalid request
When:  API call
Then:  Error response with {code, message} structure
```

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_400_bad_request_format` | Invalid input | Request | `{code: "...", message: "..."}` |
| `test_404_not_found_format` | Missing resource | Request | Standard 404 format |
| `test_500_error_format_documentation` | Server error | Request | Error structure documented |
| `test_error_codes_match_openfga_enum` | Various errors | Check codes | Match OpenFGA codes |
| `test_validation_errors_include_field_details` | Invalid field | Request | Field name in error |
| `test_error_response_consistent_across_endpoints` | Various endpoints | Errors | Same format |

### Section 18: Rate Limiting

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_api_rate_limiting_behavior` | Rapid requests | Many calls | Rate limit applied |
| `test_rate_limit_headers` | Any request | Check headers | X-RateLimit headers present |
| `test_retry_after_header_format` | Rate limited | Check header | Valid Retry-After |

### Section 19: Consistency

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_read_after_write_consistency` | Write tuple | Immediate read | Tuple visible |
| `test_check_consistency_after_write` | Write tuple | Immediate check | Returns true |
| `test_check_consistency_after_delete` | Delete tuple | Immediate check | Returns false |
| `test_check_during_model_update` | Update model | Check | Uses correct version |

### Section 20: Limits

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_maximum_tuples_in_single_write` | Max tuples | Write | Accepted |
| `test_maximum_tuple_size` | Large tuple | Write | Within limits |
| `test_maximum_authorization_model_size` | Large model | Create | Accepted |
| `test_maximum_checks_in_batch` | Max batch | Batch check | Processed |
| `test_maximum_relation_depth` | 25 levels | Check | Works at limit |
| `test_request_timeout_behavior` | Slow request | Wait | Timeout applied |

---

## 9. gRPC API (Sections 21-23)

### Section 21: gRPC Setup

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_can_connect_to_openfga_grpc_service` | Server running | Connect | Success |
| `test_grpc_reflection_works` | Server running | Describe service | Returns methods |
| `test_can_call_store_service_methods` | Server running | CreateStore | Works |
| `test_can_call_authorization_model_service_methods` | Store exists | WriteModel | Works |
| `test_can_call_tuple_service_methods` | Model exists | Write | Works |
| `test_can_call_check_service_methods` | Tuples exist | Check | Works |

### Section 22: gRPC/REST Parity

```text
Given: Same operation parameters
When:  Call via REST and gRPC
Then:  Identical results
```

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_grpc_store_create_matches_rest` | Create params | Both APIs | Same result |
| `test_grpc_store_get_matches_rest` | Store ID | Both APIs | Same result |
| `test_grpc_store_delete_matches_rest` | Store ID | Both APIs | Same result |
| `test_grpc_write_matches_rest` | Tuple data | Both APIs | Same result |
| `test_grpc_read_matches_rest` | Filter | Both APIs | Same result |
| `test_grpc_check_matches_rest` | Check params | Both APIs | Same result |
| `test_grpc_batch_check_matches_rest` | Batch params | Both APIs | Same result |
| `test_grpc_expand_matches_rest` | Expand params | Both APIs | Same result |
| `test_grpc_listobjects_matches_rest` | List params | Both APIs | Same result |

### Section 23: gRPC Features

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_grpc_error_codes_match_http` | Error condition | Both APIs | Equivalent codes |
| `test_grpc_error_details_format` | Error | gRPC call | Structured details |
| `test_grpc_metadata_headers` | Any call | Check metadata | Standard headers |
| `test_grpc_streaming_not_supported` | Server stream | Attempt | Not supported (except StreamedListObjects) |

---

## 10. CEL Conditions (Sections 24-29)

### Section 24: Model Conditions

```text
Given: Model with condition "is_weekday"
       condition is_weekday(context) { context.day_of_week < 6 }
When:  Create model
Then:  Model accepted with condition
```

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_can_create_model_with_conditions` | Condition in model | Create | Accepted |
| `test_condition_structure_name_expression_parameters` | Full condition | Create | All parts stored |
| `test_condition_uses_cel_syntax` | CEL expression | Create | Parsed correctly |
| `test_condition_parameter_types` | Various types | Create | Types supported |
| `test_relation_references_condition` | Relation with condition | Create | Linked correctly |
| `test_can_retrieve_model_with_conditions` | Model with conditions | Get | Conditions returned |

### Section 25: Tuple Conditions

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_can_write_tuple_with_condition_name` | Condition defined | Write with condition | Accepted |
| `test_can_write_tuple_with_condition_context` | Condition defined | Write with context | Accepted |
| `test_can_write_multiple_tuples_with_different_conditions` | Multiple conditions | Write batch | All accepted |
| `test_can_delete_tuple_with_condition` | Conditional tuple | Delete | Removed |
| `test_conditional_writes` | Condition model | Write | Stores condition reference |

### Section 26: Read Conditions

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_read_returns_tuple_with_condition_name` | Conditional tuple | Read | Condition name in response |
| `test_read_returns_tuple_with_condition_context` | Tuple with context | Read | Context in response |
| `test_read_filter_tuples_by_condition` | Mixed tuples | Read with filter | Filters correctly |

### Section 27: Check with Conditions

```text
Given: Tuple (alice, viewer, doc1) with condition "is_owner"
       Condition: context.user_id == "alice"
When:  Check with context {user_id: "alice"}
Then:  {"allowed": true}

When:  Check with context {user_id: "bob"}
Then:  {"allowed": false}
```

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_check_returns_true_when_condition_evaluates_true` | True condition | Check | Allowed |
| `test_check_returns_false_when_condition_evaluates_false` | False condition | Check | Not allowed |
| `test_check_accepts_context_parameter` | Condition model | Check with context | Processes context |
| `test_check_without_required_context_returns_error` | Required context | Check without | Error |
| `test_check_with_partial_context_returns_error` | Missing params | Check | Error |
| `test_check_with_wrong_context_type_returns_error` | Wrong type | Check | Error |
| `test_contextual_tuples_with_conditions` | Contextual + condition | Check | Both evaluated |
| `test_batch_check_evaluates_conditions` | Batch with conditions | Batch check | Each evaluated |

### Section 28: CEL Edge Cases

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_condition_evaluation_with_string_operations` | String CEL | Evaluate | Works |
| `test_condition_evaluation_with_timestamp_comparison` | Timestamp CEL | Evaluate | Works |
| `test_condition_evaluation_with_list_membership` | List `in` operator | Evaluate | Works |
| `test_condition_evaluation_with_logical_operators` | `&&`, `\|\|`, `!` | Evaluate | Works |
| `test_condition_context_values_match_types` | Type checking | Evaluate | Types enforced |
| `test_empty_context_no_required_params` | No params needed | Check | Works |
| `test_null_values_in_context` | Null in context | Check | Handled |
| `test_deeply_nested_map_values_in_context` | Nested maps | Check | Accessed correctly |
| `test_large_list_values_in_context` | Large list | Check | Processed |
| `test_very_long_string_values_in_context` | Long string | Check | Accepted |
| `test_cel_expression_undefined_variable` | Missing variable | Evaluate | Error |
| `test_cel_expression_division_by_zero` | Division by zero | Evaluate | Handled |
| `test_cel_expression_timeout_behavior` | Complex expression | Evaluate | Timeout applied |

### Section 29: Condition Performance

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_measure_check_latency_simple_condition` | Simple CEL | Time check | Baseline established |
| `test_measure_check_latency_complex_condition` | Complex CEL | Time check | Within limits |
| `test_measure_batch_check_with_conditions` | Batch + conditions | Time batch | Efficient |
| `test_compare_conditional_vs_non_conditional_latency` | Both types | Compare | Overhead measured |

---

## 11. ListUsers API (Section 30)

### Basic ListUsers

```text
Given: Tuples: (alice, viewer, doc1), (bob, viewer, doc1), (group:eng#member, viewer, doc1)
When:  POST /stores/{id}/list-users {object: doc1, relation: viewer, user_filters: [{type: user}]}
Then:  Response {"users": [{"object": {"type": "user", "id": "alice"}}, ...]}
```

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_listusers_returns_users_with_direct_relation` | Direct tuples | List users | Users returned |
| `test_listusers_with_userset_filter` | Userset tuples | Filter by userset | Usersets returned |
| `test_listusers_returns_wildcard` | Wildcard tuple | List users | Wildcard in response |
| `test_listusers_with_computed_relations` | Computed access | List users | Inherited users included |
| `test_listusers_with_contextual_tuples` | Contextual tuples | List users | Context evaluated |
| `test_listusers_empty_result` | No matching users | List users | Empty array |
| `test_listusers_accepts_consistency_parameter` | Consistency param | List users | Accepted |

### Error Cases

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_listusers_invalid_type_error` | Invalid type | List users | Error |
| `test_listusers_nonexistent_store_error` | No such store | List users | Error |

---

## 12. ReadChanges API (Section 31)

### Basic ReadChanges

```text
Given: Write tuple (alice, viewer, doc1), then delete it
When:  GET /stores/{id}/changes
Then:  Response with both WRITE and DELETE operations in chronological order
```

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_readchanges_returns_tuple_writes` | Write tuples | Read changes | WRITE operations |
| `test_readchanges_returns_writes_and_deletes` | Write + delete | Read changes | Both operations |
| `test_readchanges_chronological_order` | Multiple changes | Read changes | Ordered by time |
| `test_readchanges_empty_for_new_store` | New store | Read changes | Empty array |
| `test_readchanges_with_type_filter` | Multiple types | Filter by type | Only matching type |
| `test_readchanges_pagination_page_size` | Many changes | page_size param | Respects limit |
| `test_readchanges_continuation_token` | Many changes | Use token | Next page returned |
| `test_readchanges_same_token_when_no_changes` | No new changes | Poll with token | Same/similar token |
| `test_readchanges_type_filter_mismatch_with_token` | Token from type A | Filter type B | Error |

### Error Cases

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_readchanges_nonexistent_store_error` | Invalid store | Read changes | Error |

---

## 13. Assertions API (Section 32)

### Write Assertions

```text
Given: Store with model
When:  PUT /stores/{id}/assertions/{model_id} with test cases
Then:  Assertions stored for model validation
```

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_write_assertions_creates_assertions` | Model exists | Write assertions | Stored |
| `test_write_assertions_replaces_existing` | Assertions exist | Write new | Replaced |
| `test_write_empty_assertions` | Assertions exist | Write empty | Cleared |
| `test_assertions_with_contextual_tuples` | Contextual tuples | Write assertion | Context stored |
| `test_assertions_mixed_expectations` | true/false mix | Write assertions | Both stored |

### Read Assertions

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_read_assertions_returns_assertions` | Assertions exist | Read | All returned |
| `test_read_assertions_when_none_exist` | No assertions | Read | Empty array |

### Error Cases

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_assertions_invalid_model_id_error` | Invalid model ID | Write | Error |
| `test_assertions_nonexistent_store_error` | Invalid store | Write | Error |

---

## 14. Consistency Preference (Section 33)

### Consistency Parameter

```text
Given: Tuple exists
When:  Check with consistency: MINIMIZE_LATENCY
Then:  May use cache (faster)

When:  Check with consistency: HIGHER_CONSISTENCY
Then:  Reads from database (fresher)
```

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_check_accepts_consistency_parameter` | Any | MINIMIZE_LATENCY | Accepted |
| `test_check_with_higher_consistency` | Any | HIGHER_CONSISTENCY | Accepted |
| `test_check_with_unspecified_consistency` | Any | No param | Default behavior |
| `test_consistency_modes_return_same_results` | Same data | Both modes | Same result |
| `test_expand_accepts_consistency_parameter` | Any | With consistency | Accepted |
| `test_listobjects_accepts_consistency_parameter` | Any | With consistency | Accepted |
| `test_listusers_accepts_consistency_parameter` | Any | With consistency | Accepted |
| `test_batchcheck_accepts_consistency_parameter` | Any | With consistency | Accepted |
| `test_invalid_consistency_value_error` | Any | Invalid value | Error |

---

## 15. StreamedListObjects API (Section 34)

**Prerequisites**: grpcurl installed, OpenFGA with streaming support

### Basic Streaming

```text
Given: user:alice has viewer on 50 documents
When:  gRPC StreamedListObjects call
Then:  Objects streamed incrementally (not all at once)
```

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_streamed_listobjects_returns_same_as_listobjects` | Same data | Both APIs | Same objects |
| `test_streamed_listobjects_empty_results` | No access | Stream | Empty stream |
| `test_streamed_listobjects_large_result_set` | 50 objects | Stream | All 50 streamed |
| `test_streamed_listobjects_with_contextual_tuples` | Contextual tuples | Stream | Context evaluated |
| `test_streamed_listobjects_error_handling` | Invalid store | Stream | Error returned |

---

## 16. Modular Models / Schema 1.2 (Section 35)

### Schema 1.2 Support

```text
Given: OpenFGA with modular models enabled
When:  Create model with schema_version: "1.2"
Then:  Model created successfully
```

| Test Case | Given | When | Then |
|-----------|-------|------|------|
| `test_create_model_schema_1_2` | 1.2 enabled | Create model | Success |
| `test_schema_1_2_with_organization_type` | 1.2 enabled | Org type model | Success |
| `test_schema_1_2_backwards_compatible` | 1.1 + 1.2 models | Both in store | Works |
| `test_invalid_schema_version_error` | Any | schema_version: "2.0" | Error |
| `test_model_retrieval_includes_schema_version` | Model exists | Get model | Version in response |
| `test_schema_1_2_multi_type_model_compatibility` | 1.2 enabled | Multi-type model | Works |
| `test_schema_1_2_computed_relation_compatibility` | 1.2 enabled | Computed relations | Works |

---

## Appendix A: Test File to Section Mapping

| File | Section | Description |
|------|---------|-------------|
| `test_section_1.rs` | 1 | Environment Setup |
| `test_section_2.rs` | 2 | Test Generators |
| `test_section_3.rs` | 3 | Response Capture |
| `test_section_4.rs` | 4 | Store Management |
| `test_section_5.rs` | 5 | Model Creation |
| `test_section_6.rs` | 6 | Model Retrieval |
| `test_section_7_tuple_write.rs` | 7 | Tuple Write |
| `test_section_8_tuple_read.rs` | 8 | Tuple Read |
| `test_section_9_tuple_edge_cases.rs` | 9 | Tuple Edge Cases |
| `test_section_10_check_basic.rs` | 10 | Check Basic |
| `test_section_11_check_complex.rs` | 11 | Check Complex |
| `test_section_12_batch_check.rs` | 12 | Batch Check |
| `test_section_13_check_performance.rs` | 13 | Check Performance |
| `test_section_14_expand_api.rs` | 14 | Expand API |
| `test_section_15_listobjects_api.rs` | 15 | ListObjects |
| `test_section_16_edge_cases.rs` | 16 | Edge Cases |
| `test_section_17_error_format.rs` | 17 | Error Format |
| `test_section_18_rate_limiting.rs` | 18 | Rate Limiting |
| `test_section_19_consistency.rs` | 19 | Consistency |
| `test_section_20_limits.rs` | 20 | Limits |
| `test_section_21_grpc_setup.rs` | 21 | gRPC Setup |
| `test_section_22_grpc_parity.rs` | 22 | gRPC/REST Parity |
| `test_section_23_grpc_features.rs` | 23 | gRPC Features |
| `test_section_24_model_conditions.rs` | 24 | Model Conditions |
| `test_section_25_tuple_conditions.rs` | 25 | Tuple Conditions |
| `test_section_26_read_conditions.rs` | 26 | Read Conditions |
| `test_section_27_check_conditions.rs` | 27 | Check Conditions |
| `test_section_28_cel_edge_cases.rs` | 28 | CEL Edge Cases |
| `test_section_29_condition_performance.rs` | 29 | Condition Performance |
| `test_section_30_listusers_api.rs` | 30 | ListUsers API |
| `test_section_31_readchanges_api.rs` | 31 | ReadChanges API |
| `test_section_32_assertions_api.rs` | 32 | Assertions API |
| `test_section_33_consistency_preference.rs` | 33 | Consistency Preference |
| `test_section_34_streamed_listobjects.rs` | 34 | StreamedListObjects |
| `test_section_35_modular_models.rs` | 35 | Modular Models |

---

## Appendix B: Running Tests

```bash
# Run all compatibility tests
cd crates/compatibility-tests
docker-compose up -d
cargo test

# Run specific section
cargo test test_section_10

# Run with output
cargo test -- --nocapture

# Run single test
cargo test test_check_direct_relation
```

**Prerequisites**:
- Docker (for OpenFGA)
- Rust 1.75+
- grpcurl (optional, for gRPC tests)

---

## Appendix C: Specification by Example Methodology

This document follows **Specification by Example (SBE)** principles:

1. **Concrete Examples**: Each specification includes real data examples, not abstract descriptions
2. **Given-When-Then**: Tests follow the GWT pattern for clarity
3. **Living Documentation**: Tests ARE the specifications - they're executable and always current
4. **Collaboration Artifact**: This document bridges developers, testers, and stakeholders

### Benefits for RSFGA

- **Validation**: Every example is a test that validates OpenFGA compatibility
- **Regression Prevention**: Changes that break compatibility fail tests
- **Implementation Guide**: Developers know exactly what behavior to implement
- **API Documentation**: Examples serve as usage documentation

---

*Last Updated: 2026-01-09*
*Total Test Cases: ~240*
*Coverage: All OpenFGA public APIs*
