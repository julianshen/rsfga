mod common;

use anyhow::Result;
use common::{create_test_store, get_openfga_url, shared_client};
use serde_json::json;

// ============================================================================
// Section 35: Modular Models (Schema 1.2) Tests
// ============================================================================
//
// Schema version 1.2 introduces modular models that allow splitting
// authorization models across multiple files with module declarations
// and type extensions.
//
// Features:
// - `module` keyword for declaring modules
// - `extend type` for extending existing types with additional relations
// - Schema version 1.2
//
// Note: Modular models require the experimental flag:
//   --experimentals enable-modular-models
//
// The JSON API format for modular models may differ from single-file models.
//
// ============================================================================

/// Helper to check if Schema 1.2 / Modular Models is supported
async fn is_schema_1_2_supported(store_id: &str) -> bool {
    let client = shared_client();

    // Try to create a model with schema 1.2
    let model = json!({
        "schema_version": "1.2",
        "type_definitions": [
            { "type": "user" }
        ]
    });

    let response = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model)
        .send()
        .await;

    match response {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

/// Test: Create model with schema version 1.2
#[tokio::test]
async fn test_create_model_schema_1_2() -> Result<()> {
    let store_id = create_test_store().await?;

    if !is_schema_1_2_supported(&store_id).await {
        eprintln!(
            "SKIPPED: Schema 1.2 not supported (requires --experimentals enable-modular-models)"
        );
        return Ok(());
    }

    let client = shared_client();

    // Act: Create model with schema 1.2
    let model = json!({
        "schema_version": "1.2",
        "type_definitions": [
            { "type": "user" },
            {
                "type": "document",
                "relations": {
                    "viewer": { "this": {} }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [{ "type": "user" }]
                        }
                    }
                }
            }
        ]
    });

    let response = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model)
        .send()
        .await?;

    // Assert
    assert!(
        response.status().is_success(),
        "Creating model with schema 1.2 should succeed, got: {}",
        response.status()
    );

    let body: serde_json::Value = response.json().await?;
    assert!(
        body.get("authorization_model_id").is_some(),
        "Response should have authorization_model_id"
    );

    Ok(())
}

/// Test: Schema 1.2 model with organization type
///
/// Note: This test validates schema 1.2 support with an organization type.
/// True modular model features (module keyword, extend type) require DSL format
/// and are not directly testable via JSON API. This test documents current
/// JSON API behavior for schema 1.2.
#[tokio::test]
async fn test_schema_1_2_with_organization_type() -> Result<()> {
    let store_id = create_test_store().await?;

    if !is_schema_1_2_supported(&store_id).await {
        eprintln!(
            "SKIPPED: Schema 1.2 not supported (requires --experimentals enable-modular-models)"
        );
        return Ok(());
    }

    let client = shared_client();

    // Schema 1.2 model with organization type
    // Note: True modular features (module declarations, extend type) are DSL-only
    // The JSON API accepts schema 1.2 but module-specific syntax isn't available
    let model = json!({
        "schema_version": "1.2",
        "type_definitions": [
            { "type": "user" },
            {
                "type": "organization",
                "relations": {
                    "member": { "this": {} }
                },
                "metadata": {
                    "relations": {
                        "member": {
                            "directly_related_user_types": [{ "type": "user" }]
                        }
                    }
                }
            }
        ]
    });

    let response = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model)
        .send()
        .await?;

    // Assert: Model creation should succeed with schema 1.2
    assert!(
        response.status().is_success(),
        "Schema 1.2 model with organization type should succeed, got: {}",
        response.status()
    );

    let body: serde_json::Value = response.json().await?;
    assert!(
        body.get("authorization_model_id").is_some(),
        "Response should have authorization_model_id"
    );

    Ok(())
}

/// Test: Schema 1.2 backwards compatible with 1.1 models
#[tokio::test]
async fn test_schema_1_2_backwards_compatible() -> Result<()> {
    let store_id = create_test_store().await?;

    let client = shared_client();

    // Create a 1.1 model first
    let model_1_1 = json!({
        "schema_version": "1.1",
        "type_definitions": [
            { "type": "user" },
            {
                "type": "document",
                "relations": {
                    "viewer": { "this": {} }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [{ "type": "user" }]
                        }
                    }
                }
            }
        ]
    });

    let response_1_1 = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model_1_1)
        .send()
        .await?;

    assert!(
        response_1_1.status().is_success(),
        "Creating 1.1 model should succeed"
    );

    // If 1.2 is supported, create a 1.2 model in the same store
    if is_schema_1_2_supported(&store_id).await {
        let model_1_2 = json!({
            "schema_version": "1.2",
            "type_definitions": [
                { "type": "user" },
                {
                    "type": "folder",
                    "relations": {
                        "viewer": { "this": {} }
                    },
                    "metadata": {
                        "relations": {
                            "viewer": {
                                "directly_related_user_types": [{ "type": "user" }]
                            }
                        }
                    }
                }
            ]
        });

        let response_1_2 = client
            .post(format!(
                "{}/stores/{}/authorization-models",
                get_openfga_url(),
                store_id
            ))
            .json(&model_1_2)
            .send()
            .await?;

        // Both versions should work in same store
        assert!(
            response_1_2.status().is_success(),
            "Creating 1.2 model after 1.1 should succeed"
        );

        // List models should show both
        let list_response = client
            .get(format!(
                "{}/stores/{}/authorization-models",
                get_openfga_url(),
                store_id
            ))
            .send()
            .await?;

        let list_body: serde_json::Value = list_response.json().await?;
        let models = list_body
            .get("authorization_models")
            .and_then(|m| m.as_array());

        if let Some(models) = models {
            assert!(
                models.len() >= 2,
                "Should have at least 2 models (1.1 and 1.2)"
            );
        }
    }

    Ok(())
}

