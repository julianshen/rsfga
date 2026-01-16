mod common;

use anyhow::Result;
use common::{
    create_test_store, get_grpc_url, grpc_call, grpc_streaming_call, is_grpcurl_available,
    simple_document_viewer_model, write_tuples,
};
use serde_json::json;
use std::collections::HashSet;
use std::process::Command;

// ============================================================================
// Section 34: StreamedListObjects API Tests
// ============================================================================
//
// StreamedListObjects is a gRPC-only API that streams results incrementally
// instead of returning all objects in a single response.
//
// This is useful for large result sets where waiting for all results
// would cause latency issues.
//
// Endpoint: openfga.v1.OpenFGAService/StreamedListObjects (gRPC only)
//
// Note: This API is only available via gRPC, not REST.
//
// ============================================================================

/// Check if grpcurl and StreamedListObjects are available.
///
/// Returns (grpcurl_available, method_available) tuple.
/// - First check if grpcurl is installed
/// - Then check if StreamedListObjects method exists on the server
fn check_streaming_prerequisites() -> (bool, bool) {
    if !is_grpcurl_available() {
        return (false, false);
    }

    let url = get_grpc_url();

    // Check if the method exists
    let output = Command::new("grpcurl")
        .args(["-plaintext", &url, "describe", "openfga.v1.OpenFGAService"])
        .output();

    let method_available = match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            stdout.contains("StreamedListObjects")
        }
        Err(_) => false,
    };

    (true, method_available)
}

/// Macro to skip test with appropriate message based on prerequisites
macro_rules! skip_if_unavailable {
    () => {{
        let (grpcurl_ok, method_ok) = check_streaming_prerequisites();
        if !grpcurl_ok {
            eprintln!(
                "SKIPPED: grpcurl not installed. Install via: brew install grpcurl (macOS) or see https://github.com/fullstorydev/grpcurl"
            );
            return Ok(());
        }
        if !method_ok {
            eprintln!("SKIPPED: StreamedListObjects not available in this OpenFGA version");
            return Ok(());
        }
    }};
}

/// Execute StreamedListObjects via grpcurl (uses common streaming helper)
fn grpc_streamed_listobjects(
    store_id: &str,
    request: &serde_json::Value,
) -> Result<Vec<serde_json::Value>> {
    let mut full_request = request.clone();
    full_request["store_id"] = json!(store_id);

    grpc_streaming_call(
        "openfga.v1.OpenFGAService/StreamedListObjects",
        &full_request,
    )
}

/// Test: StreamedListObjects returns same results as ListObjects
#[tokio::test]
async fn test_streamed_listobjects_returns_same_as_listobjects() -> Result<()> {
    skip_if_unavailable!();

    // Arrange
    let store_id = create_test_store().await?;

    let model = simple_document_viewer_model();
    let model_data = json!({
        "store_id": store_id,
        "type_definitions": model["type_definitions"],
        "schema_version": "1.1"
    });

    grpc_call(
        "openfga.v1.OpenFGAService/WriteAuthorizationModel",
        &model_data,
    )?;

    // Write tuples
    write_tuples(
        &store_id,
        vec![
            ("user:alice", "viewer", "document:doc1"),
            ("user:alice", "viewer", "document:doc2"),
            ("user:alice", "viewer", "document:doc3"),
        ],
    )
    .await?;

    // Act: Call StreamedListObjects
    let request = json!({
        "type": "document",
        "relation": "viewer",
        "user": "user:alice"
    });

    let streamed_results = grpc_streamed_listobjects(&store_id, &request)?;

    // Collect all objects from streamed results
    let streamed_objects: HashSet<String> = streamed_results
        .iter()
        .filter_map(|r| r.get("object").and_then(|o| o.as_str()))
        .map(String::from)
        .collect();

    // Compare with regular ListObjects
    let list_request = json!({
        "store_id": store_id,
        "type": "document",
        "relation": "viewer",
        "user": "user:alice"
    });

    let list_response = grpc_call("openfga.v1.OpenFGAService/ListObjects", &list_request)?;

    let list_objects: HashSet<String> = list_response
        .get("objects")
        .and_then(|o| o.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|o| o.as_str())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();

    // Assert: Both should return the same objects
    assert_eq!(
        streamed_objects, list_objects,
        "StreamedListObjects should return same objects as ListObjects"
    );

    assert_eq!(streamed_objects.len(), 3, "Should have 3 objects");

    Ok(())
}

