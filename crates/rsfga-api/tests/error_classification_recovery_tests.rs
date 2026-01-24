//! Integration tests for error classification and recovery (Issue #208).
//!
//! These tests verify that:
//! 1. Errors are correctly classified with appropriate HTTP status codes
//! 2. Batch operations handle partial failures gracefully
//! 3. Error messages are sanitized to prevent information leakage
//! 4. Error handling is consistent across HTTP and gRPC protocols
//!
//! # Error Classification Requirements
//!
//! - Malformed inputs → 400 VALIDATION_ERROR
//! - Missing types/relations → 400 (not 500)
//! - Storage failures → 503 INTERNAL_ERROR
//! - Timeouts → 408/504 DEADLINE_EXCEEDED
//!
//! # Security Requirements
//!
//! Error messages must not leak:
//! - Stack traces
//! - Connection strings
//! - Internal type names
//! - System paths

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use rsfga_storage::MemoryDataStore;
use tonic::Request;

use common::{create_test_app, post_json, post_raw, setup_test_store};

// ============================================================================
// Section 1: Input Validation Error Classification (400 VALIDATION_ERROR)
// ============================================================================

/// Test: Malformed JSON returns 400 BAD_REQUEST (not 500)
#[tokio::test]
async fn test_malformed_json_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Send malformed JSON (missing closing brace)
    let status = post_raw(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        r#"{"tuple_key": {"user": "user:alice""#,
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Malformed JSON should return 400, not 500"
    );
}

/// Test: Missing required fields returns 400 VALIDATION_ERROR
#[tokio::test]
async fn test_missing_required_fields_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Missing tuple_key
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({}),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Missing required fields should return 400"
    );
    assert_eq!(
        response["code"], "validation_error",
        "Should return validation_error code"
    );
}

/// Test: Empty user field returns 400 VALIDATION_ERROR
#[tokio::test]
async fn test_empty_user_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "",
                "relation": "viewer",
                "object": "document:readme"
            }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Empty user should return 400"
    );
    assert_eq!(response["code"], "validation_error");
}

/// Test: Empty relation field returns 400 VALIDATION_ERROR
#[tokio::test]
async fn test_empty_relation_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "",
                "object": "document:readme"
            }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Empty relation should return 400"
    );
    assert_eq!(response["code"], "validation_error");
}

/// Test: Empty object field returns 400 VALIDATION_ERROR
#[tokio::test]
async fn test_empty_object_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": ""
            }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Empty object should return 400"
    );
    assert_eq!(response["code"], "validation_error");
}

/// Test: Invalid user format (missing colon) returns 400 VALIDATION_ERROR
#[tokio::test]
async fn test_invalid_user_format_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "alice-no-type-prefix",
                "relation": "viewer",
                "object": "document:readme"
            }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Invalid user format should return 400"
    );
    assert_eq!(response["code"], "validation_error");
}

/// Test: Invalid object format (missing colon) returns 400 VALIDATION_ERROR
#[tokio::test]
async fn test_invalid_object_format_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document-no-type-prefix"
            }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Invalid object format should return 400"
    );
    assert_eq!(response["code"], "validation_error");
}

// ============================================================================
// Section 2: Type/Relation Not Found Errors (400, not 500)
// ============================================================================

/// Test: Type not found in model returns 400 type_not_found (not 500)
#[tokio::test]
async fn test_type_not_found_returns_400_not_500() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "nonexistent_type:some_id"
            }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Type not found should return 400, not 500"
    );
    assert_eq!(
        response["code"], "type_not_found",
        "Type not found should use type_not_found error code"
    );

    let message = response["message"].as_str().unwrap_or("");
    assert!(
        message.to_lowercase().contains("type") && message.to_lowercase().contains("not found"),
        "Error message should mention 'type not found': {message}"
    );
}

