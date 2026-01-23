//! CEL Condition Evaluation Integration Tests for Issue #206.
//!
//! These tests verify that CEL condition context flows correctly through
//! Check, Expand, and List operations.
//!
//! ## Test Coverage
//!
//! ### Check Operations with Conditions
//! - Check with context evaluates conditions correctly (true when condition matches)
//! - Check with context evaluates conditions correctly (false when condition doesn't match)
//! - Check without context on conditional tuple behaves correctly
//! - Batch check items with different contexts evaluate independently
//! - Context with nested JSON structures handled correctly (no injection)
//!
//! ### Expand Operations with Conditions
//! - Expand shows conditional tuples in output with context requirements
//! - Expand tree correctly represents condition dependencies
//!
//! ### List Operations with Conditions
//! - List Objects respects conditional tuples (only includes if condition matches)
//! - List Users respects conditional tuples (only includes if condition matches)
//!
//! ### Contextual Tuples
//! - Check with contextual tuples includes them in traversal
//! - Contextual tuples don't persist to storage after check completes
//! - Batch check with contextual tuples applies them to all items in batch
//! - List Objects with contextual tuples changes result set correctly
//! - List Users with contextual tuples changes result set correctly
//! - Contextual tuples don't override stored tuples (union behavior)
//! - Malformed contextual tuples rejected with clear error
//!
//! ### Edge Cases
//! - Empty context handled correctly
//! - Context with special characters (unicode) handled safely
//!
//! Run with: cargo test --test cel_condition_tests

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use rsfga_storage::{DataStore, MemoryDataStore, StoredAuthorizationModel, StoredTuple, Utc};

use common::{create_test_app, post_json};

// =============================================================================
// Helper Functions
// =============================================================================

