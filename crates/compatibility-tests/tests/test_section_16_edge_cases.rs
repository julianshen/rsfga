mod common;

use anyhow::Result;
use common::{create_authorization_model, create_test_store, get_openfga_url, write_tuples};
use serde_json::json;

/// Test: Expand with deep relation tree (approaching server limit)
#[tokio::test]
async fn test_expand_with_deep_relation_tree() -> Result<()> {
    // Arrange: Create store with deeply nested parent-child hierarchy
    let store_id = create_test_store().await?;

    // Create model with parent hierarchy (allows deep nesting)
    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            {
                "type": "user"
            },
            {
                "type": "folder",
                "relations": {
                    "parent": {
                        "this": {}
                    },
                    "viewer": {
                        "union": {
                            "child": [
                                {
                                    "this": {}
                                },
                                {
                                    "tupleToUserset": {
                                        "tupleset": {
                                            "relation": "parent"
                                        },
                                        "computedUserset": {
                                            "relation": "viewer"
                                        }
                                    }
                                }
                            ]
                        }
                    }
                },
                "metadata": {
                    "relations": {
                        "parent": {
                            "directly_related_user_types": [{"type": "folder"}]
                        },
                        "viewer": {
                            "directly_related_user_types": [{"type": "user"}]
                        }
                    }
                }
            }
        ]
    });

    let _model_id = create_authorization_model(&store_id, model).await?;

    // Create 20-level deep hierarchy (approaching default limit of 25)
    let mut tuples = Vec::new();
    for i in 1..20 {
        tuples.push((
            format!("folder:level{}", i),
            "parent".to_string(),
            format!("folder:level{}", i + 1),
        ));
    }
    // Add viewer at the root
    tuples.push((
        "user:alice".to_string(),
        "viewer".to_string(),
        "folder:level1".to_string(),
    ));

    let tuple_refs: Vec<(&str, &str, &str)> = tuples
        .iter()
        .map(|(u, r, o)| (u.as_str(), r.as_str(), o.as_str()))
        .collect();

    write_tuples(&store_id, tuple_refs).await?;

    let client = reqwest::Client::new();

    // Act: Expand viewer at deepest level (should traverse 20 hops)
    let expand_request = json!({
        "tuple_key": {
            "relation": "viewer",
            "object": "folder:level20"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/expand", get_openfga_url(), store_id))
        .json(&expand_request)
        .send()
        .await?;

    // Assert: Expand should succeed (not hit depth limit)
    assert!(
        response.status().is_success(),
        "Expand with 20-level deep hierarchy should succeed, got: {}",
        response.status()
    );

    let response_body: serde_json::Value = response.json().await?;

    // Verify tree was returned
    assert!(
        response_body.get("tree").is_some(),
        "Should return tree for deep hierarchy"
    );

    Ok(())
}

/// Test: Expand with circular relation definition (cycle detection)
#[tokio::test]
async fn test_expand_with_circular_relations() -> Result<()> {
    // Arrange: Create store with model allowing circular references
    let store_id = create_test_store().await?;

    // Create model with parent hierarchy (can create cycles)
    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            {
                "type": "user"
            },
            {
                "type": "folder",
                "relations": {
                    "parent": {
                        "this": {}
                    },
                    "viewer": {
                        "union": {
                            "child": [
                                {
                                    "this": {}
                                },
                                {
                                    "tupleToUserset": {
                                        "tupleset": {
                                            "relation": "parent"
                                        },
                                        "computedUserset": {
                                            "relation": "viewer"
                                        }
                                    }
                                }
                            ]
                        }
                    }
                },
                "metadata": {
                    "relations": {
                        "parent": {
                            "directly_related_user_types": [{"type": "folder"}]
                        },
                        "viewer": {
                            "directly_related_user_types": [{"type": "user"}]
                        }
                    }
                }
            }
        ]
    });

    let _model_id = create_authorization_model(&store_id, model).await?;

    // Create cycle: folderA -> folderB -> folderA
    write_tuples(
        &store_id,
        vec![
            ("folder:folderB", "parent", "folder:folderA"),
            ("folder:folderA", "parent", "folder:folderB"),
            ("user:bob", "viewer", "folder:folderA"),
        ],
    )
    .await?;

    let client = reqwest::Client::new();

    // Act: Expand viewer on cyclic structure (should handle gracefully)
    let expand_request = json!({
        "tuple_key": {
            "relation": "viewer",
            "object": "folder:folderA"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/expand", get_openfga_url(), store_id))
        .json(&expand_request)
        .send()
        .await?;

    // Assert: Expand should complete without infinite loop
    // (OpenFGA should detect cycles and prevent infinite recursion)
    assert!(
        response.status().is_success(),
        "Expand should handle cycles gracefully, got: {}",
        response.status()
    );

    let response_body: serde_json::Value = response.json().await?;

    // Verify tree was returned (should not be empty/error)
    assert!(
        response_body.get("tree").is_some(),
        "Should return tree even with circular relations"
    );

    Ok(())
}

/// Test: ListObjects with large result set (scalability)
#[tokio::test]
async fn test_listobjects_with_large_result_set() -> Result<()> {
    // Arrange: Create store and model
    let store_id = create_test_store().await?;

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
            }
        ]
    });

    let _model_id = create_authorization_model(&store_id, model).await?;

    // Create 1000 documents that charlie can view
    // Note: We'll do this in batches to avoid overwhelming the write API
    const TOTAL_DOCS: usize = 1000;
    const BATCH_SIZE: usize = 100;

    for batch_start in (0..TOTAL_DOCS).step_by(BATCH_SIZE) {
        let batch_end = (batch_start + BATCH_SIZE).min(TOTAL_DOCS);
        let batch_tuples: Vec<_> = (batch_start..batch_end)
            .map(|i| {
                (
                    "user:charlie".to_string(),
                    "viewer".to_string(),
                    format!("document:doc{}", i),
                )
            })
            .collect();

        let tuple_refs: Vec<(&str, &str, &str)> = batch_tuples
            .iter()
            .map(|(u, r, o)| (u.as_str(), r.as_str(), o.as_str()))
            .collect();

        write_tuples(&store_id, tuple_refs).await?;
    }

    let client = reqwest::Client::new();

    // Act: List all objects charlie can view (should return 1000)
    let list_request = json!({
        "type": "document",
        "relation": "viewer",
        "user": "user:charlie"
    });

    let start = std::time::Instant::now();

    let response = client
        .post(format!(
            "{}/stores/{}/list-objects",
            get_openfga_url(),
            store_id
        ))
        .json(&list_request)
        .send()
        .await?;

    let duration = start.elapsed();

    // Assert: ListObjects should succeed with large result set
    assert!(
        response.status().is_success(),
        "ListObjects with 1000 results should succeed, got: {}",
        response.status()
    );

    let response_body: serde_json::Value = response.json().await?;

    let objects = response_body
        .get("objects")
        .and_then(|o| o.as_array())
        .expect("Response should have 'objects' array");

    assert_eq!(
        objects.len(),
        TOTAL_DOCS,
        "Should return all 1000 documents"
    );

    // Performance check: Should complete in reasonable time (sanity check, not strict benchmark)
    assert!(
        duration.as_secs() < 2,
        "ListObjects with 1000 results should complete in < 2s, got: {:?}",
        duration
    );

    Ok(())
}

