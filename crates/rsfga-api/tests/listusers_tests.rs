//! ListUsers API integration tests for issue #193.
//!
//! These tests cover ListUsers API edge cases:
//! - Truncation behavior with >1000 candidates
//! - Performance with 1000 candidates
//! - Concurrent ListUsers requests (thread safety)
//! - Timeout handling
//! - Domain/storage validation alignment
//! - Error scenario coverage
//! - User filter behavior
//!
//! Run with: cargo test --test listusers_tests
//!
//! ## Storage Backend Note
//!
//! These tests currently use `MemoryDataStore` directly, which is consistent
//! with the existing `listobjects_tests.rs` pattern. To run against real
//! database backends (PostgreSQL, MySQL, CockroachDB), a future enhancement
//! could add a factory function that selects the backend via environment
//! variables. This would allow the same test suite to validate behavior
//! across all supported storage implementations.

mod common;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use rsfga_storage::{DataStore, MemoryDataStore, StoredTuple, Utc};

use common::{create_test_app, post_json, setup_simple_model};

// =============================================================================
// Constants
// =============================================================================

/// Maximum number of users returned by ListUsers (OpenFGA default).
/// OpenFGA truncates results beyond this limit.
const MAX_LIST_USERS_RESULTS: usize = 1000;

/// Number of candidates for large-scale tests.
const LARGE_CANDIDATE_COUNT: usize = 1500;

/// Number of concurrent requests for thread safety tests.
const CONCURRENT_LISTUSERS_REQUESTS: usize = 50;

/// Timeout for ListUsers operations (matching OpenFGA default).
const LISTUSERS_TIMEOUT: Duration = Duration::from_secs(3);

// =============================================================================
// Section 1: Basic ListUsers Tests
// =============================================================================

/// Test: ListUsers returns direct users with view permission.
#[tokio::test]
async fn test_listusers_returns_direct_users() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    // Write tuples - alice and bob can view doc1
    for user in ["alice", "bob", "charlie"] {
        let tuple = StoredTuple {
            user_type: "user".to_string(),
            user_id: user.to_string(),
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

    // Act: List users who can view doc1
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-users"),
        serde_json::json!({
            "object": { "type": "document", "id": "doc1" },
            "relation": "viewer",
            "user_filters": [{ "type": "user" }]
        }),
    )
    .await;

    // Assert: Should succeed and return all three users
    assert_eq!(status, StatusCode::OK);

    let users = response["users"]
        .as_array()
        .expect("Response should have 'users' array");

    assert_eq!(users.len(), 3, "Should return all 3 users");

    // Verify users are returned
    let user_ids: HashSet<String> = users
        .iter()
        .filter_map(|u| u["object"]["id"].as_str().map(String::from))
        .collect();

    assert!(user_ids.contains("alice"), "Should contain alice");
    assert!(user_ids.contains("bob"), "Should contain bob");
    assert!(user_ids.contains("charlie"), "Should contain charlie");
}

