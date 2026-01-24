//! Pagination Integration Tests for Issue #207.
//!
//! These tests verify that pagination state works correctly across
//! multiple API calls for all paginated endpoints.
//!
//! ## Test Coverage
//!
//! ### Read Tuples Pagination
//! - Read with page_size returns exactly N tuples and continuation_token
//! - Second read with continuation token returns next N tuples, no overlap
//! - Final page returns remaining tuples with no continuation_token
//! - Empty result set returns no continuation_token
//! - Invalid continuation token returns appropriate error (400)
//!
//! ### Read Changes Pagination
//! - Read changes pagination works correctly
//! - Changes not skipped or duplicated across pages
//!
//! ### List Authorization Models Pagination
//! - List models pagination works correctly
//! - Page size clamped to maximum (50)
//!
//! ### List Objects/Users Truncation
//! - List Objects handles large result sets with truncation
//! - List Users handles large result sets correctly
//!
//! ### Cross-Store Token Validation
//! - Continuation token from store A doesn't work for store B queries
//!
//! Run with: cargo test --test pagination_tests

mod common;

use std::collections::HashSet;
use std::sync::Arc;

use axum::http::StatusCode;
use rsfga_storage::{DataStore, MemoryDataStore, StoredAuthorizationModel, StoredTuple, Utc};

use common::{create_test_app, post_json, setup_simple_model};

// =============================================================================
// Helper Functions
// =============================================================================

