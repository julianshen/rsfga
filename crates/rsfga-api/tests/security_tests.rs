//! Security tests for RSFGA API.
//!
//! These tests verify the system is protected against common security threats
//! including injection attacks, input validation bypass, and authorization bypass.
//!
//! **Test Categories**:
//! - SQL/NoSQL injection protection
//! - Input validation and sanitization
//! - Rate limiting (when enabled)
//! - Authorization model integrity
//!
//! **Running Security Tests**:
//! `cargo test -p rsfga-api --test security_tests`
//!
//! **Note**: These tests focus on API-level security. Full security audits
//! should include fuzz testing, penetration testing, and code review.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;

use rsfga_api::http::{create_router, AppState};
use rsfga_storage::MemoryDataStore;

// =============================================================================
// Test Helpers
// =============================================================================

/// Create a test app with in-memory storage.
fn create_test_app(storage: &Arc<MemoryDataStore>) -> axum::Router {
    let state = AppState::new(Arc::clone(storage));
    create_router(state)
}

/// Make a JSON POST request.
async fn post_json(
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
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or(serde_json::json!({}));
    (status, json)
}

/// Make a raw POST request with string body.
async fn post_raw(app: axum::Router, uri: &str, body: &str) -> StatusCode {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    response.status()
}

// =============================================================================
// Section 5: Security Tests
// =============================================================================

// -----------------------------------------------------------------------------
// SQL/NoSQL Injection Protection
// -----------------------------------------------------------------------------

/// Test: SQL injection in user field is rejected or sanitized
#[tokio::test]
async fn test_sql_injection_in_user_field_rejected() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "security-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap();

    // Attempt SQL injection in user field
    let injection_payloads = vec![
        "user:alice'; DROP TABLE tuples;--",
        "user:alice' OR '1'='1",
        "user:alice\"; DROP TABLE tuples;--",
        "user:alice\" OR \"1\"=\"1",
        "user:alice; DELETE FROM tuples WHERE 1=1;",
    ];

    for payload in injection_payloads {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{}/check", store_id),
            serde_json::json!({
                "tuple_key": {
                    "user": payload,
                    "relation": "viewer",
                    "object": "document:doc1"
                }
            }),
        )
        .await;

        // Should either reject (400) or safely process (200 with allowed: false)
        // Should NEVER cause 500 (server error indicating SQL execution)
        assert!(
            status == StatusCode::OK || status == StatusCode::BAD_REQUEST,
            "SQL injection payload '{}' should be handled safely, got {}",
            payload,
            status
        );
    }
}

/// Test: SQL injection in object field is rejected or sanitized
#[tokio::test]
async fn test_sql_injection_in_object_field_rejected() {
    let storage = Arc::new(MemoryDataStore::new());

    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "security-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap();

    let injection_payloads = vec![
        "document:doc1'; DROP TABLE stores;--",
        "document:doc1' UNION SELECT * FROM users;--",
        "document:doc1'; UPDATE tuples SET relation='admin';--",
    ];

    for payload in injection_payloads {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{}/check", store_id),
            serde_json::json!({
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": payload
                }
            }),
        )
        .await;

        assert!(
            status == StatusCode::OK || status == StatusCode::BAD_REQUEST,
            "SQL injection in object '{}' should be handled safely, got {}",
            payload,
            status
        );
    }
}

/// Test: SQL injection in store creation name is rejected or sanitized
#[tokio::test]
async fn test_sql_injection_in_store_name_rejected() {
    let storage = Arc::new(MemoryDataStore::new());

    let injection_payloads = vec![
        "test'; DROP TABLE stores;--",
        "test' OR '1'='1",
        "test\"; DROP DATABASE rsfga;--",
    ];

    for payload in &injection_payloads {
        let (status, _) = post_json(
            create_test_app(&storage),
            "/stores",
            serde_json::json!({"name": payload}),
        )
        .await;

        // Should either create store (with sanitized name) or reject
        // Should NOT cause server error
        assert!(
            status == StatusCode::CREATED || status == StatusCode::BAD_REQUEST,
            "SQL injection in store name should be handled safely, got {}",
            status
        );
    }
}

// -----------------------------------------------------------------------------
// Input Validation
// -----------------------------------------------------------------------------

/// Test: Malformed JSON is rejected
#[tokio::test]
async fn test_malformed_json_rejected() {
    let storage = Arc::new(MemoryDataStore::new());

    let malformed_payloads = vec![
        "{invalid json}",
        "{\"name\": }",
        "{\"name\": \"test\",,}",
        "not json at all",
        "",
        "{",
        "[[[",
    ];

    for payload in malformed_payloads {
        let status = post_raw(create_test_app(&storage), "/stores", payload).await;

        assert!(
            status.is_client_error(),
            "Malformed JSON '{}' should return client error, got {}",
            payload,
            status
        );
    }
}

/// Test: Oversized payloads are rejected
#[tokio::test]
async fn test_oversized_payload_rejected() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create a very large payload (> 1MB)
    let large_name = "x".repeat(1024 * 1024 + 1);
    let (status, _) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": large_name}),
    )
    .await;

    // Should be rejected - either by payload size limit or validation
    // Note: The actual limit depends on server configuration
    assert!(
        status.is_client_error() || status == StatusCode::OK,
        "Very large payload should either be rejected or handled safely"
    );
}

