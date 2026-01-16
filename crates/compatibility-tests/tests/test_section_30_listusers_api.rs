mod common;

use anyhow::Result;
use common::{
    create_authorization_model, create_test_store, get_openfga_url, shared_client, write_tuples,
};
use reqwest::StatusCode;
use serde_json::json;
use std::collections::HashSet;

// ============================================================================
// Section 30: ListUsers API Tests
// ============================================================================
//
// The ListUsers API returns all users of a given type that have a specified
// relationship with an object. This is the inverse of ListObjects.
//
// IMPORTANT: ListUsers is an experimental API in OpenFGA v1.5.4+
// It requires the flag: --experimentals enable-list-users
//
// These tests may be skipped if the OpenFGA instance doesn't have
// ListUsers enabled.
//
// ============================================================================

/// Helper to check if ListUsers API is available
async fn is_listusers_available(store_id: &str) -> bool {
    let client = shared_client();
    let request = json!({
        "object": { "type": "document", "id": "test" },
        "relation": "viewer",
        "user_filters": [{ "type": "user" }]
    });

    let response = client
        .post(format!(
            "{}/stores/{}/list-users",
            get_openfga_url(),
            store_id
        ))
        .json(&request)
        .send()
        .await;

    match response {
        Ok(resp) => {
            let status = resp.status();
            // API is available if:
            // - Request succeeds (2xx)
            // - Returns BAD_REQUEST (400) - validation failed but endpoint exists
            // - Returns UNPROCESSABLE_ENTITY (422) - validation failed but endpoint exists
            // API is NOT available if:
            // - Returns NOT_IMPLEMENTED (501) - feature not enabled
            // - Returns NOT_FOUND (404) - endpoint doesn't exist
            status.is_success()
                || status == StatusCode::BAD_REQUEST
                || status == StatusCode::UNPROCESSABLE_ENTITY
        }
        Err(_) => false,
    }
}

/// Validate the ListUsers response format according to OpenFGA API specification.
///
/// The response must have:
/// - `users` array (required)
/// - Each user must have either:
///   - `object` field with `type` and `id` (for direct users)
///   - `userset` field with `type`, `id`, and `relation` (for group references)
///   - `wildcard` field with `type` (for wildcard matches)
///
/// Returns the validated users array or panics with descriptive error.
fn validate_listusers_response(response_body: &serde_json::Value) -> &Vec<serde_json::Value> {
    // Validate root structure has 'users' array
    let users = response_body
        .get("users")
        .expect("ListUsers response must have 'users' field")
        .as_array()
        .expect("'users' field must be an array");

    // Validate each user has the required structure
    for (i, user) in users.iter().enumerate() {
        let has_object = user.get("object").is_some();
        let has_userset = user.get("userset").is_some();
        let has_wildcard = user.get("wildcard").is_some();

        assert!(
            has_object || has_userset || has_wildcard,
            "User at index {} must have 'object', 'userset', or 'wildcard' field. Got: {}",
            i,
            user
        );

        // Validate object format if present
        if let Some(object) = user.get("object") {
            assert!(
                object.get("type").and_then(|t| t.as_str()).is_some(),
                "User at index {} 'object' field missing 'type'. Got: {}",
                i,
                object
            );
            assert!(
                object.get("id").and_then(|id| id.as_str()).is_some(),
                "User at index {} 'object' field missing 'id'. Got: {}",
                i,
                object
            );
        }

        // Validate userset format if present
        if let Some(userset) = user.get("userset") {
            assert!(
                userset.get("type").and_then(|t| t.as_str()).is_some(),
                "User at index {} 'userset' field missing 'type'. Got: {}",
                i,
                userset
            );
            assert!(
                userset.get("id").and_then(|id| id.as_str()).is_some(),
                "User at index {} 'userset' field missing 'id'. Got: {}",
                i,
                userset
            );
            assert!(
                userset.get("relation").and_then(|r| r.as_str()).is_some(),
                "User at index {} 'userset' field missing 'relation'. Got: {}",
                i,
                userset
            );
        }

        // Validate wildcard format if present
        if let Some(wildcard) = user.get("wildcard") {
            assert!(
                wildcard.get("type").and_then(|t| t.as_str()).is_some(),
                "User at index {} 'wildcard' field missing 'type'. Got: {}",
                i,
                wildcard
            );
        }
    }

    users
}