/// Test: Relation not found on type returns 400 relation_not_found (not 500)
#[tokio::test]
async fn test_relation_not_found_returns_400_not_500() {
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
        "Relation not found should return 400, not 500"
    );
    assert_eq!(
        response["code"], "relation_not_found",
        "Relation not found should use relation_not_found error code"
    );

    let message = response["message"].as_str().unwrap_or("");
    assert!(
        message.to_lowercase().contains("relation") && message.to_lowercase().contains("not found"),
        "Error message should mention 'relation not found': {message}"
    );
}

/// Test: ListObjects with unknown type returns 200 with empty results
///
/// Note: Unlike Check/Expand, ListObjects returns 200 with empty objects
/// for unknown types. This is consistent with OpenFGA behavior where
/// listing returns empty results rather than errors.
#[tokio::test]
async fn test_list_objects_unknown_type_returns_empty_results() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-objects"),
        serde_json::json!({
            "type": "unknown_type",
            "relation": "viewer",
            "user": "user:alice"
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "ListObjects with unknown type returns 200 with empty results"
    );
    // Verify empty results
    let objects = response["objects"].as_array();
    assert!(
        objects.is_none() || objects.unwrap().is_empty(),
        "Should return empty objects for unknown type"
    );
}

/// Test: ListObjects with unknown relation returns 200 with empty results
///
/// Note: Unlike Check/Expand, ListObjects returns 200 with empty objects
/// for unknown relations. This is consistent with OpenFGA behavior.
#[tokio::test]
async fn test_list_objects_unknown_relation_returns_empty_results() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-objects"),
        serde_json::json!({
            "type": "document",
            "relation": "nonexistent_relation",
            "user": "user:alice"
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "ListObjects with unknown relation returns 200 with empty results"
    );
    // Verify empty results
    let objects = response["objects"].as_array();
    assert!(
        objects.is_none() || objects.unwrap().is_empty(),
        "Should return empty objects for unknown relation"
    );
}

/// Test: Expand with unknown type returns 400 VALIDATION_ERROR
#[tokio::test]
async fn test_expand_unknown_type_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/expand"),
        serde_json::json!({
            "tuple_key": {
                "relation": "viewer",
                "object": "unknown_type:some_id"
            }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Expand with unknown type should return 400"
    );
    assert_eq!(response["code"], "type_not_found");
}

/// Test: Expand with unknown relation returns 400 RELATION_NOT_FOUND
#[tokio::test]
async fn test_expand_unknown_relation_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/expand"),
        serde_json::json!({
            "tuple_key": {
                "relation": "nonexistent_relation",
                "object": "document:readme"
            }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Expand with unknown relation should return 400"
    );
    assert_eq!(response["code"], "relation_not_found");
}

// ============================================================================
// Section 3: Batch Operation Resilience (Partial Failures)
// ============================================================================

/// Test: Batch check continues processing after individual failures
#[tokio::test]
async fn test_batch_check_continues_after_individual_failures() {
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
                    },
                    {
                        "user": "user:bob",
                        "relation": "editor",
                        "object": "document:readme"
                    }
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Batch check with mix of valid checks and invalid checks
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
                    "correlation_id": "valid-check-1"
                },
                {
                    "tuple_key": {
                        "user": "user:bob",
                        "relation": "nonexistent_relation",
                        "object": "document:readme"
                    },
                    "correlation_id": "invalid-check-1"
                },
                {
                    "tuple_key": {
                        "user": "user:bob",
                        "relation": "editor",
                        "object": "document:readme"
                    },
                    "correlation_id": "valid-check-2"
                },
                {
                    "tuple_key": {
                        "user": "user:charlie",
                        "relation": "viewer",
                        "object": "unknown_type:something"
                    },
                    "correlation_id": "invalid-check-2"
                },
                {
                    "tuple_key": {
                        "user": "user:charlie",
                        "relation": "viewer",
                        "object": "document:readme"
                    },
                    "correlation_id": "valid-check-3"
                }
            ]
        }),
    )
    .await;

    // Batch check returns 200 even with partial failures
    assert_eq!(
        status,
        StatusCode::OK,
        "Batch check should return 200 even with partial failures"
    );

    let results = response["result"]
        .as_object()
        .expect("Should have result object");

    // All 5 checks should have results
    assert_eq!(
        results.len(),
        5,
        "All 5 checks should have results, got: {results:?}"
    );

    // Valid checks should have allowed field
    assert!(
        results["valid-check-1"].get("allowed").is_some(),
        "valid-check-1 should have allowed field"
    );
    assert!(
        results["valid-check-2"].get("allowed").is_some(),
        "valid-check-2 should have allowed field"
    );
    assert!(
        results["valid-check-3"].get("allowed").is_some(),
        "valid-check-3 should have allowed field"
    );

    // Invalid checks should have error field
    assert!(
        results["invalid-check-1"].get("error").is_some(),
        "invalid-check-1 should have error field"
    );
    assert!(
        results["invalid-check-2"].get("error").is_some(),
        "invalid-check-2 should have error field"
    );
}

