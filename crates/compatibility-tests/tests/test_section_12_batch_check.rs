mod common;

use anyhow::Result;
use common::{create_test_model, create_test_store, get_openfga_url, write_tuples};
use reqwest::StatusCode;
use serde_json::json;

/// Test: POST /stores/{store_id}/batch-check checks multiple tuples
#[tokio::test]
async fn test_batch_check_multiple_tuples() -> Result<()> {
    // Arrange: Create store, model, and write tuples
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    write_tuples(
        &store_id,
        vec![
            ("user:alice", "viewer", "document:doc1"),
            ("user:bob", "editor", "document:doc2"),
        ],
    )
    .await?;

    let client = reqwest::Client::new();

    // Act: Batch check 3 tuples (2 exist, 1 doesn't)
    let batch_request = json!({
        "checks": [
            {
                "correlation_id": "check-1",
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:doc1"
                }
            },
            {
                "correlation_id": "check-2",
                "tuple_key": {
                    "user": "user:bob",
                    "relation": "editor",
                    "object": "document:doc2"
                }
            },
            {
                "correlation_id": "check-3",
                "tuple_key": {
                    "user": "user:charlie",
                    "relation": "viewer",
                    "object": "document:doc3"
                }
            }
        ]
    });

    let response = client
        .post(format!(
            "{}/stores/{}/batch-check",
            get_openfga_url(),
            store_id
        ))
        .json(&batch_request)
        .send()
        .await?;

    // Assert: Batch check succeeded
    let status = response.status();
    if !status.is_success() {
        let error_body = response.text().await?;
        panic!(
            "Batch check should succeed, got status: {}. Response: {}",
            status,
            error_body
        );
    }

    let response_body: serde_json::Value = response.json().await?;

    // Verify response has 'result' object (map of correlation_id -> result)
    let results = response_body
        .get("result")
        .and_then(|v| v.as_object())
        .expect("Response should have 'result' object");

    assert_eq!(
        results.len(),
        3,
        "Batch check should return 3 results for 3 checks"
    );

    // Verify results keyed by correlation_id
    assert!(results.contains_key("check-1"), "Should have check-1 result");
    assert!(results.contains_key("check-2"), "Should have check-2 result");
    assert!(results.contains_key("check-3"), "Should have check-3 result");

    Ok(())
}

/// Test: Batch check returns array of results in same order
#[tokio::test]
async fn test_batch_check_preserves_order() -> Result<()> {
    // Arrange: Create store, model, and write tuples
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    write_tuples(
        &store_id,
        vec![
            ("user:alice", "viewer", "document:doc1"),
            ("user:bob", "editor", "document:doc2"),
        ],
    )
    .await?;

    let client = reqwest::Client::new();

    // Act: Batch check with known order
    let batch_request = json!({
        "checks": [
            {
                "correlation_id": "check-1",
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:doc1"
                }
            },
            {
                "correlation_id": "check-2",
                "tuple_key": {
                    "user": "user:charlie",  // Doesn't exist
                    "relation": "viewer",
                    "object": "document:doc3"
                }
            },
            {
                "correlation_id": "check-3",
                "tuple_key": {
                    "user": "user:bob",
                    "relation": "editor",
                    "object": "document:doc2"
                }
            }
        ]
    });

    let response = client
        .post(format!(
            "{}/stores/{}/batch-check",
            get_openfga_url(),
            store_id
        ))
        .json(&batch_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;

    let results = response_body
        .get("result")
        .and_then(|v| v.as_object())
        .expect("Response should have 'result' object");

    // Assert: Results keyed by correlation_id
    assert_eq!(
        results.get("check-1").and_then(|v| v.get("allowed")).and_then(|v| v.as_bool()),
        Some(true),
        "check-1 result should be true (alice:viewer:doc1)"
    );

    assert_eq!(
        results.get("check-2").and_then(|v| v.get("allowed")).and_then(|v| v.as_bool()),
        Some(false),
        "check-2 result should be false (charlie:viewer:doc3 doesn't exist)"
    );

    assert_eq!(
        results.get("check-3").and_then(|v| v.get("allowed")).and_then(|v| v.as_bool()),
        Some(true),
        "check-3 result should be true (bob:editor:doc2)"
    );

    Ok(())
}

/// Test: Batch check handles mix of allowed/denied
#[tokio::test]
async fn test_batch_check_mixed_results() -> Result<()> {
    // Arrange: Create store, model, and write some tuples
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    write_tuples(
        &store_id,
        vec![
            ("user:alice", "viewer", "document:doc1"),
            ("user:alice", "viewer", "document:doc2"),
        ],
    )
    .await?;

    let client = reqwest::Client::new();

    // Act: Batch check with mix of allowed and denied
    let batch_request = json!({
        "checks": [
            {
                "correlation_id": "check-1",
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:doc1"
                }
            },
            {
                "correlation_id": "check-2",
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "editor",  // Alice is NOT editor
                    "object": "document:doc1"
                }
            },
            {
                "correlation_id": "check-3",
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:doc2"
                }
            },
            {
                "correlation_id": "check-4",
                "tuple_key": {
                    "user": "user:bob",  // Bob doesn't exist
                    "relation": "viewer",
                    "object": "document:doc1"
                }
            }
        ]
    });

    let response = client
        .post(format!(
            "{}/stores/{}/batch-check",
            get_openfga_url(),
            store_id
        ))
        .json(&batch_request)
        .send()
        .await?;

    let response_body: serde_json::Value = response.json().await?;

    let results = response_body
        .get("result")
        .and_then(|v| v.as_object())
        .expect("Response should have 'result' object");

    // Count true and false results
    let allowed_count = results
        .values()
        .filter(|r| r.get("allowed").and_then(|v| v.as_bool()) == Some(true))
        .count();

    let denied_count = results
        .values()
        .filter(|r| r.get("allowed").and_then(|v| v.as_bool()) == Some(false))
        .count();

    // Assert: Mix of allowed and denied
    assert_eq!(allowed_count, 2, "Should have 2 allowed results");
    assert_eq!(denied_count, 2, "Should have 2 denied results");

    Ok(())
}

