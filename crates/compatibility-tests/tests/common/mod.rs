// Allow dead_code because each test file is compiled as a separate crate,
// so not all helper functions are used in every test file.
#![allow(dead_code)]

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::OnceLock;
use std::time::Duration;
use uuid::Uuid;

// ============================================================================
// Test Timing Constants
// ============================================================================

/// Time to wait for changelog entries to settle after write operations.
///
/// OpenFGA's changelog (ReadChanges API) may have slight propagation delays
/// depending on the storage backend. This delay ensures changelog entries
/// are visible when testing sequential write-then-read patterns.
///
/// Note: This is primarily needed for tests that verify changelog ordering
/// or pagination across rapid sequential writes.
pub const CHANGELOG_SETTLE_TIME: Duration = Duration::from_millis(100);

/// Shorter settle time for tests that need minimal delay.
///
/// Use this when testing within a single batch of writes where
/// full propagation isn't critical.
pub const CHANGELOG_SETTLE_TIME_SHORT: Duration = Duration::from_millis(50);

// ============================================================================
// Shared HTTP Client (Issue #26)
// ============================================================================

/// Global shared reqwest::Client for connection pool reuse.
/// Creating a new Client for each request is inefficient as reqwest::Client
/// holds a connection pool internally and is meant to be created once and reused.
static SHARED_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// Get a reference to the shared HTTP client.
/// This lazily initializes the client on first use and reuses it for all subsequent calls.
pub fn shared_client() -> &'static reqwest::Client {
    SHARED_CLIENT.get_or_init(reqwest::Client::new)
}

// ============================================================================
// Typed API Response Structs
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct CreateStoreResponse {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateModelResponse {
    pub authorization_model_id: String,
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct TupleKey {
    pub user: String,
    pub relation: String,
    pub object: String,
}

#[derive(Debug, Deserialize)]
pub struct Tuple {
    pub key: TupleKey,
    pub timestamp: String,
}

#[derive(Debug, Deserialize)]
pub struct ReadResponse {
    pub tuples: Vec<Tuple>,
    #[serde(default)]
    pub continuation_token: Option<String>,
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Helper function to get OpenFGA URL
pub fn get_openfga_url() -> String {
    std::env::var("OPENFGA_URL").unwrap_or_else(|_| "http://localhost:18080".to_string())
}

/// Helper function to get OpenFGA gRPC URL (port 18081)
pub fn get_grpc_url() -> String {
    let http_url = get_openfga_url();
    http_url
        .replace(":18080", ":18081")
        .replace("http://", "")
        .replace("https://", "")
}

/// Helper function to create a test store
pub async fn create_test_store() -> Result<String> {
    let client = shared_client();
    let store_name = format!("test-store-{}", Uuid::new_v4());

    let response = client
        .post(format!("{}/stores", get_openfga_url()))
        .json(&json!({ "name": store_name }))
        .send()
        .await?;

    if !response.status().is_success() {
        anyhow::bail!("Failed to create store: {}", response.status());
    }

    let store: CreateStoreResponse = response.json().await?;
    Ok(store.id)
}

/// Public helper to create any authorization model
/// This allows tests to create custom models without duplicating HTTP client code
pub async fn create_authorization_model(
    store_id: &str,
    model: serde_json::Value,
) -> Result<String> {
    let client = shared_client();

    let response = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model)
        .send()
        .await?;

    if !response.status().is_success() {
        anyhow::bail!(
            "Failed to create authorization model: {}",
            response.status()
        );
    }

    let response_body: CreateModelResponse = response.json().await?;
    Ok(response_body.authorization_model_id)
}

/// Helper function to create a simple authorization model
pub async fn create_test_model(store_id: &str) -> Result<String> {
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

    create_authorization_model(store_id, model).await
}

/// Helper function to write tuples
pub async fn write_tuples(store_id: &str, tuples: Vec<(&str, &str, &str)>) -> Result<()> {
    let client = shared_client();

    let tuple_keys: Vec<serde_json::Value> = tuples
        .into_iter()
        .map(|(user, relation, object)| {
            json!({
                "user": user,
                "relation": relation,
                "object": object
            })
        })
        .collect();

    let write_request = json!({
        "writes": {
            "tuple_keys": tuple_keys
        }
    });

    let response = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&write_request)
        .send()
        .await?;

    if !response.status().is_success() {
        anyhow::bail!("Failed to write tuples: {}", response.status());
    }

    Ok(())
}

/// Helper function to create authorization model with wildcard support
pub async fn create_wildcard_model(store_id: &str) -> Result<String> {
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
                                {
                                    "type": "user"
                                },
                                {
                                    "type": "user",
                                    "wildcard": {}
                                }
                            ]
                        }
                    }
                }
            }
        ]
    });

    create_authorization_model(store_id, model).await
}

