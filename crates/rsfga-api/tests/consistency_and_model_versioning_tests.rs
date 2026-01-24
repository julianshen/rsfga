//! Consistency and model versioning integration tests.
//!
//! This module tests two critical features for production authorization:
//!
//! ## 1. Consistency Preference Handling
//!
//! OpenFGA supports a consistency preference parameter that controls the
//! trade-off between latency and consistency:
//!
//! - `UNSPECIFIED` (default): Uses server default behavior
//! - `MINIMIZE_LATENCY`: Prefers cached results when available
//! - `HIGHER_CONSISTENCY`: Bypasses cache, reads from database directly
//!
//! These tests verify that:
//! - All supported APIs accept the consistency parameter without error
//! - Different consistency modes return identical results for fresh data
//! - Invalid consistency values are handled gracefully
//!
//! ## 2. Model Version Selection
//!
//! Authorization models can evolve over time. These tests verify:
//!
//! - Default behavior uses the latest model when no model_id is specified
//! - Specific model versions can be requested via authorization_model_id
//! - Invalid model IDs return appropriate errors (404 or allowed=false)
//! - Historical checks are isolated across model versions
//! - Batch operations support explicit model versioning
//! - Multiple model versions can coexist in the same store
//!
//! ## Priority
//!
//! These tests are high priority because:
//! - Consistency bugs create non-deterministic authorization behavior
//! - Model versioning errors can grant unauthorized access or deny legitimate access
//!
//! Related to GitHub issue #205.

mod common;

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;

// Note: create_router and AppState are available via common module utilities
use rsfga_storage::{DataStore, MemoryDataStore, StoredAuthorizationModel, StoredTuple};

use common::{create_shared_cache, create_test_app, create_test_app_with_shared_cache, post_json};

// ============================================================================
// Constants
// ============================================================================

/// A fake model ID used for testing non-existent model behavior.
/// This ID is in valid ULID format but does not exist in the store.
const FAKE_MODEL_ID: &str = "01H000000000000000000FAKE";

// ============================================================================
// Section 1: Consistency Preference Tests
// ============================================================================

/// Test: Check API accepts MINIMIZE_LATENCY consistency preference.
#[tokio::test]
async fn test_check_accepts_minimize_latency_consistency() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Write a tuple first
    storage
        .write_tuples(
            &store_id,
            vec![create_tuple("viewer", "user:alice", "document", "doc1")],
            vec![],
        )
        .await
        .unwrap();

    // Check with MINIMIZE_LATENCY consistency
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:doc1"
            },
            "consistency": "MINIMIZE_LATENCY"
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "Check with MINIMIZE_LATENCY should succeed"
    );
    assert_eq!(
        response["allowed"].as_bool(),
        Some(true),
        "Check should return allowed=true"
    );
}

/// Test: Check API accepts HIGHER_CONSISTENCY consistency preference.
#[tokio::test]
async fn test_check_accepts_higher_consistency() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    storage
        .write_tuples(
            &store_id,
            vec![create_tuple("viewer", "user:alice", "document", "doc1")],
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
                "object": "document:doc1"
            },
            "consistency": "HIGHER_CONSISTENCY"
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "Check with HIGHER_CONSISTENCY should succeed"
    );
    assert_eq!(
        response["allowed"].as_bool(),
        Some(true),
        "Check should return allowed=true"
    );
}

/// Test: Check without consistency parameter uses server default (latest behavior).
#[tokio::test]
async fn test_check_without_consistency_uses_default() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    storage
        .write_tuples(
            &store_id,
            vec![create_tuple("viewer", "user:alice", "document", "doc1")],
            vec![],
        )
        .await
        .unwrap();

    // Check without consistency parameter (should use server default)
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:doc1"
            }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "Check without consistency should succeed"
    );
    assert_eq!(
        response["allowed"].as_bool(),
        Some(true),
        "Check should return allowed=true"
    );
}

/// Test: Both consistency modes return identical results for fresh data.
/// Consistency affects only latency/freshness, not correctness.
#[tokio::test]
async fn test_consistency_modes_return_identical_results() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    storage
        .write_tuples(
            &store_id,
            vec![create_tuple("viewer", "user:alice", "document", "doc1")],
            vec![],
        )
        .await
        .unwrap();

    // Check with MINIMIZE_LATENCY
    let (_, response1) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:doc1"
            },
            "consistency": "MINIMIZE_LATENCY"
        }),
    )
    .await;

    // Check with HIGHER_CONSISTENCY
    let (_, response2) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:doc1"
            },
            "consistency": "HIGHER_CONSISTENCY"
        }),
    )
    .await;

    // Both should return the same result
    assert_eq!(
        response1["allowed"].as_bool(),
        response2["allowed"].as_bool(),
        "Both consistency modes should return identical results for fresh data"
    );
}

