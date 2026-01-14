// Allow dead_code because each test file is compiled as a separate crate,
// so not all helper functions are used in every test file.
#![allow(dead_code)]

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;
use url::Url;
use uuid::Uuid;

// ============================================================================
// Test Timing Configuration
// ============================================================================

/// Default time to wait for changelog entries to settle after write operations.
const DEFAULT_CHANGELOG_SETTLE_MS: u64 = 100;

/// Default shorter settle time for within-batch delays.
const DEFAULT_CHANGELOG_SETTLE_SHORT_MS: u64 = 50;

/// Get the changelog settle time, configurable via environment variable.
///
/// OpenFGA's changelog (ReadChanges API) may have slight propagation delays
/// depending on the storage backend. This delay ensures changelog entries
/// are visible when testing sequential write-then-read patterns.
///
/// Override with `CHANGELOG_SETTLE_TIME_MS` environment variable for CI environments
/// that may need different timing (e.g., slower storage backends).
///
/// # Example
/// ```bash
/// CHANGELOG_SETTLE_TIME_MS=200 cargo test
/// ```
pub fn changelog_settle_time() -> Duration {
    std::env::var("CHANGELOG_SETTLE_TIME_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_millis(DEFAULT_CHANGELOG_SETTLE_MS))
}

/// Get the short changelog settle time, configurable via environment variable.
///
/// Use this when testing within a single batch of writes where
/// full propagation isn't critical.
///
/// Override with `CHANGELOG_SETTLE_TIME_SHORT_MS` environment variable.
pub fn changelog_settle_time_short() -> Duration {
    std::env::var("CHANGELOG_SETTLE_TIME_SHORT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_millis(DEFAULT_CHANGELOG_SETTLE_SHORT_MS))
}

// Keep the constants for backwards compatibility but mark as deprecated
#[deprecated(
    since = "0.1.0",
    note = "Use changelog_settle_time() for CI configurability"
)]
pub const CHANGELOG_SETTLE_TIME: Duration = Duration::from_millis(100);

#[deprecated(
    since = "0.1.0",
    note = "Use changelog_settle_time_short() for CI configurability"
)]
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

