//! HTTP API tests for Section 2.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use metrics_exporter_prometheus::PrometheusBuilder;
use tower::ServiceExt; // for oneshot

use rsfga_storage::{DataStore, MemoryDataStore, StoredAuthorizationModel};

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

/// Helper function to create a simple authorization model JSON.
/// This model defines user and document types with basic relations.
fn simple_model_json() -> &'static str {
    r#"{
        "type_definitions": [
            {"type": "user"},
            {"type": "document", "relations": {"viewer": {}, "editor": {}, "owner": {}}}
        ]
    }"#
}

/// Helper function to set up a store with a basic authorization model.
/// Returns the store_id for convenience.
async fn setup_store_with_model(storage: &MemoryDataStore, store_id: &str, store_name: &str) {
    storage.create_store(store_id, store_name).await.unwrap();

    let model = StoredAuthorizationModel::new(
        format!("{}-model", store_id),
        store_id,
        "1.1",
        simple_model_json().to_string(),
    );
    storage.write_authorization_model(model).await.unwrap();
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

    // Create a store with authorization model
    setup_store_with_model(&storage, "01H0JA00000000000000000000", "Test-Store").await;

    // Write a tuple
    storage
        .write_tuple(
            "01H0JA00000000000000000000",
            rsfga_storage::StoredTuple::new("document", "readme", "viewer", "user", "alice", None),
        )
        .await
        .unwrap();

    // Check the tuple
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/01H0JA00000000000000000000/check")
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
        .create_store("01H0JA00000000000000000000", "Test-Store")
        .await
        .unwrap();

    let state = AppState::new(storage);
    let app = create_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/01H0JA00000000000000000000/check")
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

    // Create a store with authorization model
    setup_store_with_model(&storage, "01H0JA00000000000000000000", "Test-Store").await;

    let state = AppState::new(Arc::clone(&storage));
    let app = create_router(state);

    // Check for a non-existent tuple (no tuple written, so should return false)
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/01H0JA00000000000000000000/check")
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
    setup_store_with_model(&storage, "01H0JA00000000000000000000", "Test-Store").await;

    let state = AppState::new(storage);
    let app = create_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/01H0JA00000000000000000000/expand")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "tuple_key": {
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
        .create_store("01H0JA00000000000000000000", "Test-Store")
        .await
        .unwrap();

    let state = AppState::new(storage);
    let app = create_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/01H0JA00000000000000000000/write")
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
        .create_store("01H0JA00000000000000000000", "Test-Store")
        .await
        .unwrap();

    let state = AppState::new(storage);
    let app = create_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/01H0JA00000000000000000000/read")
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
        .create_store("01H0JA00000000000000000000", "Test-Store")
        .await
        .unwrap();

    let state = AppState::new(storage);
    let app = create_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/01H0JA00000000000000000000/check")
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
                .uri("/stores/01H0JA00000000000000000001/check")
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
        .create_store("01H0JA00000000000000000000", "Test-Store")
        .await
        .unwrap();

    let state = AppState::new(storage);
    let app = create_router(state);

    // Test with batch check that has too many items
    let mut checks = Vec::new();
    for i in 0..100 {
        checks.push(format!(
            r#"{{"tuple_key": {{"user": "user:{i}", "relation": "viewer", "object": "doc:{i}"}}, "correlation_id": "check-{i}"}}"#
        ));
    }
    let body = format!(r#"{{"checks": [{}]}}"#, checks.join(","));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/01H0JA00000000000000000000/batch-check")
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

    // Create a store with authorization model
    setup_store_with_model(&storage, "01H0JA00000000000000000000", "Test-Store").await;

    let state = AppState::new(Arc::clone(&storage));
    // Use a small limit for testing (1KB)
    let app = create_router_with_body_limit(state, 1024);

    // Small request (well within 1KB limit)
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/01H0JA00000000000000000000/check")
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
        .create_store("01H0JA00000000000000000000", "Test-Store")
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
                .uri("/stores/01H0JA00000000000000000000/check")
                .header("content-type", "application/json")
                .body(Body::from(large_payload))
                .unwrap(),
        )
        .await
        .unwrap();

    // Should return 413 Payload Too Large
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

// ============================================================
// UpdateStore Tests (Issue #85)
// ============================================================

/// Test: PUT /stores/{store_id} updates the store name
#[tokio::test]
async fn test_update_store_updates_name() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("01H0JA00000000000000000000", "Original-Name")
        .await
        .unwrap();

    let state = AppState::new(Arc::clone(&storage));
    let app = create_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/stores/01H0JA00000000000000000000")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name": "Updated-Name"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Response should contain the updated store
    assert_eq!(json["id"], "01H0JA00000000000000000000");
    assert_eq!(json["name"], "Updated-Name");
    assert!(json.get("created_at").is_some());
    assert!(json.get("updated_at").is_some());

    // Verify the store was actually updated in storage
    let store = storage.get_store("01H0JA00000000000000000000").await.unwrap();
    assert_eq!(store.name, "Updated-Name");
}

