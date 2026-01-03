use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// Data Structures for Response Capture

/// Represents a captured HTTP request/response pair
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CapturedHttpExchange {
    request: HttpRequest,
    response: HttpResponse,
    timestamp: String,
}

/// HTTP request details
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct HttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Option<String>,
}

/// HTTP response details
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct HttpResponse {
    status_code: u16,
    headers: HashMap<String, String>,
    body: Option<String>,
}

/// Represents a captured gRPC request/response pair
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CapturedGrpcExchange {
    service: String,
    method: String,
    request: GrpcRequest,
    response: GrpcResponse,
    timestamp: String,
}

/// gRPC request details
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct GrpcRequest {
    metadata: HashMap<String, String>,
    message: String, // JSON representation
}

/// gRPC response details
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct GrpcResponse {
    status_code: i32,
    metadata: HashMap<String, String>,
    message: String, // JSON representation
}

/// Test: Can capture HTTP request/response pairs
#[tokio::test]
async fn test_can_capture_http_request_response_pairs() -> Result<()> {
    // Arrange: Create a sample HTTP exchange
    let exchange = create_sample_http_exchange();

    // Act: Verify we can capture all details
    // Assert: Verify exchange structure
    assert_eq!(exchange.request.method, "POST", "Method should be POST");
    assert_eq!(
        exchange.request.path, "/stores",
        "Path should be /stores"
    );
    assert!(
        exchange.request.body.is_some(),
        "Request should have a body"
    );

    // Verify response captured
    assert_eq!(
        exchange.response.status_code, 201,
        "Status should be 201 Created"
    );
    assert!(
        exchange.response.body.is_some(),
        "Response should have a body"
    );

    // Verify timestamp
    assert!(
        !exchange.timestamp.is_empty(),
        "Timestamp should not be empty"
    );

    Ok(())
}

/// Test: Can capture gRPC request/response pairs
#[tokio::test]
async fn test_can_capture_grpc_request_response_pairs() -> Result<()> {
    // Arrange: Create a sample gRPC exchange
    let exchange = create_sample_grpc_exchange();

    // Act & Assert: Verify we can capture all details
    assert_eq!(
        exchange.service, "openfga.v1.OpenFGAService",
        "Service name should match"
    );
    assert_eq!(exchange.method, "Check", "Method should be Check");

    // Verify request captured
    assert!(
        !exchange.request.message.is_empty(),
        "Request message should not be empty"
    );

    // Verify response captured
    assert_eq!(
        exchange.response.status_code, 0,
        "gRPC OK status is 0"
    );
    assert!(
        !exchange.response.message.is_empty(),
        "Response message should not be empty"
    );

    Ok(())
}

/// Test: Can serialize captured data to JSON
#[tokio::test]
async fn test_can_serialize_captured_data_to_json() -> Result<()> {
    // Arrange: Create exchanges
    let http_exchange = create_sample_http_exchange();
    let grpc_exchange = create_sample_grpc_exchange();

    // Act: Serialize to JSON
    let http_json = serde_json::to_string_pretty(&http_exchange)?;
    let grpc_json = serde_json::to_string_pretty(&grpc_exchange)?;

    // Assert: Verify JSON is valid and contains expected fields
    assert!(http_json.contains("\"method\""), "JSON should contain method");
    assert!(http_json.contains("\"path\""), "JSON should contain path");
    assert!(
        http_json.contains("\"status_code\""),
        "JSON should contain status_code"
    );

    assert!(
        grpc_json.contains("\"service\""),
        "JSON should contain service"
    );
    assert!(grpc_json.contains("\"method\""), "JSON should contain method");

    // Verify we can deserialize back
    let http_roundtrip: CapturedHttpExchange = serde_json::from_str(&http_json)?;
    assert_eq!(
        http_roundtrip, http_exchange,
        "Roundtrip should preserve data"
    );

    let grpc_roundtrip: CapturedGrpcExchange = serde_json::from_str(&grpc_json)?;
    assert_eq!(
        grpc_roundtrip, grpc_exchange,
        "Roundtrip should preserve data"
    );

    Ok(())
}

/// Test: Can load captured test cases from disk
#[tokio::test]
async fn test_can_load_captured_test_cases_from_disk() -> Result<()> {
    // Arrange: Create a temporary directory and save test case
    let temp_dir = std::env::temp_dir();
    let test_file = temp_dir.join("test_http_exchange.json");

    let exchange = create_sample_http_exchange();
    let json = serde_json::to_string_pretty(&exchange)?;
    std::fs::write(&test_file, json)?;

    // Act: Load from disk
    let loaded_json = std::fs::read_to_string(&test_file)?;
    let loaded_exchange: CapturedHttpExchange = serde_json::from_str(&loaded_json)?;

    // Assert: Verify loaded data matches original
    assert_eq!(
        loaded_exchange, exchange,
        "Loaded data should match original"
    );

    // Cleanup
    std::fs::remove_file(&test_file)?;

    Ok(())
}

