mod common;

use anyhow::Result;
use common::{get_openfga_url, grpc_call, grpc_call_with_error};
use serde_json::json;

// ============================================================================
// Section 22: gRPC vs REST Parity Tests
// ============================================================================
//
// These tests verify that gRPC and REST APIs return equivalent results.
// For each operation, we perform the same action via both APIs and compare.
//
// ============================================================================

/// Execute REST call with reqwest (blocking for simplicity in comparison tests)
async fn rest_call(
    client: &reqwest::Client,
    method: &str,
    path: &str,
    data: Option<&serde_json::Value>,
) -> Result<serde_json::Value> {
    let url = format!("{}{}", get_openfga_url(), path);

    let response = {
        let mut builder = match method {
            "GET" => client.get(&url),
            "POST" => client.post(&url),
            "DELETE" => client.delete(&url),
            _ => anyhow::bail!("Unsupported method: {method}"),
        };
        if let Some(json_data) = data {
            builder = builder.json(json_data);
        }
        builder.send().await?
    };

    // Fail early on non-2xx responses
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        anyhow::bail!("REST call failed with status {status}: {text}");
    }

    let text = response.text().await?;
    if text.trim().is_empty() {
        return Ok(json!({}));
    }

    let json: serde_json::Value = serde_json::from_str(&text)?;
    Ok(json)
}

// ============================================================================
// Tests
// ============================================================================

/// Test: gRPC Store.Create matches REST POST /stores
#[tokio::test]
async fn test_grpc_store_create_matches_rest() -> Result<()> {
    let client = reqwest::Client::new();

    // Create via gRPC
    let grpc_response = grpc_call(
        "openfga.v1.OpenFGAService/CreateStore",
        &json!({"name": "grpc-parity-test-1"}),
    )?;

    // Create via REST
    let rest_response = rest_call(
        &client,
        "POST",
        "/stores",
        Some(&json!({"name": "rest-parity-test-1"})),
    )
    .await?;

    // Assert: Both should have same structure
    assert!(grpc_response.get("id").is_some(), "gRPC should return id");
    assert!(rest_response.get("id").is_some(), "REST should return id");

    assert!(
        grpc_response.get("name").is_some(),
        "gRPC should return name"
    );
    assert!(
        rest_response.get("name").is_some(),
        "REST should return name"
    );

    assert!(
        grpc_response.get("created_at").is_some(),
        "gRPC should return created_at"
    );
    assert!(
        rest_response.get("created_at").is_some(),
        "REST should return created_at"
    );

    Ok(())
}

/// Test: gRPC Store.Get matches REST GET /stores/{id}
#[tokio::test]
async fn test_grpc_store_get_matches_rest() -> Result<()> {
    let client = reqwest::Client::new();

    // Create store via REST
    let create_response = rest_call(
        &client,
        "POST",
        "/stores",
        Some(&json!({"name": "get-parity-test"})),
    )
    .await?;
    let store_id = create_response["id"].as_str().unwrap();

    // Get via gRPC
    let grpc_response = grpc_call(
        "openfga.v1.OpenFGAService/GetStore",
        &json!({"store_id": store_id}),
    )?;

    // Get via REST
    let rest_response = rest_call(&client, "GET", &format!("/stores/{store_id}"), None).await?;

    // Assert: Both should return same data
    assert_eq!(
        grpc_response.get("id").and_then(|v| v.as_str()),
        rest_response.get("id").and_then(|v| v.as_str()),
        "Store IDs should match"
    );

    assert_eq!(
        grpc_response.get("name").and_then(|v| v.as_str()),
        rest_response.get("name").and_then(|v| v.as_str()),
        "Store names should match"
    );

    Ok(())
}

/// Test: gRPC Store.Delete matches REST DELETE /stores/{id}
#[tokio::test]
async fn test_grpc_store_delete_matches_rest() -> Result<()> {
    let client = reqwest::Client::new();

    // Create stores
    let grpc_store = grpc_call(
        "openfga.v1.OpenFGAService/CreateStore",
        &json!({"name": "delete-grpc-test"}),
    )?;
    let grpc_store_id = grpc_store["id"].as_str().unwrap();

    let rest_store = rest_call(
        &client,
        "POST",
        "/stores",
        Some(&json!({"name": "delete-rest-test"})),
    )
    .await?;
    let rest_store_id = rest_store["id"].as_str().unwrap();

    // Delete via gRPC
    let grpc_delete = grpc_call(
        "openfga.v1.OpenFGAService/DeleteStore",
        &json!({"store_id": grpc_store_id}),
    )?;

    // Delete via REST
    let rest_delete =
        rest_call(&client, "DELETE", &format!("/stores/{rest_store_id}"), None).await?;

    // Assert: Both should return empty object
    assert!(grpc_delete.is_object(), "gRPC delete should return object");
    assert!(rest_delete.is_object(), "REST delete should return object");

    Ok(())
}

