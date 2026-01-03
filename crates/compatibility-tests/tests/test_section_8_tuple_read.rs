mod common;

use anyhow::Result;
use common::{create_test_model, create_test_store, get_openfga_url, write_tuples};
use serde_json::json;

/// Test: POST /stores/{store_id}/read reads tuples by filter
#[tokio::test]
async fn test_read_tuples_by_filter() -> Result<()> {
    // Arrange: Create store, model, and write tuples
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    write_tuples(
        &store_id,
        vec![
            ("user:alice", "viewer", "document:doc1"),
            ("user:bob", "editor", "document:doc1"),
        ],
    )
    .await?;

    let client = reqwest::Client::new();

    // Act: Read tuples
    let read_request = json!({});

    let response = client
        .post(format!("{}/stores/{}/read", get_openfga_url(), store_id))
        .json(&read_request)
        .send()
        .await?;

    // Assert: Read succeeded
    assert!(
        response.status().is_success(),
        "Read should succeed, got: {}",
        response.status()
    );

    let response_body: serde_json::Value = response.json().await?;

    // Verify response has 'tuples' array
    assert!(
        response_body.get("tuples").is_some(),
        "Response should contain 'tuples' array"
    );

    Ok(())
}

/// Test: Read with user filter returns matching tuples
#[tokio::test]
async fn test_read_with_user_filter() -> Result<()> {
    // Arrange: Create store, model, and write tuples
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    write_tuples(
        &store_id,
        vec![
            ("user:alice", "viewer", "document:doc1"),
            ("user:bob", "viewer", "document:doc2"),
            ("user:alice", "editor", "document:doc3"),
        ],
    )
    .await?;

    let client = reqwest::Client::new();

    // Act: Read tuples filtered by user + object type
    // Note: OpenFGA requires object field. Using type-only ("document:") requires user field.
    let read_request = json!({
        "tuple_key": {
            "user": "user:alice",
            "object": "document:"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/read", get_openfga_url(), store_id))
        .json(&read_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;
    let tuples = response_body
        .get("tuples")
        .and_then(|v| v.as_array())
        .expect("tuples field should be an array");

    // Assert: Only alice's tuples returned
    assert_eq!(
        tuples.len(),
        2,
        "Should return 2 tuples for user:alice, got: {}",
        tuples.len()
    );

    // Verify all tuples have user:alice
    for tuple in tuples {
        let user = tuple
            .get("key")
            .and_then(|k| k.get("user"))
            .and_then(|v| v.as_str())
            .expect("Tuple should have user field");

        assert_eq!(
            user, "user:alice",
            "All tuples should be for user:alice"
        );
    }

    Ok(())
}

/// Test: Read with relation filter returns matching tuples
#[tokio::test]
async fn test_read_with_relation_filter() -> Result<()> {
    // Arrange: Create store, model, and write tuples
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    write_tuples(
        &store_id,
        vec![
            ("user:alice", "viewer", "document:doc1"),
            ("user:bob", "viewer", "document:doc2"),
            ("user:charlie", "editor", "document:doc3"),
        ],
    )
    .await?;

    let client = reqwest::Client::new();

    // Act: Read tuples filtered by relation + specific object
    // Note: OpenFGA requires object field. We filter by relation on a specific object.
    let read_request = json!({
        "tuple_key": {
            "relation": "viewer",
            "object": "document:doc1"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/read", get_openfga_url(), store_id))
        .json(&read_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;
    let tuples = response_body
        .get("tuples")
        .and_then(|v| v.as_array())
        .expect("tuples field should be an array");

    // Assert: Only viewer tuples for doc1 returned
    assert_eq!(
        tuples.len(),
        1,
        "Should return 1 viewer tuple for doc1, got: {}",
        tuples.len()
    );

    // Verify tuple has viewer relation
    let tuple = &tuples[0];
    let relation = tuple
        .get("key")
        .and_then(|k| k.get("relation"))
        .and_then(|v| v.as_str())
        .expect("Tuple should have relation field");

    assert_eq!(
        relation, "viewer",
        "Tuple should have viewer relation"
    );

    Ok(())
}

/// Test: Read with object filter returns matching tuples
#[tokio::test]
async fn test_read_with_object_filter() -> Result<()> {
    // Arrange: Create store, model, and write tuples
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    write_tuples(
        &store_id,
        vec![
            ("user:alice", "viewer", "document:doc1"),
            ("user:bob", "editor", "document:doc1"),
            ("user:charlie", "viewer", "document:doc2"),
        ],
    )
    .await?;

    let client = reqwest::Client::new();

    // Act: Read tuples filtered by object
    let read_request = json!({
        "tuple_key": {
            "object": "document:doc1"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/read", get_openfga_url(), store_id))
        .json(&read_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;
    let tuples = response_body
        .get("tuples")
        .and_then(|v| v.as_array())
        .expect("tuples field should be an array");

    // Assert: Only doc1 tuples returned
    assert_eq!(
        tuples.len(),
        2,
        "Should return 2 tuples for document:doc1, got: {}",
        tuples.len()
    );

    // Verify all tuples have document:doc1
    for tuple in tuples {
        let object = tuple
            .get("key")
            .and_then(|k| k.get("object"))
            .and_then(|v| v.as_str())
            .expect("Tuple should have object field");

        assert_eq!(
            object, "document:doc1",
            "All tuples should be for document:doc1"
        );
    }

    Ok(())
}

/// Test: Read with multiple filters combines them (AND logic)
#[tokio::test]
async fn test_read_with_multiple_filters() -> Result<()> {
    // Arrange: Create store, model, and write tuples
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    write_tuples(
        &store_id,
        vec![
            ("user:alice", "viewer", "document:doc1"),
            ("user:alice", "editor", "document:doc1"),
            ("user:bob", "viewer", "document:doc1"),
            ("user:alice", "viewer", "document:doc2"),
        ],
    )
    .await?;

    let client = reqwest::Client::new();

    // Act: Read tuples filtered by user AND relation AND object
    let read_request = json!({
        "tuple_key": {
            "user": "user:alice",
            "relation": "viewer",
            "object": "document:doc1"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/read", get_openfga_url(), store_id))
        .json(&read_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;
    let tuples = response_body
        .get("tuples")
        .and_then(|v| v.as_array())
        .expect("tuples field should be an array");

    // Assert: Only the exact match returned
    assert_eq!(
        tuples.len(),
        1,
        "Should return 1 tuple matching all filters, got: {}",
        tuples.len()
    );

    // Verify the tuple matches all filters
    let tuple = &tuples[0];
    let key = tuple.get("key").expect("Tuple should have key");

    assert_eq!(
        key.get("user").and_then(|v| v.as_str()),
        Some("user:alice"),
        "Tuple should have user:alice"
    );
    assert_eq!(
        key.get("relation").and_then(|v| v.as_str()),
        Some("viewer"),
        "Tuple should have viewer relation"
    );
    assert_eq!(
        key.get("object").and_then(|v| v.as_str()),
        Some("document:doc1"),
        "Tuple should have document:doc1"
    );

    Ok(())
}

/// Test: Read with empty filter returns all tuples
#[tokio::test]
async fn test_read_with_empty_filter() -> Result<()> {
    // Arrange: Create store, model, and write tuples
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    write_tuples(
        &store_id,
        vec![
            ("user:alice", "viewer", "document:doc1"),
            ("user:bob", "editor", "document:doc2"),
            ("user:charlie", "viewer", "document:doc3"),
        ],
    )
    .await?;

    let client = reqwest::Client::new();

    // Act: Read all tuples (empty filter)
    let read_request = json!({});

    let response = client
        .post(format!("{}/stores/{}/read", get_openfga_url(), store_id))
        .json(&read_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;
    let tuples = response_body
        .get("tuples")
        .and_then(|v| v.as_array())
        .expect("tuples field should be an array");

    // Assert: All tuples returned
    assert_eq!(
        tuples.len(),
        3,
        "Should return all 3 tuples, got: {}",
        tuples.len()
    );

    Ok(())
}

/// Test: Read respects page_size parameter
#[tokio::test]
async fn test_read_respects_page_size() -> Result<()> {
    // Arrange: Create store, model, and write many tuples
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    let mut tuples = Vec::new();
    for i in 0..10 {
        tuples.push((
            format!("user:user{}", i),
            "viewer",
            format!("document:doc{}", i),
        ));
    }

    // Convert to Vec<(&str, &str, &str)>
    let tuple_refs: Vec<(&str, &str, &str)> = tuples
        .iter()
        .map(|(u, r, o)| (u.as_str(), *r, o.as_str()))
        .collect();

    write_tuples(&store_id, tuple_refs).await?;

    let client = reqwest::Client::new();

    // Act: Read with page_size=3
    let read_request = json!({
        "page_size": 3
    });

    let response = client
        .post(format!("{}/stores/{}/read", get_openfga_url(), store_id))
        .json(&read_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;
    let tuples = response_body
        .get("tuples")
        .and_then(|v| v.as_array())
        .expect("tuples field should be an array");

    // Assert: Exactly page_size tuples returned
    assert_eq!(
        tuples.len(),
        3,
        "Should return exactly 3 tuples (page_size), got: {}",
        tuples.len()
    );

    // Verify continuation_token is present (more results available)
    assert!(
        response_body.get("continuation_token").is_some(),
        "Should have continuation_token when more results exist"
    );

    Ok(())
}

/// Test: Read continuation_token enables pagination
#[tokio::test]
async fn test_read_continuation_token() -> Result<()> {
    // Arrange: Create store, model, and write many tuples
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    let mut tuples = Vec::new();
    for i in 0..10 {
        tuples.push((
            format!("user:user{}", i),
            "viewer",
            format!("document:doc{}", i),
        ));
    }

    let tuple_refs: Vec<(&str, &str, &str)> = tuples
        .iter()
        .map(|(u, r, o)| (u.as_str(), *r, o.as_str()))
        .collect();

    write_tuples(&store_id, tuple_refs).await?;

    let client = reqwest::Client::new();

    // Act: Get first page
    let read_request_first = json!({
        "page_size": 3
    });

    let response_first = client
        .post(format!("{}/stores/{}/read", get_openfga_url(), store_id))
        .json(&read_request_first)
        .send()
        .await?;

    let response_body_first: serde_json::Value = response_first.json().await?;
    let first_tuples = response_body_first
        .get("tuples")
        .and_then(|v| v.as_array())
        .expect("tuples field should be an array");

    let continuation_token = response_body_first
        .get("continuation_token")
        .and_then(|v| v.as_str())
        .expect("continuation_token should be present");

    // Act: Get second page using continuation_token
    let read_request_second = json!({
        "page_size": 3,
        "continuation_token": continuation_token
    });

    let response_second = client
        .post(format!("{}/stores/{}/read", get_openfga_url(), store_id))
        .json(&read_request_second)
        .send()
        .await?;

    let response_body_second: serde_json::Value = response_second.json().await?;
    let second_tuples = response_body_second
        .get("tuples")
        .and_then(|v| v.as_array())
        .expect("tuples field should be an array");

    // Assert: Second page has different tuples
    assert_eq!(
        second_tuples.len(),
        3,
        "Second page should also have 3 tuples, got: {}",
        second_tuples.len()
    );

    // Verify no overlap between pages using HashSet for efficiency
    use std::collections::HashSet;

    let extract_key = |t: &serde_json::Value| -> String {
        format!(
            "{}#{}#{}",
            t.get("key")
                .and_then(|k| k.get("user"))
                .and_then(|v| v.as_str())
                .expect("Tuple should have user"),
            t.get("key")
                .and_then(|k| k.get("relation"))
                .and_then(|v| v.as_str())
                .expect("Tuple should have relation"),
            t.get("key")
                .and_then(|k| k.get("object"))
                .and_then(|v| v.as_str())
                .expect("Tuple should have object")
        )
    };

    let first_keys: HashSet<String> = first_tuples.iter().map(extract_key).collect();
    let second_keys: HashSet<String> = second_tuples.iter().map(extract_key).collect();

    assert!(
        first_keys.is_disjoint(&second_keys),
        "First and second pages should not have overlapping tuples"
    );

    Ok(())
}

/// Test: Read returns empty array when no matches
#[tokio::test]
async fn test_read_returns_empty_when_no_matches() -> Result<()> {
    // Arrange: Create store and model, but don't write any tuples
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    let client = reqwest::Client::new();

    // Act: Read tuples (none exist for this user)
    let read_request = json!({
        "tuple_key": {
            "user": "user:nonexistent",
            "object": "document:"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/read", get_openfga_url(), store_id))
        .json(&read_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;
    let tuples = response_body
        .get("tuples")
        .and_then(|v| v.as_array())
        .expect("tuples field should be an array");

    // Assert: Empty array returned
    assert_eq!(
        tuples.len(),
        0,
        "Should return empty array when no matches, got: {}",
        tuples.len()
    );

    Ok(())
}
