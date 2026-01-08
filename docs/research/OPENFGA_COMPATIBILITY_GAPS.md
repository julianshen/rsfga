# OpenFGA Compatibility Test Coverage Analysis

**Date**: 2026-01-08
**Author**: Claude (Research Task)
**Purpose**: Identify gaps in the RSFGA compatibility test suite compared to full OpenFGA API specification

---

## Executive Summary

The current compatibility test suite contains **~194 tests across 29 sections** covering the core OpenFGA functionality. However, several APIs and features are **NOT covered** or only partially covered. This document identifies these gaps for future implementation.

---

## Current Test Coverage (Complete)

### Fully Tested APIs

| API | Endpoint | Test Sections | Status |
|-----|----------|---------------|--------|
| CreateStore | POST /stores | Section 4 | ✅ Complete |
| GetStore | GET /stores/{store_id} | Section 4 | ✅ Complete |
| DeleteStore | DELETE /stores/{store_id} | Section 4 | ✅ Complete |
| ListStores | GET /stores | Section 4 | ✅ Complete |
| WriteAuthorizationModel | POST /stores/{store_id}/authorization-models | Section 5-6 | ✅ Complete |
| ReadAuthorizationModels | GET /stores/{store_id}/authorization-models | Section 5 | ✅ Partial |
| Write (Tuples) | POST /stores/{store_id}/write | Sections 7, 9, 25 | ✅ Complete |
| Read (Tuples) | POST /stores/{store_id}/read | Sections 8, 26 | ✅ Complete |
| Check | POST /stores/{store_id}/check | Sections 10-11, 13, 27 | ✅ Complete |
| BatchCheck | POST /stores/{store_id}/batch-check | Section 12 | ✅ Complete |
| Expand | POST /stores/{store_id}/expand | Section 14 | ✅ Complete |
| ListObjects | POST /stores/{store_id}/list-objects | Section 15 | ✅ Complete |

### Tested Features