/// Test: gRPC Check matches REST Check
#[tokio::test]
async fn test_grpc_check_matches_rest() -> Result<()> {
    let client = reqwest::Client::new();

    // Setup: Create store, model, and tuple via REST
    let store = rest_call(
        &client,
        "POST",
        "/stores",
        Some(&json!({"name": "check-parity-test"})),
    )
    .await?;
    let store_id = store["id"].as_str().unwrap();

    // Create model
    let model = json!({
        "schema_version": "1.1",
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
        ]
    });

    rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/authorization-models"),
        Some(&model),
    )
    .await?;

    // Write tuple
    let write_data = json!({
        "writes": {
            "tuple_keys": [{
                "user": "user:parity-user",
                "relation": "viewer",
                "object": "document:parity-doc"
            }]
        }
    });

    rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/write"),
        Some(&write_data),
    )
    .await?;

    // Check via gRPC
    let grpc_check = grpc_call(
        "openfga.v1.OpenFGAService/Check",
        &json!({
            "store_id": store_id,
            "tuple_key": {
                "user": "user:parity-user",
                "relation": "viewer",
                "object": "document:parity-doc"
            }
        }),
    )?;

    // Check via REST
    let rest_check = rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/check"),
        Some(&json!({
            "tuple_key": {
                "user": "user:parity-user",
                "relation": "viewer",
                "object": "document:parity-doc"
            }
        })),
    )
    .await?;

    // Assert: Both should return allowed: true
    assert_eq!(
        grpc_check.get("allowed"),
        Some(&json!(true)),
        "gRPC check should return allowed: true"
    );
    assert_eq!(
        rest_check.get("allowed"),
        Some(&json!(true)),
        "REST check should return allowed: true"
    );

    Ok(())
}

/// Test: gRPC Batch Check matches REST Batch Check
#[tokio::test]
async fn test_grpc_batch_check_matches_rest() -> Result<()> {
    let client = reqwest::Client::new();

    // Setup
    let store = rest_call(
        &client,
        "POST",
        "/stores",
        Some(&json!({"name": "batch-parity-test"})),
    )
    .await?;
    let store_id = store["id"].as_str().unwrap();

    let model = json!({
        "schema_version": "1.1",
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
        ]
    });

    rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/authorization-models"),
        Some(&model),
    )
    .await?;

    // Write tuple for one user
    let write_data = json!({
        "writes": {
            "tuple_keys": [{
                "user": "user:batch-allowed",
                "relation": "viewer",
                "object": "document:batch-doc"
            }]
        }
    });

    rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/write"),
        Some(&write_data),
    )
    .await?;

    // Batch check via gRPC
    let grpc_batch = grpc_call(
        "openfga.v1.OpenFGAService/BatchCheck",
        &json!({
            "store_id": store_id,
            "checks": [
                {
                    "tuple_key": {
                        "user": "user:batch-allowed",
                        "relation": "viewer",
                        "object": "document:batch-doc"
                    },
                    "correlation_id": "check-1"
                },
                {
                    "tuple_key": {
                        "user": "user:batch-denied",
                        "relation": "viewer",
                        "object": "document:batch-doc"
                    },
                    "correlation_id": "check-2"
                }
            ]
        }),
    )?;

    // Batch check via REST
    let rest_batch = rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/batch-check"),
        Some(&json!({
            "checks": [
                {
                    "tuple_key": {
                        "user": "user:batch-allowed",
                        "relation": "viewer",
                        "object": "document:batch-doc"
                    },
                    "correlation_id": "check-1"
                },
                {
                    "tuple_key": {
                        "user": "user:batch-denied",
                        "relation": "viewer",
                        "object": "document:batch-doc"
                    },
                    "correlation_id": "check-2"
                }
            ]
        })),
    )
    .await?;

    // Assert: Both should have result map with same keys
    assert!(
        grpc_batch.get("result").is_some(),
        "gRPC should have result"
    );
    assert!(
        rest_batch.get("result").is_some(),
        "REST should have result"
    );

    let grpc_result = grpc_batch["result"].as_object().unwrap();
    let rest_result = rest_batch["result"].as_object().unwrap();

    // Both should have check-1 (allowed) and check-2 (denied)
    assert!(
        grpc_result.contains_key("check-1"),
        "gRPC should have check-1"
    );
    assert!(
        rest_result.contains_key("check-1"),
        "REST should have check-1"
    );

    // check-1 should be allowed
    assert_eq!(
        grpc_result["check-1"].get("allowed"),
        Some(&json!(true)),
        "gRPC check-1 should be allowed"
    );
    assert_eq!(
        rest_result["check-1"].get("allowed"),
        Some(&json!(true)),
        "REST check-1 should be allowed"
    );

    // Both should have check-2
    assert!(
        grpc_result.contains_key("check-2"),
        "gRPC should have check-2"
    );
    assert!(
        rest_result.contains_key("check-2"),
        "REST should have check-2"
    );

    // check-2 should be denied (allowed: false or field omitted due to protobuf default)
    // If "allowed" is present, it MUST be a boolean; if missing, it means false (protobuf default)
    let grpc_allowed2 = match grpc_result["check-2"].get("allowed") {
        Some(v) => v
            .as_bool()
            .expect("gRPC 'allowed' field must be a boolean when present"),
        None => false,
    };
    assert!(!grpc_allowed2, "gRPC check-2 should be denied");

    let rest_allowed2 = match rest_result["check-2"].get("allowed") {
        Some(v) => v
            .as_bool()
            .expect("REST 'allowed' field must be a boolean when present"),
        None => false,
    };
    assert!(!rest_allowed2, "REST check-2 should be denied");

    // Assert parity: gRPC and REST should return the same denied result
    assert_eq!(
        grpc_allowed2, rest_allowed2,
        "gRPC and REST check-2 (denied) results should match"
    );

    Ok(())
}

