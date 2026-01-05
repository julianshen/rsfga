mod common;

use anyhow::Result;
use common::{create_test_store, create_wildcard_model, get_openfga_url, write_tuples};
use serde_json::json;

/// Test: Check resolves multi-hop relations (parent's viewer is child's viewer)
#[tokio::test]
async fn test_check_multi_hop_relations() -> Result<()> {
    // Arrange: Create store with parent-child relationship model
    let store_id = create_test_store().await?;

    // Create model: folder has parent relation, child inherits parent's viewers
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
        "Creating multi-hop model should succeed"
    );

    // Write tuples:
    // - folder:child has parent folder:parent
    // - user:alice is viewer of folder:parent
    write_tuples(
        &store_id,
        vec![
            ("folder:parent", "parent", "folder:child"),
            ("user:alice", "viewer", "folder:parent"),
        ],
    )
    .await?;

    // Act: Check if alice is viewer of folder:child (through parent relation)
    let check_request = json!({
        "tuple_key": {
            "user": "user:alice",
            "relation": "viewer",
            "object": "folder:child"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;

    // Assert: allowed should be true (multi-hop: parent's viewer)
    assert_eq!(
        response_body.get("allowed").and_then(|v| v.as_bool()),
        Some(true),
        "Check should resolve multi-hop relations: parent's viewer is child's viewer"
    );

    Ok(())
}

/// Test: Check resolves deeply nested relations (5+ hops)
#[tokio::test]
async fn test_check_deeply_nested_relations() -> Result<()> {
    // Arrange: Create store with deep hierarchy
    let store_id = create_test_store().await?;

    // Same parent-child model from previous test
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
        "Creating deep hierarchy model should succeed"
    );

    // Create 6-level deep hierarchy:
    // level1 -> level2 -> level3 -> level4 -> level5 -> level6
    write_tuples(
        &store_id,
        vec![
            ("folder:level1", "parent", "folder:level2"),
            ("folder:level2", "parent", "folder:level3"),
            ("folder:level3", "parent", "folder:level4"),
            ("folder:level4", "parent", "folder:level5"),
            ("folder:level5", "parent", "folder:level6"),
            ("user:bob", "viewer", "folder:level1"),
        ],
    )
    .await?;

    // Act: Check if bob is viewer of level6 (6 hops deep)
    let check_request = json!({
        "tuple_key": {
            "user": "user:bob",
            "relation": "viewer",
            "object": "folder:level6"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;

    // Assert: allowed should be true (6-hop traversal)
    assert_eq!(
        response_body.get("allowed").and_then(|v| v.as_bool()),
        Some(true),
        "Check should resolve deeply nested relations (6 hops)"
    );

    Ok(())
}

/// Test: Check handles union of multiple relations
#[tokio::test]
async fn test_check_union_multiple_relations() -> Result<()> {
    // Arrange: Create model with 3-way union
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
                    "owner": {
                        "this": {}
                    },
                    "editor": {
                        "this": {}
                    },
                    "viewer": {
                        "this": {}
                    },
                    "can_access": {
                        "union": {
                            "child": [
                                {
                                    "computedUserset": {
                                        "relation": "owner"
                                    }
                                },
                                {
                                    "computedUserset": {
                                        "relation": "editor"
                                    }
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

    // Write tuple: charlie is only viewer (not owner or editor)
    write_tuples(&store_id, vec![("user:charlie", "viewer", "document:doc1")]).await?;

    // Act: Check if charlie can_access (union of owner/editor/viewer)
    let check_request = json!({
        "tuple_key": {
            "user": "user:charlie",
            "relation": "can_access",
            "object": "document:doc1"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;

    // Assert: allowed should be true (union includes viewer)
    assert_eq!(
        response_body.get("allowed").and_then(|v| v.as_bool()),
        Some(true),
        "Check should handle union of multiple relations"
    );

    Ok(())
}

/// Test: Check handles intersection correctly (must satisfy all)
#[tokio::test]
async fn test_check_intersection_all_required() -> Result<()> {
    // Arrange: Create model with intersection
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
                    "approved": {
                        "this": {}
                    },
                    "editor": {
                        "this": {}
                    },
                    "can_publish": {
                        "intersection": {
                            "child": [
                                {
                                    "computedUserset": {
                                        "relation": "approved"
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
                        "approved": {
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

    // Write tuple: dave is approved but NOT editor (missing one requirement)
    write_tuples(&store_id, vec![("user:dave", "approved", "document:doc2")]).await?;

    // Act: Check if dave can_publish (requires approved AND editor)
    let check_request = json!({
        "tuple_key": {
            "user": "user:dave",
            "relation": "can_publish",
            "object": "document:doc2"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;

    // Assert: allowed should be FALSE (missing editor requirement)
    assert_eq!(
        response_body.get("allowed").and_then(|v| v.as_bool()),
        Some(false),
        "Check intersection should require ALL conditions (dave is approved but not editor)"
    );

    Ok(())
}

/// Test: Check handles but-not exclusion
#[tokio::test]
async fn test_check_exclusion_but_not() -> Result<()> {
    // Arrange: Create model with exclusion
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
        "Creating exclusion model should succeed, got: {}",
        model_response.status()
    );

    // Write tuples: eve is viewer AND blocked
    write_tuples(
        &store_id,
        vec![
            ("user:eve", "viewer", "document:doc3"),
            ("user:eve", "blocked", "document:doc3"),
        ],
    )
    .await?;

    // Act: Check if eve can_view (viewer BUT NOT blocked)
    let check_request = json!({
        "tuple_key": {
            "user": "user:eve",
            "relation": "can_view",
            "object": "document:doc3"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;

    // Assert: allowed should be FALSE (blocked subtracts from viewer)
    assert_eq!(
        response_body.get("allowed").and_then(|v| v.as_bool()),
        Some(false),
        "Check exclusion should subtract blocked from viewer"
    );

    Ok(())
}

/// Test: Check with cycle detection doesn't infinite loop
#[tokio::test]
async fn test_check_cycle_detection() -> Result<()> {
    // Arrange: Create model that could create cycles
    let store_id = create_test_store().await?;

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
        "Creating cycle detection model should succeed, got: {}",
        model_response.status()
    );

    // Create cycle: folderA -> folderB -> folderA
    write_tuples(
        &store_id,
        vec![
            ("folder:folderB", "parent", "folder:folderA"),
            ("folder:folderA", "parent", "folder:folderB"),
        ],
    )
    .await?;

    // Act: Check should complete without infinite loop
    let check_request = json!({
        "tuple_key": {
            "user": "user:frank",
            "relation": "viewer",
            "object": "folder:folderA"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    // Assert: Should complete successfully (with cycle detection)
    assert!(
        response.status().is_success(),
        "Check should handle cycles without infinite loop"
    );

    let response_body: serde_json::Value = response.json().await?;

    // Should return false since frank is not actually a viewer
    assert_eq!(
        response_body.get("allowed").and_then(|v| v.as_bool()),
        Some(false),
        "Check should return false for non-existent permission (after cycle detection)"
    );

    Ok(())
}

/// Test: Check with wildcards (user:*)
#[tokio::test]
async fn test_check_with_wildcards() -> Result<()> {
    // Arrange: Create store with wildcard support
    let store_id = create_test_store().await?;
    let _model_id = create_wildcard_model(&store_id).await?;

    // Write wildcard tuple: user:* is viewer (public access)
    write_tuples(&store_id, vec![("user:*", "viewer", "document:public")]).await?;

    let client = reqwest::Client::new();

    // Act: Check if any user (george) is viewer
    let check_request = json!({
        "tuple_key": {
            "user": "user:george",
            "relation": "viewer",
            "object": "document:public"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;

    // Assert: allowed should be true (wildcard matches any user)
    assert_eq!(
        response_body.get("allowed").and_then(|v| v.as_bool()),
        Some(true),
        "Check should resolve wildcard user:* to match any user"
    );

    Ok(())
}

/// Test: Check respects authorization model version
#[tokio::test]
async fn test_check_respects_model_version() -> Result<()> {
    // Arrange: Create store with initial model
    let store_id = create_test_store().await?;

    // Create first model version (only has viewer)
    let model_v1 = json!({
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

    let client = reqwest::Client::new();

    let response_v1 = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model_v1)
        .send()
        .await?;

    let model_v1_body: serde_json::Value = response_v1.json().await?;
    let model_v1_id = model_v1_body
        .get("authorization_model_id")
        .and_then(|v| v.as_str())
        .expect("Should have model ID");

    // Create second model version (adds editor relation)
    let model_v2 = json!({
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

    let response_v2 = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model_v2)
        .send()
        .await?;

    assert!(
        response_v2.status().is_success(),
        "Creating second model version should succeed, got: {}",
        response_v2.status()
    );

    // Write tuple using v2 model (has editor)
    write_tuples(&store_id, vec![("user:helen", "editor", "document:doc1")]).await?;

    // Act: Check with explicit v1 model (doesn't have editor relation)
    let check_request = json!({
        "tuple_key": {
            "user": "user:helen",
            "relation": "editor",
            "object": "document:doc1"
        },
        "authorization_model_id": model_v1_id
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    // Assert: Should return error (editor doesn't exist in v1)
    // Note: OpenFGA might return 400 or specific error for invalid relation
    assert!(
        !response.status().is_success(),
        "Check with old model version should fail for new relation type"
    );

    Ok(())
}
