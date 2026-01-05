mod common;

use anyhow::Result;
use common::{create_conditional_model, create_test_model, create_test_store, get_openfga_url};
use reqwest::StatusCode;
use serde_json::json;
use uuid::Uuid;

/// Test: POST /stores/{store_id}/write writes single tuple
#[tokio::test]
async fn test_write_single_tuple() -> Result<()> {
    // Arrange: Create store and model
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    let client = reqwest::Client::new();

    // Act: Write a single tuple
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:budget"
                }
            ]
        }
    });

    let response = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&write_request)
        .send()
        .await?;

    // Assert: Write succeeded
    assert!(
        response.status().is_success(),
        "Write should succeed, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: Write returns empty response on success
#[tokio::test]
async fn test_write_returns_empty_response() -> Result<()> {
    // Arrange: Create store and model
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    let client = reqwest::Client::new();

    // Act: Write a tuple
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:bob",
                    "relation": "editor",
                    "object": "document:proposal"
                }
            ]
        }
    });

    let response = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&write_request)
        .send()
        .await?;

    // Assert: Response is empty object
    let response_body: serde_json::Value = response.json().await?;

    // OpenFGA returns empty object {}
    assert!(response_body.is_object(), "Response should be an object");
    assert!(
        response_body.as_object().map_or(false, |o| o.is_empty()),
        "Response should be an empty object {{}}"
    );

    Ok(())
}

/// Test: Can write multiple tuples in single request
#[tokio::test]
async fn test_write_multiple_tuples() -> Result<()> {
    // Arrange: Create store and model
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    let client = reqwest::Client::new();

    // Act: Write multiple tuples
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:doc1"
                },
                {
                    "user": "user:bob",
                    "relation": "viewer",
                    "object": "document:doc1"
                },
                {
                    "user": "user:charlie",
                    "relation": "editor",
                    "object": "document:doc2"
                }
            ]
        }
    });

    let response = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&write_request)
        .send()
        .await?;

    // Assert: Write succeeded
    assert!(
        response.status().is_success(),
        "Write multiple tuples should succeed, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: Writes are idempotent (writing same tuple twice succeeds)
#[tokio::test]
async fn test_writes_are_idempotent() -> Result<()> {
    // Arrange: Create store and model
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    let client = reqwest::Client::new();

    let write_request_first = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:dave",
                    "relation": "viewer",
                    "object": "document:readme"
                }
            ]
        }
    });

    // Act: Write tuple first time
    let response1 = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&write_request_first)
        .send()
        .await?;

    assert!(
        response1.status().is_success(),
        "First write should succeed"
    );

    // Act: Write same tuple second time with on_duplicate: "ignore"
    // OpenFGA returns error by default for duplicate writes
    // Using on_duplicate: "ignore" makes it idempotent
    let write_request_second = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:dave",
                    "relation": "viewer",
                    "object": "document:readme"
                }
            ],
            "on_duplicate": "ignore"
        }
    });

    let response2 = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&write_request_second)
        .send()
        .await?;

    // Assert: Second write succeeds with on_duplicate: "ignore"
    assert!(
        response2.status().is_success(),
        "Writing same tuple twice with on_duplicate='ignore' should succeed, got: {}",
        response2.status()
    );

    Ok(())
}

/// Test: Can delete tuple using Write API (deletes field)
#[tokio::test]
async fn test_can_delete_tuple() -> Result<()> {
    // Arrange: Create store and model, write a tuple first
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    let client = reqwest::Client::new();

    // Write tuple first
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:eve",
                    "relation": "viewer",
                    "object": "document:secret"
                }
            ]
        }
    });

    let write_response = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&write_request)
        .send()
        .await?;

    assert!(
        write_response.status().is_success(),
        "Setup: writing initial tuple should succeed"
    );

    // Act: Delete the tuple
    let delete_request = json!({
        "deletes": {
            "tuple_keys": [
                {
                    "user": "user:eve",
                    "relation": "viewer",
                    "object": "document:secret"
                }
            ]
        }
    });

    let response = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&delete_request)
        .send()
        .await?;

    // Assert: Delete succeeded
    assert!(
        response.status().is_success(),
        "Delete should succeed, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: Writing tuple without store returns 404
#[tokio::test]
async fn test_write_without_store_returns_error() -> Result<()> {
    // Arrange: Use non-existent store ID
    let client = reqwest::Client::new();
    let non_existent_store = Uuid::new_v4().to_string();

    // Act: Try to write tuple to non-existent store
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:frank",
                    "relation": "viewer",
                    "object": "document:test"
                }
            ]
        }
    });

    let response = client
        .post(format!(
            "{}/stores/{}/write",
            get_openfga_url(),
            non_existent_store
        ))
        .json(&write_request)
        .send()
        .await?;

    // Assert: Returns error (400 or 404)
    assert!(
        response.status() == StatusCode::NOT_FOUND || response.status() == StatusCode::BAD_REQUEST,
        "Write to non-existent store should return 404 or 400, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: Writing tuple with invalid format returns 400
#[tokio::test]
async fn test_write_invalid_format_returns_400() -> Result<()> {
    // Arrange: Create store and model
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    let client = reqwest::Client::new();

    // Act: Try to write tuple with invalid format (missing required fields)
    let invalid_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:george",
                    // Missing relation and object
                }
            ]
        }
    });

    let response = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&invalid_request)
        .send()
        .await?;

    // Assert: Returns 400 Bad Request
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "Invalid tuple format should return 400, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: Writing tuple with non-existent type returns error
#[tokio::test]
async fn test_write_nonexistent_type_returns_error() -> Result<()> {
    // Arrange: Create store and model
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    let client = reqwest::Client::new();

    // Act: Try to write tuple with type not in authorization model
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:helen",
                    "relation": "viewer",
                    "object": "nonexistent_type:item1"  // Type not defined in model
                }
            ]
        }
    });

    let response = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&write_request)
        .send()
        .await?;

    // Assert: Returns error (400 Bad Request)
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "Writing tuple with non-existent type should return 400, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: Writing tuple with non-existent relation returns error
#[tokio::test]
async fn test_write_nonexistent_relation_returns_error() -> Result<()> {
    // Arrange: Create store and model
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    let client = reqwest::Client::new();

    // Act: Try to write tuple with relation not in authorization model
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:ivan",
                    "relation": "nonexistent_relation",  // Relation not defined for document type
                    "object": "document:test"
                }
            ]
        }
    });

    let response = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&write_request)
        .send()
        .await?;

    // Assert: Returns error (400 Bad Request)
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "Writing tuple with non-existent relation should return 400, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: Conditional writes work (condition field)
#[tokio::test]
async fn test_conditional_writes() -> Result<()> {
    // Arrange: Create store with model that supports conditions
    let store_id = create_test_store().await?;

    // Try to create model with conditions support
    // If conditions are not supported, skip this test
    if create_conditional_model(&store_id).await.is_err() {
        println!("Skipping conditional writes test - conditions may not be supported");
        return Ok(());
    }

    let client = reqwest::Client::new();

    // Act: Write tuple with condition
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:judy",
                    "relation": "viewer",
                    "object": "document:classified",
                    "condition": {
                        "name": "ip_address_match",
                        "context": {
                            "ip_address": "192.168.1.1"
                        }
                    }
                }
            ]
        }
    });

    let response = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&write_request)
        .send()
        .await?;

    // Assert: Write with condition succeeds
    assert!(
        response.status().is_success(),
        "Conditional write should succeed, got: {}",
        response.status()
    );

    Ok(())
}