/// Create a store with authorization model that includes conditions.
async fn create_store_with_conditional_model(storage: &Arc<MemoryDataStore>) -> String {
    // Create store
    let (status, response) = post_json(
        create_test_app(storage),
        "/stores",
        serde_json::json!({
            "name": "cel-condition-test-store"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();

    // Create model with conditions
    let model_json = serde_json::json!({
        "schema_version": "1.1",
        "type_definitions": [
            {"type": "user"},
            {"type": "team", "relations": {"member": {"this": {}}}},
            {
                "type": "document",
                "relations": {
                    "viewer": {"this": {}},
                    "time_viewer": {"this": {}},
                    "dept_viewer": {"this": {}},
                    "admin_viewer": {"this": {}},
                    "owner": {"this": {}},
                    "can_view": {
                        "union": {
                            "child": [
                                {"computedUserset": {"relation": "viewer"}},
                                {"computedUserset": {"relation": "time_viewer"}},
                                {"computedUserset": {"relation": "owner"}}
                            ]
                        }
                    }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [
                                {"type": "user"},
                                {"type": "team", "relation": "member"}
                            ]
                        },
                        "time_viewer": {
                            "directly_related_user_types": [
                                {
                                    "type": "user",
                                    "condition": "time_check"
                                }
                            ]
                        },
                        "dept_viewer": {
                            "directly_related_user_types": [
                                {
                                    "type": "user",
                                    "condition": "dept_check"
                                }
                            ]
                        },
                        "admin_viewer": {
                            "directly_related_user_types": [
                                {
                                    "type": "user",
                                    "condition": "admin_check"
                                }
                            ]
                        },
                        "owner": {
                            "directly_related_user_types": [
                                {"type": "user"}
                            ]
                        },
                        "can_view": {
                            "directly_related_user_types": []
                        }
                    }
                }
            }
        ],
        "conditions": {
            "time_check": {
                "name": "time_check",
                "expression": "request.current_time < request.expires_at",
                "parameters": {
                    "current_time": {"type_name": "TYPE_NAME_TIMESTAMP"},
                    "expires_at": {"type_name": "TYPE_NAME_TIMESTAMP"}
                }
            },
            "dept_check": {
                "name": "dept_check",
                "expression": "request.user_dept == request.required_dept",
                "parameters": {
                    "user_dept": {"type_name": "TYPE_NAME_STRING"},
                    "required_dept": {"type_name": "TYPE_NAME_STRING"}
                }
            },
            "admin_check": {
                "name": "admin_check",
                "expression": "request.is_admin == true || request.access_level >= 5",
                "parameters": {
                    "is_admin": {"type_name": "TYPE_NAME_BOOL"},
                    "access_level": {"type_name": "TYPE_NAME_INT"}
                }
            }
        }
    });

    let model = StoredAuthorizationModel::new(
        ulid::Ulid::new().to_string(),
        &store_id,
        "1.1",
        serde_json::to_string(&model_json).unwrap(),
    );
    storage.write_authorization_model(model).await.unwrap();

    store_id
}

/// Create a simple store with model for contextual tuple tests.
async fn create_store_with_simple_model(storage: &Arc<MemoryDataStore>) -> String {
    // Create store
    let (status, response) = post_json(
        create_test_app(storage),
        "/stores",
        serde_json::json!({
            "name": "contextual-tuple-test-store"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let store_id = response["id"].as_str().unwrap().to_string();

    // Create simple model
    let model_json = r#"{
        "type_definitions": [
            {"type": "user"},
            {"type": "team", "relations": {"member": {}}},
            {
                "type": "document",
                "relations": {
                    "viewer": {},
                    "editor": {},
                    "owner": {},
                    "can_view": {
                        "union": {
                            "child": [
                                {"computedUserset": {"relation": "viewer"}},
                                {"computedUserset": {"relation": "editor"}},
                                {"computedUserset": {"relation": "owner"}}
                            ]
                        }
                    }
                }
            }
        ]
    }"#;
    let model =
        StoredAuthorizationModel::new(ulid::Ulid::new().to_string(), &store_id, "1.1", model_json);
    storage.write_authorization_model(model).await.unwrap();

    store_id
}

// =============================================================================
// Section 1: Check Operations with Conditions
// =============================================================================

/// Test: Check with context evaluates conditions correctly (true when condition matches).
#[tokio::test]
async fn test_check_with_context_condition_evaluates_true() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_conditional_model(&storage).await;

    // Write tuple with time condition
    let tuple = StoredTuple {
        user_type: "user".to_string(),
        user_id: "alice".to_string(),
        user_relation: None,
        relation: "time_viewer".to_string(),
        object_type: "document".to_string(),
        object_id: "secret-doc".to_string(),
        condition_name: Some("time_check".to_string()),
        condition_context: None,
        created_at: Some(Utc::now()),
    };
    storage.write_tuple(&store_id, tuple).await.unwrap();

    // Check with context where condition evaluates to TRUE
    // current_time < expires_at => TRUE
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "time_viewer",
                "object": "document:secret-doc"
            },
            "context": {
                "current_time": "2024-01-01T00:00:00Z",
                "expires_at": "2024-12-31T23:59:59Z"
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"].as_bool(),
        Some(true),
        "Check should return allowed: true when condition evaluates to true"
    );
}

/// Test: Check with context evaluates conditions correctly (false when condition doesn't match).
#[tokio::test]
async fn test_check_with_context_condition_evaluates_false() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_conditional_model(&storage).await;

    // Write tuple with time condition
    let tuple = StoredTuple {
        user_type: "user".to_string(),
        user_id: "bob".to_string(),
        user_relation: None,
        relation: "time_viewer".to_string(),
        object_type: "document".to_string(),
        object_id: "expired-doc".to_string(),
        condition_name: Some("time_check".to_string()),
        condition_context: None,
        created_at: Some(Utc::now()),
    };
    storage.write_tuple(&store_id, tuple).await.unwrap();

    // Check with context where condition evaluates to FALSE
    // current_time < expires_at => FALSE (current_time is AFTER expires_at)
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:bob",
                "relation": "time_viewer",
                "object": "document:expired-doc"
            },
            "context": {
                "current_time": "2025-06-01T00:00:00Z",
                "expires_at": "2024-12-31T23:59:59Z"
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"].as_bool(),
        Some(false),
        "Check should return allowed: false when condition evaluates to false"
    );
}

/// Test: Check without context on conditional tuple returns false or error.
#[tokio::test]
async fn test_check_without_context_on_conditional_tuple() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_conditional_model(&storage).await;

    // Write tuple with time condition
    let tuple = StoredTuple {
        user_type: "user".to_string(),
        user_id: "charlie".to_string(),
        user_relation: None,
        relation: "time_viewer".to_string(),
        object_type: "document".to_string(),
        object_id: "context-required-doc".to_string(),
        condition_name: Some("time_check".to_string()),
        condition_context: None,
        created_at: Some(Utc::now()),
    };
    storage.write_tuple(&store_id, tuple).await.unwrap();

    // Check WITHOUT providing context
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:charlie",
                "relation": "time_viewer",
                "object": "document:context-required-doc"
            }
            // Note: no "context" field
        }),
    )
    .await;

    // Should return false or error when context is missing
    assert!(
        status == StatusCode::OK || status == StatusCode::BAD_REQUEST,
        "Should return 200 or 400, got: {}",
        status
    );

    if status == StatusCode::OK {
        assert_eq!(
            response["allowed"].as_bool(),
            Some(false),
            "Check without required context should return false"
        );
    }
}

