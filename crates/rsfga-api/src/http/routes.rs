//! HTTP route definitions and handlers.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tower_http::limit::RequestBodyLimitLayer;
use tracing::error;

use rsfga_storage::{DataStore, PaginationOptions, StorageError, StoredAuthorizationModel};

use super::state::AppState;
use crate::observability::{metrics_handler, MetricsState};
use crate::utils::{format_user, parse_object, parse_user, MAX_BATCH_SIZE};

/// Default request body size limit (1MB).
/// This prevents memory exhaustion from oversized payloads.
pub const DEFAULT_BODY_LIMIT: usize = 1024 * 1024;

/// Private helper for common API routes.
///
/// This consolidates all OpenFGA-compatible routes in one place to avoid duplication.
fn api_routes<S: DataStore>() -> Router<Arc<AppState<S>>> {
    Router::new()
        // Store management
        .route("/stores", post(create_store::<S>).get(list_stores::<S>))
        .route(
            "/stores/:store_id",
            get(get_store::<S>)
                .put(update_store::<S>)
                .delete(delete_store::<S>),
        )
        // Authorization model management
        .route(
            "/stores/:store_id/authorization-models",
            post(write_authorization_model::<S>).get(list_authorization_models::<S>),
        )
        .route(
            "/stores/:store_id/authorization-models/:authorization_model_id",
            get(get_authorization_model::<S>),
        )
        // Authorization operations
        .route("/stores/:store_id/check", post(check::<S>))
        .route("/stores/:store_id/batch-check", post(batch_check::<S>))
        .route("/stores/:store_id/expand", post(expand::<S>))
        .route("/stores/:store_id/write", post(write_tuples::<S>))
        .route("/stores/:store_id/read", post(read_tuples::<S>))
        .route("/stores/:store_id/list-objects", post(list_objects::<S>))
}

/// Creates the HTTP router with all OpenFGA-compatible endpoints.
///
/// Applies the default body size limit (1MB) to protect against oversized payloads.
pub fn create_router<S: DataStore>(state: AppState<S>) -> Router {
    create_router_with_body_limit(state, DEFAULT_BODY_LIMIT)
}

/// Creates the HTTP router with a custom body size limit.
///
/// # Arguments
///
/// * `state` - Application state with storage backend
/// * `body_limit` - Maximum request body size in bytes
pub fn create_router_with_body_limit<S: DataStore>(
    state: AppState<S>,
    body_limit: usize,
) -> Router {
    let shared_state = Arc::new(state);
    api_routes::<S>()
        // Health and readiness checks
        .route("/health", get(health_check))
        .route("/ready", get(readiness_check::<S>))
        .with_state(shared_state)
        // Apply body size limit layer
        .layer(RequestBodyLimitLayer::new(body_limit))
}

/// Creates the HTTP router with observability endpoints.
///
/// This includes all OpenFGA-compatible endpoints plus:
/// - `/metrics` - Prometheus metrics endpoint
/// - `/health` - Basic health check
/// - `/ready` - Readiness check (validates dependencies)
///
/// Applies the default body size limit (1MB) to protect against oversized payloads.
///
/// # Arguments
///
/// * `state` - Application state with storage backend
/// * `metrics_state` - Metrics state for Prometheus endpoint
pub fn create_router_with_observability<S: DataStore>(
    state: AppState<S>,
    metrics_state: MetricsState,
) -> Router {
    create_router_with_observability_and_limit(state, metrics_state, DEFAULT_BODY_LIMIT)
}

/// Creates the HTTP router with observability endpoints and custom body size limit.
///
/// # Arguments
///
/// * `state` - Application state with storage backend
/// * `metrics_state` - Metrics state for Prometheus endpoint
/// * `body_limit` - Maximum request body size in bytes
pub fn create_router_with_observability_and_limit<S: DataStore>(
    state: AppState<S>,
    metrics_state: MetricsState,
    body_limit: usize,
) -> Router {
    let shared_state = Arc::new(state);

    // Create the API router with readiness check
    let api_router = api_routes::<S>()
        .route("/ready", get(readiness_check::<S>))
        .with_state(shared_state)
        // Apply body size limit layer to API routes only
        .layer(RequestBodyLimitLayer::new(body_limit));

    // Create observability router (metrics, health) - no body limit needed
    let observability_router = Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/health", get(health_check))
        .with_state(metrics_state);

    // Merge routers
    api_router.merge(observability_router)
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
// Health and Readiness Checks
// ============================================================

/// Basic health check - returns 200 if the server is running.
///
/// This is a liveness probe that indicates the server process is alive.
/// It does NOT check dependencies.
async fn health_check() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

/// Readiness check - validates that all dependencies are accessible.
///
/// This is a readiness probe that checks:
/// - Storage backend connectivity (by attempting to list stores)
///
/// Returns 200 if ready, 503 if dependencies are unavailable.
///
/// Note: Error details are logged but not exposed in the response
/// to avoid leaking internal implementation details.
async fn readiness_check<S: DataStore>(State(state): State<Arc<AppState<S>>>) -> impl IntoResponse {
    // Check storage connectivity by attempting to list stores
    match state.storage.list_stores().await {
        Ok(_) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "status": "ready",
                "checks": {
                    "storage": "ok"
                }
            })),
        ),
        Err(e) => {
            // Log the full error for debugging, but don't expose it
            error!("Readiness check failed: storage unavailable: {}", e);
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "status": "not_ready",
                    "checks": {
                        "storage": "unavailable"
                    }
                })),
            )
        }
    }
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

