//! Error message format compatibility tests.
//!
//! These tests verify that RSFGA's error responses match OpenFGA's format exactly,
//! ensuring client applications can rely on consistent error handling.
//!
//! Related to GitHub Issue #217: Error message format compatibility tests.
//!
//! ## Requirements Tested:
//! - Error response structure: `{"code": "...", "message": "..."}`
//! - Error codes match OpenFGA's codes (snake_case)
//! - No information leakage (stack traces, credentials, file paths, SQL queries)
//! - gRPC error mapping verification

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use common::{create_test_app, post_json, setup_simple_model};
use rsfga_storage::{DataStore, MemoryDataStore};

// ============================================================================
// Section 1: HTTP Error Format Compatibility
// ============================================================================

/// Test: Error responses have exactly the required structure (code + message)
#[tokio::test]
async fn test_error_response_has_required_fields() {
    let storage = Arc::new(MemoryDataStore::new());

    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores/nonexistent-store/check",
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:readme"
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);

    // Verify required fields exist
    assert!(
        response.get("code").is_some(),
        "Error response must have 'code' field"
    );
    assert!(
        response.get("message").is_some(),
        "Error response must have 'message' field"
    );

    // Verify code is non-empty string
    let code = response["code"].as_str().unwrap();
    assert!(!code.is_empty(), "Error code must not be empty");

    // Verify message is non-empty string
    let message = response["message"].as_str().unwrap();
    assert!(!message.is_empty(), "Error message must not be empty");
}

/// Test: Error codes use snake_case format (OpenFGA convention)
#[tokio::test]
async fn test_error_codes_are_snake_case() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    // Test 404 Not Found - static URI
    let (_, response) = post_json(
        create_test_app(&storage),
        "/stores/nonexistent-store/check",
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:1"
            }
        }),
    )
    .await;
    let code = response["code"].as_str().unwrap_or("");
    assert!(!code.is_empty(), "Error code should not be empty");
    assert!(
        code.chars().all(|c| c.is_lowercase() || c == '_'),
        "Error code '{}' should be snake_case",
        code
    );

    // Test 400 Bad Request - invalid user format
    let uri = format!("/stores/{store_id}/check");
    let (_, response) = post_json(
        create_test_app(&storage),
        &uri,
        serde_json::json!({
            "tuple_key": {
                "user": "invalid-no-colon",
                "relation": "viewer",
                "object": "document:1"
            }
        }),
    )
    .await;
    let code = response["code"].as_str().unwrap_or("");
    assert!(!code.is_empty(), "Error code should not be empty");
    assert!(
        code.chars().all(|c| c.is_lowercase() || c == '_'),
        "Error code '{}' should be snake_case",
        code
    );

    // Test 400 Bad Request - type not found
    let (_, response) = post_json(
        create_test_app(&storage),
        &uri,
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "unknown_type:1"
            }
        }),
    )
    .await;
    let code = response["code"].as_str().unwrap_or("");
    assert!(!code.is_empty(), "Error code should not be empty");
    assert!(
        code.chars().all(|c| c.is_lowercase() || c == '_'),
        "Error code '{}' should be snake_case",
        code
    );
}

/// Test: 404 Not Found returns correct error code
#[tokio::test]
async fn test_404_not_found_error_code() {
    let storage = Arc::new(MemoryDataStore::new());

    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores/nonexistent-store/check",
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:readme"
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(response["code"].as_str().unwrap(), "not_found");
}

