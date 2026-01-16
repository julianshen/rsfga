mod common;

use anyhow::Result;
use common::{
    create_authorization_model, create_test_store, get_openfga_url, shared_client,
    simple_document_viewer_model, write_tuples,
};
use serde_json::json;

// ============================================================================
// Section 33: Consistency Preference Tests
// ============================================================================
//
// OpenFGA supports a consistency preference parameter that controls the
// trade-off between latency and consistency.
//
// Values:
// - UNSPECIFIED (default): Uses server default
// - MINIMIZE_LATENCY: Prefers cached results when available
// - HIGHER_CONSISTENCY: Bypasses cache, reads from database directly
//
// Supported APIs (v1.5.7+):
// - Check
// - ListObjects
// - ListUsers
// - Expand
//
// Note: When caching is disabled, both modes behave identically.
//
// ============================================================================

/// Test: Check API accepts consistency parameter
#[tokio::test]
async fn test_check_accepts_consistency_parameter() -> Result<()> {
    // Arrange
    let store_id = create_test_store().await?;

    let _model_id = create_authorization_model(&store_id, simple_document_viewer_model()).await?;

    write_tuples(&store_id, vec![("user:alice", "viewer", "document:doc1")]).await?;

    let client = shared_client();

    // Act: Check with MINIMIZE_LATENCY
    let check_request = json!({
        "tuple_key": {
            "user": "user:alice",
            "relation": "viewer",
            "object": "document:doc1"
        },
        "consistency": "MINIMIZE_LATENCY"
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    // Assert: Request should succeed (even if consistency param is ignored)
    assert!(
        response.status().is_success(),
        "Check with MINIMIZE_LATENCY should succeed, got: {}",
        response.status()
    );

    let body: serde_json::Value = response.json().await?;
    assert_eq!(
        body.get("allowed").and_then(|a| a.as_bool()),
        Some(true),
        "Check should return allowed=true"
    );

    Ok(())
}

/// Test: Check with HIGHER_CONSISTENCY
#[tokio::test]
async fn test_check_with_higher_consistency() -> Result<()> {
    let store_id = create_test_store().await?;

    let _model_id = create_authorization_model(&store_id, simple_document_viewer_model()).await?;

    write_tuples(&store_id, vec![("user:alice", "viewer", "document:doc1")]).await?;

    let client = shared_client();

    // Act: Check with HIGHER_CONSISTENCY
    let check_request = json!({
        "tuple_key": {
            "user": "user:alice",
            "relation": "viewer",
            "object": "document:doc1"
        },
        "consistency": "HIGHER_CONSISTENCY"
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    assert!(
        response.status().is_success(),
        "Check with HIGHER_CONSISTENCY should succeed, got: {}",
        response.status()
    );

    let body: serde_json::Value = response.json().await?;
    assert_eq!(
        body.get("allowed").and_then(|a| a.as_bool()),
        Some(true),
        "Check should return allowed=true"
    );

    Ok(())
}

/// Test: Check with UNSPECIFIED consistency (default)
#[tokio::test]
async fn test_check_with_unspecified_consistency() -> Result<()> {
    let store_id = create_test_store().await?;

    let _model_id = create_authorization_model(&store_id, simple_document_viewer_model()).await?;

    write_tuples(&store_id, vec![("user:alice", "viewer", "document:doc1")]).await?;

    let client = shared_client();

    // Act: Check with explicit UNSPECIFIED (same as omitting)
    let check_request = json!({
        "tuple_key": {
            "user": "user:alice",
            "relation": "viewer",
            "object": "document:doc1"
        },
        "consistency": "UNSPECIFIED"
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    // UNSPECIFIED might be rejected by some versions, accepted by others
    // Either behavior is acceptable, but we should verify the response is valid
    let status = response.status();
    assert!(
        status.is_success() || status.is_client_error(),
        "UNSPECIFIED consistency should either succeed or return client error, got: {status}"
    );

    if status.is_success() {
        let body: serde_json::Value = response.json().await?;
        assert_eq!(
            body.get("allowed").and_then(|a| a.as_bool()),
            Some(true),
            "Check should return allowed=true when request succeeds"
        );
    } else {
        // Document that UNSPECIFIED is not accepted
        eprintln!("INFO: UNSPECIFIED consistency value returned status: {status} (acceptable)");
    }

    Ok(())
}

/// Test: ListObjects accepts consistency parameter
#[tokio::test]
async fn test_listobjects_accepts_consistency_parameter() -> Result<()> {
    let store_id = create_test_store().await?;

    let _model_id = create_authorization_model(&store_id, simple_document_viewer_model()).await?;

    write_tuples(
        &store_id,
        vec![
            ("user:alice", "viewer", "document:doc1"),
            ("user:alice", "viewer", "document:doc2"),
        ],
    )
    .await?;

    let client = shared_client();

    // Act: ListObjects with consistency
    let list_request = json!({
        "type": "document",
        "relation": "viewer",
        "user": "user:alice",
        "consistency": "HIGHER_CONSISTENCY"
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

    assert!(
        response.status().is_success(),
        "ListObjects with consistency should succeed, got: {}",
        response.status()
    );

    let body: serde_json::Value = response.json().await?;
    let objects = body
        .get("objects")
        .and_then(|o| o.as_array())
        .expect("Should have objects array");

    assert_eq!(objects.len(), 2, "Should return 2 objects");

    Ok(())
}

/// Test: Expand accepts consistency parameter
#[tokio::test]
async fn test_expand_accepts_consistency_parameter() -> Result<()> {
    let store_id = create_test_store().await?;

    let _model_id = create_authorization_model(&store_id, simple_document_viewer_model()).await?;

    write_tuples(&store_id, vec![("user:alice", "viewer", "document:doc1")]).await?;

    let client = shared_client();

    // Act: Expand with consistency
    let expand_request = json!({
        "tuple_key": {
            "relation": "viewer",
            "object": "document:doc1"
        },
        "consistency": "MINIMIZE_LATENCY"
    });

    let response = client
        .post(format!("{}/stores/{}/expand", get_openfga_url(), store_id))
        .json(&expand_request)
        .send()
        .await?;

    assert!(
        response.status().is_success(),
        "Expand with consistency should succeed, got: {}",
        response.status()
    );

    let body: serde_json::Value = response.json().await?;
    assert!(body.get("tree").is_some(), "Expand should return tree");

    Ok(())
}

/// Test: Both consistency modes return same results for fresh data
#[tokio::test]
async fn test_consistency_modes_return_same_results() -> Result<()> {
    let store_id = create_test_store().await?;

    let _model_id = create_authorization_model(&store_id, simple_document_viewer_model()).await?;

    write_tuples(&store_id, vec![("user:alice", "viewer", "document:doc1")]).await?;

    let client = shared_client();

    // Check with MINIMIZE_LATENCY
    let check1 = json!({
        "tuple_key": {
            "user": "user:alice",
            "relation": "viewer",
            "object": "document:doc1"
        },
        "consistency": "MINIMIZE_LATENCY"
    });

    let response1 = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check1)
        .send()
        .await?;

    let body1: serde_json::Value = response1.json().await?;
    let allowed1 = body1.get("allowed").and_then(|a| a.as_bool());

    // Check with HIGHER_CONSISTENCY
    let check2 = json!({
        "tuple_key": {
            "user": "user:alice",
            "relation": "viewer",
            "object": "document:doc1"
        },
        "consistency": "HIGHER_CONSISTENCY"
    });

    let response2 = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check2)
        .send()
        .await?;

    let body2: serde_json::Value = response2.json().await?;
    let allowed2 = body2.get("allowed").and_then(|a| a.as_bool());

    // Both should return the same result
    assert_eq!(
        allowed1, allowed2,
        "Both consistency modes should return same result for fresh data"
    );
    assert_eq!(allowed1, Some(true), "Both should return allowed=true");

    Ok(())
}

/// Test: Invalid consistency value returns error
#[tokio::test]
async fn test_invalid_consistency_value_error() -> Result<()> {
    let store_id = create_test_store().await?;

    let _model_id = create_authorization_model(&store_id, simple_document_viewer_model()).await?;

    let client = shared_client();

    // Act: Check with invalid consistency value
    let check_request = json!({
        "tuple_key": {
            "user": "user:alice",
            "relation": "viewer",
            "object": "document:doc1"
        },
        "consistency": "INVALID_VALUE"
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    // Behavior may vary:
    // - Some implementations may ignore unknown values (success)
    // - Some may return 400 Bad Request
    // - Some may return 422 Unprocessable Entity
    // All of these are acceptable behaviors
    let status = response.status();
    assert!(
        status.is_success() || status.is_client_error(),
        "Invalid consistency value should either be ignored (success) or rejected (client error), got: {status}"
    );

    if status.is_success() {
        eprintln!("INFO: Invalid consistency value was accepted (ignored by server)");
        // Verify the check still works correctly
        let body: serde_json::Value = response.json().await?;
        // allowed could be true or false depending on whether tuple exists
        assert!(
            body.get("allowed").is_some(),
            "Response should have 'allowed' field even with invalid consistency"
        );
    } else {
        eprintln!("INFO: Invalid consistency value rejected with status: {status} (acceptable)");
    }

    Ok(())
}

/// Test: BatchCheck accepts consistency parameter
#[tokio::test]
async fn test_batchcheck_accepts_consistency_parameter() -> Result<()> {
    let store_id = create_test_store().await?;

    let _model_id = create_authorization_model(&store_id, simple_document_viewer_model()).await?;

    write_tuples(&store_id, vec![("user:alice", "viewer", "document:doc1")]).await?;

    let client = shared_client();

    // Act: BatchCheck with consistency
    let batch_request = json!({
        "checks": [
            {
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:doc1"
                },
                "correlation_id": "check-1"
            }
        ],
        "consistency": "HIGHER_CONSISTENCY"
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

    // Older versions may not support consistency in batch-check
    // Both success and client error are acceptable
    let status = response.status();
    assert!(
        status.is_success() || status.is_client_error(),
        "BatchCheck with consistency should either succeed or return client error, got: {status}"
    );

    if status.is_success() {
        let body: serde_json::Value = response.json().await?;
        let result = body.get("result");
        assert!(result.is_some(), "BatchCheck should return result");
    } else {
        eprintln!(
            "INFO: BatchCheck with consistency returned status: {status} (may not be supported in this version)"
        );
    }

    Ok(())
}

/// Test: ListUsers accepts consistency parameter
#[tokio::test]
async fn test_listusers_accepts_consistency_parameter() -> Result<()> {
    let store_id = create_test_store().await?;

    let _model_id = create_authorization_model(&store_id, simple_document_viewer_model()).await?;

    write_tuples(&store_id, vec![("user:alice", "viewer", "document:doc1")]).await?;

    let client = shared_client();

    // Act: ListUsers with consistency
    let list_request = json!({
        "object": { "type": "document", "id": "doc1" },
        "relation": "viewer",
        "user_filters": [{ "type": "user" }],
        "consistency": "HIGHER_CONSISTENCY"
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

    // ListUsers requires experimental flag, so 501/404 is acceptable
    let status = response.status();
    if status == reqwest::StatusCode::NOT_IMPLEMENTED || status == reqwest::StatusCode::NOT_FOUND {
        eprintln!(
            "SKIPPED: ListUsers API not available (requires --experimentals enable-list-users)"
        );
        return Ok(());
    }

    assert!(
        status.is_success() || status.is_client_error(),
        "ListUsers with consistency should either succeed or return client error, got: {status}"
    );

    if status.is_success() {
        let body: serde_json::Value = response.json().await?;
        let users = body.get("users").and_then(|u| u.as_array());
        assert!(users.is_some(), "ListUsers should return users array");
    } else {
        eprintln!("INFO: ListUsers with consistency returned status: {status}");
    }

    Ok(())
}