/// Create a store with authorization model and return the store ID.
async fn create_store_with_model(storage: &Arc<MemoryDataStore>) -> String {
    let (status, response) = post_json(
        create_test_app(storage),
        "/stores",
        serde_json::json!({
            "name": "pagination-test-store"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();

    setup_simple_model(storage, &store_id).await;

    store_id
}

/// Write N tuples to storage for pagination testing.
async fn write_test_tuples(storage: &Arc<MemoryDataStore>, store_id: &str, count: usize) {
    for i in 0..count {
        let tuple = StoredTuple {
            user_type: "user".to_string(),
            user_id: format!("user{i:04}"),
            user_relation: None,
            relation: "viewer".to_string(),
            object_type: "document".to_string(),
            object_id: format!("doc{i:04}"),
            condition_name: None,
            condition_context: None,
            created_at: Some(Utc::now()),
        };
        storage.write_tuple(store_id, tuple).await.unwrap();
    }
}

/// Make a GET request and return status + parsed JSON response.
async fn get_json(app: axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .body(Body::empty())
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
        serde_json::from_slice(&body).unwrap_or_else(|_| {
            serde_json::json!({
                "raw_body": String::from_utf8_lossy(&body).to_string()
            })
        })
    };
    (status, json)
}

// =============================================================================
// Section 1: Read Tuples Pagination Tests
// =============================================================================

/// Test: Read with page_size returns exactly N tuples and continuation_token.
#[tokio::test]
async fn test_read_tuples_with_page_size_returns_exact_count() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    // Write 25 tuples
    write_test_tuples(&storage, &store_id, 25).await;

    // Read with page_size=10
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/read"),
        serde_json::json!({
            "page_size": 10
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let tuples = response["tuples"]
        .as_array()
        .expect("Should have tuples array");

    assert_eq!(tuples.len(), 10, "Should return exactly 10 tuples");

    assert!(
        response["continuation_token"].is_string(),
        "Should have continuation_token when more results exist"
    );
}

/// Test: Second read with continuation token returns next N tuples, no overlap.
#[tokio::test]
async fn test_read_tuples_continuation_no_overlap() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    // Write 25 tuples
    write_test_tuples(&storage, &store_id, 25).await;

    // First read
    let (status, response1) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/read"),
        serde_json::json!({
            "page_size": 10
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let tuples1: HashSet<String> = response1["tuples"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["key"]["object"].as_str().unwrap().to_string())
        .collect();

    let token1 = response1["continuation_token"]
        .as_str()
        .expect("Should have token");

    // Second read with continuation token
    let (status, response2) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/read"),
        serde_json::json!({
            "page_size": 10,
            "continuation_token": token1
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let tuples2: HashSet<String> = response2["tuples"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["key"]["object"].as_str().unwrap().to_string())
        .collect();

    // Verify no overlap
    let overlap: HashSet<_> = tuples1.intersection(&tuples2).collect();
    assert!(
        overlap.is_empty(),
        "Pages should not overlap. Overlapping items: {:?}",
        overlap
    );

    assert_eq!(tuples2.len(), 10, "Second page should also have 10 tuples");
}

/// Test: Final page returns remaining tuples with no continuation_token.
#[tokio::test]
async fn test_read_tuples_final_page_no_token() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    // Write 15 tuples
    write_test_tuples(&storage, &store_id, 15).await;

    // First read (10 tuples)
    let (status, response1) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/read"),
        serde_json::json!({
            "page_size": 10
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let token1 = response1["continuation_token"]
        .as_str()
        .expect("Should have token");

    // Second read (remaining 5 tuples)
    let (status, response2) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/read"),
        serde_json::json!({
            "page_size": 10,
            "continuation_token": token1
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let tuples2 = response2["tuples"].as_array().unwrap();
    assert_eq!(
        tuples2.len(),
        5,
        "Final page should have 5 remaining tuples"
    );

    assert!(
        response2["continuation_token"].is_null() || response2.get("continuation_token").is_none(),
        "Final page should not have continuation_token"
    );
}

/// Test: Empty result set returns no continuation_token.
#[tokio::test]
async fn test_read_tuples_empty_result_no_token() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    // No tuples written

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/read"),
        serde_json::json!({
            "page_size": 10
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let tuples = response["tuples"].as_array().unwrap();
    assert_eq!(tuples.len(), 0, "Should return empty array");

    assert!(
        response["continuation_token"].is_null() || response.get("continuation_token").is_none(),
        "Empty result should not have continuation_token"
    );
}

/// Test: Invalid continuation token returns appropriate error (400).
#[tokio::test]
async fn test_read_tuples_invalid_continuation_token_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    write_test_tuples(&storage, &store_id, 10).await;

    // Read with invalid continuation token
    let (status, _response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/read"),
        serde_json::json!({
            "page_size": 10,
            "continuation_token": "invalid-token-not-base64"
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Invalid continuation token should return 400"
    );
}

/// Test: Negative page_size returns error.
#[tokio::test]
async fn test_read_tuples_negative_page_size_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/read"),
        serde_json::json!({
            "page_size": -10
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Negative page_size should return 400, got: {} - {:?}",
        status,
        response
    );
}

/// Test: Zero page_size returns error.
#[tokio::test]
async fn test_read_tuples_zero_page_size_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    let (status, _response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/read"),
        serde_json::json!({
            "page_size": 0
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Zero page_size should return 400"
    );
}

/// Test: Read all tuples across multiple pages collects all without duplicates.
#[tokio::test]
async fn test_read_tuples_pagination_collects_all() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    // Write 45 tuples
    write_test_tuples(&storage, &store_id, 45).await;

    let mut all_objects: HashSet<String> = HashSet::new();
    let mut continuation_token: Option<String> = None;
    let mut page_count = 0;

    loop {
        let request = if let Some(ref token) = continuation_token {
            serde_json::json!({
                "page_size": 10,
                "continuation_token": token
            })
        } else {
            serde_json::json!({
                "page_size": 10
            })
        };

        let (status, response) = post_json(
            create_test_app(&storage),
            &format!("/stores/{store_id}/read"),
            request,
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let tuples = response["tuples"].as_array().unwrap();
        for tuple in tuples {
            let object = tuple["key"]["object"].as_str().unwrap().to_string();
            let is_new = all_objects.insert(object.clone());
            assert!(is_new, "Duplicate object found: {}", object);
        }

        page_count += 1;

        continuation_token = response["continuation_token"].as_str().map(String::from);
        if continuation_token.is_none() {
            break;
        }
    }

    assert_eq!(all_objects.len(), 45, "Should collect all 45 tuples");
    assert_eq!(page_count, 5, "Should take 5 pages (10+10+10+10+5)");
}

// =============================================================================
// Section 2: Read Changes Pagination Tests
// =============================================================================

/// Test: Read changes pagination works correctly.
#[tokio::test]
async fn test_read_changes_pagination_works() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    // Write 25 tuples (each creates a change)
    write_test_tuples(&storage, &store_id, 25).await;

    // First read changes
    let (status, response1) = get_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/changes?page_size=10"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let changes1 = response1["changes"].as_array().unwrap();
    assert_eq!(changes1.len(), 10, "Should return 10 changes");

    let token1 = response1["continuation_token"]
        .as_str()
        .expect("Should have token");

    // Second read with continuation token
    let (status, response2) = get_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/changes?page_size=10&continuation_token={token1}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let changes2 = response2["changes"].as_array().unwrap();
    assert_eq!(changes2.len(), 10, "Should return 10 more changes");
}

/// Test: Read changes collects all without skipping.
#[tokio::test]
async fn test_read_changes_no_skipped_items() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    // Write 35 tuples
    write_test_tuples(&storage, &store_id, 35).await;

    let mut all_operations: Vec<String> = Vec::new();
    let mut continuation_token: Option<String> = None;

    loop {
        let url = if let Some(ref token) = continuation_token {
            format!("/stores/{store_id}/changes?page_size=10&continuation_token={token}")
        } else {
            format!("/stores/{store_id}/changes?page_size=10")
        };

        let (status, response) = get_json(create_test_app(&storage), &url).await;
        assert_eq!(status, StatusCode::OK);

        let changes = response["changes"].as_array().unwrap();
        for change in changes {
            let tuple_key = &change["tuple_key"];
            let operation = format!(
                "{}:{}:{}",
                tuple_key["user"].as_str().unwrap_or_default(),
                tuple_key["relation"].as_str().unwrap_or_default(),
                tuple_key["object"].as_str().unwrap_or_default()
            );
            all_operations.push(operation);
        }

        continuation_token = response["continuation_token"].as_str().map(String::from);
        if continuation_token.is_none() {
            break;
        }
    }

    assert_eq!(
        all_operations.len(),
        35,
        "Should collect all 35 changes without skipping"
    );
}

// =============================================================================
// Section 3: List Authorization Models Pagination Tests
// =============================================================================

/// Test: List authorization models pagination works correctly.
#[tokio::test]
async fn test_list_authorization_models_pagination_works() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create store
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({
            "name": "models-pagination-test"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap();

    // Create 15 authorization models
    for i in 0..15 {
        let model_json = serde_json::json!({
            "type_definitions": [
                {"type": format!("type{i}")},
                {"type": "user"}
            ]
        });
        let model = StoredAuthorizationModel::new(
            ulid::Ulid::new().to_string(),
            store_id,
            "1.1",
            serde_json::to_string(&model_json).unwrap(),
        );
        storage.write_authorization_model(model).await.unwrap();
        // Small delay to ensure different timestamps
        tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
    }

    // First page (page_size=10)
    let (status, response1) = get_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/authorization-models?page_size=10"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let models1 = response1["authorization_models"].as_array().unwrap();
    assert_eq!(models1.len(), 10, "Should return 10 models");

    let token1 = response1["continuation_token"]
        .as_str()
        .expect("Should have token");

    // Second page
    let (status, response2) = get_json(
        create_test_app(&storage),
        &format!(
            "/stores/{store_id}/authorization-models?page_size=10&continuation_token={token1}"
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let models2 = response2["authorization_models"].as_array().unwrap();
    assert_eq!(models2.len(), 5, "Should return remaining 5 models");

    assert!(
        response2["continuation_token"].is_null() || response2.get("continuation_token").is_none(),
        "Final page should not have token"
    );
}

/// Test: List authorization models page_size is clamped to maximum (50).
#[tokio::test]
async fn test_list_authorization_models_page_size_clamped() {
    let storage = Arc::new(MemoryDataStore::new());

    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({
            "name": "page-size-clamp-test"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap();

    // Create 60 models
    for i in 0..60 {
        let model_json = serde_json::json!({
            "type_definitions": [
                {"type": format!("type{i}")},
                {"type": "user"}
            ]
        });
        let model = StoredAuthorizationModel::new(
            ulid::Ulid::new().to_string(),
            store_id,
            "1.1",
            serde_json::to_string(&model_json).unwrap(),
        );
        storage.write_authorization_model(model).await.unwrap();
    }

    // Request with page_size=100 (should be clamped to 50)
    let (status, response) = get_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/authorization-models?page_size=100"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let models = response["authorization_models"].as_array().unwrap();
    assert!(
        models.len() <= 50,
        "Page size should be clamped to 50, got {}",
        models.len()
    );
}

/// Test: List authorization models collects all without duplicates.
#[tokio::test]
async fn test_list_authorization_models_no_duplicates() {
    let storage = Arc::new(MemoryDataStore::new());

    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({
            "name": "no-duplicates-test"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap();

    // Create 25 models
    for i in 0..25 {
        let model_json = serde_json::json!({
            "type_definitions": [
                {"type": format!("type{i}")},
                {"type": "user"}
            ]
        });
        let model = StoredAuthorizationModel::new(
            ulid::Ulid::new().to_string(),
            store_id,
            "1.1",
            serde_json::to_string(&model_json).unwrap(),
        );
        storage.write_authorization_model(model).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
    }

    let mut all_ids: HashSet<String> = HashSet::new();
    let mut continuation_token: Option<String> = None;

    loop {
        let url = if let Some(ref token) = continuation_token {
            format!(
                "/stores/{store_id}/authorization-models?page_size=10&continuation_token={token}"
            )
        } else {
            format!("/stores/{store_id}/authorization-models?page_size=10")
        };

        let (status, response) = get_json(create_test_app(&storage), &url).await;
        assert_eq!(status, StatusCode::OK);

        let models = response["authorization_models"].as_array().unwrap();
        for model in models {
            let id = model["id"].as_str().unwrap().to_string();
            let is_new = all_ids.insert(id.clone());
            assert!(is_new, "Duplicate model ID found: {}", id);
        }

        continuation_token = response["continuation_token"].as_str().map(String::from);
        if continuation_token.is_none() {
            break;
        }
    }

    assert_eq!(all_ids.len(), 25, "Should collect all 25 models");
}

// =============================================================================
// Section 4: List Objects/Users Truncation Tests
// =============================================================================

/// Test: List Objects returns results without hanging on large datasets.
#[tokio::test]
async fn test_listobjects_handles_large_result_sets() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    // Write 100 tuples - alice can view 100 documents
    for i in 0..100 {
        let tuple = StoredTuple {
            user_type: "user".to_string(),
            user_id: "alice".to_string(),
            user_relation: None,
            relation: "viewer".to_string(),
            object_type: "document".to_string(),
            object_id: format!("doc{i:04}"),
            condition_name: None,
            condition_context: None,
            created_at: Some(Utc::now()),
        };
        storage.write_tuple(&store_id, tuple).await.unwrap();
    }

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-objects"),
        serde_json::json!({
            "type": "document",
            "relation": "viewer",
            "user": "user:alice"
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let objects = response["objects"].as_array().unwrap();
    assert!(
        !objects.is_empty(),
        "Should return objects for large result set"
    );
    assert!(objects.len() <= 1000, "Should not exceed max limit of 1000");
}

/// Test: List Users handles large result sets correctly.
#[tokio::test]
async fn test_listusers_handles_large_result_sets() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    // Write 100 tuples - 100 different users can view doc1
    for i in 0..100 {
        let tuple = StoredTuple {
            user_type: "user".to_string(),
            user_id: format!("user{i:04}"),
            user_relation: None,
            relation: "viewer".to_string(),
            object_type: "document".to_string(),
            object_id: "doc1".to_string(),
            condition_name: None,
            condition_context: None,
            created_at: Some(Utc::now()),
        };
        storage.write_tuple(&store_id, tuple).await.unwrap();
    }

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-users"),
        serde_json::json!({
            "object": {"type": "document", "id": "doc1"},
            "relation": "viewer",
            "user_filters": [{"type": "user"}]
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let users = response["users"].as_array().unwrap();
    assert_eq!(
        users.len(),
        100,
        "Should return all 100 users when under limit"
    );
}

/// Test: List Users truncates at 1000 users.
#[tokio::test]
async fn test_listusers_truncates_at_limit() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    // Write 1500 tuples - 1500 different users can view doc1
    for i in 0..1500 {
        let tuple = StoredTuple {
            user_type: "user".to_string(),
            user_id: format!("user{i:04}"),
            user_relation: None,
            relation: "viewer".to_string(),
            object_type: "document".to_string(),
            object_id: "truncate-doc".to_string(),
            condition_name: None,
            condition_context: None,
            created_at: Some(Utc::now()),
        };
        storage.write_tuple(&store_id, tuple).await.unwrap();
    }

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-users"),
        serde_json::json!({
            "object": {"type": "document", "id": "truncate-doc"},
            "relation": "viewer",
            "user_filters": [{"type": "user"}]
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let users = response["users"].as_array().unwrap();
    assert!(
        users.len() <= 1000,
        "Should truncate to max 1000 users, got {}",
        users.len()
    );
}

// =============================================================================
// Section 5: Cross-Store Token Validation Tests
// =============================================================================

/// Test: Continuation token from store A doesn't work for store B queries.
///
/// This test verifies that using a continuation token from one store with
/// another store either fails or returns the correct store's data (no leakage).
#[tokio::test]
async fn test_cross_store_token_validation() {
    let storage = Arc::new(MemoryDataStore::new());

    // Create store A
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({
            "name": "store-a"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_a = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_a).await;

    // Create store B
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores",
        serde_json::json!({
            "name": "store-b"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_b = response["id"].as_str().unwrap().to_string();
    setup_simple_model(&storage, &store_b).await;

    // Write DISTINGUISHABLE tuples to store A (prefixed with "store_a_")
    for i in 0..25 {
        let tuple = StoredTuple {
            user_type: "user".to_string(),
            user_id: format!("store_a_user{i:04}"),
            user_relation: None,
            relation: "viewer".to_string(),
            object_type: "document".to_string(),
            object_id: format!("store_a_doc{i:04}"),
            condition_name: None,
            condition_context: None,
            created_at: Some(Utc::now()),
        };
        storage.write_tuple(&store_a, tuple).await.unwrap();
    }

    // Write DISTINGUISHABLE tuples to store B (prefixed with "store_b_")
    for i in 0..25 {
        let tuple = StoredTuple {
            user_type: "user".to_string(),
            user_id: format!("store_b_user{i:04}"),
            user_relation: None,
            relation: "viewer".to_string(),
            object_type: "document".to_string(),
            object_id: format!("store_b_doc{i:04}"),
            condition_name: None,
            condition_context: None,
            created_at: Some(Utc::now()),
        };
        storage.write_tuple(&store_b, tuple).await.unwrap();
    }

    // Get continuation token from store A
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_a}/read"),
        serde_json::json!({
            "page_size": 10
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Verify first page contains store A data
    let store_a_tuples = response["tuples"].as_array().unwrap();
    for tuple in store_a_tuples {
        let user = tuple["key"]["user"].as_str().unwrap();
        assert!(
            user.starts_with("user:store_a_"),
            "Store A should return store_a_ prefixed users, got: {}",
            user
        );
    }

    let token_from_store_a = response["continuation_token"]
        .as_str()
        .expect("Should have token");

    // Try to use store A's token with store B
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_b}/read"),
        serde_json::json!({
            "page_size": 10,
            "continuation_token": token_from_store_a
        }),
    )
    .await;

    // This documents actual behavior:
    // - If tokens are store-scoped: Should return 400 or empty results
    // - If tokens are just cursors: May work but MUST return store B's data only
    // The key requirement is NO DATA LEAKAGE from store A to store B
    assert!(
        status == StatusCode::OK || status == StatusCode::BAD_REQUEST,
        "Cross-store token should either work (different data) or fail, got: {}",
        status
    );

    // CRITICAL: If status is OK, verify NO data leakage - all tuples must belong to store B
    if status == StatusCode::OK {
        let tuples = response["tuples"].as_array().unwrap();
        for tuple in tuples {
            let user = tuple["key"]["user"].as_str().unwrap();
            let object = tuple["key"]["object"].as_str().unwrap();

            // Verify user belongs to store B (not store A)
            assert!(
                !user.contains("store_a_"),
                "DATA LEAKAGE: Store B returned store A's user data: {}",
                user
            );

            // Verify object belongs to store B (not store A)
            assert!(
                !object.contains("store_a_"),
                "DATA LEAKAGE: Store B returned store A's object data: {}",
                object
            );

            // Verify data is from store B
            assert!(
                user.starts_with("user:store_b_"),
                "Store B should only return store_b_ prefixed users, got: {}",
                user
            );
            assert!(
                object.starts_with("document:store_b_"),
                "Store B should only return store_b_ prefixed objects, got: {}",
                object
            );
        }
    }
}

// =============================================================================
// Section 6: Edge Cases
// =============================================================================

/// Test: Pagination with filter returns filtered results on first page.
///
/// Note: The in-memory storage uses cursor-based pagination that encodes
/// the full tuple key. When using filters, the continuation token still
/// works but pagination may not be as efficient as with a full tuple key filter.
/// This test documents that filtering works correctly on the first page.
#[tokio::test]
async fn test_read_tuples_with_relation_filter() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    // Write tuples for two different relations
    for i in 0..20 {
        let tuple = StoredTuple {
            user_type: "user".to_string(),
            user_id: format!("user{i:04}"),
            user_relation: None,
            relation: "viewer".to_string(),
            object_type: "document".to_string(),
            object_id: format!("doc{i:04}"),
            condition_name: None,
            condition_context: None,
            created_at: Some(Utc::now()),
        };
        storage.write_tuple(&store_id, tuple).await.unwrap();
    }

    for i in 0..10 {
        let tuple = StoredTuple {
            user_type: "user".to_string(),
            user_id: format!("editor{i:04}"),
            user_relation: None,
            relation: "editor".to_string(),
            object_type: "document".to_string(),
            object_id: format!("doc{i:04}"),
            condition_name: None,
            condition_context: None,
            created_at: Some(Utc::now()),
        };
        storage.write_tuple(&store_id, tuple).await.unwrap();
    }

    // Read only viewer tuples (need all three tuple_key fields, empty string = no filter)
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/read"),
        serde_json::json!({
            "tuple_key": {
                "user": "",
                "relation": "viewer",
                "object": ""
            },
            "page_size": 50
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let tuples = response["tuples"].as_array().unwrap();

    // All tuples should be viewers
    for tuple in tuples {
        let relation = tuple["key"]["relation"].as_str().unwrap();
        assert_eq!(
            relation, "viewer",
            "Filtered read should only return viewers"
        );
    }

    assert_eq!(tuples.len(), 20, "Should return all 20 viewer tuples");
}

/// Test: Empty continuation token is treated as starting from beginning.
#[tokio::test]
async fn test_read_tuples_empty_continuation_token() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    write_test_tuples(&storage, &store_id, 10).await;

    // Read with empty string continuation token
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/read"),
        serde_json::json!({
            "page_size": 10,
            "continuation_token": ""
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let tuples = response["tuples"].as_array().unwrap();
    assert_eq!(
        tuples.len(),
        10,
        "Empty token should be treated as no token"
    );
}

/// Test: Nonexistent store returns 404.
#[tokio::test]
async fn test_read_tuples_nonexistent_store_returns_404() {
    let storage = Arc::new(MemoryDataStore::new());

    // Use a valid ULID format that doesn't exist (OpenFGA validates format first, returning 400 for invalid ULIDs)
    let nonexistent_store_id = ulid::Ulid::new().to_string();

    let (status, _response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{}/read", nonexistent_store_id),
        serde_json::json!({
            "page_size": 10
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "Nonexistent store should return 404"
    );
}

/// Test: Read changes with type filter.
#[tokio::test]
async fn test_read_changes_with_type_filter() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    // Write tuples for document type
    for i in 0..10 {
        let tuple = StoredTuple {
            user_type: "user".to_string(),
            user_id: format!("user{i:04}"),
            user_relation: None,
            relation: "viewer".to_string(),
            object_type: "document".to_string(),
            object_id: format!("doc{i:04}"),
            condition_name: None,
            condition_context: None,
            created_at: Some(Utc::now()),
        };
        storage.write_tuple(&store_id, tuple).await.unwrap();
    }

    // Write tuples for team type
    for i in 0..5 {
        let tuple = StoredTuple {
            user_type: "user".to_string(),
            user_id: format!("member{i:04}"),
            user_relation: None,
            relation: "member".to_string(),
            object_type: "team".to_string(),
            object_id: format!("team{i:04}"),
            condition_name: None,
            condition_context: None,
            created_at: Some(Utc::now()),
        };
        storage.write_tuple(&store_id, tuple).await.unwrap();
    }

    // Read changes filtered by document type
    let (status, response) = get_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/changes?type=document"),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let changes = response["changes"].as_array().unwrap();

    // All changes should be for document type
    for change in changes {
        let object = change["tuple_key"]["object"].as_str().unwrap();
        assert!(
            object.starts_with("document:"),
            "Filtered changes should only include document type, got: {}",
            object
        );
    }
}