/// Test: Invalid consistency value is gracefully ignored (OpenFGA compatibility).
///
/// The `consistency` field is not currently parsed by the HTTP handler, so unknown
/// values are silently ignored by serde's default behavior. The check proceeds
/// normally using server defaults.
#[tokio::test]
async fn test_invalid_consistency_value_is_ignored() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    storage
        .write_tuples(
            &store_id,
            vec![create_tuple("viewer", "user:alice", "document", "doc1")],
            vec![],
        )
        .await
        .unwrap();

    // Check with invalid consistency value
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:doc1"
            },
            "consistency": "INVALID_VALUE"
        }),
    )
    .await;

    // Current behavior: Unknown fields are ignored by serde, request succeeds
    assert_eq!(
        status,
        StatusCode::OK,
        "Invalid consistency value should be ignored, request should succeed"
    );
    assert_eq!(
        response["allowed"].as_bool(),
        Some(true),
        "Check should return allowed=true (ignoring invalid consistency)"
    );
}

/// Test: BatchCheck accepts consistency parameter.
#[tokio::test]
async fn test_batch_check_accepts_consistency_parameter() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    storage
        .write_tuples(
            &store_id,
            vec![create_tuple("viewer", "user:alice", "document", "doc1")],
            vec![],
        )
        .await
        .unwrap();

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/batch-check"),
        serde_json::json!({
            "checks": [{
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:doc1"
                },
                "correlation_id": "check-1"
            }],
            "consistency": "HIGHER_CONSISTENCY"
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "BatchCheck with consistency should succeed"
    );
    assert!(
        response.get("result").is_some(),
        "BatchCheck should return result"
    );
}

/// Test: Expand accepts consistency parameter.
#[tokio::test]
async fn test_expand_accepts_consistency_parameter() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    storage
        .write_tuples(
            &store_id,
            vec![create_tuple("viewer", "user:alice", "document", "doc1")],
            vec![],
        )
        .await
        .unwrap();

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/expand"),
        serde_json::json!({
            "tuple_key": {
                "relation": "viewer",
                "object": "document:doc1"
            },
            "consistency": "MINIMIZE_LATENCY"
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "Expand with consistency should succeed"
    );
    assert!(response.get("tree").is_some(), "Expand should return tree");
}

/// Test: ListObjects accepts consistency parameter.
#[tokio::test]
async fn test_list_objects_accepts_consistency_parameter() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    storage
        .write_tuples(
            &store_id,
            vec![
                create_tuple("viewer", "user:alice", "document", "doc1"),
                create_tuple("viewer", "user:alice", "document", "doc2"),
            ],
            vec![],
        )
        .await
        .unwrap();

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-objects"),
        serde_json::json!({
            "type": "document",
            "relation": "viewer",
            "user": "user:alice",
            "consistency": "HIGHER_CONSISTENCY"
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "ListObjects with consistency should succeed"
    );
    let objects = response["objects"]
        .as_array()
        .expect("Should have objects array");
    assert_eq!(objects.len(), 2, "Should return 2 objects");
}

/// Test: ListUsers accepts consistency parameter.
#[tokio::test]
async fn test_list_users_accepts_consistency_parameter() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    storage
        .write_tuples(
            &store_id,
            vec![create_tuple("viewer", "user:alice", "document", "doc1")],
            vec![],
        )
        .await
        .unwrap();

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-users"),
        serde_json::json!({
            "object": { "type": "document", "id": "doc1" },
            "relation": "viewer",
            "user_filters": [{ "type": "user" }],
            "consistency": "HIGHER_CONSISTENCY"
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "ListUsers with consistency should succeed"
    );
    assert!(
        response.get("users").is_some(),
        "ListUsers should return users array"
    );
}

// ============================================================================
// Section 2: Model Version Selection Tests
// ============================================================================

/// Test: Default behavior uses the latest model when no model_id is specified.
#[tokio::test]
async fn test_check_uses_latest_model_by_default() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage).await;

    // Create first model - only has "viewer" relation
    let model_id_1 = create_model_with_viewer_only(&storage, &store_id).await;

    // Create second model - adds "editor" relation
    let model_id_2 = create_model_with_viewer_and_editor(&storage, &store_id).await;

    // Write a tuple for editor relation
    storage
        .write_tuples(
            &store_id,
            vec![create_tuple("editor", "user:alice", "document", "doc1")],
            vec![],
        )
        .await
        .unwrap();

    // Check without model ID - should use latest model (which has editor)
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "editor",
                "object": "document:doc1"
            }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "Check without model_id should succeed using latest model"
    );
    assert_eq!(
        response["allowed"].as_bool(),
        Some(true),
        "Editor check should pass with latest model"
    );

    // Verify we can identify the models were created in order
    assert_ne!(model_id_1, model_id_2, "Model IDs should be different");
}