/// Test: Batch check maintains correlation_id for all items
#[tokio::test]
async fn test_batch_check_maintains_correlation_ids() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let correlation_ids = vec!["check-uuid-1-abc", "check-uuid-2-def", "check-uuid-3-ghi"];

    let checks: Vec<_> = correlation_ids
        .iter()
        .enumerate()
        .map(|(i, id)| {
            serde_json::json!({
                "tuple_key": {
                    "user": format!("user:user{i}"),
                    "relation": "viewer",
                    "object": "document:readme"
                },
                "correlation_id": id
            })
        })
        .collect();

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/batch-check"),
        serde_json::json!({ "checks": checks }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let results = response["result"]
        .as_object()
        .expect("Should have result object");

    // Verify all correlation_ids are present in results
    for id in &correlation_ids {
        assert!(
            results.contains_key(*id),
            "Result should contain correlation_id '{id}'"
        );
    }
}

/// Test: Batch check processes all items even when first item fails
#[tokio::test]
async fn test_batch_check_does_not_stop_at_first_error() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/batch-check"),
        serde_json::json!({
            "checks": [
                {
                    "tuple_key": {
                        "user": "user:alice",
                        "relation": "unknown_relation_first",
                        "object": "document:readme"
                    },
                    "correlation_id": "first-item-fails"
                },
                {
                    "tuple_key": {
                        "user": "user:bob",
                        "relation": "viewer",
                        "object": "document:readme"
                    },
                    "correlation_id": "second-item-should-process"
                },
                {
                    "tuple_key": {
                        "user": "user:charlie",
                        "relation": "viewer",
                        "object": "document:readme"
                    },
                    "correlation_id": "third-item-should-process"
                }
            ]
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let results = response["result"]
        .as_object()
        .expect("Should have result object");

    // All items should have results
    assert_eq!(
        results.len(),
        3,
        "All 3 items should be processed even if first fails"
    );

    // First item should have error
    assert!(
        results["first-item-fails"].get("error").is_some(),
        "First item should have error"
    );

    // Subsequent items should still be processed and have allowed field
    assert!(
        results["second-item-should-process"]
            .get("allowed")
            .is_some(),
        "Second item should have been processed"
    );
    assert!(
        results["third-item-should-process"]
            .get("allowed")
            .is_some(),
        "Third item should have been processed"
    );
}

/// Test: Batch check error codes distinguish validation vs internal errors
#[tokio::test]
async fn test_batch_check_error_code_classification() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/batch-check"),
        serde_json::json!({
            "checks": [
                {
                    "tuple_key": {
                        "user": "user:alice",
                        "relation": "nonexistent_relation",
                        "object": "document:readme"
                    },
                    "correlation_id": "validation-error-check"
                }
            ]
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let results = response["result"]
        .as_object()
        .expect("Should have result object");

    let error = results["validation-error-check"]["error"]
        .as_object()
        .expect("Should have error object");

    // Error code should be 400 for validation error
    assert_eq!(
        error["code"], 400,
        "Validation error in batch check should have code 400"
    );
}

// ============================================================================
// Section 4: Error Message Security (No Information Leakage)
// ============================================================================

/// Test: Error messages do not contain stack traces
#[tokio::test]
async fn test_error_messages_no_stack_traces() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (_, response) = post_json(
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

    let message = response["message"].as_str().unwrap_or("").to_lowercase();

    // Check for common stack trace indicators
    assert!(
        !message.contains("at line"),
        "Error message should not contain 'at line': {message}"
    );
    assert!(
        !message.contains("stack:"),
        "Error message should not contain 'stack:': {message}"
    );
    assert!(
        !message.contains("backtrace"),
        "Error message should not contain 'backtrace': {message}"
    );
    assert!(
        !message.contains(".rs:"),
        "Error message should not contain Rust source file references: {message}"
    );
    assert!(
        !message.contains("panicked"),
        "Error message should not contain 'panicked': {message}"
    );
}

/// Test: Error messages do not contain internal type names
#[tokio::test]
async fn test_error_messages_no_internal_type_names() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (_, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "nonexistent",
                "object": "document:readme"
            }
        }),
    )
    .await;

    let message = response["message"].as_str().unwrap_or("").to_lowercase();

    // Check for internal Rust type names
    assert!(
        !message.contains("domainerror"),
        "Error message should not contain 'DomainError': {message}"
    );
    assert!(
        !message.contains("resolvererror"),
        "Error message should not contain 'ResolverError': {message}"
    );
    assert!(
        !message.contains("storageerror"),
        "Error message should not contain 'StorageError': {message}"
    );
    assert!(
        !message.contains("arc<"),
        "Error message should not contain 'Arc<': {message}"
    );
    assert!(
        !message.contains("box<"),
        "Error message should not contain 'Box<': {message}"
    );
}

