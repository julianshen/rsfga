//! Shared test utilities for RSFGA API tests.
//!
//! This module provides common test helpers used across integration,
//! security, and stress test suites.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;

use rsfga_api::http::{create_router, AppState};
use rsfga_storage::MemoryDataStore;

/// Create a test app with in-memory storage.
///
/// Each call creates a fresh `AppState` wrapping the shared storage,
/// which is the correct pattern for Axum's `oneshot` testing.
pub fn create_test_app(storage: &Arc<MemoryDataStore>) -> axum::Router {
    let state = AppState::new(Arc::clone(storage));
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
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or(serde_json::json!({}));
    (status, json)
}

/// Make a raw POST request with string body and return status only.
#[allow(dead_code)]
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
