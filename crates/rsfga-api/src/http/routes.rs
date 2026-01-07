//! HTTP route definitions and handlers.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tracing::error;

use rsfga_storage::{DataStore, StorageError};

use super::state::AppState;

/// Creates the HTTP router with all OpenFGA-compatible endpoints.
pub fn create_router<S: DataStore>(state: AppState<S>) -> Router {
    Router::new()
        // Store management
        .route("/stores", post(create_store::<S>))
        .route("/stores", get(list_stores::<S>))
        .route("/stores/:store_id", get(get_store::<S>))
        .route("/stores/:store_id", delete(delete_store::<S>))
        // Authorization operations
        .route("/stores/:store_id/check", post(check::<S>))
        .route("/stores/:store_id/batch-check", post(batch_check::<S>))
        .route("/stores/:store_id/expand", post(expand::<S>))
        .route("/stores/:store_id/write", post(write_tuples::<S>))
        .route("/stores/:store_id/read", post(read_tuples::<S>))
        .route("/stores/:store_id/list-objects", post(list_objects::<S>))
        // Health check
        .route("/health", get(health_check))
        .with_state(Arc::new(state))
}

// ============================================================
// Error Handling
// ============================================================

/// API error response format matching OpenFGA.
#[derive(Debug, Serialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

impl ApiError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new("not_found", message)
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new("validation_error", message)
    }

    pub fn internal_error(message: impl Into<String>) -> Self {
        Self::new("internal_error", message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let status = match self.code.as_str() {
            "not_found" => StatusCode::NOT_FOUND,
            "validation_error" => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(self)).into_response()
    }
}

impl From<StorageError> for ApiError {
    fn from(err: StorageError) -> Self {
        match &err {
            StorageError::StoreNotFound { store_id } => {
                ApiError::not_found(format!("store not found: {}", store_id))
            }
            StorageError::InvalidInput { message } => ApiError::invalid_input(message),
            _ => {
                error!("Storage error: {}", err);
                ApiError::internal_error(err.to_string())
            }
        }
    }
}

type ApiResult<T> = Result<T, ApiError>;

// ============================================================
// Health Check
// ============================================================

async fn health_check() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

// ============================================================
// Store Management
// ============================================================

/// Request body for creating a store.
#[derive(Debug, Deserialize)]
pub struct CreateStoreRequest {
    pub name: String,
}

/// Response for store operations.
#[derive(Debug, Serialize)]
pub struct StoreResponse {
    pub id: String,
    pub name: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

async fn create_store<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Json(body): Json<CreateStoreRequest>,
) -> ApiResult<impl IntoResponse> {
    let id = uuid::Uuid::new_v4().to_string();
    let store = state.storage.create_store(&id, &body.name).await?;

    Ok((
        StatusCode::CREATED,
        Json(StoreResponse {
            id: store.id,
            name: store.name,
            created_at: Some(store.created_at.to_rfc3339()),
            updated_at: Some(store.updated_at.to_rfc3339()),
        }),
    ))
}

async fn get_store<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(store_id): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let store = state.storage.get_store(&store_id).await?;

    Ok(Json(StoreResponse {
        id: store.id,
        name: store.name,
        created_at: Some(store.created_at.to_rfc3339()),
        updated_at: Some(store.updated_at.to_rfc3339()),
    }))
}

async fn list_stores<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
) -> ApiResult<impl IntoResponse> {
    let stores = state.storage.list_stores().await?;

    let response: Vec<StoreResponse> = stores
        .into_iter()
        .map(|s| StoreResponse {
            id: s.id,
            name: s.name,
            created_at: Some(s.created_at.to_rfc3339()),
            updated_at: Some(s.updated_at.to_rfc3339()),
        })
        .collect();

    Ok(Json(serde_json::json!({ "stores": response })))
}

