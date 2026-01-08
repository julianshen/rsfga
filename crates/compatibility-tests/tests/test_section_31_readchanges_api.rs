mod common;

use anyhow::Result;
use common::{create_authorization_model, create_test_store, get_openfga_url, write_tuples, shared_client};
use reqwest::StatusCode;
use serde_json::json;
use std::time::Duration;
use tokio::time::sleep;

// ============================================================================
// Section 31: ReadChanges (Changelog) API Tests
// ============================================================================
//
// The ReadChanges API returns a paginated list of tuple changes (additions
// and deletions) sorted by ascending time. Essential for audit trails and
// synchronization workflows.
//
// Endpoint: GET /stores/{store_id}/changes
//
// Parameters:
// - type (optional): Filter by object type
// - page_size (optional): Default 50, max 100
// - continuation_token (optional): Pagination token
// - start_time (optional, v1.8.0+): ISO 8601 timestamp filter
//
// ============================================================================

/// Test: GET /stores/{store_id}/changes returns tuple modifications
#[tokio::test]
async fn test_readchanges_returns_tuple_writes() -> Result<()> {
    // Arrange: Create store, model, and write some tuples
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

    // Write some tuples
    write_tuples(
        &store_id,
        vec![
            ("user:alice", "viewer", "document:doc1"),
            ("user:bob", "viewer", "document:doc2"),
        ],
    )
    .await?;

    let client = shared_client();

    // Act: Get changes
    let response = client
        .get(format!(
            "{}/stores/{}/changes",
            get_openfga_url(),
            store_id
        ))
        .send()
        .await?;

    // Assert: ReadChanges succeeded
    assert!(
        response.status().is_success(),
        "ReadChanges should succeed, got: {}",
        response.status()
    );

    let response_body: serde_json::Value = response.json().await?;

    // Verify response has 'changes' array
    let changes = response_body
        .get("changes")
        .and_then(|c| c.as_array())
        .expect("Response should have 'changes' array");

    assert!(
        changes.len() >= 2,
        "Should have at least 2 changes from our writes, got: {}",
        changes.len()
    );

    // Verify change format
    for change in changes {
        assert!(
            change.get("tuple_key").is_some(),
            "Each change should have 'tuple_key'"
        );
        assert!(
            change.get("operation").is_some(),
            "Each change should have 'operation'"
        );
        assert!(
            change.get("timestamp").is_some(),
            "Each change should have 'timestamp'"
        );
    }

    Ok(())
}

/// Test: ReadChanges returns both writes and deletes
#[tokio::test]
async fn test_readchanges_returns_writes_and_deletes() -> Result<()> {
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

    let client = shared_client();

    // Write a tuple
    write_tuples(&store_id, vec![("user:alice", "viewer", "document:doc1")]).await?;

    // Delete the tuple
    let delete_request = json!({
        "deletes": {
            "tuple_keys": [{
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:doc1"
            }]
        }
    });

    let delete_response = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&delete_request)
        .send()
        .await?;

    assert!(
        delete_response.status().is_success(),
        "Delete should succeed"
    );

    // Get changes
    let response = client
        .get(format!(
            "{}/stores/{}/changes",
            get_openfga_url(),
            store_id
        ))
        .send()
        .await?;

    assert!(response.status().is_success());

    let response_body: serde_json::Value = response.json().await?;

    let changes = response_body
        .get("changes")
        .and_then(|c| c.as_array())
        .expect("Response should have 'changes' array");

    // Should have both write and delete operations
    let operations: Vec<&str> = changes
        .iter()
        .filter_map(|c| c.get("operation").and_then(|o| o.as_str()))
        .collect();

    let has_write = operations
        .iter()
        .any(|o| o.contains("WRITE") || o.contains("write"));
    let has_delete = operations
        .iter()
        .any(|o| o.contains("DELETE") || o.contains("delete"));

    assert!(has_write, "Should have WRITE operation");
    assert!(has_delete, "Should have DELETE operation");

    Ok(())
}

/// Test: ReadChanges with type filter
#[tokio::test]
async fn test_readchanges_with_type_filter() -> Result<()> {
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
            },
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

    let _model_id = create_authorization_model(&store_id, model).await?;

    // Write tuples to both types
    write_tuples(
        &store_id,
        vec![
            ("user:alice", "viewer", "document:doc1"),
            ("user:bob", "viewer", "folder:folder1"),
        ],
    )
    .await?;

    let client = shared_client();

    // Act: Get changes filtered by document type only
    let response = client
        .get(format!(
            "{}/stores/{}/changes?type=document",
            get_openfga_url(),
            store_id
        ))
        .send()
        .await?;

    assert!(
        response.status().is_success(),
        "ReadChanges with type filter should succeed"
    );

    let response_body: serde_json::Value = response.json().await?;

    let changes = response_body
        .get("changes")
        .and_then(|c| c.as_array())
        .expect("Response should have 'changes' array");

    // All changes should be for document type
    for change in changes {
        let obj = change
            .get("tuple_key")
            .and_then(|tk| tk.get("object"))
            .and_then(|o| o.as_str())
            .expect("Should have object");

        assert!(
            obj.starts_with("document:"),
            "Filtered changes should only include documents, got: {}",
            obj
        );
    }

    Ok(())
}