/// Helper function to create model with userset relations
pub async fn create_userset_model(store_id: &str) -> Result<String> {
    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            {
                "type": "user"
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
                            "directly_related_user_types": [
                                {"type": "user"},
                                {"type": "folder", "relation": "viewer"}
                            ]
                        }
                    }
                }
            }
        ]
    });

    create_authorization_model(store_id, model).await
}

/// Helper function to create model with conditional support
pub async fn create_conditional_model(store_id: &str) -> Result<String> {
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
                                {
                                    "type": "user",
                                    "condition": "ip_address_match"
                                }
                            ]
                        }
                    }
                }
            }
        ],
        "conditions": {
            "ip_address_match": {
                "name": "ip_address_match",
                "expression": "request.ip_address == '192.168.1.1'",
                "parameters": {
                    "ip_address": {
                        "type_name": "TYPE_NAME_STRING"
                    }
                }
            }
        }
    });

    create_authorization_model(store_id, model).await
}

// ============================================================================
// Common Model Helpers
// ============================================================================

/// Create a simple document-viewer model JSON (for consistency tests and basic scenarios).
/// This is the minimal model with a user type and a document type with a viewer relation.
pub fn simple_document_viewer_model() -> serde_json::Value {
    json!({
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
    })
}

// ============================================================================
// CEL Condition Helpers (Milestone 0.8)
// ============================================================================

/// Helper function to create model with time-based condition
pub async fn create_time_condition_model(store_id: &str) -> Result<String> {
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
                                {
                                    "type": "user",
                                    "condition": "time_access"
                                }
                            ]
                        }
                    }
                }
            }
        ],
        "conditions": {
            "time_access": {
                "name": "time_access",
                "expression": "request.current_time < request.expires_at",
                "parameters": {
                    "current_time": {
                        "type_name": "TYPE_NAME_TIMESTAMP"
                    },
                    "expires_at": {
                        "type_name": "TYPE_NAME_TIMESTAMP"
                    }
                }
            }
        }
    });

    create_authorization_model(store_id, model).await
}

/// Helper function to write a tuple with a condition
pub async fn write_tuple_with_condition(
    store_id: &str,
    user: &str,
    relation: &str,
    object: &str,
    condition_name: &str,
    condition_context: Option<serde_json::Value>,
) -> Result<()> {
    let client = shared_client();

    let mut tuple = json!({
        "user": user,
        "relation": relation,
        "object": object,
        "condition": {
            "name": condition_name
        }
    });

    if let Some(ctx) = condition_context {
        tuple["condition"]["context"] = ctx;
    }

    let write_request = json!({
        "writes": {
            "tuple_keys": [tuple]
        }
    });

    let response = client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&write_request)
        .send()
        .await?;

    if !response.status().is_success() {
        anyhow::bail!(
            "Failed to write tuple with condition: {}",
            response.status()
        );
    }

    Ok(())
}

/// Helper function to check a permission with context for condition evaluation
pub async fn check_permission_with_context(
    client: &reqwest::Client,
    store_id: &str,
    user: &str,
    relation: &str,
    object: &str,
    context: serde_json::Value,
) -> Result<bool> {
    let check_request = json!({
        "tuple_key": {
            "user": user,
            "relation": relation,
            "object": object
        },
        "context": context
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    if !response.status().is_success() {
        anyhow::bail!(
            "Check with context failed with status: {}",
            response.status()
        );
    }

    let body: serde_json::Value = response.json().await?;
    body.get("allowed")
        .and_then(|v| v.as_bool())
        .ok_or_else(|| anyhow::anyhow!("Response missing or invalid 'allowed' field"))
}

/// Helper function to write sequential test tuples for pagination testing
pub async fn write_sequential_tuples(store_id: &str, count: usize) -> Result<()> {
    let tuples: Vec<_> = (0..count)
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

    write_tuples(store_id, tuple_refs).await
}

// ============================================================================
// HTTP Client Helper
// ============================================================================

/// Helper function to get an HTTP client.
/// Returns a clone of the shared client (cheap due to internal Arc).
/// Prefer using `shared_client()` directly when possible.
#[deprecated(
    since = "0.1.0",
    note = "Use shared_client() instead for better efficiency"
)]
pub fn http_client() -> reqwest::Client {
    shared_client().clone()
}

// ============================================================================
// gRPC Helpers
// ============================================================================

/// Execute gRPC call with grpcurl and return parsed JSON response
pub fn grpc_call(method: &str, data: &serde_json::Value) -> Result<serde_json::Value> {
    use std::process::Command;

    let url = get_grpc_url();
    let data_str = serde_json::to_string(data)?;

    let output = Command::new("grpcurl")
        .args(["-plaintext", "-d", &data_str, &url, method])
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        // Always fail when grpcurl exits with non-zero status.
        // Tests that need to inspect error responses should use grpc_call_with_error.
        anyhow::bail!("grpcurl failed: {} {}", stderr, stdout);
    }

    if stdout.trim().is_empty() {
        return Ok(json!({}));
    }

    let json: serde_json::Value = serde_json::from_str(&stdout)?;
    Ok(json)
}

