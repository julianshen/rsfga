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

/// Set up a test store with a simple authorization model.
///
/// Creates a new store with a unique ID and writes an authorization model
/// with user and document types. Returns the store ID.
pub async fn setup_test_store(storage: &MemoryDataStore) -> String {
    let store_id = ulid::Ulid::new().to_string();

    // Create store
    storage.create_store(&store_id, "Test Store").await.unwrap();

    // Create authorization model with document and user types
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

// =============================================================================
// Metrics Test Helpers
// =============================================================================

use metrics_exporter_prometheus::PrometheusBuilder;
use rsfga_api::http::create_router_with_observability;
use rsfga_api::observability::MetricsState;
use std::sync::OnceLock;

/// Global metrics state for tests.
///
/// Because the metrics crate uses a global recorder, we need to share a single
/// recorder across all tests. This is initialized lazily on first use.
static GLOBAL_METRICS_STATE: OnceLock<MetricsState> = OnceLock::new();

/// Initialize the global metrics recorder (if not already initialized).
///
/// Returns the MetricsState that can be used to render metrics.
/// This is safe to call multiple times - only the first call initializes the recorder.
pub fn init_global_metrics() -> MetricsState {
    GLOBAL_METRICS_STATE
        .get_or_init(|| {
            let builder = PrometheusBuilder::new();
            let handle = builder
                .install_recorder()
                .expect("Failed to install metrics recorder");
            MetricsState::new(handle)
        })
        .clone()
}

/// Create a test app with metrics enabled.
///
/// Returns (Router, MetricsState) where MetricsState can be used to read metrics.
///
/// **NOTE**: Uses the global metrics recorder. Call metrics operations before
/// calling `metrics_state.render()` to see them in the output.
pub fn create_test_app_with_metrics(
    storage: &Arc<MemoryDataStore>,
) -> (axum::Router, MetricsState) {
    let metrics_state = init_global_metrics();
    let state = AppState::new(Arc::clone(storage));
    let router = create_router_with_observability(state, metrics_state.clone());

    (router, metrics_state)
}

/// Create a test app with metrics enabled and shared cache.
pub fn create_test_app_with_metrics_and_cache(
    storage: &Arc<MemoryDataStore>,
    cache: &Arc<CheckCache>,
) -> (axum::Router, MetricsState) {
    let metrics_state = init_global_metrics();
    let state = AppState::with_shared_cache(Arc::clone(storage), Arc::clone(cache));
    let router = create_router_with_observability(state, metrics_state.clone());

    (router, metrics_state)
}

/// Fetch metrics from the /metrics endpoint and return raw text.
pub async fn get_metrics(app: axum::Router) -> (StatusCode, String) {
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();

    (status, String::from_utf8_lossy(&body).to_string())
}

/// Parse a specific counter metric value from Prometheus output.
///
/// Returns Some(value) if the metric is found, None otherwise.
/// Handles metrics with or without labels.
///
/// # Arguments
///
/// * `metrics_text` - Raw Prometheus metrics text
/// * `metric_name` - Name of the metric to find (e.g., "rsfga_cache_hits_total")
pub fn parse_metric_value(metrics_text: &str, metric_name: &str) -> Option<f64> {
    for line in metrics_text.lines() {
        // Skip comments and empty lines
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }

        // Parse line format: metric_name{labels} value OR metric_name value
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let name = parts[0].split('{').next().unwrap_or(parts[0]);
            if name == metric_name {
                return parts[1].parse().ok();
            }
        }
    }
    None
}

/// Check if a metric exists in the Prometheus output.
pub fn metric_exists(metrics_text: &str, metric_name: &str) -> bool {
    for line in metrics_text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if !parts.is_empty() {
            let name = parts[0].split('{').next().unwrap_or(parts[0]);
            if name == metric_name {
                return true;
            }
        }
    }
    false
}
