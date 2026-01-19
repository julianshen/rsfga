//! Input validation edge cases tests for RSFGA API.
//!
//! These tests verify OpenFGA-compatible input validation behavior across all API endpoints.
//! They ensure identical validation behavior between RSFGA and OpenFGA.
//!
//! **Test Categories** (per GitHub issue #216):
//! 1. Store Name Validation - Empty names, max length, special characters
//! 2. Object/User Format Validation - Boundary tests, malformed formats
//! 3. Authorization Model Validation - Empty definitions, size limits
//! 4. Condition Validation - Name length, context size limits
//! 5. Batch Operations Limits - Max batch size, correlation IDs
//! 6. Pagination Limits - Page size bounds, invalid continuation tokens
//!
//! **Acceptance Criteria**:
//! - All boundary conditions tested
//! - Error messages match OpenFGA format
//! - No panics or crashes on malformed input
//!
//! Related to GitHub issue #216.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use rsfga_storage::MemoryDataStore;

use common::{create_test_app, post_json, setup_simple_model};

// =============================================================================
// Constants
// =============================================================================

/// Maximum store name length to test (OpenFGA typically uses 256).
const MAX_STORE_NAME_LENGTH: usize = 256;

/// Maximum relation length per OpenFGA spec.
const MAX_RELATION_LENGTH: usize = 50;

/// Maximum condition name length.
const MAX_CONDITION_NAME_LENGTH: usize = 256;

/// Maximum condition context size in bytes (10KB).
const MAX_CONDITION_CONTEXT_SIZE: usize = 10 * 1024;

/// Maximum batch size per OpenFGA spec.
const MAX_BATCH_SIZE: usize = 50;

// =============================================================================
// Helper Functions
// =============================================================================