impl From<rsfga_storage::Store> for StoreResponse {
    fn from(store: rsfga_storage::Store) -> Self {
        Self {
            id: store.id,
            name: store.name,
            created_at: Some(store.created_at.to_rfc3339()),
            updated_at: Some(store.updated_at.to_rfc3339()),
        }
    }
}

async fn create_store<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Json(body): Json<CreateStoreRequest>,
) -> ApiResult<impl IntoResponse> {
    let id = ulid::Ulid::new().to_string();
    let store = state.storage.create_store(&id, &body.name).await?;

    Ok((StatusCode::CREATED, Json(StoreResponse::from(store))))
}

async fn get_store<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(store_id): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let store = state.storage.get_store(&store_id).await?;
    Ok(Json(StoreResponse::from(store)))
}

async fn list_stores<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
) -> ApiResult<impl IntoResponse> {
    let stores = state.storage.list_stores().await?;
    let response: Vec<StoreResponse> = stores.into_iter().map(StoreResponse::from).collect();
    Ok(Json(serde_json::json!({ "stores": response })))
}

/// Request body for updating a store.
#[derive(Debug, Deserialize)]
pub struct UpdateStoreRequest {
    pub name: String,
}

async fn update_store<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(store_id): Path<String>,
    Json(body): Json<UpdateStoreRequest>,
) -> ApiResult<impl IntoResponse> {
    let store = state.storage.update_store(&store_id, &body.name).await?;
    Ok(Json(StoreResponse::from(store)))
}

async fn delete_store<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(store_id): Path<String>,
) -> ApiResult<impl IntoResponse> {
    state.storage.delete_store(&store_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ============================================================
// Authorization Model Management
// ============================================================

/// Request body for writing an authorization model.
/// Matches OpenFGA's WriteAuthorizationModel request format.
#[derive(Debug, Deserialize)]
pub struct WriteAuthorizationModelRequest {
    /// Schema version (e.g., "1.1").
    #[serde(default = "default_schema_version")]
    pub schema_version: String,
    /// Type definitions for the model.
    pub type_definitions: Vec<serde_json::Value>,
    /// Optional conditions for the model.
    #[serde(default)]
    pub conditions: Option<serde_json::Value>,
}

fn default_schema_version() -> String {
    "1.1".to_string()
}

/// Response for write authorization model.
#[derive(Debug, Serialize)]
pub struct WriteAuthorizationModelResponse {
    pub authorization_model_id: String,
}

/// Response for a single authorization model.
#[derive(Debug, Serialize)]
pub struct AuthorizationModelResponse {
    pub id: String,
    pub schema_version: String,
    pub type_definitions: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conditions: Option<serde_json::Value>,
}

impl TryFrom<StoredAuthorizationModel> for AuthorizationModelResponse {
    type Error = ApiError;

    fn try_from(model: StoredAuthorizationModel) -> Result<Self, Self::Error> {
        // Parse the stored JSON back into structured data
        let parsed: serde_json::Value = serde_json::from_str(&model.model_json).map_err(|e| {
            error!("Failed to parse stored model JSON: {}", e);
            ApiError::internal_error("Failed to parse authorization model")
        })?;

        let type_definitions = parsed
            .get("type_definitions")
            .and_then(|v| v.as_array())
            .cloned()
            .ok_or_else(|| {
                error!("Stored model missing type_definitions: {}", model.id);
                ApiError::internal_error("Stored authorization model is invalid")
            })?;

        // Filter out null conditions (treat JSON null as absent)
        let conditions = parsed.get("conditions").cloned().filter(|v| !v.is_null());

        Ok(Self {
            id: model.id,
            schema_version: model.schema_version,
            type_definitions,
            conditions,
        })
    }
}

/// Response for listing authorization models.
#[derive(Debug, Serialize)]
pub struct ListAuthorizationModelsResponse {
    pub authorization_models: Vec<AuthorizationModelResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuation_token: Option<String>,
}

/// Query parameters for listing authorization models.
#[derive(Debug, Deserialize)]
pub struct ListAuthorizationModelsQuery {
    #[serde(default)]
    pub page_size: Option<u32>,
    #[serde(default)]
    pub continuation_token: Option<String>,
}

async fn write_authorization_model<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(store_id): Path<String>,
    Json(body): Json<WriteAuthorizationModelRequest>,
) -> ApiResult<impl IntoResponse> {
    // Validate type_definitions is not empty (OpenFGA requirement)
    if body.type_definitions.is_empty() {
        return Err(ApiError::invalid_input("type_definitions cannot be empty"));
    }

    // Generate a new ULID for the model
    let model_id = ulid::Ulid::new().to_string();

    // Serialize the model data to JSON for storage (omit conditions if absent/null)
    let mut model_json = serde_json::json!({
        "type_definitions": body.type_definitions,
    });
    // Only include conditions if present and not null (OpenFGA compatibility)
    if let Some(ref conditions) = body.conditions {
        if !conditions.is_null() {
            model_json["conditions"] = conditions.clone();
        }
    }

    let model = StoredAuthorizationModel::new(
        &model_id,
        &store_id,
        &body.schema_version,
        model_json.to_string(),
    );

    state.storage.write_authorization_model(model).await?;

    Ok((
        StatusCode::CREATED,
        Json(WriteAuthorizationModelResponse {
            authorization_model_id: model_id,
        }),
    ))
}

/// Path parameters for authorization model routes.
#[derive(Debug, Deserialize)]
pub struct AuthorizationModelPath {
    pub store_id: String,
    pub authorization_model_id: String,
}

async fn get_authorization_model<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(path): Path<AuthorizationModelPath>,
) -> ApiResult<impl IntoResponse> {
    let model = state
        .storage
        .get_authorization_model(&path.store_id, &path.authorization_model_id)
        .await
        .map_err(|e| match e {
            StorageError::ModelNotFound { model_id } => {
                ApiError::not_found(format!("authorization model not found: {}", model_id))
            }
            other => ApiError::from(other),
        })?;

    let response = AuthorizationModelResponse::try_from(model)?;
    Ok(Json(serde_json::json!({
        "authorization_model": response
    })))
}