/// Test: Invalid schema version returns error
#[tokio::test]
async fn test_invalid_schema_version_error() -> Result<()> {
    let store_id = create_test_store().await?;

    let client = shared_client();

    // Act: Try to create model with invalid schema version
    let model = json!({
        "schema_version": "2.0",  // Invalid - doesn't exist
        "type_definitions": [
            { "type": "user" }
        ]
    });

    let response = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model)
        .send()
        .await?;

    // Assert: Should return error
    assert!(
        !response.status().is_success(),
        "Creating model with invalid schema version should fail"
    );

    Ok(())
}

/// Test: Model retrieval includes schema version
#[tokio::test]
async fn test_model_retrieval_includes_schema_version() -> Result<()> {
    let store_id = create_test_store().await?;

    let client = shared_client();

    // Create a model with explicit schema version
    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            { "type": "user" },
            {
                "type": "document",
                "relations": {
                    "viewer": { "this": {} }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [{ "type": "user" }]
                        }
                    }
                }
            }
        ]
    });

    let create_response = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model)
        .send()
        .await?;

    let create_body: serde_json::Value = create_response.json().await?;
    let model_id = create_body
        .get("authorization_model_id")
        .and_then(|m| m.as_str())
        .expect("Should have model ID");

    // Retrieve the model
    let get_response = client
        .get(format!(
            "{}/stores/{}/authorization-models/{}",
            get_openfga_url(),
            store_id,
            model_id
        ))
        .send()
        .await?;

    assert!(get_response.status().is_success());

    let get_body: serde_json::Value = get_response.json().await?;

    // Check schema version is included in response
    let retrieved_model = get_body
        .get("authorization_model")
        .expect("Should have authorization_model");

    let schema_version = retrieved_model
        .get("schema_version")
        .and_then(|s| s.as_str());

    assert_eq!(
        schema_version,
        Some("1.1"),
        "Retrieved model should include schema_version"
    );

    Ok(())
}

/// Test: Type definitions work correctly with schema 1.2
#[tokio::test]
async fn test_type_definitions_schema_1_2() -> Result<()> {
    let store_id = create_test_store().await?;

    if !is_schema_1_2_supported(&store_id).await {
        eprintln!("SKIPPED: Schema 1.2 not supported");
        return Ok(());
    }

    let client = shared_client();

    // Create model with multiple types
    let model = json!({
        "schema_version": "1.2",
        "type_definitions": [
            { "type": "user" },
            {
                "type": "group",
                "relations": {
                    "member": { "this": {} }
                },
                "metadata": {
                    "relations": {
                        "member": {
                            "directly_related_user_types": [{ "type": "user" }]
                        }
                    }
                }
            },
            {
                "type": "document",
                "relations": {
                    "viewer": { "this": {} },
                    "editor": { "this": {} }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [
                                { "type": "user" },
                                { "type": "group", "relation": "member" }
                            ]
                        },
                        "editor": {
                            "directly_related_user_types": [{ "type": "user" }]
                        }
                    }
                }
            }
        ]
    });

    let response = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model)
        .send()
        .await?;

    assert!(
        response.status().is_success(),
        "Creating schema 1.2 model with multiple types should succeed"
    );

    Ok(())
}

/// Test: Computed relations work with schema 1.2
#[tokio::test]
async fn test_computed_relations_schema_1_2() -> Result<()> {
    let store_id = create_test_store().await?;

    if !is_schema_1_2_supported(&store_id).await {
        eprintln!("SKIPPED: Schema 1.2 not supported");
        return Ok(());
    }

    let client = shared_client();

    // Create model with computed relations
    let model = json!({
        "schema_version": "1.2",
        "type_definitions": [
            { "type": "user" },
            {
                "type": "document",
                "relations": {
                    "owner": { "this": {} },
                    "editor": {
                        "union": {
                            "child": [
                                { "this": {} },
                                { "computedUserset": { "relation": "owner" } }
                            ]
                        }
                    },
                    "viewer": {
                        "union": {
                            "child": [
                                { "this": {} },
                                { "computedUserset": { "relation": "editor" } }
                            ]
                        }
                    }
                },
                "metadata": {
                    "relations": {
                        "owner": {
                            "directly_related_user_types": [{ "type": "user" }]
                        },
                        "editor": {
                            "directly_related_user_types": [{ "type": "user" }]
                        },
                        "viewer": {
                            "directly_related_user_types": [{ "type": "user" }]
                        }
                    }
                }
            }
        ]
    });

    let response = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model)
        .send()
        .await?;

    assert!(
        response.status().is_success(),
        "Creating schema 1.2 model with computed relations should succeed"
    );

    Ok(())
}