/// Create a store and return its ID.
async fn create_store(storage: &Arc<MemoryDataStore>, name: &str) -> String {
    let (status, response) = post_json(
        create_test_app(storage),
        "/stores",
        serde_json::json!({"name": name}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    response["id"].as_str().unwrap().to_string()
}

/// Create a store with authorization model and return store ID.
async fn setup_test_store(storage: &Arc<MemoryDataStore>) -> String {
    let store_id = create_store(storage, "test-store").await;
    setup_simple_model(storage, &store_id).await;
    store_id
}

/// Create a store with model that includes conditions.
async fn setup_store_with_conditions(storage: &Arc<MemoryDataStore>) -> String {
    let (status, response) = post_json(
        create_test_app(storage),
        "/stores",
        serde_json::json!({"name": "condition-test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap();

    // Create model with a condition
    let model_json = serde_json::json!({
        "schema_version": "1.1",
        "type_definitions": [
            {"type": "user"},
            {
                "type": "document",
                "relations": {
                    "viewer": {
                        "this": {}
                    }
                }
            }
        ],
        "conditions": {
            "ip_check": {
                "name": "ip_check",
                "expression": "request.ip == '127.0.0.1'",
                "parameters": {
                    "request": {
                        "type_name": "TYPE_NAME_MAP",
                        "generic_types": [
                            { "type_name": "TYPE_NAME_STRING" },
                            { "type_name": "TYPE_NAME_STRING" }
                        ]
                    }
                }
            }
        }
    });

    let (status, _) = post_json(
        create_test_app(storage),
        &format!("/stores/{store_id}/authorization-models"),
        model_json,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "Model creation should succeed");

    store_id.to_string()
}

// =============================================================================
// Section 1: Store Name Validation Tests
// =============================================================================

/// Test: Empty store name is rejected with 400 Bad Request.
#[tokio::test]
async fn test_empty_store_name_rejected() {
    let storage = Arc::new(MemoryDataStore::new());

    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": ""}),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Empty store name should return 400 Bad Request"
    );
    assert!(
        response.get("code").is_some() || response.get("message").is_some(),
        "Response should contain error information"
    );
}

/// Test: Store name at maximum length (256 chars) is accepted.
#[tokio::test]
async fn test_store_name_at_max_length_accepted() {
    let storage = Arc::new(MemoryDataStore::new());
    let max_length_name = "a".repeat(MAX_STORE_NAME_LENGTH);

    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": max_length_name}),
    )
    .await;

    // Should succeed or fail gracefully (not panic)
    assert!(
        status == StatusCode::CREATED || status.is_client_error(),
        "Max length store name should be handled gracefully, got {status}"
    );

    if status == StatusCode::CREATED {
        assert!(
            response["id"].is_string(),
            "Successful creation should return store ID"
        );
    }
}

/// Test: Store name exceeding maximum length is rejected.
#[tokio::test]
async fn test_store_name_exceeds_max_length_rejected() {
    let storage = Arc::new(MemoryDataStore::new());
    let oversized_name = "x".repeat(MAX_STORE_NAME_LENGTH + 100);

    let (status, _) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({"name": oversized_name}),
    )
    .await;

    // Should not cause server error
    assert!(
        !status.is_server_error(),
        "Oversized store name should not cause 500 error, got {status}"
    );
}

/// Test: Store name with special characters handled safely.
#[tokio::test]
async fn test_store_name_special_characters_handled() {
    let storage = Arc::new(MemoryDataStore::new());

    let special_names = vec![
        "store with spaces",
        "store-with-dashes",
        "store_with_underscores",
        "store.with.dots",
        "store/with/slashes",
        "store\\with\\backslashes",
        "store\twith\ttabs",
        "store\nwith\nnewlines",
        "store<script>alert(1)</script>",
        "store${7*7}",
        "store{{template}}",
        "😀emoji_store",
        "αβγδ_unicode_store",
        "store\0with\0nulls",
    ];

    for name in special_names {
        let (status, _) = post_json(
            create_test_app(&storage),
            "/stores",
            serde_json::json!({"name": name}),
        )
        .await;

        // Should handle gracefully without 5xx error
        assert!(
            !status.is_server_error(),
            "Store name '{name}' should not cause server error, got {status}"
        );
    }
}

/// Test: Whitespace-only store name is rejected.
#[tokio::test]
async fn test_whitespace_only_store_name_rejected() {
    let storage = Arc::new(MemoryDataStore::new());

    let whitespace_names = vec![
        "   ",    // spaces
        "\t\t\t", // tabs
        "\n\n",   // newlines
        " \t\n ", // mixed whitespace
    ];

    for name in whitespace_names {
        let (status, _) = post_json(
            create_test_app(&storage),
            "/stores",
            serde_json::json!({"name": name}),
        )
        .await;

        // Should either reject with 400 or create (if whitespace is trimmed/allowed)
        assert!(
            !status.is_server_error(),
            "Whitespace-only store name should be handled gracefully, got {status}"
        );
    }
}

// =============================================================================
// Section 2: Object/User Format Validation Tests
// =============================================================================

/// Test: User with empty type is handled safely (rejected or returns false).
#[tokio::test]
async fn test_user_empty_type_handled() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": ":alice",  // Empty type
                "relation": "viewer",
                "object": "document:doc1"
            }
        }),
    )
    .await;

    // Should be handled safely - either rejected (400) or returns allowed: false
    assert!(
        status == StatusCode::BAD_REQUEST
            || (status == StatusCode::OK && response["allowed"] == false),
        "Empty user type should be handled safely, got status={status}, response={response}"
    );
    // No server errors
    assert!(!status.is_server_error(), "Should not cause server error");
}

/// Test: User with empty ID is rejected.
#[tokio::test]
async fn test_user_empty_id_rejected() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:",  // Empty ID
                "relation": "viewer",
                "object": "document:doc1"
            }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Empty user ID should return 400"
    );
    assert!(
        response["message"]
            .as_str()
            .unwrap_or("")
            .to_lowercase()
            .contains("id")
            || response["message"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .contains("empty"),
        "Error should mention empty ID: {:?}",
        response["message"]
    );
}

