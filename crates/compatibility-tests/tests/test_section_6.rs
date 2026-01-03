use anyhow::Result;
use reqwest::StatusCode;
use serde_json::json;
use uuid::Uuid;

/// Helper function to get OpenFGA URL
fn get_openfga_url() -> String {
    std::env::var("OPENFGA_URL").unwrap_or_else(|_| "http://localhost:18080".to_string())
}

/// Helper function to create a test store
async fn create_test_store() -> Result<String> {
    let client = reqwest::Client::new();
    let store_name = format!("test-store-{}", Uuid::new_v4());

    let response = client
        .post(format!("{}/stores", get_openfga_url()))
        .json(&json!({ "name": store_name }))
        .send()
        .await?;

    let store: serde_json::Value = response.json().await?;
    Ok(store.get("id").and_then(|v| v.as_str()).unwrap().to_string())
}

/// Helper function to create a test authorization model
async fn create_test_model(store_id: &str) -> Result<String> {
    let client = reqwest::Client::new();

    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            {
                "type": "user"
            },
            {
                "type": "document",
                "relations": {
                    "viewer": {
                        "this": {}
                    }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [{"type": "user"}]
                        }
                    }
                }
            }
        ]
    });

    let response = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;
    Ok(response_body
        .get("authorization_model_id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string())
}

/// Test: GET /stores/{store_id}/authorization-models/{model_id} retrieves model
#[tokio::test]
async fn test_get_authorization_model_retrieves_model() -> Result<()> {
    // Arrange: Create a store and model
    let store_id = create_test_store().await?;
    let model_id = create_test_model(&store_id).await?;

    let client = reqwest::Client::new();

    // Act: Retrieve the model
    let response = client
        .get(format!(
            "{}/stores/{}/authorization-models/{}",
            get_openfga_url(),
            store_id,
            model_id
        ))
        .send()
        .await?;

    // Assert: Model retrieved successfully
    assert!(
        response.status().is_success(),
        "GET model should succeed, got: {}",
        response.status()
    );

    let model: serde_json::Value = response.json().await?;

    // Verify model has authorization_model field
    assert!(
        model.get("authorization_model").is_some(),
        "Response should contain 'authorization_model' field"
    );

    // Verify model ID matches
    let retrieved_model = model.get("authorization_model").unwrap();
    assert_eq!(
        retrieved_model.get("id").and_then(|v| v.as_str()),
        Some(model_id.as_str()),
        "Retrieved model should have correct ID"
    );

    Ok(())
}

/// Test: GET returns 404 for non-existent model
#[tokio::test]
async fn test_get_nonexistent_model_returns_404() -> Result<()> {
    // Arrange: Create a store
    let store_id = create_test_store().await?;
    let non_existent_id = Uuid::new_v4().to_string();

    let client = reqwest::Client::new();

    // Act: Try to retrieve non-existent model
    let response = client
        .get(format!(
            "{}/stores/{}/authorization-models/{}",
            get_openfga_url(),
            store_id,
            non_existent_id
        ))
        .send()
        .await?;

    // Assert: Returns 404 or 400 (OpenFGA may validate format first)
    assert!(
        response.status() == StatusCode::NOT_FOUND
            || response.status() == StatusCode::BAD_REQUEST,
        "GET non-existent model should return 404 or 400, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: GET /stores/{store_id}/authorization-models lists all models
#[tokio::test]
async fn test_list_authorization_models() -> Result<()> {
    // Arrange: Create a store and multiple models
    let store_id = create_test_store().await?;
    let model_id_1 = create_test_model(&store_id).await?;
    let model_id_2 = create_test_model(&store_id).await?;

    let client = reqwest::Client::new();

    // Act: List all models
    let response = client
        .get(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .send()
        .await?;

    // Assert: List succeeded
    assert!(
        response.status().is_success(),
        "LIST models should succeed, got: {}",
        response.status()
    );

    let list_body: serde_json::Value = response.json().await?;

    // Verify response has 'authorization_models' array
    assert!(
        list_body.get("authorization_models").is_some(),
        "Response should contain 'authorization_models' array"
    );

    let models = list_body
        .get("authorization_models")
        .and_then(|v| v.as_array())
        .unwrap();

    // Verify we have at least our 2 models
    assert!(
        models.len() >= 2,
        "Should have at least 2 models, got: {}",
        models.len()
    );

    // Verify our model IDs are in the list
    let model_ids: Vec<String> = models
        .iter()
        .filter_map(|m| m.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .collect();

    assert!(
        model_ids.contains(&model_id_1),
        "List should contain first model"
    );
    assert!(
        model_ids.contains(&model_id_2),
        "List should contain second model"
    );

    Ok(())
}

/// Test: Latest model can be retrieved
#[tokio::test]
async fn test_can_retrieve_latest_model() -> Result<()> {
    // Arrange: Create a store and multiple models
    let store_id = create_test_store().await?;
    let _model_id_1 = create_test_model(&store_id).await?;
    let model_id_2 = create_test_model(&store_id).await?;

    let client = reqwest::Client::new();

    // Act: List models
    let response = client
        .get(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .send()
        .await?;

    let list_body: serde_json::Value = response.json().await?;
    let models = list_body
        .get("authorization_models")
        .and_then(|v| v.as_array())
        .unwrap();

    // Assert: Latest model is the most recently created one
    // Models should be ordered with latest first or have a way to identify latest
    let first_model_id = models
        .first()
        .and_then(|m| m.get("id").and_then(|v| v.as_str()))
        .unwrap();

    // The latest model should be model_id_2 (most recently created)
    assert_eq!(
        first_model_id, model_id_2,
        "Latest model should be the most recently created"
    );

    Ok(())
}

/// Test: Model response includes schema_version
#[tokio::test]
async fn test_model_response_includes_schema_version() -> Result<()> {
    // Arrange: Create a store and model
    let store_id = create_test_store().await?;
    let model_id = create_test_model(&store_id).await?;

    let client = reqwest::Client::new();

    // Act: Retrieve the model
    let response = client
        .get(format!(
            "{}/stores/{}/authorization-models/{}",
            get_openfga_url(),
            store_id,
            model_id
        ))
        .send()
        .await?;

    let model: serde_json::Value = response.json().await?;
    let auth_model = model.get("authorization_model").unwrap();

    // Assert: Model includes schema_version
    assert!(
        auth_model.get("schema_version").is_some(),
        "Model should include 'schema_version' field"
    );

    let schema_version = auth_model.get("schema_version").and_then(|v| v.as_str()).unwrap();

    assert_eq!(
        schema_version, "1.1",
        "Schema version should be 1.1"
    );

    Ok(())
}

/// Test: Model response includes type_definitions
#[tokio::test]
async fn test_model_response_includes_type_definitions() -> Result<()> {
    // Arrange: Create a store and model
    let store_id = create_test_store().await?;
    let model_id = create_test_model(&store_id).await?;

    let client = reqwest::Client::new();

    // Act: Retrieve the model
    let response = client
        .get(format!(
            "{}/stores/{}/authorization-models/{}",
            get_openfga_url(),
            store_id,
            model_id
        ))
        .send()
        .await?;

    let model: serde_json::Value = response.json().await?;
    let auth_model = model.get("authorization_model").unwrap();

    // Assert: Model includes type_definitions
    assert!(
        auth_model.get("type_definitions").is_some(),
        "Model should include 'type_definitions' field"
    );

    let type_definitions = auth_model
        .get("type_definitions")
        .and_then(|v| v.as_array())
        .unwrap();

    // Verify we have the expected types (user and document)
    assert!(
        type_definitions.len() >= 2,
        "Should have at least 2 type definitions, got: {}",
        type_definitions.len()
    );

    // Verify types include user and document
    let type_names: Vec<String> = type_definitions
        .iter()
        .filter_map(|t| t.get("type").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .collect();

    assert!(
        type_names.contains(&"user".to_string()),
        "Should have 'user' type"
    );
    assert!(
        type_names.contains(&"document".to_string()),
        "Should have 'document' type"
    );

    Ok(())
}