/// Test: ListUsers returns empty when no users have permission.
#[tokio::test]
async fn test_listusers_returns_empty_when_no_users() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    // No tuples written - no one has access

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-users"),
        serde_json::json!({
            "object": { "type": "document", "id": "doc1" },
            "relation": "viewer",
            "user_filters": [{ "type": "user" }]
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let users = response["users"]
        .as_array()
        .expect("Response should have 'users' array");

    assert_eq!(users.len(), 0, "Should return empty array when no users");
}

/// Test: ListUsers with user filter restricts results.
#[tokio::test]
async fn test_listusers_with_user_filter_restricts_type() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    // Write user tuple
    let user_tuple = StoredTuple {
        user_type: "user".to_string(),
        user_id: "alice".to_string(),
        user_relation: None,
        relation: "viewer".to_string(),
        object_type: "document".to_string(),
        object_id: "doc1".to_string(),
        condition_name: None,
        condition_context: None,
        created_at: Some(Utc::now()),
    };
    storage.write_tuple(&store_id, user_tuple).await.unwrap();

    // Write team member tuple (team#member relation)
    let team_tuple = StoredTuple {
        user_type: "team".to_string(),
        user_id: "engineering".to_string(),
        user_relation: Some("member".to_string()),
        relation: "viewer".to_string(),
        object_type: "document".to_string(),
        object_id: "doc1".to_string(),
        condition_name: None,
        condition_context: None,
        created_at: Some(Utc::now()),
    };
    storage.write_tuple(&store_id, team_tuple).await.unwrap();

    // Act: List only users (not teams)
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-users"),
        serde_json::json!({
            "object": { "type": "document", "id": "doc1" },
            "relation": "viewer",
            "user_filters": [{ "type": "user" }]  // Only users, not teams
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let users = response["users"]
        .as_array()
        .expect("Response should have 'users' array");

    // Should only return alice, not the team
    assert_eq!(users.len(), 1, "Should return only 1 user (not team)");

    let user_id = users[0]["object"]["id"].as_str();
    assert_eq!(user_id, Some("alice"), "Should return alice");
}

// =============================================================================
// Section 2: Truncation Behavior Tests
// =============================================================================

/// Test: ListUsers truncates results when exceeding 1000 candidates.
///
/// OpenFGA returns at most 1000 users from ListUsers, even if more
/// users match. This test verifies truncation behavior.
#[tokio::test]
async fn test_listusers_truncates_results_over_1000_candidates() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    // Write 1500 tuples - many users can view doc1
    for i in 0..LARGE_CANDIDATE_COUNT {
        let tuple = StoredTuple {
            user_type: "user".to_string(),
            user_id: format!("user{i}"),
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

    // Act: List users who can view doc1
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-users"),
        serde_json::json!({
            "object": { "type": "document", "id": "doc1" },
            "relation": "viewer",
            "user_filters": [{ "type": "user" }]
        }),
    )
    .await;

    // Assert: Should succeed and return at most 1000 users
    assert_eq!(status, StatusCode::OK);

    let users = response["users"]
        .as_array()
        .expect("Response should have 'users' array");

    assert!(
        users.len() <= MAX_LIST_USERS_RESULTS,
        "ListUsers should truncate to {} results, got {}",
        MAX_LIST_USERS_RESULTS,
        users.len()
    );

    // Verify we got exactly 1000 (the max)
    assert_eq!(
        users.len(),
        MAX_LIST_USERS_RESULTS,
        "Should return exactly {} users when more exist",
        MAX_LIST_USERS_RESULTS
    );
}

/// Test: ListUsers returns all users when count is under limit.
#[tokio::test]
async fn test_listusers_returns_all_when_under_limit() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    // Write 500 tuples (under the 1000 limit)
    for i in 0..500 {
        let tuple = StoredTuple {
            user_type: "user".to_string(),
            user_id: format!("user{i}"),
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
            "object": { "type": "document", "id": "doc1" },
            "relation": "viewer",
            "user_filters": [{ "type": "user" }]
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let users = response["users"]
        .as_array()
        .expect("Response should have 'users' array");

    assert_eq!(
        users.len(),
        500,
        "Should return all 500 users when under limit"
    );
}

// =============================================================================
// Section 3: Performance Tests
// =============================================================================

/// Test: ListUsers performance with 1000 candidates.
///
/// Verifies ListUsers completes within acceptable time bounds
/// when processing the maximum number of candidates.
#[tokio::test]
async fn test_listusers_performance_with_1000_candidates() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    // Write exactly 1000 tuples
    for i in 0..1000 {
        let tuple = StoredTuple {
            user_type: "user".to_string(),
            user_id: format!("perf_user{i}"),
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

    // Measure performance
    let start = Instant::now();

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-users"),
        serde_json::json!({
            "object": { "type": "document", "id": "doc1" },
            "relation": "viewer",
            "user_filters": [{ "type": "user" }]
        }),
    )
    .await;

    let elapsed = start.elapsed();

    // Assert: Should complete successfully
    assert_eq!(status, StatusCode::OK);

    let users = response["users"]
        .as_array()
        .expect("Response should have 'users' array");

    assert_eq!(users.len(), 1000, "Should return all 1000 users");

    // Performance assertion: should complete within timeout
    assert!(
        elapsed < LISTUSERS_TIMEOUT,
        "ListUsers with 1000 candidates should complete within {:?}, took {:?}",
        LISTUSERS_TIMEOUT,
        elapsed
    );

    // Informational: log actual performance
    println!(
        "ListUsers with 1000 candidates completed in {:?} ({:.2} users/ms)",
        elapsed,
        1000.0 / elapsed.as_millis() as f64
    );
}

// =============================================================================
// Section 4: Concurrent Request Tests (Thread Safety)
// =============================================================================

/// Test: Concurrent ListUsers requests don't interfere with each other.
///
/// Verifies thread safety by running multiple concurrent ListUsers
/// requests for different objects and checking results are isolated.
#[tokio::test]
async fn test_listusers_concurrent_requests_are_isolated() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    // Create different documents with different user access patterns
    for doc_idx in 0..CONCURRENT_LISTUSERS_REQUESTS {
        // Each document has exactly `doc_idx + 1` users with access
        for user_idx in 0..=doc_idx {
            let tuple = StoredTuple {
                user_type: "user".to_string(),
                user_id: format!("user{user_idx}"),
                user_relation: None,
                relation: "viewer".to_string(),
                object_type: "document".to_string(),
                object_id: format!("doc{doc_idx}"),
                condition_name: None,
                condition_context: None,
                created_at: Some(Utc::now()),
            };
            storage.write_tuple(&store_id, tuple).await.unwrap();
        }
    }

    // Run concurrent requests
    let mut handles = Vec::new();
    for doc_idx in 0..CONCURRENT_LISTUSERS_REQUESTS {
        let storage_clone = Arc::clone(&storage);
        let store_id_clone = store_id.clone();
        let expected_count = doc_idx + 1;

        handles.push(tokio::spawn(async move {
            let (status, response) = post_json(
                create_test_app(&storage_clone),
                &format!("/stores/{store_id_clone}/list-users"),
                serde_json::json!({
                    "object": { "type": "document", "id": format!("doc{doc_idx}") },
                    "relation": "viewer",
                    "user_filters": [{ "type": "user" }]
                }),
            )
            .await;

            (doc_idx, expected_count, status, response)
        }));
    }

    // Verify all results
    for handle in handles {
        let (doc_idx, expected_count, status, response) = handle.await.unwrap();

        assert_eq!(
            status,
            StatusCode::OK,
            "Request for doc{} should succeed",
            doc_idx
        );

        let users = response["users"]
            .as_array()
            .expect("Response should have 'users' array");

        assert_eq!(
            users.len(),
            expected_count,
            "doc{} should have exactly {} users with access, got {}",
            doc_idx,
            expected_count,
            users.len()
        );
    }
}

/// Test: Concurrent ListUsers requests for same object are consistent.
#[tokio::test]
async fn test_listusers_concurrent_same_object_consistent() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    // Write tuples for a single document
    for i in 0..100 {
        let tuple = StoredTuple {
            user_type: "user".to_string(),
            user_id: format!("concurrent_user{i}"),
            user_relation: None,
            relation: "viewer".to_string(),
            object_type: "document".to_string(),
            object_id: "concurrent_doc".to_string(),
            condition_name: None,
            condition_context: None,
            created_at: Some(Utc::now()),
        };
        storage.write_tuple(&store_id, tuple).await.unwrap();
    }

    // Run multiple concurrent requests for the same object
    let mut handles = Vec::new();
    for _ in 0..20 {
        let storage_clone = Arc::clone(&storage);
        let store_id_clone = store_id.clone();

        handles.push(tokio::spawn(async move {
            let (status, response) = post_json(
                create_test_app(&storage_clone),
                &format!("/stores/{store_id_clone}/list-users"),
                serde_json::json!({
                    "object": { "type": "document", "id": "concurrent_doc" },
                    "relation": "viewer",
                    "user_filters": [{ "type": "user" }]
                }),
            )
            .await;

            (status, response)
        }));
    }

    // Verify all results are identical
    let mut all_results: Vec<HashSet<String>> = Vec::new();

    for handle in handles {
        let (status, response) = handle.await.unwrap();
        assert_eq!(status, StatusCode::OK);

        let users: HashSet<String> = response["users"]
            .as_array()
            .expect("Response should have 'users' array")
            .iter()
            .filter_map(|u| u["object"]["id"].as_str().map(String::from))
            .collect();

        all_results.push(users);
    }

    // All results should be identical
    let first_result = &all_results[0];
    for (idx, result) in all_results.iter().enumerate() {
        assert_eq!(
            result, first_result,
            "Concurrent request {} returned different results",
            idx
        );
    }
}

// =============================================================================
// Section 5: Timeout Handling Tests
// =============================================================================

/// Test: ListUsers respects timeout for expensive queries.
///
/// Note: This test verifies the request completes in a reasonable time
/// and doesn't hang indefinitely.
#[tokio::test]
async fn test_listusers_respects_timeout() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    // Write many tuples to simulate an expensive query
    for i in 0..LARGE_CANDIDATE_COUNT {
        let tuple = StoredTuple {
            user_type: "user".to_string(),
            user_id: format!("timeout_user{i}"),
            user_relation: None,
            relation: "viewer".to_string(),
            object_type: "document".to_string(),
            object_id: "timeout_doc".to_string(),
            condition_name: None,
            condition_context: None,
            created_at: Some(Utc::now()),
        };
        storage.write_tuple(&store_id, tuple).await.unwrap();
    }

    let start = Instant::now();

    let (status, _response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-users"),
        serde_json::json!({
            "object": { "type": "document", "id": "timeout_doc" },
            "relation": "viewer",
            "user_filters": [{ "type": "user" }]
        }),
    )
    .await;

    let elapsed = start.elapsed();

    // The request should complete around the timeout duration, not hang indefinitely.
    // We add a grace period to account for test overhead.
    assert!(
        elapsed < LISTUSERS_TIMEOUT.saturating_add(Duration::from_secs(2)),
        "ListUsers should respect timeout. Took {:?}, expected to be less than ~{:?}",
        elapsed,
        LISTUSERS_TIMEOUT
    );

    // Either success (200) or timeout (504) is acceptable
    assert!(
        status == StatusCode::OK || status == StatusCode::GATEWAY_TIMEOUT,
        "Expected 200 or 504, got {}",
        status
    );
}