/// Test: PUT /stores/{store_id} returns 404 for non-existent store
#[tokio::test]
async fn test_update_store_returns_404_for_nonexistent() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/stores/01H0JA00000000000000000001")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name": "New-Name"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// Test: PUT /stores/{store_id} validates the name
#[tokio::test]
async fn test_update_store_validates_name() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("01H0JA00000000000000000000", "Original-Name")
        .await
        .unwrap();

    let state = AppState::new(storage);
    let app = create_router(state);

    // Empty name should fail validation
    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/stores/01H0JA00000000000000000000")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name": ""}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    // Should return 400 for invalid name
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

// ============================================================
// ULID Format Tests (PR #116)
// ============================================================

/// Test: Created store ID is in ULID format
///
/// Verifies that store IDs are generated using ULID format (26 characters,
/// Crockford Base32) for OpenFGA CLI compatibility.
#[tokio::test]
async fn test_created_store_id_is_ulid_format() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name": "01H0JA00000000000000000000"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);

    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    let id = json["id"].as_str().expect("id should be a string");

    // ULID format: 26 characters, Crockford Base32 (0-9, A-Z excluding I, L, O, U)
    assert_eq!(id.len(), 26, "ULID should be 26 characters");
    assert!(
        id.chars().all(|c| c.is_ascii_alphanumeric()),
        "ULID should only contain alphanumeric characters"
    );
    // ULID is uppercase Crockford Base32
    assert!(
        id.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()),
        "ULID should be uppercase"
    );
}

// ============================================================
// Null Handling Tests (PR #116)
// ============================================================

/// Test: Check endpoint handles null contextual_tuples.tuple_keys
///
/// The OpenFGA CLI sends `contextual_tuples: { tuple_keys: null }` instead of
/// an empty array. This test verifies we handle this gracefully.
#[tokio::test]
async fn test_check_handles_null_contextual_tuple_keys() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create a store with authorization model
    setup_store_with_model(&storage, "01H0JA00000000000000000000", "Test-Store").await;

    // Write a tuple to check against
    storage
        .write_tuple(
            "01H0JA00000000000000000000",
            rsfga_storage::StoredTuple::new("document", "readme", "viewer", "user", "alice", None),
        )
        .await
        .unwrap();

    let state = AppState::new(Arc::clone(&storage));
    let app = create_router(state);

    // Request with contextual_tuples.tuple_keys: null (as sent by fga CLI)
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/01H0JA00000000000000000000/check")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "tuple_key": {
                            "user": "user:alice",
                            "relation": "viewer",
                            "object": "document:readme"
                        },
                        "contextual_tuples": {
                            "tuple_keys": null
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

/// Test: Check endpoint handles missing contextual_tuples
///
/// Verifies check works when contextual_tuples is entirely omitted.
#[tokio::test]
async fn test_check_handles_missing_contextual_tuples() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create a store with authorization model
    setup_store_with_model(&storage, "01H0JA00000000000000000000", "Test-Store").await;

    storage
        .write_tuple(
            "01H0JA00000000000000000000",
            rsfga_storage::StoredTuple::new("document", "readme", "viewer", "user", "bob", None),
        )
        .await
        .unwrap();

    let state = AppState::new(Arc::clone(&storage));
    let app = create_router(state);

    // Request without contextual_tuples field at all
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/01H0JA00000000000000000000/check")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "tuple_key": {
                            "user": "user:bob",
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

// ============================================================
// Authorization Model Tests (Issue #117)
// ============================================================

/// Test: POST /stores/{store_id}/authorization-models creates a model
#[tokio::test]
async fn test_write_authorization_model_returns_201() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("01H0JA00000000000000000000", "Test-Store")
        .await
        .unwrap();

    let state = AppState::new(storage);
    let app = create_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/01H0JA00000000000000000000/authorization-models")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "schema_version": "1.1",
                        "type_definitions": [
                            {
                                "type": "user"
                            },
                            {
                                "type": "document",
                                "relations": {
                                    "viewer": {
                                        "this": {}
                                    }
                                },
                                "metadata": {
                                    "relations": {
                                        "viewer": {
                                            "directly_related_user_types": [
                                                {"type": "user"}
                                            ]
                                        }
                                    }
                                }
                            }
                        ]
                    }"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);

    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Verify response contains authorization_model_id in ULID format
    let model_id = json["authorization_model_id"]
        .as_str()
        .expect("authorization_model_id should be a string");
    assert_eq!(model_id.len(), 26, "Model ID should be a ULID (26 chars)");
}