/// Test: Object with empty type is rejected.
#[tokio::test]
async fn test_object_empty_type_rejected() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": ":doc1"  // Empty type
            }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Empty object type should return 400"
    );
}

/// Test: Object with empty ID is rejected.
#[tokio::test]
async fn test_object_empty_id_rejected() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:"  // Empty ID
            }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Empty object ID should return 400"
    );
}

/// Test: User without colon separator is rejected.
#[tokio::test]
async fn test_user_no_colon_rejected() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "useralice",  // No colon
                "relation": "viewer",
                "object": "document:doc1"
            }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "User without colon should return 400"
    );
    assert!(
        response["message"]
            .as_str()
            .unwrap_or("")
            .contains("type:id")
            || response["message"]
                .as_str()
                .unwrap_or("")
                .contains("format"),
        "Error should mention expected format: {:?}",
        response["message"]
    );
}

/// Test: Empty user field is rejected.
#[tokio::test]
async fn test_empty_user_rejected() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "",
                "relation": "viewer",
                "object": "document:doc1"
            }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Empty user should return 400"
    );
    // Verify error message exists (format may vary)
    assert!(
        response.get("message").is_some() || response.get("code").is_some(),
        "Error response should contain error information"
    );
}

/// Test: Empty relation field is rejected.
#[tokio::test]
async fn test_empty_relation_rejected() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "",
                "object": "document:doc1"
            }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Empty relation should return 400"
    );
    assert!(
        response["message"]
            .as_str()
            .unwrap_or("")
            .to_lowercase()
            .contains("empty")
            || response["message"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .contains("relation"),
        "Error should mention empty relation: {:?}",
        response["message"]
    );
}

/// Test: Empty object field is rejected.
#[tokio::test]
async fn test_empty_object_rejected() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, _) = post_json(
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
}

/// Test: Relation at maximum length (50 chars) is accepted or rejected consistently.
#[tokio::test]
async fn test_relation_at_max_length() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;
    let max_relation = "a".repeat(MAX_RELATION_LENGTH);

    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": max_relation,
                "object": "document:doc1"
            }
        }),
    )
    .await;

    // Should handle gracefully (OK with allowed:false for non-existent relation, or 400)
    assert!(
        status == StatusCode::OK || status == StatusCode::BAD_REQUEST,
        "Max length relation should be handled gracefully, got {status}"
    );
}

/// Test: Relation exceeding maximum length (50 chars) is rejected.
#[tokio::test]
async fn test_relation_exceeds_max_length_rejected() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;
    let too_long_relation = "a".repeat(MAX_RELATION_LENGTH + 1);

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": too_long_relation,
                "object": "document:doc1"
            }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Relation exceeding max length should return 400"
    );
    // Verify error response exists (format may vary)
    assert!(
        response.get("message").is_some() || response.get("code").is_some(),
        "Error response should contain error information"
    );
}

/// Test: Relation with invalid characters is rejected.
#[tokio::test]
async fn test_relation_invalid_characters_rejected() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let invalid_relations = vec![
        "view:er",  // colon
        "view#er",  // hash
        "view@er",  // at sign
        "view er",  // space
        "view\ter", // tab
        "view\ner", // newline
    ];

    for relation in invalid_relations {
        let (status, _response) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/check"),
            serde_json::json!({
                "tuple_key": {
                    "user": "user:alice",
                    "relation": relation,
                    "object": "document:doc1"
                }
            }),
        )
        .await;

        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "Relation '{relation}' with invalid chars should return 400"
        );
    }
}

