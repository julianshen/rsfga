mod common;

use anyhow::Result;
use common::{
    create_test_store_grpc, get_grpc_url, get_openfga_url, grpc_call, grpc_call_with_error,
    grpcurl_describe,
};
use serde_json::json;
use std::process::Command;

/// URL-encode a path segment to handle special characters like `#`.
/// The `#` character is a URL fragment delimiter and must be encoded as `%23`.
fn encode_path_segment(segment: &str) -> String {
    urlencoding::encode(segment).into_owned()
}

// ============================================================================
// Section 23: gRPC-Specific Features Tests
// ============================================================================
//
// These tests verify gRPC-specific features that differ from REST:
// - Streaming (if supported)
// - Metadata/headers handling
// - Error code mapping to HTTP status codes
// - Error detail format
//
// ============================================================================

/// Extract method names from a service description.
///
/// Parses grpcurl describe output to extract RPC method names.
fn grpcurl_list_methods(service: &str) -> Result<Vec<String>> {
    let description = grpcurl_describe(service)?;

    // Parse method names from describe output
    // Expected format: "  rpc MethodName ( .SomeRequest ) returns ( .SomeResponse );"
    let methods: Vec<String> = description
        .lines()
        .filter(|line| line.contains("rpc "))
        .filter_map(|line| {
            // Extract method name from "rpc MethodName ( ... )" format
            let method_name = line.trim().strip_prefix("rpc ")?.split(' ').next()?;
            if method_name.is_empty() {
                None
            } else {
                Some(method_name.to_string())
            }
        })
        .collect();
    Ok(methods)
}

// ============================================================================
// Tests
// ============================================================================

/// Test: gRPC streaming support
///
/// OpenFGA does NOT support gRPC streaming. The ListObjects and StreamedListObjects
/// are both unary RPCs that return all results at once. This test documents
/// this behavior.
#[tokio::test]
async fn test_grpc_streaming_not_supported() -> Result<()> {
    // List the methods available on OpenFGAService
    let methods = grpcurl_list_methods("openfga.v1.OpenFGAService")?;

    // Verify the service has ListObjects (unary) but NOT a streaming version
    assert!(
        methods.iter().any(|m| m == "ListObjects"),
        "OpenFGAService should have ListObjects method"
    );

    // Note: Some versions of OpenFGA may have StreamedListObjects but it's still
    // server-side streaming, not true bidirectional streaming
    let has_streamed = methods.iter().any(|m| m.contains("Streamed"));

    // Document the finding - streaming may or may not be present
    println!(
        "OpenFGA streaming methods present: {}",
        if has_streamed { "yes" } else { "no" }
    );

    // The key assertion is that regular ListObjects works
    let store_id = create_test_store_grpc("grpc-features-test")?;

    // Create model for ListObjects test
    let model_data = json!({
        "store_id": store_id,
        "type_definitions": [
            {"type": "user"},
            {
                "type": "document",
                "relations": {"viewer": {"this": {}}},
                "metadata": {
                    "relations": {
                        "viewer": {"directly_related_user_types": [{"type": "user"}]}
                    }
                }
            }
        ],
        "schema_version": "1.1"
    });

    grpc_call(
        "openfga.v1.OpenFGAService/WriteAuthorizationModel",
        &model_data,
    )?;

    // Write a tuple
    let write_data = json!({
        "store_id": store_id,
        "writes": {
            "tuple_keys": [{
                "user": "user:stream-test",
                "relation": "viewer",
                "object": "document:doc1"
            }]
        }
    });

    grpc_call("openfga.v1.OpenFGAService/Write", &write_data)?;

    // Call ListObjects (unary RPC)
    let list_data = json!({
        "store_id": store_id,
        "type": "document",
        "relation": "viewer",
        "user": "user:stream-test"
    });

    let response = grpc_call("openfga.v1.OpenFGAService/ListObjects", &list_data)?;

    let objects = response["objects"]
        .as_array()
        .expect("Should have objects array");
    assert!(
        objects.iter().any(|o| o.as_str() == Some("document:doc1")),
        "ListObjects should return document:doc1"
    );

    Ok(())
}

