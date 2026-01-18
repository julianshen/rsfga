mod common;

use anyhow::Result;
use common::{grpc_call, grpcurl_describe, grpcurl_list};
use serde_json::json;

// ============================================================================
// Tests
// ============================================================================

/// Test: Can connect to OpenFGA gRPC service
#[tokio::test]
async fn test_can_connect_to_openfga_grpc_service() -> Result<()> {
    // Act: List available gRPC services
    let services = grpcurl_list()?;

    // Assert: OpenFGA service should be available
    assert!(
        services
            .iter()
            .any(|s| s.contains("openfga.v1.OpenFGAService")),
        "OpenFGA gRPC service should be available. Found: {services:?}"
    );

    // Also verify health service is available
    assert!(
        services.iter().any(|s| s.contains("grpc.health.v1.Health")),
        "gRPC health service should be available"
    );

    Ok(())
}

/// Test: gRPC reflection works (can discover services)
#[tokio::test]
async fn test_grpc_reflection_works() -> Result<()> {
    // Act: Describe the OpenFGA service to verify reflection
    let description = grpcurl_describe("openfga.v1.OpenFGAService")?;

    // Assert: Should return service description with methods
    assert!(
        description.contains("service OpenFGAService"),
        "Should describe OpenFGAService"
    );
    assert!(
        description.contains("CreateStore"),
        "Should include CreateStore method"
    );
    assert!(description.contains("Check"), "Should include Check method");
    assert!(description.contains("Write"), "Should include Write method");

    // Verify we can describe a specific method
    let check_desc = grpcurl_describe("openfga.v1.OpenFGAService.Check")?;
    assert!(
        check_desc.contains("CheckRequest") && check_desc.contains("CheckResponse"),
        "Should describe Check method with request/response types"
    );

    Ok(())
}

/// Test: Can call Store service methods
#[tokio::test]
async fn test_can_call_store_service_methods() -> Result<()> {
    // Act: Create a store via gRPC
    let create_request = json!({
        "name": "grpc-store-test"
    });

    let response = grpc_call("openfga.v1.OpenFGAService/CreateStore", &create_request)?;

    // Assert: Should return store with ID
    assert!(
        response.get("id").is_some(),
        "CreateStore should return store ID"
    );
    let store_id = response["id"]
        .as_str()
        .expect("CreateStore response should have string 'id'");
    assert!(!store_id.is_empty(), "Store ID should not be empty");

    // Act: Get store via gRPC
    let get_request = json!({
        "store_id": store_id
    });

    let get_response = grpc_call("openfga.v1.OpenFGAService/GetStore", &get_request)?;

    // Assert: Should return the same store
    assert_eq!(
        get_response.get("id").and_then(|v| v.as_str()),
        Some(store_id),
        "GetStore should return the same store"
    );
    assert_eq!(
        get_response.get("name").and_then(|v| v.as_str()),
        Some("grpc-store-test"),
        "Store name should match"
    );

    // Act: List stores via gRPC
    let list_response = grpc_call("openfga.v1.OpenFGAService/ListStores", &json!({}))?;

    // Assert: Should return stores array
    assert!(
        list_response.get("stores").is_some(),
        "ListStores should return stores array"
    );

    // Act: Delete store via gRPC
    let delete_request = json!({
        "store_id": store_id
    });

    let delete_response = grpc_call("openfga.v1.OpenFGAService/DeleteStore", &delete_request)?;

    // Assert: Delete should succeed (empty response)
    assert!(
        delete_response.is_object(),
        "DeleteStore should return empty object"
    );

    Ok(())
}

/// Test: Can call Authorization Model service methods
#[tokio::test]
async fn test_can_call_authorization_model_service_methods() -> Result<()> {
    // Create a store first
    let store_response = grpc_call(
        "openfga.v1.OpenFGAService/CreateStore",
        &json!({"name": "grpc-model-test"}),
    )?;
    let store_id = store_response["id"]
        .as_str()
        .expect("CreateStore response should have string 'id'");

    // Act: Write an authorization model via gRPC
    let model_request = json!({
        "store_id": store_id,
        "type_definitions": [
            {"type": "user"},
            {
                "type": "document",
                "relations": {
                    "viewer": {"this": {}}
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [{"type": "user"}]
                        }
                    }
                }
            }
        ],
        "schema_version": "1.1"
    });

    let model_response = grpc_call(
        "openfga.v1.OpenFGAService/WriteAuthorizationModel",
        &model_request,
    )?;

    // Assert: Should return model ID
    assert!(
        model_response.get("authorization_model_id").is_some()
            || model_response.get("authorizationModelId").is_some(),
        "WriteAuthorizationModel should return model ID (checked both snake_case and camelCase)"
    );
    let model_id = model_response.get("authorization_model_id")
        .or_else(|| model_response.get("authorizationModelId"))
        .and_then(|v| v.as_str())
        .expect("WriteAuthorizationModel response should have string 'authorization_model_id' or 'authorizationModelId'");

    // Act: Read the authorization model
    let read_request = json!({
        "store_id": store_id,
        "id": model_id
    });

    let read_response = grpc_call(
        "openfga.v1.OpenFGAService/ReadAuthorizationModel",
        &read_request,
    )?;

    // Assert: Should return the model
    assert!(
        read_response.get("authorization_model").is_some()
            || read_response.get("authorizationModel").is_some(),
        "ReadAuthorizationModel should return the model (checked both snake_case and camelCase)"
    );

    // Act: List authorization models
    let list_request = json!({
        "store_id": store_id
    });

    let list_response = grpc_call(
        "openfga.v1.OpenFGAService/ReadAuthorizationModels",
        &list_request,
    )?;

    // Assert: Should return models array
    assert!(
        list_response.get("authorization_models").is_some() || list_response.get("authorizationModels").is_some(),
        "ReadAuthorizationModels should return models array (checked both snake_case and camelCase)"
    );

    Ok(())
}

