mod common;

use anyhow::Result;
use common::{
    create_authorization_model, create_test_model, create_test_store, get_openfga_url, write_tuples,
};
use serde_json::json;
use std::collections::HashSet;

/// Test: POST /stores/{store_id}/list-objects returns objects user can access
#[tokio::test]
async fn test_listobjects_returns_accessible_objects() -> Result<()> {
    // Arrange: Create store, model, and write tuples
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    write_tuples(
        &store_id,
        vec![
            ("user:alice", "viewer", "document:doc1"),
            ("user:alice", "viewer", "document:doc2"),
            ("user:bob", "viewer", "document:doc3"),
        ],
    )
    .await?;

    let client = reqwest::Client::new();

    // Act: List objects alice can view
    let list_request = json!({
        "type": "document",
        "relation": "viewer",
        "user": "user:alice"
    });

    let response = client
        .post(format!(
            "{}/stores/{}/list-objects",
            get_openfga_url(),
            store_id
        ))
        .json(&list_request)
        .send()
        .await?;

    // Assert: ListObjects succeeded
    assert!(
        response.status().is_success(),
        "ListObjects should succeed, got: {}",
        response.status()
    );

    let response_body: serde_json::Value = response.json().await?;

    // Verify response has 'objects' array
    let objects = response_body
        .get("objects")
        .and_then(|o| o.as_array())
        .expect("Response should have 'objects' array");

    // Convert to HashSet for robust, order-independent comparison
    let object_ids: HashSet<String> = objects
        .iter()
        .filter_map(|o| o.as_str())
        .map(String::from)
        .collect();

    let expected_ids: HashSet<String> = ["document:doc1".to_string(), "document:doc2".to_string()]
        .into_iter()
        .collect();

    assert_eq!(
        object_ids, expected_ids,
        "Alice should have access to exactly doc1 and doc2"
    );

    Ok(())
}

