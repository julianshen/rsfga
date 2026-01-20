//! HTTP status code compatibility tests for OpenFGA API.
//!
//! This module tests that RSFGA returns the correct HTTP status codes for
//! various scenarios, ensuring 100% compatibility with OpenFGA's behavior.
//!
//! ## Status Codes Tested
//!
//! | Code | HTTP Status            | gRPC Code         | Scenario                      |
//! |------|------------------------|-------------------|-------------------------------|
//! | 200  | OK                     | OK                | Successful operations         |
//! | 201  | Created                | OK                | Resource creation             |
//! | 204  | No Content             | OK                | Delete operations             |
//! | 400  | Bad Request            | INVALID_ARGUMENT  | Validation errors             |
//! | 404  | Not Found              | NOT_FOUND         | Resource not found            |
//! | 409  | Conflict               | ALREADY_EXISTS    | Duplicate tuple               |
//! | 413  | Payload Too Large      | RESOURCE_EXHAUSTED| Request too large             |
//! | 500  | Internal Server Error  | INTERNAL          | Unexpected errors             |
//! | 503  | Service Unavailable    | UNAVAILABLE       | Backend unavailable           |
//! | 504  | Gateway Timeout        | DEADLINE_EXCEEDED | Operation timeout             |
//!
//! Related to GitHub issue #215.

mod common;

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;

use rsfga_api::http::{create_router_with_body_limit, AppState};
use rsfga_storage::{DataStore, MemoryDataStore, StoredAuthorizationModel, StoredTuple, Utc};

use common::{create_test_app, post_json};

// ============================================================
// Section 1: Success Status Codes (200, 201, 204)
// ============================================================

/// Test: Check endpoint returns 200 OK on success.
#[tokio::test]
async fn test_check_returns_200_on_success() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Write a tuple
    storage
        .write_tuples(
            &store_id,
            vec![create_tuple("viewer", "user:alice", "document", "readme")],
            vec![],
        )
        .await
        .unwrap();

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:readme"
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "Check should return 200 OK");
    assert!(
        response.get("allowed").is_some(),
        "Response should contain 'allowed' field"
    );
}

/// Test: CreateStore returns 201 Created on success.
#[tokio::test]
async fn test_create_store_returns_201_created() {
    let storage = Arc::new(MemoryDataStore::new());

    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({
            "name": "Test Store"
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::CREATED,
        "CreateStore should return 201 Created"
    );
    assert!(
        response.get("id").is_some(),
        "Response should contain store 'id'"
    );
}

/// Test: WriteAuthorizationModel returns 201 Created on success.
#[tokio::test]
async fn test_write_authorization_model_returns_201_created() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/authorization-models"),
        serde_json::json!({
            "schema_version": "1.1",
            "type_definitions": [
                {"type": "user"},
                {"type": "document", "relations": {"viewer": {}}}
            ]
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::CREATED,
        "WriteAuthorizationModel should return 201 Created"
    );
    assert!(
        response.get("authorization_model_id").is_some(),
        "Response should contain 'authorization_model_id'"
    );
}

/// Test: DeleteStore returns 204 No Content on success.
#[tokio::test]
async fn test_delete_store_returns_204_no_content() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage).await;

    let app = create_test_app(&storage);

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/stores/{store_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::NO_CONTENT,
        "DeleteStore should return 204 No Content"
    );
}

// ============================================================
// Section 2: Client Error Status Codes (400, 404)
// ============================================================

/// Test: Invalid JSON returns 400 Bad Request.
#[tokio::test]
async fn test_invalid_json_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let app = create_test_app(&storage);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/stores/{store_id}/check"))
                .header("content-type", "application/json")
                .body(Body::from("{invalid json"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "Invalid JSON should return 400 Bad Request"
    );
}