/// Test: Can compare two responses for equality
#[tokio::test]
async fn test_can_compare_two_responses_for_equality() -> Result<()> {
    // Arrange: Create two identical responses
    let response1 = create_sample_http_response();
    let response2 = create_sample_http_response();

    // Act: Compare responses
    let are_equal = compare_http_responses(&response1, &response2)?;

    // Assert: Should be equal
    assert!(are_equal, "Identical responses should be equal");

    // Create a different response
    let mut response3 = create_sample_http_response();
    response3.status_code = 404;

    // Compare different responses
    let are_different = compare_http_responses(&response1, &response3)?;

    // Assert: Should not be equal
    assert!(!are_different, "Different responses should not be equal");

    Ok(())
}

/// Test: Can detect breaking changes in response format
#[tokio::test]
async fn test_can_detect_breaking_changes_in_response_format() -> Result<()> {
    // Arrange: Create original and modified responses
    let original = r#"{"id": "store123", "name": "my-store"}"#;
    let compatible = r#"{"id": "store123", "name": "my-store", "created_at": "2024-01-01"}"#;
    let breaking = r#"{"store_id": "store123", "name": "my-store"}"#;

    // Act: Detect changes
    let is_compatible = detect_breaking_changes(original, compatible)?;
    let is_breaking = detect_breaking_changes(original, breaking)?;

    // Assert: Adding fields is compatible, changing field names is breaking
    assert!(
        is_compatible,
        "Adding new fields should be compatible"
    );
    assert!(
        !is_breaking,
        "Changing field names should be breaking"
    );

    Ok(())
}

// Helper Functions

/// Create a sample HTTP exchange for testing
fn create_sample_http_exchange() -> CapturedHttpExchange {
    let mut request_headers = HashMap::new();
    request_headers.insert("Content-Type".to_string(), "application/json".to_string());

    let mut response_headers = HashMap::new();
    response_headers.insert("Content-Type".to_string(), "application/json".to_string());

    CapturedHttpExchange {
        request: HttpRequest {
            method: "POST".to_string(),
            path: "/stores".to_string(),
            headers: request_headers,
            body: Some(r#"{"name": "test-store"}"#.to_string()),
        },
        response: HttpResponse {
            status_code: 201,
            headers: response_headers,
            body: Some(r#"{"id": "store123", "name": "test-store"}"#.to_string()),
        },
        timestamp: "2024-01-01T00:00:00Z".to_string(),
    }
}

/// Create a sample gRPC exchange for testing
fn create_sample_grpc_exchange() -> CapturedGrpcExchange {
    let mut request_metadata = HashMap::new();
    request_metadata.insert("content-type".to_string(), "application/grpc".to_string());

    let mut response_metadata = HashMap::new();
    response_metadata.insert("content-type".to_string(), "application/grpc".to_string());

    CapturedGrpcExchange {
        service: "openfga.v1.OpenFGAService".to_string(),
        method: "Check".to_string(),
        request: GrpcRequest {
            metadata: request_metadata,
            message: r#"{"tuple_key": {"user": "user:alice", "relation": "viewer", "object": "document:readme"}}"#.to_string(),
        },
        response: GrpcResponse {
            status_code: 0, // gRPC OK
            metadata: response_metadata,
            message: r#"{"allowed": true}"#.to_string(),
        },
        timestamp: "2024-01-01T00:00:00Z".to_string(),
    }
}

/// Create a sample HTTP response for testing
fn create_sample_http_response() -> HttpResponse {
    let mut headers = HashMap::new();
    headers.insert("Content-Type".to_string(), "application/json".to_string());

    HttpResponse {
        status_code: 200,
        headers,
        body: Some(r#"{"id": "store123", "name": "test-store"}"#.to_string()),
    }
}

/// Compare two HTTP responses for equality
fn compare_http_responses(response1: &HttpResponse, response2: &HttpResponse) -> Result<bool> {
    Ok(response1 == response2)
}

/// Detect breaking changes between two JSON responses
///
/// A change is breaking if:
/// - A field is removed or renamed
/// - A field's type changes
///
/// Adding new fields is NOT breaking (forward compatibility)
fn detect_breaking_changes(original: &str, modified: &str) -> Result<bool> {
    let original_json: serde_json::Value = serde_json::from_str(original)?;
    let modified_json: serde_json::Value = serde_json::from_str(modified)?;

    // Get original fields
    let original_obj = original_json.as_object().ok_or_else(|| {
        anyhow::anyhow!("Original JSON must be an object")
    })?;

    let modified_obj = modified_json.as_object().ok_or_else(|| {
        anyhow::anyhow!("Modified JSON must be an object")
    })?;

    // Check if all original fields exist in modified
    for (key, original_value) in original_obj {
        if let Some(modified_value) = modified_obj.get(key) {
            // Field exists - check if type changed
            if std::mem::discriminant(original_value) != std::mem::discriminant(modified_value) {
                // Type changed - breaking!
                return Ok(false);
            }
        } else {
            // Field removed - breaking!
            return Ok(false);
        }
    }

    // All original fields present with same types - compatible!
    Ok(true)
}