async fn delete_store<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(store_id): Path<String>,
) -> ApiResult<impl IntoResponse> {
    state.storage.delete_store(&store_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ============================================================
// Check Operation
// ============================================================

/// Request body for check operation.
// Fields will be used when full resolver is integrated.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct CheckRequestBody {
    pub tuple_key: TupleKeyBody,
    #[serde(default)]
    pub authorization_model_id: Option<String>,
    #[serde(default)]
    pub contextual_tuples: Option<ContextualTuplesBody>,
}

#[derive(Debug, Deserialize)]
pub struct TupleKeyBody {
    pub user: String,
    pub relation: String,
    pub object: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ContextualTuplesBody {
    pub tuple_keys: Vec<TupleKeyBody>,
}

/// Response for check operation.
#[derive(Debug, Serialize)]
pub struct CheckResponseBody {
    pub allowed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
}

async fn check<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(store_id): Path<String>,
    Json(body): Json<CheckRequestBody>,
) -> ApiResult<impl IntoResponse> {
    // Validate store exists
    let _ = state.storage.get_store(&store_id).await?;

    // For now, we perform a simple tuple lookup.
    // Full resolver integration will be added later.
    let filter = rsfga_storage::TupleFilter {
        object_type: body
            .tuple_key
            .object
            .split(':')
            .next()
            .map(|s| s.to_string()),
        object_id: body
            .tuple_key
            .object
            .split(':')
            .nth(1)
            .map(|s| s.to_string()),
        relation: Some(body.tuple_key.relation.clone()),
        user: Some(body.tuple_key.user.clone()),
    };

    let tuples = state.storage.read_tuples(&store_id, &filter).await?;

    Ok(Json(CheckResponseBody {
        allowed: !tuples.is_empty(),
        resolution: None,
    }))
}

// ============================================================
// Batch Check Operation
// ============================================================

/// Request body for batch check operation.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct BatchCheckRequestBody {
    pub checks: Vec<BatchCheckItemBody>,
    #[serde(default)]
    pub authorization_model_id: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct BatchCheckItemBody {
    pub tuple_key: TupleKeyBody,
    pub correlation_id: String,
    #[serde(default)]
    pub contextual_tuples: Option<ContextualTuplesBody>,
}

/// Response for batch check operation.
#[derive(Debug, Serialize)]
pub struct BatchCheckResponseBody {
    pub result: std::collections::HashMap<String, BatchCheckSingleResultBody>,
}

#[derive(Debug, Serialize)]
pub struct BatchCheckSingleResultBody {
    pub allowed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<BatchCheckErrorBody>,
}

#[derive(Debug, Serialize)]
pub struct BatchCheckErrorBody {
    pub code: i32,
    pub message: String,
}

async fn batch_check<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(store_id): Path<String>,
    Json(body): Json<BatchCheckRequestBody>,
) -> ApiResult<impl IntoResponse> {
    use rsfga_server::handlers::batch::MAX_BATCH_SIZE;

    // Validate store exists
    let _ = state.storage.get_store(&store_id).await?;

    // Validate batch size
    if body.checks.is_empty() {
        return Err(ApiError::invalid_input("batch request cannot be empty"));
    }
    if body.checks.len() > MAX_BATCH_SIZE {
        return Err(ApiError::invalid_input(format!(
            "batch size {} exceeds maximum allowed {}",
            body.checks.len(),
            MAX_BATCH_SIZE
        )));
    }

    // Process each check
    let mut result_map = std::collections::HashMap::new();
    for item in body.checks {
        let filter = rsfga_storage::TupleFilter {
            object_type: item
                .tuple_key
                .object
                .split(':')
                .next()
                .map(|s| s.to_string()),
            object_id: item
                .tuple_key
                .object
                .split(':')
                .nth(1)
                .map(|s| s.to_string()),
            relation: Some(item.tuple_key.relation.clone()),
            user: Some(item.tuple_key.user.clone()),
        };

        let tuples = state.storage.read_tuples(&store_id, &filter).await?;

        result_map.insert(
            item.correlation_id,
            BatchCheckSingleResultBody {
                allowed: !tuples.is_empty(),
                error: None,
            },
        );
    }

    Ok(Json(BatchCheckResponseBody { result: result_map }))
}

// ============================================================
// Expand Operation
// ============================================================

/// Request body for expand operation.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ExpandRequestBody {
    pub tuple_key: TupleKeyBody,
    #[serde(default)]
    pub authorization_model_id: Option<String>,
}

/// Response for expand operation (stub).
#[derive(Debug, Serialize)]
pub struct ExpandResponseBody {
    pub tree: Option<serde_json::Value>,
}

async fn expand<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(store_id): Path<String>,
    Json(_body): Json<ExpandRequestBody>,
) -> ApiResult<impl IntoResponse> {
    // Validate store exists
    let _ = state.storage.get_store(&store_id).await?;

    // Expand is not yet implemented - return empty tree
    Ok(Json(ExpandResponseBody { tree: None }))
}

// ============================================================
// Write Operation
// ============================================================