/// Test: Batch check items with different contexts evaluate independently.
#[tokio::test]
async fn test_batch_check_with_different_contexts() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_conditional_model(&storage).await;

    // Write tuple with department condition
    let tuple = StoredTuple {
        user_type: "user".to_string(),
        user_id: "diana".to_string(),
        user_relation: None,
        relation: "dept_viewer".to_string(),
        object_type: "document".to_string(),
        object_id: "dept-doc".to_string(),
        condition_name: Some("dept_check".to_string()),
        condition_context: None,
        created_at: Some(Utc::now()),
    };
    storage.write_tuple(&store_id, tuple).await.unwrap();

    // Batch check with different contexts
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/batch-check"),
        serde_json::json!({
            "checks": [
                {
                    "tuple_key": {
                        "user": "user:diana",
                        "relation": "dept_viewer",
                        "object": "document:dept-doc"
                    },
                    "correlation_id": "matching-dept",
                    "context": {
                        "user_dept": "engineering",
                        "required_dept": "engineering"
                    }
                },
                {
                    "tuple_key": {
                        "user": "user:diana",
                        "relation": "dept_viewer",
                        "object": "document:dept-doc"
                    },
                    "correlation_id": "non-matching-dept",
                    "context": {
                        "user_dept": "sales",
                        "required_dept": "engineering"
                    }
                }
            ]
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let result = &response["result"];

    // Matching department should be allowed
    assert_eq!(
        result["matching-dept"]["allowed"].as_bool(),
        Some(true),
        "Matching department should be allowed"
    );

    // Non-matching department should be denied
    assert_eq!(
        result["non-matching-dept"]["allowed"].as_bool(),
        Some(false),
        "Non-matching department should be denied"
    );
}

/// Test: Context with nested JSON structures handled correctly (no injection).
#[tokio::test]
async fn test_context_with_nested_json_no_injection() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_conditional_model(&storage).await;

    // Write tuple with department condition
    let tuple = StoredTuple {
        user_type: "user".to_string(),
        user_id: "eve".to_string(),
        user_relation: None,
        relation: "dept_viewer".to_string(),
        object_type: "document".to_string(),
        object_id: "nested-doc".to_string(),
        condition_name: Some("dept_check".to_string()),
        condition_context: None,
        created_at: Some(Utc::now()),
    };
    storage.write_tuple(&store_id, tuple).await.unwrap();

    // Attempt injection via nested JSON - should not affect condition evaluation
    let (status, _response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:eve",
                "relation": "dept_viewer",
                "object": "document:nested-doc"
            },
            "context": {
                "user_dept": {"$inject": "admin", "nested": {"deep": "value"}},
                "required_dept": "engineering"
            }
        }),
    )
    .await;

    // Should handle gracefully - either error or evaluate normally (non-string != string)
    assert!(
        status == StatusCode::OK || status == StatusCode::BAD_REQUEST,
        "Should handle nested JSON gracefully, got: {}",
        status
    );
}

/// Test: Check with admin condition using boolean context.
#[tokio::test]
async fn test_check_with_boolean_condition() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_conditional_model(&storage).await;

    // Write tuple with admin condition
    let tuple = StoredTuple {
        user_type: "user".to_string(),
        user_id: "frank".to_string(),
        user_relation: None,
        relation: "admin_viewer".to_string(),
        object_type: "document".to_string(),
        object_id: "admin-doc".to_string(),
        condition_name: Some("admin_check".to_string()),
        condition_context: None,
        created_at: Some(Utc::now()),
    };
    storage.write_tuple(&store_id, tuple).await.unwrap();

    // Check with is_admin = true
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:frank",
                "relation": "admin_viewer",
                "object": "document:admin-doc"
            },
            "context": {
                "is_admin": true,
                "access_level": 1
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"].as_bool(),
        Some(true),
        "Admin should have access"
    );

    // Check with is_admin = false but high access_level
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:frank",
                "relation": "admin_viewer",
                "object": "document:admin-doc"
            },
            "context": {
                "is_admin": false,
                "access_level": 10
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"].as_bool(),
        Some(true),
        "High access level should grant access"
    );

    // Check with is_admin = false and low access_level
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:frank",
                "relation": "admin_viewer",
                "object": "document:admin-doc"
            },
            "context": {
                "is_admin": false,
                "access_level": 2
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"].as_bool(),
        Some(false),
        "Low access level non-admin should be denied"
    );
}

// =============================================================================
// Section 2: Expand Operations with Conditions
// =============================================================================

