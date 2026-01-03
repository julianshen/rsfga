use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// Data Structures for Response Capture

/// Represents a captured HTTP request/response pair
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CapturedHttpExchange {
    request: HttpRequest,
    response: HttpResponse,
    timestamp: DateTime<Utc>,
}

/// HTTP request details
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct HttpRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Option<String>,
}

/// HTTP response details
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct HttpResponse {
    status_code: u16,
    headers: BTreeMap<String, String>,
    body: Option<String>,
}

/// Represents a captured gRPC request/response pair
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CapturedGrpcExchange {
    service: String,
    method: String,
    request: GrpcRequest,
    response: GrpcResponse,
    timestamp: DateTime<Utc>,
}

/// gRPC request details
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct GrpcRequest {
    metadata: BTreeMap<String, String>,
    message: String, // JSON representation
}

/// gRPC response details
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct GrpcResponse {
    status_code: i32,
    metadata: BTreeMap<String, String>,
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

    // Verify timestamp is a valid DateTime
    let expected_timestamp = DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    assert_eq!(
        exchange.timestamp, expected_timestamp,
        "Timestamp should match expected value"
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
    // Arrange: Create a temporary file and save test case
    let temp_file = tempfile::Builder::new()
        .prefix("test_http_exchange")
        .suffix(".json")
        .tempfile()?;

    let exchange = create_sample_http_exchange();
    let json = serde_json::to_string_pretty(&exchange)?;
    std::fs::write(temp_file.path(), json)?;

    // Act: Load from disk
    let loaded_json = std::fs::read_to_string(temp_file.path())?;
    let loaded_exchange: CapturedHttpExchange = serde_json::from_str(&loaded_json)?;

    // Assert: Verify loaded data matches original
    assert_eq!(
        loaded_exchange, exchange,
        "Loaded data should match original"
    );

    // Temp file automatically cleaned up when temp_file goes out of scope
    Ok(())
}

/// Test: Can compare two responses for equality
#[tokio::test]
async fn test_can_compare_two_responses_for_equality() -> Result<()> {
    // Arrange: Create two identical responses
    let response1 = create_sample_http_response();
    let response2 = create_sample_http_response();

    // Assert: Should be equal
    assert_eq!(response1, response2, "Identical responses should be equal");

    // Arrange: Create a different response
    let mut response3 = create_sample_http_response();
    response3.status_code = 404;

    // Assert: Should not be equal
    assert_ne!(response1, response3, "Different responses should not be equal");

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
    let is_compatible_with_breaking_payload = detect_breaking_changes(original, breaking)?;

    // Assert: Adding fields is compatible, changing field names is breaking
    assert!(
        is_compatible,
        "Adding new fields should be compatible"
    );
    assert!(
        !is_compatible_with_breaking_payload,
        "Changing field names should be breaking"
    );

    Ok(())
}

/// Test: Can detect breaking changes in nested objects
#[tokio::test]
async fn test_can_detect_breaking_changes_in_nested_objects() -> Result<()> {
    // Arrange: Create responses with nested type changes
    let original = r#"{"store": {"id": "store123", "metadata": {"version": "1.0"}}}"#;

    // Compatible: Adding new nested field
    let compatible = r#"{"store": {"id": "store123", "metadata": {"version": "1.0", "created_at": "2024-01-01"}}}"#;

    // Breaking: Type change in nested field (string -> number)
    let breaking_nested_type = r#"{"store": {"id": 123, "metadata": {"version": "1.0"}}}"#;

    // Breaking: Removed nested field
    let breaking_removed_field = r#"{"store": {"id": "store123", "metadata": {}}}"#;

    // Breaking: Type change in deeply nested field
    let breaking_deep = r#"{"store": {"id": "store123", "metadata": {"version": 1.0}}}"#;

    // Act & Assert: Test each scenario
    assert!(
        detect_breaking_changes(original, compatible)?,
        "Adding nested field should be compatible"
    );

    assert!(
        !detect_breaking_changes(original, breaking_nested_type)?,
        "Type change in nested field should be breaking"
    );

    assert!(
        !detect_breaking_changes(original, breaking_removed_field)?,
        "Removing nested field should be breaking"
    );

    assert!(
        !detect_breaking_changes(original, breaking_deep)?,
        "Type change in deeply nested field should be breaking"
    );

    Ok(())
}

/// Test: Can detect breaking changes in arrays
#[tokio::test]
async fn test_can_detect_breaking_changes_in_arrays() -> Result<()> {
    // Arrange: Create responses with array fields
    let original = r#"{"stores": [{"id": "store1", "name": "Store 1"}]}"#;

    // Compatible: Same array structure
    let compatible = r#"{"stores": [{"id": "store1", "name": "Store 1"}, {"id": "store2", "name": "Store 2"}]}"#;

    // Breaking: Array element type changed
    let breaking = r#"{"stores": [{"id": 123, "name": "Store 1"}]}"#;

    // Act & Assert
    assert!(
        detect_breaking_changes(original, compatible)?,
        "Adding array elements should be compatible"
    );

    assert!(
        !detect_breaking_changes(original, breaking)?,
        "Type change in array element should be breaking"
    );

    Ok(())
}

// Helper Functions

/// Create a sample HTTP exchange for testing
fn create_sample_http_exchange() -> CapturedHttpExchange {
    let mut request_headers = BTreeMap::new();
    request_headers.insert("Content-Type".to_string(), "application/json".to_string());

    let mut response_headers = BTreeMap::new();
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
        timestamp: DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
    }
}

