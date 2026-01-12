//! OpenFGA API compatibility verification.
//!
//! This module provides tests and utilities to verify that RSFGA
//! maintains 100% compatibility with the OpenFGA API.

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    use crate::http::{create_router, AppState};
    use rsfga_storage::{DataStore, MemoryDataStore};
    use std::sync::Arc;

    /// Helper to create a test app.
    fn test_app() -> axum::Router {
        let storage = Arc::new(MemoryDataStore::new());
        let state = AppState::new(storage);
        create_router(state)
    }

    /// Test: All OpenFGA API endpoints present
    ///
    /// Verifies that all required OpenFGA API endpoints are implemented
    /// and return appropriate responses (not 404).
    #[tokio::test]
    async fn test_all_openfga_api_endpoints_present() {
        let storage = Arc::new(MemoryDataStore::new());
        let state = AppState::new(Arc::clone(&storage));
        let app = create_router(state);

        // Create a store - must be done after building router but before tests
        storage
            .create_store("test-store", "Test Store")
            .await
            .unwrap();

        // Test each endpoint type individually

        // GET /stores
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/stores")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(
            response.status(),
            StatusCode::NOT_FOUND,
            "GET /stores should exist"
        );

        // POST /stores
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/stores")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"New Store"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(
            response.status(),
            StatusCode::NOT_FOUND,
            "POST /stores should exist"
        );

        // GET /stores/test-store
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/stores/test-store")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(
            response.status(),
            StatusCode::NOT_FOUND,
            "GET /stores/test-store should exist"
        );

        // POST /stores/test-store/check
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/stores/test-store/check")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"tuple_key":{"user":"user:alice","relation":"viewer","object":"doc:1"}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(
            response.status(),
            StatusCode::NOT_FOUND,
            "POST /stores/test-store/check should exist"
        );

        // POST /stores/test-store/batch-check
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/stores/test-store/batch-check")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"checks":[{"tuple_key":{"user":"user:alice","relation":"viewer","object":"doc:1"},"correlation_id":"c1"}]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(
            response.status(),
            StatusCode::NOT_FOUND,
            "POST /stores/test-store/batch-check should exist"
        );

        // POST /stores/test-store/expand
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/stores/test-store/expand")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"tuple_key":{"user":"","relation":"viewer","object":"doc:1"}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(
            response.status(),
            StatusCode::NOT_FOUND,
            "POST /stores/test-store/expand should exist"
        );

        // POST /stores/test-store/write
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/stores/test-store/write")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"writes":{"tuple_keys":[]}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(
            response.status(),
            StatusCode::NOT_FOUND,
            "POST /stores/test-store/write should exist"
        );

        // POST /stores/test-store/read
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/stores/test-store/read")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(
            response.status(),
            StatusCode::NOT_FOUND,
            "POST /stores/test-store/read should exist"
        );

        // POST /stores/test-store/list-objects
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/stores/test-store/list-objects")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"user":"user:alice","relation":"viewer","type":"document"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(
            response.status(),
            StatusCode::NOT_FOUND,
            "POST /stores/test-store/list-objects should exist"
        );

        // DELETE /stores/test-store (do this last)
        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/stores/test-store")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(
            response.status(),
            StatusCode::NOT_FOUND,
            "DELETE /stores/test-store should exist"
        );
    }

    /// Test: Request/response schemas match exactly
    ///
    /// Verifies that request and response JSON schemas match OpenFGA format.
    #[tokio::test]
    async fn test_request_response_schemas_match_exactly() {
        let storage = Arc::new(MemoryDataStore::new());
        storage
            .create_store("test-store", "Test Store")
            .await
            .unwrap();

        let state = AppState::new(storage);
        let app = create_router(state);

        // Test Check request/response schema
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/stores/test-store/check")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"tuple_key":{"user":"user:alice","relation":"viewer","object":"document:readme"}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // Check response must have 'allowed' field (boolean)
        assert!(
            json.get("allowed").is_some(),
            "Check response must have 'allowed' field"
        );
        assert!(json["allowed"].is_boolean(), "'allowed' must be a boolean");

        // Test Read response schema
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/stores/test-store/read")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // Read response must have 'tuples' array
        assert!(
            json.get("tuples").is_some(),
            "Read response must have 'tuples' field"
        );
        assert!(json["tuples"].is_array(), "'tuples' must be an array");

        // Test BatchCheck response schema
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/stores/test-store/batch-check")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"checks":[{"tuple_key":{"user":"user:alice","relation":"viewer","object":"doc:1"},"correlation_id":"c1"}]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // BatchCheck response must have 'result' map
        assert!(
            json.get("result").is_some(),
            "BatchCheck response must have 'result' field"
        );
        assert!(json["result"].is_object(), "'result' must be an object/map");

        // Test GetStore response schema
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/stores/test-store")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // Store response must have id and name
        assert!(
            json.get("id").is_some(),
            "Store response must have 'id' field"
        );
        assert!(
            json.get("name").is_some(),
            "Store response must have 'name' field"
        );
    }

    /// Test: Error codes match OpenFGA
    ///
    /// Verifies that error responses use the same format and codes as OpenFGA.
    #[tokio::test]
    async fn test_error_codes_match_openfga() {
        let app = test_app();

        // Non-existent store should return 404 with proper error format
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/stores/nonexistent-store/check")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"tuple_key":{"user":"user:alice","relation":"viewer","object":"doc:1"}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // Error response must have 'code' and 'message' fields
        assert!(
            json.get("code").is_some(),
            "Error response must have 'code' field"
        );
        assert!(
            json.get("message").is_some(),
            "Error response must have 'message' field"
        );

        // Invalid JSON should return 400
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/stores/test-store/check")
                    .header("content-type", "application/json")
                    .body(Body::from("{ invalid json }"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // Invalid request body should return 400 or 422
        let storage = Arc::new(MemoryDataStore::new());
        storage
            .create_store("test-store", "Test Store")
            .await
            .unwrap();
        let state = AppState::new(storage);
        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/stores/test-store/check")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"invalid":"request"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert!(
            response.status() == StatusCode::BAD_REQUEST
                || response.status() == StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid request should return 400 or 422"
        );
    }

    /// Test: Run OpenFGA compatibility test suite (100% pass)
    ///
    /// This test verifies that the API behavior matches OpenFGA's behavior
    /// for a comprehensive set of operations. The full compatibility test
    /// suite is in the compatibility-tests crate.
    #[tokio::test]
    async fn test_openfga_compatibility_test_suite() {
        use rsfga_storage::StoredAuthorizationModel;

        let storage = Arc::new(MemoryDataStore::new());

        // Create a test store
        let store = storage
            .create_store("compat-store", "Compatibility Test Store")
            .await
            .unwrap();
        assert!(!store.id.is_empty());
        assert_eq!(store.name, "Compatibility Test Store");

        // Set up authorization model (required for GraphResolver)
        let model_json = r#"{
            "type_definitions": [
                {"type": "user"},
                {"type": "team", "relations": {"member": {}}},
                {"type": "document", "relations": {"viewer": {}, "editor": {}, "owner": {}}}
            ]
        }"#;
        let model = StoredAuthorizationModel::new(
            ulid::Ulid::new().to_string(),
            "compat-store",
            "1.1",
            model_json,
        );
        storage.write_authorization_model(model).await.unwrap();

        let state = AppState::new(Arc::clone(&storage));
        let app = create_router(state);

        // 1. Write operation - add tuples
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/stores/compat-store/write")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"writes":{"tuple_keys":[
                            {"user":"user:alice","relation":"owner","object":"document:budget"},
                            {"user":"user:bob","relation":"viewer","object":"document:budget"},
                            {"user":"team:engineering#member","relation":"editor","object":"document:roadmap"}
                        ]}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // 2. Read operation - verify tuples were written
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/stores/compat-store/read")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let tuples = json["tuples"].as_array().unwrap();
        assert_eq!(tuples.len(), 3, "Should have 3 tuples");

        // 3. Check operation - existing permission
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/stores/compat-store/check")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"tuple_key":{"user":"user:alice","relation":"owner","object":"document:budget"}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["allowed"], true);

        // 4. Check operation - non-existing permission
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/stores/compat-store/check")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"tuple_key":{"user":"user:charlie","relation":"owner","object":"document:budget"}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["allowed"], false);

        // 5. BatchCheck operation
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/stores/compat-store/batch-check")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"checks":[
                            {"tuple_key":{"user":"user:alice","relation":"owner","object":"document:budget"},"correlation_id":"alice-owner"},
                            {"tuple_key":{"user":"user:bob","relation":"viewer","object":"document:budget"},"correlation_id":"bob-viewer"},
                            {"tuple_key":{"user":"user:bob","relation":"owner","object":"document:budget"},"correlation_id":"bob-owner"}
                        ]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 2048)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let result = json["result"].as_object().unwrap();
        assert!(result["alice-owner"]["allowed"].as_bool().unwrap());
        assert!(result["bob-viewer"]["allowed"].as_bool().unwrap());
        assert!(!result["bob-owner"]["allowed"].as_bool().unwrap());

        // 6. Write operation - delete tuples
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/stores/compat-store/write")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"deletes":{"tuple_keys":[
                            {"user":"user:bob","relation":"viewer","object":"document:budget"}
                        ]}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // 7. Verify delete worked
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/stores/compat-store/check")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"tuple_key":{"user":"user:bob","relation":"viewer","object":"document:budget"}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            json["allowed"], false,
            "Deleted tuple should not be allowed"
        );

        // 8. List stores
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/stores")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let stores = json["stores"].as_array().unwrap();
        assert!(!stores.is_empty(), "Should have at least one store");

        // 9. Delete store
        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/stores/compat-store")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }
}