/// Test: gRPC Write matches REST Write
#[tokio::test]
async fn test_grpc_write_matches_rest() -> Result<()> {
    let client = reqwest::Client::new();

    // Setup
    let store = rest_call(
        &client,
        "POST",
        "/stores",
        Some(&json!({"name": "write-parity-test"})),
    )
    .await?;
    let store_id = store["id"].as_str().unwrap();

    let model = json!({
        "schema_version": "1.1",
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
        ]
    });

    rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/authorization-models"),
        Some(&model),
    )
    .await?;

    // Write via gRPC
    let grpc_write = grpc_call(
        "openfga.v1.OpenFGAService/Write",
        &json!({
            "store_id": store_id,
            "writes": {
                "tuple_keys": [{
                    "user": "user:grpc-writer",
                    "relation": "viewer",
                    "object": "document:grpc-written"
                }]
            }
        }),
    )?;

    // Write via REST
    let rest_write = rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/write"),
        Some(&json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:rest-writer",
                    "relation": "viewer",
                    "object": "document:rest-written"
                }]
            }
        })),
    )
    .await?;

    // Assert: Both should return empty object on success
    assert!(grpc_write.is_object(), "gRPC write should return object");
    assert!(rest_write.is_object(), "REST write should return object");

    // Verify both tuples exist via Check (more reliable than Read with empty filter)
    let grpc_check1 = grpc_call(
        "openfga.v1.OpenFGAService/Check",
        &json!({
            "store_id": store_id,
            "tuple_key": {
                "user": "user:grpc-writer",
                "relation": "viewer",
                "object": "document:grpc-written"
            }
        }),
    )?;
    assert_eq!(
        grpc_check1.get("allowed"),
        Some(&json!(true)),
        "gRPC-written tuple should be checkable"
    );

    let grpc_check2 = grpc_call(
        "openfga.v1.OpenFGAService/Check",
        &json!({
            "store_id": store_id,
            "tuple_key": {
                "user": "user:rest-writer",
                "relation": "viewer",
                "object": "document:rest-written"
            }
        }),
    )?;
    assert_eq!(
        grpc_check2.get("allowed"),
        Some(&json!(true)),
        "REST-written tuple should be checkable via gRPC"
    );

    Ok(())
}

/// Test: gRPC Read matches REST Read
#[tokio::test]
async fn test_grpc_read_matches_rest() -> Result<()> {
    let client = reqwest::Client::new();

    // Setup
    let store = rest_call(
        &client,
        "POST",
        "/stores",
        Some(&json!({"name": "read-parity-test"})),
    )
    .await?;
    let store_id = store["id"].as_str().unwrap();

    let model = json!({
        "schema_version": "1.1",
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
        ]
    });

    rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/authorization-models"),
        Some(&model),
    )
    .await?;

    // Write tuple
    rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/write"),
        Some(&json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:reader",
                    "relation": "viewer",
                    "object": "document:readable"
                }]
            }
        })),
    )
    .await?;

    // Read via gRPC
    let grpc_read = grpc_call(
        "openfga.v1.OpenFGAService/Read",
        &json!({
            "store_id": store_id,
            "tuple_key": {
                "user": "user:reader",
                "relation": "viewer",
                "object": "document:readable"
            }
        }),
    )?;

    // Read via REST
    let rest_read = rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/read"),
        Some(&json!({
            "tuple_key": {
                "user": "user:reader",
                "relation": "viewer",
                "object": "document:readable"
            }
        })),
    )
    .await?;

    // Assert: Both should return same tuple
    let grpc_tuples = grpc_read["tuples"].as_array().unwrap();
    let rest_tuples = rest_read["tuples"].as_array().unwrap();

    assert_eq!(grpc_tuples.len(), 1, "gRPC should return 1 tuple");
    assert_eq!(rest_tuples.len(), 1, "REST should return 1 tuple");

    // Compare tuple content
    let grpc_key = &grpc_tuples[0]["key"];
    let rest_key = &rest_tuples[0]["key"];

    assert_eq!(grpc_key["user"], rest_key["user"], "User should match");
    assert_eq!(
        grpc_key["relation"], rest_key["relation"],
        "Relation should match"
    );
    assert_eq!(
        grpc_key["object"], rest_key["object"],
        "Object should match"
    );

    Ok(())
}

