use anyhow::Result;
use reqwest::StatusCode;
use serde_json::json;
use uuid::Uuid;

/// Helper function to get OpenFGA URL
fn get_openfga_url() -> String {
    std::env::var("OPENFGA_URL").unwrap_or_else(|_| "http://localhost:18080".to_string())
}

/// Helper function to create a test store
async fn create_test_store() -> Result<String> {
    let client = reqwest::Client::new();
    let store_name = format!("test-store-{}", Uuid::new_v4());

    let response = client
        .post(format!("{}/stores", get_openfga_url()))
        .json(&json!({ "name": store_name }))
        .send()
        .await?;

    let store: serde_json::Value = response.json().await?;
    Ok(store
        .get("id")
        .and_then(|v| v.as_str())
        .expect("Created store should have an ID")
        .to_string())
}

/// Test: POST /stores/{store_id}/authorization-models creates model
#[tokio::test]
async fn test_post_authorization_model_creates_model() -> Result<()> {
    // Arrange: Create a store
    let store_id = create_test_store().await?;
    let client = reqwest::Client::new();

    // Simple authorization model with direct relation
    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            {
                "type": "user"
            },
            {
                "type": "document",
                "relations": {
                    "viewer": {
                        "this": {}
                    }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [
                                {"type": "user"}
                            ]
                        }
                    }
                }
            }
        ]
    });

    // Act: Create authorization model
    let response = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model)
        .send()
        .await?;

    // Assert: Model created successfully
    assert!(
        response.status().is_success(),
        "Model creation should succeed, got: {}",
        response.status()
    );

    let response_body: serde_json::Value = response.json().await?;

    // Verify response contains authorization_model_id
    assert!(
        response_body.get("authorization_model_id").is_some(),
        "Response should contain 'authorization_model_id'"
    );

    Ok(())
}

/// Test: Model creation returns generated model_id
#[tokio::test]
async fn test_model_creation_returns_generated_model_id() -> Result<()> {
    // Arrange: Create a store
    let store_id = create_test_store().await?;
    let client = reqwest::Client::new();

    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            {
                "type": "user"
            }
        ]
    });

    // Act: Create authorization model
    let response = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model)
        .send()
        .await?;

    // Assert: Response contains model ID
    let response_body: serde_json::Value = response.json().await?;

    let model_id = response_body
        .get("authorization_model_id")
        .and_then(|v| v.as_str())
        .unwrap();

    assert!(
        !model_id.is_empty(),
        "Model ID should not be empty"
    );

    // Verify it's a valid ID format (looks like ULID or UUID)
    assert!(
        model_id.len() > 10,
        "Model ID should be a reasonable length, got: {}",
        model_id.len()
    );

    Ok(())
}

