//! Integration tests for error handling and error conversion paths.
//!
//! These tests verify that domain errors are correctly converted to HTTP status codes
//! and that error messages are consistent across the API layer.
//!
//! Related to Issue #179: Improve DomainError handling and error conversion testing.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use rsfga_storage::{DataStore, MemoryDataStore, StoredAuthorizationModel};

use common::{create_test_app, post_json};

// ============================================================
// Section 1: Store Not Found Errors (404)
// ============================================================

/// Test: Check on non-existent store returns 404 Not Found
#[tokio::test]
async fn test_check_nonexistent_store_returns_404() {
    let storage = Arc::new(MemoryDataStore::new());

    // Use a valid ULID format that doesn't exist (OpenFGA validates format first, returning 400 for invalid ULIDs)
    let nonexistent_store_id = ulid::Ulid::new().to_string();

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{nonexistent_store_id}/check"),
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
        "Expected 404 for non-existent store"
    );

    // Verify error message contains store identifier
    let error_msg = response["message"].as_str().unwrap_or("");
    assert!(
        error_msg.contains("store") && error_msg.contains("not found"),
        "Error message should indicate store not found: {error_msg}"
    );
}

/// Test: Expand on non-existent store returns 404 Not Found
#[tokio::test]
async fn test_expand_nonexistent_store_returns_404() {
    let storage = Arc::new(MemoryDataStore::new());

    // Use a valid ULID format that doesn't exist (OpenFGA validates format first, returning 400 for invalid ULIDs)
    let nonexistent_store_id = ulid::Ulid::new().to_string();

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{nonexistent_store_id}/expand"),
        serde_json::json!({
            "tuple_key": {
                "relation": "viewer",
                "object": "document:readme"
            }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "Expected 404 for non-existent store"
    );

    let error_msg = response["message"].as_str().unwrap_or("");
    assert!(
        error_msg.contains("store") && error_msg.contains("not found"),
        "Error message should indicate store not found: {error_msg}"
    );
}

/// Test: ListObjects on non-existent store returns 404 Not Found
#[tokio::test]
async fn test_list_objects_nonexistent_store_returns_404() {
    let storage = Arc::new(MemoryDataStore::new());

    // Use a valid ULID format that doesn't exist (OpenFGA validates format first, returning 400 for invalid ULIDs)
    let nonexistent_store_id = ulid::Ulid::new().to_string();

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{nonexistent_store_id}/list-objects"),
        serde_json::json!({
            "type": "document",
            "relation": "viewer",
            "user": "user:alice"
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "Expected 404 for non-existent store"
    );

    let error_msg = response["message"].as_str().unwrap_or("");
    assert!(
        error_msg.contains("store") && error_msg.contains("not found"),
        "Error message should indicate store not found: {error_msg}"
    );
}

// ============================================================
// Section 2: Invalid Input Errors (400)
// ============================================================

/// Test: Check with invalid user format returns 400 Bad Request
#[tokio::test]
async fn test_check_invalid_user_format_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create store and model first
    let store_id = setup_test_store(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "invalid-user-no-colon",
                "relation": "viewer",
                "object": "document:readme"
            }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Expected 400 for invalid user format"
    );

    let error_msg = response["message"].as_str().unwrap_or("");
    assert!(
        error_msg.contains("invalid") && error_msg.contains("user"),
        "Error message should indicate invalid user: {error_msg}"
    );
}

/// Test: Check with invalid object format returns 400 Bad Request
#[tokio::test]
async fn test_check_invalid_object_format_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());

    let store_id = setup_test_store(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "invalid-object-no-colon"
            }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Expected 400 for invalid object format"
    );

    let error_msg = response["message"].as_str().unwrap_or("");
    assert!(
        error_msg.contains("invalid") && error_msg.contains("object"),
        "Error message should indicate invalid object: {error_msg}"
    );
}

/// Test: Check with type not in model returns 400 Bad Request
#[tokio::test]
async fn test_check_type_not_found_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());

    let store_id = setup_test_store(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "nonexistent_type:something"
            }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Expected 400 for type not found"
    );

    let error_msg = response["message"].as_str().unwrap_or("");
    assert!(
        error_msg.contains("type") && error_msg.contains("not found"),
        "Error message should indicate type not found: {error_msg}"
    );
}