/// Test: Error messages do not contain file system paths
#[tokio::test]
async fn test_error_messages_no_file_paths() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (_, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "missing",
                "object": "document:readme"
            }
        }),
    )
    .await;

    let message = response["message"].as_str().unwrap_or("").to_lowercase();

    // Check for file system paths
    assert!(
        !message.contains("/home/"),
        "Error message should not contain '/home/': {message}"
    );
    assert!(
        !message.contains("/usr/"),
        "Error message should not contain '/usr/': {message}"
    );
    assert!(
        !message.contains("/var/"),
        "Error message should not contain '/var/': {message}"
    );
    assert!(
        !message.contains("c:\\"),
        "Error message should not contain 'C:\\': {message}"
    );
    assert!(
        !message.contains("/target/"),
        "Error message should not contain '/target/': {message}"
    );
}

/// Test: Error messages are helpful for debugging
#[tokio::test]
async fn test_error_messages_are_helpful() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Test relation not found message
    let (_, response) = post_json(
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

    let message = response["message"].as_str().unwrap_or("").to_lowercase();

    // Message should be helpful (mention what's wrong)
    assert!(
        message.contains("relation") || message.contains("not found"),
        "Error message should mention the issue: {message}"
    );
}

/// Test: Batch check error messages are sanitized
#[tokio::test]
async fn test_batch_check_error_messages_sanitized() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (_, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/batch-check"),
        serde_json::json!({
            "checks": [
                {
                    "tuple_key": {
                        "user": "user:alice",
                        "relation": "nonexistent",
                        "object": "document:readme"
                    },
                    "correlation_id": "check-1"
                }
            ]
        }),
    )
    .await;

    let results = response["result"].as_object().unwrap();
    let error = &results["check-1"]["error"];
    let message = error["message"].as_str().unwrap_or("").to_lowercase();

    // Batch error messages should also be sanitized
    assert!(
        !message.contains("domainerror"),
        "Batch error should not leak internal type names"
    );
    assert!(
        !message.contains(".rs:"),
        "Batch error should not leak source file references"
    );
}

// ============================================================================
// Section 5: Retryable vs Non-Retryable Error Distinction
// ============================================================================

/// Test: 400 errors indicate non-retryable client errors
#[tokio::test]
async fn test_400_errors_are_non_retryable() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Invalid input is non-retryable
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "invalid-format",
                "relation": "viewer",
                "object": "document:readme"
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(response["code"], "validation_error");

    // Client should NOT retry 400 errors - they need to fix the request
}