- Direct relations (this keyword)
- Computed relations (union, intersection, difference)
- TupleToUserset (from keyword, parent-child)
- Wildcards (user:*)
- Usersets (type#relation)
- CEL Conditions (Milestone 0.8)
- Contextual tuples
- Context parameter for condition evaluation
- Multi-hop relation resolution (up to 25 levels)
- Cycle detection
- Authorization model versioning
- Concurrent write handling
- Read-after-write consistency
- Error handling and validation
- gRPC/REST parity

---

## GAPS - Not Covered

### 1. ListUsers API ⚠️ HIGH PRIORITY

**Endpoint**: `POST /stores/{store_id}/list-users`

**Description**: Returns all users of a given type that have a specified relationship with an object. This is the inverse of ListObjects.

**Availability**: OpenFGA v1.5.4+ (experimental, requires `--experimentals enable-list-users` flag)

**Request Parameters**:
```json
{
  "object": { "type": "document", "id": "planning" },
  "user_filters": [{ "type": "user" }],
  "relation": "reader",
  "authorization_model_id": "optional"
}
```

**Response Format**:
```json
{
  "users": [
    { "object": { "type": "user", "id": "anne" } },
    { "userset": { "type": "group", "id": "eng", "relation": "member" } },
    { "wildcard": { "type": "user" } }
  ]
}
```

**Test Cases Needed**:
- Basic ListUsers with user type filter
- ListUsers with userset filter (group#member)
- ListUsers returns wildcard results
- ListUsers with contextual tuples
- ListUsers with conditions and context
- ListUsers with computed relations
- Empty result handling
- Error handling (invalid type, non-existent store)

**Sources**: [OpenFGA ListUsers Docs](https://openfga.dev/docs/getting-started/perform-list-users)

---

### 2. ReadChanges API ⚠️ HIGH PRIORITY

**Endpoint**: `GET /stores/{store_id}/changes`

**Description**: Returns a paginated list of tuple changes (additions and deletions) sorted by ascending time. Essential for audit trails and synchronization.

**Request Parameters**:
- `type` (optional): Filter by object type
- `page_size` (optional): Default 50, max 100
- `continuation_token` (optional): Pagination token
- `start_time` (optional, v1.8.0+): ISO 8601 timestamp filter

**Response Format**:
```json
{
  "changes": [
    {
      "tuple_key": {
        "user": "user:alice",
        "relation": "viewer",
        "object": "document:doc1"
      },
      "operation": "TUPLE_OPERATION_WRITE",
      "timestamp": "2024-01-01T00:00:00Z"
    }
  ],
  "continuation_token": "..."
}
```

**Test Cases Needed**:
- Basic ReadChanges returns tuple modifications
- ReadChanges with type filter
- ReadChanges pagination (page_size, continuation_token)
- ReadChanges returns both writes and deletes
- Continuation token persistence (same token when no changes)
- ReadChanges with start_time parameter (v1.8.0+)
- Error: type filter mismatch with continuation token
- Empty result handling

**Sources**: [OpenFGA ReadChanges Docs](https://openfga.dev/docs/interacting/read-tuple-changes)

---

### 3. Assertions API ⚠️ MEDIUM PRIORITY

**Endpoints**:
- `PUT /stores/{store_id}/assertions/{authorization_model_id}` (WriteAssertions)
- `GET /stores/{store_id}/assertions/{authorization_model_id}` (ReadAssertions)

**Description**: Assertions are test cases stored with an authorization model to validate expected authorization outcomes. Used for regression testing and model validation.

**WriteAssertions Request**:
```json
{
  "assertions": [
    {
      "tuple_key": {
        "user": "user:anne",
        "relation": "can_view",
        "object": "document:roadmap"
      },
      "expectation": true
    },
    {
      "tuple_key": {
        "user": "user:bob",
        "relation": "can_edit",
        "object": "document:roadmap"
      },
      "expectation": false,
      "contextual_tuples": {
        "tuple_keys": [...]
      }
    }
  ]
}
```

**Test Cases Needed**:
- WriteAssertions creates assertions for model
- WriteAssertions replaces all existing assertions (upsert semantics)
- ReadAssertions returns all assertions for model
- Assertions with contextual tuples
- Assertions with condition context
- Empty assertions handling
- Invalid model_id error handling

**Sources**: [OpenFGA Testing Models](https://openfga.dev/docs/modeling/testing)

---

### 4. StreamedListObjects API ⚠️ LOW PRIORITY

**Endpoint**: `POST /stores/{store_id}/streamed-list-objects` (gRPC only)

**Description**: Same functionality as ListObjects but streams results incrementally. Useful for large result sets.

**Notes**:
- Only available via gRPC
- SDK support is being added incrementally
- Same request format as ListObjects
- Returns stream of objects instead of single response

**Test Cases Needed**:
- StreamedListObjects returns same results as ListObjects
- StreamedListObjects handles large result sets
- Error handling parity with ListObjects

---

### 5. Consistency Preference Parameter ⚠️ MEDIUM PRIORITY

**Parameter**: `consistency` in Check, ListObjects, ListUsers, Expand

**Values**:
- `MINIMIZE_LATENCY` (default): Uses cache when enabled
- `HIGHER_CONSISTENCY`: Bypasses cache, reads from database directly

**Availability**: OpenFGA v1.5.7+

**Test Cases Needed**:
- Check with MINIMIZE_LATENCY (default behavior)
- Check with HIGHER_CONSISTENCY bypasses cache
- ListObjects with consistency parameter
- ListUsers with consistency parameter
- Expand with consistency parameter
- Behavior when caching is disabled (both modes equivalent)

**Sources**: [OpenFGA Consistency Docs](https://openfga.dev/docs/interacting/consistency)

---

### 6. Modular Models (Schema 1.2) ⚠️ LOW PRIORITY

**Description**: Allows splitting authorization models across multiple files with module declarations and type extensions.

**Features**:
- `module` keyword for declaring modules
- `extend type` for extending existing types
- `fga.mod` manifest file
- Schema version 1.2

**Example**:
```
// core.fga
module core
model
  schema 1.2
type user
type organization
  relations
    define member: [user]

// wiki.fga
module wiki
extend type organization
  relations
    define can_create_space: member
```

**Test Cases Needed**:
- Model creation with schema 1.2
- Model with module declaration
- Model with type extension
- Combined model from multiple modules
- Error: extending non-existent type
- Error: duplicate relation in extension

**Notes**: Requires `--experimentals enable-modular-models` flag

**Sources**: [OpenFGA Modular Models](https://openfga.dev/docs/modeling/modular-models)

---

### 7. ReadAuthorizationModel (Single Model) ⚠️ LOW PRIORITY

**Endpoint**: `GET /stores/{store_id}/authorization-models/{id}`

**Description**: Retrieve a specific authorization model by ID.

**Current Status**: May be partially tested in Section 5, but needs explicit validation.

**Test Cases Needed**:
- Get specific model by ID
- Model not found error (404)
- Invalid model ID format error (400)
- Model contains all expected fields

---

### 8. ReadAuthorizationModels Pagination ⚠️ LOW PRIORITY

**Endpoint**: `GET /stores/{store_id}/authorization-models`

**Parameters**:
- `page_size`: Number of models per page
- `continuation_token`: Pagination token

**Test Cases Needed**:
- List models with pagination
- Continuation token works correctly
- Models returned in reverse chronological order

---

## Feature Comparison Matrix

| Feature | OpenFGA | RSFGA Tests | Gap |
|---------|---------|-------------|-----|
| Store CRUD | ✅ | ✅ | None |
| Authorization Models | ✅ | ✅ | Pagination |
| Tuple Write/Read | ✅ | ✅ | None |
| Check API | ✅ | ✅ | Consistency param |
| BatchCheck API | ✅ | ✅ | None |
| Expand API | ✅ | ✅ | Contextual tuples in expand |
| ListObjects API | ✅ | ✅ | Consistency param |
| **ListUsers API** | ✅ | ❌ | **Full implementation** |
| **ReadChanges API** | ✅ | ❌ | **Full implementation** |
| **Assertions API** | ✅ | ❌ | **Full implementation** |
| StreamedListObjects | ✅ | ❌ | gRPC streaming |
| CEL Conditions | ✅ | ✅ | None |
| Contextual Tuples | ✅ | ✅ | None |
| Wildcards | ✅ | ✅ | None |
| Usersets | ✅ | ✅ | None |
| TupleToUserset | ✅ | ✅ | None |
| Consistency Modes | ✅ | ❌ | Full implementation |
| Modular Models | ✅ | ❌ | Schema 1.2 |
| gRPC Parity | ✅ | ✅ | Streaming |

---

## Priority Recommendations

### Phase 1: High Priority (Milestone 0.9)
1. **ListUsers API** - Essential for reverse lookup scenarios
2. **ReadChanges API** - Required for audit trails and sync

### Phase 2: Medium Priority (Milestone 0.10)
3. **Assertions API** - Important for model testing workflows
4. **Consistency Preference** - Required for production deployments with caching

### Phase 3: Low Priority (Future)
5. **StreamedListObjects** - Nice-to-have for large result sets
6. **Modular Models** - Enterprise feature
7. **Pagination enhancements** - Minor gaps

---

## References

### Official Documentation
- [OpenFGA API Explorer](https://openfga.dev/api/service)
- [OpenFGA Configuration Language](https://openfga.dev/docs/configuration-language)
- [OpenFGA Concepts](https://openfga.dev/docs/concepts)
- [OpenFGA Relationship Queries](https://openfga.dev/docs/interacting/relationship-queries)

### Protocol Buffers
- [OpenFGA API GitHub](https://github.com/openfga/api)
- [Buf Registry](https://buf.build/openfga/api)

### SDKs
- [Go SDK API Docs](https://github.com/openfga/go-sdk/blob/main/docs/OpenFgaApi.md)

---

## Appendix: Complete OpenFGA API List

| # | Method | HTTP | Endpoint | Tested |
|---|--------|------|----------|--------|
| 1 | CreateStore | POST | /stores | ✅ |
| 2 | GetStore | GET | /stores/{store_id} | ✅ |
| 3 | DeleteStore | DELETE | /stores/{store_id} | ✅ |
| 4 | ListStores | GET | /stores | ✅ |
| 5 | WriteAuthorizationModel | POST | /stores/{store_id}/authorization-models | ✅ |
| 6 | ReadAuthorizationModel | GET | /stores/{store_id}/authorization-models/{id} | ⚠️ Partial |
| 7 | ReadAuthorizationModels | GET | /stores/{store_id}/authorization-models | ⚠️ Partial |
| 8 | Write | POST | /stores/{store_id}/write | ✅ |
| 9 | Read | POST | /stores/{store_id}/read | ✅ |
| 10 | Check | POST | /stores/{store_id}/check | ✅ |
| 11 | BatchCheck | POST | /stores/{store_id}/batch-check | ✅ |
| 12 | Expand | POST | /stores/{store_id}/expand | ✅ |
| 13 | ListObjects | POST | /stores/{store_id}/list-objects | ✅ |
| 14 | **ListUsers** | POST | /stores/{store_id}/list-users | ❌ |
| 15 | **ReadChanges** | GET | /stores/{store_id}/changes | ❌ |
| 16 | **WriteAssertions** | PUT | /stores/{store_id}/assertions/{model_id} | ❌ |
| 17 | **ReadAssertions** | GET | /stores/{store_id}/assertions/{model_id} | ❌ |
| 18 | StreamedListObjects | POST | /stores/{store_id}/streamed-list-objects | ❌ |