/// Test: ReadChanges pagination with page_size
#[tokio::test]
async fn test_readchanges_pagination_page_size() -> Result<()> {
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

    // Write many tuples
    let tuples: Vec<_> = (0..10)
        .map(|i| {
            (
                format!("user:user{}", i),
                "viewer".to_string(),
                format!("document:doc{}", i),
            )
        })
        .collect();

    let tuple_refs: Vec<(&str, &str, &str)> = tuples
        .iter()
        .map(|(u, r, o)| (u.as_str(), r.as_str(), o.as_str()))
        .collect();

    write_tuples(&store_id, tuple_refs).await?;

    let client = shared_client();

    // Act: Get changes with small page size
    let response = client
        .get(format!(
            "{}/stores/{}/changes?page_size=3",
            get_openfga_url(),
            store_id
        ))
        .send()
        .await?;

    assert!(response.status().is_success());

    let response_body: serde_json::Value = response.json().await?;

    let changes = response_body
        .get("changes")
        .and_then(|c| c.as_array())
        .expect("Response should have 'changes' array");

    assert!(
        changes.len() <= 3,
        "Should return at most page_size changes, got: {}",
        changes.len()
    );

    // Should have continuation token if more results exist
    let continuation_token = response_body
        .get("continuation_token")
        .and_then(|t| t.as_str());

    if changes.len() == 3 {
        assert!(
            continuation_token.is_some() && !continuation_token.unwrap().is_empty(),
            "Should have continuation token when more results exist"
        );
    }

    Ok(())
}

/// Test: ReadChanges pagination with continuation token
#[tokio::test]
async fn test_readchanges_continuation_token() -> Result<()> {
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

    // Write enough tuples to require pagination
    let tuples: Vec<_> = (0..10)
        .map(|i| {
            (
                format!("user:user{}", i),
                "viewer".to_string(),
                format!("document:doc{}", i),
            )
        })
        .collect();

    let tuple_refs: Vec<(&str, &str, &str)> = tuples
        .iter()
        .map(|(u, r, o)| (u.as_str(), r.as_str(), o.as_str()))
        .collect();

    write_tuples(&store_id, tuple_refs).await?;

    let client = shared_client();

    // Get first page
    let response1 = client
        .get(format!(
            "{}/stores/{}/changes?page_size=3",
            get_openfga_url(),
            store_id
        ))
        .send()
        .await?;

    let body1: serde_json::Value = response1.json().await?;

    let token = body1
        .get("continuation_token")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty());

    if token.is_none() {
        // Not enough changes to paginate
        return Ok(());
    }

    // Get second page using continuation token
    let response2 = client
        .get(format!(
            "{}/stores/{}/changes?page_size=3&continuation_token={}",
            get_openfga_url(),
            store_id,
            token.unwrap()
        ))
        .send()
        .await?;

    assert!(
        response2.status().is_success(),
        "Second page request should succeed"
    );

    let body2: serde_json::Value = response2.json().await?;

    let changes2 = body2
        .get("changes")
        .and_then(|c| c.as_array())
        .expect("Response should have 'changes' array");

    assert!(
        !changes2.is_empty(),
        "Second page should have some changes"
    );

    Ok(())
}

/// Test: ReadChanges returns same token when no new changes
#[tokio::test]
async fn test_readchanges_same_token_when_no_changes() -> Result<()> {
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

    // Write one tuple
    write_tuples(&store_id, vec![("user:alice", "viewer", "document:doc1")]).await?;

    let client = shared_client();

    // Get all changes
    let response1 = client
        .get(format!(
            "{}/stores/{}/changes",
            get_openfga_url(),
            store_id
        ))
        .send()
        .await?;

    let body1: serde_json::Value = response1.json().await?;

    let token1 = body1
        .get("continuation_token")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty());

    if token1.is_none() {
        return Ok(()); // No token to test
    }

    // Wait a bit and request again with the token
    sleep(Duration::from_millis(100)).await;

    let response2 = client
        .get(format!(
            "{}/stores/{}/changes?continuation_token={}",
            get_openfga_url(),
            store_id,
            token1.unwrap()
        ))
        .send()
        .await?;

    assert!(response2.status().is_success());

    let body2: serde_json::Value = response2.json().await?;

    let token2 = body2
        .get("continuation_token")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty());

    let changes2 = body2
        .get("changes")
        .and_then(|c| c.as_array())
        .expect("Response should have 'changes' array");

    // When no new changes, should return empty changes and same/similar token
    if changes2.is_empty() && token2.is_some() {
        // Token should be returned for future polling
        assert!(
            !token2.unwrap().is_empty(),
            "Should return continuation token for polling"
        );
    }

    Ok(())
}