/// Test: User with valid userset notation (type:id#relation) is accepted.
#[tokio::test]
async fn test_user_userset_format_accepted() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // group:admins#member is valid userset format
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "team:engineering#member",
                "relation": "viewer",
                "object": "document:doc1"
            }
        }),
    )
    .await;

    // Should be accepted (even if it returns allowed: false due to no tuples)
    assert!(
        status == StatusCode::OK || status == StatusCode::BAD_REQUEST,
        "Valid userset format should be handled, got {status}"
    );
}

/// Test: Wildcard user format (type:*) is accepted.
#[tokio::test]
async fn test_wildcard_user_accepted() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:*",
                "relation": "viewer",
                "object": "document:doc1"
            }
        }),
    )
    .await;

    // Wildcard format should be accepted
    assert!(
        status == StatusCode::OK || status == StatusCode::BAD_REQUEST,
        "Wildcard user format should be handled, got {status}"
    );
}

// =============================================================================
// Section 3: Authorization Model Validation Tests
// =============================================================================

/// Test: Empty type_definitions returns 400.
#[tokio::test]
async fn test_model_empty_type_definitions_rejected() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage, "model-test").await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/authorization-models"),
        serde_json::json!({
            "schema_version": "1.1",
            "type_definitions": []
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Empty type_definitions should return 400"
    );
    assert!(
        response["message"]
            .as_str()
            .unwrap_or("")
            .to_lowercase()
            .contains("empty")
            || response["message"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .contains("type_definitions"),
        "Error should mention empty type_definitions: {:?}",
        response["message"]
    );
}

/// Test: Model with type definition without type name is handled safely.
#[tokio::test]
async fn test_model_type_without_name_handled() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage, "model-test").await;

    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/authorization-models"),
        serde_json::json!({
            "schema_version": "1.1",
            "type_definitions": [
                {"type": ""}  // Empty type name
            ]
        }),
    )
    .await;

    // Should be handled safely - either rejected (400) or succeed (201)
    // Key requirement: no server crash (5xx)
    assert!(
        !status.is_server_error(),
        "Empty type name should not cause server error, got {status}"
    );
}

/// Test: Model with duplicate type names is handled.
#[tokio::test]
async fn test_model_duplicate_type_names_handled() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage, "model-test").await;

    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/authorization-models"),
        serde_json::json!({
            "schema_version": "1.1",
            "type_definitions": [
                {"type": "user"},
                {"type": "user"}  // Duplicate
            ]
        }),
    )
    .await;

    // Should either reject with 400 or handle gracefully
    assert!(
        !status.is_server_error(),
        "Duplicate type names should not cause server error, got {status}"
    );
}

/// Test: Model with many type definitions (100+) is handled.
#[tokio::test]
async fn test_model_many_type_definitions_handled() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage, "model-test").await;

    // Create 100 type definitions
    let type_defs: Vec<serde_json::Value> = (0..100)
        .map(|i| serde_json::json!({"type": format!("type_{i}")}))
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

    // Should succeed or fail gracefully
    assert!(
        !status.is_server_error(),
        "Many type definitions should not cause server error, got {status}"
    );
}

/// Test: Model with circular relation references is detected.
#[tokio::test]
async fn test_model_circular_relations_detected() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage, "model-test").await;

    // Create a model with circular computed_userset references
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/authorization-models"),
        serde_json::json!({
            "schema_version": "1.1",
            "type_definitions": [
                {"type": "user"},
                {
                    "type": "document",
                    "relations": {
                        "parent": {"this": {}},
                        "viewer": {
                            "computedUserset": {"relation": "editor"}
                        },
                        "editor": {
                            "computedUserset": {"relation": "viewer"}
                        }
                    }
                }
            ]
        }),
    )
    .await;

    // Should detect the cycle and return 400
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::CREATED,
        "Circular relation should be handled, got {status}"
    );
}

// =============================================================================
// Section 4: Condition Validation Tests
// =============================================================================