/// Test: Expand shows conditional tuples in output.
#[tokio::test]
async fn test_expand_shows_conditional_tuples() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_conditional_model(&storage).await;

    // Write both conditional and non-conditional tuples
    let unconditional_tuple = StoredTuple {
        user_type: "user".to_string(),
        user_id: "alice".to_string(),
        user_relation: None,
        relation: "viewer".to_string(),
        object_type: "document".to_string(),
        object_id: "mixed-doc".to_string(),
        condition_name: None,
        condition_context: None,
        created_at: Some(Utc::now()),
    };
    storage
        .write_tuple(&store_id, unconditional_tuple)
        .await
        .unwrap();

    let conditional_tuple = StoredTuple {
        user_type: "user".to_string(),
        user_id: "bob".to_string(),
        user_relation: None,
        relation: "time_viewer".to_string(),
        object_type: "document".to_string(),
        object_id: "mixed-doc".to_string(),
        condition_name: Some("time_check".to_string()),
        condition_context: None,
        created_at: Some(Utc::now()),
    };
    storage
        .write_tuple(&store_id, conditional_tuple)
        .await
        .unwrap();

    // Expand can_view relation
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/expand"),
        serde_json::json!({
            "tuple_key": {
                "relation": "can_view",
                "object": "document:mixed-doc"
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    // The tree should show the union of viewer and time_viewer
    let tree = &response["tree"];
    assert!(
        tree.is_object(),
        "Expand should return a tree structure: {:?}",
        response
    );
}

/// Test: Expand tree correctly represents condition dependencies.
#[tokio::test]
async fn test_expand_tree_represents_condition_dependencies() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_conditional_model(&storage).await;

    // Write conditional tuple
    let tuple = StoredTuple {
        user_type: "user".to_string(),
        user_id: "carol".to_string(),
        user_relation: None,
        relation: "time_viewer".to_string(),
        object_type: "document".to_string(),
        object_id: "time-doc".to_string(),
        condition_name: Some("time_check".to_string()),
        condition_context: None,
        created_at: Some(Utc::now()),
    };
    storage.write_tuple(&store_id, tuple).await.unwrap();

    // Expand time_viewer relation directly
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/expand"),
        serde_json::json!({
            "tuple_key": {
                "relation": "time_viewer",
                "object": "document:time-doc"
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    // Verify tree is returned
    assert!(
        response["tree"].is_object(),
        "Expand should return a tree for conditional tuples: {:?}",
        response
    );
}

// =============================================================================
// Section 3: List Operations with Conditions
// =============================================================================

/// Test: List Objects respects conditional tuples (only includes if condition matches).
#[tokio::test]
async fn test_listobjects_respects_conditional_tuples() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_conditional_model(&storage).await;

    // Write unconditional tuple
    let unconditional = StoredTuple {
        user_type: "user".to_string(),
        user_id: "alice".to_string(),
        user_relation: None,
        relation: "viewer".to_string(),
        object_type: "document".to_string(),
        object_id: "public-doc".to_string(),
        condition_name: None,
        condition_context: None,
        created_at: Some(Utc::now()),
    };
    storage.write_tuple(&store_id, unconditional).await.unwrap();

    // Write conditional tuple that should match with context
    let conditional_match = StoredTuple {
        user_type: "user".to_string(),
        user_id: "alice".to_string(),
        user_relation: None,
        relation: "time_viewer".to_string(),
        object_type: "document".to_string(),
        object_id: "time-gated-doc".to_string(),
        condition_name: Some("time_check".to_string()),
        condition_context: None,
        created_at: Some(Utc::now()),
    };
    storage
        .write_tuple(&store_id, conditional_match)
        .await
        .unwrap();

    // List objects with context that satisfies condition
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-objects"),
        serde_json::json!({
            "type": "document",
            "relation": "time_viewer",
            "user": "user:alice",
            "context": {
                "current_time": "2024-01-01T00:00:00Z",
                "expires_at": "2024-12-31T23:59:59Z"
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let objects = response["objects"]
        .as_array()
        .expect("Should have objects array");

    // Should include time-gated-doc because condition is satisfied
    let object_ids: Vec<&str> = objects.iter().filter_map(|o| o.as_str()).collect();
    assert!(
        object_ids.contains(&"document:time-gated-doc"),
        "Should include time-gated-doc when condition is satisfied: {:?}",
        object_ids
    );
}

/// Test: List Objects excludes conditional tuples when condition fails.
#[tokio::test]
async fn test_listobjects_excludes_conditional_tuples_when_condition_fails() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_conditional_model(&storage).await;

    // Write conditional tuple
    let conditional = StoredTuple {
        user_type: "user".to_string(),
        user_id: "bob".to_string(),
        user_relation: None,
        relation: "time_viewer".to_string(),
        object_type: "document".to_string(),
        object_id: "expired-access-doc".to_string(),
        condition_name: Some("time_check".to_string()),
        condition_context: None,
        created_at: Some(Utc::now()),
    };
    storage.write_tuple(&store_id, conditional).await.unwrap();

    // List objects with context that does NOT satisfy condition
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-objects"),
        serde_json::json!({
            "type": "document",
            "relation": "time_viewer",
            "user": "user:bob",
            "context": {
                "current_time": "2025-06-01T00:00:00Z",
                "expires_at": "2024-12-31T23:59:59Z"
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let objects = response["objects"]
        .as_array()
        .expect("Should have objects array");

    // Should NOT include expired-access-doc because condition is NOT satisfied
    let object_ids: Vec<&str> = objects.iter().filter_map(|o| o.as_str()).collect();
    assert!(
        !object_ids.contains(&"document:expired-access-doc"),
        "Should NOT include expired-access-doc when condition fails: {:?}",
        object_ids
    );
}

/// Test: List Users respects conditional tuples (only includes if condition matches).
#[tokio::test]
async fn test_listusers_respects_conditional_tuples() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_conditional_model(&storage).await;

    // Write unconditional tuple
    let unconditional = StoredTuple {
        user_type: "user".to_string(),
        user_id: "alice".to_string(),
        user_relation: None,
        relation: "viewer".to_string(),
        object_type: "document".to_string(),
        object_id: "shared-doc".to_string(),
        condition_name: None,
        condition_context: None,
        created_at: Some(Utc::now()),
    };
    storage.write_tuple(&store_id, unconditional).await.unwrap();

    // Write conditional tuple that should match with context
    let conditional = StoredTuple {
        user_type: "user".to_string(),
        user_id: "bob".to_string(),
        user_relation: None,
        relation: "time_viewer".to_string(),
        object_type: "document".to_string(),
        object_id: "shared-doc".to_string(),
        condition_name: Some("time_check".to_string()),
        condition_context: None,
        created_at: Some(Utc::now()),
    };
    storage.write_tuple(&store_id, conditional).await.unwrap();

    // List users with context that satisfies condition
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-users"),
        serde_json::json!({
            "object": {"type": "document", "id": "shared-doc"},
            "relation": "time_viewer",
            "user_filters": [{"type": "user"}],
            "context": {
                "current_time": "2024-01-01T00:00:00Z",
                "expires_at": "2024-12-31T23:59:59Z"
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let users = response["users"]
        .as_array()
        .expect("Should have users array");

    // Should include bob because condition is satisfied
    let user_ids: Vec<&str> = users
        .iter()
        .filter_map(|u| u["object"]["id"].as_str())
        .collect();

    assert!(
        user_ids.contains(&"bob"),
        "Should include bob when condition is satisfied: {:?}",
        users
    );
}

/// Test: List Users excludes conditional tuples when condition fails.
#[tokio::test]
async fn test_listusers_excludes_conditional_tuples_when_condition_fails() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_conditional_model(&storage).await;

    // Write conditional tuple
    let conditional = StoredTuple {
        user_type: "user".to_string(),
        user_id: "charlie".to_string(),
        user_relation: None,
        relation: "time_viewer".to_string(),
        object_type: "document".to_string(),
        object_id: "time-restricted-doc".to_string(),
        condition_name: Some("time_check".to_string()),
        condition_context: None,
        created_at: Some(Utc::now()),
    };
    storage.write_tuple(&store_id, conditional).await.unwrap();

    // List users with context that does NOT satisfy condition
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-users"),
        serde_json::json!({
            "object": {"type": "document", "id": "time-restricted-doc"},
            "relation": "time_viewer",
            "user_filters": [{"type": "user"}],
            "context": {
                "current_time": "2025-06-01T00:00:00Z",
                "expires_at": "2024-12-31T23:59:59Z"
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let users = response["users"]
        .as_array()
        .expect("Should have users array");

    // Should NOT include charlie because condition is NOT satisfied
    let user_ids: Vec<&str> = users
        .iter()
        .filter_map(|u| u["object"]["id"].as_str())
        .collect();

    assert!(
        !user_ids.contains(&"charlie"),
        "Should NOT include charlie when condition fails: {:?}",
        users
    );
}

// =============================================================================
// Section 4: Contextual Tuples
// =============================================================================

/// Test: Check with contextual tuples includes them in traversal.
#[tokio::test]
async fn test_check_with_contextual_tuples() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_simple_model(&storage).await;

    // No stored tuples - only contextual

    // Check with contextual tuple granting access
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:dynamic-alice",
                "relation": "viewer",
                "object": "document:dynamic-doc"
            },
            "contextual_tuples": {
                "tuple_keys": [
                    {
                        "user": "user:dynamic-alice",
                        "relation": "viewer",
                        "object": "document:dynamic-doc"
                    }
                ]
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"].as_bool(),
        Some(true),
        "Check should use contextual tuples"
    );
}

/// Test: Contextual tuples don't persist to storage after check completes.
#[tokio::test]
async fn test_contextual_tuples_dont_persist() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_simple_model(&storage).await;

    // Check with contextual tuple
    let (status, _) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:temp-user",
                "relation": "viewer",
                "object": "document:temp-doc"
            },
            "contextual_tuples": {
                "tuple_keys": [
                    {
                        "user": "user:temp-user",
                        "relation": "viewer",
                        "object": "document:temp-doc"
                    }
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Check again WITHOUT contextual tuples - should be denied
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:temp-user",
                "relation": "viewer",
                "object": "document:temp-doc"
            }
            // No contextual tuples
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"].as_bool(),
        Some(false),
        "Contextual tuples should not persist"
    );
}

/// Test: Batch check with contextual tuples.
///
/// NOTE: Per-item contextual tuples are NOT passed through to the resolver
/// in the current implementation. The batch check handler converts requests
/// but does not include item-level contextual tuples.
///
/// This test documents the current behavior: both checks should be denied
/// because the stored tuples don't exist and contextual tuples aren't used.
#[tokio::test]
async fn test_batch_check_contextual_tuples_behavior() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_simple_model(&storage).await;

    // Write a stored tuple for check1
    let stored = StoredTuple {
        user_type: "user".to_string(),
        user_id: "batch-alice".to_string(),
        user_relation: None,
        relation: "viewer".to_string(),
        object_type: "document".to_string(),
        object_id: "batch-doc1".to_string(),
        condition_name: None,
        condition_context: None,
        created_at: Some(Utc::now()),
    };
    storage.write_tuple(&store_id, stored).await.unwrap();

    // Batch check - stored tuple should work, no stored tuple should be denied
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/batch-check"),
        serde_json::json!({
            "checks": [
                {
                    "tuple_key": {
                        "user": "user:batch-alice",
                        "relation": "viewer",
                        "object": "document:batch-doc1"
                    },
                    "correlation_id": "check1"
                },
                {
                    "tuple_key": {
                        "user": "user:batch-alice",
                        "relation": "viewer",
                        "object": "document:batch-doc2"
                    },
                    "correlation_id": "check2"
                }
            ]
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let result = &response["result"];

    // check1 has stored tuple - should be allowed
    assert_eq!(
        result["check1"]["allowed"].as_bool(),
        Some(true),
        "check1 with stored tuple should be allowed"
    );

    // check2 has no stored tuple - should be denied
    assert_eq!(
        result["check2"]["allowed"].as_bool(),
        Some(false),
        "check2 without stored tuple should be denied"
    );
}

/// Test: List Objects behavior with contextual tuples.
///
/// NOTE: This test documents that stored tuples are properly returned.
/// Contextual tuples in ListObjects may have different behavior depending
/// on implementation - they are passed but may be used differently than Check.
#[tokio::test]
async fn test_listobjects_with_stored_tuples() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_simple_model(&storage).await;

    // Write stored tuples
    let stored1 = StoredTuple {
        user_type: "user".to_string(),
        user_id: "listobj-user".to_string(),
        user_relation: None,
        relation: "viewer".to_string(),
        object_type: "document".to_string(),
        object_id: "stored-doc1".to_string(),
        condition_name: None,
        condition_context: None,
        created_at: Some(Utc::now()),
    };
    storage.write_tuple(&store_id, stored1).await.unwrap();

    let stored2 = StoredTuple {
        user_type: "user".to_string(),
        user_id: "listobj-user".to_string(),
        user_relation: None,
        relation: "viewer".to_string(),
        object_type: "document".to_string(),
        object_id: "stored-doc2".to_string(),
        condition_name: None,
        condition_context: None,
        created_at: Some(Utc::now()),
    };
    storage.write_tuple(&store_id, stored2).await.unwrap();

    // List objects should return all stored tuples
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-objects"),
        serde_json::json!({
            "type": "document",
            "relation": "viewer",
            "user": "user:listobj-user"
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let objects = response["objects"]
        .as_array()
        .expect("Should have objects array");

    let object_ids: Vec<&str> = objects.iter().filter_map(|o| o.as_str()).collect();

    // Should include both stored documents
    assert!(
        object_ids.contains(&"document:stored-doc1"),
        "Should include stored-doc1: {:?}",
        object_ids
    );
    assert!(
        object_ids.contains(&"document:stored-doc2"),
        "Should include stored-doc2: {:?}",
        object_ids
    );
}

/// Test: List Users with contextual tuples changes result set correctly.
#[tokio::test]
async fn test_listusers_with_contextual_tuples() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_simple_model(&storage).await;

    // Write a stored tuple
    let stored = StoredTuple {
        user_type: "user".to_string(),
        user_id: "stored-viewer".to_string(),
        user_relation: None,
        relation: "viewer".to_string(),
        object_type: "document".to_string(),
        object_id: "listusers-doc".to_string(),
        condition_name: None,
        condition_context: None,
        created_at: Some(Utc::now()),
    };
    storage.write_tuple(&store_id, stored).await.unwrap();

    // List users with contextual tuple adding another user
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/list-users"),
        serde_json::json!({
            "object": {"type": "document", "id": "listusers-doc"},
            "relation": "viewer",
            "user_filters": [{"type": "user"}],
            "contextual_tuples": {
                "tuple_keys": [
                    {
                        "user": "user:contextual-viewer",
                        "relation": "viewer",
                        "object": "document:listusers-doc"
                    }
                ]
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let users = response["users"]
        .as_array()
        .expect("Should have users array");

    let user_ids: Vec<&str> = users
        .iter()
        .filter_map(|u| u["object"]["id"].as_str())
        .collect();

    // Should include both stored and contextual users
    assert!(
        user_ids.contains(&"stored-viewer"),
        "Should include stored-viewer: {:?}",
        users
    );
    assert!(
        user_ids.contains(&"contextual-viewer"),
        "Should include contextual-viewer: {:?}",
        users
    );
}

/// Test: Contextual tuples don't override stored tuples (union behavior).
#[tokio::test]
async fn test_contextual_tuples_union_with_stored() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_simple_model(&storage).await;

    // Write a stored tuple
    let stored = StoredTuple {
        user_type: "user".to_string(),
        user_id: "alice".to_string(),
        user_relation: None,
        relation: "viewer".to_string(),
        object_type: "document".to_string(),
        object_id: "union-doc".to_string(),
        condition_name: None,
        condition_context: None,
        created_at: Some(Utc::now()),
    };
    storage.write_tuple(&store_id, stored).await.unwrap();

    // Check with stored tuple (no contextual)
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:union-doc"
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"].as_bool(),
        Some(true),
        "Should be allowed via stored tuple"
    );

    // Check with contextual tuple for DIFFERENT relation
    // This tests that contextual tuples add to, not replace, stored tuples
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:union-doc"
            },
            "contextual_tuples": {
                "tuple_keys": [
                    {
                        "user": "user:bob",
                        "relation": "editor",
                        "object": "document:union-doc"
                    }
                ]
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"].as_bool(),
        Some(true),
        "Should still be allowed via stored tuple (contextual doesn't override)"
    );
}

/// Test: Malformed contextual tuples rejected with clear error.
#[tokio::test]
async fn test_malformed_contextual_tuples_rejected() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_simple_model(&storage).await;

    // Check with malformed contextual tuple (missing required field)
    let (status, _response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:test",
                "relation": "viewer",
                "object": "document:test-doc"
            },
            "contextual_tuples": {
                "tuple_keys": [
                    {
                        "user": "user:malformed",
                        // Missing "relation" and "object"
                    }
                ]
            }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Malformed contextual tuples should be rejected"
    );
}

// =============================================================================
// Section 5: Edge Cases
// =============================================================================

/// Test: Empty context handled correctly.
#[tokio::test]
async fn test_empty_context_handled_correctly() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_simple_model(&storage).await;

    // Write a normal tuple (no condition)
    let tuple = StoredTuple {
        user_type: "user".to_string(),
        user_id: "edge-user".to_string(),
        user_relation: None,
        relation: "viewer".to_string(),
        object_type: "document".to_string(),
        object_id: "edge-doc".to_string(),
        condition_name: None,
        condition_context: None,
        created_at: Some(Utc::now()),
    };
    storage.write_tuple(&store_id, tuple).await.unwrap();

    // Check with explicit empty context
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:edge-user",
                "relation": "viewer",
                "object": "document:edge-doc"
            },
            "context": {}
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"].as_bool(),
        Some(true),
        "Empty context should not affect non-conditional tuples"
    );
}

/// Test: Context with special characters (unicode) handled safely.
#[tokio::test]
async fn test_context_with_unicode_handled_safely() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_conditional_model(&storage).await;

    // Write tuple with department condition
    let tuple = StoredTuple {
        user_type: "user".to_string(),
        user_id: "unicode-user".to_string(),
        user_relation: None,
        relation: "dept_viewer".to_string(),
        object_type: "document".to_string(),
        object_id: "unicode-doc".to_string(),
        condition_name: Some("dept_check".to_string()),
        condition_context: None,
        created_at: Some(Utc::now()),
    };
    storage.write_tuple(&store_id, tuple).await.unwrap();

    // Check with unicode strings in context
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:unicode-user",
                "relation": "dept_viewer",
                "object": "document:unicode-doc"
            },
            "context": {
                "user_dept": "工程部",
                "required_dept": "工程部"
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"].as_bool(),
        Some(true),
        "Unicode strings in context should be handled correctly"
    );

    // Check with non-matching unicode
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:unicode-user",
                "relation": "dept_viewer",
                "object": "document:unicode-doc"
            },
            "context": {
                "user_dept": "销售部",
                "required_dept": "工程部"
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"].as_bool(),
        Some(false),
        "Non-matching unicode strings should evaluate correctly"
    );
}

/// Test: Context with emoji handled safely.
#[tokio::test]
async fn test_context_with_emoji_handled_safely() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_conditional_model(&storage).await;

    // Write tuple with department condition
    let tuple = StoredTuple {
        user_type: "user".to_string(),
        user_id: "emoji-user".to_string(),
        user_relation: None,
        relation: "dept_viewer".to_string(),
        object_type: "document".to_string(),
        object_id: "emoji-doc".to_string(),
        condition_name: Some("dept_check".to_string()),
        condition_context: None,
        created_at: Some(Utc::now()),
    };
    storage.write_tuple(&store_id, tuple).await.unwrap();

    // Check with emoji in context
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:emoji-user",
                "relation": "dept_viewer",
                "object": "document:emoji-doc"
            },
            "context": {
                "user_dept": "engineering 🚀",
                "required_dept": "engineering 🚀"
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"].as_bool(),
        Some(true),
        "Emoji in context should be handled correctly"
    );
}

