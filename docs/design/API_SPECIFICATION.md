# RSFGA API Specification

## Overview

RSFGA provides 100% API compatibility with OpenFGA, supporting both HTTP (REST) and gRPC protocols. This document specifies all API endpoints, request/response formats, and behavior.

**Base URLs**:
- HTTP: `http://localhost:8080` (default)
- gRPC: `localhost:8081` (default)

**API Version**: v1 (OpenFGA compatible)

---

## Table of Contents

1. [Authentication](#authentication)
2. [Stores API](#stores-api)
3. [Authorization Models API](#authorization-models-api)
4. [Relationship Tuples API](#relationship-tuples-api)
5. [Check API](#check-api)
6. [Batch Check API](#batch-check-api)
7. [Expand API](#expand-api)
8. [List Objects API](#list-objects-api)
9. [List Users API](#list-users-api)
10. [Error Responses](#error-responses)

---

## Authentication

### Bearer Token Authentication

All API requests require authentication via Bearer token:

```http
Authorization: Bearer <token>
```

**Example**:
```bash
curl -H "Authorization: Bearer sk_test_1234567890" \
  http://localhost:8080/stores
```

### API Key Authentication (Alternative)

```http
X-API-Key: <api-key>
```

---

## Stores API

### Create Store

Create a new authorization store.

**HTTP**:
```http
POST /stores
Content-Type: application/json

{
  "name": "My Application"
}
```

**Response** (`201 Created`):
```json
{
  "id": "01HQWV7Y8Z9X6W5V4U3T2S1R0Q",
  "name": "My Application",
  "created_at": "2024-01-15T10:30:00Z",
  "updated_at": "2024-01-15T10:30:00Z"
}
```

**gRPC**:
```protobuf
rpc CreateStore(CreateStoreRequest) returns (CreateStoreResponse);

message CreateStoreRequest {
  string name = 1;
}

message CreateStoreResponse {
  string id = 1;
  string name = 2;
  google.protobuf.Timestamp created_at = 3;
  google.protobuf.Timestamp updated_at = 4;
}
```

---

### Get Store

Retrieve store details.

**HTTP**:
```http
GET /stores/{store_id}
```

**Response** (`200 OK`):
```json
{
  "id": "01HQWV7Y8Z9X6W5V4U3T2S1R0Q",
  "name": "My Application",
  "created_at": "2024-01-15T10:30:00Z",
  "updated_at": "2024-01-15T10:30:00Z"
}
```

---

### List Stores

List all stores with pagination.

**HTTP**:
```http
GET /stores?page_size=10&continuation_token=<token>
```

**Response** (`200 OK`):
```json
{
  "stores": [
    {
      "id": "01HQWV7Y8Z9X6W5V4U3T2S1R0Q",
      "name": "My Application",
      "created_at": "2024-01-15T10:30:00Z",
      "updated_at": "2024-01-15T10:30:00Z"
    }
  ],
  "continuation_token": "eyJsYXN0X2lkIjoiMDFIUVdWN1k4WjlYNlc1VjRVM1QyUzFSMFEifQ=="
}
```

---

### Delete Store

Delete a store and all its data.

**HTTP**:
```http
DELETE /stores/{store_id}
```

**Response** (`204 No Content`)**

---

## Authorization Models API

### Write Authorization Model

Create a new authorization model.

**HTTP**:
```http
POST /stores/{store_id}/authorization-models
Content-Type: application/json

{
  "schema_version": "1.1",
  "type_definitions": [
    {
      "type": "user"
    },
    {
      "type": "document",
      "relations": {
        "reader": {
          "this": {}
        },
        "writer": {
          "this": {}
        },
        "owner": {
          "this": {}
        }
      },
      "metadata": {
        "relations": {
          "reader": {
            "directly_related_user_types": [
              { "type": "user" }
            ]
          },
          "writer": {
            "directly_related_user_types": [
              { "type": "user" }
            ]
          },
          "owner": {
            "directly_related_user_types": [
              { "type": "user" }
            ]
          }
        }
      }
    }
  ]
}
```

**Response** (`201 Created`):
```json
{
  "authorization_model_id": "01HQWV7Y8Z9X6W5V4U3T2S1R0Q"
}
```

---

### Read Authorization Model

Retrieve a specific authorization model.

**HTTP**:
```http
GET /stores/{store_id}/authorization-models/{model_id}
```

**Response** (`200 OK`):
```json
{
  "authorization_model": {
    "id": "01HQWV7Y8Z9X6W5V4U3T2S1R0Q",
    "schema_version": "1.1",
    "type_definitions": [
      // ... type definitions
    ]
  }
}
```

---

### Read Latest Authorization Model

Get the most recent authorization model.

**HTTP**:
```http
GET /stores/{store_id}/authorization-models/latest
```

---

### List Authorization Models

List all models with pagination.

**HTTP**:
```http
GET /stores/{store_id}/authorization-models?page_size=10&continuation_token=<token>
```

**Response** (`200 OK`):
```json
{
  "authorization_models": [
    {
      "id": "01HQWV7Y8Z9X6W5V4U3T2S1R0Q",
      "schema_version": "1.1",
      "type_definitions": [...]
    }
  ],
  "continuation_token": "..."
}
```

---

## Relationship Tuples API

### Write Tuples

Write (add/delete) relationship tuples.

**HTTP**:
```http
POST /stores/{store_id}/write
Content-Type: application/json

{
  "writes": {
    "tuple_keys": [
      {
        "user": "user:alice",
        "relation": "reader",
        "object": "document:readme"
      },
      {
        "user": "user:bob",
        "relation": "writer",
        "object": "document:readme"
      }
    ]
  },
  "deletes": {
    "tuple_keys": [
      {
        "user": "user:charlie",
        "relation": "reader",
        "object": "document:readme"
      }
    ]
  },
  "authorization_model_id": "01HQWV7Y8Z9X6W5V4U3T2S1R0Q"
}
```

**Response** (`200 OK`):
```json
{}
```

**Notes**:
- Writes and deletes are applied atomically
- `authorization_model_id` is optional (uses latest if omitted)
- Duplicate tuples are ignored (idempotent)

---

### Read Tuples

Read relationship tuples with filtering.

**HTTP**:
```http
POST /stores/{store_id}/read
Content-Type: application/json

{
  "tuple_key": {
    "user": "user:alice",
    "relation": "reader",
    "object": "document:readme"
  },
  "page_size": 10,
  "continuation_token": "..."
}
```

**Response** (`200 OK`):
```json
{
  "tuples": [
    {
      "key": {
        "user": "user:alice",
        "relation": "reader",
        "object": "document:readme"
      },
      "timestamp": "2024-01-15T10:30:00Z"
    }
  ],
  "continuation_token": "..."
}
```

**Filtering**:
- Omit fields to use as wildcards
- Example: `{"object": "document:readme"}` returns all tuples for that document

---

### Read Changes

Stream changes to tuples (change data capture).

**HTTP**:
```http
GET /stores/{store_id}/changes?type=document&page_size=10&continuation_token=...
```

**Response** (`200 OK`):
```json
{
  "changes": [
    {
      "tuple_key": {
        "user": "user:alice",
        "relation": "reader",
        "object": "document:readme"
      },
      "operation": "TUPLE_OPERATION_WRITE",
      "timestamp": "2024-01-15T10:30:00Z"
    },
    {
      "tuple_key": {
        "user": "user:bob",
        "relation": "reader",
        "object": "document:readme"
      },
      "operation": "TUPLE_OPERATION_DELETE",
      "timestamp": "2024-01-15T10:31:00Z"
    }
  ],
  "continuation_token": "..."
}
```

---

## Check API

### Check Permission

Check if a user has a specific relation to an object.

**HTTP**:
```http
POST /stores/{store_id}/check
Content-Type: application/json

{
  "tuple_key": {
    "user": "user:alice",
    "relation": "reader",
    "object": "document:readme"
  },
  "contextual_tuples": {
    "tuple_keys": [
      {
        "user": "user:alice",
        "relation": "member",
        "object": "team:engineering"
      }
    ]
  },
  "authorization_model_id": "01HQWV7Y8Z9X6W5V4U3T2S1R0Q",
  "trace": false
}
```

**Response** (`200 OK`):
```json
{
  "allowed": true,
  "resolution": ""
}
```

**With Trace** (`trace: true`):
```json
{
  "allowed": true,
  "resolution": "document:readme#reader@user:alice was computed via: document:readme#writer->user:alice"
}
```

**gRPC**:
```protobuf
rpc Check(CheckRequest) returns (CheckResponse);

message CheckRequest {
  string store_id = 1;
  TupleKey tuple_key = 2;
  ContextualTupleKeys contextual_tuples = 3;
  string authorization_model_id = 4;
  bool trace = 5;
  google.protobuf.Struct context = 6;  // For CEL conditions
}

message CheckResponse {
  bool allowed = 1;
  string resolution = 2;
}
```

**Request Parameters**:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `tuple_key.user` | string | Yes | User/subject identifier (e.g., `user:alice`, `team:eng#member`) |
| `tuple_key.relation` | string | Yes | Relation to check (e.g., `reader`, `owner`) |
| `tuple_key.object` | string | Yes | Object identifier (e.g., `document:readme`) |
| `contextual_tuples` | array | No | Temporary tuples for this check only |
| `authorization_model_id` | string | No | Model version (uses latest if omitted) |
| `trace` | boolean | No | Include resolution trace in response |
| `context` | object | No | Context for CEL condition evaluation |

**Response Fields**:

| Field | Type | Description |
|-------|------|-------------|
| `allowed` | boolean | `true` if user has the relation, `false` otherwise |
| `resolution` | string | Human-readable explanation (if `trace: true`) |

---

## Batch Check API

### Batch Check Permissions

Check multiple permissions in a single request.

**HTTP**:
```http
POST /stores/{store_id}/batch-check
Content-Type: application/json

{
  "checks": [
    {
      "tuple_key": {
        "user": "user:alice",
        "relation": "reader",
        "object": "document:readme"
      }
    },
    {
      "tuple_key": {
        "user": "user:alice",
        "relation": "writer",
        "object": "document:readme"
      }
    },
    {
      "tuple_key": {
        "user": "user:bob",
        "relation": "reader",
        "object": "document:roadmap"
      }
    }
  ],
  "authorization_model_id": "01HQWV7Y8Z9X6W5V4U3T2S1R0Q",
  "max_parallel_checks": 10
}
```

**Response** (`200 OK`):
```json
{
  "results": [
    {
      "allowed": true,
      "request": {
        "tuple_key": {
          "user": "user:alice",
          "relation": "reader",
          "object": "document:readme"
        }
      }
    },
    {
      "allowed": false,
      "request": {
        "tuple_key": {
          "user": "user:alice",
          "relation": "writer",
          "object": "document:readme"
        }
      }
    },
    {
      "allowed": true,
      "request": {
        "tuple_key": {
          "user": "user:bob",
          "relation": "reader",
          "object": "document:roadmap"
        }
      }
    }
  ]
}
```

**Limits**:
- Max checks per request: 100 (configurable)
- Results returned in same order as requests
- Deduplication applied automatically

---

## Expand API

### Expand Permission

Expand a relation to show all users/usersets that have it.

**HTTP**:
```http
POST /stores/{store_id}/expand
Content-Type: application/json

{
  "tuple_key": {
    "relation": "reader",
    "object": "document:readme"
  },
  "authorization_model_id": "01HQWV7Y8Z9X6W5V4U3T2S1R0Q"
}
```

**Response** (`200 OK`):
```json
{
  "tree": {
    "root": {
      "name": "document:readme#reader",
      "union": {
        "nodes": [
          {
            "name": "document:readme#reader",
            "leaf": {
              "users": {
                "users": ["user:alice", "user:bob"]
              }
            }
          },
          {
            "name": "document:readme#writer",
            "leaf": {
              "computed": {
                "userset": "document:readme#writer"
              }
            }
          }
        ]
      }
    }
  }
}
```

**Use Cases**:
- Debugging authorization rules
- Understanding permission inheritance
- Auditing access

---

## List Objects API

### List Accessible Objects

List all objects a user can access with a specific relation.

**HTTP**:
```http
POST /stores/{store_id}/list-objects
Content-Type: application/json

{
  "user": "user:alice",
  "relation": "reader",
  "type": "document",
  "authorization_model_id": "01HQWV7Y8Z9X6W5V4U3T2S1R0Q",
  "contextual_tuples": {
    "tuple_keys": []
  }
}
```

**Response** (`200 OK`):
```json
{
  "objects": [
    "document:readme",
    "document:roadmap",
    "document:architecture"
  ]
}
```

**Notes**:
- Returns object IDs only (not full objects)
- Limited to 1000 objects by default
- For large result sets, use pagination (future enhancement)

---

## List Users API

### List Users with Permission

List all users with a specific relation to an object.

**HTTP**:
```http
POST /stores/{store_id}/list-users
Content-Type: application/json

{
  "object": {
    "type": "document",
    "id": "readme"
  },
  "relation": "reader",
  "user_filters": [
    {
      "type": "user"
    }
  ],
  "authorization_model_id": "01HQWV7Y8Z9X6W5V4U3T2S1R0Q",
  "contextual_tuples": {
    "tuple_keys": []
  }
}
```

**Response** (`200 OK`):
```json
{
  "users": [
    {
      "object": {
        "type": "user",
        "id": "alice"
      }
    },
    {
      "object": {
        "type": "user",
        "id": "bob"
      }
    },
    {
      "object": {
        "type": "team",
        "id": "engineering",
        "relation": "member"
      }
    }
  ]
}
```

---

## Error Responses

### Error Format

All errors follow a consistent format:

```json
{
  "code": "invalid_request",
  "message": "The request is missing required field 'user'",
  "details": {
    "field": "user",
    "reason": "required"
  }
}
```

### HTTP Status Codes

| Code | Meaning | Example |
|------|---------|---------|
| `200` | Success | Check returned result |
| `201` | Created | Store/model created |
| `204` | No Content | Store deleted |
| `400` | Bad Request | Invalid request body |
| `401` | Unauthorized | Missing/invalid auth token |
| `403` | Forbidden | Insufficient permissions |
| `404` | Not Found | Store/model doesn't exist |
| `409` | Conflict | Model validation failed |
| `422` | Unprocessable Entity | Tuple validation failed |
| `429` | Too Many Requests | Rate limit exceeded |
| `500` | Internal Server Error | Server error |
| `503` | Service Unavailable | Database unavailable |

### Error Codes

| Code | HTTP Status | Description |
|------|-------------|-------------|
| `invalid_request` | 400 | Malformed request |
| `unauthorized` | 401 | Authentication failed |
| `forbidden` | 403 | Authorization failed |
| `not_found` | 404 | Resource not found |
| `model_validation_failed` | 409 | Invalid authorization model |
| `tuple_validation_failed` | 422 | Tuple doesn't match model |
| `rate_limit_exceeded` | 429 | Too many requests |
| `internal_error` | 500 | Server error |
| `database_error` | 500 | Database error |
| `timeout` | 504 | Request timeout |

### Example Errors

**Missing Required Field**:
```json
{
  "code": "invalid_request",
  "message": "Missing required field 'tuple_key.user'",
  "details": {
    "field": "tuple_key.user"
  }
}
```

**Tuple Validation Failed**:
```json
{
  "code": "tuple_validation_failed",
  "message": "User type 'group' is not allowed for relation 'reader' on type 'document'",
  "details": {
    "user_type": "group",
    "relation": "reader",
    "object_type": "document",
    "allowed_user_types": ["user", "team#member"]
  }
}
```

**Rate Limit Exceeded**:
```json
{
  "code": "rate_limit_exceeded",
  "message": "Rate limit of 100 requests per second exceeded",
  "details": {
    "limit": 100,
    "window": "1s",
    "retry_after": 0.5
  }
}
```

---

## Rate Limiting

### Rate Limit Headers

All responses include rate limit information:

```http
X-RateLimit-Limit: 100
X-RateLimit-Remaining: 42
X-RateLimit-Reset: 1705315800
```

### Default Limits

| Endpoint | Limit | Window |
|----------|-------|--------|
| Check | 1000 req/s | Per store |
| Batch Check | 100 req/s | Per store |
| Write | 100 req/s | Per store |
| Read | 500 req/s | Per store |
| List Objects | 100 req/s | Per store |

**Note**: Limits are configurable per deployment.

---

## Pagination

### Continuation Tokens

Endpoints that return lists use continuation tokens for pagination:

**Request**:
```http
GET /stores?page_size=10&continuation_token=eyJsYXN0X2lkIjoiMDFIUSJ9
```

**Response**:
```json
{
  "stores": [...],
  "continuation_token": "eyJsYXN0X2lkIjoiMDFIUldWN1k4WjlYIn0="
}
```

**Notes**:
- Tokens are opaque (base64-encoded)
- Tokens expire after 24 hours
- Empty token means no more results

---

## Consistency Modes

### Strong Consistency

Bypass cache, always query database:

```http
POST /stores/{store_id}/check
X-Consistency: strong

{
  "tuple_key": {...}
}
```

**Use Cases**:
- Security-critical checks
- After recent writes
- Debugging

**Performance Impact**: 2-5x slower than cached checks

### Eventual Consistency (Default)

Use cache, may return stale results:

```http
POST /stores/{store_id}/check

{
  "tuple_key": {...}
}
```

**Staleness Window**: 1-10 seconds (configurable)

---

## gRPC Service Definition

### Proto File

```protobuf
syntax = "proto3";

package openfga.v1;

service OpenFGAService {
  // Stores
  rpc CreateStore(CreateStoreRequest) returns (CreateStoreResponse);
  rpc GetStore(GetStoreRequest) returns (GetStoreResponse);
  rpc ListStores(ListStoresRequest) returns (ListStoresResponse);
  rpc DeleteStore(DeleteStoreRequest) returns (DeleteStoreResponse);

  // Authorization Models
  rpc WriteAuthorizationModel(WriteAuthorizationModelRequest)
      returns (WriteAuthorizationModelResponse);
  rpc ReadAuthorizationModel(ReadAuthorizationModelRequest)
      returns (ReadAuthorizationModelResponse);
  rpc ReadAuthorizationModels(ReadAuthorizationModelsRequest)
      returns (ReadAuthorizationModelsResponse);

  // Tuples
  rpc Write(WriteRequest) returns (WriteResponse);
  rpc Read(ReadRequest) returns (ReadResponse);
  rpc ReadChanges(ReadChangesRequest) returns (ReadChangesResponse);

  // Queries
  rpc Check(CheckRequest) returns (CheckResponse);
  rpc BatchCheck(BatchCheckRequest) returns (BatchCheckResponse);
  rpc Expand(ExpandRequest) returns (ExpandResponse);
  rpc ListObjects(ListObjectsRequest) returns (ListObjectsResponse);
  rpc ListUsers(ListUsersRequest) returns (ListUsersResponse);
}
```

---

## API Versioning

### Version Header

Specify API version via header:

```http
X-OpenFGA-API-Version: 1.0
```

**Default**: Latest stable version (1.0)

### Version Compatibility

- **v1.0**: OpenFGA compatible, stable
- **v1.1**: Future enhancements (backward compatible)
- **v2.0**: Breaking changes (future)

---

## Best Practices

### 1. Use Batch Check for Multiple Checks

**Bad** (N requests):
```javascript
for (const doc of documents) {
  const allowed = await client.check('user:alice', 'reader', doc);
}
```

**Good** (1 request):
```javascript
const results = await client.batchCheck(
  documents.map(doc => ({
    user: 'user:alice',
    relation: 'reader',
    object: doc
  }))
);
```

### 2. Specify Authorization Model ID

**Bad** (uses latest, may change):
```json
{
  "tuple_key": {...}
}
```

**Good** (explicit version):
```json
{
  "tuple_key": {...},
  "authorization_model_id": "01HQWV7Y8Z9X6W5V4U3T2S1R0Q"
}
```

### 3. Use Contextual Tuples for Temporary Access

**Example**: Grant temporary access without writing to DB:
```json
{
  "tuple_key": {
    "user": "user:alice",
    "relation": "reader",
    "object": "document:secret"
  },
  "contextual_tuples": {
    "tuple_keys": [
      {
        "user": "user:alice",
        "relation": "temp_access",
        "object": "document:secret"
      }
    ]
  }
}
```

### 4. Handle Rate Limits Gracefully

```javascript
async function checkWithRetry(request) {
  try {
    return await client.check(request);
  } catch (error) {
    if (error.code === 'rate_limit_exceeded') {
      const retryAfter = error.details.retry_after * 1000;
      await sleep(retryAfter);
      return checkWithRetry(request);
    }
    throw error;
  }
}
```

---

## Summary

RSFGA provides a complete, OpenFGA-compatible API with:

- ✅ **HTTP and gRPC** support
- ✅ **Full CRUD** for stores, models, and tuples
- ✅ **Authorization queries**: Check, Batch Check, Expand, List Objects/Users
- ✅ **Consistency modes**: Strong and eventual
- ✅ **Rate limiting** and pagination
- ✅ **Comprehensive error handling**

**Next**: See [DATA_MODELS.md](./DATA_MODELS.md) for detailed data structure specifications.