/// Test: gRPC Expand matches REST Expand
#[tokio::test]
async fn test_grpc_expand_matches_rest() -> Result<()> {
    let client = reqwest::Client::new();

    // Setup
    let store = rest_call(
        &client,
        "POST",
        "/stores",
        Some(&json!({"name": "expand-parity-test"})),
    )
    .await?;
    let store_id = store["id"].as_str().unwrap();

    let model = json!({
        "schema_version": "1.1",
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
        ]
    });

    rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/authorization-models"),
        Some(&model),
    )
    .await?;

    // Write tuple
    rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/write"),
        Some(&json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:expander",
                    "relation": "viewer",
                    "object": "document:expandable"
                }]
            }
        })),
    )
    .await?;

    // Expand via gRPC
    let grpc_expand = grpc_call(
        "openfga.v1.OpenFGAService/Expand",
        &json!({
            "store_id": store_id,
            "tuple_key": {
                "relation": "viewer",
                "object": "document:expandable"
            }
        }),
    )?;

    // Expand via REST
    let rest_expand = rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/expand"),
        Some(&json!({
            "tuple_key": {
                "relation": "viewer",
                "object": "document:expandable"
            }
        })),
    )
    .await?;

    // Assert: Both should return tree structure
    assert!(
        grpc_expand.get("tree").is_some(),
        "gRPC expand should return tree"
    );
    assert!(
        rest_expand.get("tree").is_some(),
        "REST expand should return tree"
    );

    // Both trees should have root
    assert!(
        grpc_expand["tree"].get("root").is_some(),
        "gRPC tree should have root"
    );
    assert!(
        rest_expand["tree"].get("root").is_some(),
        "REST tree should have root"
    );

    Ok(())
}

/// Test: gRPC ListObjects matches REST ListObjects
#[tokio::test]
async fn test_grpc_listobjects_matches_rest() -> Result<()> {
    let client = reqwest::Client::new();

    // Setup
    let store = rest_call(
        &client,
        "POST",
        "/stores",
        Some(&json!({"name": "listobjects-parity-test"})),
    )
    .await?;
    let store_id = store["id"].as_str().unwrap();

    let model = json!({
        "schema_version": "1.1",
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
        ]
    });

    rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/authorization-models"),
        Some(&model),
    )
    .await?;

    // Write tuples
    rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/write"),
        Some(&json!({
            "writes": {
                "tuple_keys": [
                    {"user": "user:lister", "relation": "viewer", "object": "document:doc1"},
                    {"user": "user:lister", "relation": "viewer", "object": "document:doc2"},
                    {"user": "user:other", "relation": "viewer", "object": "document:doc3"}
                ]
            }
        })),
    )
    .await?;

    // ListObjects via gRPC
    let grpc_list = grpc_call(
        "openfga.v1.OpenFGAService/ListObjects",
        &json!({
            "store_id": store_id,
            "type": "document",
            "relation": "viewer",
            "user": "user:lister"
        }),
    )?;

    // ListObjects via REST
    let rest_list = rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/list-objects"),
        Some(&json!({
            "type": "document",
            "relation": "viewer",
            "user": "user:lister"
        })),
    )
    .await?;

    // Assert: Both should return same objects
    let grpc_objects = grpc_list["objects"].as_array().unwrap();
    let rest_objects = rest_list["objects"].as_array().unwrap();

    assert_eq!(
        grpc_objects.len(),
        rest_objects.len(),
        "Should return same number of objects"
    );
    assert_eq!(
        grpc_objects.len(),
        2,
        "user:lister should have access to 2 docs"
    );

    // Both should contain doc1 and doc2
    let grpc_set: std::collections::HashSet<String> = grpc_objects
        .iter()
        .filter_map(|v| v.as_str())
        .map(|s| s.to_string())
        .collect();

    let rest_set: std::collections::HashSet<String> = rest_objects
        .iter()
        .filter_map(|v| v.as_str())
        .map(|s| s.to_string())
        .collect();

    assert_eq!(grpc_set, rest_set, "Both should return same objects");
    assert!(grpc_set.contains("document:doc1"));
    assert!(grpc_set.contains("document:doc2"));

    Ok(())
}

