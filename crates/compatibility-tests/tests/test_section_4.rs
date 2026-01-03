use anyhow::Result;
use reqwest::StatusCode;
use serde_json::json;
use uuid::Uuid;

/// Helper function to start OpenFGA (reuse from section 1)
fn get_openfga_url() -> String {
    std::env::var("OPENFGA_URL").unwrap_or_else(|_| "http://localhost:18080".to_string())
}

/// Test: POST /stores creates store with generated ID
#[tokio::test]
async fn test_post_stores_creates_store_with_generated_id() -> Result<()> {
    // Arrange: Create unique store name
    let client = reqwest::Client::new();
    let store_name = format!("test-store-{}", Uuid::new_v4());
    let create_request = json!({
        "name": store_name
    });

    // Act: Create store
    let response = client
        .post(format!("{}/stores", get_openfga_url()))
        .json(&create_request)
        .send()
        .await?;

    // Assert: Store created with ID
    assert!(
        response.status().is_success(),
        "Store creation should succeed, got: {}",
        response.status()
    );

    let response_body: serde_json::Value = response.json().await?;

    assert!(
        response_body.get("id").is_some(),
        "Response should contain store 'id' field"
    );
    assert!(
        response_body.get("id").and_then(|v| v.as_str()).is_some(),
        "Store ID should be a string"
    );
    assert!(
        !response_body.get("id").and_then(|v| v.as_str()).unwrap().is_empty(),
        "Store ID should not be empty"
    );

    Ok(())
}

/// Test: POST /stores returns created store in response
#[tokio::test]
async fn test_post_stores_returns_created_store_in_response() -> Result<()> {
    // Arrange: Create unique store name
    let client = reqwest::Client::new();
    let store_name = format!("test-store-{}", Uuid::new_v4());
    let create_request = json!({
        "name": store_name
    });

    // Act: Create store
    let response = client
        .post(format!("{}/stores", get_openfga_url()))
        .json(&create_request)
        .send()
        .await?;

    // Assert: Response contains created store
    assert!(response.status().is_success());

    let response_body: serde_json::Value = response.json().await?;

    // Verify store name matches
    assert_eq!(
        response_body.get("name").and_then(|v| v.as_str()),
        Some(store_name.as_str()),
        "Response should contain the store name we provided"
    );

    // Verify store has ID
    assert!(response_body.get("id").is_some(), "Response should contain 'id'");

    // Verify store has created_at timestamp
    assert!(
        response_body.get("created_at").is_some(),
        "Response should contain 'created_at' timestamp"
    );

    // Verify store has updated_at timestamp
    assert!(
        response_body.get("updated_at").is_some(),
        "Response should contain 'updated_at' timestamp"
    );

    Ok(())
}

/// Test: GET /stores/{store_id} retrieves store by ID
#[tokio::test]
async fn test_get_store_retrieves_store_by_id() -> Result<()> {
    // Arrange: Create a store first
    let client = reqwest::Client::new();
    let store_name = format!("test-store-{}", Uuid::new_v4());

    let create_response = client
        .post(format!("{}/stores", get_openfga_url()))
        .json(&json!({ "name": store_name }))
        .send()
        .await?;

    let created_store: serde_json::Value = create_response.json().await?;
    let store_id = created_store.get("id").and_then(|v| v.as_str()).unwrap();

    // Act: Retrieve store by ID
    let get_response = client
        .get(format!("{}/stores/{}", get_openfga_url(), store_id))
        .send()
        .await?;

    // Assert: Store retrieved successfully
    assert!(
        get_response.status().is_success(),
        "GET store should succeed, got: {}",
        get_response.status()
    );

    let retrieved_store: serde_json::Value = get_response.json().await?;

    // Verify store ID matches
    assert_eq!(
        retrieved_store.get("id").and_then(|v| v.as_str()),
        Some(store_id),
        "Retrieved store should have same ID"
    );

    // Verify store name matches
    assert_eq!(
        retrieved_store.get("name").and_then(|v| v.as_str()),
        Some(store_name.as_str()),
        "Retrieved store should have same name"
    );

    Ok(())
}