/// Helper function to get OpenFGA gRPC URL.
///
/// Returns the host:port string (without scheme) for use with grpcurl.
///
/// The gRPC URL can be configured via:
/// - `OPENFGA_GRPC_URL` environment variable (host:port format, e.g., "localhost:18081")
/// - Derived from `OPENFGA_URL` by adding 1 to the HTTP port
///
/// # Examples
///
/// ```ignore
/// // With OPENFGA_URL=http://localhost:18080 -> returns "localhost:18081"
/// // With OPENFGA_GRPC_URL=myhost:9090 -> returns "myhost:9090"
/// ```
pub fn get_grpc_url() -> String {
    // First check if gRPC URL is explicitly set
    if let Ok(grpc_url) = std::env::var("OPENFGA_GRPC_URL") {
        return grpc_url;
    }

    // Otherwise derive from HTTP URL
    let http_url = get_openfga_url();

    // Parse the URL properly
    match Url::parse(&http_url) {
        Ok(parsed) => {
            let host = parsed.host_str().unwrap_or("localhost");
            // Use port_or_known_default() to handle default ports (80 for http, 443 for https)
            // If the scheme is unknown, fall back to OpenFGA's default port 18080
            let http_port = parsed.port_or_known_default().unwrap_or(18080);
            let grpc_port = http_port + 1;
            format!("{}:{}", host, grpc_port)
        }
        Err(_) => {
            // Fallback: try to extract host:port manually for malformed URLs
            let without_scheme = http_url
                .strip_prefix("http://")
                .or_else(|| http_url.strip_prefix("https://"))
                .unwrap_or(&http_url);

            // Remove any path component
            let host_port = without_scheme.split('/').next().unwrap_or(without_scheme);

            // Try to increment the port if present
            if let Some((host, port_str)) = host_port.rsplit_once(':') {
                if let Ok(port) = port_str.parse::<u16>() {
                    return format!("{}:{}", host, port + 1);
                }
            }

            // Default fallback
            format!(
                "{}:18081",
                host_port.split(':').next().unwrap_or("localhost")
            )
        }
    }
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

/// Check if grpcurl is available on the system.
///
/// grpcurl is required for gRPC-specific tests (Sections 21, 23, 34).
/// Install via:
/// - macOS: `brew install grpcurl`
/// - Linux: Download from https://github.com/fullstorydev/grpcurl/releases
/// - Go: `go install github.com/fullstorydev/grpcurl/cmd/grpcurl@latest`
pub fn is_grpcurl_available() -> bool {
    Command::new("grpcurl")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Execute gRPC call with grpcurl and return parsed JSON response
pub fn grpc_call(method: &str, data: &serde_json::Value) -> Result<serde_json::Value> {
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

    // Fail explicitly if we had partial success - some lines parsed, others didn't.
    // This indicates corrupted or malformed streaming output that should not be silently ignored.
    if parse_errors > 0 && !results.is_empty() {
        anyhow::bail!(
            "Partial NDJSON parse failure: {} lines parsed successfully, {} failed. This may indicate truncated or corrupted streaming output. Preview:\n{}",
            results.len(),
            parse_errors,
            &trimmed[..trimmed.len().min(500)]
        );
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

/// Execute gRPC call with grpcurl and capture full output including errors.
///
/// Unlike `grpc_call` which only returns success responses, this function
/// returns a tuple of (success, stdout, stderr) to allow inspection of
/// error responses.
///
/// # Returns
///
/// * `(true, stdout, stderr)` - Command succeeded (exit code 0)
/// * `(false, stdout, stderr)` - Command failed (non-zero exit code)
pub fn grpc_call_with_error(
    method: &str,
    data: &serde_json::Value,
) -> Result<(bool, String, String)> {
    let url = get_grpc_url();
    let data_str = serde_json::to_string(data)?;

    let output = Command::new("grpcurl")
        .args(["-plaintext", "-d", &data_str, &url, method])
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    Ok((output.status.success(), stdout, stderr))
}

/// List available gRPC services on the server.
///
/// Uses grpcurl's reflection to discover available services.
///
/// # Returns
///
/// A list of fully qualified service names (e.g., "openfga.v1.OpenFGAService").
pub fn grpcurl_list() -> Result<Vec<String>> {
    let url = get_grpc_url();
    let output = Command::new("grpcurl")
        .args(["-plaintext", &url, "list"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("grpcurl list failed: {}", stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let services: Vec<String> = stdout.lines().map(|s| s.to_string()).collect();
    Ok(services)
}

/// Describe a gRPC service or method.
///
/// Uses grpcurl's reflection to get the description of a service or method.
///
/// # Arguments
///
/// * `target` - Service name (e.g., "openfga.v1.OpenFGAService") or
///   method name (e.g., "openfga.v1.OpenFGAService.Check")
///
/// # Returns
///
/// The protobuf description of the service or method.
pub fn grpcurl_describe(target: &str) -> Result<String> {
    let url = get_grpc_url();
    let output = Command::new("grpcurl")
        .args(["-plaintext", &url, "describe", target])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("grpcurl describe failed: {}", stderr);
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
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

// ============================================================================
// Unit Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that get_grpc_url derives port correctly from default HTTP URL
    #[test]
    fn test_get_grpc_url_derives_port_from_http() {
        // Clear any existing env vars for predictable test
        std::env::remove_var("OPENFGA_GRPC_URL");
        std::env::remove_var("OPENFGA_URL");

        // Default should be localhost:18081 (18080 + 1)
        let result = get_grpc_url();
        assert_eq!(result, "localhost:18081");
    }

    /// Test that OPENFGA_GRPC_URL takes precedence
    #[test]
    fn test_get_grpc_url_respects_grpc_url_env_var() {
        // Set up
        std::env::set_var("OPENFGA_GRPC_URL", "custom-host:9999");
        std::env::remove_var("OPENFGA_URL");

        let result = get_grpc_url();

        // Clean up
        std::env::remove_var("OPENFGA_GRPC_URL");

        assert_eq!(result, "custom-host:9999");
    }

    /// Test port derivation with custom OPENFGA_URL
    #[test]
    fn test_get_grpc_url_derives_from_custom_http_url() {
        // Set up
        std::env::remove_var("OPENFGA_GRPC_URL");
        std::env::set_var("OPENFGA_URL", "http://myserver:8080");

        let result = get_grpc_url();

        // Clean up
        std::env::remove_var("OPENFGA_URL");

        assert_eq!(result, "myserver:8081");
    }

    /// Test with HTTPS URL
    #[test]
    fn test_get_grpc_url_handles_https() {
        // Set up
        std::env::remove_var("OPENFGA_GRPC_URL");
        std::env::set_var("OPENFGA_URL", "https://secure.example.com:443");

        let result = get_grpc_url();

        // Clean up
        std::env::remove_var("OPENFGA_URL");

        assert_eq!(result, "secure.example.com:444");
    }

    /// Test URL without explicit port (uses scheme's default port)
    #[test]
    fn test_get_grpc_url_handles_url_without_port() {
        // Set up
        std::env::remove_var("OPENFGA_GRPC_URL");
        std::env::set_var("OPENFGA_URL", "http://example.com");

        let result = get_grpc_url();

        // Clean up
        std::env::remove_var("OPENFGA_URL");

        // When no port specified, url crate uses scheme's default (80 for http)
        // So gRPC port is 81
        assert_eq!(result, "example.com:81");
    }

    /// Test URL with path component (should be stripped)
    #[test]
    fn test_get_grpc_url_strips_path() {
        // Set up
        std::env::remove_var("OPENFGA_GRPC_URL");
        std::env::set_var("OPENFGA_URL", "http://localhost:8080/v1/api");

        let result = get_grpc_url();

        // Clean up
        std::env::remove_var("OPENFGA_URL");

        assert_eq!(result, "localhost:8081");
    }
}
