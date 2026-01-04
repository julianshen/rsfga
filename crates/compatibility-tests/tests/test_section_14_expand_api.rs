mod common;

use anyhow::Result;
use common::{create_test_model, create_test_store, get_openfga_url, write_tuples};
use reqwest::StatusCode;
use serde_json::json;
use uuid::Uuid;

/// Test: POST /stores/{store_id}/expand returns relation tree
#[tokio::test]
async fn test_expand_returns_relation_tree() -> Result<()> {
    // Arrange: Create store, model, and write tuple
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    write_tuples(
        &store_id,
        vec![("user:alice", "viewer", "document:doc1")],
    )
    .await?;

    let client = reqwest::Client::new();

    // Act: Expand the viewer relation for doc1
    let expand_request = json!({
        "tuple_key": {
            "relation": "viewer",
            "object": "document:doc1"
        }
    });

    let response = client
        .post(format!(
            "{}/stores/{}/expand",
            get_openfga_url(),
            store_id
        ))
        .json(&expand_request)
        .send()
        .await?;

    // Assert: Expand succeeded
    assert!(
        response.status().is_success(),
        "Expand should succeed, got: {}",
        response.status()
    );

    let response_body: serde_json::Value = response.json().await?;

    // Verify response has 'tree' object
    assert!(
        response_body.get("tree").is_some(),
        "Response should have 'tree' field"
    );

    // Verify tree has 'root' node
    let tree = response_body.get("tree").unwrap();
    assert!(
        tree.get("root").is_some(),
        "Tree should have 'root' node"
    );

    Ok(())
}