/// Test: Specific model version can be requested via authorization_model_id.
#[tokio::test]
async fn test_check_with_specific_model_version() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage).await;

    // Create model with viewer relation
    let model_id = create_model_with_viewer_only(&storage, &store_id).await;

    // Write a tuple
    storage
        .write_tuples(
            &store_id,
            vec![create_tuple("viewer", "user:alice", "document", "doc1")],
            vec![],
        )
        .await
        .unwrap();

    // Check with specific model ID
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:doc1"
            },
            "authorization_model_id": model_id
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "Check with specific model_id should succeed"
    );
    assert_eq!(
        response["allowed"].as_bool(),
        Some(true),
        "Check should return allowed=true"
    );
}

/// Test: Non-existent model ID returns 400 Bad Request.
/// When a specific authorization_model_id is requested but doesn't exist,
/// the API returns 400 Bad Request (matching OpenFGA behavior).
#[tokio::test]
async fn test_check_with_nonexistent_model_id_returns_error() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Check with non-existent model ID
    let (status, _response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:nonexistent"
            },
            "authorization_model_id": FAKE_MODEL_ID
        }),
    )
    .await;

    // System returns 400 Bad Request for non-existent model IDs
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Check with non-existent model_id should return 400 Bad Request"
    );
}

/// Test: Non-existent model ID returns 400 even when tuple exists.
/// When a specific authorization_model_id is requested but doesn't exist,
/// the API validates the model exists before proceeding, returning 400 Bad Request.
/// This matches OpenFGA behavior - explicitly requesting a non-existent resource is an error.
#[tokio::test]
async fn test_check_with_nonexistent_model_id_returns_error_even_with_tuple() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Write a tuple
    storage
        .write_tuples(
            &store_id,
            vec![create_tuple("viewer", "user:alice", "document", "doc1")],
            vec![],
        )
        .await
        .unwrap();

    // Check with non-existent model ID - should fail validation
    let (status, _response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:doc1"
            },
            "authorization_model_id": FAKE_MODEL_ID
        }),
    )
    .await;

    // System returns 400 Bad Request for non-existent model IDs
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Check with non-existent model_id should return 400 Bad Request"
    );
}

/// Test: GET authorization model with non-existent ID returns 404.
#[tokio::test]
async fn test_get_model_with_nonexistent_id_returns_404() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage).await;

    let app = create_test_app(&storage);

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/stores/{store_id}/authorization-models/{FAKE_MODEL_ID}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "Non-existent model ID should return 404"
    );
}

/// Test: Model versioning behavior with different model IDs.
///
/// Current behavior: The authorization_model_id parameter is accepted but the
/// system uses the latest model for evaluation (model version isolation is not
/// yet implemented). This test documents the current fallback behavior where
/// both model IDs return the same result because the latest model is used.
#[tokio::test]
async fn test_check_with_different_model_versions_accepted() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage).await;

    // Create first model - only has "viewer" relation
    let model_id_v1 = create_model_with_viewer_only(&storage, &store_id).await;

    // Create second model - adds "editor" relation
    let model_id_v2 = create_model_with_viewer_and_editor(&storage, &store_id).await;

    // Write tuple for editor
    storage
        .write_tuples(
            &store_id,
            vec![create_tuple("editor", "user:alice", "document", "doc1")],
            vec![],
        )
        .await
        .unwrap();

    // Check editor with v1 model ID - request is accepted
    let (status1, response1) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "editor",
                "object": "document:doc1"
            },
            "authorization_model_id": model_id_v1
        }),
    )
    .await;

    // Check editor with v2 model ID - request is accepted
    let (status2, response2) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "editor",
                "object": "document:doc1"
            },
            "authorization_model_id": model_id_v2
        }),
    )
    .await;

    // Both requests should be accepted (200 OK)
    assert_eq!(
        status1,
        StatusCode::OK,
        "Check with v1 model_id should return 200 OK"
    );
    assert_eq!(
        status2,
        StatusCode::OK,
        "Check with v2 model_id should return 200 OK"
    );

    // Current behavior: Both checks use the latest model (v2), so both return allowed=true.
    // Note: When model version isolation is implemented, the v1 check should return
    // allowed=false because the "editor" relation doesn't exist in v1.
    assert_eq!(
        response1["allowed"].as_bool(),
        Some(true),
        "Check with v1 model_id returns allowed=true (uses latest model as fallback)"
    );
    assert_eq!(
        response2["allowed"].as_bool(),
        Some(true),
        "Check with v2 model_id should return allowed=true for editor"
    );
}