/// Test: 404 errors indicate non-retryable resource not found
#[tokio::test]
async fn test_404_errors_are_non_retryable() {
    let storage = Arc::new(MemoryDataStore::new());

    // Use a valid ULID format that doesn't exist in storage
    // (OpenFGA validates store ID format first - invalid format returns 400, valid format not found returns 404)
    let nonexistent_store = ulid::Ulid::new().to_string();

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{nonexistent_store}/check"),
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
    assert_eq!(response["code"], "store_id_not_found");

    // Client should NOT retry 404 errors - resource doesn't exist
}

// ============================================================================
// Section 6: Error Consistency Across Endpoints
// ============================================================================

/// Test: Same validation error returns same status across Check and Expand
///
/// Note: ListObjects has different behavior - it returns 200 with empty results
/// for unknown relations (like a query returning no matches). Check and Expand
/// validate relations and return 400 for unknown relations.
#[tokio::test]
async fn test_validation_error_consistent_across_check_and_expand() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Test Check endpoint with unknown relation
    let (check_status, check_response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "unknown_relation",
                "object": "document:readme"
            }
        }),
    )
    .await;

    // Test Expand endpoint with unknown relation
    let (expand_status, expand_response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/expand"),
        serde_json::json!({
            "tuple_key": {
                "relation": "unknown_relation",
                "object": "document:readme"
            }
        }),
    )
    .await;

    // Both should return 400
    assert_eq!(
        check_status,
        StatusCode::BAD_REQUEST,
        "Check should return 400"
    );
    assert_eq!(
        expand_status,
        StatusCode::BAD_REQUEST,
        "Expand should return 400"
    );

    // Both should use relation_not_found code for unknown relations
    assert_eq!(check_response["code"], "relation_not_found");
    assert_eq!(expand_response["code"], "relation_not_found");
}

/// Test: Store not found returns 404 consistently across all endpoints
#[tokio::test]
async fn test_store_not_found_consistent_across_endpoints() {
    let storage = Arc::new(MemoryDataStore::new());
    // Use a valid ULID format that doesn't exist in storage
    // (OpenFGA validates store ID format first - invalid format returns 400, valid format not found returns 404)
    let nonexistent_store = ulid::Ulid::new().to_string();

    // Test all endpoints
    let endpoints = vec![
        (
            format!("/stores/{nonexistent_store}/check"),
            serde_json::json!({
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:readme"
                }
            }),
        ),
        (
            format!("/stores/{nonexistent_store}/expand"),
            serde_json::json!({
                "tuple_key": {
                    "relation": "viewer",
                    "object": "document:readme"
                }
            }),
        ),
        (
            format!("/stores/{nonexistent_store}/list-objects"),
            serde_json::json!({
                "type": "document",
                "relation": "viewer",
                "user": "user:alice"
            }),
        ),
        (
            format!("/stores/{nonexistent_store}/batch-check"),
            serde_json::json!({
                "checks": [{
                    "tuple_key": {
                        "user": "user:alice",
                        "relation": "viewer",
                        "object": "document:readme"
                    },
                    "correlation_id": "check-1"
                }]
            }),
        ),
    ];

    for (uri, body) in endpoints {
        let (status, response) = post_json(create_test_app(&storage), &uri, body).await;

        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "Endpoint {uri} should return 404 for nonexistent store"
        );
        assert_eq!(
            response["code"], "store_id_not_found",
            "Endpoint {uri} should return store_id_not_found code"
        );
    }
}

// ============================================================================
// Section 7: gRPC Protocol Consistency
// ============================================================================

use rsfga_api::grpc::OpenFgaGrpcService;
use rsfga_api::proto::openfga::v1::open_fga_service_server::OpenFgaService;
use rsfga_api::proto::openfga::v1::*;
use rsfga_domain::cache::CheckCacheConfig;