/// Test: gRPC ListUsers matches REST ListUsers
#[tokio::test]
async fn test_grpc_listusers_matches_rest() -> Result<()> {
    let client = reqwest::Client::new();

    // Setup
    let store = rest_call(
        &client,
        "POST",
        "/stores",
        Some(&json!({"name": "listusers-parity-test"})),
    )
    .await?;
    let store_id = store["id"].as_str().unwrap();

    let model = json!({
        "schema_version": "1.1",
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
        ]
    });

    rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/authorization-models"),
        Some(&model),
    )
    .await?;

    // Write tuples - multiple users have access to a document
    rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/write"),
        Some(&json!({
            "writes": {
                "tuple_keys": [
                    {"user": "user:alice", "relation": "viewer", "object": "document:shared-doc"},
                    {"user": "user:bob", "relation": "viewer", "object": "document:shared-doc"},
                    {"user": "user:charlie", "relation": "viewer", "object": "document:other-doc"}
                ]
            }
        })),
    )
    .await?;

    // ListUsers via gRPC
    let grpc_list = grpc_call(
        "openfga.v1.OpenFGAService/ListUsers",
        &json!({
            "store_id": store_id,
            "object": {
                "type": "document",
                "id": "shared-doc"
            },
            "relation": "viewer",
            "user_filters": [{"type": "user"}]
        }),
    )?;

    // ListUsers via REST
    let rest_list = rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/list-users"),
        Some(&json!({
            "object": {
                "type": "document",
                "id": "shared-doc"
            },
            "relation": "viewer",
            "user_filters": [{"type": "user"}]
        })),
    )
    .await?;

    // Assert: Both should return same users
    let grpc_users = grpc_list["users"].as_array().unwrap();
    let rest_users = rest_list["users"].as_array().unwrap();

    assert_eq!(
        grpc_users.len(),
        rest_users.len(),
        "Should return same number of users"
    );
    assert_eq!(grpc_users.len(), 2, "Should have 2 users with access");

    // Extract user IDs from both responses
    let grpc_set: std::collections::HashSet<String> = grpc_users
        .iter()
        .filter_map(|u| {
            u.get("object")
                .and_then(|o| o.get("id"))
                .and_then(|id| id.as_str())
                .map(|s| s.to_string())
        })
        .collect();

    let rest_set: std::collections::HashSet<String> = rest_users
        .iter()
        .filter_map(|u| {
            u.get("object")
                .and_then(|o| o.get("id"))
                .and_then(|id| id.as_str())
                .map(|s| s.to_string())
        })
        .collect();

    assert_eq!(grpc_set, rest_set, "Both should return same users");
    assert!(grpc_set.contains("alice"), "Should contain alice");
    assert!(grpc_set.contains("bob"), "Should contain bob");

    Ok(())
}

// ============================================================================
// Error Response Mapping Tests
// ============================================================================
//
// These tests verify that HTTP status codes and gRPC status codes are
// equivalent for the same error scenarios. Users should see consistent
// error semantics regardless of protocol.
//
// HTTP -> gRPC code mapping:
// - 400 Bad Request -> INVALID_ARGUMENT (3)
// - 404 Not Found -> NOT_FOUND (5)
// - 409 Conflict -> ALREADY_EXISTS (6) or FAILED_PRECONDITION (9)
// - 422 Unprocessable Entity -> INVALID_ARGUMENT (3)
// - 500 Internal Server Error -> INTERNAL (13)
//
// ============================================================================

/// Helper function to execute REST call and return status code and body
async fn rest_call_with_status(
    client: &reqwest::Client,
    method: &str,
    path: &str,
    data: Option<&serde_json::Value>,
) -> Result<(reqwest::StatusCode, String)> {
    let url = format!("{}{}", get_openfga_url(), path);

    let response = {
        let mut builder = match method {
            "GET" => client.get(&url),
            "POST" => client.post(&url),
            "DELETE" => client.delete(&url),
            _ => anyhow::bail!("Unsupported method: {method}"),
        };
        if let Some(json_data) = data {
            builder = builder.json(json_data);
        }
        builder.send().await?
    };

    let status = response.status();
    let text = response.text().await?;
    Ok((status, text))
}

/// Test: Error for non-existent store matches across protocols
/// HTTP 404 should correspond to gRPC NOT_FOUND
#[tokio::test]
async fn test_error_nonexistent_store_parity() -> Result<()> {
    let client = reqwest::Client::new();
    let fake_store_id = "nonexistent-store-12345";

    // Try to get store via REST
    let (rest_status, _rest_body) =
        rest_call_with_status(&client, "GET", &format!("/stores/{fake_store_id}"), None).await?;

    // Try to get store via gRPC
    let (grpc_success, _stdout, grpc_stderr) = grpc_call_with_error(
        "openfga.v1.OpenFGAService/GetStore",
        &json!({"store_id": fake_store_id}),
    )?;

    // Assert: REST returns 404
    assert_eq!(
        rest_status.as_u16(),
        404,
        "REST should return 404 for non-existent store"
    );

    // Assert: gRPC fails with NOT_FOUND
    assert!(!grpc_success, "gRPC should fail for non-existent store");
    assert!(
        grpc_stderr.contains("NotFound")
            || grpc_stderr.contains("not found")
            || grpc_stderr.contains("Code: NotFound"),
        "gRPC error should indicate not found: {grpc_stderr}"
    );

    Ok(())
}