/// Test: ListObjects with direct relations returns correct objects
#[tokio::test]
async fn test_listobjects_with_direct_relations() -> Result<()> {
    // Arrange: Create store, model, and write direct relation tuples
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    write_tuples(
        &store_id,
        vec![
            ("user:charlie", "editor", "document:doc1"),
            ("user:charlie", "editor", "document:doc2"),
            ("user:dave", "editor", "document:doc3"),
        ],
    )
    .await?;

    let client = reqwest::Client::new();

    // Act: List objects charlie can edit
    let list_request = json!({
        "type": "document",
        "relation": "editor",
        "user": "user:charlie"
    });

    let response = client
        .post(format!(
            "{}/stores/{}/list-objects",
            get_openfga_url(),
            store_id
        ))
        .json(&list_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;

    let objects = response_body
        .get("objects")
        .and_then(|o| o.as_array())
        .expect("Response should have 'objects' array");

    // Charlie should have access to exactly doc1 and doc2 (not doc3)
    let object_ids: HashSet<String> = objects
        .iter()
        .filter_map(|o| o.as_str())
        .map(String::from)
        .collect();

    let expected_ids: HashSet<String> = ["document:doc1".to_string(), "document:doc2".to_string()]
        .into_iter()
        .collect();

    assert_eq!(
        object_ids, expected_ids,
        "Charlie should have access to exactly doc1 and doc2 as editor"
    );

    Ok(())
}

/// Test: ListObjects with computed relations returns correct objects
#[tokio::test]
async fn test_listobjects_with_computed_relations() -> Result<()> {
    // Arrange: Create store with computed relation model
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

    let _model_id = create_authorization_model(&store_id, model).await?;

    // Write tuples: eve is viewer of doc1, editor of doc2
    write_tuples(
        &store_id,
        vec![
            ("user:eve", "viewer", "document:doc1"),
            ("user:eve", "editor", "document:doc2"),
        ],
    )
    .await?;

    let client = reqwest::Client::new();

    // Act: List objects eve can_access (union of viewer and editor)
    let list_request = json!({
        "type": "document",
        "relation": "can_access",
        "user": "user:eve"
    });

    let response = client
        .post(format!(
            "{}/stores/{}/list-objects",
            get_openfga_url(),
            store_id
        ))
        .json(&list_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;

    let objects = response_body
        .get("objects")
        .and_then(|o| o.as_array())
        .expect("Response should have 'objects' array");

    // Eve should have access to exactly doc1 (viewer) and doc2 (editor) via can_access union
    let object_ids: HashSet<String> = objects
        .iter()
        .filter_map(|o| o.as_str())
        .map(String::from)
        .collect();

    let expected_ids: HashSet<String> = ["document:doc1".to_string(), "document:doc2".to_string()]
        .into_iter()
        .collect();

    assert_eq!(
        object_ids, expected_ids,
        "Eve should have access to exactly doc1 and doc2 via union"
    );

    Ok(())
}

/// Test: ListObjects with type filter limits results to specified type
#[tokio::test]
async fn test_listobjects_with_type_filter() -> Result<()> {
    // Arrange: Create store and model with multiple types
    let store_id = create_test_store().await?;

    // Create model with document and folder types
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
                "type": "folder",
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

    // Write tuples for both documents and folders
    write_tuples(
        &store_id,
        vec![
            ("user:frank", "viewer", "document:doc1"),
            ("user:frank", "viewer", "document:doc2"),
            ("user:frank", "viewer", "folder:folder1"),
            ("user:frank", "viewer", "folder:folder2"),
        ],
    )
    .await?;

    let client = reqwest::Client::new();

    // Act: List ONLY documents frank can view (type filter)
    let list_request = json!({
        "type": "document",
        "relation": "viewer",
        "user": "user:frank"
    });

    let response = client
        .post(format!(
            "{}/stores/{}/list-objects",
            get_openfga_url(),
            store_id
        ))
        .json(&list_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;

    let objects = response_body
        .get("objects")
        .and_then(|o| o.as_array())
        .expect("Response should have 'objects' array");

    // Should return exactly 2 documents (not folders)
    let object_ids: HashSet<String> = objects
        .iter()
        .filter_map(|o| o.as_str())
        .map(String::from)
        .collect();

    // All results should be documents (type filter worked)
    for object_id in &object_ids {
        assert!(
            object_id.starts_with("document:"),
            "All results should be documents, got: {}",
            object_id
        );
    }

    let expected_ids: HashSet<String> = ["document:doc1".to_string(), "document:doc2".to_string()]
        .into_iter()
        .collect();

    assert_eq!(
        object_ids, expected_ids,
        "Should return exactly doc1 and doc2 (filtered by type)"
    );

    Ok(())
}

/// Test: ListObjects with no accessible objects returns empty array
#[tokio::test]
async fn test_listobjects_with_no_accessible_objects() -> Result<()> {
    // Arrange: Create store and model, but DON'T write any tuples for user
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    // Write tuples for other users, but not for george
    write_tuples(
        &store_id,
        vec![
            ("user:alice", "viewer", "document:doc1"),
            ("user:bob", "viewer", "document:doc2"),
        ],
    )
    .await?;

    let client = reqwest::Client::new();

    // Act: List objects george can view (should be none)
    let list_request = json!({
        "type": "document",
        "relation": "viewer",
        "user": "user:george"
    });

    let response = client
        .post(format!(
            "{}/stores/{}/list-objects",
            get_openfga_url(),
            store_id
        ))
        .json(&list_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;

    // Assert: Should return empty array
    let objects = response_body
        .get("objects")
        .and_then(|o| o.as_array())
        .expect("Response should have 'objects' array");

    assert_eq!(
        objects.len(),
        0,
        "User with no access should get empty array, got: {} objects",
        objects.len()
    );

    Ok(())
}

/// Test: ListObjects with contextual_tuples considers temporary permissions
#[tokio::test]
async fn test_listobjects_with_contextual_tuples() -> Result<()> {
    // Arrange: Create store and model, but DON'T write permanent tuples
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    let client = reqwest::Client::new();

    // Act: List objects with contextual tuple (temporary permission)
    let list_request = json!({
        "type": "document",
        "relation": "viewer",
        "user": "user:helen",
        "contextual_tuples": {
            "tuple_keys": [
                {
                    "user": "user:helen",
                    "relation": "viewer",
                    "object": "document:doc1"
                },
                {
                    "user": "user:helen",
                    "relation": "viewer",
                    "object": "document:doc2"
                }
            ]
        }
    });

    let response = client
        .post(format!(
            "{}/stores/{}/list-objects",
            get_openfga_url(),
            store_id
        ))
        .json(&list_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;

    // Assert: Should return exactly the objects from contextual tuples
    let objects = response_body
        .get("objects")
        .and_then(|o| o.as_array())
        .expect("Response should have 'objects' array");

    let object_ids: HashSet<String> = objects
        .iter()
        .filter_map(|o| o.as_str())
        .map(String::from)
        .collect();

    let expected_ids: HashSet<String> = ["document:doc1".to_string(), "document:doc2".to_string()]
        .into_iter()
        .collect();

    assert_eq!(
        object_ids, expected_ids,
        "Should return exactly the objects from contextual tuples"
    );

    Ok(())
}

/// Test: ListObjects with wildcards returns all matching objects
#[tokio::test]
async fn test_listobjects_with_wildcards() -> Result<()> {
    // Arrange: Create store with wildcard support model
    let store_id = create_test_store().await?;

    // Create model that supports wildcard users
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
                                {"type": "user"},
                                {"type": "user", "wildcard": {}}
                            ]
                        }
                    }
                }
            }
        ]
    });

    let _model_id = create_authorization_model(&store_id, model).await?;

    // Write tuples with wildcard (public access)
    write_tuples(
        &store_id,
        vec![
            ("user:*", "viewer", "document:public1"),
            ("user:*", "viewer", "document:public2"),
            ("user:ivan", "viewer", "document:private1"),
        ],
    )
    .await?;

    let client = reqwest::Client::new();

    // Act: List objects ANY user can view (including wildcard grants)
    // Note: Specific user should see both wildcard and direct grants
    let list_request = json!({
        "type": "document",
        "relation": "viewer",
        "user": "user:judy"  // Judy has no direct grants, but gets wildcard access
    });

    let response = client
        .post(format!(
            "{}/stores/{}/list-objects",
            get_openfga_url(),
            store_id
        ))
        .json(&list_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;

    let objects = response_body
        .get("objects")
        .and_then(|o| o.as_array())
        .expect("Response should have 'objects' array");

    // Judy should see exactly wildcard-granted objects (public1, public2)
    let object_ids: HashSet<String> = objects
        .iter()
        .filter_map(|o| o.as_str())
        .map(String::from)
        .collect();

    let expected_ids: HashSet<String> = [
        "document:public1".to_string(),
        "document:public2".to_string(),
    ]
    .into_iter()
    .collect();

    assert_eq!(
        object_ids, expected_ids,
        "User 'judy' should have access to exactly 2 wildcard-granted objects (public1 and public2)"
    );

    Ok(())
}