/// Test: gRPC Check with unknown type returns INVALID_ARGUMENT
#[tokio::test]
async fn test_grpc_check_unknown_type_returns_invalid_argument() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;
    let service = create_grpc_service(&storage);

    let request = Request::new(CheckRequest {
        store_id: store_id.clone(),
        tuple_key: Some(TupleKey {
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "unknown_type:something".to_string(),
            condition: None,
        }),
        contextual_tuples: None,
        authorization_model_id: String::new(),
        trace: false,
        context: None,
        consistency: 0,
    });

    let result = service.check(request).await;

    assert!(result.is_err(), "Should return error for unknown type");
    let status = result.unwrap_err();
    assert_eq!(
        status.code(),
        tonic::Code::InvalidArgument,
        "Unknown type should return INVALID_ARGUMENT, got {:?}",
        status.code()
    );
}

/// Test: gRPC Check with unknown relation returns INVALID_ARGUMENT
#[tokio::test]
async fn test_grpc_check_unknown_relation_returns_invalid_argument() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;
    let service = create_grpc_service(&storage);

    let request = Request::new(CheckRequest {
        store_id: store_id.clone(),
        tuple_key: Some(TupleKey {
            user: "user:alice".to_string(),
            relation: "unknown_relation".to_string(),
            object: "document:readme".to_string(),
            condition: None,
        }),
        contextual_tuples: None,
        authorization_model_id: String::new(),
        trace: false,
        context: None,
        consistency: 0,
    });

    let result = service.check(request).await;

    assert!(result.is_err(), "Should return error for unknown relation");
    let status = result.unwrap_err();
    assert_eq!(
        status.code(),
        tonic::Code::InvalidArgument,
        "Unknown relation should return INVALID_ARGUMENT, got {:?}",
        status.code()
    );
}

/// Test: gRPC Check with nonexistent store returns NOT_FOUND
#[tokio::test]
async fn test_grpc_check_nonexistent_store_returns_not_found() {
    let storage = Arc::new(MemoryDataStore::new());
    let service = create_grpc_service(&storage);

    // Use a valid ULID format that doesn't exist in storage
    // (OpenFGA validates store ID format first - invalid format returns INVALID_ARGUMENT,
    // valid format not found returns NOT_FOUND)
    let nonexistent_store = ulid::Ulid::new().to_string();

    let request = Request::new(CheckRequest {
        store_id: nonexistent_store,
        tuple_key: Some(TupleKey {
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:readme".to_string(),
            condition: None,
        }),
        contextual_tuples: None,
        authorization_model_id: String::new(),
        trace: false,
        context: None,
        consistency: 0,
    });

    let result = service.check(request).await;

    assert!(result.is_err(), "Should return error for nonexistent store");
    let status = result.unwrap_err();
    assert_eq!(
        status.code(),
        tonic::Code::NotFound,
        "Nonexistent store should return NOT_FOUND, got {:?}",
        status.code()
    );
}

/// Test: gRPC BatchCheck with partial failures returns success with per-item errors
#[tokio::test]
async fn test_grpc_batch_check_partial_failures() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;
    let service = create_grpc_service(&storage);

    let request = Request::new(BatchCheckRequest {
        store_id: store_id.clone(),
        checks: vec![
            BatchCheckItem {
                tuple_key: Some(TupleKey {
                    user: "user:alice".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:readme".to_string(),
                    condition: None,
                }),
                correlation_id: "valid-check".to_string(),
                contextual_tuples: None,
                context: None,
            },
            BatchCheckItem {
                tuple_key: Some(TupleKey {
                    user: "user:bob".to_string(),
                    relation: "unknown_relation".to_string(),
                    object: "document:readme".to_string(),
                    condition: None,
                }),
                correlation_id: "invalid-check".to_string(),
                contextual_tuples: None,
                context: None,
            },
        ],
        ..Default::default()
    });

    let result = service.batch_check(request).await;

    // Batch check should succeed overall
    assert!(
        result.is_ok(),
        "Batch check with partial failures should succeed overall"
    );

    let response = result.unwrap().into_inner();
    assert_eq!(
        response.result.len(),
        2,
        "Should have results for both items"
    );

    // Valid check should have allowed field
    let valid_result = response.result.get("valid-check").unwrap();
    assert!(
        valid_result.error.is_none(),
        "Valid check should not have error"
    );

    // Invalid check should have error
    let invalid_result = response.result.get("invalid-check").unwrap();
    assert!(
        invalid_result.error.is_some(),
        "Invalid check should have error"
    );
}