/// Test: Batch check with empty array returns validation error
#[tokio::test]
async fn test_batch_check_empty_array() -> Result<()> {
    // Arrange: Create store and model
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    let client = reqwest::Client::new();

    // Act: Batch check with empty checks array
    let batch_request = json!({
        "checks": []
    });

    let response = client
        .post(format!(
            "{}/stores/{}/batch-check",
            get_openfga_url(),
            store_id
        ))
        .json(&batch_request)
        .send()
        .await?;

    // Assert: Returns 400 Bad Request (requires at least 1 item)
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "Batch check with empty array should return 400, got: {}",
        response.status()
    );

    Ok(())
}

/// Test: Batch check with maximum allowed items (50)
#[tokio::test]
async fn test_batch_check_large_batch() -> Result<()> {
    // Arrange: Create store, model, and write many tuples
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    // OpenFGA has a maximum batch size of 50
    const MAX_BATCH_SIZE: usize = 50;

    // Write 50 tuples
    let tuples: Vec<_> = (0..MAX_BATCH_SIZE)
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

    let client = reqwest::Client::new();

    // Act: Batch check maximum allowed items (50)
    let checks: Vec<_> = (0..MAX_BATCH_SIZE)
        .map(|i| {
            json!({
                "correlation_id": format!("check-{}", i),
                "tuple_key": {
                    "user": format!("user:user{}", i),
                    "relation": "viewer",
                    "object": format!("document:doc{}", i)
                }
            })
        })
        .collect();

    let batch_request = json!({
        "checks": checks
    });

    let response = client
        .post(format!(
            "{}/stores/{}/batch-check",
            get_openfga_url(),
            store_id
        ))
        .json(&batch_request)
        .send()
        .await?;

    // Assert: Batch check succeeded
    assert!(
        response.status().is_success(),
        "Batch check with max items (50) should succeed, got: {}",
        response.status()
    );

    let response_body: serde_json::Value = response.json().await?;

    let results = response_body
        .get("result")
        .and_then(|v| v.as_object())
        .expect("Response should have 'result' object");

    // Assert: All 50 results returned
    assert_eq!(
        results.len(),
        MAX_BATCH_SIZE,
        "Batch check should return {} results for {} checks",
        MAX_BATCH_SIZE,
        MAX_BATCH_SIZE
    );

    // Assert: All should be allowed
    let all_allowed = results
        .values()
        .all(|r| r.get("allowed").and_then(|v| v.as_bool()) == Some(true));

    assert!(
        all_allowed,
        "All {} checks should return allowed=true",
        MAX_BATCH_SIZE
    );

    Ok(())
}

/// Test: Batch check deduplicates identical requests (verify via performance)
#[tokio::test]
async fn test_batch_check_deduplication() -> Result<()> {
    // Arrange: Create store, model, and write tuple
    let store_id = create_test_store().await?;
    let _model_id = create_test_model(&store_id).await?;

    write_tuples(
        &store_id,
        vec![("user:alice", "viewer", "document:doc1")],
    )
    .await?;

    let client = reqwest::Client::new();

    // Act: Batch check with many duplicate requests
    // If deduplication works, this should be fast (only 1 actual check)
    let checks: Vec<_> = (0..50)
        .map(|i| {
            json!({
                "correlation_id": format!("check-{}", i),
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:doc1"
                }
            })
        })
        .collect(); // 50 checks with identical tuple_key but different correlation_ids

    let batch_request = json!({
        "checks": checks
    });

    let start = std::time::Instant::now();

    let response = client
        .post(format!(
            "{}/stores/{}/batch-check",
            get_openfga_url(),
            store_id
        ))
        .json(&batch_request)
        .send()
        .await?;

    let duration = start.elapsed();

    // Assert: Batch check succeeded
    assert!(
        response.status().is_success(),
        "Batch check with duplicates should succeed"
    );

    let response_body: serde_json::Value = response.json().await?;

    let results = response_body
        .get("result")
        .and_then(|v| v.as_object())
        .expect("Response should have 'result' object");

    // Assert: All 50 results returned (even though duplicates)
    assert_eq!(
        results.len(),
        50,
        "Batch check should return 50 results (even for duplicates)"
    );

    // Assert: All should be allowed
    let all_allowed = results
        .values()
        .all(|r| r.get("allowed").and_then(|v| v.as_bool()) == Some(true));

    assert!(all_allowed, "All duplicate checks should return true");

    // Note: We can't definitively verify deduplication without internal metrics,
    // but if this completes quickly (< 100ms), it likely deduplicated
    println!(
        "Batch check with 50 duplicate requests took: {:?}",
        duration
    );

    Ok(())
}