/// Test: Can create model with only direct relations
#[tokio::test]
async fn test_can_create_model_with_direct_relations() -> Result<()> {
    // Arrange: Create a store
    let store_id = create_test_store().await?;
    let client = reqwest::Client::new();

    // Model with multiple direct relations
    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            {
                "type": "user"
            },
            {
                "type": "document",
                "relations": {
                    "owner": {
                        "this": {}
                    },
                    "editor": {
                        "this": {}
                    },
                    "viewer": {
                        "this": {}
                    }
                },
                "metadata": {
                    "relations": {
                        "owner": {
                            "directly_related_user_types": [{"type": "user"}]
                        },
                        "editor": {
                            "directly_related_user_types": [{"type": "user"}]
                        },
                        "viewer": {
                            "directly_related_user_types": [{"type": "user"}]
                        }
                    }
                }
            }
        ]
    });

    // Act: Create model
    let response = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model)
        .send()
        .await?;

    // Assert: Model created successfully
    assert!(
        response.status().is_success(),
        "Model with direct relations should be created, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: Can create model with computed relations (union, intersection)
#[tokio::test]
async fn test_can_create_model_with_computed_relations() -> Result<()> {
    // Arrange: Create a store
    let store_id = create_test_store().await?;
    let client = reqwest::Client::new();

    // Model with computed relations (union and intersection)
    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            {
                "type": "user"
            },
            {
                "type": "document",
                "relations": {
                    "owner": {
                        "this": {}
                    },
                    "editor": {
                        "this": {}
                    },
                    "viewer": {
                        "union": {
                            "child": [
                                {
                                    "this": {}
                                },
                                {
                                    "computedUserset": {
                                        "relation": "editor"
                                    }
                                },
                                {
                                    "computedUserset": {
                                        "relation": "owner"
                                    }
                                }
                            ]
                        }
                    },
                    "can_edit": {
                        "intersection": {
                            "child": [
                                {
                                    "this": {}
                                },
                                {
                                    "computedUserset": {
                                        "relation": "viewer"
                                    }
                                }
                            ]
                        }
                    }
                },
                "metadata": {
                    "relations": {
                        "owner": {
                            "directly_related_user_types": [{"type": "user"}]
                        },
                        "editor": {
                            "directly_related_user_types": [{"type": "user"}]
                        },
                        "viewer": {
                            "directly_related_user_types": [{"type": "user"}]
                        },
                        "can_edit": {
                            "directly_related_user_types": [{"type": "user"}]
                        }
                    }
                }
            }
        ]
    });

    // Act: Create model
    let response = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model)
        .send()
        .await?;

    // Assert: Model created successfully
    assert!(
        response.status().is_success(),
        "Model with computed relations should be created, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: Can create model with 'this' keyword
#[tokio::test]
async fn test_can_create_model_with_this_keyword() -> Result<()> {
    // Arrange: Create a store
    let store_id = create_test_store().await?;
    let client = reqwest::Client::new();

    // Model using 'this' keyword for direct assignment
    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            {
                "type": "user"
            },
            {
                "type": "folder",
                "relations": {
                    "owner": {
                        "this": {}
                    }
                },
                "metadata": {
                    "relations": {
                        "owner": {
                            "directly_related_user_types": [{"type": "user"}]
                        }
                    }
                }
            }
        ]
    });

    // Act: Create model
    let response = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model)
        .send()
        .await?;

    // Assert: Model created successfully
    assert!(
        response.status().is_success(),
        "Model with 'this' keyword should be created, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: Can create model with wildcards
#[tokio::test]
async fn test_can_create_model_with_wildcards() -> Result<()> {
    // Arrange: Create a store
    let store_id = create_test_store().await?;
    let client = reqwest::Client::new();

    // Model with wildcard support (user:*)
    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            {
                "type": "user",
                "relations": {},
                "metadata": null
            },
            {
                "type": "document",
                "relations": {
                    "viewer": {
                        "this": {}
                    }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [
                                {
                                    "type": "user"
                                },
                                {
                                    "type": "user",
                                    "wildcard": {}
                                }
                            ]
                        }
                    }
                }
            }
        ]
    });

    // Act: Create model
    let response = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model)
        .send()
        .await?;

    // Assert: Model created successfully
    assert!(
        response.status().is_success(),
        "Model with wildcards should be created, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: Invalid model syntax returns 400 error
#[tokio::test]
async fn test_invalid_model_syntax_returns_400() -> Result<()> {
    // Arrange: Create a store
    let store_id = create_test_store().await?;
    let client = reqwest::Client::new();

    // Invalid model: missing required fields
    let invalid_model = json!({
        "schema_version": "1.1"
        // Missing type_definitions
    });

    // Act: Try to create invalid model
    let response = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&invalid_model)
        .send()
        .await?;

    // Assert: Returns 400 Bad Request
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "Invalid model should return 400, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: Duplicate type definitions return error
#[tokio::test]
async fn test_duplicate_type_definitions_return_error() -> Result<()> {
    // Arrange: Create a store
    let store_id = create_test_store().await?;
    let client = reqwest::Client::new();

    // Model with duplicate type definitions
    // Both types have valid metadata to ensure we're testing duplicate detection,
    // not metadata validation
    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            {
                "type": "user"
            },
            {
                "type": "document",
                "relations": {
                    "viewer": {
                        "this": {}
                    }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [{"type": "user"}]
                        }
                    }
                }
            },
            {
                "type": "document",  // Duplicate!
                "relations": {
                    "owner": {
                        "this": {}
                    }
                },
                "metadata": {
                    "relations": {
                        "owner": {
                            "directly_related_user_types": [{"type": "user"}]
                        }
                    }
                }
            }
        ]
    });

    // Act: Try to create model with duplicates
    let response = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model)
        .send()
        .await?;

    // Assert: Returns error (400 Bad Request)
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "Duplicate type definitions should return 400, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: Undefined relation references return error
#[tokio::test]
async fn test_undefined_relation_references_return_error() -> Result<()> {
    // Arrange: Create a store
    let store_id = create_test_store().await?;
    let client = reqwest::Client::new();

    // Model referencing undefined relation
    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            {
                "type": "user",
                "relations": {},
                "metadata": null
            },
            {
                "type": "document",
                "relations": {
                    "viewer": {
                        "computedUserset": {
                            "relation": "nonexistent_relation"  // Undefined!
                        }
                    }
                },
                "metadata": null
            }
        ]
    });

    // Act: Try to create model with undefined reference
    let response = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model)
        .send()
        .await?;

    // Assert: Returns error (400 Bad Request)
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "Undefined relation reference should return 400, got: {}",
        response.status()
    );

    Ok(())
}