/// Test: gRPC error messages are sanitized
#[tokio::test]
async fn test_grpc_error_messages_sanitized() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;
    let service = create_grpc_service(&storage);

    let request = Request::new(CheckRequest {
        store_id: store_id.clone(),
        tuple_key: Some(TupleKey {
            user: "user:alice".to_string(),
            relation: "nonexistent_relation".to_string(),
            object: "document:readme".to_string(),
            condition: None,
        }),
        contextual_tuples: None,
        authorization_model_id: String::new(),
        trace: false,
        context: None,
        consistency: 0,
    });

    let result = service.check(request).await;
    let status = result.unwrap_err();
    let message = status.message().to_lowercase();

    // gRPC error messages should also be sanitized
    assert!(
        !message.contains("domainerror"),
        "gRPC error should not leak internal type names: {message}"
    );
    assert!(
        !message.contains(".rs:"),
        "gRPC error should not leak source file references: {message}"
    );
    assert!(
        !message.contains("panicked"),
        "gRPC error should not contain panic info: {message}"
    );
}

/// Test: HTTP and gRPC return equivalent error classifications
#[tokio::test]
async fn test_http_grpc_error_classification_equivalence() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Test HTTP endpoint
    let (http_status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "unknown_relation",
                "object": "document:readme"
            }
        }),
    )
    .await;

    // Test gRPC endpoint
    let service = create_grpc_service(&storage);
    let grpc_result = service
        .check(Request::new(CheckRequest {
            store_id: store_id.clone(),
            tuple_key: Some(TupleKey {
                user: "user:alice".to_string(),
                relation: "unknown_relation".to_string(),
                object: "document:readme".to_string(),
                condition: None,
            }),
            contextual_tuples: None,
            authorization_model_id: String::new(),
            trace: false,
            context: None,
            consistency: 0,
        }))
        .await;

    // HTTP 400 Bad Request should map to gRPC InvalidArgument
    assert_eq!(http_status, StatusCode::BAD_REQUEST);
    assert_eq!(
        grpc_result.unwrap_err().code(),
        tonic::Code::InvalidArgument
    );
}

// ============================================================================
// Section 8: Additional Security Tests (Connection Strings)
// ============================================================================

/// Test: Error messages do not contain connection strings
#[tokio::test]
async fn test_error_messages_no_connection_strings() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (_, response) = post_json(
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

    let message = response["message"].as_str().unwrap_or("").to_lowercase();

    // Check for connection string patterns
    assert!(
        !message.contains("postgres://"),
        "Error message should not contain postgres connection strings: {message}"
    );
    assert!(
        !message.contains("mysql://"),
        "Error message should not contain mysql connection strings: {message}"
    );
    assert!(
        !message.contains("mongodb://"),
        "Error message should not contain mongodb connection strings: {message}"
    );
    assert!(
        !message.contains("redis://"),
        "Error message should not contain redis connection strings: {message}"
    );
    assert!(
        !message.contains("@localhost"),
        "Error message should not contain host credentials: {message}"
    );
    assert!(
        !message.contains("password="),
        "Error message should not contain password parameters: {message}"
    );
    assert!(
        !message.contains("user="),
        "Error message should not contain user parameters: {message}"
    );
}