/// Test: POST /stores/{store_id}/list-users returns users with direct relation
#[tokio::test]
async fn test_listusers_returns_users_with_direct_relation() -> Result<()> {
    // Arrange: Create store, model, and write tuples
    let store_id = create_test_store().await?;

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

    let _model_id = create_authorization_model(&store_id, model).await?;

    // Check if ListUsers is available
    if !is_listusers_available(&store_id).await {
        eprintln!(
            "SKIPPED: ListUsers API not available (requires --experimentals enable-list-users)"
        );
        return Ok(());
    }

    write_tuples(
        &store_id,
        vec![
            ("user:alice", "viewer", "document:doc1"),
            ("user:bob", "viewer", "document:doc1"),
            ("user:charlie", "viewer", "document:doc2"),
        ],
    )
    .await?;

    let client = shared_client();

    // Act: List users who are viewers of doc1
    let list_request = json!({
        "object": { "type": "document", "id": "doc1" },
        "relation": "viewer",
        "user_filters": [{ "type": "user" }]
    });

    let response = client
        .post(format!(
            "{}/stores/{}/list-users",
            get_openfga_url(),
            store_id
        ))
        .json(&list_request)
        .send()
        .await?;

    // Assert: ListUsers succeeded
    assert!(
        response.status().is_success(),
        "ListUsers should succeed, got: {}",
        response.status()
    );

    let response_body: serde_json::Value = response.json().await?;

    // Validate response format according to OpenFGA API specification
    let users = validate_listusers_response(&response_body);

    // Extract user IDs from response
    let user_ids: HashSet<String> = users
        .iter()
        .filter_map(|u| {
            u.get("object").and_then(|o| {
                let user_type = o.get("type")?.as_str()?;
                let user_id = o.get("id")?.as_str()?;
                Some(format!("{}:{}", user_type, user_id))
            })
        })
        .collect();

    let expected: HashSet<String> = ["user:alice", "user:bob"]
        .iter()
        .map(|s| s.to_string())
        .collect();

    assert_eq!(
        user_ids, expected,
        "Should return exactly alice and bob as viewers of doc1"
    );

    Ok(())
}

/// Test: ListUsers with userset filter returns group members
#[tokio::test]
async fn test_listusers_with_userset_filter() -> Result<()> {
    // Arrange: Create store with group membership model
    let store_id = create_test_store().await?;

    let model = json!({
        "schema_version": "1.1",
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
                    "viewer": { "this": {} }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [
                                { "type": "user" },
                                { "type": "group", "relation": "member" }
                            ]
                        }
                    }
                }
            }
        ]
    });

    let _model_id = create_authorization_model(&store_id, model).await?;

    if !is_listusers_available(&store_id).await {
        eprintln!("SKIPPED: ListUsers API not available");
        return Ok(());
    }

    // Group engineering has members
    write_tuples(
        &store_id,
        vec![
            ("user:alice", "member", "group:engineering"),
            ("user:bob", "member", "group:engineering"),
            // Grant group access to document
            ("group:engineering#member", "viewer", "document:doc1"),
        ],
    )
    .await?;

    let client = shared_client();

    // Act: List users who are viewers through group membership
    let list_request = json!({
        "object": { "type": "document", "id": "doc1" },
        "relation": "viewer",
        "user_filters": [{ "type": "group", "relation": "member" }]
    });

    let response = client
        .post(format!(
            "{}/stores/{}/list-users",
            get_openfga_url(),
            store_id
        ))
        .json(&list_request)
        .send()
        .await?;

    assert!(
        response.status().is_success(),
        "ListUsers with userset filter should succeed"
    );

    let response_body: serde_json::Value = response.json().await?;

    // Validate response format and check for userset references
    let users = validate_listusers_response(&response_body);

    // Check for userset format in response
    let has_userset = users.iter().any(|u| u.get("userset").is_some());

    assert!(
        has_userset || !users.is_empty(),
        "Should return userset reference or expanded users"
    );

    Ok(())
}