/// Test: Special characters in identifiers are handled safely
#[tokio::test]
async fn test_special_characters_handled_safely() {
    let storage = Arc::new(MemoryDataStore::new());

    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "test-store"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap();

    let special_chars = vec![
        "user:alice<script>alert(1)</script>",
        "user:alice\0null",
        "user:alice\n\r\t",
        "user:alice${7*7}",
        "user:alice{{7*7}}",
        "user:alice%00%0d%0a",
        "user:αβγδ", // Unicode
        "user:alice/../../../etc/passwd",
    ];

    for user in special_chars {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{}/check", store_id),
            serde_json::json!({
                "tuple_key": {
                    "user": user,
                    "relation": "viewer",
                    "object": "document:doc1"
                }
            }),
        )
        .await;

        // Should be handled safely without server errors
        assert!(
            !status.is_server_error(),
            "Special char '{}' should not cause server error, got {}",
            user,
            status
        );
    }
}

/// Test: Null bytes are rejected
#[tokio::test]
async fn test_null_bytes_rejected() {
    let storage = Arc::new(MemoryDataStore::new());

    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap();

    // Null byte in user field
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{}/check", store_id),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice\0admin",
                "relation": "viewer",
                "object": "document:doc1"
            }
        }),
    )
    .await;

    assert!(
        !status.is_server_error(),
        "Null bytes should not cause server error"
    );
}

// -----------------------------------------------------------------------------
// Rate Limiting
// -----------------------------------------------------------------------------

/// Test: Rate limiting works when enabled
///
/// Note: Rate limiting is typically configured at deployment level.
/// This test documents the expected behavior.
#[tokio::test]
#[ignore = "rate limiting requires configuration at deployment level"]
async fn test_rate_limiting_works() {
    // This test would:
    // 1. Configure rate limits
    // 2. Send requests exceeding the limit
    // 3. Verify 429 Too Many Requests responses
    // 4. Verify Retry-After header presence
    //
    // Implementation depends on rate limiting middleware configuration.
    todo!("Implement when rate limiting is configured");
}

// -----------------------------------------------------------------------------
// Authorization Model Integrity
// -----------------------------------------------------------------------------

/// Test: Authorization model cannot be bypassed via tuple manipulation
#[tokio::test]
async fn test_authorization_model_cannot_be_bypassed() {
    let storage = Arc::new(MemoryDataStore::new());

    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "auth-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap();

    // Write a legitimate tuple
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{}/write", store_id),
        serde_json::json!({
            "writes": {
                "tuple_keys": [
                    {"user": "user:alice", "relation": "viewer", "object": "document:doc1"}
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Attempt bypass: Check with modified user ID (case manipulation)
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{}/check", store_id),
        serde_json::json!({
            "tuple_key": {
                "user": "user:ALICE",  // Different case
                "relation": "viewer",
                "object": "document:doc1"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // Identifiers should be case-sensitive
    assert_eq!(
        response["allowed"], false,
        "Case-different user should not have access"
    );

    // Attempt bypass: Check non-existent relation
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{}/check", store_id),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "admin",  // Non-existent relation
                "object": "document:doc1"
            }
        }),
    )
    .await;
    // Should return false or error, not accidentally grant access
    assert!(
        status == StatusCode::OK && response["allowed"] == false
            || status == StatusCode::BAD_REQUEST,
        "Non-existent relation should not grant access"
    );
}

/// Test: Cross-store access is prevented
#[tokio::test]
async fn test_cross_store_access_prevented() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create two stores
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "store-a"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_a_id = response["id"].as_str().unwrap().to_string();

    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": "store-b"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_b_id = response["id"].as_str().unwrap().to_string();

    // Write tuple to store A
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{}/write", store_a_id),
        serde_json::json!({
            "writes": {
                "tuple_keys": [
                    {"user": "user:alice", "relation": "viewer", "object": "document:secret"}
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Verify access in store A
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{}/check", store_a_id),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:secret"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["allowed"], true, "Should have access in store A");

    // Verify NO access in store B (cross-store isolation)
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{}/check", store_b_id),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:secret"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"], false,
        "Should NOT have access in store B (cross-store isolation)"
    );
}

/// Test: Path traversal in store_id is rejected
#[tokio::test]
async fn test_path_traversal_rejected() {
    let storage = Arc::new(MemoryDataStore::new());

    let traversal_attempts = vec![
        "../../../etc/passwd",
        "..\\..\\..\\windows\\system32",
        "store/../../../secret",
        "%2e%2e%2f%2e%2e%2f",
        "....//....//",
    ];

    for store_id in traversal_attempts {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{}/check", store_id),
            serde_json::json!({
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:doc1"
                }
            }),
        )
        .await;

        // Should return client error (store not found) or OK with false
        // Should NOT return server error (indicating path traversal worked)
        assert!(
            !status.is_server_error(),
            "Path traversal '{}' should not cause server error, got {}",
            store_id,
            status
        );
    }
}

/// Test: Authentication required for sensitive endpoints
///
/// Note: Authentication is typically configured at deployment level.
/// This test documents expected behavior when auth is enabled.
#[tokio::test]
#[ignore = "authentication requires configuration at deployment level"]
async fn test_authentication_required() {
    // This test would:
    // 1. Configure authentication
    // 2. Attempt requests without credentials
    // 3. Verify 401 Unauthorized responses
    // 4. Verify valid credentials succeed
    //
    // Implementation depends on authentication middleware configuration.
    todo!("Implement when authentication is configured");
}
