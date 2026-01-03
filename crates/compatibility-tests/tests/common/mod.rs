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

/// Private helper to create any authorization model
async fn create_authorization_model(
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
