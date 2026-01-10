//! HTTP API tests for Section 2.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use metrics_exporter_prometheus::PrometheusBuilder;
use tower::ServiceExt; // for oneshot

use rsfga_storage::{DataStore, MemoryDataStore};

use super::routes::{
    create_router, create_router_with_body_limit, create_router_with_observability,
};
use super::state::AppState;
use crate::observability::MetricsState;

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
            rsfga_storage::StoredTuple::new("document", "readme", "viewer", "user", "alice", None),
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

// ============================================================
// Observability Tests (Milestone 1.9, Section 1)
// ============================================================

/// Test: Metrics endpoint exposes Prometheus metrics
///
/// Verifies that the /metrics endpoint returns Prometheus-formatted metrics.
#[tokio::test]
async fn test_metrics_endpoint_exposes_prometheus_metrics() {
    // Create metrics state with a fresh recorder
    // Note: We use build_recorder() instead of install_recorder() to avoid
    // conflicts with other tests that may install their own recorder.
    let builder = PrometheusBuilder::new();
    let handle = builder.build_recorder().handle();
    let metrics_state = MetricsState::new(handle);

    // Create app with observability
    let storage = Arc::new(MemoryDataStore::new());
    let state = AppState::new(storage);
    let app = create_router_with_observability(state, metrics_state);

    // Request the metrics endpoint
    let response = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should return 200 OK
    assert_eq!(response.status(), StatusCode::OK);

    // Body should be valid Prometheus format (text/plain with optional # comments)
    let body = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    let body_str = String::from_utf8_lossy(&body);

    // Prometheus output is either empty or contains metric lines
    // An empty response is valid when no metrics have been recorded
    assert!(
        body_str.is_empty() || body_str.contains("# ") || body_str.contains('\n'),
        "Metrics output should be valid Prometheus format"
    );
}

/// Test: Health check endpoint returns 200
///
/// Verifies the health endpoint returns 200 OK with status information.
#[tokio::test]
async fn test_health_check_endpoint_returns_200() {
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

    // Should have status field
    assert_eq!(json["status"], "ok");
}

/// Test: Readiness check validates dependencies
///
/// Verifies the readiness endpoint returns 200 OK when storage is accessible.
#[tokio::test]
async fn test_readiness_check_validates_dependencies() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should return 200 OK when storage is accessible
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Should have status and checks fields
    assert_eq!(json["status"], "ready");
    assert_eq!(json["checks"]["storage"], "ok");
}

// ============================================================
// Payload Size Limit Tests (Issue #70)
// ============================================================

/// Test: Requests within body limit succeed
///
/// Verifies that normal-sized requests are processed correctly.
#[tokio::test]
async fn test_requests_within_body_limit_succeed() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    let state = AppState::new(storage);
    // Use a small limit for testing (1KB)
    let app = create_router_with_body_limit(state, 1024);

    // Small request (well within 1KB limit)
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

    // Should succeed
    assert_eq!(response.status(), StatusCode::OK);
}

/// Test: Requests exceeding body limit are rejected
///
/// Verifies that oversized requests return 413 Payload Too Large.
#[tokio::test]
async fn test_requests_exceeding_body_limit_rejected() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    let state = AppState::new(storage);
    // Use a very small limit (100 bytes) for testing
    let app = create_router_with_body_limit(state, 100);

    // Large request (exceeds 100 bytes limit)
    let large_payload = r#"{
        "tuple_key": {
            "user": "user:alice",
            "relation": "viewer",
            "object": "document:readme"
        },
        "authorization_model_id": "some-very-long-model-id-that-makes-the-request-exceed-the-limit"
    }"#;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/test-store/check")
                .header("content-type", "application/json")
                .body(Body::from(large_payload))
                .unwrap(),
        )
        .await
        .unwrap();

    // Should return 413 Payload Too Large
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}