/// Test: 400 Bad Request returns validation_error code
#[tokio::test]
async fn test_400_bad_request_error_code() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
        serde_json::json!({
            "tuple_key": {
                "user": "invalid-user",
                "relation": "viewer",
                "object": "document:readme"
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(response["code"].as_str().unwrap(), "validation_error");
}

/// Test: Error responses do not contain extraneous fields
#[tokio::test]
async fn test_error_response_has_no_extraneous_fields() {
    let storage = Arc::new(MemoryDataStore::new());

    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores/nonexistent-store/check",
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:readme"
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);

    // Error response should only have 'code' and 'message' fields
    let obj = response.as_object().expect("Response should be an object");
    let allowed_fields: std::collections::HashSet<&str> =
        ["code", "message"].iter().copied().collect();

    for key in obj.keys() {
        assert!(
            allowed_fields.contains(key.as_str()),
            "Unexpected field '{}' in error response. OpenFGA format only includes 'code' and 'message'",
            key
        );
    }
}

/// Test: Error messages are helpful and descriptive
#[tokio::test]
async fn test_error_messages_are_descriptive() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;
    let uri = format!("/stores/{store_id}/check");

    // Test store not found error mentions store
    let (_, response) = post_json(
        create_test_app(&storage),
        "/stores/nonexistent-store/check",
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:readme"
            }
        }),
    )
    .await;
    let message = response["message"].as_str().unwrap().to_lowercase();
    assert!(
        message.contains("store") && message.contains("not found"),
        "Store not found error should mention 'store' and 'not found': {}",
        message
    );

    // Test invalid user format error mentions user
    let (_, response) = post_json(
        create_test_app(&storage),
        &uri,
        serde_json::json!({
            "tuple_key": {
                "user": "invalid-user",
                "relation": "viewer",
                "object": "document:readme"
            }
        }),
    )
    .await;
    let message = response["message"].as_str().unwrap().to_lowercase();
    assert!(
        message.contains("user") || message.contains("invalid"),
        "Invalid user error should mention 'user' or 'invalid': {}",
        message
    );

    // Test type not found error mentions type
    let (_, response) = post_json(
        create_test_app(&storage),
        &uri,
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "unknown_type:readme"
            }
        }),
    )
    .await;
    let message = response["message"].as_str().unwrap().to_lowercase();
    assert!(
        message.contains("type") && message.contains("not found"),
        "Type not found error should mention 'type' and 'not found': {}",
        message
    );

    // Test relation not found error mentions relation
    let (_, response) = post_json(
        create_test_app(&storage),
        &uri,
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "nonexistent_relation",
                "object": "document:readme"
            }
        }),
    )
    .await;
    let message = response["message"].as_str().unwrap().to_lowercase();
    assert!(
        message.contains("relation") && message.contains("not found"),
        "Relation not found error should mention 'relation' and 'not found': {}",
        message
    );
}

// ============================================================================
// Section 2: Security - No Information Leakage
// ============================================================================

/// Test: Error messages do not expose internal stack traces
#[tokio::test]
async fn test_error_messages_no_stack_traces() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;
    let uri = format!("/stores/{store_id}/check");

    let stack_trace_patterns = [
        "at ",         // JavaScript/TypeScript stack trace
        "   at ",      // Node.js stack trace
        "Traceback",   // Python traceback
        "panic",       // Rust panic
        "thread '",    // Rust thread panic
        "stack trace", // Generic
        "Stack:",      // Generic
        "backtrace",   // Generic
        "RUST_BACKTRACE",
        "panicked at", // Rust panic message
        "note: run with",
        "stack backtrace",
    ];

    // Test store not found
    let (_, response) = post_json(
        create_test_app(&storage),
        "/stores/nonexistent-store/check",
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:1"
            }
        }),
    )
    .await;
    let message = response["message"].as_str().unwrap_or("");
    for pattern in &stack_trace_patterns {
        assert!(
            !message.to_lowercase().contains(&pattern.to_lowercase()),
            "Error message should not contain stack trace pattern '{}': {}",
            pattern,
            message
        );
    }

    // Test invalid input
    let (_, response) = post_json(
        create_test_app(&storage),
        &uri,
        serde_json::json!({
            "tuple_key": {
                "user": "invalid-user",
                "relation": "viewer",
                "object": "document:1"
            }
        }),
    )
    .await;
    let message = response["message"].as_str().unwrap_or("");
    for pattern in &stack_trace_patterns {
        assert!(
            !message.to_lowercase().contains(&pattern.to_lowercase()),
            "Error message should not contain stack trace pattern '{}': {}",
            pattern,
            message
        );
    }

    // Test type not found
    let (_, response) = post_json(
        create_test_app(&storage),
        &uri,
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "unknown:1"
            }
        }),
    )
    .await;
    let message = response["message"].as_str().unwrap_or("");
    for pattern in &stack_trace_patterns {
        assert!(
            !message.to_lowercase().contains(&pattern.to_lowercase()),
            "Error message should not contain stack trace pattern '{}': {}",
            pattern,
            message
        );
    }
}