/// Test: ListUsers returns wildcard results
#[tokio::test]
async fn test_listusers_returns_wildcard() -> Result<()> {
    // Arrange: Create store with wildcard support
    let store_id = create_test_store().await?;

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
                            "directly_related_user_types": [
                                { "type": "user" },
                                { "type": "user", "wildcard": {} }
                            ]
                        }
                    }
                }
            }
        ]
    });

    let _model_id = create_authorization_model(&store_id, model).await?;

    if !is_listusers_available(&store_id).await {
        eprintln!("SKIPPED: ListUsers API not available");
        return Ok(());
    }

    // Grant public access via wildcard
    write_tuples(&store_id, vec![("user:*", "viewer", "document:public-doc")]).await?;

    let client = shared_client();

    // Act: List users who are viewers of public doc
    let list_request = json!({
        "object": { "type": "document", "id": "public-doc" },
        "relation": "viewer",
        "user_filters": [{ "type": "user" }]
    });

    let response = client
        .post(format!(
            "{}/stores/{}/list-users",
            get_openfga_url(),
            store_id
        ))
        .json(&list_request)
        .send()
        .await?;

    assert!(
        response.status().is_success(),
        "ListUsers should succeed for wildcard"
    );

    let response_body: serde_json::Value = response.json().await?;

    // Validate response format
    let users = validate_listusers_response(&response_body);

    // Should return wildcard in response
    let has_wildcard = users.iter().any(|u| u.get("wildcard").is_some());

    assert!(
        has_wildcard,
        "Should return wildcard result for public access"
    );

    Ok(())
}

/// Test: ListUsers with contextual tuples
#[tokio::test]
async fn test_listusers_with_contextual_tuples() -> Result<()> {
    let store_id = create_test_store().await?;

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

    let _model_id = create_authorization_model(&store_id, model).await?;

    if !is_listusers_available(&store_id).await {
        eprintln!("SKIPPED: ListUsers API not available");
        return Ok(());
    }

    let client = shared_client();

    // Act: List users with contextual tuple (no stored tuples)
    let list_request = json!({
        "object": { "type": "document", "id": "temp-doc" },
        "relation": "viewer",
        "user_filters": [{ "type": "user" }],
        "contextual_tuples": {
            "tuple_keys": [
                {
                    "user": "user:temp-user",
                    "relation": "viewer",
                    "object": "document:temp-doc"
                }
            ]
        }
    });

    let response = client
        .post(format!(
            "{}/stores/{}/list-users",
            get_openfga_url(),
            store_id
        ))
        .json(&list_request)
        .send()
        .await?;

    assert!(
        response.status().is_success(),
        "ListUsers with contextual tuples should succeed"
    );

    let response_body: serde_json::Value = response.json().await?;

    // Validate response format
    let users = validate_listusers_response(&response_body);

    // Should return user from contextual tuple
    let user_ids: HashSet<String> = users
        .iter()
        .filter_map(|u| {
            u.get("object").and_then(|o| {
                let user_type = o.get("type")?.as_str()?;
                let user_id = o.get("id")?.as_str()?;
                Some(format!("{}:{}", user_type, user_id))
            })
        })
        .collect();

    assert!(
        user_ids.contains("user:temp-user"),
        "Should return user from contextual tuple"
    );

    Ok(())
}