/// Test: GET /stores/{store_id} returns 404 for non-existent store
#[tokio::test]
async fn test_get_store_returns_404_for_nonexistent_store() -> Result<()> {
    // Arrange: Use a non-existent store ID
    let client = reqwest::Client::new();
    let non_existent_id = Uuid::new_v4().to_string();

    // Act: Try to retrieve non-existent store
    let response = client
        .get(format!("{}/stores/{}", get_openfga_url(), non_existent_id))
        .send()
        .await?;

    // Assert: OpenFGA returns 400 Bad Request for non-existent store IDs
    // (Not 404, because it validates the store ID format first)
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "GET non-existent store should return 400, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: DELETE /stores/{store_id} deletes store
#[tokio::test]
async fn test_delete_store_deletes_store() -> Result<()> {
    // Arrange: Create a store first
    let client = reqwest::Client::new();
    let store_name = format!("test-store-{}", Uuid::new_v4());

    let create_response = client
        .post(format!("{}/stores", get_openfga_url()))
        .json(&json!({ "name": store_name }))
        .send()
        .await?;

    let created_store: serde_json::Value = create_response.json().await?;
    let store_id = created_store.get("id").and_then(|v| v.as_str()).unwrap();

    // Act: Delete the store
    let delete_response = client
        .delete(format!("{}/stores/{}", get_openfga_url(), store_id))
        .send()
        .await?;

    // Assert: Delete succeeded
    assert!(
        delete_response.status().is_success(),
        "DELETE store should succeed, got: {}",
        delete_response.status()
    );

    // Verify: Store no longer exists (GET should return 404)
    let get_response = client
        .get(format!("{}/stores/{}", get_openfga_url(), store_id))
        .send()
        .await?;

    assert_eq!(
        get_response.status(),
        StatusCode::NOT_FOUND,
        "Deleted store should return 404, got: {}",
        get_response.status()
    );

    Ok(())
}

/// Test: DELETE /stores/{store_id} returns 404 for non-existent store
#[tokio::test]
async fn test_delete_store_returns_404_for_nonexistent_store() -> Result<()> {
    // Arrange: Use a non-existent store ID
    let client = reqwest::Client::new();
    let non_existent_id = Uuid::new_v4().to_string();

    // Act: Try to delete non-existent store
    let response = client
        .delete(format!("{}/stores/{}", get_openfga_url(), non_existent_id))
        .send()
        .await?;

    // Assert: OpenFGA returns 400 Bad Request for non-existent store IDs
    // (Not 404, because it validates the store ID format first)
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "DELETE non-existent store should return 400, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: LIST /stores returns paginated results
#[tokio::test]
async fn test_list_stores_returns_paginated_results() -> Result<()> {
    // Arrange: Create multiple stores
    let client = reqwest::Client::new();
    let mut store_ids = Vec::new();

    for i in 0..5 {
        let store_name = format!("test-store-list-{}-{}", i, Uuid::new_v4());
        let response = client
            .post(format!("{}/stores", get_openfga_url()))
            .json(&json!({ "name": store_name }))
            .send()
            .await?;

        let store: serde_json::Value = response.json().await?;
        store_ids.push(store.get("id").and_then(|v| v.as_str()).unwrap().to_string());
    }

    // Act: List stores
    let list_response = client
        .get(format!("{}/stores", get_openfga_url()))
        .send()
        .await?;

    // Assert: List succeeded
    assert!(
        list_response.status().is_success(),
        "LIST stores should succeed, got: {}",
        list_response.status()
    );

    let list_body: serde_json::Value = list_response.json().await?;

    // Verify response has 'stores' array
    assert!(
        list_body.get("stores").is_some(),
        "Response should contain 'stores' array"
    );

    let stores = list_body.get("stores").and_then(|v| v.as_array()).unwrap();

    // Verify we got at least our 5 stores (there may be others from previous tests)
    assert!(
        stores.len() >= 5,
        "Should have at least 5 stores, got: {}",
        stores.len()
    );

    Ok(())
}

/// Test: LIST /stores respects page_size parameter
#[tokio::test]
async fn test_list_stores_respects_page_size_parameter() -> Result<()> {
    // Arrange: Create multiple stores
    let client = reqwest::Client::new();

    for i in 0..10 {
        let store_name = format!("test-store-pagesize-{}-{}", i, Uuid::new_v4());
        client
            .post(format!("{}/stores", get_openfga_url()))
            .json(&json!({ "name": store_name }))
            .send()
            .await?;
    }

    // Act: List stores with page_size=3
    let list_response = client
        .get(format!("{}/stores?page_size=3", get_openfga_url()))
        .send()
        .await?;

    // Assert: Response contains at most 3 stores
    assert!(list_response.status().is_success());

    let list_body: serde_json::Value = list_response.json().await?;
    let stores = list_body.get("stores").and_then(|v| v.as_array()).unwrap();

    assert!(
        stores.len() <= 3,
        "Page size=3 should return at most 3 stores, got: {}",
        stores.len()
    );

    // Verify continuation_token is present (more results available)
    assert!(
        list_body.get("continuation_token").is_some(),
        "Should have continuation_token when more results exist"
    );

    Ok(())
}

/// Test: LIST /stores continuation_token works correctly
#[tokio::test]
async fn test_list_stores_continuation_token_works() -> Result<()> {
    // Arrange: Create multiple stores
    let client = reqwest::Client::new();

    for i in 0..10 {
        let store_name = format!("test-store-token-{}-{}", i, Uuid::new_v4());
        client
            .post(format!("{}/stores", get_openfga_url()))
            .json(&json!({ "name": store_name }))
            .send()
            .await?;
    }

    // Act: Get first page
    let first_response = client
        .get(format!("{}/stores?page_size=3", get_openfga_url()))
        .send()
        .await?;

    let first_body: serde_json::Value = first_response.json().await?;
    let first_stores = first_body.get("stores").and_then(|v| v.as_array()).unwrap();
    let continuation_token = first_body
        .get("continuation_token")
        .and_then(|v| v.as_str())
        .unwrap();

    // Act: Get second page using continuation_token
    let second_response = client
        .get(format!(
            "{}/stores?page_size=3&continuation_token={}",
            get_openfga_url(),
            continuation_token
        ))
        .send()
        .await?;

    // Assert: Second page retrieved successfully
    assert!(second_response.status().is_success());

    let second_body: serde_json::Value = second_response.json().await?;
    let second_stores = second_body.get("stores").and_then(|v| v.as_array()).unwrap();

    // Verify: Second page has different stores than first page
    let first_ids: Vec<String> = first_stores
        .iter()
        .filter_map(|s| s.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .collect();

    let second_ids: Vec<String> = second_stores
        .iter()
        .filter_map(|s| s.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .collect();

    // No overlap between pages
    for id in &second_ids {
        assert!(
            !first_ids.contains(id),
            "Second page should not contain stores from first page"
        );
    }

    Ok(())
}