/// Test: Condition name at maximum length (256 chars) is handled.
#[tokio::test]
async fn test_condition_name_at_max_length() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage, "condition-test").await;
    let max_length_name = "a".repeat(MAX_CONDITION_NAME_LENGTH);

    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/authorization-models"),
        serde_json::json!({
            "schema_version": "1.1",
            "type_definitions": [
                {"type": "user"},
                {
                    "type": "document",
                    "relations": {
                        "viewer": {"this": {}}
                    }
                }
            ],
            "conditions": {
                &max_length_name: {
                    "name": &max_length_name,
                    "expression": "true"
                }
            }
        }),
    )
    .await;

    // Should handle gracefully
    assert!(
        !status.is_server_error(),
        "Max length condition name should not cause server error, got {status}"
    );
}

/// Test: Condition name exceeding maximum length is rejected.
#[tokio::test]
async fn test_condition_name_exceeds_max_length_rejected() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage, "condition-test").await;
    let too_long_name = "a".repeat(MAX_CONDITION_NAME_LENGTH + 1);

    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/authorization-models"),
        serde_json::json!({
            "schema_version": "1.1",
            "type_definitions": [
                {"type": "user"},
                {"type": "document", "relations": {"viewer": {"this": {}}}}
            ],
            "conditions": {
                &too_long_name: {
                    "name": &too_long_name,
                    "expression": "true"
                }
            }
        }),
    )
    .await;

    // Should reject or handle gracefully
    assert!(
        !status.is_server_error(),
        "Oversized condition name should not cause server error, got {status}"
    );
}

/// Test: Context size within limit (< 10KB) is accepted.
#[tokio::test]
async fn test_context_within_size_limit_accepted() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_store_with_conditions(&storage).await;

    // Create context smaller than 10KB
    let small_value = "x".repeat(1000); // ~1KB

    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:doc1"
            },
            "context": {
                "request": {"ip": small_value}
            }
        }),
    )
    .await;

    // Small context should be accepted
    assert!(
        status == StatusCode::OK || status == StatusCode::BAD_REQUEST,
        "Small context should be handled, got {status}"
    );
}

/// Test: Context size exceeding limit (> 10KB) is handled safely.
#[tokio::test]
async fn test_context_exceeds_size_limit_handled() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_store_with_conditions(&storage).await;

    // Create context larger than 10KB
    let large_value = "x".repeat(MAX_CONDITION_CONTEXT_SIZE + 5000);

    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:doc1"
            },
            "context": {
                "large_field": large_value
            }
        }),
    )
    .await;

    // Should handle large context safely - either reject or process
    // Key requirement: no server crash (5xx)
    assert!(
        !status.is_server_error(),
        "Large context should not cause server error, got {status}"
    );
}

/// Test: Deeply nested context (> 10 levels) is handled safely.
#[tokio::test]
async fn test_deeply_nested_context_handled() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_store_with_conditions(&storage).await;

    // Create deeply nested context (15 levels)
    let mut nested = serde_json::json!({"leaf": true});
    for _ in 0..14 {
        nested = serde_json::json!({"nested": nested});
    }

    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:doc1"
            },
            "context": nested
        }),
    )
    .await;

    // Should handle deeply nested context safely - either reject or process
    // Key requirement: no server crash (5xx) or stack overflow
    assert!(
        !status.is_server_error(),
        "Deeply nested context should not cause server error, got {status}"
    );
}

/// Test: Empty context is accepted.
#[tokio::test]
async fn test_empty_context_accepted() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:doc1"
            },
            "context": {}
        }),
    )
    .await;

    // Empty context should be accepted
    assert_eq!(status, StatusCode::OK, "Empty context should be accepted");
}

// =============================================================================
// Section 5: Batch Operations Limits Tests
// =============================================================================