/// Default and maximum page size for listing authorization models (OpenFGA limit).
const DEFAULT_AUTHORIZATION_MODELS_PAGE_SIZE: u32 = 50;

async fn list_authorization_models<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(store_id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<ListAuthorizationModelsQuery>,
) -> ApiResult<impl IntoResponse> {
    // Use OpenFGA default (50) when not specified, clamp to max 50 when provided
    let page_size = Some(
        query
            .page_size
            .unwrap_or(DEFAULT_AUTHORIZATION_MODELS_PAGE_SIZE)
            .min(DEFAULT_AUTHORIZATION_MODELS_PAGE_SIZE),
    );

    let pagination = PaginationOptions {
        page_size,
        continuation_token: query.continuation_token,
    };

    let result = state
        .storage
        .list_authorization_models_paginated(&store_id, &pagination)
        .await?;

    let models: Result<Vec<AuthorizationModelResponse>, ApiError> = result
        .items
        .into_iter()
        .map(AuthorizationModelResponse::try_from)
        .collect();

    Ok(Json(ListAuthorizationModelsResponse {
        authorization_models: models?,
        continuation_token: result.continuation_token,
    }))
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
    #[serde(default, deserialize_with = "deserialize_null_as_empty_vec")]
    pub tuple_keys: Vec<TupleKeyBody>,
}

fn deserialize_null_as_empty_vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    let opt = Option::<Vec<T>>::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
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

    // Validate object format (required: "type:id")
    let (object_type, object_id) = parse_object(&body.tuple_key.object).ok_or_else(|| {
        ApiError::invalid_input(format!(
            "invalid object format '{}': expected 'type:id'",
            body.tuple_key.object
        ))
    })?;

    // For now, we perform a simple tuple lookup.
    // Full resolver integration will be added later.
    let filter = rsfga_storage::TupleFilter {
        object_type: Some(object_type.to_string()),
        object_id: Some(object_id.to_string()),
        relation: Some(body.tuple_key.relation.clone()),
        user: Some(body.tuple_key.user.clone()),
        condition_name: None,
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

// NOTE: This implementation performs sequential tuple lookups for simplicity.
// In Milestone 1.8 (Server Integration), this will delegate to BatchCheckHandler
// in rsfga-server to leverage parallel execution, request deduplication, and
// singleflight optimizations. See plan.md for the integration roadmap.

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

    // Process each check independently - errors are captured per-item, not propagated
    // This matches OpenFGA behavior where each check in a batch is independent
    let mut result_map = std::collections::HashMap::new();
    for item in body.checks.into_iter() {
        let result = process_single_check(state.storage.as_ref(), &store_id, &item).await;
        result_map.insert(item.correlation_id, result);
    }

    Ok(Json(BatchCheckResponseBody { result: result_map }))
}

