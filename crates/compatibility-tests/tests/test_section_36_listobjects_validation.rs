mod common;

use anyhow::Result;
use common::{create_test_model, create_test_store, get_openfga_url, grpc_call_with_error};
use serde_json::json;

/// Test: gRPC ListObjects rejects context that exceeds size limit
#[tokio::test]
async fn test_grpc_listobjects_rejects_large_context() -> Result<()> {
    // Arrange
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    // Create a large context (approx 70KB > 64KB limit)
    let large_value = "x".repeat(70 * 1024);
    let context = json!({
        "large_param": large_value
    });

    let request = json!({
        "store_id": store_id,
        "type": "document",
        "relation": "viewer",
        "user": "user:alice",
        "context": context
    });

    // Act
    let (success, stdout, stderr) =
        grpc_call_with_error("openfga.v1.OpenFGAService/ListObjects", &request)?;

    // Assert
    assert!(
        !success,
        "Request with huge context should fail, but succeeded with: {}",
        stdout
    );
    assert!(
        stderr.contains("context size exceeds maximum") || stderr.contains("InvalidArgument"),
        "Error message should mention context size or InvalidArgument, got: {}",
        stderr
    );

    Ok(())
}

/// Test: gRPC ListObjects rejects context that exceeds nesting depth
#[tokio::test]
async fn test_grpc_listobjects_rejects_deep_context() -> Result<()> {
    // Arrange
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    // Create a deeply nested context (depth 20 > limit usually 5-10)
    let mut context = json!({"leaf": "value"});
    for _ in 0..20 {
        context = json!({"wrapped": context});
    }

    let request = json!({
        "store_id": store_id,
        "type": "document",
        "relation": "viewer",
        "user": "user:alice",
        "context": context
    });

    // Act
    let (success, stdout, stderr) =
        grpc_call_with_error("openfga.v1.OpenFGAService/ListObjects", &request)?;

    // Assert
    assert!(
        !success,
        "Request with deep context should fail, but succeeded with: {}",
        stdout
    );
    assert!(
        stderr.contains("context nested too deeply") || stderr.contains("InvalidArgument"),
        "Error message should mention nesting depth or InvalidArgument, got: {}",
        stderr
    );

    Ok(())
}

/// Test: HTTP ListObjects rejects context that exceeds size limit
#[tokio::test]
async fn test_http_listobjects_rejects_large_context() -> Result<()> {
    // Arrange
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;
    let client = reqwest::Client::new();

    // Create a large context
    let large_value = "x".repeat(70 * 1024);
    let context = json!({
        "large_param": large_value
    });

    let request = json!({
        "type": "document",
        "relation": "viewer",
        "user": "user:alice",
        "context": context
    });

    // Act
    let response = client
        .post(format!(
            "{}/stores/{}/list-objects",
            get_openfga_url(),
            store_id
        ))
        .json(&request)
        .send()
        .await?;

    // Assert
    assert!(
        !response.status().is_success(),
        "HTTP request should fail with large context"
    );
    assert_eq!(
        response.status(),
        reqwest::StatusCode::BAD_REQUEST,
        "Should return 400 Bad Request"
    );

    let body = response.text().await?;
    assert!(
        body.contains("size exceeds maximum") || body.contains("Validation error"),
        "Error body should mention size limit, got: {}",
        body
    );

    Ok(())
}

// =============================================================================
// Positive Success Case Tests
// =============================================================================

/// Test: HTTP ListObjects returns objects user has access to
#[tokio::test]
async fn test_http_listobjects_returns_accessible_objects() -> Result<()> {
    use common::{shared_client, write_tuples};
    use std::collections::HashSet;

    // Arrange
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    // Write tuples granting access
    write_tuples(
        &store_id,
        vec![
            ("user:alice", "viewer", "document:doc1"),
            ("user:alice", "viewer", "document:doc2"),
            ("user:bob", "viewer", "document:doc3"),
        ],
    )
    .await?;

    let client = shared_client();

    // Act: List objects alice can view
    let request = json!({
        "type": "document",
        "relation": "viewer",
        "user": "user:alice"
    });

    let response = client
        .post(format!(
            "{}/stores/{}/list-objects",
            get_openfga_url(),
            store_id
        ))
        .json(&request)
        .send()
        .await?;

    // Assert
    assert!(
        response.status().is_success(),
        "ListObjects should succeed, got: {}",
        response.status()
    );

    let body: serde_json::Value = response.json().await?;
    let objects = body
        .get("objects")
        .and_then(|o| o.as_array())
        .expect("Response should have 'objects' array");

    let object_ids: HashSet<String> = objects
        .iter()
        .filter_map(|o| o.as_str())
        .map(String::from)
        .collect();

    let expected: HashSet<String> = ["document:doc1", "document:doc2"]
        .iter()
        .map(|s| s.to_string())
        .collect();

    assert_eq!(
        object_ids, expected,
        "Alice should have access to exactly doc1 and doc2"
    );

    Ok(())
}

/// Test: HTTP ListObjects returns empty array when no access
#[tokio::test]
async fn test_http_listobjects_returns_empty_when_no_access() -> Result<()> {
    use common::{shared_client, write_tuples};

    // Arrange
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    // Write tuples for OTHER users, not for target user
    write_tuples(
        &store_id,
        vec![
            ("user:alice", "viewer", "document:doc1"),
            ("user:bob", "viewer", "document:doc2"),
        ],
    )
    .await?;

    let client = shared_client();

    // Act: List objects charlie can view (should be none)
    let request = json!({
        "type": "document",
        "relation": "viewer",
        "user": "user:charlie"
    });

    let response = client
        .post(format!(
            "{}/stores/{}/list-objects",
            get_openfga_url(),
            store_id
        ))
        .json(&request)
        .send()
        .await?;

    // Assert
    assert!(response.status().is_success());

    let body: serde_json::Value = response.json().await?;
    let objects = body
        .get("objects")
        .and_then(|o| o.as_array())
        .expect("Response should have 'objects' array");

    assert!(
        objects.is_empty(),
        "Charlie should have no accessible objects, got: {:?}",
        objects
    );

    Ok(())
}

/// Test: HTTP ListObjects with valid context succeeds
#[tokio::test]
async fn test_http_listobjects_with_valid_context_succeeds() -> Result<()> {
    use common::shared_client;

    // Arrange
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    let client = shared_client();

    // Act: List objects with a valid (small) context
    let request = json!({
        "type": "document",
        "relation": "viewer",
        "user": "user:alice",
        "context": {
            "ip_address": "192.168.1.1",
            "request_time": "2024-01-15T10:00:00Z"
        }
    });

    let response = client
        .post(format!(
            "{}/stores/{}/list-objects",
            get_openfga_url(),
            store_id
        ))
        .json(&request)
        .send()
        .await?;

    // Assert: Request should succeed (context is valid)
    assert!(
        response.status().is_success(),
        "ListObjects with valid context should succeed, got: {}",
        response.status()
    );

    let body: serde_json::Value = response.json().await?;
    assert!(
        body.get("objects").is_some(),
        "Response should have 'objects' field"
    );

    Ok(())
}