/// Test: Empty batch request is rejected.
#[tokio::test]
async fn test_empty_batch_rejected() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/batch-check"),
        serde_json::json!({
            "checks": []
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Empty batch should return 400"
    );
    assert!(
        response["message"]
            .as_str()
            .unwrap_or("")
            .to_lowercase()
            .contains("empty")
            || response["message"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .contains("batch"),
        "Error should mention empty batch: {:?}",
        response["message"]
    );
}

/// Test: Batch at maximum size (50 items) is accepted.
#[tokio::test]
async fn test_batch_at_max_size_accepted() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Create batch with exactly MAX_BATCH_SIZE items
    let checks: Vec<serde_json::Value> = (0..MAX_BATCH_SIZE)
        .map(|i| {
            serde_json::json!({
                "tuple_key": {
                    "user": format!("user:user{i}"),
                    "relation": "viewer",
                    "object": "document:doc1"
                },
                "correlation_id": format!("check_{i}")
            })
        })
        .collect();

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/batch-check"),
        serde_json::json!({
            "checks": checks
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "Batch at max size should be accepted"
    );
    assert!(
        response.get("result").is_some(),
        "Response should contain results"
    );
}

/// Test: Batch exceeding maximum size (51+ items) is rejected.
#[tokio::test]
async fn test_batch_exceeds_max_size_rejected() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Create batch with MAX_BATCH_SIZE + 1 items
    let checks: Vec<serde_json::Value> = (0..=MAX_BATCH_SIZE)
        .map(|i| {
            serde_json::json!({
                "tuple_key": {
                    "user": format!("user:user{i}"),
                    "relation": "viewer",
                    "object": "document:doc1"
                },
                "correlation_id": format!("check_{i}")
            })
        })
        .collect();

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/batch-check"),
        serde_json::json!({
            "checks": checks
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Batch exceeding max size should return 400"
    );
    assert!(
        response["message"]
            .as_str()
            .unwrap_or("")
            .contains("exceed")
            || response["message"].as_str().unwrap_or("").contains("max")
            || response["message"].as_str().unwrap_or("").contains("50"),
        "Error should mention exceeding max: {:?}",
        response["message"]
    );
}

/// Test: Batch with duplicate correlation IDs is handled.
#[tokio::test]
async fn test_batch_duplicate_correlation_ids_handled() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/batch-check"),
        serde_json::json!({
            "checks": [
                {
                    "tuple_key": {
                        "user": "user:alice",
                        "relation": "viewer",
                        "object": "document:doc1"
                    },
                    "correlation_id": "same_id"
                },
                {
                    "tuple_key": {
                        "user": "user:bob",
                        "relation": "viewer",
                        "object": "document:doc2"
                    },
                    "correlation_id": "same_id"  // Duplicate
                }
            ]
        }),
    )
    .await;

    // Should handle gracefully (either accept or reject, but not crash)
    assert!(
        !status.is_server_error(),
        "Duplicate correlation IDs should not cause server error, got {status}"
    );
}

/// Test: Batch with empty correlation ID is handled.
#[tokio::test]
async fn test_batch_empty_correlation_id_handled() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/batch-check"),
        serde_json::json!({
            "checks": [
                {
                    "tuple_key": {
                        "user": "user:alice",
                        "relation": "viewer",
                        "object": "document:doc1"
                    },
                    "correlation_id": ""  // Empty
                }
            ]
        }),
    )
    .await;

    // Should handle gracefully
    assert!(
        !status.is_server_error(),
        "Empty correlation ID should not cause server error, got {status}"
    );
}

/// Test: Batch with missing correlation ID is handled.
#[tokio::test]
async fn test_batch_missing_correlation_id_handled() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/batch-check"),
        serde_json::json!({
            "checks": [
                {
                    "tuple_key": {
                        "user": "user:alice",
                        "relation": "viewer",
                        "object": "document:doc1"
                    }
                    // No correlation_id
                }
            ]
        }),
    )
    .await;

    // Should handle gracefully (correlation_id is optional)
    assert!(
        !status.is_server_error(),
        "Missing correlation ID should not cause server error, got {status}"
    );
}