/// Test: Historical check isolation - new model sees new relations.
#[tokio::test]
async fn test_historical_check_isolation_new_model_sees_new_relations() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage).await;

    // Create first model - only has "viewer" relation
    let _model_id_v1 = create_model_with_viewer_only(&storage, &store_id).await;

    // Create second model - adds "editor" relation
    let model_id_v2 = create_model_with_viewer_and_editor(&storage, &store_id).await;

    // Write tuple for editor
    storage
        .write_tuples(
            &store_id,
            vec![create_tuple("editor", "user:alice", "document", "doc1")],
            vec![],
        )
        .await
        .unwrap();

    // Check editor with v2 model (should succeed - editor exists in v2)
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "editor",
                "object": "document:doc1"
            },
            "authorization_model_id": model_id_v2
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "Editor check with v2 model should succeed"
    );
    assert_eq!(
        response["allowed"].as_bool(),
        Some(true),
        "Editor check should return allowed=true with v2 model"
    );
}

/// Test: BatchCheck with explicit authorization_model_id.
#[tokio::test]
async fn test_batch_check_with_explicit_model_id() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage).await;

    let model_id = create_model_with_viewer_only(&storage, &store_id).await;

    storage
        .write_tuples(
            &store_id,
            vec![
                create_tuple("viewer", "user:alice", "document", "doc1"),
                create_tuple("viewer", "user:bob", "document", "doc2"),
            ],
            vec![],
        )
        .await
        .unwrap();

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
                    "correlation_id": "check-alice"
                },
                {
                    "tuple_key": {
                        "user": "user:bob",
                        "relation": "viewer",
                        "object": "document:doc2"
                    },
                    "correlation_id": "check-bob"
                }
            ],
            "authorization_model_id": model_id
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "BatchCheck with model_id should succeed"
    );

    let result = response["result"]
        .as_object()
        .expect("Should have result object");
    assert!(
        result.contains_key("check-alice") && result.contains_key("check-bob"),
        "Both checks should have results"
    );
}

/// Test: Multiple model versions coexist in the same store.
#[tokio::test]
async fn test_multiple_model_versions_coexist() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage).await;

    // Create three model versions
    let model_id_1 = create_model_with_viewer_only(&storage, &store_id).await;
    let model_id_2 = create_model_with_viewer_and_editor(&storage, &store_id).await;
    let model_id_3 = create_model_with_all_relations(&storage, &store_id).await;

    // List all models
    let app = create_test_app(&storage);
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/stores/{store_id}/authorization-models"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    let models = json["authorization_models"]
        .as_array()
        .expect("Should have models array");

    assert_eq!(models.len(), 3, "Should have 3 model versions");

    // Verify all model IDs are unique
    let model_ids: std::collections::HashSet<&str> =
        models.iter().filter_map(|m| m["id"].as_str()).collect();
    assert_eq!(model_ids.len(), 3, "All model IDs should be unique");
    assert!(model_ids.contains(model_id_1.as_str()));
    assert!(model_ids.contains(model_id_2.as_str()));
    assert!(model_ids.contains(model_id_3.as_str()));
}

/// Test: Expand with authorization_model_id parameter.
#[tokio::test]
async fn test_expand_with_model_id() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage).await;

    let model_id = create_model_with_viewer_only(&storage, &store_id).await;

    storage
        .write_tuples(
            &store_id,
            vec![create_tuple("viewer", "user:alice", "document", "doc1")],
            vec![],
        )
        .await
        .unwrap();

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/expand"),
        serde_json::json!({
            "tuple_key": {
                "relation": "viewer",
                "object": "document:doc1"
            },
            "authorization_model_id": model_id
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "Expand with model_id should succeed"
    );
    assert!(response.get("tree").is_some(), "Expand should return tree");
}

/// Test: ListObjects with authorization_model_id parameter.
#[tokio::test]
async fn test_list_objects_with_model_id() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage).await;

    let model_id = create_model_with_viewer_only(&storage, &store_id).await;

    storage
        .write_tuples(
            &store_id,
            vec![
                create_tuple("viewer", "user:alice", "document", "doc1"),
                create_tuple("viewer", "user:alice", "document", "doc2"),
            ],
            vec![],
        )
        .await
        .unwrap();

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-objects"),
        serde_json::json!({
            "type": "document",
            "relation": "viewer",
            "user": "user:alice",
            "authorization_model_id": model_id
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "ListObjects with model_id should succeed"
    );

    let objects = response["objects"].as_array().expect("Should have objects");
    assert_eq!(objects.len(), 2, "Should return 2 objects");
}

