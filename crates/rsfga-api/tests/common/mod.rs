//! Shared test utilities for RSFGA API tests.
//!
//! This module provides common test helpers used across integration,
//! security, and stress test suites.

// Allow dead_code because constants/functions are used across different test files,
// but Clippy analyzes each test file independently and can't see cross-file usage.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

// =============================================================================
// Test Constants
// =============================================================================

/// Number of tuples for large model tests (integration tests).
pub const LARGE_MODEL_TUPLE_COUNT: usize = 1000;

/// Batch size for writing tuples in large model tests.
pub const LARGE_MODEL_BATCH_SIZE: usize = 100;

/// Depth for hierarchy tests (matches OpenFGA default limit of 25).
pub const HIERARCHY_TEST_DEPTH: usize = 20;

/// Number of concurrent requests for stress tests.
pub const STRESS_TEST_CONCURRENT_REQUESTS: usize = 1000;

/// Number of concurrent requests for overload tests.
pub const OVERLOAD_TEST_CONCURRENT_REQUESTS: usize = 5000;

/// Timeout for stress test completion.
pub const STRESS_TEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Duration for sustained load tests.
pub const SUSTAINED_LOAD_DURATION: Duration = Duration::from_secs(5);

/// Number of workers for sustained load tests.
pub const SUSTAINED_LOAD_WORKERS: usize = 10;

/// Number of concurrent write operations for write stress tests.
pub const WRITE_STRESS_CONCURRENT_OPS: usize = 100;

/// Number of concurrent clients for integration tests.
pub const CONCURRENT_CLIENT_COUNT: usize = 50;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;

use rsfga_api::http::{create_router, AppState};
use rsfga_domain::cache::CheckCacheConfig;
use rsfga_storage::{DataStore, MemoryDataStore, StoredAuthorizationModel};

/// Set up a simple authorization model for tests.
///
/// Creates a model with user, team, and document types.
pub async fn setup_simple_model(storage: &MemoryDataStore, store_id: &str) {
    let model_json = r#"{
        "type_definitions": [
            {"type": "user"},
            {"type": "team", "relations": {"member": {}}},
            {"type": "document", "relations": {"viewer": {}, "editor": {}, "owner": {}}}
        ]
    }"#;
    let model =
        StoredAuthorizationModel::new(ulid::Ulid::new().to_string(), store_id, "1.1", model_json);
    storage.write_authorization_model(model).await.unwrap();
}

/// Create a test app with in-memory storage.
///
/// Each call creates a fresh `AppState` wrapping the shared storage,
/// which is the correct pattern for Axum's `oneshot` testing.
pub fn create_test_app(storage: &Arc<MemoryDataStore>) -> axum::Router {
    let state = AppState::new(Arc::clone(storage));
    create_router(state)
}

use rsfga_domain::cache::CheckCache;

/// Create a shared cache for testing.
///
/// Returns an Arc-wrapped cache that can be shared across multiple AppState instances.
pub fn create_shared_cache() -> Arc<CheckCache> {
    let cache_config = CheckCacheConfig::default().with_enabled(true);
    Arc::new(CheckCache::new(cache_config))
}

/// Create a shared cache with custom configuration.
pub fn create_shared_cache_with_config(cache_config: CheckCacheConfig) -> Arc<CheckCache> {
    Arc::new(CheckCache::new(cache_config))
}

/// Create a test app with a shared cache.
///
/// All AppState instances created with the same cache will share cache entries,
/// allowing proper testing of cache invalidation.
pub fn create_test_app_with_shared_cache(
    storage: &Arc<MemoryDataStore>,
    cache: &Arc<CheckCache>,
) -> axum::Router {
    let state = AppState::with_shared_cache(Arc::clone(storage), Arc::clone(cache));
    create_router(state)
}

/// Create a test app with caching enabled (convenience function).
///
/// Note: This creates a new cache for each call. For testing cache invalidation,
/// use `create_test_app_with_shared_cache` with a shared cache instead.
pub fn create_test_app_with_cache(storage: &Arc<MemoryDataStore>) -> axum::Router {
    let cache_config = CheckCacheConfig::default().with_enabled(true);
    let state = AppState::with_cache_config(Arc::clone(storage), cache_config);
    create_router(state)
}

/// Make a JSON POST request and return status + parsed JSON response.
pub async fn post_json(
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
    // Parse JSON, allowing empty or non-JSON responses (e.g., 413 Payload Too Large)
    let json: serde_json::Value = if body.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_slice(&body).unwrap_or_else(|_| {
            // For non-JSON responses (like plain text error messages), wrap in JSON
            serde_json::json!({
                "raw_body": String::from_utf8_lossy(&body).to_string()
            })
        })
    };
    (status, json)
}

/// Make a raw POST request with string body and return status only.
pub async fn post_raw(app: axum::Router, uri: &str, body: &str) -> StatusCode {
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