/// Test: Expand shows direct relations as leaf nodes
#[tokio::test]
async fn test_expand_shows_direct_relations_as_leaf() -> Result<()> {
    // Arrange: Create store, model, and write direct tuple
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    write_tuples(
        &store_id,
        vec![
            ("user:alice", "viewer", "document:doc1"),
            ("user:bob", "viewer", "document:doc1"),
        ],
    )
    .await?;

    let client = reqwest::Client::new();

    // Act: Expand viewer relation
    let expand_request = json!({
        "tuple_key": {
            "relation": "viewer",
            "object": "document:doc1"
        }
    });

    let response = client
        .post(format!(
            "{}/stores/{}/expand",
            get_openfga_url(),
            store_id
        ))
        .json(&expand_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;

    // Assert: Root node should have a leaf with users
    let root = response_body
        .get("tree")
        .and_then(|t| t.get("root"))
        .expect("Should have root node");

    // Check if root has leaf node
    let leaf = root.get("leaf");
    assert!(leaf.is_some(), "Root should have leaf node for direct relations");

    // Leaf should contain users
    let leaf_value = leaf.unwrap();
    assert!(
        leaf_value.get("users").is_some(),
        "Leaf should contain users field"
    );

    Ok(())
}

/// Test: Expand shows computed relations as tree nodes
#[tokio::test]
async fn test_expand_shows_computed_relations() -> Result<()> {
    // Arrange: Create store with computed relation model
    let store_id = create_test_store().await?;

    // Create model with computed relation: can_view = viewer
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
                    "can_view": {
                        "computedUserset": {
                            "relation": "viewer"
                        }
                    }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
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
        "Creating computed relation model should succeed, got: {}",
        model_response.status()
    );

    write_tuples(&store_id, vec![("user:charlie", "viewer", "document:doc2")])
        .await?;

    // Act: Expand can_view relation (should show computed reference to viewer)
    let expand_request = json!({
        "tuple_key": {
            "relation": "can_view",
            "object": "document:doc2"
        }
    });

    let response = client
        .post(format!(
            "{}/stores/{}/expand",
            get_openfga_url(),
            store_id
        ))
        .json(&expand_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;

    // Assert: Should show computed relation in the tree
    let root = response_body
        .get("tree")
        .and_then(|t| t.get("root"))
        .expect("Should have root node");

    // Root should have leaf with computed userset
    let leaf = root.get("leaf");
    assert!(
        leaf.is_some(),
        "Root should have leaf node for computed relation"
    );

    let leaf_value = leaf.unwrap();
    assert!(
        leaf_value.get("computed").is_some(),
        "Leaf should contain computed field for computed relation"
    );

    Ok(())
}

/// Test: Expand includes union branches
#[tokio::test]
async fn test_expand_includes_union_branches() -> Result<()> {
    // Arrange: Create store with union relation
    let store_id = create_test_store().await?;

    // Create model with union: can_access = viewer + editor
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
                    "can_access": {
                        "union": {
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
        "Creating union model should succeed, got: {}",
        model_response.status()
    );

    write_tuples(
        &store_id,
        vec![
            ("user:dave", "viewer", "document:doc3"),
            ("user:eve", "editor", "document:doc3"),
        ],
    )
    .await?;

    // Act: Expand can_access relation (should show union branches)
    let expand_request = json!({
        "tuple_key": {
            "relation": "can_access",
            "object": "document:doc3"
        }
    });

    let response = client
        .post(format!(
            "{}/stores/{}/expand",
            get_openfga_url(),
            store_id
        ))
        .json(&expand_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;

    // Assert: Root should have union node
    let root = response_body
        .get("tree")
        .and_then(|t| t.get("root"))
        .expect("Should have root node");

    assert!(
        root.get("union").is_some(),
        "Root should have 'union' node for union relation"
    );

    // Union should have multiple nodes (viewer and editor branches)
    let union_nodes = root
        .get("union")
        .and_then(|u| u.get("nodes"))
        .and_then(|n| n.as_array())
        .expect("Union should have nodes array");

    assert!(
        union_nodes.len() >= 2,
        "Union should have at least 2 branches (viewer and editor), got: {}",
        union_nodes.len()
    );

    Ok(())
}

/// Test: Expand includes intersection branches
#[tokio::test]
async fn test_expand_includes_intersection_branches() -> Result<()> {
    // Arrange: Create store with intersection relation
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
        "Creating intersection model should succeed, got: {}",
        model_response.status()
    );

    write_tuples(
        &store_id,
        vec![
            ("user:frank", "viewer", "document:doc4"),
            ("user:frank", "editor", "document:doc4"),
        ],
    )
    .await?;

    // Act: Expand can_edit relation (should show intersection branches)
    let expand_request = json!({
        "tuple_key": {
            "relation": "can_edit",
            "object": "document:doc4"
        }
    });

    let response = client
        .post(format!(
            "{}/stores/{}/expand",
            get_openfga_url(),
            store_id
        ))
        .json(&expand_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;

    // Assert: Root should have intersection node
    let root = response_body
        .get("tree")
        .and_then(|t| t.get("root"))
        .expect("Should have root node");

    assert!(
        root.get("intersection").is_some(),
        "Root should have 'intersection' node for intersection relation"
    );

    // Intersection should have multiple nodes (viewer and editor branches)
    let intersection_nodes = root
        .get("intersection")
        .and_then(|i| i.get("nodes"))
        .and_then(|n| n.as_array())
        .expect("Intersection should have nodes array");

    assert!(
        intersection_nodes.len() >= 2,
        "Intersection should have at least 2 branches (viewer and editor), got: {}",
        intersection_nodes.len()
    );

    Ok(())
}

/// Test: Expand includes difference (but-not) branches
#[tokio::test]
async fn test_expand_includes_difference_branches() -> Result<()> {
    // Arrange: Create store with difference relation
    let store_id = create_test_store().await?;

    // Create model with difference: can_view = viewer BUT NOT blocked
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
                    "blocked": {
                        "this": {}
                    },
                    "can_view": {
                        "difference": {
                            "base": {
                                "computedUserset": {
                                    "relation": "viewer"
                                }
                            },
                            "subtract": {
                                "computedUserset": {
                                    "relation": "blocked"
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
                        "blocked": {
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
        "Creating difference model should succeed, got: {}",
        model_response.status()
    );

    write_tuples(
        &store_id,
        vec![
            ("user:george", "viewer", "document:doc5"),
            ("user:helen", "blocked", "document:doc5"),
        ],
    )
    .await?;

    // Act: Expand can_view relation (should show difference branches)
    let expand_request = json!({
        "tuple_key": {
            "relation": "can_view",
            "object": "document:doc5"
        }
    });

    let response = client
        .post(format!(
            "{}/stores/{}/expand",
            get_openfga_url(),
            store_id
        ))
        .json(&expand_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;

    // Assert: Root should have difference node
    let root = response_body
        .get("tree")
        .and_then(|t| t.get("root"))
        .expect("Should have root node");

    assert!(
        root.get("difference").is_some(),
        "Root should have 'difference' node for difference relation"
    );

    // Difference should have base and subtract nodes
    let difference = root.get("difference").unwrap();
    assert!(
        difference.get("base").is_some(),
        "Difference should have 'base' node"
    );
    assert!(
        difference.get("subtract").is_some(),
        "Difference should have 'subtract' node"
    );

    Ok(())
}

/// Test: Expand with non-existent object returns empty or minimal tree
#[tokio::test]
async fn test_expand_with_nonexistent_object() -> Result<()> {
    // Arrange: Create store and model, but DON'T write tuples
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    let client = reqwest::Client::new();

    // Act: Expand for non-existent object
    let expand_request = json!({
        "tuple_key": {
            "relation": "viewer",
            "object": "document:nonexistent"
        }
    });

    let response = client
        .post(format!(
            "{}/stores/{}/expand",
            get_openfga_url(),
            store_id
        ))
        .json(&expand_request)
        .send()
        .await?;

    // Assert: Expand should succeed (might return empty tree or leaf)
    assert!(
        response.status().is_success(),
        "Expand should succeed even for non-existent object, got: {}",
        response.status()
    );

    let response_body: serde_json::Value = response.json().await?;

    // Verify response has tree
    assert!(
        response_body.get("tree").is_some(),
        "Response should have 'tree' field even for non-existent object"
    );

    Ok(())
}

/// Test: Expand tree structure is deterministic (same result on repeat)
#[tokio::test]
async fn test_expand_tree_is_deterministic() -> Result<()> {
    // Arrange: Create store, model, and write tuples
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    write_tuples(
        &store_id,
        vec![
            ("user:ivan", "viewer", "document:doc6"),
            ("user:judy", "viewer", "document:doc6"),
        ],
    )
    .await?;

    let client = reqwest::Client::new();

    let expand_request = json!({
        "tuple_key": {
            "relation": "viewer",
            "object": "document:doc6"
        }
    });

    // Act: Expand same query 3 times
    let mut responses = Vec::new();
    for _ in 0..3 {
        let response = client
            .post(format!(
                "{}/stores/{}/expand",
                get_openfga_url(),
                store_id
            ))
            .json(&expand_request)
            .send()
            .await?;

        let response_body: serde_json::Value = response.json().await?;
        responses.push(response_body);
    }

    // Assert: All responses should be identical
    let first_response = &responses[0];
    for (i, response) in responses.iter().enumerate().skip(1) {
        assert_eq!(
            first_response, response,
            "Expand response {} should match first response (deterministic)",
            i + 1
        );
    }

    Ok(())
}

/// Test: Expand with invalid store returns 404 or 400
#[tokio::test]
async fn test_expand_with_invalid_store() -> Result<()> {
    // Arrange: Use non-existent store ID
    let client = reqwest::Client::new();
    let non_existent_store = Uuid::new_v4().to_string();

    // Act: Try to expand with non-existent store
    let expand_request = json!({
        "tuple_key": {
            "relation": "viewer",
            "object": "document:test"
        }
    });

    let response = client
        .post(format!(
            "{}/stores/{}/expand",
            get_openfga_url(),
            non_existent_store
        ))
        .json(&expand_request)
        .send()
        .await?;

    // Assert: Returns 400 or 404
    assert!(
        response.status() == StatusCode::NOT_FOUND
            || response.status() == StatusCode::BAD_REQUEST,
        "Expand with non-existent store should return 404 or 400, got: {}",
        response.status()
    );

    Ok(())
}
