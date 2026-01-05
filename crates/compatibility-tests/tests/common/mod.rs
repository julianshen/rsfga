// Allow dead_code because each test file is compiled as a separate crate,
// so not all helper functions are used in every test file.
#![allow(dead_code)]

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

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
    let client = reqwest::Client::new();
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
    let client = reqwest::Client::new();

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
    let client = reqwest::Client::new();

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
                        "type_name": "string"
                    }
                }
            }
        }
    });

    create_authorization_model(store_id, model).await
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

/// Helper function to create an HTTP client
pub fn http_client() -> reqwest::Client {
    reqwest::Client::new()
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
        // Check if it's a gRPC error (which might still have JSON in stdout)
        if !stdout.trim().is_empty() {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stdout) {
                return Ok(json);
            }
        }
        anyhow::bail!("grpcurl failed: {} {}", stderr, stdout);
    }

    if stdout.trim().is_empty() {
        return Ok(json!({}));
    }

    let json: serde_json::Value = serde_json::from_str(&stdout)?;
    Ok(json)
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