/// Parse grpcurl streaming response output.
///
/// grpcurl outputs streaming responses in various formats:
/// - Newline-delimited JSON (NDJSON): one JSON object per line
/// - JSON array: all objects in a single array
/// - Single JSON object: when only one response is streamed
///
/// This function handles all these formats gracefully.
pub fn parse_grpc_streaming_response(output: &str) -> Result<Vec<serde_json::Value>> {
    let trimmed = output.trim();

    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    // Try parsing as newline-delimited JSON (NDJSON) first
    let mut results: Vec<serde_json::Value> = Vec::new();
    let mut parse_errors = 0;

    for line in trimmed.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<serde_json::Value>(line) {
            Ok(value) => results.push(value),
            Err(_) => parse_errors += 1,
        }
    }

    // If we had parse errors but no results, try alternative formats
    if results.is_empty() && parse_errors > 0 {
        // Try as JSON array
        if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(trimmed) {
            return Ok(arr);
        }
        // Try as single JSON object
        if let Ok(obj) = serde_json::from_str::<serde_json::Value>(trimmed) {
            return Ok(vec![obj]);
        }
        // Return error instead of silently returning empty Vec
        anyhow::bail!(
            "Failed to parse grpcurl streaming output ({} lines, {} parse errors). Output preview:\n{}",
            trimmed.lines().count(),
            parse_errors,
            &trimmed[..trimmed.len().min(500)]
        );
    }

    Ok(results)
}

/// Execute gRPC streaming call and return parsed response messages.
///
/// Unlike `grpc_call` which expects a single response, this handles
/// streaming RPCs that return multiple messages (e.g., StreamedListObjects).
pub fn grpc_streaming_call(
    method: &str,
    data: &serde_json::Value,
) -> Result<Vec<serde_json::Value>> {
    use std::process::Command;

    let url = get_grpc_url();
    let data_str = serde_json::to_string(data)?;

    let output = Command::new("grpcurl")
        .args(["-plaintext", "-d", &data_str, &url, method])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("grpcurl streaming call failed: {}", stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_grpc_streaming_response(&stdout)
}

/// Helper to create a test store via gRPC
pub fn create_test_store_grpc(name: &str) -> Result<String> {
    let response = grpc_call(
        "openfga.v1.OpenFGAService/CreateStore",
        &json!({"name": name}),
    )?;
    let store_id = response["id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("CreateStore response missing 'id' field"))?;
    Ok(store_id.to_string())
}

// ============================================================================
// Check API Helpers
// ============================================================================

/// Build a check request JSON payload
pub fn build_check_request(user: &str, relation: &str, object: &str) -> serde_json::Value {
    json!({
        "tuple_key": {
            "user": user,
            "relation": relation,
            "object": object
        }
    })
}

/// Helper function to check a permission and return the result
/// Accepts a reference to reqwest::Client for connection reuse
pub async fn check_permission(
    client: &reqwest::Client,
    store_id: &str,
    user: &str,
    relation: &str,
    object: &str,
) -> Result<bool> {
    let check_request = build_check_request(user, relation, object);

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    if !response.status().is_success() {
        anyhow::bail!("Check failed with status: {}", response.status());
    }

    let body: serde_json::Value = response.json().await?;
    body.get("allowed")
        .and_then(|v| v.as_bool())
        .ok_or_else(|| anyhow::anyhow!("Response missing or invalid 'allowed' field"))
}

// ============================================================================
// Read API Helpers
// ============================================================================

/// Helper function to read tuples with optional filter
/// Accepts a reference to reqwest::Client for connection reuse
pub async fn read_tuples_filtered(
    client: &reqwest::Client,
    store_id: &str,
    user: Option<&str>,
    relation: Option<&str>,
    object: Option<&str>,
) -> Result<Vec<Tuple>> {
    let mut tuple_key = serde_json::Map::new();
    if let Some(u) = user {
        tuple_key.insert("user".to_string(), json!(u));
    }
    if let Some(r) = relation {
        tuple_key.insert("relation".to_string(), json!(r));
    }
    if let Some(o) = object {
        tuple_key.insert("object".to_string(), json!(o));
    }

    let read_request = json!({
        "tuple_key": tuple_key
    });

    let response = client
        .post(format!("{}/stores/{}/read", get_openfga_url(), store_id))
        .json(&read_request)
        .send()
        .await?;

    if !response.status().is_success() {
        anyhow::bail!("Read tuples failed with status: {}", response.status());
    }

    let read_response: ReadResponse = response.json().await?;
    Ok(read_response.tuples)
}