/// Test: ListUsers with computed relations
#[tokio::test]
async fn test_listusers_with_computed_relations() -> Result<()> {
    let store_id = create_test_store().await?;

    // Model where viewer includes editor (union)
    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            { "type": "user" },
            {
                "type": "document",
                "relations": {
                    "editor": { "this": {} },
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

    let _model_id = create_authorization_model(&store_id, model).await?;

    if !is_listusers_available(&store_id).await {
        eprintln!("SKIPPED: ListUsers API not available");
        return Ok(());
    }

    write_tuples(
        &store_id,
        vec![
            ("user:alice", "viewer", "document:doc1"),
            ("user:bob", "editor", "document:doc1"), // Should also be a viewer
        ],
    )
    .await?;

    let client = shared_client();

    // Act: List all viewers (should include editor via union)
    let list_request = json!({
        "object": { "type": "document", "id": "doc1" },
        "relation": "viewer",
        "user_filters": [{ "type": "user" }]
    });

    let response = client
        .post(format!(
            "{}/stores/{}/list-users",
            get_openfga_url(),
            store_id
        ))
        .json(&list_request)
        .send()
        .await?;

    assert!(
        response.status().is_success(),
        "ListUsers with computed relations should succeed"
    );

    let response_body: serde_json::Value = response.json().await?;

    // Validate response format
    let users = validate_listusers_response(&response_body);

    let user_ids: HashSet<String> = users
        .iter()
        .filter_map(|u| {
            u.get("object").and_then(|o| {
                let user_type = o.get("type")?.as_str()?;
                let user_id = o.get("id")?.as_str()?;
                Some(format!("{}:{}", user_type, user_id))
            })
        })
        .collect();

    // Both alice (direct viewer) and bob (editor -> viewer) should be returned
    assert!(
        user_ids.contains("user:alice"),
        "Should include direct viewer"
    );
    assert!(
        user_ids.contains("user:bob"),
        "Should include editor via computed relation"
    );

    Ok(())
}

/// Test: ListUsers with no matching users returns empty
#[tokio::test]
async fn test_listusers_empty_result() -> Result<()> {
    let store_id = create_test_store().await?;

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

    let _model_id = create_authorization_model(&store_id, model).await?;

    if !is_listusers_available(&store_id).await {
        eprintln!("SKIPPED: ListUsers API not available");
        return Ok(());
    }

    // Don't write any tuples

    let client = shared_client();

    let list_request = json!({
        "object": { "type": "document", "id": "no-viewers-doc" },
        "relation": "viewer",
        "user_filters": [{ "type": "user" }]
    });

    let response = client
        .post(format!(
            "{}/stores/{}/list-users",
            get_openfga_url(),
            store_id
        ))
        .json(&list_request)
        .send()
        .await?;

    assert!(response.status().is_success());

    let response_body: serde_json::Value = response.json().await?;

    // Validate response format
    let users = validate_listusers_response(&response_body);

    assert_eq!(users.len(), 0, "Should return empty array when no users");

    Ok(())
}

/// Test: ListUsers with invalid type returns error
#[tokio::test]
async fn test_listusers_invalid_type_error() -> Result<()> {
    let store_id = create_test_store().await?;

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

    let _model_id = create_authorization_model(&store_id, model).await?;

    if !is_listusers_available(&store_id).await {
        eprintln!("SKIPPED: ListUsers API not available");
        return Ok(());
    }

    let client = shared_client();

    // Invalid type in object
    let list_request = json!({
        "object": { "type": "nonexistent_type", "id": "doc1" },
        "relation": "viewer",
        "user_filters": [{ "type": "user" }]
    });

    let response = client
        .post(format!(
            "{}/stores/{}/list-users",
            get_openfga_url(),
            store_id
        ))
        .json(&list_request)
        .send()
        .await?;

    assert!(
        response.status() == StatusCode::BAD_REQUEST
            || response.status() == StatusCode::UNPROCESSABLE_ENTITY,
        "ListUsers with invalid type should return error, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: ListUsers with non-existent store returns error
#[tokio::test]
async fn test_listusers_nonexistent_store_error() -> Result<()> {
    let client = shared_client();

    let list_request = json!({
        "object": { "type": "document", "id": "doc1" },
        "relation": "viewer",
        "user_filters": [{ "type": "user" }]
    });

    let response = client
        .post(format!(
            "{}/stores/{}/list-users",
            get_openfga_url(),
            "01ARZ3NDEKTSV4RRFFQ69G5FAV" // Valid ULID format but doesn't exist
        ))
        .json(&list_request)
        .send()
        .await?;

    // Should return 404 or 400
    assert!(
        response.status() == StatusCode::NOT_FOUND || response.status() == StatusCode::BAD_REQUEST,
        "ListUsers with non-existent store should return 404 or 400, got: {}",
        response.status()
    );

    Ok(())
}