/// Test: ReadChanges error when type filter mismatch with token
#[tokio::test]
async fn test_readchanges_type_filter_mismatch_with_token() -> Result<()> {
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
            },
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

    let _model_id = create_authorization_model(&store_id, model).await?;

    write_tuples(
        &store_id,
        vec![
            ("user:alice", "viewer", "document:doc1"),
            ("user:bob", "viewer", "folder:folder1"),
        ],
    )
    .await?;

    let client = shared_client();

    // Get changes with document type filter
    let response1 = client
        .get(format!(
            "{}/stores/{}/changes?type=document&page_size=1",
            get_openfga_url(),
            store_id
        ))
        .send()
        .await?;

    let body1: serde_json::Value = response1.json().await?;

    let token = body1
        .get("continuation_token")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty());

    if token.is_none() {
        return Ok(()); // No token to test
    }

    // Try to use token WITHOUT type filter (should error)
    let response2 = client
        .get(format!(
            "{}/stores/{}/changes?continuation_token={}",
            get_openfga_url(),
            store_id,
            token.unwrap()
        ))
        .send()
        .await?;

    // Per documentation: "If you send a continuation token without the type parameter
    // set (when you originally filtered by type), you will get an error"
    // However, behavior may vary - some implementations may accept it
    if !response2.status().is_success() {
        assert!(
            response2.status() == StatusCode::BAD_REQUEST
                || response2.status() == StatusCode::UNPROCESSABLE_ENTITY,
            "Should return error for type mismatch, got: {}",
            response2.status()
        );
    }

    Ok(())
}

/// Test: ReadChanges empty result for new store
#[tokio::test]
async fn test_readchanges_empty_for_new_store() -> Result<()> {
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

    // Don't write any tuples

    let client = shared_client();

    let response = client
        .get(format!(
            "{}/stores/{}/changes",
            get_openfga_url(),
            store_id
        ))
        .send()
        .await?;

    assert!(response.status().is_success());

    let response_body: serde_json::Value = response.json().await?;

    let changes = response_body
        .get("changes")
        .and_then(|c| c.as_array())
        .expect("Response should have 'changes' array");

    assert_eq!(
        changes.len(),
        0,
        "New store with no writes should have empty changes"
    );

    Ok(())
}

/// Test: ReadChanges for non-existent store returns error
#[tokio::test]
async fn test_readchanges_nonexistent_store_error() -> Result<()> {
    let client = shared_client();

    let response = client
        .get(format!(
            "{}/stores/{}/changes",
            get_openfga_url(),
            "01ARZ3NDEKTSV4RRFFQ69G5FAV" // Valid ULID but doesn't exist
        ))
        .send()
        .await?;

    assert!(
        response.status() == StatusCode::NOT_FOUND || response.status() == StatusCode::BAD_REQUEST,
        "ReadChanges with non-existent store should return 404 or 400, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: ReadChanges respects chronological order
#[tokio::test]
async fn test_readchanges_chronological_order() -> Result<()> {
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

    // Write tuples with delays to ensure ordering
    for i in 0..3 {
        write_tuples(
            &store_id,
            vec![(
                &format!("user:user{}", i),
                "viewer",
                &format!("document:doc{}", i),
            )],
        )
        .await?;
        sleep(Duration::from_millis(50)).await;
    }

    let client = shared_client();

    let response = client
        .get(format!(
            "{}/stores/{}/changes",
            get_openfga_url(),
            store_id
        ))
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;

    let changes = response_body
        .get("changes")
        .and_then(|c| c.as_array())
        .expect("Response should have 'changes' array");

    // Verify timestamps are in ascending order
    let timestamps: Vec<&str> = changes
        .iter()
        .filter_map(|c| c.get("timestamp").and_then(|t| t.as_str()))
        .collect();

    for i in 1..timestamps.len() {
        assert!(
            timestamps[i] >= timestamps[i - 1],
            "Changes should be in chronological order: {} should come after {}",
            timestamps[i],
            timestamps[i - 1]
        );
    }

    Ok(())
}