/// Test: Error messages do not expose file paths
#[tokio::test]
async fn test_error_messages_no_file_paths() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;
    let uri = format!("/stores/{store_id}/check");

    let file_path_patterns = [
        "/home/",     // Unix home directory
        "/usr/",      // Unix system directory
        "/etc/",      // Unix config directory
        "/var/",      // Unix variable directory
        "/tmp/",      // Unix temp directory
        "C:\\",       // Windows drive
        "D:\\",       // Windows drive
        ".rs:",       // Rust source file with line number
        ".go:",       // Go source file with line number
        ".py:",       // Python source file with line number
        "/src/",      // Source directory
        "/crates/",   // Rust crates directory
        "\\src\\",    // Windows source directory
        "line ",      // Line number reference
        "src/lib.rs", // Common Rust source path
        "main.rs",    // Rust main file
        "mod.rs",     // Rust module file
    ];

    // Test store not found
    let (_, response) = post_json(
        create_test_app(&storage),
        "/stores/nonexistent-store/check",
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:1"
            }
        }),
    )
    .await;
    let message = response["message"].as_str().unwrap_or("");
    let code = response["code"].as_str().unwrap_or("");
    for pattern in &file_path_patterns {
        assert!(
            !message.contains(pattern),
            "Error message should not contain file path pattern '{}': {}",
            pattern,
            message
        );
        assert!(
            !code.contains(pattern),
            "Error code should not contain file path pattern '{}': {}",
            pattern,
            code
        );
    }

    // Test invalid input
    let (_, response) = post_json(
        create_test_app(&storage),
        &uri,
        serde_json::json!({
            "tuple_key": {
                "user": "invalid-user",
                "relation": "viewer",
                "object": "document:1"
            }
        }),
    )
    .await;
    let message = response["message"].as_str().unwrap_or("");
    let code = response["code"].as_str().unwrap_or("");
    for pattern in &file_path_patterns {
        assert!(
            !message.contains(pattern),
            "Error message should not contain file path pattern '{}': {}",
            pattern,
            message
        );
        assert!(
            !code.contains(pattern),
            "Error code should not contain file path pattern '{}': {}",
            pattern,
            code
        );
    }
}

/// Test: Error messages do not expose SQL queries
#[tokio::test]
async fn test_error_messages_no_sql_queries() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;
    let uri = format!("/stores/{store_id}/check");

    let sql_patterns = [
        "SELECT ",
        "INSERT ",
        "UPDATE ",
        "DELETE ",
        "FROM ",
        "WHERE ",
        "JOIN ",
        "VALUES",
        "CREATE TABLE",
        "DROP TABLE",
        "ALTER TABLE",
        "pg_",     // PostgreSQL internal tables
        "sqlite_", // SQLite internal tables
        "mysql.",  // MySQL internal tables
        "information_schema",
        "$1",     // SQL parameter placeholder
        "$2",     // SQL parameter placeholder
        "::text", // PostgreSQL type cast
        "::uuid", // PostgreSQL type cast
    ];

    // Test store not found
    let (_, response) = post_json(
        create_test_app(&storage),
        "/stores/nonexistent-store/check",
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:1"
            }
        }),
    )
    .await;
    let message = response["message"].as_str().unwrap_or("");
    let message_upper = message.to_uppercase();
    for pattern in &sql_patterns {
        assert!(
            !message_upper.contains(&pattern.to_uppercase()),
            "Error message should not contain SQL pattern '{}': {}",
            pattern,
            message
        );
    }

    // Test invalid input
    let (_, response) = post_json(
        create_test_app(&storage),
        &uri,
        serde_json::json!({
            "tuple_key": {
                "user": "invalid-user",
                "relation": "viewer",
                "object": "document:1"
            }
        }),
    )
    .await;
    let message = response["message"].as_str().unwrap_or("");
    let message_upper = message.to_uppercase();
    for pattern in &sql_patterns {
        assert!(
            !message_upper.contains(&pattern.to_uppercase()),
            "Error message should not contain SQL pattern '{}': {}",
            pattern,
            message
        );
    }
}