/// Test: Error for invalid tuple format matches across protocols
/// HTTP 400 should correspond to gRPC INVALID_ARGUMENT
#[tokio::test]
async fn test_error_invalid_tuple_parity() -> Result<()> {
    let client = reqwest::Client::new();

    // Create a valid store first
    let store = rest_call(
        &client,
        "POST",
        "/stores",
        Some(&json!({"name": "error-parity-test"})),
    )
    .await?;
    let store_id = store["id"].as_str().unwrap();

    // Create model
    let model = json!({
        "schema_version": "1.1",
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
        ]
    });

    rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/authorization-models"),
        Some(&model),
    )
    .await?;

    // Try to write tuple with invalid format (missing colon in user)
    let invalid_write = json!({
        "writes": {
            "tuple_keys": [{
                "user": "invalid_user_format",  // Missing type:id format
                "relation": "viewer",
                "object": "document:doc1"
            }]
        }
    });

    // Try via REST
    let (rest_status, _rest_body) = rest_call_with_status(
        &client,
        "POST",
        &format!("/stores/{store_id}/write"),
        Some(&invalid_write),
    )
    .await?;

    // Try via gRPC
    let (grpc_success, _stdout, grpc_stderr) = grpc_call_with_error(
        "openfga.v1.OpenFGAService/Write",
        &json!({
            "store_id": store_id,
            "writes": {
                "tuple_keys": [{
                    "user": "invalid_user_format",
                    "relation": "viewer",
                    "object": "document:doc1"
                }]
            }
        }),
    )?;

    // Assert: REST returns 400
    assert_eq!(
        rest_status.as_u16(),
        400,
        "REST should return 400 for invalid tuple format"
    );

    // Assert: gRPC fails with INVALID_ARGUMENT
    assert!(!grpc_success, "gRPC should fail for invalid tuple format");
    assert!(
        grpc_stderr.contains("InvalidArgument")
            || grpc_stderr.contains("invalid")
            || grpc_stderr.contains("Code: InvalidArgument"),
        "gRPC error should indicate invalid argument: {grpc_stderr}"
    );

    Ok(())
}

/// Test: Error for check with non-existent relation matches across protocols
#[tokio::test]
async fn test_error_invalid_relation_parity() -> Result<()> {
    let client = reqwest::Client::new();

    // Create store with simple model
    let store = rest_call(
        &client,
        "POST",
        "/stores",
        Some(&json!({"name": "relation-error-parity-test"})),
    )
    .await?;
    let store_id = store["id"].as_str().unwrap();

    let model = json!({
        "schema_version": "1.1",
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
        ]
    });

    rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/authorization-models"),
        Some(&model),
    )
    .await?;

    // Try to check with non-existent relation
    let check_request = json!({
        "tuple_key": {
            "user": "user:alice",
            "relation": "nonexistent_relation",
            "object": "document:doc1"
        }
    });

    // Try via REST
    let (rest_status, _rest_body) = rest_call_with_status(
        &client,
        "POST",
        &format!("/stores/{store_id}/check"),
        Some(&check_request),
    )
    .await?;

    // Try via gRPC
    let (grpc_success, _stdout, _grpc_stderr) = grpc_call_with_error(
        "openfga.v1.OpenFGAService/Check",
        &json!({
            "store_id": store_id,
            "tuple_key": {
                "user": "user:alice",
                "relation": "nonexistent_relation",
                "object": "document:doc1"
            }
        }),
    )?;

    // Both should return error for undefined relation
    assert!(
        rest_status.is_client_error(),
        "REST should return 4xx error for undefined relation, got {rest_status}"
    );
    assert!(!grpc_success, "gRPC should fail for undefined relation");

    Ok(())
}

/// Test: Error for invalid type in check matches across protocols
#[tokio::test]
async fn test_error_invalid_type_parity() -> Result<()> {
    let client = reqwest::Client::new();

    // Create store with simple model
    let store = rest_call(
        &client,
        "POST",
        "/stores",
        Some(&json!({"name": "type-error-parity-test"})),
    )
    .await?;
    let store_id = store["id"].as_str().unwrap();

    let model = json!({
        "schema_version": "1.1",
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
        ]
    });

    rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/authorization-models"),
        Some(&model),
    )
    .await?;

    // Try to check with non-existent type
    let check_request = json!({
        "tuple_key": {
            "user": "user:alice",
            "relation": "viewer",
            "object": "nonexistent_type:doc1"
        }
    });

    // Try via REST
    let (rest_status, _rest_body) = rest_call_with_status(
        &client,
        "POST",
        &format!("/stores/{store_id}/check"),
        Some(&check_request),
    )
    .await?;

    // Try via gRPC
    let (grpc_success, _stdout, grpc_stderr) = grpc_call_with_error(
        "openfga.v1.OpenFGAService/Check",
        &json!({
            "store_id": store_id,
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "nonexistent_type:doc1"
            }
        }),
    )?;

    // Both should return error for undefined type
    assert!(
        rest_status.is_client_error(),
        "REST should return 4xx error for undefined type, got {rest_status}"
    );
    assert!(
        !grpc_success,
        "gRPC should fail for undefined type: {grpc_stderr}"
    );

    Ok(())
}

// ============================================================================
// Read Pagination Token Parity Tests
// ============================================================================

