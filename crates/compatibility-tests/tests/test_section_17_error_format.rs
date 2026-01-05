mod common;

use anyhow::Result;
use common::{create_test_model, create_test_store, get_openfga_url};
use serde::Deserialize;
use serde_json::json;

// ============================================================================
// Error Response Structure
// ============================================================================

/// OpenFGA error response format
#[derive(Debug, Deserialize)]
struct ErrorResponse {
    code: String,
    message: String,
}

// ============================================================================
// Section 17: Error Response Format Tests
// ============================================================================

/// Test: 400 Bad Request format (error code, message, details)
#[tokio::test]
async fn test_400_bad_request_format() -> Result<()> {
    // Arrange
    let client = reqwest::Client::new();

    // Act: Send request with invalid store ID format (not valid ULID)
    let response = client
        .get(format!("{}/stores/invalid-store-id", get_openfga_url()))
        .send()
        .await?;

    // Assert: Should return 400 with proper error format
    assert_eq!(
        response.status().as_u16(),
        400,
        "Invalid store ID format should return 400"
    );

    let error: ErrorResponse = response.json().await?;

    // Verify error response structure
    assert!(
        !error.code.is_empty(),
        "Error response should have non-empty 'code' field"
    );
    assert!(
        !error.message.is_empty(),
        "Error response should have non-empty 'message' field"
    );

    // Verify error code format (should be snake_case)
    assert_eq!(
        error.code, "validation_error",
        "Invalid format should return validation_error code"
    );

    // Verify message contains useful information
    assert!(
        error.message.contains("StoreId"),
        "Error message should reference the invalid field: {}",
        error.message
    );

    Ok(())
}

/// Test: 404 Not Found format
#[tokio::test]
async fn test_404_not_found_format() -> Result<()> {
    // Arrange
    let client = reqwest::Client::new();

    // Act: Request non-existent store with valid ULID format
    // Using a valid ULID format that doesn't exist
    let response = client
        .get(format!(
            "{}/stores/01AAAAAAAAAAAAAAAAAAAAAA00",
            get_openfga_url()
        ))
        .send()
        .await?;

    // Assert: Should return 404 with proper error format
    assert_eq!(
        response.status().as_u16(),
        404,
        "Non-existent store should return 404"
    );

    let error: ErrorResponse = response.json().await?;

    // Verify error response structure
    assert!(
        !error.code.is_empty(),
        "Error response should have non-empty 'code' field"
    );
    assert!(
        !error.message.is_empty(),
        "Error response should have non-empty 'message' field"
    );

    // Verify error code
    assert_eq!(
        error.code, "store_id_not_found",
        "Non-existent store should return store_id_not_found code"
    );

    Ok(())
}

/// Test: 500 Internal Server Error format
/// Note: 500 errors are difficult to trigger intentionally.
/// This test documents the expected format when they do occur.
#[tokio::test]
async fn test_500_error_format_documentation() -> Result<()> {
    // This test documents the expected 500 error format.
    // Since 500 errors are hard to trigger intentionally without causing
    // actual server issues, we document the expected format based on
    // OpenFGA's error handling patterns.
    //
    // Expected 500 error format:
    // {
    //     "code": "internal_error",
    //     "message": "<error description>"
    // }
    //
    // HTTP Status: 500
    //
    // Note: In production, 500 errors typically occur due to:
    // - Database connectivity issues
    // - Resource exhaustion
    // - Unexpected internal state
    //
    // RSFGA should match this format for any internal errors.

    // For now, verify we can handle the format if we receive it
    let error_json = json!({
        "code": "internal_error",
        "message": "An internal error occurred"
    });

    let error: ErrorResponse = serde_json::from_value(error_json)?;
    assert_eq!(error.code, "internal_error");
    assert!(!error.message.is_empty());

    Ok(())
}

/// Test: Error response is consistent across all endpoints
#[tokio::test]
async fn test_error_response_consistent_across_endpoints() -> Result<()> {
    // Arrange
    let store_id = create_test_store().await?;
    let client = reqwest::Client::new();

    // Test errors from different endpoints

    // 1. Authorization Model endpoint error (no models exist)
    let model_error_response = client
        .get(format!(
            "{}/stores/{}/authorization-models/01AAAAAAAAAAAAAAAAAAAAAA00",
            get_openfga_url(),
            store_id
        ))
        .send()
        .await?;

    let model_error: ErrorResponse = model_error_response.json().await?;
    assert!(!model_error.code.is_empty(), "Model error should have code");
    assert!(
        !model_error.message.is_empty(),
        "Model error should have message"
    );

    // 2. Write endpoint error (no model exists)
    let write_error_response = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:test",
                    "relation": "viewer",
                    "object": "document:1"
                }]
            }
        }))
        .send()
        .await?;

    let write_error: ErrorResponse = write_error_response.json().await?;
    assert!(!write_error.code.is_empty(), "Write error should have code");
    assert!(
        !write_error.message.is_empty(),
        "Write error should have message"
    );

    // 3. Check endpoint error (no model exists)
    let check_error_response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&json!({
            "tuple_key": {
                "user": "user:test",
                "relation": "viewer",
                "object": "document:1"
            }
        }))
        .send()
        .await?;

    let check_error: ErrorResponse = check_error_response.json().await?;
    assert!(!check_error.code.is_empty(), "Check error should have code");
    assert!(
        !check_error.message.is_empty(),
        "Check error should have message"
    );

    // All errors should have the same structure (code + message)
    // This confirms consistency across endpoints
    assert!(
        model_error
            .code
            .chars()
            .all(|c| c.is_lowercase() || c == '_'),
        "Error code should be snake_case: {}",
        model_error.code
    );
    assert!(
        write_error
            .code
            .chars()
            .all(|c| c.is_lowercase() || c == '_'),
        "Error code should be snake_case: {}",
        write_error.code
    );
    assert!(
        check_error
            .code
            .chars()
            .all(|c| c.is_lowercase() || c == '_'),
        "Error code should be snake_case: {}",
        check_error.code
    );

    Ok(())
}