/// Test: GET /stores/{store_id}/authorization-models lists models
#[tokio::test]
async fn test_list_authorization_models_returns_200() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("01H0JA00000000000000000000", "Test-Store")
        .await
        .unwrap();

    // Write a model directly to storage
    let model = rsfga_storage::StoredAuthorizationModel::new(
        "01HXK0ABCDEFGHIJKLMNOPQRST",
        "01H0JA00000000000000000000",
        "1.1",
        r#"{"type_definitions": [{"type": "user"}]}"#,
    );
    storage.write_authorization_model(model).await.unwrap();

    let state = AppState::new(storage);
    let app = create_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/stores/01H0JA00000000000000000000/authorization-models")
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

    let models = json["authorization_models"]
        .as_array()
        .expect("authorization_models should be an array");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0]["id"], "01HXK0ABCDEFGHIJKLMNOPQRST");
    assert_eq!(models[0]["schema_version"], "1.1");
}

/// Test: GET /stores/{store_id}/authorization-models/{id} returns a specific model
#[tokio::test]
async fn test_get_authorization_model_returns_200() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("01H0JA00000000000000000000", "Test-Store")
        .await
        .unwrap();

    // Write a model directly to storage
    let model = rsfga_storage::StoredAuthorizationModel::new(
        "01HXK0ABCDEFGHIJKLMNOPQRST",
        "01H0JA00000000000000000000",
        "1.1",
        r#"{"type_definitions": [{"type": "user"}], "conditions": null}"#,
    );
    storage.write_authorization_model(model).await.unwrap();

    let state = AppState::new(storage);
    let app = create_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/stores/01H0JA00000000000000000000/authorization-models/01HXK0ABCDEFGHIJKLMNOPQRST")
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

    let model = &json["authorization_model"];
    assert_eq!(model["id"], "01HXK0ABCDEFGHIJKLMNOPQRST");
    assert_eq!(model["schema_version"], "1.1");
    assert!(model["type_definitions"].is_array());
}

/// Test: GET /stores/{store_id}/authorization-models/{id} returns 404 for nonexistent model
#[tokio::test]
async fn test_get_nonexistent_authorization_model_returns_404() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("01H0JA00000000000000000000", "Test-Store")
        .await
        .unwrap();

    let state = AppState::new(storage);
    let app = create_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/stores/01H0JA00000000000000000000/authorization-models/nonexistent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// Test: POST /stores/{store_id}/authorization-models returns 404 for nonexistent store
#[tokio::test]
async fn test_write_authorization_model_to_nonexistent_store_returns_404() {
    let storage = Arc::new(MemoryDataStore::new());
    let state = AppState::new(storage);
    let app = create_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/01H0JA00000000000000000001/authorization-models")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "schema_version": "1.1",
                        "type_definitions": [{"type": "user"}]
                    }"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// Test: POST /stores/{store_id}/authorization-models returns 400 for empty type_definitions
#[tokio::test]
async fn test_write_authorization_model_with_empty_type_definitions_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("01H0JA00000000000000000000", "Test-Store")
        .await
        .unwrap();

    let state = AppState::new(storage);
    let app = create_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/01H0JA00000000000000000000/authorization-models")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "schema_version": "1.1",
                        "type_definitions": []
                    }"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["message"]
        .as_str()
        .unwrap()
        .contains("type_definitions cannot be empty"));
}

/// Test: GET /stores/{store_id}/authorization-models with invalid continuation_token returns 400
#[tokio::test]
async fn test_list_authorization_models_with_invalid_continuation_token_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("01H0JA00000000000000000000", "Test-Store")
        .await
        .unwrap();

    let state = AppState::new(storage);
    let app = create_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/stores/01H0JA00000000000000000000/authorization-models?continuation_token=invalid")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["message"]
        .as_str()
        .unwrap()
        .contains("invalid continuation_token"));
}

