//! HTTP API tests for Section 2.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt; // for oneshot

use rsfga_storage::{DataStore, MemoryDataStore};

use super::routes::create_router;
use super::state::AppState;

/// Helper to create a test app with in-memory storage.
fn test_app() -> axum::Router {
    let storage = Arc::new(MemoryDataStore::new());
    let state = AppState::new(storage);
    create_router(state)
}

/// Test: Server starts on configured port
///
/// This test verifies the router can be created and responds to health checks.
#[tokio::test]
async fn test_server_starts_on_configured_port() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
}

/// Test: POST /stores/{store_id}/check returns 200
///
/// Verifies check endpoint returns correct status for existing tuples.
#[tokio::test]
async fn test_check_endpoint_returns_200() {
    let storage = Arc::new(MemoryDataStore::new());
    let state = AppState::new(Arc::clone(&storage));
    let app = create_router(state);

    // Create a store
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    // Write a tuple
    storage
        .write_tuple(
            "test-store",
            rsfga_storage::StoredTuple {
                object_type: "document".to_string(),
                object_id: "readme".to_string(),
                relation: "viewer".to_string(),
                user_type: "user".to_string(),
                user_id: "alice".to_string(),
                user_relation: None,
            },
        )
        .await
        .unwrap();

    // Check the tuple
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/test-store/check")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "tuple_key": {
                            "user": "user:alice",
                            "relation": "viewer",
                            "object": "document:readme"
                        }
                    }"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["allowed"], true);
}

/// Test: Check endpoint validates request body
///
/// Verifies check endpoint returns 400 for invalid JSON.
#[tokio::test]
async fn test_check_endpoint_validates_request_body() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    let state = AppState::new(storage);
    let app = create_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/test-store/check")
                .header("content-type", "application/json")
                .body(Body::from(r#"{ "invalid": "body" }"#))
                .unwrap(),
        )
        .await
        .unwrap();

    // Should return 422 (Unprocessable Entity) for invalid schema
    assert!(
        response.status() == StatusCode::UNPROCESSABLE_ENTITY
            || response.status() == StatusCode::BAD_REQUEST
    );
}

/// Test: Check endpoint returns correct response format
///
/// Verifies response includes `allowed` and optionally `resolution`.
#[tokio::test]
async fn test_check_endpoint_returns_correct_response_format() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    let state = AppState::new(storage);
    let app = create_router(state);

    // Check for a non-existent tuple
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/test-store/check")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "tuple_key": {
                            "user": "user:bob",
                            "relation": "viewer",
                            "object": "document:secret"
                        }
                    }"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Must have allowed field
    assert!(json.get("allowed").is_some());
    assert_eq!(json["allowed"], false);
}

/// Test: POST /stores/{store_id}/expand returns 200
#[tokio::test]
async fn test_expand_endpoint_returns_200() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    let state = AppState::new(storage);
    let app = create_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/test-store/expand")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "tuple_key": {
                            "user": "",
                            "relation": "viewer",
                            "object": "document:readme"
                        }
                    }"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

/// Test: POST /stores/{store_id}/write returns 200
#[tokio::test]
async fn test_write_endpoint_returns_200() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    let state = AppState::new(storage);
    let app = create_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/test-store/write")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "writes": {
                            "tuple_keys": [
                                {
                                    "user": "user:alice",
                                    "relation": "viewer",
                                    "object": "document:readme"
                                }
                            ]
                        }
                    }"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

/// Test: POST /stores/{store_id}/read returns 200
#[tokio::test]
async fn test_read_endpoint_returns_200() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    let state = AppState::new(storage);
    let app = create_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/test-store/read")
                .header("content-type", "application/json")
                .body(Body::from(r#"{}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("tuples").is_some());
}

/// Test: Invalid JSON returns 400
#[tokio::test]
async fn test_invalid_json_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    let state = AppState::new(storage);
    let app = create_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/test-store/check")
                .header("content-type", "application/json")
                .body(Body::from("{ invalid json }"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// Test: Non-existent store returns 404
#[tokio::test]
async fn test_nonexistent_store_returns_404() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/nonexistent-store/check")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "tuple_key": {
                            "user": "user:alice",
                            "relation": "viewer",
                            "object": "document:readme"
                        }
                    }"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// Test: Validation errors return 400 with details
///
/// This test verifies that validation error responses include proper error details.
/// We test with a batch check that has too many items, which triggers validation.
#[tokio::test]
async fn test_validation_errors_return_400_with_details() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    let state = AppState::new(storage);
    let app = create_router(state);

    // Test with batch check that has too many items
    let mut checks = Vec::new();
    for i in 0..100 {
        checks.push(format!(
            r#"{{"tuple_key": {{"user": "user:{}", "relation": "viewer", "object": "doc:{}"}}, "correlation_id": "check-{}"}}"#,
            i, i, i
        ));
    }
    let body = format!(r#"{{"checks": [{}]}}"#, checks.join(","));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/test-store/batch-check")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    // Should return 400 for batch size exceeded
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Should have error details
    assert!(json.get("code").is_some());
    assert!(json.get("message").is_some());
}