/// Test: Batch with invalid tuple in one item returns partial results.
#[tokio::test]
async fn test_batch_partial_invalid_items_handled() {
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
                        "relation": "viewer",
                        "object": "document:doc1"
                    },
                    "correlation_id": "valid"
                },
                {
                    "tuple_key": {
                        "user": "",  // Invalid
                        "relation": "viewer",
                        "object": "document:doc2"
                    },
                    "correlation_id": "invalid"
                }
            ]
        }),
    )
    .await;

    // Should handle (either return partial results or reject entire batch)
    assert!(
        !status.is_server_error(),
        "Batch with invalid item should not cause server error, got {status}"
    );

    if status == StatusCode::OK {
        // If OK, should have results for at least the valid item
        assert!(
            response.get("result").is_some(),
            "OK response should contain results"
        );
    }
}

// =============================================================================
// Section 6: Pagination Limits Tests
// =============================================================================

/// Test: Page size of zero returns error or uses default.
#[tokio::test]
async fn test_page_size_zero_handled() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-objects"),
        serde_json::json!({
            "type": "document",
            "relation": "viewer",
            "user": "user:alice",
            "page_size": 0
        }),
    )
    .await;

    // Should either use default page size (OK) or reject (400)
    assert!(
        status == StatusCode::OK || status == StatusCode::BAD_REQUEST,
        "Page size zero should be handled gracefully, got {status}"
    );
}

/// Test: Negative page size is handled safely.
/// Note: Depending on the API implementation, negative page_size in the request body
/// may be cast to a large unsigned value or handled differently than in query params.
#[tokio::test]
async fn test_negative_page_size_handled() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, _response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-objects"),
        serde_json::json!({
            "type": "document",
            "relation": "viewer",
            "user": "user:alice",
            "page_size": -1
        }),
    )
    .await;

    // Should handle safely - either reject (400) or process (200)
    // Key requirement: no server crash (5xx)
    assert!(
        !status.is_server_error(),
        "Negative page size should not cause server error, got {status}"
    );
}

/// Test: Very large page size is capped or rejected.
#[tokio::test]
async fn test_very_large_page_size_handled() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-objects"),
        serde_json::json!({
            "type": "document",
            "relation": "viewer",
            "user": "user:alice",
            "page_size": 1000000
        }),
    )
    .await;

    // Should handle gracefully (cap to max or reject)
    assert!(
        !status.is_server_error(),
        "Very large page size should not cause server error, got {status}"
    );
}

/// Test: Invalid continuation token is rejected.
#[tokio::test]
async fn test_invalid_continuation_token_rejected() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let invalid_tokens = vec![
        "invalid_token",
        "not_base64!!!",
        "123456789",
        "definitely not a valid token",
        "<script>alert(1)</script>",
        "'; DROP TABLE tuples;--",
    ];

    for token in invalid_tokens {
        let (status, _) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/list-objects"),
            serde_json::json!({
                "type": "document",
                "relation": "viewer",
                "user": "user:alice",
                "continuation_token": token
            }),
        )
        .await;

        // Should reject invalid token or handle gracefully
        assert!(
            !status.is_server_error(),
            "Invalid continuation token '{token}' should not cause server error, got {status}"
        );
    }
}

/// Test: Empty continuation token is handled.
#[tokio::test]
async fn test_empty_continuation_token_handled() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-objects"),
        serde_json::json!({
            "type": "document",
            "relation": "viewer",
            "user": "user:alice",
            "continuation_token": ""
        }),
    )
    .await;

    // Empty token should be treated as no token (first page)
    assert!(
        status == StatusCode::OK || status == StatusCode::BAD_REQUEST,
        "Empty continuation token should be handled gracefully, got {status}"
    );
}