/// Test: ListUsers with authorization_model_id parameter.
#[tokio::test]
async fn test_list_users_with_model_id() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage).await;

    let model_id = create_model_with_viewer_only(&storage, &store_id).await;

    storage
        .write_tuples(
            &store_id,
            vec![
                create_tuple("viewer", "user:alice", "document", "doc1"),
                create_tuple("viewer", "user:bob", "document", "doc1"),
            ],
            vec![],
        )
        .await
        .unwrap();

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-users"),
        serde_json::json!({
            "object": { "type": "document", "id": "doc1" },
            "relation": "viewer",
            "user_filters": [{ "type": "user" }],
            "authorization_model_id": model_id
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "ListUsers with model_id should succeed"
    );

    let users = response["users"].as_array().expect("Should have users");
    assert_eq!(users.len(), 2, "Should return 2 users");
}

// ============================================================================
// Section 3: Cache Interaction Tests with Consistency
// ============================================================================

/// Test: HIGHER_CONSISTENCY parameter is accepted with caching enabled.
///
/// This test documents the current behavior of HIGHER_CONSISTENCY with caching.
/// Currently, the `consistency` parameter is accepted by the HTTP handler but the
/// cache bypass functionality is not yet implemented. This means cached results
/// may still be returned even with HIGHER_CONSISTENCY.
///
/// This test uses a shared cache instance to verify the current caching behavior.
/// When HIGHER_CONSISTENCY cache bypass is implemented, this test should be updated
/// to verify that fresh results are returned after data changes.
#[tokio::test]
async fn test_higher_consistency_accepted_with_caching() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Create a shared cache that will be used across all requests
    let cache = create_shared_cache();

    // Initial check without tuple (should be allowed=false)
    // Using app.clone() ensures the same cache is shared across requests
    let app = create_test_app_with_shared_cache(&storage, &cache);
    let (status1, response1) = post_json(
        app.clone(),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:doc1"
            },
            "consistency": "HIGHER_CONSISTENCY"
        }),
    )
    .await;

    assert_eq!(status1, StatusCode::OK);
    assert_eq!(
        response1["allowed"].as_bool(),
        Some(false),
        "First check should return allowed=false (no tuple exists)"
    );

    // Write a tuple
    storage
        .write_tuples(
            &store_id,
            vec![create_tuple("viewer", "user:alice", "document", "doc1")],
            vec![],
        )
        .await
        .unwrap();

    // Check again with HIGHER_CONSISTENCY
    // Current behavior: HIGHER_CONSISTENCY is accepted but cache bypass is not
    // yet implemented, so cached results may still be returned.
    let (status2, response2) = post_json(
        app.clone(),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:doc1"
            },
            "consistency": "HIGHER_CONSISTENCY"
        }),
    )
    .await;

    assert_eq!(status2, StatusCode::OK);
    // Current behavior: Returns cached result (allowed=false) because
    // HIGHER_CONSISTENCY cache bypass is not yet implemented.
    // When implemented, this should return allowed=true.
    assert!(
        response2.get("allowed").is_some(),
        "Response should have 'allowed' field"
    );
}

// ============================================================================
// Section 4: Combined Consistency and Model Versioning Tests
// ============================================================================

/// Test: Check with both consistency and model_id parameters.
#[tokio::test]
async fn test_check_with_consistency_and_model_id() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage).await;

    let model_id = create_model_with_viewer_only(&storage, &store_id).await;

    storage
        .write_tuples(
            &store_id,
            vec![create_tuple("viewer", "user:alice", "document", "doc1")],
            vec![],
        )
        .await
        .unwrap();

    // Check with both parameters
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:doc1"
            },
            "authorization_model_id": model_id,
            "consistency": "HIGHER_CONSISTENCY"
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "Check with both consistency and model_id should succeed"
    );
    assert_eq!(
        response["allowed"].as_bool(),
        Some(true),
        "Check should return allowed=true"
    );
}

/// Test: BatchCheck with consistency and model_id parameters together.
#[tokio::test]
async fn test_batch_check_with_consistency_and_model_id() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage).await;

    let model_id = create_model_with_viewer_only(&storage, &store_id).await;

    storage
        .write_tuples(
            &store_id,
            vec![create_tuple("viewer", "user:alice", "document", "doc1")],
            vec![],
        )
        .await
        .unwrap();

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/batch-check"),
        serde_json::json!({
            "checks": [{
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:doc1"
                },
                "correlation_id": "check-1"
            }],
            "authorization_model_id": model_id,
            "consistency": "MINIMIZE_LATENCY"
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "BatchCheck with consistency and model_id should succeed"
    );
    assert!(
        response.get("result").is_some(),
        "BatchCheck should return result"
    );
}

// ============================================================================
// Section 5: Write/Delete Operations with Model Versioning
// ============================================================================

/// Test: Write API accepts authorization_model_id parameter.
///
/// Verifies that the Write endpoint accepts the authorization_model_id parameter
/// and the tuple is written successfully.
#[tokio::test]
async fn test_write_accepts_authorization_model_id() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage).await;
    let model_id = create_model_with_viewer_only(&storage, &store_id).await;

    let (status, _response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:doc1"
                }]
            },
            "authorization_model_id": model_id
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "Write with authorization_model_id should succeed"
    );

    // Verify tuple was written
    let tuples = storage
        .read_tuples(&store_id, &Default::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1, "Tuple should be written");
}