/// Test: Expand and ListObjects with complex nested computed relations
#[tokio::test]
async fn test_complex_nested_computed_relations() -> Result<()> {
    // Arrange: Create store with complex multi-level computed relations
    let store_id = create_test_store().await?;

    // Create model with multiple levels of computed relations:
    // editor implies viewer, owner implies editor
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
                        "union": {
                            "child": [
                                {
                                    "this": {}
                                },
                                {
                                    "computedUserset": {
                                        "relation": "owner"
                                    }
                                }
                            ]
                        }
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
                        }
                    }
                }
            }
        ]
    });

    let _model_id = create_authorization_model(&store_id, model).await?;

    // Write tuple: dave is owner (should imply editor and viewer)
    write_tuples(&store_id, vec![("user:dave", "owner", "document:doc1")]).await?;

    let client = reqwest::Client::new();

    // Act 1: Expand viewer relation (should show nested computed path)
    let expand_request = json!({
        "tuple_key": {
            "relation": "viewer",
            "object": "document:doc1"
        }
    });

    let expand_response = client
        .post(format!("{}/stores/{}/expand", get_openfga_url(), store_id))
        .json(&expand_request)
        .send()
        .await?;

    assert!(
        expand_response.status().is_success(),
        "Expand should succeed for nested computed relations"
    );

    let expand_body: serde_json::Value = expand_response.json().await?;

    // Verify tree shows union structure
    let root = expand_body
        .get("tree")
        .and_then(|t| t.get("root"))
        .expect("Should have root node");

    assert!(
        root.get("union").is_some(),
        "Root should have union node for computed relation"
    );

    // Act 2: ListObjects with viewer relation (should find doc1 via owner -> editor -> viewer)
    let list_request = json!({
        "type": "document",
        "relation": "viewer",
        "user": "user:dave"
    });

    let list_response = client
        .post(format!(
            "{}/stores/{}/list-objects",
            get_openfga_url(),
            store_id
        ))
        .json(&list_request)
        .send()
        .await?;

    let list_body: serde_json::Value = list_response.json().await?;

    let objects = list_body
        .get("objects")
        .and_then(|o| o.as_array())
        .expect("Response should have 'objects' array");

    // Dave should be able to view doc1 (owner implies editor implies viewer)
    assert_eq!(
        objects.len(),
        1,
        "Owner should be viewer via nested computed relations"
    );

    let object_ids: Vec<String> = objects
        .iter()
        .filter_map(|o| o.as_str())
        .map(|s| s.to_string())
        .collect();

    assert!(object_ids.contains(&"document:doc1".to_string()));

    Ok(())
}