/// Test: StreamedListObjects handles empty results
#[tokio::test]
async fn test_streamed_listobjects_empty_results() -> Result<()> {
    skip_if_unavailable!();

    let store_id = create_test_store().await?;

    let model = simple_document_viewer_model();
    let model_data = json!({
        "store_id": store_id,
        "type_definitions": model["type_definitions"],
        "schema_version": "1.1"
    });

    grpc_call(
        "openfga.v1.OpenFGAService/WriteAuthorizationModel",
        &model_data,
    )?;

    // Don't write any tuples

    // Act: Call StreamedListObjects
    let request = json!({
        "type": "document",
        "relation": "viewer",
        "user": "user:nobody"
    });

    let results = grpc_streamed_listobjects(&store_id, &request)?;

    // Assert: Should return empty (no objects)
    let objects: Vec<&str> = results
        .iter()
        .filter_map(|r| r.get("object").and_then(|o| o.as_str()))
        .collect();

    assert!(
        objects.is_empty(),
        "StreamedListObjects should return no objects when none match"
    );

    Ok(())
}

/// Test: StreamedListObjects handles large result sets
#[tokio::test]
async fn test_streamed_listobjects_large_result_set() -> Result<()> {
    skip_if_unavailable!();

    let store_id = create_test_store().await?;

    let model = simple_document_viewer_model();
    let model_data = json!({
        "store_id": store_id,
        "type_definitions": model["type_definitions"],
        "schema_version": "1.1"
    });

    grpc_call(
        "openfga.v1.OpenFGAService/WriteAuthorizationModel",
        &model_data,
    )?;

    // Write many tuples (50 documents)
    let tuples: Vec<_> = (0..50)
        .map(|i| {
            (
                "user:alice".to_string(),
                "viewer".to_string(),
                format!("document:doc{}", i),
            )
        })
        .collect();

    // Write in batches (max 100 per write)
    let tuple_refs: Vec<(&str, &str, &str)> = tuples
        .iter()
        .map(|(u, r, o)| (u.as_str(), r.as_str(), o.as_str()))
        .collect();

    write_tuples(&store_id, tuple_refs).await?;

    // Act: Call StreamedListObjects
    let request = json!({
        "type": "document",
        "relation": "viewer",
        "user": "user:alice"
    });

    let results = grpc_streamed_listobjects(&store_id, &request)?;

    // Collect objects
    let objects: HashSet<String> = results
        .iter()
        .filter_map(|r| r.get("object").and_then(|o| o.as_str()))
        .map(String::from)
        .collect();

    // Assert: Should return all 50 objects
    assert_eq!(
        objects.len(),
        50,
        "StreamedListObjects should return all 50 objects, got: {}",
        objects.len()
    );

    Ok(())
}

/// Test: StreamedListObjects with contextual tuples
#[tokio::test]
async fn test_streamed_listobjects_with_contextual_tuples() -> Result<()> {
    skip_if_unavailable!();

    let store_id = create_test_store().await?;

    let model = simple_document_viewer_model();
    let model_data = json!({
        "store_id": store_id,
        "type_definitions": model["type_definitions"],
        "schema_version": "1.1"
    });

    grpc_call(
        "openfga.v1.OpenFGAService/WriteAuthorizationModel",
        &model_data,
    )?;

    // Act: Call StreamedListObjects with contextual tuple
    let request = json!({
        "type": "document",
        "relation": "viewer",
        "user": "user:temp",
        "contextual_tuples": {
            "tuple_keys": [
                {
                    "user": "user:temp",
                    "relation": "viewer",
                    "object": "document:temp-doc"
                }
            ]
        }
    });

    let results = grpc_streamed_listobjects(&store_id, &request)?;

    // Collect objects
    let objects: HashSet<String> = results
        .iter()
        .filter_map(|r| r.get("object").and_then(|o| o.as_str()))
        .map(String::from)
        .collect();

    // Assert: Should include the contextual tuple's object
    assert!(
        objects.contains("document:temp-doc"),
        "StreamedListObjects should include object from contextual tuple"
    );

    Ok(())
}

/// Test: StreamedListObjects error handling
#[tokio::test]
async fn test_streamed_listobjects_error_handling() -> Result<()> {
    skip_if_unavailable!();

    // Act: Call with non-existent store
    let request = json!({
        "type": "document",
        "relation": "viewer",
        "user": "user:alice"
    });

    let result = grpc_streamed_listobjects("01ARZ3NDEKTSV4RRFFQ69G5FAV", &request);

    // Assert: Should return error for non-existent store
    assert!(
        result.is_err(),
        "StreamedListObjects with non-existent store should fail"
    );

    Ok(())
}
