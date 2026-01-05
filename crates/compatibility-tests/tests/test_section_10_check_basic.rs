mod common;

use anyhow::Result;
use common::{create_test_model, create_test_store, get_openfga_url, write_tuples};
use reqwest::StatusCode;
use serde_json::json;
use uuid::Uuid;

/// Test: POST /stores/{store_id}/check performs direct relation check
#[tokio::test]
async fn test_check_direct_relation() -> Result<()> {
    // Arrange: Create store, model, and write tuple
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    write_tuples(&store_id, vec![("user:alice", "viewer", "document:doc1")]).await?;

    let client = reqwest::Client::new();

    // Act: Check if alice is a viewer of doc1
    let check_request = json!({
        "tuple_key": {
            "user": "user:alice",
            "relation": "viewer",
            "object": "document:doc1"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    // Assert: Check succeeded
    assert!(
        response.status().is_success(),
        "Check should succeed, got: {}",
        response.status()
    );

    let response_body: serde_json::Value = response.json().await?;

    // Verify response has 'allowed' field set to true
    assert_eq!(
        response_body.get("allowed").and_then(|v| v.as_bool()),
        Some(true),
        "Check for an existing direct relation should return {{allowed: true}}"
    );

    Ok(())
}

/// Test: Check returns {allowed: true} when tuple exists
#[tokio::test]
async fn test_check_returns_true_when_tuple_exists() -> Result<()> {
    // Arrange: Create store, model, and write tuple
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    write_tuples(&store_id, vec![("user:bob", "editor", "document:doc2")]).await?;

    let client = reqwest::Client::new();

    // Act: Check if bob is an editor of doc2
    let check_request = json!({
        "tuple_key": {
            "user": "user:bob",
            "relation": "editor",
            "object": "document:doc2"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;

    // Assert: allowed should be true
    assert_eq!(
        response_body.get("allowed").and_then(|v| v.as_bool()),
        Some(true),
        "Check should return {{allowed: true}} when tuple exists"
    );

    Ok(())
}

/// Test: Check returns {allowed: false} when tuple doesn't exist
#[tokio::test]
async fn test_check_returns_false_when_tuple_does_not_exist() -> Result<()> {
    // Arrange: Create store and model, but DON'T write the tuple
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    let client = reqwest::Client::new();

    // Act: Check for non-existent tuple
    let check_request = json!({
        "tuple_key": {
            "user": "user:charlie",
            "relation": "viewer",
            "object": "document:doc3"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;

    // Assert: allowed should be false
    assert_eq!(
        response_body.get("allowed").and_then(|v| v.as_bool()),
        Some(false),
        "Check should return {{allowed: false}} when tuple doesn't exist"
    );

    Ok(())
}

/// Test: Check follows computed relations (union)
#[tokio::test]
async fn test_check_follows_computed_union() -> Result<()> {
    // Arrange: Create store with union relation model
    let store_id = create_test_store().await?;

    // Create model with union: can_view = viewer + editor
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
                    },
                    "editor": {
                        "this": {}
                    },
                    "can_view": {
                        "union": {
                            "child": [
                                {
                                    "this": {}
                                },
                                {
                                    "computedUserset": {
                                        "relation": "viewer"
                                    }
                                },
                                {
                                    "computedUserset": {
                                        "relation": "editor"
                                    }
                                }
                            ]
                        }
                    }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [{"type": "user"}]
                        },
                        "editor": {
                            "directly_related_user_types": [{"type": "user"}]
                        },
                        "can_view": {
                            "directly_related_user_types": [{"type": "user"}]
                        }
                    }
                }
            }
        ]
    });

    let client = reqwest::Client::new();

    let model_response = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model)
        .send()
        .await?;

    assert!(
        model_response.status().is_success(),
        "Creating union model should succeed"
    );

    // Write tuple: dave is editor
    write_tuples(&store_id, vec![("user:dave", "editor", "document:doc4")]).await?;

    // Act: Check if dave can_view (should follow union to editor)
    let check_request = json!({
        "tuple_key": {
            "user": "user:dave",
            "relation": "can_view",
            "object": "document:doc4"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;

    // Assert: allowed should be true (union includes editor)
    assert_eq!(
        response_body.get("allowed").and_then(|v| v.as_bool()),
        Some(true),
        "Check should follow union relation: editor implies can_view"
    );

    Ok(())
}

/// Test: Check follows computed relations (intersection)
#[tokio::test]
async fn test_check_follows_computed_intersection() -> Result<()> {
    // Arrange: Create store with intersection relation model
    let store_id = create_test_store().await?;

    // Create model with intersection: can_edit = viewer AND editor
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
                    },
                    "editor": {
                        "this": {}
                    },
                    "can_edit": {
                        "intersection": {
                            "child": [
                                {
                                    "computedUserset": {
                                        "relation": "viewer"
                                    }
                                },
                                {
                                    "computedUserset": {
                                        "relation": "editor"
                                    }
                                }
                            ]
                        }
                    }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [{"type": "user"}]
                        },
                        "editor": {
                            "directly_related_user_types": [{"type": "user"}]
                        }
                    }
                }
            }
        ]
    });

    let client = reqwest::Client::new();

    let model_response = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model)
        .send()
        .await?;

    assert!(
        model_response.status().is_success(),
        "Creating intersection model should succeed"
    );

    // Write tuples: eve is BOTH viewer and editor
    write_tuples(
        &store_id,
        vec![
            ("user:eve", "viewer", "document:doc5"),
            ("user:eve", "editor", "document:doc5"),
        ],
    )
    .await?;

    // Act: Check if eve can_edit (requires both viewer AND editor)
    let check_request = json!({
        "tuple_key": {
            "user": "user:eve",
            "relation": "can_edit",
            "object": "document:doc5"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;

    // Assert: allowed should be true (has both viewer AND editor)
    assert_eq!(
        response_body.get("allowed").and_then(|v| v.as_bool()),
        Some(true),
        "Check should follow intersection relation: user has both viewer AND editor"
    );

    Ok(())
}

/// Test: Check follows computed relations (exclusion/but-not)
#[tokio::test]
async fn test_check_follows_computed_exclusion() -> Result<()> {
    // Arrange: Create store with exclusion relation model
    let store_id = create_test_store().await?;

    // Create model with exclusion: can_view_only = viewer BUT NOT editor
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
                    },
                    "editor": {
                        "this": {}
                    },
                    "can_view_only": {
                        "difference": {
                            "base": {
                                "computedUserset": {
                                    "relation": "viewer"
                                }
                            },
                            "subtract": {
                                "computedUserset": {
                                    "relation": "editor"
                                }
                            }
                        }
                    }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [{"type": "user"}]
                        },
                        "editor": {
                            "directly_related_user_types": [{"type": "user"}]
                        }
                    }
                }
            }
        ]
    });

    let client = reqwest::Client::new();

    let model_response = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model)
        .send()
        .await?;

    assert!(
        model_response.status().is_success(),
        "Creating exclusion model should succeed"
    );

    // Write tuple: frank is viewer but NOT editor
    write_tuples(&store_id, vec![("user:frank", "viewer", "document:doc6")]).await?;

    // Act: Check if frank can_view_only (viewer BUT NOT editor)
    let check_request = json!({
        "tuple_key": {
            "user": "user:frank",
            "relation": "can_view_only",
            "object": "document:doc6"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;

    // Assert: allowed should be true (viewer but NOT editor)
    assert_eq!(
        response_body.get("allowed").and_then(|v| v.as_bool()),
        Some(true),
        "Check should follow exclusion relation: user is viewer BUT NOT editor"
    );

    Ok(())
}

/// Test: Check resolves 'this' keyword correctly
#[tokio::test]
async fn test_check_resolves_this_keyword() -> Result<()> {
    // Arrange: Create store with model that uses 'this'
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    // The test model uses "this": {} for direct relations
    write_tuples(&store_id, vec![("user:george", "viewer", "document:doc7")]).await?;

    let client = reqwest::Client::new();

    // Act: Check direct relation (uses 'this')
    let check_request = json!({
        "tuple_key": {
            "user": "user:george",
            "relation": "viewer",
            "object": "document:doc7"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;

    // Assert: allowed should be true (this keyword resolved)
    assert_eq!(
        response_body.get("allowed").and_then(|v| v.as_bool()),
        Some(true),
        "Check should resolve 'this' keyword correctly"
    );

    Ok(())
}

/// Test: Check with contextual_tuples considers them
#[tokio::test]
async fn test_check_with_contextual_tuples() -> Result<()> {
    // Arrange: Create store and model, but DON'T write permanent tuple
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    let client = reqwest::Client::new();

    // Act: Check with contextual tuple (temporary permission)
    let check_request = json!({
        "tuple_key": {
            "user": "user:helen",
            "relation": "viewer",
            "object": "document:doc8"
        },
        "contextual_tuples": {
            "tuple_keys": [
                {
                    "user": "user:helen",
                    "relation": "viewer",
                    "object": "document:doc8"
                }
            ]
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;

    // Assert: allowed should be true (contextual tuple considered)
    assert_eq!(
        response_body.get("allowed").and_then(|v| v.as_bool()),
        Some(true),
        "Check should consider contextual_tuples"
    );

    Ok(())
}

/// Test: Check with non-existent store returns 404
#[tokio::test]
async fn test_check_with_nonexistent_store() -> Result<()> {
    // Arrange: Use non-existent store ID
    let client = reqwest::Client::new();
    let non_existent_store = Uuid::new_v4().to_string();

    // Act: Try to check with non-existent store
    let check_request = json!({
        "tuple_key": {
            "user": "user:ivan",
            "relation": "viewer",
            "object": "document:test"
        }
    });

    let response = client
        .post(format!(
            "{}/stores/{}/check",
            get_openfga_url(),
            non_existent_store
        ))
        .json(&check_request)
        .send()
        .await?;

    // Assert: Returns 400 or 404
    assert!(
        response.status() == StatusCode::NOT_FOUND || response.status() == StatusCode::BAD_REQUEST,
        "Check with non-existent store should return 404 or 400, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: Check with invalid tuple format returns 400
#[tokio::test]
async fn test_check_with_invalid_format() -> Result<()> {
    // Arrange: Create store and model
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    let client = reqwest::Client::new();

    // Act: Try to check with invalid format (missing required fields)
    let invalid_request = json!({
        "tuple_key": {
            "user": "user:judy",
            // Missing relation and object
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&invalid_request)
        .send()
        .await?;

    // Assert: Returns 400 Bad Request
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "Invalid check format should return 400, got: {}",
        response.status()
    );

    Ok(())
}