/// Process a single check item, returning a result with either allowed status or error.
async fn process_single_check<S: DataStore>(
    storage: &S,
    store_id: &str,
    item: &BatchCheckItemBody,
) -> BatchCheckSingleResultBody {
    // Validate object format (required: "type:id")
    let (object_type, object_id) = match parse_object(&item.tuple_key.object) {
        Some((t, i)) => (t, i),
        None => {
            return BatchCheckSingleResultBody {
                allowed: false,
                error: Some(BatchCheckErrorBody {
                    code: 400,
                    message: format!(
                        "invalid object format '{}': expected 'type:id'",
                        item.tuple_key.object
                    ),
                }),
            };
        }
    };

    // Validate user format
    if parse_user(&item.tuple_key.user).is_none() {
        return BatchCheckSingleResultBody {
            allowed: false,
            error: Some(BatchCheckErrorBody {
                code: 400,
                message: format!(
                    "invalid user format '{}': expected 'type:id' or 'type:id#relation'",
                    item.tuple_key.user
                ),
            }),
        };
    }

    let filter = rsfga_storage::TupleFilter {
        object_type: Some(object_type.to_string()),
        object_id: Some(object_id.to_string()),
        relation: Some(item.tuple_key.relation.clone()),
        user: Some(item.tuple_key.user.clone()),
        condition_name: None,
    };

    match storage.read_tuples(store_id, &filter).await {
        Ok(tuples) => BatchCheckSingleResultBody {
            allowed: !tuples.is_empty(),
            error: None,
        },
        Err(e) => BatchCheckSingleResultBody {
            allowed: false,
            error: Some(BatchCheckErrorBody {
                code: 500,
                message: format!("storage error: {}", e),
            }),
        },
    }
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

    // Convert write tuples - fail if any tuple key is invalid
    let writes: Vec<StoredTuple> = body
        .writes
        .map(|w| {
            w.tuple_keys
                .into_iter()
                .enumerate()
                .map(|(i, tk)| {
                    parse_tuple_key(&tk).ok_or_else(|| {
                        ApiError::invalid_input(format!(
                            "invalid tuple_key at writes index {}: user='{}', object='{}'",
                            i, tk.user, tk.object
                        ))
                    })
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();

    // Convert delete tuples - fail if any tuple key is invalid
    let deletes: Vec<StoredTuple> = body
        .deletes
        .map(|d| {
            d.tuple_keys
                .into_iter()
                .enumerate()
                .map(|(i, tk)| {
                    // Use parse_tuple_fields directly to avoid cloning
                    parse_tuple_fields(&tk.user, &tk.relation, &tk.object).ok_or_else(|| {
                        ApiError::invalid_input(format!(
                            "invalid tuple_key at deletes index {}: user='{}', object='{}'",
                            i, tk.user, tk.object
                        ))
                    })
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();

    state
        .storage
        .write_tuples(&store_id, writes, deletes)
        .await?;

    Ok(Json(serde_json::json!({})))
}

/// Parses a tuple key into a StoredTuple.
fn parse_tuple_key(tk: &TupleKeyBody) -> Option<rsfga_storage::StoredTuple> {
    parse_tuple_fields(&tk.user, &tk.relation, &tk.object)
}

/// Parses tuple fields directly into a StoredTuple.
///
/// This avoids unnecessary cloning when converting from TupleKeyWithoutConditionBody.
/// Uses `parse_user` and `parse_object` for consistent validation across all handlers.
fn parse_tuple_fields(
    user: &str,
    relation: &str,
    object: &str,
) -> Option<rsfga_storage::StoredTuple> {
    // Parse user: "user:alice" or "team:eng#member"
    let (user_type, user_id, user_relation) = parse_user(user)?;

    // Parse object: "document:readme" - use parse_object for consistent validation
    let (object_type, object_id) = parse_object(object)?;

    Some(rsfga_storage::StoredTuple {
        object_type: object_type.to_string(),
        object_id: object_id.to_string(),
        relation: relation.to_string(),
        user_type: user_type.to_string(),
        user_id: user_id.to_string(),
        user_relation: user_relation.map(|s| s.to_string()),
        // TODO(#84): Parse condition from request when API support is added
        condition_name: None,
        condition_context: None,
    })
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

    // Build filter from request - use parse_object for consistent validation
    // Invalid object format is treated as "no object filter" rather than an error
    // since read is a query operation, not a write
    let filter = if let Some(tk) = body.tuple_key {
        let object_filter = if !tk.object.is_empty() {
            // Use parse_object for consistent validation (rejects ":", ":id", "type:")
            parse_object(&tk.object).map(|(t, i)| (t.to_string(), i.to_string()))
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
            condition_name: None,
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