/// Test: Check with relation not on type returns 400 Bad Request
#[tokio::test]
async fn test_check_relation_not_found_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());

    let store_id = setup_test_store(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "nonexistent_relation",
                "object": "document:readme"
            }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Expected 400 for relation not found"
    );

    let error_msg = response["message"].as_str().unwrap_or("");
    assert!(
        error_msg.contains("relation") && error_msg.contains("not found"),
        "Error message should indicate relation not found: {error_msg}"
    );
}

// ============================================================
// Section 3: Write Operation Errors
// ============================================================

/// Test: Write to non-existent store returns 404
#[tokio::test]
async fn test_write_nonexistent_store_returns_404() {
    let storage = Arc::new(MemoryDataStore::new());

    // Use a valid ULID format that doesn't exist (OpenFGA validates format first, returning 400 for invalid ULIDs)
    let nonexistent_store_id = ulid::Ulid::new().to_string();

    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{nonexistent_store_id}/write"),
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
        StatusCode::NOT_FOUND,
        "Expected 404 for non-existent store"
    );
}

// ============================================================
// Section 4: Read/List Operation Errors
// ============================================================

/// Test: Read tuples from non-existent store returns 404
#[tokio::test]
async fn test_read_tuples_nonexistent_store_returns_404() {
    let storage = Arc::new(MemoryDataStore::new());

    // Use a valid ULID format that doesn't exist (OpenFGA validates format first, returning 400 for invalid ULIDs)
    let nonexistent_store_id = ulid::Ulid::new().to_string();

    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{nonexistent_store_id}/read"),
        serde_json::json!({}),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "Expected 404 for non-existent store"
    );
}

// ============================================================
// Section 5: Batch Check Error Handling
// ============================================================

/// Test: Batch check with mixed valid/invalid requests returns partial results
#[tokio::test]
async fn test_batch_check_with_invalid_request_returns_error_for_that_request() {
    let storage = Arc::new(MemoryDataStore::new());

    let store_id = setup_test_store(&storage).await;

    // Write a valid tuple
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
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
    assert_eq!(status, StatusCode::OK);

    // Batch check with one valid and one invalid request
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/batch-check"),
        serde_json::json!({
            "checks": [
                {
                    "tuple_key": {
                        "user": "user:alice",
                        "relation": "viewer",
                        "object": "document:readme"
                    },
                    "correlation_id": "valid-check"
                },
                {
                    "tuple_key": {
                        "user": "user:bob",
                        "relation": "nonexistent_relation",
                        "object": "document:readme"
                    },
                    "correlation_id": "invalid-check"
                }
            ]
        }),
    )
    .await;

    // Batch check should return 200 with results containing errors
    assert_eq!(
        status,
        StatusCode::OK,
        "Batch check should return 200 even with errors"
    );

    // Verify we got results (field is "result" not "results")
    let results = response["result"].as_object();
    assert!(results.is_some(), "Should have result object: {response}");

    let results = results.unwrap();

    // Valid check should have allowed field
    let valid_result = &results["valid-check"];
    assert!(
        valid_result.get("allowed").is_some(),
        "Valid check should have allowed field"
    );

    // Invalid check should have error field
    let invalid_result = &results["invalid-check"];
    assert!(
        invalid_result.get("error").is_some(),
        "Invalid check should have error field: {invalid_result}"
    );
}

// ============================================================
// Section 6: Error Message Consistency
// ============================================================

/// Test: Error responses include consistent structure
#[tokio::test]
async fn test_error_response_structure_is_consistent() {
    let storage = Arc::new(MemoryDataStore::new());

    // Use a valid ULID format that doesn't exist (OpenFGA validates format first, returning 400 for invalid ULIDs)
    let nonexistent_store_id = ulid::Ulid::new().to_string();

    // Test 404 error structure
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{nonexistent_store_id}/check"),
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

    // Error response should have a "message" field
    assert!(
        response.get("message").is_some() || response.get("error").is_some(),
        "Error response should have 'message' or 'error' field: {response}"
    );
}

// ============================================================
// Helper Functions
// ============================================================

/// Set up a test store with a simple authorization model
async fn setup_test_store(storage: &MemoryDataStore) -> String {
    let store_id = ulid::Ulid::new().to_string();

    // Create store
    storage.create_store(&store_id, "Test Store").await.unwrap();

    // Create authorization model
    let model_json = r#"{
        "type_definitions": [
            {"type": "user"},
            {"type": "document", "relations": {"viewer": {}, "editor": {}, "owner": {}}}
        ]
    }"#;
    let model =
        StoredAuthorizationModel::new(ulid::Ulid::new().to_string(), &store_id, "1.1", model_json);
    storage.write_authorization_model(model).await.unwrap();

    store_id
}