/// Test: Batch check error messages do not contain connection strings
#[tokio::test]
async fn test_batch_check_error_messages_no_connection_strings() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (_, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/batch-check"),
        serde_json::json!({
            "checks": [
                {
                    "tuple_key": {
                        "user": "user:alice",
                        "relation": "nonexistent",
                        "object": "document:readme"
                    },
                    "correlation_id": "check-1"
                }
            ]
        }),
    )
    .await;

    let results = response["result"].as_object().unwrap();
    let error = &results["check-1"]["error"];
    let message = error["message"].as_str().unwrap_or("").to_lowercase();

    // Check for connection string patterns in batch error messages
    assert!(
        !message.contains("postgres://"),
        "Batch error should not contain postgres connection strings"
    );
    assert!(
        !message.contains("mysql://"),
        "Batch error should not contain mysql connection strings"
    );
    assert!(
        !message.contains("password="),
        "Batch error should not contain password parameters"
    );
}

// ============================================================================
// Section 9: Error Code Verification Tests
// ============================================================================

/// Test: Verify the error code structure matches expected format
#[tokio::test]
async fn test_error_response_has_code_and_message() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "invalid-format",
                "relation": "viewer",
                "object": "document:readme"
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Error response should have both 'code' and 'message' fields
    assert!(
        response.get("code").is_some(),
        "Error response should have 'code' field: {response}"
    );
    assert!(
        response.get("message").is_some(),
        "Error response should have 'message' field: {response}"
    );

    // Code should be a string
    assert!(
        response["code"].is_string(),
        "Error code should be a string: {response}"
    );
}

/// Test: gRPC error does not contain connection strings
#[tokio::test]
async fn test_grpc_error_no_connection_strings() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;
    let service = create_grpc_service(&storage);

    let request = Request::new(CheckRequest {
        store_id: store_id.clone(),
        tuple_key: Some(TupleKey {
            user: "user:alice".to_string(),
            relation: "nonexistent_relation".to_string(),
            object: "document:readme".to_string(),
            condition: None,
        }),
        contextual_tuples: None,
        authorization_model_id: String::new(),
        trace: false,
        context: None,
        consistency: 0,
    });

    let result = service.check(request).await;
    let status = result.unwrap_err();
    let message = status.message().to_lowercase();

    // gRPC errors should also not contain connection strings
    assert!(
        !message.contains("postgres://"),
        "gRPC error should not contain postgres connection strings: {message}"
    );
    assert!(
        !message.contains("mysql://"),
        "gRPC error should not contain mysql connection strings: {message}"
    );
    assert!(
        !message.contains("password="),
        "gRPC error should not contain password parameters: {message}"
    );
}

/// Test: Write operation with invalid data returns proper error
#[tokio::test]
async fn test_write_invalid_tuple_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [
                    {
                        "user": "invalid-user-format",
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
        StatusCode::BAD_REQUEST,
        "Write with invalid tuple should return 400"
    );
    assert_eq!(response["code"], "validation_error");
}

/// Test: Write to unknown type returns 400 Bad Request
///
/// OpenFGA validates that types exist in the authorization model at write time.
/// Tuples referencing undefined types are rejected.
#[tokio::test]
async fn test_write_unknown_type_rejected() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [
                    {
                        "user": "user:alice",
                        "relation": "viewer",
                        "object": "unknown_type:something"
                    }
                ]
            }
        }),
    )
    .await;

    // Write rejects tuples for unknown types
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Write to unknown type should return 400 Bad Request"
    );
    assert_eq!(response["code"], "validation_error");
}

/// Test: Write with unknown relation returns 400 Bad Request
///
/// OpenFGA validates that relations exist in the authorization model at write time.
/// Tuples referencing undefined relations are rejected.
#[tokio::test]
async fn test_write_unknown_relation_rejected() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [
                    {
                        "user": "user:alice",
                        "relation": "unknown_relation",
                        "object": "document:readme"
                    }
                ]
            }
        }),
    )
    .await;

    // Write rejects tuples with unknown relations
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Write with unknown relation should return 400 Bad Request"
    );
    assert_eq!(response["code"], "validation_error");
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Create a gRPC service for testing
fn create_grpc_service(storage: &Arc<MemoryDataStore>) -> OpenFgaGrpcService<MemoryDataStore> {
    let cache_config = CheckCacheConfig::default().with_enabled(true);
    OpenFgaGrpcService::with_cache_config(Arc::clone(storage), cache_config)
}