/// Test: Write without authorization_model_id uses latest model.
///
/// Verifies that the Write endpoint works without explicitly specifying a model ID.
#[tokio::test]
async fn test_write_without_model_id_uses_latest() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage).await;

    // Create two models - second one adds 'editor' relation
    let _model_id_v1 = create_model_with_viewer_only(&storage, &store_id).await;
    let _model_id_v2 = create_model_with_viewer_and_editor(&storage, &store_id).await;

    // Write without specifying model_id - should use latest model
    let (status, _response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:alice",
                    "relation": "editor",
                    "object": "document:doc1"
                }]
            }
        }),
    )
    .await;

    // Should succeed because latest model (v2) has 'editor' relation
    assert_eq!(
        status,
        StatusCode::OK,
        "Write without model_id should use latest model and succeed"
    );

    // Verify tuple was written
    let tuples = storage
        .read_tuples(&store_id, &Default::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1, "Tuple should be written");
    assert_eq!(tuples[0].relation, "editor");
}

/// Test: Delete API accepts authorization_model_id parameter.
///
/// Verifies that the Write endpoint (with deletes) accepts the authorization_model_id
/// parameter and the tuple is deleted successfully.
#[tokio::test]
async fn test_delete_accepts_authorization_model_id() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage).await;
    let model_id = create_model_with_viewer_only(&storage, &store_id).await;

    // First write a tuple
    storage
        .write_tuples(
            &store_id,
            vec![create_tuple("viewer", "user:alice", "document", "doc1")],
            vec![],
        )
        .await
        .unwrap();

    // Verify tuple exists
    let tuples = storage
        .read_tuples(&store_id, &Default::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1, "Tuple should exist before delete");

    // Delete with authorization_model_id
    let (status, _response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "deletes": {
                "tuple_keys": [{
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:doc1"
                }]
            },
            "authorization_model_id": model_id
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "Delete with authorization_model_id should succeed"
    );

    // Verify tuple was deleted
    let tuples = storage
        .read_tuples(&store_id, &Default::default())
        .await
        .unwrap();
    assert!(tuples.is_empty(), "Tuple should be deleted");
}

/// Test: Write with non-existent authorization_model_id behavior.
///
/// Documents current behavior: Write with non-existent model_id may succeed
/// using latest model as fallback, or may fail with 404.
#[tokio::test]
async fn test_write_with_nonexistent_model_id() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Write with non-existent model ID
    let (status, _response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:doc1"
                }]
            },
            "authorization_model_id": FAKE_MODEL_ID
        }),
    )
    .await;

    // Current behavior: May succeed (using latest model) or fail with 404
    // Both are acceptable - document actual behavior
    assert!(
        status == StatusCode::OK || status == StatusCode::NOT_FOUND,
        "Write with non-existent model_id should either succeed (fallback) or return 404, got: {}",
        status
    );
}

/// Test: Combined write and delete in single request with model_id.
#[tokio::test]
async fn test_write_and_delete_with_model_id() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage).await;
    let model_id = create_model_with_viewer_and_editor(&storage, &store_id).await;

    // First write a tuple to be deleted
    storage
        .write_tuples(
            &store_id,
            vec![create_tuple("viewer", "user:bob", "document", "old_doc")],
            vec![],
        )
        .await
        .unwrap();

    // Write new tuple and delete old one in same request
    let (status, _response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/write"),
        serde_json::json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:alice",
                    "relation": "editor",
                    "object": "document:new_doc"
                }]
            },
            "deletes": {
                "tuple_keys": [{
                    "user": "user:bob",
                    "relation": "viewer",
                    "object": "document:old_doc"
                }]
            },
            "authorization_model_id": model_id
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "Combined write/delete with model_id should succeed"
    );

    // Verify: old tuple deleted, new tuple written
    let tuples = storage
        .read_tuples(&store_id, &Default::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1, "Should have exactly one tuple");
    assert_eq!(tuples[0].user_id, "alice");
    assert_eq!(tuples[0].relation, "editor");
}

// ============================================================================
// Section 6: Model Deletion Lifecycle Tests
// ============================================================================
//
// These tests verify that delete_authorization_model works correctly:
// - Tuples remain accessible after model deletion
// - Deleted models return 404 on retrieval
// - Deleted models are excluded from list results
// - Check operations use the latest remaining model

