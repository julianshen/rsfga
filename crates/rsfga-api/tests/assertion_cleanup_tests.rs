//! Tests for assertion cleanup when stores or models are deleted.
//!
//! This test suite verifies that assertions are properly cleaned up:
//! - When a store is deleted, all assertions for that store are removed
//! - When an authorization model is deleted, assertions for that model are removed
//!
//! See: https://github.com/openfga/rsfga/issues/237

mod common;

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;

use rsfga_api::http::{create_router, AppState};
use rsfga_storage::{DataStore, MemoryDataStore, StoredAuthorizationModel};

/// Helper to make a PUT request (for write_assertions).
async fn put_json(
    app: axum::Router,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
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
        serde_json::from_slice(&body).unwrap_or_else(
            |_| serde_json::json!({ "raw_body": String::from_utf8_lossy(&body).to_string() }),
        )
    };
    (status, json)
}

/// Helper to make a DELETE request.
async fn delete_request(app: axum::Router, uri: &str) -> StatusCode {
    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    response.status()
}

/// Create a test app returning AppState for inspection.
fn create_test_state(storage: Arc<MemoryDataStore>) -> AppState<MemoryDataStore> {
    AppState::new(storage)
}

// ============================================================================
// Store Deletion Tests
// ============================================================================

/// Test that deleting a store cleans up all associated assertions.
#[tokio::test]
async fn test_delete_store_cleans_up_assertions() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create a store
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    // Create an authorization model
    let model_json = r#"{"type_definitions": [{"type": "user"}, {"type": "document", "relations": {"viewer": {"this": {}}}}]}"#;
    let model =
        StoredAuthorizationModel::new("model-1".to_string(), "test-store", "1.1", model_json);
    storage.write_authorization_model(model).await.unwrap();

    // Create state and app
    let state = create_test_state(Arc::clone(&storage));
    let assertions = Arc::clone(&state.assertions);

    // Write assertions
    let app = create_router(state);
    let (status, _) = put_json(
        app,
        "/stores/test-store/assertions/model-1",
        serde_json::json!({
            "assertions": [{
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:readme"
                },
                "expectation": true
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Verify assertions exist
    assert_eq!(assertions.len(), 1);
    assert!(assertions.contains_key(&("test-store".to_string(), "model-1".to_string())));

    // Create new state sharing the same assertions map and delete the store
    let state2 = AppState::new(Arc::clone(&storage));
    // Copy the assertions to the new state for sharing
    for entry in assertions.iter() {
        state2
            .assertions
            .insert(entry.key().clone(), entry.value().clone());
    }
    let assertions2 = Arc::clone(&state2.assertions);

    let app = create_router(state2);
    let status = delete_request(app, "/stores/test-store").await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Verify assertions are cleaned up
    assert_eq!(assertions2.len(), 0);
}

/// Test that deleting a store cleans up assertions for multiple models.
#[tokio::test]
async fn test_delete_store_cleans_up_multiple_model_assertions() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create a store
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    // Create two authorization models
    let model_json = r#"{"type_definitions": [{"type": "user"}, {"type": "document", "relations": {"viewer": {"this": {}}}}]}"#;
    let model1 =
        StoredAuthorizationModel::new("model-1".to_string(), "test-store", "1.1", model_json);
    let model2 =
        StoredAuthorizationModel::new("model-2".to_string(), "test-store", "1.1", model_json);
    storage.write_authorization_model(model1).await.unwrap();
    storage.write_authorization_model(model2).await.unwrap();

    // Create state
    let state = create_test_state(Arc::clone(&storage));
    let assertions = Arc::clone(&state.assertions);

    // Write assertions for model-1
    let app = create_router(state);
    let (status, _) = put_json(
        app,
        "/stores/test-store/assertions/model-1",
        serde_json::json!({
            "assertions": [{
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:readme"
                },
                "expectation": true
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Write assertions for model-2 (need to create new state sharing assertions)
    let state2 = AppState::new(Arc::clone(&storage));
    for entry in assertions.iter() {
        state2
            .assertions
            .insert(entry.key().clone(), entry.value().clone());
    }
    let assertions2 = Arc::clone(&state2.assertions);

    let app = create_router(state2);
    let (status, _) = put_json(
        app,
        "/stores/test-store/assertions/model-2",
        serde_json::json!({
            "assertions": [{
                "tuple_key": {
                    "user": "user:bob",
                    "relation": "viewer",
                    "object": "document:readme"
                },
                "expectation": false
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Verify both assertions exist
    assert_eq!(assertions2.len(), 2);

    // Delete the store
    let state3 = AppState::new(Arc::clone(&storage));
    for entry in assertions2.iter() {
        state3
            .assertions
            .insert(entry.key().clone(), entry.value().clone());
    }
    let assertions3 = Arc::clone(&state3.assertions);

    let app = create_router(state3);
    let status = delete_request(app, "/stores/test-store").await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Verify all assertions are cleaned up
    assert_eq!(assertions3.len(), 0);
}

/// Test that deleting one store doesn't affect assertions from other stores.
#[tokio::test]
async fn test_delete_store_preserves_other_store_assertions() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create two stores
    storage.create_store("store-1", "Store 1").await.unwrap();
    storage.create_store("store-2", "Store 2").await.unwrap();

    // Create authorization models in both stores
    let model_json = r#"{"type_definitions": [{"type": "user"}, {"type": "document", "relations": {"viewer": {"this": {}}}}]}"#;
    let model1 = StoredAuthorizationModel::new("model-a".to_string(), "store-1", "1.1", model_json);
    let model2 = StoredAuthorizationModel::new("model-b".to_string(), "store-2", "1.1", model_json);
    storage.write_authorization_model(model1).await.unwrap();
    storage.write_authorization_model(model2).await.unwrap();

    // Create state
    let state = create_test_state(Arc::clone(&storage));
    let assertions = Arc::clone(&state.assertions);

    // Write assertions for store-1
    let app = create_router(state);
    let (status, _) = put_json(
        app,
        "/stores/store-1/assertions/model-a",
        serde_json::json!({
            "assertions": [{
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:readme"
                },
                "expectation": true
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Write assertions for store-2
    let state2 = AppState::new(Arc::clone(&storage));
    for entry in assertions.iter() {
        state2
            .assertions
            .insert(entry.key().clone(), entry.value().clone());
    }
    let assertions2 = Arc::clone(&state2.assertions);

    let app = create_router(state2);
    let (status, _) = put_json(
        app,
        "/stores/store-2/assertions/model-b",
        serde_json::json!({
            "assertions": [{
                "tuple_key": {
                    "user": "user:bob",
                    "relation": "viewer",
                    "object": "document:readme"
                },
                "expectation": false
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Verify both assertions exist
    assert_eq!(assertions2.len(), 2);

    // Delete store-1
    let state3 = AppState::new(Arc::clone(&storage));
    for entry in assertions2.iter() {
        state3
            .assertions
            .insert(entry.key().clone(), entry.value().clone());
    }
    let assertions3 = Arc::clone(&state3.assertions);

    let app = create_router(state3);
    let status = delete_request(app, "/stores/store-1").await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Verify only store-1 assertions are removed
    assert_eq!(assertions3.len(), 1);
    assert!(!assertions3.contains_key(&("store-1".to_string(), "model-a".to_string())));
    assert!(assertions3.contains_key(&("store-2".to_string(), "model-b".to_string())));
}

// ============================================================================
// Authorization Model Deletion Tests
// ============================================================================

/// Test that deleting an authorization model cleans up its assertions.
#[tokio::test]
async fn test_delete_authorization_model_cleans_up_assertions() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create a store
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    // Create an authorization model
    let model_json = r#"{"type_definitions": [{"type": "user"}, {"type": "document", "relations": {"viewer": {"this": {}}}}]}"#;
    let model =
        StoredAuthorizationModel::new("model-1".to_string(), "test-store", "1.1", model_json);
    storage.write_authorization_model(model).await.unwrap();

    // Create state
    let state = create_test_state(Arc::clone(&storage));
    let assertions = Arc::clone(&state.assertions);

    // Write assertions
    let app = create_router(state);
    let (status, _) = put_json(
        app,
        "/stores/test-store/assertions/model-1",
        serde_json::json!({
            "assertions": [{
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:readme"
                },
                "expectation": true
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Verify assertions exist
    assert_eq!(assertions.len(), 1);

    // Delete the authorization model
    let state2 = AppState::new(Arc::clone(&storage));
    for entry in assertions.iter() {
        state2
            .assertions
            .insert(entry.key().clone(), entry.value().clone());
    }
    let assertions2 = Arc::clone(&state2.assertions);

    let app = create_router(state2);
    let status = delete_request(app, "/stores/test-store/authorization-models/model-1").await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Verify assertions are cleaned up
    assert_eq!(assertions2.len(), 0);
}

/// Test that deleting one model doesn't affect assertions from other models.
#[tokio::test]
async fn test_delete_model_preserves_other_model_assertions() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create a store
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    // Create two authorization models
    let model_json = r#"{"type_definitions": [{"type": "user"}, {"type": "document", "relations": {"viewer": {"this": {}}}}]}"#;
    let model1 =
        StoredAuthorizationModel::new("model-1".to_string(), "test-store", "1.1", model_json);
    let model2 =
        StoredAuthorizationModel::new("model-2".to_string(), "test-store", "1.1", model_json);
    storage.write_authorization_model(model1).await.unwrap();
    storage.write_authorization_model(model2).await.unwrap();

    // Create state
    let state = create_test_state(Arc::clone(&storage));
    let assertions = Arc::clone(&state.assertions);

    // Write assertions for model-1
    let app = create_router(state);
    let (status, _) = put_json(
        app,
        "/stores/test-store/assertions/model-1",
        serde_json::json!({
            "assertions": [{
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:readme"
                },
                "expectation": true
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Write assertions for model-2
    let state2 = AppState::new(Arc::clone(&storage));
    for entry in assertions.iter() {
        state2
            .assertions
            .insert(entry.key().clone(), entry.value().clone());
    }
    let assertions2 = Arc::clone(&state2.assertions);

    let app = create_router(state2);
    let (status, _) = put_json(
        app,
        "/stores/test-store/assertions/model-2",
        serde_json::json!({
            "assertions": [{
                "tuple_key": {
                    "user": "user:bob",
                    "relation": "viewer",
                    "object": "document:readme"
                },
                "expectation": false
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Verify both assertions exist
    assert_eq!(assertions2.len(), 2);

    // Delete model-1
    let state3 = AppState::new(Arc::clone(&storage));
    for entry in assertions2.iter() {
        state3
            .assertions
            .insert(entry.key().clone(), entry.value().clone());
    }
    let assertions3 = Arc::clone(&state3.assertions);

    let app = create_router(state3);
    let status = delete_request(app, "/stores/test-store/authorization-models/model-1").await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Verify only model-1 assertions are removed
    assert_eq!(assertions3.len(), 1);
    assert!(!assertions3.contains_key(&("test-store".to_string(), "model-1".to_string())));
    assert!(assertions3.contains_key(&("test-store".to_string(), "model-2".to_string())));
}

/// Test that deleting a non-existent model returns 404.
#[tokio::test]
async fn test_delete_nonexistent_model_returns_404() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create a store
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    let state = create_test_state(storage);
    let app = create_router(state);

    // Try to delete a non-existent model
    let status = delete_request(app, "/stores/test-store/authorization-models/nonexistent").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Test that deleting a model from a non-existent store returns 404.
#[tokio::test]
async fn test_delete_model_from_nonexistent_store_returns_404() {
    let storage = Arc::new(MemoryDataStore::new());

    let state = create_test_state(storage);
    let app = create_router(state);

    // Try to delete a model from a non-existent store
    let status = delete_request(app, "/stores/nonexistent/authorization-models/model-1").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ============================================================================
// Edge Cases
// ============================================================================

/// Test that deleting a store with no assertions succeeds.
#[tokio::test]
async fn test_delete_store_with_no_assertions_succeeds() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create a store
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    let state = create_test_state(Arc::clone(&storage));
    let assertions = Arc::clone(&state.assertions);

    // Verify no assertions exist
    assert_eq!(assertions.len(), 0);

    // Delete the store
    let app = create_router(state);
    let status = delete_request(app, "/stores/test-store").await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Verify still no assertions
    assert_eq!(assertions.len(), 0);
}

/// Test that deleting a model with no assertions succeeds.
#[tokio::test]
async fn test_delete_model_with_no_assertions_succeeds() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create a store
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    // Create an authorization model
    let model_json = r#"{"type_definitions": [{"type": "user"}, {"type": "document", "relations": {"viewer": {"this": {}}}}]}"#;
    let model =
        StoredAuthorizationModel::new("model-1".to_string(), "test-store", "1.1", model_json);
    storage.write_authorization_model(model).await.unwrap();

    let state = create_test_state(Arc::clone(&storage));
    let assertions = Arc::clone(&state.assertions);

    // Verify no assertions exist
    assert_eq!(assertions.len(), 0);

    // Delete the model
    let app = create_router(state);
    let status = delete_request(app, "/stores/test-store/authorization-models/model-1").await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Verify still no assertions
    assert_eq!(assertions.len(), 0);
}
