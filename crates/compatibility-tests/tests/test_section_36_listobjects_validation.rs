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