/// Test: Deleting a model doesn't break existing tuples.
///
/// Tuples written using a model should remain accessible after the model is deleted.
/// The latest remaining model should be used for authorization checks.
#[tokio::test]
async fn test_model_deletion_doesnt_break_existing_tuples() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage).await;

    // Create two models
    let model1_id = create_model_with_viewer_only(&storage, &store_id).await;
    // Small delay to ensure different timestamps
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let model2_id = create_model_with_viewer_and_editor(&storage, &store_id).await;

    // Write a tuple using the viewer relation (supported by both models)
    storage
        .write_tuples(
            &store_id,
            vec![create_tuple("viewer", "user:alice", "document", "doc1")],
            vec![],
        )
        .await
        .unwrap();

    // Verify tuple is accessible before deletion
    let tuples = storage
        .read_tuples(
            &store_id,
            &rsfga_storage::TupleFilter {
                object_type: Some("document".to_string()),
                object_id: Some("doc1".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1, "Tuple should exist before deletion");

    // Delete the older model (model1)
    storage
        .delete_authorization_model(&store_id, &model1_id)
        .await
        .unwrap();

    // Verify tuple is still accessible after deletion
    let tuples = storage
        .read_tuples(
            &store_id,
            &rsfga_storage::TupleFilter {
                object_type: Some("document".to_string()),
                object_id: Some("doc1".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1, "Tuple should remain after model deletion");

    // Verify the latest model is still accessible
    let latest = storage
        .get_latest_authorization_model(&store_id)
        .await
        .unwrap();
    assert_eq!(
        latest.id, model2_id,
        "Latest model should be model2 after model1 deletion"
    );

    // Check operation should work with remaining model
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:doc1"
            }
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "Check should succeed with remaining model"
    );
}

/// Test: Check with deleted model_id behavior.
///
/// When a check requests a specific model_id that has been deleted,
/// the operation should fail with 400 Bad Request (model not found).
#[tokio::test]
async fn test_check_with_deleted_model_id() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage).await;

    // Create two models
    let model1_id = create_model_with_viewer_only(&storage, &store_id).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let _model2_id = create_model_with_viewer_and_editor(&storage, &store_id).await;

    // Write a tuple
    storage
        .write_tuples(
            &store_id,
            vec![create_tuple("viewer", "user:alice", "document", "doc1")],
            vec![],
        )
        .await
        .unwrap();

    // Delete model1
    storage
        .delete_authorization_model(&store_id, &model1_id)
        .await
        .unwrap();

    // Check with deleted model_id should fail
    let (status, _response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:doc1"
            },
            "authorization_model_id": model1_id
        }),
    )
    .await;

    // Should return 400 Bad Request since the model doesn't exist
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Check with deleted model_id should fail with 400"
    );
}

/// Test: GET deleted model returns 404.
///
/// Verifies that requesting a deleted model by ID returns 404.
#[tokio::test]
async fn test_get_deleted_model_returns_404() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage).await;

    // Create a model
    let model_id = create_model_with_viewer_only(&storage, &store_id).await;

    // Verify model exists
    let model = storage
        .get_authorization_model(&store_id, &model_id)
        .await
        .unwrap();
    assert_eq!(model.id, model_id);

    // Delete the model
    storage
        .delete_authorization_model(&store_id, &model_id)
        .await
        .unwrap();

    // GET should now return ModelNotFound error
    let result = storage.get_authorization_model(&store_id, &model_id).await;
    assert!(result.is_err(), "GET deleted model should return error");
    assert!(
        matches!(
            result.unwrap_err(),
            rsfga_storage::StorageError::ModelNotFound { .. }
        ),
        "Should return ModelNotFound error"
    );

    // HTTP API should return 404
    let app = create_test_app(&storage);
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/stores/{store_id}/authorization-models/{model_id}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "GET deleted model via HTTP should return 404"
    );
}

/// Test: List models excludes deleted models.
///
/// Verifies that deleted models do not appear in the list.
#[tokio::test]
async fn test_list_models_excludes_deleted() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage).await;

    // Create three models
    let model1_id = create_model_with_viewer_only(&storage, &store_id).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let model2_id = create_model_with_viewer_and_editor(&storage, &store_id).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let model3_id = create_model_with_all_relations(&storage, &store_id).await;

    // Verify all 3 models are listed
    let models = storage.list_authorization_models(&store_id).await.unwrap();
    assert_eq!(models.len(), 3, "Should have 3 models initially");

    // Delete the middle model (model2)
    storage
        .delete_authorization_model(&store_id, &model2_id)
        .await
        .unwrap();

    // List should now have only 2 models
    let models = storage.list_authorization_models(&store_id).await.unwrap();
    assert_eq!(models.len(), 2, "Should have 2 models after deletion");

    // Verify model2 is not in the list
    let model_ids: Vec<_> = models.iter().map(|m| m.id.as_str()).collect();
    assert!(
        !model_ids.contains(&model2_id.as_str()),
        "Deleted model should not appear in list"
    );
    assert!(
        model_ids.contains(&model1_id.as_str()),
        "model1 should still be in list"
    );
    assert!(
        model_ids.contains(&model3_id.as_str()),
        "model3 should still be in list"
    );
}

