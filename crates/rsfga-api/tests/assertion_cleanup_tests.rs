//! Tests for assertion cleanup when stores or models are deleted.
//!
//! This test suite verifies that assertions are properly cleaned up:
//! - When a store is deleted, all assertions for that store are removed
//! - When an authorization model is deleted, assertions for that model are removed
//!
//! # Test Design
//!
//! These tests use a single shared AppState per test to accurately reflect
//! production behavior where all requests share the same in-memory state.
//! The assertions map is accessed via Arc to verify cleanup without creating
//! multiple AppState instances.
//!
//! See: https://github.com/julianshen/rsfga/issues/237

mod common;

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use dashmap::DashMap;
use tower::ServiceExt;

use rsfga_api::http::{create_router, AppState, AssertionKey, StoredAssertion};
use rsfga_storage::{DataStore, MemoryDataStore, StoredAuthorizationModel};

// ============================================================================
// Test Helpers
// ============================================================================

/// Test context holding shared state for assertion cleanup tests.
///
/// This ensures all requests in a test share the same AppState, accurately
/// reflecting production behavior.
struct TestContext {
    storage: Arc<MemoryDataStore>,
    assertions: Arc<DashMap<AssertionKey, Vec<StoredAssertion>>>,
    router: axum::Router,
}

impl TestContext {
    /// Create a new test context with fresh storage and state.
    fn new() -> Self {
        let storage = Arc::new(MemoryDataStore::new());
        let state = AppState::new(Arc::clone(&storage));
        let assertions = Arc::clone(&state.assertions);
        let router = create_router(state);

        Self {
            storage,
            assertions,
            router,
        }
    }