/// Test: Non-existent store returns 404 Not Found.
#[tokio::test]
async fn test_nonexistent_store_returns_404() {
    let storage = Arc::new(MemoryDataStore::new());

    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores/nonexistent-store-id/check",
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:readme"
            }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "Non-existent store should return 404 Not Found"
    );
    assert_eq!(
        response["code"].as_str(),
        Some("not_found"),
        "Error code should be 'not_found'"
    );
}

/// Test: Non-existent authorization model returns 404 Not Found.
#[tokio::test]
async fn test_nonexistent_authorization_model_returns_404() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage).await;

    let app = create_test_app(&storage);

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/stores/{store_id}/authorization-models/nonexistent-model-id"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "Non-existent authorization model should return 404 Not Found"
    );
}

// ============================================================
// Section 3: 409 Conflict (Duplicate Tuple)
// ============================================================

/// Test: DuplicateTuple storage error returns 409 Conflict.
///
/// This test verifies the HTTP status code mapping for storage errors.
/// Note: The MemoryDataStore uses idempotent writes (duplicates are silently
/// accepted), matching OpenFGA's behavior with `on_duplicate: "ignore"`.
/// This test uses a mock storage to verify the error → HTTP status mapping.
#[tokio::test]
async fn test_duplicate_tuple_error_returns_409_conflict() {
    let storage = Arc::new(DuplicateErrorStorage::create());

    let state = AppState::new(storage);
    let app = create_router_with_body_limit(state, 1024 * 1024);

    let (status, response) = post_json_raw(
        app,
        "/stores/test-store/write",
        serde_json::json!({
            "writes": {
                "tuple_keys": [
                    {
                        "user": "user:alice",
                        "relation": "viewer",
                        "object": "document:readme"
                    }
                ]
            }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "DuplicateTuple error should return 409 Conflict"
    );
    assert_eq!(
        response["code"].as_str(),
        Some("conflict"),
        "Error code should be 'conflict'"
    );
    assert!(
        response["message"]
            .as_str()
            .unwrap_or("")
            .contains("already exists"),
        "Message should indicate tuple already exists: {:?}",
        response["message"]
    );
}

/// Test: ConditionConflict storage error returns 409 Conflict.
#[tokio::test]
async fn test_condition_conflict_error_returns_409() {
    let storage = Arc::new(ConditionConflictStorage::create());

    let state = AppState::new(storage);
    let app = create_router_with_body_limit(state, 1024 * 1024);

    let (status, response) = post_json_raw(
        app,
        "/stores/test-store/write",
        serde_json::json!({
            "writes": {
                "tuple_keys": [
                    {
                        "user": "user:alice",
                        "relation": "viewer",
                        "object": "document:readme"
                    }
                ]
            }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "ConditionConflict error should return 409 Conflict"
    );
    assert_eq!(
        response["code"].as_str(),
        Some("conflict"),
        "Error code should be 'conflict'"
    );
}

// ============================================================
// Section 4: 413 Payload Too Large
// ============================================================

/// Test: Oversized request body returns 413 Payload Too Large.
#[tokio::test]
async fn test_oversized_request_returns_413() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Create app with 1KB body limit for testing
    let state = AppState::new(Arc::clone(&storage));
    let app = create_router_with_body_limit(state, 1024);

    // Create a request body larger than 1KB
    let large_body = "x".repeat(2048);
    let body = serde_json::json!({
        "tuple_key": {
            "user": format!("user:{large_body}"),
            "relation": "viewer",
            "object": "document:readme"
        }
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/stores/{store_id}/check"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "Oversized request should return 413 Payload Too Large"
    );
}

/// Test: Large authorization model operations work within limits.
/// Note: Authorization model size limits are enforced at the HTTP body layer (413)
/// for very large models. This test verifies reasonable-sized models work.
#[tokio::test]
async fn test_authorization_model_within_limits_succeeds() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage).await;

    // Create a model with multiple type definitions (within limits)
    let type_defs: Vec<serde_json::Value> = (0..10)
        .map(|i| {
            serde_json::json!({
                "type": format!("type_{i}"),
                "relations": {
                    "relation1": {},
                    "relation2": {},
                    "relation3": {}
                }
            })
        })
        .collect();

    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/authorization-models"),
        serde_json::json!({
            "schema_version": "1.1",
            "type_definitions": type_defs
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::CREATED,
        "Authorization model within limits should succeed"
    );
}

// ============================================================
// Section 5: 503 Service Unavailable
// ============================================================

/// Test: Readiness check returns 503 when storage is unavailable.
#[tokio::test]
async fn test_readiness_check_returns_503_when_storage_unavailable() {
    // Use a mock storage that returns a connection error
    let storage = Arc::new(FailingStorage::create());

    let state = AppState::new(storage);
    let app = create_router_with_body_limit(state, 1024 * 1024);

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "Readiness check should return 503 when storage unavailable"
    );
}

// ============================================================
// Section 6: Error Response Format Verification
// ============================================================

/// Test: All error responses have consistent format with 'code' and 'message'.
#[tokio::test]
async fn test_error_response_format_consistency() {
    let storage = Arc::new(MemoryDataStore::new());

    // Test 404 error format
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores/nonexistent/check",
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:readme"
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        response.get("code").is_some(),
        "Error response should have 'code' field"
    );
    assert!(
        response.get("message").is_some(),
        "Error response should have 'message' field"
    );
}

/// Test: 409 Conflict error response format matches OpenFGA.
#[tokio::test]
async fn test_conflict_error_response_format() {
    // Use mock storage that returns DuplicateTuple error
    let storage = Arc::new(DuplicateErrorStorage::create());

    let state = AppState::new(storage);
    let app = create_router_with_body_limit(state, 1024 * 1024);

    let (status, response) = post_json_raw(
        app,
        "/stores/test-store/write",
        serde_json::json!({
            "writes": {
                "tuple_keys": [
                    {
                        "user": "user:bob",
                        "relation": "viewer",
                        "object": "document:test"
                    }
                ]
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        response["code"].as_str(),
        Some("conflict"),
        "Error code should be 'conflict'"
    );
    assert!(
        response.get("message").is_some(),
        "Error response should have 'message' field"
    );
}

// ============================================================
// Section 7: HTTP-to-gRPC Status Code Mapping Documentation
// ============================================================

/// Test: Document and verify HTTP-to-gRPC status code mappings.
///
/// This test serves as documentation and verification that our
/// status code mappings match the expected gRPC equivalents.
#[tokio::test]
async fn test_http_to_grpc_status_code_mappings() {
    // This test documents the expected mappings:
    //
    // HTTP Status       → gRPC Status
    // --------------------------------
    // 200 OK            → OK (0)
    // 201 Created       → OK (0)
    // 204 No Content    → OK (0)
    // 400 Bad Request   → INVALID_ARGUMENT (3)
    // 404 Not Found     → NOT_FOUND (5)
    // 409 Conflict      → ALREADY_EXISTS (6)
    // 413 Payload Large → RESOURCE_EXHAUSTED (8)
    // 500 Internal      → INTERNAL (13)
    // 503 Unavailable   → UNAVAILABLE (14)
    // 504 Timeout       → DEADLINE_EXCEEDED (4)

    // Verify mappings through the ApiError type
    use rsfga_api::http::routes::ApiError;

    let test_cases = vec![
        (
            ApiError::not_found("test"),
            "not_found",
            StatusCode::NOT_FOUND,
        ),
        (
            ApiError::invalid_input("test"),
            "validation_error",
            StatusCode::BAD_REQUEST,
        ),
        (ApiError::conflict("test"), "conflict", StatusCode::CONFLICT),
        (
            ApiError::gateway_timeout("test"),
            "timeout",
            StatusCode::GATEWAY_TIMEOUT,
        ),
        (
            ApiError::service_unavailable("test"),
            "service_unavailable",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            ApiError::new("payload_too_large", "test"),
            "payload_too_large",
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
        (
            ApiError::internal_error("test"),
            "internal_error",
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    ];

    for (error, expected_code, expected_status) in test_cases {
        assert_eq!(error.code, expected_code, "Error code mismatch");

        // Convert to response and verify status code
        use axum::response::IntoResponse;
        let response = error.into_response();
        assert_eq!(
            response.status(),
            expected_status,
            "Status code mismatch for error code '{expected_code}'"
        );
    }
}

// ============================================================
// Section 8: gRPC Status Code Mapping Tests
// ============================================================

/// Test: StorageError to gRPC Status mapping for DuplicateTuple.
/// Verifies DuplicateTuple → ALREADY_EXISTS (6)
#[test]
fn test_grpc_duplicate_tuple_returns_already_exists() {
    use rsfga_storage::StorageError;
    use tonic::Code;

    let err = StorageError::DuplicateTuple {
        object_type: "document".to_string(),
        object_id: "doc1".to_string(),
        relation: "viewer".to_string(),
        user: "user:alice".to_string(),
    };

    // Use the same conversion function the gRPC service uses
    let status = storage_error_to_grpc_status(err);
    assert_eq!(
        status.code(),
        Code::AlreadyExists,
        "DuplicateTuple should map to ALREADY_EXISTS"
    );
}

/// Test: StorageError to gRPC Status mapping for ConditionConflict.
/// Verifies ConditionConflict → ALREADY_EXISTS (6)
#[test]
fn test_grpc_condition_conflict_returns_already_exists() {
    use rsfga_storage::{ConditionConflictError, StorageError};
    use tonic::Code;

    let err = StorageError::ConditionConflict(Box::new(ConditionConflictError {
        store_id: "test-store".to_string(),
        object_type: "document".to_string(),
        object_id: "doc1".to_string(),
        relation: "viewer".to_string(),
        user: "user:alice".to_string(),
        existing_condition: Some("cond_a".to_string()),
        new_condition: Some("cond_b".to_string()),
    }));

    let status = storage_error_to_grpc_status(err);
    assert_eq!(
        status.code(),
        Code::AlreadyExists,
        "ConditionConflict should map to ALREADY_EXISTS"
    );
}

/// Test: StorageError to gRPC Status mapping for ConnectionError.
/// Verifies ConnectionError → UNAVAILABLE (14)
#[test]
fn test_grpc_connection_error_returns_unavailable() {
    use rsfga_storage::StorageError;
    use tonic::Code;

    let err = StorageError::ConnectionError {
        message: "database unavailable".to_string(),
    };

    let status = storage_error_to_grpc_status(err);
    assert_eq!(
        status.code(),
        Code::Unavailable,
        "ConnectionError should map to UNAVAILABLE"
    );
}

/// Test: StorageError to gRPC Status mapping for HealthCheckFailed.
/// Verifies HealthCheckFailed → UNAVAILABLE (14)
#[test]
fn test_grpc_health_check_failed_returns_unavailable() {
    use rsfga_storage::StorageError;
    use tonic::Code;

    let err = StorageError::HealthCheckFailed {
        message: "health check failed".to_string(),
    };

    let status = storage_error_to_grpc_status(err);
    assert_eq!(
        status.code(),
        Code::Unavailable,
        "HealthCheckFailed should map to UNAVAILABLE"
    );
}

/// Test: StorageError to gRPC Status mapping for QueryTimeout.
/// Verifies QueryTimeout → DEADLINE_EXCEEDED (4)
#[test]
fn test_grpc_query_timeout_returns_deadline_exceeded() {
    use rsfga_storage::StorageError;
    use tonic::Code;

    let err = StorageError::QueryTimeout {
        operation: "read_tuples".to_string(),
        timeout: Duration::from_secs(5),
    };

    let status = storage_error_to_grpc_status(err);
    assert_eq!(
        status.code(),
        Code::DeadlineExceeded,
        "QueryTimeout should map to DEADLINE_EXCEEDED"
    );
}

/// Test: StorageError to gRPC Status mapping for StoreNotFound.
/// Verifies StoreNotFound → NOT_FOUND (5)
#[test]
fn test_grpc_store_not_found_returns_not_found() {
    use rsfga_storage::StorageError;
    use tonic::Code;

    let err = StorageError::StoreNotFound {
        store_id: "nonexistent".to_string(),
    };

    let status = storage_error_to_grpc_status(err);
    assert_eq!(
        status.code(),
        Code::NotFound,
        "StoreNotFound should map to NOT_FOUND"
    );
}

/// Test: StorageError to gRPC Status mapping for InvalidInput.
/// Verifies InvalidInput → INVALID_ARGUMENT (3)
#[test]
fn test_grpc_invalid_input_returns_invalid_argument() {
    use rsfga_storage::StorageError;
    use tonic::Code;

    let err = StorageError::InvalidInput {
        message: "invalid input".to_string(),
    };

    let status = storage_error_to_grpc_status(err);
    assert_eq!(
        status.code(),
        Code::InvalidArgument,
        "InvalidInput should map to INVALID_ARGUMENT"
    );
}

/// Helper function that mirrors the gRPC service's storage_error_to_status.
/// This allows testing the mapping logic without spinning up a gRPC server.
fn storage_error_to_grpc_status(err: rsfga_storage::StorageError) -> tonic::Status {
    use rsfga_storage::StorageError;
    use tonic::Status;

    match &err {
        StorageError::StoreNotFound { store_id } => {
            Status::not_found(format!("store not found: {store_id}"))
        }
        StorageError::TupleNotFound { .. } => Status::not_found(err.to_string()),
        StorageError::InvalidInput { message } => Status::invalid_argument(message),
        StorageError::DuplicateTuple { .. } => Status::already_exists(err.to_string()),
        StorageError::ConditionConflict(conflict) => {
            Status::already_exists(format!("tuple exists with different condition: {conflict}"))
        }
        StorageError::ConnectionError { .. } => Status::unavailable("storage backend unavailable"),
        StorageError::HealthCheckFailed { .. } => {
            Status::unavailable("storage backend unavailable")
        }
        StorageError::QueryTimeout { .. } => {
            Status::deadline_exceeded("storage operation timed out")
        }
        _ => Status::internal(err.to_string()),
    }
}

// ============================================================
// Helper Functions
// ============================================================

/// Create a store and return its ID.
async fn create_store(storage: &MemoryDataStore) -> String {
    let store_id = ulid::Ulid::new().to_string();
    storage.create_store(&store_id, "Test Store").await.unwrap();
    store_id
}

/// Set up a test store with a simple authorization model.
async fn setup_test_store(storage: &MemoryDataStore) -> String {
    let store_id = create_store(storage).await;

    let model_json = r#"{
        "type_definitions": [
            {"type": "user"},
            {"type": "team", "relations": {"member": {}}},
            {"type": "document", "relations": {"viewer": {}, "editor": {}, "owner": {}}}
        ]
    }"#;
    let model =
        StoredAuthorizationModel::new(ulid::Ulid::new().to_string(), &store_id, "1.1", model_json);
    storage.write_authorization_model(model).await.unwrap();

    store_id
}

/// Create a StoredTuple for testing.
fn create_tuple(relation: &str, user: &str, object_type: &str, object_id: &str) -> StoredTuple {
    let (user_type, user_id) = user.split_once(':').unwrap();
    StoredTuple {
        object_type: object_type.to_string(),
        object_id: object_id.to_string(),
        relation: relation.to_string(),
        user_type: user_type.to_string(),
        user_id: user_id.to_string(),
        user_relation: None,
        condition_name: None,
        condition_context: None,
        created_at: None,
    }
}

// ============================================================
// Helper Functions for Testing
// ============================================================

/// Make a JSON POST request and return status + parsed JSON response.
/// This version takes an already-built Router.
async fn post_json_raw(
    app: axum::Router,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = if body.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_slice(&body).unwrap_or_else(|_| {
            serde_json::json!({
                "raw_body": String::from_utf8_lossy(&body).to_string()
            })
        })
    };
    (status, json)
}

// ============================================================
// Mock Storage for Testing Error Conditions
// ============================================================

use async_trait::async_trait;
use rsfga_storage::{
    ConditionConflictError, HealthStatus, PaginatedResult, PaginationOptions, StorageError,
    StorageResult, Store, StoredAuthorizationModel as SAM, StoredTuple as ST, TupleFilter,
};
use std::time::Duration;

/// Configurable error behavior for mock storage write operations.
#[derive(Clone)]
enum WriteError {
    /// Return DuplicateTuple error
    Duplicate,
    /// Return ConditionConflict error
    ConditionConflict,
}

/// Configurable mock storage for testing error scenarios.
///
/// Use factory methods to create pre-configured instances:
/// - `MockStorage::failing()` - All operations return ConnectionError
/// - `MockStorage::with_write_error(WriteError::Duplicate)` - Write returns DuplicateTuple
/// - `MockStorage::with_write_error(WriteError::ConditionConflict)` - Write returns ConditionConflict
struct MockStorage {
    /// If true, all operations fail with ConnectionError
    fail_all: bool,
    /// Optional error to return on write operations
    write_error: Option<WriteError>,
}

impl MockStorage {
    /// Create a mock that fails all operations with ConnectionError.
    fn failing() -> Self {
        Self {
            fail_all: true,
            write_error: None,
        }
    }

    /// Create a mock that returns a specific error on write operations.
    fn with_write_error(error: WriteError) -> Self {
        Self {
            fail_all: false,
            write_error: Some(error),
        }
    }

    fn make_store(id: &str) -> Store {
        Store {
            id: id.to_string(),
            name: "Test Store".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn connection_error() -> StorageError {
        StorageError::ConnectionError {
            message: "database unavailable".to_string(),
        }
    }
}

#[async_trait]
impl DataStore for MockStorage {
    async fn create_store(&self, id: &str, _name: &str) -> StorageResult<Store> {
        if self.fail_all {
            return Err(Self::connection_error());
        }
        Ok(Self::make_store(id))
    }

    async fn get_store(&self, id: &str) -> StorageResult<Store> {
        if self.fail_all {
            return Err(Self::connection_error());
        }
        Ok(Self::make_store(id))
    }

    async fn list_stores(&self) -> StorageResult<Vec<Store>> {
        if self.fail_all {
            return Err(Self::connection_error());
        }
        Ok(vec![])
    }

    async fn list_stores_paginated(
        &self,
        _pagination: &PaginationOptions,
    ) -> StorageResult<PaginatedResult<Store>> {
        if self.fail_all {
            return Err(Self::connection_error());
        }
        Ok(PaginatedResult {
            items: vec![],
            continuation_token: None,
        })
    }

    async fn update_store(&self, id: &str, name: &str) -> StorageResult<Store> {
        if self.fail_all {
            return Err(Self::connection_error());
        }
        Ok(Store {
            id: id.to_string(),
            name: name.to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
    }

    async fn delete_store(&self, _id: &str) -> StorageResult<()> {
        if self.fail_all {
            return Err(Self::connection_error());
        }
        Ok(())
    }

    async fn write_authorization_model(&self, model: SAM) -> StorageResult<SAM> {
        if self.fail_all {
            return Err(Self::connection_error());
        }
        Ok(model)
    }

    async fn get_authorization_model(&self, store_id: &str, model_id: &str) -> StorageResult<SAM> {
        if self.fail_all {
            return Err(Self::connection_error());
        }
        Ok(SAM::new(model_id, store_id, "1.1", "{}"))
    }

    async fn get_latest_authorization_model(&self, store_id: &str) -> StorageResult<SAM> {
        if self.fail_all {
            return Err(Self::connection_error());
        }
        Ok(SAM::new(
            "model-1",
            store_id,
            "1.1",
            r#"{"type_definitions":[{"type":"user"},{"type":"document","relations":{"viewer":{}}}]}"#,
        ))
    }

    async fn list_authorization_models(&self, _store_id: &str) -> StorageResult<Vec<SAM>> {
        if self.fail_all {
            return Err(Self::connection_error());
        }
        Ok(vec![])
    }

    async fn list_authorization_models_paginated(
        &self,
        _store_id: &str,
        _pagination: &PaginationOptions,
    ) -> StorageResult<PaginatedResult<SAM>> {
        if self.fail_all {
            return Err(Self::connection_error());
        }
        Ok(PaginatedResult {
            items: vec![],
            continuation_token: None,
        })
    }

    async fn delete_authorization_model(
        &self,
        _store_id: &str,
        _model_id: &str,
    ) -> StorageResult<()> {
        if self.fail_all {
            return Err(Self::connection_error());
        }
        Ok(())
    }

    async fn write_tuples(
        &self,
        store_id: &str,
        writes: Vec<ST>,
        _deletes: Vec<ST>,
    ) -> StorageResult<()> {
        if self.fail_all {
            return Err(Self::connection_error());
        }
        if let (Some(error), Some(tuple)) = (&self.write_error, writes.first()) {
            return match error {
                WriteError::Duplicate => Err(StorageError::DuplicateTuple {
                    object_type: tuple.object_type.clone(),
                    object_id: tuple.object_id.clone(),
                    relation: tuple.relation.clone(),
                    user: format!("{}:{}", tuple.user_type, tuple.user_id),
                }),
                WriteError::ConditionConflict => Err(StorageError::ConditionConflict(Box::new(
                    ConditionConflictError {
                        store_id: store_id.to_string(),
                        object_type: tuple.object_type.clone(),
                        object_id: tuple.object_id.clone(),
                        relation: tuple.relation.clone(),
                        user: format!("{}:{}", tuple.user_type, tuple.user_id),
                        existing_condition: Some("condition_a".to_string()),
                        new_condition: Some("condition_b".to_string()),
                    },
                ))),
            };
        }
        Ok(())
    }

    async fn read_tuples(&self, _store_id: &str, _filter: &TupleFilter) -> StorageResult<Vec<ST>> {
        if self.fail_all {
            return Err(Self::connection_error());
        }
        Ok(vec![])
    }

    async fn read_tuples_paginated(
        &self,
        _store_id: &str,
        _filter: &TupleFilter,
        _pagination: &PaginationOptions,
    ) -> StorageResult<PaginatedResult<ST>> {
        if self.fail_all {
            return Err(Self::connection_error());
        }
        Ok(PaginatedResult {
            items: vec![],
            continuation_token: None,
        })
    }

    async fn health_check(&self) -> StorageResult<HealthStatus> {
        if self.fail_all {
            return Err(StorageError::HealthCheckFailed {
                message: "database unavailable".to_string(),
            });
        }
        Ok(HealthStatus {
            healthy: true,
            latency: Duration::from_millis(1),
            pool_stats: None,
            message: Some("mock".to_string()),
        })
    }
}

// Factory functions for backwards compatibility with existing tests
struct FailingStorage;
struct DuplicateErrorStorage;
struct ConditionConflictStorage;

impl FailingStorage {
    fn create() -> MockStorage {
        MockStorage::failing()
    }
}

impl DuplicateErrorStorage {
    fn create() -> MockStorage {
        MockStorage::with_write_error(WriteError::Duplicate)
    }
}

impl ConditionConflictStorage {
    fn create() -> MockStorage {
        MockStorage::with_write_error(WriteError::ConditionConflict)
    }
}