/// Test: Error codes match OpenFGA error code enum
#[tokio::test]
async fn test_error_codes_match_openfga_enum() -> Result<()> {
    // Arrange
    let store_id = create_test_store().await?;
    let client = reqwest::Client::new();

    // OpenFGA defines a set of standard error codes.
    // This test verifies that common errors return the expected codes.

    // Known OpenFGA error codes (from their protobuf definitions):
    // - validation_error: Input validation failures
    // - store_id_not_found: Store doesn't exist
    // - authorization_model_not_found: Model doesn't exist
    // - latest_authorization_model_not_found: No models in store
    // - type_definitions_too_few_items: Empty type definitions
    // - invalid_write_input: Invalid write request
    // - cannot_allow_duplicate_tuples_in_one_request: Duplicate tuples
    // - cannot_allow_duplicate_types_in_one_request: Duplicate types
    // - invalid_continuation_token: Bad pagination token

    // Test 1: validation_error
    let response1 = client
        .get(format!("{}/stores/invalid-id", get_openfga_url()))
        .send()
        .await?;
    let error1: ErrorResponse = response1.json().await?;
    assert_eq!(error1.code, "validation_error");

    // Test 2: store_id_not_found
    let response2 = client
        .get(format!(
            "{}/stores/01AAAAAAAAAAAAAAAAAAAAAA00",
            get_openfga_url()
        ))
        .send()
        .await?;
    let error2: ErrorResponse = response2.json().await?;
    assert_eq!(error2.code, "store_id_not_found");

    // Test 3: authorization_model_not_found
    let response3 = client
        .get(format!(
            "{}/stores/{}/authorization-models/01AAAAAAAAAAAAAAAAAAAAAA00",
            get_openfga_url(),
            store_id
        ))
        .send()
        .await?;
    let error3: ErrorResponse = response3.json().await?;
    assert_eq!(error3.code, "authorization_model_not_found");

    // Test 4: type_definitions_too_few_items
    let response4 = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&json!({
            "schema_version": "1.1",
            "type_definitions": []
        }))
        .send()
        .await?;
    let error4: ErrorResponse = response4.json().await?;
    assert_eq!(error4.code, "type_definitions_too_few_items");

    Ok(())
}

/// Test: Validation errors include field-level details
#[tokio::test]
async fn test_validation_errors_include_field_details() -> Result<()> {
    // Arrange
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;
    let client = reqwest::Client::new();

    // Test 1: Missing required fields in check request
    let response1 = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&json!({
            "tuple_key": {
                "user": "user:test"
                // Missing: relation, object
            }
        }))
        .send()
        .await?;

    let error1: ErrorResponse = response1.json().await?;
    assert_eq!(error1.code, "validation_error");

    // Error message should mention the missing/invalid fields
    assert!(
        error1.message.contains("Relation") || error1.message.contains("relation"),
        "Error should mention missing relation field: {}",
        error1.message
    );
    assert!(
        error1.message.contains("Object") || error1.message.contains("object"),
        "Error should mention missing object field: {}",
        error1.message
    );

    // Test 2: Invalid format in write request
    let response2 = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&json!({
            "writes": {
                "tuple_keys": [{
                    "user": "", // Empty user
                    "relation": "viewer",
                    "object": "document:1"
                }]
            }
        }))
        .send()
        .await?;

    let error2: ErrorResponse = response2.json().await?;

    // Error should indicate which field is invalid
    assert!(
        error2.message.to_lowercase().contains("user")
            || error2.message.contains("TupleKey")
            || error2.message.contains("tuple"),
        "Error should indicate invalid user field: {}",
        error2.message
    );

    // Test 3: Multiple validation errors in one request
    // Empty store name should fail validation
    let response3 = client
        .post(format!("{}/stores", get_openfga_url()))
        .json(&json!({
            "name": "ab" // Too short (min 3 chars)
        }))
        .send()
        .await?;

    let error3: ErrorResponse = response3.json().await?;
    assert_eq!(error3.code, "validation_error");
    assert!(
        error3.message.contains("Name"),
        "Error should mention the Name field: {}",
        error3.message
    );

    Ok(())
}