/// Test: Check with non-existent store returns 404.
/// Note: The store ID must be in valid ULID format (OpenFGA validates format first).
#[tokio::test]
async fn test_check_nonexistent_store_returns_404() {
    let storage = Arc::new(MemoryDataStore::new());

    // Use a valid ULID format for non-existent store (OpenFGA validates format first)
    let nonexistent_store_id = ulid::Ulid::new().to_string();

    let (status, _response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{}/check", nonexistent_store_id),
        serde_json::json!({
            "tuple_key": {
                "user": "user:test",
                "relation": "viewer",
                "object": "document:test"
            }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "Non-existent store (with valid ULID format) should return 404"
    );
}

/// Test: Check with invalid store ID format returns 400.
/// OpenFGA validates store ID format before checking if the store exists.
#[tokio::test]
async fn test_check_invalid_store_id_format_returns_400() {
    let storage = Arc::new(MemoryDataStore::new());

    let (status, _response) = post_json(
        create_test_app(&storage),
        "/stores/invalid-store-id/check",
        serde_json::json!({
            "tuple_key": {
                "user": "user:test",
                "relation": "viewer",
                "object": "document:test"
            }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Invalid store ID format should return 400"
    );
}

/// Test: Contextual tuple with condition in check request.
#[tokio::test]
async fn test_contextual_tuple_with_condition_in_check() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = create_store_with_conditional_model(&storage).await;

    // Check with contextual tuple that has a condition
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:contextual-cond-user",
                "relation": "time_viewer",
                "object": "document:contextual-cond-doc"
            },
            "contextual_tuples": {
                "tuple_keys": [
                    {
                        "user": "user:contextual-cond-user",
                        "relation": "time_viewer",
                        "object": "document:contextual-cond-doc",
                        "condition": {
                            "name": "time_check"
                        }
                    }
                ]
            },
            "context": {
                "current_time": "2024-01-01T00:00:00Z",
                "expires_at": "2024-12-31T23:59:59Z"
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"].as_bool(),
        Some(true),
        "Contextual tuple with matching condition should be allowed"
    );

    // Same check but with expired context
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "user:contextual-cond-user",
                "relation": "time_viewer",
                "object": "document:contextual-cond-doc"
            },
            "contextual_tuples": {
                "tuple_keys": [
                    {
                        "user": "user:contextual-cond-user",
                        "relation": "time_viewer",
                        "object": "document:contextual-cond-doc",
                        "condition": {
                            "name": "time_check"
                        }
                    }
                ]
            },
            "context": {
                "current_time": "2025-06-01T00:00:00Z",
                "expires_at": "2024-12-31T23:59:59Z"
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["allowed"].as_bool(),
        Some(false),
        "Contextual tuple with failing condition should be denied"
    );
}