/// Create a sample gRPC exchange for testing
fn create_sample_grpc_exchange() -> CapturedGrpcExchange {
    let mut request_metadata = BTreeMap::new();
    request_metadata.insert("content-type".to_string(), "application/grpc".to_string());

    let mut response_metadata = BTreeMap::new();
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
        timestamp: DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
    }
}

/// Create a sample HTTP response for testing
fn create_sample_http_response() -> HttpResponse {
    let mut headers = BTreeMap::new();
    headers.insert("Content-Type".to_string(), "application/json".to_string());

    HttpResponse {
        status_code: 200,
        headers,
        body: Some(r#"{"id": "store123", "name": "test-store"}"#.to_string()),
    }
}

/// Detect breaking changes between two JSON responses
///
/// A change is breaking if:
/// - A field is removed or renamed
/// - A field's type changes (including in nested objects)
///
/// Adding new fields is NOT breaking (forward compatibility)
fn detect_breaking_changes(original: &str, modified: &str) -> Result<bool> {
    let original_json: serde_json::Value = serde_json::from_str(original)?;
    let modified_json: serde_json::Value = serde_json::from_str(modified)?;

    check_compatible(&original_json, &modified_json)
}

/// Recursively check if two JSON values are compatible
///
/// Compatibility rules:
/// - Objects: All original fields must exist in modified with compatible types
/// - Arrays: Element types must be compatible (checks first element)
/// - Primitives: Must be same type
fn check_compatible(original: &serde_json::Value, modified: &serde_json::Value) -> Result<bool> {
    use serde_json::Value;

    match (original, modified) {
        // Both are objects - recursively check all fields
        (Value::Object(orig_map), Value::Object(mod_map)) => {
            for (key, orig_value) in orig_map {
                if let Some(mod_value) = mod_map.get(key) {
                    // Field exists - recursively check compatibility
                    if !check_compatible(orig_value, mod_value)? {
                        return Ok(false);
                    }
                } else {
                    // Field removed - breaking!
                    return Ok(false);
                }
            }
            Ok(true)
        }

        // Both are arrays - check element types recursively
        (Value::Array(orig_arr), Value::Array(mod_arr)) => {
            // For arrays, we check if the first element types are compatible
            // (OpenFGA arrays are typically homogeneous)
            if let (Some(orig_first), Some(mod_first)) = (orig_arr.first(), mod_arr.first()) {
                check_compatible(orig_first, mod_first)
            } else {
                // Empty arrays or modified array is empty - compatible
                Ok(true)
            }
        }

        // Both are same primitive type - compatible
        (Value::Null, Value::Null)
        | (Value::Bool(_), Value::Bool(_))
        | (Value::Number(_), Value::Number(_))
        | (Value::String(_), Value::String(_)) => Ok(true),

        // Different types - breaking change!
        _ => Ok(false),
    }
}