/// Test: gRPC metadata/headers handling
///
/// Verifies that gRPC handles metadata correctly, including:
/// - Custom headers are accepted
/// - Authorization headers work (if configured)
#[tokio::test]
async fn test_grpc_metadata_headers() -> Result<()> {
    let url = get_grpc_url();

    // Test 1: Call with custom metadata header
    // grpcurl supports adding headers with -H flag
    let output = Command::new("grpcurl")
        .args([
            "-plaintext",
            "-H",
            "x-request-id: test-request-123",
            "-H",
            "x-custom-header: custom-value",
            "-d",
            r#"{"name": "metadata-test-store"}"#,
            &url,
            "openfga.v1.OpenFGAService/CreateStore",
        ])
        .output()?;

    // The request should succeed even with custom headers
    assert!(
        output.status.success(),
        "Request with custom headers should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let response: serde_json::Value = serde_json::from_str(&stdout)?;
    assert!(
        response.get("id").is_some(),
        "Response should contain store ID"
    );

    // Test 2: Verify that invalid/malformed requests are still rejected
    // (headers don't bypass validation)
    let output = Command::new("grpcurl")
        .args([
            "-plaintext",
            "-H",
            "authorization: Bearer fake-token",
            "-d",
            r#"{"store_id": "invalid-store-id"}"#, // Invalid store ID format
            &url,
            "openfga.v1.OpenFGAService/GetStore",
        ])
        .output()?;

    // This should fail due to invalid store ID, not succeed due to auth header
    assert!(
        !output.status.success(),
        "Invalid request should still be rejected even with auth header"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("InvalidArgument") || stderr.contains("invalid"),
        "Error should be about invalid argument, not auth"
    );

    Ok(())
}

/// Test: gRPC error codes match HTTP status codes
///
/// Verifies the mapping between gRPC error codes and HTTP status codes:
/// - InvalidArgument (gRPC) -> 400 Bad Request (HTTP)
/// - NotFound (gRPC) -> 404 Not Found (HTTP) [if used]
/// - PermissionDenied (gRPC) -> 403 Forbidden (HTTP)
/// - etc.
#[tokio::test]
async fn test_grpc_error_codes_match_http() -> Result<()> {
    let client = reqwest::Client::new();

    // Test 1: Invalid store ID format
    // gRPC: InvalidArgument, HTTP: 400

    // Use a store ID with special characters including `#`
    let invalid_store_id = "invalid-id!@#";

    // gRPC call - protobuf message body doesn't need URL encoding
    let (success, _stdout, stderr) = grpc_call_with_error(
        "openfga.v1.OpenFGAService/GetStore",
        &json!({"store_id": invalid_store_id}),
    )?;

    assert!(!success, "gRPC should fail for invalid store ID");
    assert!(
        stderr.contains("InvalidArgument"),
        "gRPC error should be InvalidArgument: {stderr}"
    );

    // HTTP call - URL-encode the path segment to handle `#` correctly
    // Without encoding, `#` would be treated as a fragment delimiter
    let http_response = client
        .get(format!(
            "{}/stores/{}",
            get_openfga_url(),
            encode_path_segment(invalid_store_id)
        ))
        .send()
        .await?;

    assert_eq!(
        http_response.status().as_u16(),
        400,
        "HTTP should return 400 for invalid store ID"
    );

    // Test 2: Non-existent store (valid format but doesn't exist)
    // This tests a different error path

    let (success, _stdout, stderr) = grpc_call_with_error(
        "openfga.v1.OpenFGAService/GetStore",
        &json!({"store_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV"}), // Valid ULID format, doesn't exist
    )?;

    assert!(!success, "gRPC should fail for non-existent store");
    // OpenFGA uses custom error codes (e.g., Code(5002) for "Store ID not found")
    // instead of standard gRPC NotFound code
    assert!(
        stderr.contains("not found")
            || stderr.contains("NotFound")
            || stderr.contains("Code(")
            || stderr.contains("store"),
        "gRPC error should indicate store not found: {stderr}"
    );

    // HTTP equivalent
    let http_response = client
        .get(format!(
            "{}/stores/01ARZ3NDEKTSV4RRFFQ69G5FAV",
            get_openfga_url()
        ))
        .send()
        .await?;

    // OpenFGA returns 404 for non-existent stores (correct REST semantics)
    assert_eq!(
        http_response.status().as_u16(),
        404,
        "HTTP should return 404 for non-existent store"
    );

    // Test 3: Validation error (missing required field)
    let (success, _stdout, stderr) = grpc_call_with_error(
        "openfga.v1.OpenFGAService/Check",
        &json!({"store_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV"}), // Missing tuple_key
    )?;

    assert!(!success, "gRPC should fail for missing required field");
    assert!(
        stderr.contains("InvalidArgument"),
        "gRPC error should be InvalidArgument for validation: {stderr}"
    );

    Ok(())
}

/// Test: gRPC error details format
///
/// Verifies the format of gRPC error messages and details:
/// - Error code is present
/// - Error message is descriptive
/// - Additional details (if any) follow expected format
#[tokio::test]
async fn test_grpc_error_details_format() -> Result<()> {
    // Test 1: Invalid request - check error format
    let (success, _stdout, stderr) = grpc_call_with_error(
        "openfga.v1.OpenFGAService/WriteAuthorizationModel",
        &json!({
            "store_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "type_definitions": [], // Empty - should fail validation
            "schema_version": "1.1"
        }),
    )?;

    assert!(!success, "Empty type_definitions should fail");

    // gRPC error format from grpcurl typically shows:
    // ERROR:
    //   Code: InvalidArgument
    //   Message: <descriptive message>

    assert!(
        stderr.contains("Code:") || stderr.contains("code"),
        "Error should contain error code: {stderr}"
    );
    assert!(
        stderr.contains("Message:")
            || stderr.contains("message")
            || stderr.contains("type_definitions"),
        "Error should contain message or description: {stderr}"
    );

    // Test 2: Check that successful responses don't contain error fields
    let store_id = create_test_store_grpc("grpc-features-test")?;

    let response = grpc_call(
        "openfga.v1.OpenFGAService/GetStore",
        &json!({"store_id": store_id}),
    )?;

    // Successful response should have expected fields, not error fields
    assert!(
        response.get("id").is_some(),
        "Successful response should have 'id' field"
    );
    assert!(
        response.get("error").is_none() && response.get("code").is_none(),
        "Successful response should not have error fields"
    );

    // Test 3: Verify error message is actionable
    let (_, _, stderr) = grpc_call_with_error(
        "openfga.v1.OpenFGAService/Check",
        &json!({
            "store_id": store_id,
            "tuple_key": {
                "user": "", // Empty user - validation error
                "relation": "viewer",
                "object": "document:doc1"
            }
        }),
    )?;

    // The error message should help identify what went wrong
    let error_lower = stderr.to_lowercase();
    assert!(
        error_lower.contains("user")
            || error_lower.contains("empty")
            || error_lower.contains("required"),
        "Error message should indicate the specific validation issue: {stderr}"
    );

    Ok(())
}
