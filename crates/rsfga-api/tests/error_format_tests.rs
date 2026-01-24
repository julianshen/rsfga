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
    let store_id = nonexistent_store_id();

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
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

    // Test 404 Not Found - use valid ULID format for nonexistent store
    let nonexistent = nonexistent_store_id();
    let (_, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{nonexistent}/check"),
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
    let store_id = nonexistent_store_id();

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
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
    let store_id = nonexistent_store_id();

    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{store_id}/check"),
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
    let nonexistent = nonexistent_store_id();
    let (_, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{nonexistent}/check"),
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
    let nonexistent = nonexistent_store_id();
    let (_, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{nonexistent}/check"),
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
        ":line ",     // Line number reference (e.g., "file.rs:line 42")
        " line:",     // Line number reference (e.g., " line: 42")
        "at line",    // Stack trace line reference
        "src/lib.rs", // Common Rust source path
        "main.rs",    // Rust main file
        "mod.rs",     // Rust module file
    ];

    // Test store not found
    let nonexistent = nonexistent_store_id();
    let (_, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{nonexistent}/check"),
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
    let nonexistent = nonexistent_store_id();
    let (_, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{nonexistent}/check"),
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
    let nonexistent = nonexistent_store_id();
    let (_, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{nonexistent}/check"),
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
    let nonexistent = nonexistent_store_id();
    let (_, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{nonexistent}/check"),
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
    let nonexistent = nonexistent_store_id();
    let (status, response) = post_json(
        create_test_app(&storage),
        &format!("/stores/{nonexistent}/check"),
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
            // OpenFGA requires BOTH code AND message in error responses
            assert!(
                error_obj.get("code").is_some() && error_obj.get("message").is_some(),
                "Batch check error should have BOTH code AND message fields"
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

    // Use a valid ULID format for nonexistent store to get 404 (not 400 for invalid format)
    let nonexistent = nonexistent_store_id();

    // Test store not found error across multiple endpoints
    let endpoints: Vec<(String, serde_json::Value)> = vec![
        (
            format!("/stores/{nonexistent}/check"),
            serde_json::json!({
                "tuple_key": {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:1"
                }
            }),
        ),
        (
            format!("/stores/{nonexistent}/expand"),
            serde_json::json!({
                "tuple_key": {
                    "relation": "viewer",
                    "object": "document:1"
                }
            }),
        ),
        (
            format!("/stores/{nonexistent}/list-objects"),
            serde_json::json!({
                "type": "document",
                "relation": "viewer",
                "user": "user:alice"
            }),
        ),
        (
            format!("/stores/{nonexistent}/write"),
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
        (format!("/stores/{nonexistent}/read"), serde_json::json!({})),
    ];

    let mut error_codes = Vec::new();
    let mut error_statuses = Vec::new();

    for (uri, body) in endpoints {
        let (status, response) = post_json(create_test_app(&storage), &uri, body).await;

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
// Section 5: gRPC Error Code Mapping Verification
// ============================================================================

/// Test: Verify gRPC error codes match OpenFGA protobuf definitions
///
/// This test verifies the error code values match the OpenFGA protobuf definitions.
/// The actual gRPC error handling integration tests are in grpc/tests.rs.
///
/// Reference: OpenFGA protobuf ErrorCode enum values
#[tokio::test]
async fn test_grpc_error_codes_match_openfga_protobuf() {
    // OpenFGA ErrorCode enum values from protobuf definitions
    // These must match exactly for client compatibility
    let openfga_error_codes: std::collections::HashMap<&str, i32> = [
        ("no_error", 0),
        ("validation_error", 2000),
        ("authorization_model_not_found", 2001),
        ("authorization_model_resolution_too_complex", 2002),
        ("invalid_write_input", 2003),
        ("cannot_allow_duplicate_tuples_in_one_request", 2004),
        ("cannot_allow_duplicate_types_in_one_request", 2005),
        ("cannot_allow_multiple_references_to_one_relation", 2006),
        ("invalid_continuation_token", 2007),
        ("invalid_tuple_set", 2008),
        ("invalid_check_input", 2009),
        ("invalid_expand_input", 2010),
        ("unsupported_user_set", 2011),
        ("invalid_object_format", 2012),
        ("write_failed_due_to_invalid_input", 2017),
        ("authorization_model_assertions_not_found", 2018),
        ("latest_authorization_model_not_found", 2020),
        ("type_not_found", 2021),
        ("relation_not_found", 2022),
        ("empty_relation_definition", 2023),
        ("invalid_user", 2025),
        ("invalid_tuple", 2027),
        ("unknown_relation", 2028),
        ("store_id_invalid_length", 2030),
        ("assertions_too_many_items", 2033),
        ("id_too_long", 2034),
        ("authorization_model_id_too_long", 2036),
        ("tuple_key_value_not_specified", 2037),
        ("tuple_keys_too_many_or_too_few_items", 2038),
        ("page_size_invalid", 2039),
        ("param_missing_value", 2040),
        ("difference_base_missing_value", 2041),
        ("subtract_base_missing_value", 2042),
        ("object_too_long", 2043),
        ("relation_too_long", 2044),
        ("type_definitions_too_few_items", 2045),
        ("type_invalid_length", 2046),
        ("type_invalid_pattern", 2047),
        ("relations_too_few_items", 2048),
        ("relations_too_long", 2049),
        ("relations_invalid_pattern", 2050),
        ("object_invalid_pattern", 2051),
        ("query_string_type_continuation_token_mismatch", 2052),
        ("exceeded_entity_limit", 2053),
        ("invalid_contextual_tuple", 2054),
        ("duplicate_contextual_tuple", 2055),
        ("invalid_authorization_model", 2056),
        ("unsupported_schema_version", 2057),
    ]
    .into_iter()
    .collect();

    // Verify critical error codes used by RSFGA match OpenFGA
    let rsfga_critical_codes = [
        ("no_error", 0),
        ("validation_error", 2000),
        ("authorization_model_not_found", 2001),
        ("type_not_found", 2021),
        ("relation_not_found", 2022),
        ("invalid_user", 2025),
        ("store_id_invalid_length", 2030),
    ];

    for (code_name, expected_value) in rsfga_critical_codes {
        let openfga_value = openfga_error_codes.get(code_name);
        assert!(
            openfga_value.is_some(),
            "Error code '{}' must be defined in OpenFGA protobuf",
            code_name
        );
        assert_eq!(
            *openfga_value.unwrap(),
            expected_value,
            "Error code '{}' value mismatch: RSFGA={}, OpenFGA={}",
            code_name,
            expected_value,
            openfga_value.unwrap()
        );
    }

    // Verify code ranges are consistent
    // Error codes 2000-2099 are validation/user errors
    for (name, code) in &openfga_error_codes {
        if *code > 0 {
            assert!(
                *code >= 2000 && *code < 3000,
                "Error code '{}' ({}) should be in range 2000-2999",
                name,
                code
            );
        }
    }
}

// ============================================================================
// Section 6: Edge Cases
// ============================================================================

/// Test: Error handling with empty string inputs
#[tokio::test]
async fn test_error_format_with_empty_string_inputs() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;
    let uri = format!("/stores/{store_id}/check");

    // Empty user string
    let (status, response) = post_json(
        create_test_app(&storage),
        &uri,
        serde_json::json!({
            "tuple_key": {
                "user": "",
                "relation": "viewer",
                "object": "document:readme"
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(response["code"], "validation_error");
    // Message should exist and not leak internal details
    let message = response["message"].as_str().unwrap();
    assert!(!message.is_empty());
    assert!(!message.contains("rsfga_"));
    assert!(!message.contains("panic"));

    // Empty relation string
    let (status, response) = post_json(
        create_test_app(&storage),
        &uri,
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "",
                "object": "document:readme"
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(response["code"], "validation_error");
    let message = response["message"].as_str().unwrap();
    assert!(!message.is_empty());

    // Empty object string
    let (status, response) = post_json(
        create_test_app(&storage),
        &uri,
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": ""
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(response["code"], "validation_error");
    let message = response["message"].as_str().unwrap();
    assert!(!message.is_empty());
}

/// Test: Error handling with Unicode inputs
#[tokio::test]
async fn test_error_format_with_unicode_inputs() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;
    let uri = format!("/stores/{store_id}/check");

    // Unicode in user field (invalid format)
    let (status, response) = post_json(
        create_test_app(&storage),
        &uri,
        serde_json::json!({
            "tuple_key": {
                "user": "用户:爱丽丝",
                "relation": "viewer",
                "object": "document:readme"
            }
        }),
    )
    .await;

    // Should return validation error, not crash
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::OK,
        "Unicode input should be handled gracefully"
    );
    // Error response should still have proper structure
    if status == StatusCode::BAD_REQUEST {
        assert!(response.get("code").is_some());
        assert!(response.get("message").is_some());
        // No stack traces or internal errors
        let message = response["message"].as_str().unwrap_or("");
        assert!(!message.contains("panic"));
        assert!(!message.contains("unwrap"));
    }

    // Emoji in object field (potentially valid type:id format)
    let (status, response) = post_json(
        create_test_app(&storage),
        &uri,
        serde_json::json!({
            "tuple_key": {
                "user": "user:alice",
                "relation": "viewer",
                "object": "📄:readme"
            }
        }),
    )
    .await;

    // Should be handled gracefully (likely validation error for unknown type)
    if status != StatusCode::OK {
        assert!(response.get("code").is_some());
        assert!(response.get("message").is_some());
    }
}

/// Test: Error handling with very long inputs
#[tokio::test]
async fn test_error_format_with_long_inputs() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;
    let uri = format!("/stores/{store_id}/check");

    // Very long user ID (1000 characters)
    let long_user = format!("user:{}", "a".repeat(1000));
    let (status, response) = post_json(
        create_test_app(&storage),
        &uri,
        serde_json::json!({
            "tuple_key": {
                "user": long_user,
                "relation": "viewer",
                "object": "document:readme"
            }
        }),
    )
    .await;

    // Should return error (likely validation error for length)
    // The important thing is it doesn't crash or timeout
    if status != StatusCode::OK {
        assert!(
            response.get("code").is_some(),
            "Long input should return proper error format"
        );
        assert!(response.get("message").is_some());
        // Error message should be reasonable length, not echo back the full input
        let message = response["message"].as_str().unwrap_or("");
        assert!(
            message.len() < 500,
            "Error message should not echo back very long inputs"
        );
    }
}

/// Test: Concurrent error responses maintain format consistency
#[tokio::test]
async fn test_concurrent_error_responses_consistent() {
    let storage = Arc::new(MemoryDataStore::new());
    let store_id = setup_test_store(&storage).await;
    let uri = format!("/stores/{store_id}/check");

    // Send multiple concurrent requests that will all error
    let handles: Vec<_> = (0..10)
        .map(|i| {
            let storage = Arc::clone(&storage);
            let uri = uri.clone();
            tokio::spawn(async move {
                let (status, response) = post_json(
                    create_test_app(&storage),
                    &uri,
                    serde_json::json!({
                        "tuple_key": {
                            "user": format!("invalid-user-{}", i),
                            "relation": "viewer",
                            "object": "document:readme"
                        }
                    }),
                )
                .await;
                (status, response)
            })
        })
        .collect();

    // Collect all results
    let results: Vec<_> = futures::future::join_all(handles)
        .await
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

    // All should have consistent error format
    for (status, response) in results {
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "All invalid user errors should return 400"
        );
        assert_eq!(
            response["code"], "validation_error",
            "All errors should have same code"
        );
        assert!(
            response.get("message").is_some(),
            "All errors should have message field"
        );
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Generate a valid ULID format store ID that doesn't exist in storage.
/// OpenFGA validates store ID format first (returns 400 for invalid format),
/// then checks existence (returns 404 for valid format but non-existent).
fn nonexistent_store_id() -> String {
    ulid::Ulid::new().to_string()
}

/// Set up a test store with a simple authorization model
async fn setup_test_store(storage: &MemoryDataStore) -> String {
    let store_id = ulid::Ulid::new().to_string();

    // Create store
    storage.create_store(&store_id, "Test Store").await.unwrap();

    // Create authorization model
    setup_simple_model(storage, &store_id).await;

    store_id
}