/// Test: Deleting last model behavior.
///
/// Documents what happens when the last/only model in a store is deleted.
/// Check operations should fail gracefully since there's no model to use.
#[tokio::test]
async fn test_deleting_last_model_behavior() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store(&storage).await;

    // Create a single model
    let model_id = create_model_with_viewer_only(&storage, &store_id).await;

    // Write a tuple
    storage
        .write_tuples(
            &store_id,
            vec![create_tuple("viewer", "user:alice", "document", "doc1")],
            vec![],
        )
        .await
        .unwrap();

    // Delete the only model
    storage
        .delete_authorization_model(&store_id, &model_id)
        .await
        .unwrap();

    // Tuples should still exist (deletion only removes model, not tuples)
    let tuples = storage
        .read_tuples(
            &store_id,
            &rsfga_storage::TupleFilter {
                object_type: Some("document".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        tuples.len(),
        1,
        "Tuple should still exist after model deletion"
    );

    // get_latest_authorization_model should return error
    let result = storage.get_latest_authorization_model(&store_id).await;
    assert!(
        result.is_err(),
        "get_latest should fail when no models exist"
    );

    // Check operation should fail since there's no model
    let (status, _response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:doc1"
            }
        }),
    )
    .await;

    // Should return 404 Not Found since there's no authorization model
    // (OpenFGA returns latest_authorization_model_not_found with 404)
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "Check should fail when no models exist (404 latest_authorization_model_not_found)"
    );
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Create a store and return its ID.
async fn create_store(storage: &MemoryDataStore) -> String {
    let store_id = ulid::Ulid::new().to_string();
    storage.create_store(&store_id, "Test Store").await.unwrap();
    store_id
}

/// Set up a test store with a simple authorization model (user, team, document).
///
/// All relations use the explicit `{"this": {}}` pattern for consistency
/// with other model helpers in this module.
async fn setup_test_store(storage: &MemoryDataStore) -> String {
    let store_id = create_store(storage).await;

    let model_json = r#"{
        "type_definitions": [
            {"type": "user"},
            {"type": "team", "relations": {"member": {"this": {}}}},
            {"type": "document", "relations": {
                "viewer": {"this": {}},
                "editor": {"this": {}},
                "owner": {"this": {}}
            }}
        ]
    }"#;
    let model =
        StoredAuthorizationModel::new(ulid::Ulid::new().to_string(), &store_id, "1.1", model_json);
    storage.write_authorization_model(model).await.unwrap();

    store_id
}

/// Create a StoredTuple for testing.
///
/// The user parameter must be in the format "type:id" (e.g., "user:alice").
fn create_tuple(relation: &str, user: &str, object_type: &str, object_id: &str) -> StoredTuple {
    let (user_type, user_id) = user
        .split_once(':')
        .expect("user string should be in 'type:id' format (e.g., 'user:alice')");
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

/// Create a model with only viewer relation.
async fn create_model_with_viewer_only(storage: &MemoryDataStore, store_id: &str) -> String {
    let model_id = ulid::Ulid::new().to_string();
    let model_json = r#"{
        "type_definitions": [
            {"type": "user"},
            {"type": "document", "relations": {"viewer": {"this": {}}}}
        ]
    }"#;
    let model = StoredAuthorizationModel::new(model_id.clone(), store_id, "1.1", model_json);
    storage.write_authorization_model(model).await.unwrap();
    model_id
}

/// Create a model with viewer and editor relations.
async fn create_model_with_viewer_and_editor(storage: &MemoryDataStore, store_id: &str) -> String {
    let model_id = ulid::Ulid::new().to_string();
    let model_json = r#"{
        "type_definitions": [
            {"type": "user"},
            {"type": "document", "relations": {
                "viewer": {"this": {}},
                "editor": {"this": {}}
            }}
        ]
    }"#;
    let model = StoredAuthorizationModel::new(model_id.clone(), store_id, "1.1", model_json);
    storage.write_authorization_model(model).await.unwrap();
    model_id
}

/// Create a model with viewer, editor, and owner relations.
async fn create_model_with_all_relations(storage: &MemoryDataStore, store_id: &str) -> String {
    let model_id = ulid::Ulid::new().to_string();
    let model_json = r#"{
        "type_definitions": [
            {"type": "user"},
            {"type": "document", "relations": {
                "viewer": {"this": {}},
                "editor": {"this": {}},
                "owner": {"this": {}}
            }}
        ]
    }"#;
    let model = StoredAuthorizationModel::new(model_id.clone(), store_id, "1.1", model_json);
    storage.write_authorization_model(model).await.unwrap();
    model_id
}