// =============================================================================
// Section 6: Domain/Storage Validation Alignment Tests
// =============================================================================

/// Test: ListUsers validates store exists.
#[tokio::test]
async fn test_listusers_returns_error_for_nonexistent_store() {
    let storage = Arc::new(MemoryDataStore::new());

    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores/nonexistent-store/list-users",
        serde_json::json!({
            "object": { "type": "document", "id": "doc1" },
            "relation": "viewer",
            "user_filters": [{ "type": "user" }]
        }),
    )
    .await;

    // Should return 404 for nonexistent store
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "Nonexistent store should return 404, got {}: {:?}",
        status,
        response
    );
}

/// Test: ListUsers validates type exists in model.
#[tokio::test]
async fn test_listusers_returns_error_for_nonexistent_type() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-users"),
        serde_json::json!({
            "object": { "type": "nonexistent_type", "id": "obj1" },
            "relation": "viewer",
            "user_filters": [{ "type": "user" }]
        }),
    )
    .await;

    // Should return 400 for invalid type
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Nonexistent type should return 400, got {}: {:?}",
        status,
        response
    );
}

/// Test: ListUsers validates relation exists on type.
#[tokio::test]
async fn test_listusers_returns_error_for_nonexistent_relation() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-users"),
        serde_json::json!({
            "object": { "type": "document", "id": "doc1" },
            "relation": "nonexistent_relation",
            "user_filters": [{ "type": "user" }]
        }),
    )
    .await;

    // Should return 400 for invalid relation
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Nonexistent relation should return 400, got {}: {:?}",
        status,
        response
    );
}