    /// Make a PUT request (for write_assertions).
    ///
    /// Note: We clone the router for each request. The cloned router shares
    /// the same underlying state (storage, assertions via Arc), so this
    /// accurately reflects production behavior.
    async fn put_json(
        &self,
        uri: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let request = Request::builder()
            .method("PUT")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let response = self.router.clone().oneshot(request).await.unwrap();
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

    /// Make a DELETE request.
    async fn delete(&self, uri: &str) -> StatusCode {
        let request = Request::builder()
            .method("DELETE")
            .uri(uri)
            .body(Body::empty())
            .unwrap();

        let response = self.router.clone().oneshot(request).await.unwrap();
        response.status()
    }

    /// Create a store in storage.
    async fn create_store(&self, store_id: &str, name: &str) {
        self.storage.create_store(store_id, name).await.unwrap();
    }

    /// Create an authorization model in storage.
    async fn create_model(&self, model_id: &str, store_id: &str) {
        let model_json = r#"{"type_definitions": [{"type": "user"}, {"type": "document", "relations": {"viewer": {"this": {}}}}]}"#;
        let model =
            StoredAuthorizationModel::new(model_id.to_string(), store_id, "1.1", model_json);
        self.storage.write_authorization_model(model).await.unwrap();
    }

    /// Write a test assertion for a model.
    async fn write_assertion(&self, store_id: &str, model_id: &str, user: &str) {
        let (status, _) = self
            .put_json(
                &format!("/stores/{store_id}/assertions/{model_id}"),
                serde_json::json!({
                    "assertions": [{
                        "tuple_key": {
                            "user": user,
                            "relation": "viewer",
                            "object": "document:readme"
                        },
                        "expectation": true
                    }]
                }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "Failed to write assertion");
    }

    /// Get the number of assertion entries.
    fn assertion_count(&self) -> usize {
        self.assertions.len()
    }

    /// Check if an assertion key exists.
    fn has_assertion(&self, store_id: &str, model_id: &str) -> bool {
        self.assertions
            .contains_key(&(store_id.to_string(), model_id.to_string()))
    }
}

// ============================================================================
// Store Deletion Tests
// ============================================================================

/// Test that deleting a store cleans up all associated assertions.
#[tokio::test]
async fn test_delete_store_cleans_up_assertions() {
    let ctx = TestContext::new();

    // Setup: Create store, model, and assertion
    ctx.create_store("test-store", "Test Store").await;
    ctx.create_model("model-1", "test-store").await;
    ctx.write_assertion("test-store", "model-1", "user:alice")
        .await;

    // Verify assertion exists
    assert_eq!(ctx.assertion_count(), 1);
    assert!(ctx.has_assertion("test-store", "model-1"));

    // Delete the store
    let status = ctx.delete("/stores/test-store").await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Verify assertions are cleaned up
    assert_eq!(ctx.assertion_count(), 0);
}

/// Test that deleting a store cleans up assertions for multiple models.
#[tokio::test]
async fn test_delete_store_cleans_up_multiple_model_assertions() {
    let ctx = TestContext::new();

    // Setup: Create store with two models and assertions
    ctx.create_store("test-store", "Test Store").await;
    ctx.create_model("model-1", "test-store").await;
    ctx.create_model("model-2", "test-store").await;
    ctx.write_assertion("test-store", "model-1", "user:alice")
        .await;
    ctx.write_assertion("test-store", "model-2", "user:bob")
        .await;

    // Verify both assertions exist
    assert_eq!(ctx.assertion_count(), 2);

    // Delete the store
    let status = ctx.delete("/stores/test-store").await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Verify all assertions are cleaned up
    assert_eq!(ctx.assertion_count(), 0);
}

/// Test that deleting one store doesn't affect assertions from other stores.
#[tokio::test]
async fn test_delete_store_preserves_other_store_assertions() {
    let ctx = TestContext::new();

    // Setup: Create two stores with assertions
    ctx.create_store("store-1", "Store 1").await;
    ctx.create_store("store-2", "Store 2").await;
    ctx.create_model("model-a", "store-1").await;
    ctx.create_model("model-b", "store-2").await;
    ctx.write_assertion("store-1", "model-a", "user:alice")
        .await;
    ctx.write_assertion("store-2", "model-b", "user:bob").await;

    // Verify both assertions exist
    assert_eq!(ctx.assertion_count(), 2);

    // Delete store-1
    let status = ctx.delete("/stores/store-1").await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Verify only store-1 assertions are removed
    assert_eq!(ctx.assertion_count(), 1);
    assert!(!ctx.has_assertion("store-1", "model-a"));
    assert!(ctx.has_assertion("store-2", "model-b"));
}

// ============================================================================
// Authorization Model Deletion Tests
// ============================================================================

/// Test that deleting an authorization model cleans up its assertions.
#[tokio::test]
async fn test_delete_authorization_model_cleans_up_assertions() {
    let ctx = TestContext::new();

    // Setup: Create store, model, and assertion
    ctx.create_store("test-store", "Test Store").await;
    ctx.create_model("model-1", "test-store").await;
    ctx.write_assertion("test-store", "model-1", "user:alice")
        .await;

    // Verify assertion exists
    assert_eq!(ctx.assertion_count(), 1);

    // Delete the authorization model
    let status = ctx
        .delete("/stores/test-store/authorization-models/model-1")
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Verify assertions are cleaned up
    assert_eq!(ctx.assertion_count(), 0);
}

/// Test that deleting one model doesn't affect assertions from other models.
#[tokio::test]
async fn test_delete_model_preserves_other_model_assertions() {
    let ctx = TestContext::new();

    // Setup: Create store with two models and assertions
    ctx.create_store("test-store", "Test Store").await;
    ctx.create_model("model-1", "test-store").await;
    ctx.create_model("model-2", "test-store").await;
    ctx.write_assertion("test-store", "model-1", "user:alice")
        .await;
    ctx.write_assertion("test-store", "model-2", "user:bob")
        .await;

    // Verify both assertions exist
    assert_eq!(ctx.assertion_count(), 2);

    // Delete model-1
    let status = ctx
        .delete("/stores/test-store/authorization-models/model-1")
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Verify only model-1 assertions are removed
    assert_eq!(ctx.assertion_count(), 1);
    assert!(!ctx.has_assertion("test-store", "model-1"));
    assert!(ctx.has_assertion("test-store", "model-2"));
}

/// Test that deleting a non-existent model returns 404.
#[tokio::test]
async fn test_delete_nonexistent_model_returns_404() {
    let ctx = TestContext::new();

    // Setup: Create store only (no model)
    ctx.create_store("test-store", "Test Store").await;

    // Try to delete a non-existent model
    let status = ctx
        .delete("/stores/test-store/authorization-models/nonexistent")
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Test that deleting a model from a non-existent store returns 404.
#[tokio::test]
async fn test_delete_model_from_nonexistent_store_returns_404() {
    let ctx = TestContext::new();

    // Try to delete a model from a non-existent store
    let status = ctx
        .delete("/stores/nonexistent/authorization-models/model-1")
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ============================================================================
// Edge Cases
// ============================================================================

/// Test that deleting a store with no assertions succeeds.
#[tokio::test]
async fn test_delete_store_with_no_assertions_succeeds() {
    let ctx = TestContext::new();

    // Setup: Create store only (no assertions)
    ctx.create_store("test-store", "Test Store").await;

    // Verify no assertions exist
    assert_eq!(ctx.assertion_count(), 0);

    // Delete the store
    let status = ctx.delete("/stores/test-store").await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Verify still no assertions
    assert_eq!(ctx.assertion_count(), 0);
}

/// Test that deleting a model with no assertions succeeds.
#[tokio::test]
async fn test_delete_model_with_no_assertions_succeeds() {
    let ctx = TestContext::new();

    // Setup: Create store and model (no assertions)
    ctx.create_store("test-store", "Test Store").await;
    ctx.create_model("model-1", "test-store").await;

    // Verify no assertions exist
    assert_eq!(ctx.assertion_count(), 0);

    // Delete the model
    let status = ctx
        .delete("/stores/test-store/authorization-models/model-1")
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Verify still no assertions
    assert_eq!(ctx.assertion_count(), 0);
}