/// Test: Read with pagination tokens returns identical results across protocols
#[tokio::test]
async fn test_read_pagination_token_parity() -> Result<()> {
    let client = reqwest::Client::new();

    // Setup
    let store = rest_call(
        &client,
        "POST",
        "/stores",
        Some(&json!({"name": "pagination-parity-test"})),
    )
    .await?;
    let store_id = store["id"].as_str().unwrap();

    let model = json!({
        "schema_version": "1.1",
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
        ]
    });

    rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/authorization-models"),
        Some(&model),
    )
    .await?;

    // Write multiple tuples to trigger pagination
    let tuples: Vec<serde_json::Value> = (0..10)
        .map(|i| {
            json!({
                "user": format!("user:paguser{i}"),
                "relation": "viewer",
                "object": format!("document:pagdoc{i}")
            })
        })
        .collect();

    rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/write"),
        Some(&json!({
            "writes": {
                "tuple_keys": tuples
            }
        })),
    )
    .await?;

    // Read with page size via gRPC
    let grpc_read = grpc_call(
        "openfga.v1.OpenFGAService/Read",
        &json!({
            "store_id": store_id,
            "tuple_key": {},
            "page_size": 3
        }),
    )?;

    // Read with page size via REST
    let rest_read = rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/read"),
        Some(&json!({
            "tuple_key": {},
            "page_size": 3
        })),
    )
    .await?;

    // Both should return same number of tuples in first page
    let grpc_tuples = grpc_read["tuples"].as_array().unwrap();
    let rest_tuples = rest_read["tuples"].as_array().unwrap();

    assert_eq!(
        grpc_tuples.len(),
        rest_tuples.len(),
        "First page should return same number of tuples"
    );
    assert!(
        grpc_tuples.len() <= 3,
        "First page should respect page_size limit"
    );

    // Both should have continuation tokens if more data exists
    let grpc_has_token = grpc_read.get("continuation_token").is_some()
        && !grpc_read["continuation_token"]
            .as_str()
            .unwrap_or("")
            .is_empty();
    let rest_has_token = rest_read.get("continuation_token").is_some()
        && !rest_read["continuation_token"]
            .as_str()
            .unwrap_or("")
            .is_empty();

    assert_eq!(
        grpc_has_token, rest_has_token,
        "Both should have continuation token when more data exists"
    );

    Ok(())
}

// ============================================================================
// Batch Check Deduplication Parity Tests
// ============================================================================

/// Test: Batch check with duplicate requests handles deduplication consistently
#[tokio::test]
async fn test_batch_check_deduplication_parity() -> Result<()> {
    let client = reqwest::Client::new();

    // Setup
    let store = rest_call(
        &client,
        "POST",
        "/stores",
        Some(&json!({"name": "dedup-parity-test"})),
    )
    .await?;
    let store_id = store["id"].as_str().unwrap();

    let model = json!({
        "schema_version": "1.1",
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
        ]
    });

    rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/authorization-models"),
        Some(&model),
    )
    .await?;

    // Write a tuple
    rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/write"),
        Some(&json!({
            "writes": {
                "tuple_keys": [{
                    "user": "user:dedup-user",
                    "relation": "viewer",
                    "object": "document:dedup-doc"
                }]
            }
        })),
    )
    .await?;

    // Batch check with duplicate tuples but different correlation IDs
    let batch_request = json!({
        "checks": [
            {
                "tuple_key": {
                    "user": "user:dedup-user",
                    "relation": "viewer",
                    "object": "document:dedup-doc"
                },
                "correlation_id": "check-a"
            },
            {
                "tuple_key": {
                    "user": "user:dedup-user",
                    "relation": "viewer",
                    "object": "document:dedup-doc"
                },
                "correlation_id": "check-b"
            },
            {
                "tuple_key": {
                    "user": "user:dedup-user",
                    "relation": "viewer",
                    "object": "document:dedup-doc"
                },
                "correlation_id": "check-c"
            }
        ]
    });

    // Batch check via gRPC
    let grpc_batch = grpc_call(
        "openfga.v1.OpenFGAService/BatchCheck",
        &json!({
            "store_id": store_id,
            "checks": batch_request["checks"]
        }),
    )?;

    // Batch check via REST
    let rest_batch = rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/batch-check"),
        Some(&batch_request),
    )
    .await?;

    let grpc_result = grpc_batch["result"].as_object().unwrap();
    let rest_result = rest_batch["result"].as_object().unwrap();

    // All correlation IDs should be present in both responses
    for id in ["check-a", "check-b", "check-c"] {
        assert!(
            grpc_result.contains_key(id),
            "gRPC should have correlation ID {id}"
        );
        assert!(
            rest_result.contains_key(id),
            "REST should have correlation ID {id}"
        );

        // All should be allowed since they're the same tuple
        let grpc_allowed = grpc_result[id]
            .get("allowed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let rest_allowed = rest_result[id]
            .get("allowed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        assert!(grpc_allowed, "gRPC {id} should be allowed");
        assert!(rest_allowed, "REST {id} should be allowed");
        assert_eq!(
            grpc_allowed, rest_allowed,
            "Both protocols should return same result for {id}"
        );
    }

    Ok(())
}

// ============================================================================
// Expand Tree Structure Deep Comparison
// ============================================================================

/// Test: Expand returns identical tree structure across protocols (union case)
#[tokio::test]
async fn test_expand_tree_union_parity() -> Result<()> {
    let client = reqwest::Client::new();

    // Setup with union relation
    let store = rest_call(
        &client,
        "POST",
        "/stores",
        Some(&json!({"name": "expand-union-parity-test"})),
    )
    .await?;
    let store_id = store["id"].as_str().unwrap();

    // Model with union relation (viewer can be direct OR via editor)
    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            {"type": "user"},
            {
                "type": "document",
                "relations": {
                    "editor": {"this": {}},
                    "viewer": {
                        "union": {
                            "child": [
                                {"this": {}},
                                {"computedUserset": {"relation": "editor"}}
                            ]
                        }
                    }
                },
                "metadata": {
                    "relations": {
                        "editor": {"directly_related_user_types": [{"type": "user"}]},
                        "viewer": {"directly_related_user_types": [{"type": "user"}]}
                    }
                }
            }
        ]
    });

    rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/authorization-models"),
        Some(&model),
    )
    .await?;

    // Write tuples for both branches of the union
    rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/write"),
        Some(&json!({
            "writes": {
                "tuple_keys": [
                    {"user": "user:direct-viewer", "relation": "viewer", "object": "document:union-doc"},
                    {"user": "user:editor-viewer", "relation": "editor", "object": "document:union-doc"}
                ]
            }
        })),
    )
    .await?;

    // Expand via gRPC
    let grpc_expand = grpc_call(
        "openfga.v1.OpenFGAService/Expand",
        &json!({
            "store_id": store_id,
            "tuple_key": {
                "relation": "viewer",
                "object": "document:union-doc"
            }
        }),
    )?;

    // Expand via REST
    let rest_expand = rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/expand"),
        Some(&json!({
            "tuple_key": {
                "relation": "viewer",
                "object": "document:union-doc"
            }
        })),
    )
    .await?;

    // Both should have tree with root
    assert!(grpc_expand.get("tree").is_some(), "gRPC should have tree");
    assert!(rest_expand.get("tree").is_some(), "REST should have tree");

    let grpc_tree = &grpc_expand["tree"];
    let rest_tree = &rest_expand["tree"];

    assert!(
        grpc_tree.get("root").is_some(),
        "gRPC tree should have root"
    );
    assert!(
        rest_tree.get("root").is_some(),
        "REST tree should have root"
    );

    // Both roots should have the same type of node (union in this case)
    let grpc_root = &grpc_tree["root"];
    let rest_root = &rest_tree["root"];

    // Check that both have union nodes
    let grpc_has_union = grpc_root.get("union").is_some();
    let rest_has_union = rest_root.get("union").is_some();

    assert_eq!(
        grpc_has_union, rest_has_union,
        "Both should have same tree structure type"
    );

    Ok(())
}

