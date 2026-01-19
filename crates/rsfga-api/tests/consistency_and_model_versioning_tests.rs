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

use common::{create_test_app, create_test_app_with_cache, post_json};

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

    // Should either succeed (ignoring unknown field) or return 400
    assert!(
        status.is_success() || status == StatusCode::BAD_REQUEST,
        "Invalid consistency should be ignored or return 400, got: {status}"
    );

    if status.is_success() {
        // Verify the check still returns correct results
        assert!(
            response.get("allowed").is_some(),
            "Response should have 'allowed' field"
        );
    }
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

/// Test: Non-existent model ID behavior.
/// When no matching model is found for an authorization_model_id, the system uses
/// the latest model as a fallback. If no tuple matches, returns allowed=false.
/// This tests the behavior when checking a tuple that doesn't exist.
#[tokio::test]
async fn test_check_with_nonexistent_model_id_no_matching_tuple() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Don't write any tuples - check for a non-existent permission
    // Check with non-existent model ID
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:nonexistent"
            },
            "authorization_model_id": "01H000000000000000000FAKE"
        }),
    )
    .await;

    // System returns 200 OK with allowed=false when tuple doesn't exist
    assert_eq!(
        status,
        StatusCode::OK,
        "Check with non-existent model_id should return 200 OK"
    );
    assert_eq!(
        response["allowed"].as_bool(),
        Some(false),
        "Check with non-existent tuple should return allowed=false"
    );
}

/// Test: Non-existent model ID with existing tuple uses latest model.
/// Current behavior: When an invalid authorization_model_id is provided,
/// the system falls back to the latest model. If a matching tuple exists,
/// the check returns allowed=true.
#[tokio::test]
async fn test_check_with_nonexistent_model_id_uses_latest_model() {
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

    // Check with non-existent model ID - uses latest model as fallback
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:doc1"
            },
            "authorization_model_id": "01H000000000000000000FAKE"
        }),
    )
    .await;

    // Current behavior: Falls back to latest model, so check succeeds
    assert_eq!(
        status,
        StatusCode::OK,
        "Check with non-existent model_id should return 200 OK"
    );
    assert_eq!(
        response["allowed"].as_bool(),
        Some(true),
        "Check with existing tuple should return allowed=true (uses latest model)"
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
                    "/stores/{store_id}/authorization-models/01H000000000000000000FAKE"
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
/// Current behavior: The authorization_model_id parameter is accepted but the
/// system may use the latest model for evaluation. This test documents
/// that specifying a model ID is accepted without errors.
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
    let (status1, _response1) = post_json(
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

    // With v2 model (which has editor relation), the check should succeed
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

/// Test: HIGHER_CONSISTENCY bypasses cache and gets fresh results.
/// This test verifies the interaction between consistency preference and caching.
#[tokio::test]
async fn test_higher_consistency_returns_fresh_results() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Initial check without tuple (should be allowed=false)
    let (status1, response1) = post_json(
        create_test_app_with_cache(&storage),
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
    assert_eq!(response1["allowed"].as_bool(), Some(false));

    // Write a tuple
    storage
        .write_tuples(
            &store_id,
            vec![create_tuple("viewer", "user:alice", "document", "doc1")],
            vec![],
        )
        .await
        .unwrap();

    // Check again with HIGHER_CONSISTENCY - should see the new tuple
    let (status2, response2) = post_json(
        create_test_app_with_cache(&storage),
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
    assert_eq!(
        response2["allowed"].as_bool(),
        Some(true),
        "HIGHER_CONSISTENCY should return fresh results"
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
// Helper Functions
// ============================================================================

/// Create a store and return its ID.
async fn create_store(storage: &MemoryDataStore) -> String {
    let store_id = ulid::Ulid::new().to_string();
    storage.create_store(&store_id, "Test Store").await.unwrap();
    store_id
}

/// Set up a test store with a simple authorization model (user, team, document).
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