/// Test: GET /stores/{store_id}/authorization-models pagination with continuation_token works
#[tokio::test]
async fn test_list_authorization_models_pagination_works() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("01H0JA00000000000000000000", "Test-Store")
        .await
        .unwrap();

    // Create 3 models
    for i in 0..3 {
        let model = StoredAuthorizationModel::new(
            format!("model-{i}"),
            "01H0JA00000000000000000000",
            "1.1",
            format!(r#"{{"type_definitions": [{{"type": "type{i}"}}]}}"#),
        );
        storage.write_authorization_model(model).await.unwrap();
    }

    let state = AppState::new(storage);
    let app = create_router(state);

    // First page (page_size=2)
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/stores/01H0JA00000000000000000000/authorization-models?page_size=2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    let models = json["authorization_models"].as_array().unwrap();
    assert_eq!(models.len(), 2);
    let token = json["continuation_token"].as_str().unwrap();

    // Second page using continuation_token
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/stores/01H0JA00000000000000000000/authorization-models?page_size=2&continuation_token={token}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    let models = json["authorization_models"].as_array().unwrap();
    assert_eq!(models.len(), 1); // Only 1 remaining model
    assert!(json["continuation_token"].is_null()); // No more pages
}

/// Test: POST /stores/{store_id}/authorization-models rejects oversized models
#[tokio::test]
async fn test_write_authorization_model_rejects_oversized_model() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("01H0JA00000000000000000000", "Test-Store")
        .await
        .unwrap();

    let state = AppState::new(storage);
    // Use a higher body limit (2MB) so the request passes Axum's middleware
    // and our handler's validation logic can reject it with a proper error message
    let app = create_router_with_body_limit(state, 2 * 1024 * 1024);

    // Create a model larger than 1MB (our application limit)
    let large_type = "x".repeat(1024 * 1024); // 1MB string
    let body_json = serde_json::json!({
        "schema_version": "1.1",
        "type_definitions": [{"type": large_type}]
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/01H0JA00000000000000000000/authorization-models")
                .header("content-type", "application/json")
                .body(Body::from(body_json.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["message"]
        .as_str()
        .unwrap()
        .contains("exceeds maximum size"));
}

/// Test: GET /stores/{store_id}/authorization-models returns latest model first
///
/// Verifies that when listing authorization models, the most recently created
/// model appears first. This is the expected way to "get the latest model" in
/// the OpenFGA API - by listing models and taking the first one.
#[tokio::test]
async fn test_list_authorization_models_returns_latest_first() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("01H0JA00000000000000000000", "Test-Store")
        .await
        .unwrap();

    // Create first model
    let model_json1 = serde_json::json!({
        "schema_version": "1.1",
        "type_definitions": [{"type": "document"}]
    });
    let model1 =
        StoredAuthorizationModel::new("model-001", "01H0JA00000000000000000000", "1.1", model_json1.to_string());
    storage.write_authorization_model(model1).await.unwrap();

    // Small delay to ensure different timestamps
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    // Create second (more recent) model
    let model_json2 = serde_json::json!({
        "schema_version": "1.1",
        "type_definitions": [{"type": "user"}]
    });
    let model2 =
        StoredAuthorizationModel::new("model-002", "01H0JA00000000000000000000", "1.1", model_json2.to_string());
    storage.write_authorization_model(model2).await.unwrap();

    let state = AppState::new(storage);
    let app = create_router(state);

    // List models
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/stores/01H0JA00000000000000000000/authorization-models")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    let models = json["authorization_models"].as_array().unwrap();
    assert_eq!(models.len(), 2);

    // Most recent model (model-002) should be first
    let first_model_id = models[0]["id"].as_str().unwrap();
    let second_model_id = models[1]["id"].as_str().unwrap();

    assert_eq!(first_model_id, "model-002", "Latest model should be first");
    assert_eq!(second_model_id, "model-001", "Older model should be second");
}

// ============================================================
// Condition Parsing Tests (Issue #84)
// ============================================================

/// Test: POST /stores/{store_id}/write correctly parses conditions
///
/// Verifies that when a tuple includes a condition, the condition name
/// and context are correctly extracted and stored.
#[tokio::test]
async fn test_write_endpoint_parses_conditions() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("01H0JA00000000000000000000", "Test-Store")
        .await
        .unwrap();

    let state = AppState::new(Arc::clone(&storage));
    let app = create_router(state);

    // Write a tuple with a condition containing context
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/01H0JA00000000000000000000/write")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "writes": {
                            "tuple_keys": [
                                {
                                    "user": "user:alice",
                                    "relation": "viewer",
                                    "object": "document:secret",
                                    "condition": {
                                        "name": "ip_restriction",
                                        "context": {
                                            "allowed_ip": "192.168.1.100",
                                            "max_age": 30
                                        }
                                    }
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

    // Verify the tuple was written with the condition
    let tuples = storage
        .read_tuples("01H0JA00000000000000000000", &Default::default())
        .await
        .unwrap();

    assert_eq!(tuples.len(), 1);
    let tuple = &tuples[0];

    // Verify condition name
    assert_eq!(tuple.condition_name, Some("ip_restriction".to_string()));

    // Verify condition context
    let context = tuple
        .condition_context
        .as_ref()
        .expect("condition context should be present");
    assert_eq!(
        context.get("allowed_ip"),
        Some(&serde_json::json!("192.168.1.100"))
    );
    assert_eq!(context.get("max_age"), Some(&serde_json::json!(30)));
}

/// Test: POST /stores/{store_id}/write parses condition without context
///
/// Verifies that a condition with only a name (no context) is correctly parsed.
#[tokio::test]
async fn test_write_endpoint_parses_condition_without_context() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("01H0JA00000000000000000000", "Test-Store")
        .await
        .unwrap();

    let state = AppState::new(Arc::clone(&storage));
    let app = create_router(state);

    // Write a tuple with a condition but no context
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/01H0JA00000000000000000000/write")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "writes": {
                            "tuple_keys": [
                                {
                                    "user": "user:bob",
                                    "relation": "editor",
                                    "object": "document:report",
                                    "condition": {
                                        "name": "time_based_access"
                                    }
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

    // Verify the tuple was written
    let tuples = storage
        .read_tuples("01H0JA00000000000000000000", &Default::default())
        .await
        .unwrap();

    assert_eq!(tuples.len(), 1);
    let tuple = &tuples[0];

    // Verify condition name is present but context is None
    assert_eq!(tuple.condition_name, Some("time_based_access".to_string()));
    assert!(tuple.condition_context.is_none());
}

/// Test: POST /stores/{store_id}/write ignores empty condition name
///
/// Verifies that a condition with an empty name is treated as no condition.
#[tokio::test]
async fn test_write_endpoint_ignores_empty_condition_name() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("01H0JA00000000000000000000", "Test-Store")
        .await
        .unwrap();

    let state = AppState::new(Arc::clone(&storage));
    let app = create_router(state);

    // Write a tuple with a condition that has an empty name
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/01H0JA00000000000000000000/write")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "writes": {
                            "tuple_keys": [
                                {
                                    "user": "user:carol",
                                    "relation": "owner",
                                    "object": "document:public",
                                    "condition": {
                                        "name": "",
                                        "context": {"key": "value"}
                                    }
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

    // Verify the tuple was written without condition
    let tuples = storage
        .read_tuples("01H0JA00000000000000000000", &Default::default())
        .await
        .unwrap();

    assert_eq!(tuples.len(), 1);
    let tuple = &tuples[0];

    // Empty condition name should be treated as no condition
    assert!(tuple.condition_name.is_none());
    assert!(tuple.condition_context.is_none());
}

/// Test: POST /stores/{store_id}/write handles multiple tuples with conditions
///
/// Verifies that multiple tuples with different conditions are correctly parsed.
#[tokio::test]
async fn test_write_endpoint_handles_multiple_tuples_with_conditions() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("01H0JA00000000000000000000", "Test-Store")
        .await
        .unwrap();

    let state = AppState::new(Arc::clone(&storage));
    let app = create_router(state);

    // Write multiple tuples with different conditions
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/01H0JA00000000000000000000/write")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "writes": {
                            "tuple_keys": [
                                {
                                    "user": "user:alice",
                                    "relation": "viewer",
                                    "object": "document:doc1",
                                    "condition": {
                                        "name": "condition_a",
                                        "context": {"param": "value_a"}
                                    }
                                },
                                {
                                    "user": "user:bob",
                                    "relation": "editor",
                                    "object": "document:doc2"
                                },
                                {
                                    "user": "user:carol",
                                    "relation": "owner",
                                    "object": "document:doc3",
                                    "condition": {
                                        "name": "condition_c"
                                    }
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

    // Verify all tuples were written
    let tuples = storage
        .read_tuples("01H0JA00000000000000000000", &Default::default())
        .await
        .unwrap();

    assert_eq!(tuples.len(), 3);

    // Find each tuple and verify its condition
    let alice_tuple = tuples.iter().find(|t| t.user_id == "alice").unwrap();
    assert_eq!(alice_tuple.condition_name, Some("condition_a".to_string()));
    assert!(alice_tuple.condition_context.is_some());

    let bob_tuple = tuples.iter().find(|t| t.user_id == "bob").unwrap();
    assert!(bob_tuple.condition_name.is_none());
    assert!(bob_tuple.condition_context.is_none());

    let carol_tuple = tuples.iter().find(|t| t.user_id == "carol").unwrap();
    assert_eq!(carol_tuple.condition_name, Some("condition_c".to_string()));
    assert!(carol_tuple.condition_context.is_none());
}

/// Test: POST /stores/{store_id}/write rejects invalid condition name
///
/// Verifies that condition names with special characters are rejected (security I4).
#[tokio::test]
async fn test_write_endpoint_rejects_invalid_condition_name() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("01H0JA00000000000000000000", "Test-Store")
        .await
        .unwrap();

    let state = AppState::new(Arc::clone(&storage));
    let app = create_router(state);

    // Write a tuple with an invalid condition name (special characters)
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/01H0JA00000000000000000000/write")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "writes": {
                            "tuple_keys": [
                                {
                                    "user": "user:alice",
                                    "relation": "viewer",
                                    "object": "document:secret",
                                    "condition": {
                                        "name": "invalid;DROP TABLE--"
                                    }
                                }
                            ]
                        }
                    }"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let message = json["message"].as_str().unwrap();
    assert!(message.contains("invalid condition name"));
}

/// Test: POST /stores/{store_id}/write accepts valid condition names
///
/// Verifies that valid condition names with underscore and hyphen are accepted.
#[tokio::test]
async fn test_write_endpoint_accepts_valid_condition_name_formats() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("01H0JA00000000000000000000", "Test-Store")
        .await
        .unwrap();

    let state = AppState::new(Arc::clone(&storage));
    let app = create_router(state);

    // Write a tuple with a valid condition name (underscore and hyphen)
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/01H0JA00000000000000000000/write")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "writes": {
                            "tuple_keys": [
                                {
                                    "user": "user:alice",
                                    "relation": "viewer",
                                    "object": "document:report",
                                    "condition": {
                                        "name": "ip_restriction-v2"
                                    }
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

    // Verify the tuple was written with the condition
    let tuples = storage
        .read_tuples("01H0JA00000000000000000000", &Default::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1);
    assert_eq!(
        tuples[0].condition_name,
        Some("ip_restriction-v2".to_string())
    );
}

/// Test: GET /stores/{store_id}/read returns conditions stored with tuples
///
/// Verifies read-after-write contract: conditions written are returned by Read (I2).
#[tokio::test]
async fn test_read_endpoint_returns_conditions() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("01H0JA00000000000000000000", "Test-Store")
        .await
        .unwrap();

    // First, write a tuple with a condition
    let write_state = AppState::new(Arc::clone(&storage));
    let write_app = create_router(write_state);
    let write_response = write_app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/01H0JA00000000000000000000/write")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "writes": {
                            "tuple_keys": [
                                {
                                    "user": "user:alice",
                                    "relation": "viewer",
                                    "object": "document:secret",
                                    "condition": {
                                        "name": "ip_restriction",
                                        "context": {
                                            "allowed_ip": "192.168.1.100"
                                        }
                                    }
                                }
                            ]
                        }
                    }"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(write_response.status(), StatusCode::OK);

    // Now read the tuples back - create new app with same storage
    let read_state = AppState::new(Arc::clone(&storage));
    let app2 = create_router(read_state);
    let read_response = app2
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stores/01H0JA00000000000000000000/read")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(read_response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(read_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    let tuples = json["tuples"].as_array().unwrap();
    assert_eq!(tuples.len(), 1);

    let key = &tuples[0]["key"];
    assert_eq!(key["user"], "user:alice");
    assert_eq!(key["relation"], "viewer");
    assert_eq!(key["object"], "document:secret");

    // Verify condition is returned
    let condition = &key["condition"];
    assert_eq!(condition["name"], "ip_restriction");
    assert_eq!(condition["context"]["allowed_ip"], "192.168.1.100");
}