/// Test: ListUsers with user_filter type not in model returns empty results.
///
/// When filtering by a type that doesn't exist in the model, the API
/// returns an empty users array rather than an error. This is consistent
/// with OpenFGA behavior.
#[tokio::test]
async fn test_listusers_returns_empty_for_user_filter_type_not_in_model() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    // Add a user with access so we can verify the filter works
    let tuple = StoredTuple {
        user_type: "user".to_string(),
        user_id: "alice".to_string(),
        user_relation: None,
        relation: "viewer".to_string(),
        object_type: "document".to_string(),
        object_id: "doc1".to_string(),
        condition_name: None,
        condition_context: None,
        created_at: Some(Utc::now()),
    };
    storage.write_tuple(&store_id, tuple).await.unwrap();

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-users"),
        serde_json::json!({
            "object": { "type": "document", "id": "doc1" },
            "relation": "viewer",
            "user_filters": [{ "type": "invalid_type_not_in_model" }]
        }),
    )
    .await;

    // Returns 200 OK with empty results (filters out everything)
    assert_eq!(
        status,
        StatusCode::OK,
        "User filter type not in model should return 200, got {}: {:?}",
        status,
        response
    );

    let users = response["users"]
        .as_array()
        .expect("Response should have 'users' array");

    assert_eq!(
        users.len(),
        0,
        "Should return empty array when filter type doesn't match"
    );
}

// =============================================================================
// Section 7: Error Scenario Coverage Tests
// =============================================================================

/// Test: ListUsers with missing object returns 400.
#[tokio::test]
async fn test_listusers_missing_object_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-users"),
        serde_json::json!({
            "relation": "viewer",
            "user_filters": [{ "type": "user" }]
            // Missing "object" field
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Missing 'object' field should return 400, got {}: {:?}",
        status,
        response
    );
}