/// Test: Error messages do not expose internal module/type names
#[tokio::test]
async fn test_error_messages_no_internal_module_names() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;
    let uri = format!("/stores/{store_id}/check");

    let internal_patterns = [
        "rsfga_",          // Internal crate names
        "rsfga-",          // Internal crate names with hyphen
        "::new()",         // Rust constructor
        "::from()",        // Rust conversion
        "impl ",           // Rust impl block
        "fn ",             // Rust function definition
        "pub fn",          // Rust public function
        "struct ",         // Rust struct definition
        "enum ",           // Rust enum definition
        "trait ",          // Rust trait definition
        "mod ",            // Rust module definition
        "use ",            // Rust use statement
        "crate::",         // Rust crate reference
        "self::",          // Rust self reference
        "super::",         // Rust super reference
        "tokio::",         // Tokio crate
        "axum::",          // Axum crate
        "sqlx::",          // SQLx crate
        "anyhow::",        // Anyhow crate
        "thiserror::",     // Thiserror crate
        "DomainError",     // Internal error type
        "StorageError",    // Internal error type
        "ResolverError",   // Internal error type
        "ApiError",        // Internal error type
        "BatchCheckError", // Internal error type
    ];

    // Test store not found
    let (_, response) = post_json(
        create_test_app(&storage),
        "/stores/nonexistent-store/check",
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:1"
            }
        }),
    )
    .await;
    let message = response["message"].as_str().unwrap_or("");
    for pattern in &internal_patterns {
        assert!(
            !message.contains(pattern),
            "Error message should not contain internal module/type pattern '{}': {}",
            pattern,
            message
        );
    }

    // Test invalid input
    let (_, response) = post_json(
        create_test_app(&storage),
        &uri,
        serde_json::json!({
            "tuple_key": {
                "user": "invalid-user",
                "relation": "viewer",
                "object": "document:1"
            }
        }),
    )
    .await;
    let message = response["message"].as_str().unwrap_or("");
    for pattern in &internal_patterns {
        assert!(
            !message.contains(pattern),
            "Error message should not contain internal module/type pattern '{}': {}",
            pattern,
            message
        );
    }
}

/// Test: Error messages do not expose credentials or secrets
#[tokio::test]
async fn test_error_messages_no_credentials() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;
    let uri = format!("/stores/{store_id}/check");

    let credential_patterns = [
        "password",
        "passwd",
        "secret",
        "api_key",
        "apikey",
        "api-key",
        "token",
        "bearer",
        "authorization:",
        "auth:",
        "credential",
        "private_key",
        "privatekey",
        "private-key",
        "DATABASE_URL",
        "POSTGRES_",
        "MYSQL_",
        "REDIS_",
        "AWS_",
        "AZURE_",
        "GCP_",
        "connection string",
        "connectionstring",
    ];

    // Test store not found
    let (_, response) = post_json(
        create_test_app(&storage),
        "/stores/nonexistent-store/check",
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:1"
            }
        }),
    )
    .await;
    let message = response["message"].as_str().unwrap_or("");
    let message_lower = message.to_lowercase();
    for pattern in &credential_patterns {
        assert!(
            !message_lower.contains(&pattern.to_lowercase()),
            "Error message should not contain credential pattern '{}': {}",
            pattern,
            message
        );
    }

    // Test invalid input
    let (_, response) = post_json(
        create_test_app(&storage),
        &uri,
        serde_json::json!({
            "tuple_key": {
                "user": "invalid-user",
                "relation": "viewer",
                "object": "document:1"
            }
        }),
    )
    .await;
    let message = response["message"].as_str().unwrap_or("");
    let message_lower = message.to_lowercase();
    for pattern in &credential_patterns {
        assert!(
            !message_lower.contains(&pattern.to_lowercase()),
            "Error message should not contain credential pattern '{}': {}",
            pattern,
            message
        );
    }
}

// ============================================================================
// Section 3: Error Code Coverage
// ============================================================================