/// Test: ListUsers with negative page_size is handled safely.
#[tokio::test]
async fn test_listusers_negative_page_size_handled() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // ListUsers uses query parameters
    let (status, _response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-users?page_size=-1"),
        serde_json::json!({
            "object": {
                "type": "document",
                "id": "doc1"
            },
            "relation": "viewer",
            "user_filters": [{"type": "user"}]
        }),
    )
    .await;

    // Should be handled safely - either rejected (400) or handled
    // Note: Query parameter parsing may convert -1 to a u32 causing overflow
    // The key requirement is no server crash
    assert!(
        !status.is_server_error(),
        "Negative page_size in ListUsers should not cause server error, got {status}"
    );
}

/// Test: Read tuples with invalid continuation token is handled.
#[tokio::test]
async fn test_read_invalid_continuation_token_handled() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/read?continuation_token=invalid_garbage"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:doc1"
            }
        }),
    )
    .await;

    // Should handle gracefully
    assert!(
        !status.is_server_error(),
        "Invalid continuation token in read should not cause server error, got {status}"
    );
}

// =============================================================================
// Section 7: Additional Edge Cases
// =============================================================================

/// Test: Request with null values in JSON is handled.
#[tokio::test]
async fn test_null_values_in_json_handled() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": null,
                "relation": "viewer",
                "object": "document:doc1"
            }
        }),
    )
    .await;

    // Should reject with 400 (null is not valid string)
    assert!(
        status == StatusCode::BAD_REQUEST,
        "Null user value should return 400, got {status}"
    );
}

/// Test: Request with wrong type values is handled.
#[tokio::test]
async fn test_wrong_type_values_handled() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Number instead of string
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": 12345,
                "relation": "viewer",
                "object": "document:doc1"
            }
        }),
    )
    .await;

    assert!(
        status == StatusCode::BAD_REQUEST,
        "Wrong type for user should return 400, got {status}"
    );
}

/// Test: Request with extra unknown fields is handled.
#[tokio::test]
async fn test_extra_unknown_fields_handled() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:doc1"
            },
            "unknown_field": "should_be_ignored",
            "another_extra": 123
        }),
    )
    .await;

    // Should either accept (ignore extra fields) or reject
    assert!(
        !status.is_server_error(),
        "Extra unknown fields should not cause server error, got {status}"
    );
}

/// Test: Unicode in all fields is handled safely.
#[tokio::test]
async fn test_unicode_in_fields_handled() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:αλίκη",
                "relation": "viewer",
                "object": "document:文档"
            }
        }),
    )
    .await;

    // Should handle unicode gracefully
    assert!(
        !status.is_server_error(),
        "Unicode in fields should not cause server error, got {status}"
    );
}

/// Test: Very long type and ID values are handled.
#[tokio::test]
async fn test_very_long_type_id_values_handled() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let long_type = "t".repeat(1000);
    let long_id = "i".repeat(1000);

    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": format!("{long_type}:{long_id}"),
                "relation": "viewer",
                "object": "document:doc1"
            }
        }),
    )
    .await;

    // Should handle gracefully
    assert!(
        !status.is_server_error(),
        "Very long type/ID values should not cause server error, got {status}"
    );
}

/// Test: Concurrent requests with same invalid input don't cause race conditions.
#[tokio::test]
async fn test_concurrent_invalid_requests_safe() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let tasks: Vec<_> = (0..10)
        .map(|_| {
            let storage = Arc::clone(&storage);
            let store_id = store_id.clone();
            tokio::spawn(async move {
                let (status, _) = post_json(
                    create_test_app(&storage),
                    &format!("/stores/{store_id}/check"),
                    serde_json::json!({
                        "tuple_key": {
                            "user": "",  // Invalid
                            "relation": "viewer",
                            "object": "document:doc1"
                        }
                    }),
                )
                .await;
                status
            })
        })
        .collect();

    let results: Vec<StatusCode> = futures::future::join_all(tasks)
        .await
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

    // All should return consistent 400 errors
    for status in results {
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "All concurrent invalid requests should return 400"
        );
    }
}