/// Test: Can call Tuple service methods
#[tokio::test]
async fn test_can_call_tuple_service_methods() -> Result<()> {
    // Create store and model
    let store_response = grpc_call(
        "openfga.v1.OpenFGAService/CreateStore",
        &json!({"name": "grpc-tuple-test"}),
    )?;
    let store_id = store_response["id"]
        .as_str()
        .expect("CreateStore response should have string 'id'");

    let model_request = json!({
        "store_id": store_id,
        "type_definitions": [
            {"type": "user"},
            {
                "type": "document",
                "relations": {
                    "viewer": {"this": {}}
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [{"type": "user"}]
                        }
                    }
                }
            }
        ],
        "schema_version": "1.1"
    });

    grpc_call(
        "openfga.v1.OpenFGAService/WriteAuthorizationModel",
        &model_request,
    )?;

    // Act: Write tuples via gRPC
    let write_request = json!({
        "store_id": store_id,
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:doc1"
                }
            ]
        }
    });

    let write_response = grpc_call("openfga.v1.OpenFGAService/Write", &write_request)?;

    // Assert: Write should succeed (empty response)
    assert!(
        write_response.is_object(),
        "Write should return object (empty on success)"
    );

    // Act: Read tuples via gRPC
    let read_request = json!({
        "store_id": store_id,
        "tuple_key": {
            "user": "user:alice",
            "relation": "viewer",
            "object": "document:doc1"
        }
    });

    let read_response = grpc_call("openfga.v1.OpenFGAService/Read", &read_request)?;

    // Assert: Should return the tuple
    assert!(
        read_response.get("tuples").is_some(),
        "Read should return tuples array"
    );
    let tuples = read_response["tuples"]
        .as_array()
        .expect("Read response should have 'tuples' array");
    assert_eq!(tuples.len(), 1, "Should return exactly one tuple");

    Ok(())
}

/// Test: Can call Check service methods
#[tokio::test]
async fn test_can_call_check_service_methods() -> Result<()> {
    // Create store and model
    let store_response = grpc_call(
        "openfga.v1.OpenFGAService/CreateStore",
        &json!({"name": "grpc-check-test"}),
    )?;
    let store_id = store_response["id"]
        .as_str()
        .expect("CreateStore response should have string 'id'");

    let model_request = json!({
        "store_id": store_id,
        "type_definitions": [
            {"type": "user"},
            {
                "type": "document",
                "relations": {
                    "viewer": {"this": {}}
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [{"type": "user"}]
                        }
                    }
                }
            }
        ],
        "schema_version": "1.1"
    });

    grpc_call(
        "openfga.v1.OpenFGAService/WriteAuthorizationModel",
        &model_request,
    )?;

    // Write a tuple
    let write_request = json!({
        "store_id": store_id,
        "writes": {
            "tuple_keys": [{
                "user": "user:bob",
                "relation": "viewer",
                "object": "document:secret"
            }]
        }
    });

    grpc_call("openfga.v1.OpenFGAService/Write", &write_request)?;

    // Act: Check via gRPC (should return allowed: true)
    let check_request = json!({
        "store_id": store_id,
        "tuple_key": {
            "user": "user:bob",
            "relation": "viewer",
            "object": "document:secret"
        }
    });

    let check_response = grpc_call("openfga.v1.OpenFGAService/Check", &check_request)?;

    // Assert: Should return allowed: true
    assert_eq!(
        check_response.get("allowed"),
        Some(&json!(true)),
        "Check should return allowed: true for existing tuple"
    );

    // Act: Check for non-existent tuple (should return allowed: false)
    let check_request2 = json!({
        "store_id": store_id,
        "tuple_key": {
            "user": "user:charlie",
            "relation": "viewer",
            "object": "document:secret"
        }
    });

    let check_response2 = grpc_call("openfga.v1.OpenFGAService/Check", &check_request2)?;

    // Assert: Should return allowed: false
    // Note: gRPC/protobuf omits fields with default values, so "allowed": false
    // may appear as missing (empty object) or as explicit false
    // However, if "allowed" is present, it MUST be a boolean
    let allowed = match check_response2.get("allowed") {
        Some(v) => v
            .as_bool()
            .expect("'allowed' field must be a boolean when present"),
        None => false, // Protobuf default: missing field means false
    };

    assert!(
        !allowed,
        "Check should return allowed: false for non-existent tuple, got: {check_response2:?}"
    );

    Ok(())
}