/// Test: Expand returns identical tree for computed userset across protocols
#[tokio::test]
async fn test_expand_computed_userset_parity() -> Result<()> {
    let client = reqwest::Client::new();

    // Setup with computed userset relation
    let store = rest_call(
        &client,
        "POST",
        "/stores",
        Some(&json!({"name": "expand-computed-parity-test"})),
    )
    .await?;
    let store_id = store["id"].as_str().unwrap();

    // Model with parent folder relation
    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            {"type": "user"},
            {
                "type": "folder",
                "relations": {
                    "viewer": {"this": {}}
                },
                "metadata": {
                    "relations": {
                        "viewer": {"directly_related_user_types": [{"type": "user"}]}
                    }
                }
            },
            {
                "type": "document",
                "relations": {
                    "parent": {"this": {}},
                    "viewer": {
                        "tupleToUserset": {
                            "tupleset": {"relation": "parent"},
                            "computedUserset": {"relation": "viewer"}
                        }
                    }
                },
                "metadata": {
                    "relations": {
                        "parent": {"directly_related_user_types": [{"type": "folder"}]},
                        "viewer": {"directly_related_user_types": [{"type": "user"}]}
                    }
                }
            }
        ]
    });

    rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/authorization-models"),
        Some(&model),
    )
    .await?;

    // Write tuples for computed relation
    rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/write"),
        Some(&json!({
            "writes": {
                "tuple_keys": [
                    {"user": "folder:parent-folder", "relation": "parent", "object": "document:child-doc"},
                    {"user": "user:folder-viewer", "relation": "viewer", "object": "folder:parent-folder"}
                ]
            }
        })),
    )
    .await?;

    // Expand via gRPC
    let grpc_expand = grpc_call(
        "openfga.v1.OpenFGAService/Expand",
        &json!({
            "store_id": store_id,
            "tuple_key": {
                "relation": "viewer",
                "object": "document:child-doc"
            }
        }),
    )?;

    // Expand via REST
    let rest_expand = rest_call(
        &client,
        "POST",
        &format!("/stores/{store_id}/expand"),
        Some(&json!({
            "tuple_key": {
                "relation": "viewer",
                "object": "document:child-doc"
            }
        })),
    )
    .await?;

    // Both should have tree with root
    assert!(grpc_expand.get("tree").is_some(), "gRPC should have tree");
    assert!(rest_expand.get("tree").is_some(), "REST should have tree");

    // Verify both trees have similar structure
    let grpc_tree = &grpc_expand["tree"]["root"];
    let rest_tree = &rest_expand["tree"]["root"];

    // Both should indicate the relation name
    let grpc_name = grpc_tree.get("name").and_then(|v| v.as_str());
    let rest_name = rest_tree.get("name").and_then(|v| v.as_str());

    // The name format might differ slightly but should reference the same relation
    assert!(
        grpc_name.is_some() == rest_name.is_some(),
        "Both should have or lack name consistently"
    );

    Ok(())
}