/// Test: ListUsers with missing relation returns 400.
#[tokio::test]
async fn test_listusers_missing_relation_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-users"),
        serde_json::json!({
            "object": { "type": "document", "id": "doc1" },
            "user_filters": [{ "type": "user" }]
            // Missing "relation" field
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Missing 'relation' field should return 400, got {}: {:?}",
        status,
        response
    );
}

/// Test: ListUsers with missing user_filters returns 400.
#[tokio::test]
async fn test_listusers_missing_user_filters_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-users"),
        serde_json::json!({
            "object": { "type": "document", "id": "doc1" },
            "relation": "viewer"
            // Missing "user_filters" field
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Missing 'user_filters' field should return 400, got {}: {:?}",
        status,
        response
    );
}

/// Test: ListUsers with empty request body returns 400.
#[tokio::test]
async fn test_listusers_empty_body_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-users"),
        serde_json::json!({}),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Empty body should return 400, got {}: {:?}",
        status,
        response
    );
}

/// Test: ListUsers with empty user_filters array returns 400.
#[tokio::test]
async fn test_listusers_empty_user_filters_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-users"),
        serde_json::json!({
            "object": { "type": "document", "id": "doc1" },
            "relation": "viewer",
            "user_filters": []  // Empty array
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Empty user_filters should return 400, got {}: {:?}",
        status,
        response
    );
}

/// Test: ListUsers with empty object id returns 400.
#[tokio::test]
async fn test_listusers_empty_object_id_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-users"),
        serde_json::json!({
            "object": { "type": "document", "id": "" },
            "relation": "viewer",
            "user_filters": [{ "type": "user" }]
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Empty object id should return 400, got {}: {:?}",
        status,
        response
    );
}

/// Test: ListUsers with empty relation returns 400.
#[tokio::test]
async fn test_listusers_empty_relation_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-users"),
        serde_json::json!({
            "object": { "type": "document", "id": "doc1" },
            "relation": "",
            "user_filters": [{ "type": "user" }]
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Empty relation should return 400, got {}: {:?}",
        status,
        response
    );
}

// =============================================================================
// Section 8: User Filter with Relation Tests
// =============================================================================

/// Test: ListUsers with user_filter relation filters correctly.
#[tokio::test]
async fn test_listusers_user_filter_with_relation() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    // Write a team#member tuple that has viewer access
    let team_tuple = StoredTuple {
        user_type: "team".to_string(),
        user_id: "engineering".to_string(),
        user_relation: Some("member".to_string()),
        relation: "viewer".to_string(),
        object_type: "document".to_string(),
        object_id: "doc1".to_string(),
        condition_name: None,
        condition_context: None,
        created_at: Some(Utc::now()),
    };
    storage.write_tuple(&store_id, team_tuple).await.unwrap();

    // Act: List users with team#member filter
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-users"),
        serde_json::json!({
            "object": { "type": "document", "id": "doc1" },
            "relation": "viewer",
            "user_filters": [{ "type": "team", "relation": "member" }]
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let users = response["users"]
        .as_array()
        .expect("Response should have 'users' array");

    // Should return the team#member
    assert_eq!(users.len(), 1, "Should return 1 team#member");

    // Userset results have "userset" wrapper instead of "object"
    let user = &users[0];
    assert_eq!(user["userset"]["type"].as_str(), Some("team"));
    assert_eq!(user["userset"]["id"].as_str(), Some("engineering"));
    assert_eq!(user["userset"]["relation"].as_str(), Some("member"));
}

/// Test: ListUsers with invalid user_filter relation format returns 400.
#[tokio::test]
async fn test_listusers_invalid_user_filter_relation_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_model(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-users"),
        serde_json::json!({
            "object": { "type": "document", "id": "doc1" },
            "relation": "viewer",
            "user_filters": [{ "type": "team", "relation": "" }]  // Empty relation
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Empty user_filter relation should return 400, got {}: {:?}",
        status,
        response
    );
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Create a store with authorization model and return the store ID.
async fn create_store_with_model(storage: &Arc<MemoryDataStore>) -> String {
    // Create store
    let (status, response) = post_json(
        create_test_app(storage),
        "/stores",
        serde_json::json!({
            "name": "listusers-test-store"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();

    // Setup authorization model
    setup_simple_model(storage, &store_id).await;

    store_id
}