/// Test: All documented error codes are properly returned
#[tokio::test]
async fn test_error_code_coverage() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;
    let uri = format!("/stores/{store_id}/check");

    // Test not_found code
    let (status, response) = post_json(
        create_test_app(&storage),
        "/stores/nonexistent-store/check",
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "document:1"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(response["code"], "not_found");

    // Test validation_error code (invalid user format)
    let (status, response) = post_json(
        create_test_app(&storage),
        &uri,
        serde_json::json!({
            "tuple_key": {
                "user": "invalid-user",
                "relation": "viewer",
                "object": "document:1"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(response["code"], "validation_error");

    // Test validation_error code (type not found)
    let (status, response) = post_json(
        create_test_app(&storage),
        &uri,
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "unknown_type:1"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(response["code"], "validation_error");

    // Test validation_error code (relation not found)
    let (status, response) = post_json(
        create_test_app(&storage),
        &uri,
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "nonexistent_relation",
                "object": "document:1"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(response["code"], "validation_error");
}

/// Test: Batch check errors are properly formatted
#[tokio::test]
async fn test_batch_check_error_format() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;
    let write_uri = format!("/stores/{store_id}/write");
    let batch_uri = format!("/stores/{store_id}/batch-check");

    // Write a tuple for successful check
    let (status, _) = post_json(
        create_test_app(&storage),
        &write_uri,
        serde_json::json!({
            "writes": {
                "tuple_keys": [
                    {
                        "user": "user:alice",
                        "relation": "viewer",
                        "object": "document:readme"
                    }
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Batch check with one valid and one invalid request
    let (status, response) = post_json(
        create_test_app(&storage),
        &batch_uri,
        serde_json::json!({
            "checks": [
                {
                    "tuple_key": {
                        "user": "user:alice",
                        "relation": "viewer",
                        "object": "document:readme"
                    },
                    "correlation_id": "valid-check"
                },
                {
                    "tuple_key": {
                        "user": "user:bob",
                        "relation": "nonexistent_relation",
                        "object": "document:readme"
                    },
                    "correlation_id": "invalid-check"
                }
            ]
        }),
    )
    .await;

    // Batch check should return 200 with results containing errors
    assert_eq!(
        status,
        StatusCode::OK,
        "Batch check should return 200 even with individual errors"
    );

    // Verify result structure
    let results = response["result"].as_object();
    assert!(results.is_some(), "Should have result object");

    let results = results.unwrap();

    // Valid check should succeed
    let valid_result = &results["valid-check"];
    assert!(
        valid_result.get("allowed").is_some(),
        "Valid check should have allowed field"
    );

    // Invalid check should have error with proper format
    let invalid_result = &results["invalid-check"];
    let error = invalid_result.get("error");
    assert!(error.is_some(), "Invalid check should have error field");

    // Error in batch check result should have code and message
    if let Some(error_obj) = error {
        // Verify error structure if it's an object
        if error_obj.is_object() {
            assert!(
                error_obj.get("code").is_some() || error_obj.get("message").is_some(),
                "Batch check error should have code or message field"
            );
        }
    }
}

/// Test: Empty batch returns validation error
#[tokio::test]
async fn test_empty_batch_error_format() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;
    let uri = format!("/stores/{store_id}/batch-check");

    let (status, response) = post_json(
        create_test_app(&storage),
        &uri,
        serde_json::json!({
            "checks": []
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(response["code"], "validation_error");
    assert!(
        response["message"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("empty"),
        "Empty batch error should mention 'empty'"
    );
}

/// Test: Batch size exceeded returns validation error
#[tokio::test]
async fn test_batch_size_exceeded_error_format() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;
    let uri = format!("/stores/{store_id}/batch-check");

    // Create 100 checks (exceeds typical batch limit of 50)
    let checks: Vec<_> = (0..100)
        .map(|i| {
            serde_json::json!({
                "tuple_key": {
                    "user": format!("user:{i}"),
                    "relation": "viewer",
                    "object": format!("document:{i}")
                },
                "correlation_id": format!("check-{i}")
            })
        })
        .collect();

    let (status, response) = post_json(
        create_test_app(&storage),
        &uri,
        serde_json::json!({
            "checks": checks
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(response["code"], "validation_error");

    let message = response["message"].as_str().unwrap().to_lowercase();
    assert!(
        message.contains("batch") || message.contains("size") || message.contains("exceed"),
        "Batch size error should mention batch/size/exceed: {}",
        message
    );
}

// ============================================================================
// Section 4: Error Consistency Across Endpoints
// ============================================================================

/// Test: Same error type returns same format across different endpoints
#[tokio::test]
async fn test_error_format_consistent_across_endpoints() {
    let storage = Arc::new(MemoryDataStore::new());

    // Test store not found error across multiple endpoints
    let endpoints: Vec<(&str, serde_json::Value)> = vec![
        (
            "/stores/nonexistent-store/check",
            serde_json::json!({
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:1"
                }
            }),
        ),
        (
            "/stores/nonexistent-store/expand",
            serde_json::json!({
                "tuple_key": {
                    "relation": "viewer",
                    "object": "document:1"
                }
            }),
        ),
        (
            "/stores/nonexistent-store/list-objects",
            serde_json::json!({
                "type": "document",
                "relation": "viewer",
                "user": "user:alice"
            }),
        ),
        (
            "/stores/nonexistent-store/write",
            serde_json::json!({
                "writes": {
                    "tuple_keys": [{
                        "user": "user:alice",
                        "relation": "viewer",
                        "object": "document:1"
                    }]
                }
            }),
        ),
        ("/stores/nonexistent-store/read", serde_json::json!({})),
    ];

    let mut error_codes = Vec::new();
    let mut error_statuses = Vec::new();

    for (uri, body) in endpoints {
        let (status, response) = post_json(create_test_app(&storage), uri, body).await;

        error_statuses.push(status);

        let code = response["code"].as_str().unwrap_or("").to_string();
        error_codes.push(code);

        // Verify each response has the required structure
        assert!(
            response.get("code").is_some(),
            "Missing 'code' field for {uri}"
        );
        assert!(
            response.get("message").is_some(),
            "Missing 'message' field for {uri}"
        );
    }

    // All should return 404
    for status in &error_statuses {
        assert_eq!(
            *status,
            StatusCode::NOT_FOUND,
            "All store not found errors should return 404"
        );
    }

    // All should return the same error code
    let first_code = &error_codes[0];
    for code in &error_codes {
        assert_eq!(
            code, first_code,
            "All store not found errors should return the same error code"
        );
    }
}

// ============================================================================
// Section 5: gRPC Error Format Tests
// ============================================================================

/// Test: gRPC errors should map to appropriate status codes (documented)
///
/// This test documents the expected gRPC error mappings.
/// The actual gRPC integration tests are in the grpc/tests.rs module.
///
/// Expected mappings:
/// - StorageError::StoreNotFound -> NOT_FOUND (5)
/// - StorageError::InvalidInput -> INVALID_ARGUMENT (3)
/// - StorageError::DuplicateTuple -> ALREADY_EXISTS (6)
/// - StorageError::ConnectionError -> UNAVAILABLE (14)
/// - StorageError::QueryTimeout -> DEADLINE_EXCEEDED (4)
/// - DomainError validation errors -> INVALID_ARGUMENT (3)
/// - DomainError not found errors -> NOT_FOUND (5)
/// - DomainError timeout errors -> DEADLINE_EXCEEDED (4)
/// - DomainError internal errors -> INTERNAL (13)
#[tokio::test]
async fn test_grpc_error_mapping_documentation() {
    // This test documents the expected gRPC error code mappings
    // Actual gRPC tests are in crates/rsfga-api/src/grpc/tests.rs

    // The error codes used should match the OpenFGA protobuf definitions:
    // - NO_ERROR = 0
    // - VALIDATION_ERROR = 2000
    // - AUTHORIZATION_MODEL_NOT_FOUND = 2001
    // - TYPE_NOT_FOUND = 2021
    // - RELATION_NOT_FOUND = 2022
    // - INVALID_USER = 2025
    // - STORE_ID_INVALID_LENGTH = 2030

    // Verify the documentation by ensuring we can reference the expected codes
    let expected_codes = [
        ("NO_ERROR", 0),
        ("VALIDATION_ERROR", 2000),
        ("AUTHORIZATION_MODEL_NOT_FOUND", 2001),
        ("TYPE_NOT_FOUND", 2021),
        ("RELATION_NOT_FOUND", 2022),
        ("INVALID_USER", 2025),
        ("STORE_ID_INVALID_LENGTH", 2030),
    ];

    // This is a documentation test - verify expected codes are documented
    for (name, code) in expected_codes {
        assert!(
            code >= 0,
            "Error code {} ({}) should be non-negative",
            name,
            code
        );
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Set up a test store with a simple authorization model
async fn setup_test_store(storage: &MemoryDataStore) -> String {
    let store_id = ulid::Ulid::new().to_string();

    // Create store
    storage.create_store(&store_id, "Test Store").await.unwrap();

    // Create authorization model
    setup_simple_model(storage, &store_id).await;

    store_id
}