/// Request body for write operation.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct WriteRequestBody {
    #[serde(default)]
    pub writes: Option<WriteTuplesBody>,
    #[serde(default)]
    pub deletes: Option<DeleteTuplesBody>,
    #[serde(default)]
    pub authorization_model_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WriteTuplesBody {
    pub tuple_keys: Vec<TupleKeyBody>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteTuplesBody {
    pub tuple_keys: Vec<TupleKeyWithoutConditionBody>,
}

#[derive(Debug, Deserialize)]
pub struct TupleKeyWithoutConditionBody {
    pub user: String,
    pub relation: String,
    pub object: String,
}

async fn write_tuples<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(store_id): Path<String>,
    Json(body): Json<WriteRequestBody>,
) -> ApiResult<impl IntoResponse> {
    use rsfga_storage::StoredTuple;

    // Validate store exists
    let _ = state.storage.get_store(&store_id).await?;

    // Convert write tuples
    let writes: Vec<StoredTuple> = body
        .writes
        .map(|w| {
            w.tuple_keys
                .into_iter()
                .filter_map(|tk| parse_tuple_key(&tk))
                .collect()
        })
        .unwrap_or_default();

    // Convert delete tuples
    let deletes: Vec<StoredTuple> = body
        .deletes
        .map(|d| {
            d.tuple_keys
                .into_iter()
                .filter_map(|tk| {
                    parse_tuple_key(&TupleKeyBody {
                        user: tk.user,
                        relation: tk.relation,
                        object: tk.object,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    state
        .storage
        .write_tuples(&store_id, writes, deletes)
        .await?;

    Ok(Json(serde_json::json!({})))
}

/// Parses a tuple key into a StoredTuple.
fn parse_tuple_key(tk: &TupleKeyBody) -> Option<rsfga_storage::StoredTuple> {
    // Parse user: "user:alice" or "team:eng#member"
    let (user_type, user_id, user_relation) = parse_user(&tk.user)?;

    // Parse object: "document:readme"
    let (object_type, object_id) = tk.object.split_once(':')?;

    Some(rsfga_storage::StoredTuple {
        object_type: object_type.to_string(),
        object_id: object_id.to_string(),
        relation: tk.relation.clone(),
        user_type: user_type.to_string(),
        user_id: user_id.to_string(),
        user_relation: user_relation.map(|s| s.to_string()),
    })
}

/// Parses a user string into (type, id, optional_relation).
fn parse_user(user: &str) -> Option<(&str, &str, Option<&str>)> {
    // Check for userset format: "team:eng#member"
    if let Some((type_id, relation)) = user.split_once('#') {
        let (user_type, user_id) = type_id.split_once(':')?;
        Some((user_type, user_id, Some(relation)))
    } else {
        // Simple format: "user:alice"
        let (user_type, user_id) = user.split_once(':')?;
        Some((user_type, user_id, None))
    }
}

// ============================================================
// Read Operation
// ============================================================

/// Request body for read operation.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ReadRequestBody {
    #[serde(default)]
    pub tuple_key: Option<TupleKeyWithoutConditionBody>,
    #[serde(default)]
    pub page_size: Option<i32>,
    #[serde(default)]
    pub continuation_token: Option<String>,
}

/// Response for read operation.
#[derive(Debug, Serialize)]
pub struct ReadResponseBody {
    pub tuples: Vec<TupleResponseBody>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuation_token: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TupleResponseBody {
    pub key: TupleKeyResponseBody,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TupleKeyResponseBody {
    pub user: String,
    pub relation: String,
    pub object: String,
}

async fn read_tuples<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(store_id): Path<String>,
    Json(body): Json<ReadRequestBody>,
) -> ApiResult<impl IntoResponse> {
    use rsfga_storage::TupleFilter;

    // Validate store exists
    let _ = state.storage.get_store(&store_id).await?;

    // Build filter from request
    let filter = if let Some(tk) = body.tuple_key {
        let object_filter = if !tk.object.is_empty() {
            tk.object
                .split_once(':')
                .map(|(t, i)| (t.to_string(), i.to_string()))
        } else {
            None
        };

        TupleFilter {
            object_type: object_filter.as_ref().map(|(t, _)| t.clone()),
            object_id: object_filter.map(|(_, i)| i),
            relation: if !tk.relation.is_empty() {
                Some(tk.relation)
            } else {
                None
            },
            user: if !tk.user.is_empty() {
                Some(tk.user)
            } else {
                None
            },
        }
    } else {
        TupleFilter::default()
    };

    let tuples = state.storage.read_tuples(&store_id, &filter).await?;

    // Convert to response format
    let response_tuples: Vec<TupleResponseBody> = tuples
        .into_iter()
        .map(|t| TupleResponseBody {
            key: TupleKeyResponseBody {
                user: format_user(&t.user_type, &t.user_id, t.user_relation.as_deref()),
                relation: t.relation,
                object: format!("{}:{}", t.object_type, t.object_id),
            },
            timestamp: None,
        })
        .collect();

    Ok(Json(ReadResponseBody {
        tuples: response_tuples,
        continuation_token: None,
    }))
}

/// Formats a user for the response.
fn format_user(user_type: &str, user_id: &str, user_relation: Option<&str>) -> String {
    if let Some(rel) = user_relation {
        format!("{}:{}#{}", user_type, user_id, rel)
    } else {
        format!("{}:{}", user_type, user_id)
    }
}

// ============================================================
// List Objects Operation
// ============================================================

/// Request body for list objects operation.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ListObjectsRequestBody {
    pub user: String,
    pub relation: String,
    pub r#type: String,
    #[serde(default)]
    pub authorization_model_id: Option<String>,
    #[serde(default)]
    pub contextual_tuples: Option<ContextualTuplesBody>,
}

/// Response for list objects operation (stub).
#[derive(Debug, Serialize)]
pub struct ListObjectsResponseBody {
    pub objects: Vec<String>,
}

async fn list_objects<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(store_id): Path<String>,
    Json(_body): Json<ListObjectsRequestBody>,
) -> ApiResult<impl IntoResponse> {
    // Validate store exists
    let _ = state.storage.get_store(&store_id).await?;

    // ListObjects is not yet implemented - return empty list
    Ok(Json(ListObjectsResponseBody { objects: vec![] }))
}
