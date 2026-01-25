//! HTTP route definitions and handlers.

use std::sync::Arc;

use axum::{
    async_trait,
    extract::{FromRequest, Path, Request, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tower_http::limit::RequestBodyLimitLayer;
use tracing::error;

use rsfga_domain::cel::global_cache;
use rsfga_domain::error::DomainError;
use rsfga_domain::resolver::{CheckRequest as DomainCheckRequest, ContextualTuple};
use rsfga_storage::{DataStore, PaginationOptions, StorageError, StoredAuthorizationModel, Utc};

use super::state::AppState;
use crate::observability::{metrics_handler, MetricsState};
use crate::utils::{format_user, parse_object, parse_user};

/// Custom JSON extractor that returns 400 Bad Request instead of 422 Unprocessable Entity
/// for deserialization errors (OpenFGA compatibility).
///
/// Preserves 413 Payload Too Large for body limit errors.
pub struct JsonBadRequest<T>(pub T);

#[async_trait]
impl<S, T> FromRequest<S> for JsonBadRequest<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = (StatusCode, Json<ApiError>);

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(value)) => Ok(JsonBadRequest(value)),
            Err(rejection) => {
                use axum::extract::rejection::JsonRejection;

                // Preserve 413 Payload Too Large for body limit errors
                let status = match &rejection {
                    JsonRejection::BytesRejection(_) => {
                        // BytesRejection wraps body limit errors - check if it's a 413
                        let inner_status = rejection.status();
                        if inner_status == StatusCode::PAYLOAD_TOO_LARGE {
                            StatusCode::PAYLOAD_TOO_LARGE
                        } else {
                            StatusCode::BAD_REQUEST
                        }
                    }
                    _ => StatusCode::BAD_REQUEST,
                };

                let message = rejection.body_text();
                let error = if status == StatusCode::PAYLOAD_TOO_LARGE {
                    ApiError::new("payload_too_large", message)
                } else {
                    ApiError::validation_error(message)
                };

                Err((status, Json(error)))
            }
        }
    }
}

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
            get(get_authorization_model::<S>).delete(delete_authorization_model::<S>),
        )
        // Authorization operations
        .route("/stores/:store_id/check", post(check::<S>))
        .route("/stores/:store_id/batch-check", post(batch_check::<S>))
        .route("/stores/:store_id/expand", post(expand::<S>))
        .route("/stores/:store_id/write", post(write_tuples::<S>))
        .route("/stores/:store_id/read", post(read_tuples::<S>))
        .route("/stores/:store_id/list-objects", post(list_objects::<S>))
        .route("/stores/:store_id/list-users", post(list_users::<S>))
        .route("/stores/:store_id/changes", get(read_changes::<S>))
        .route(
            "/stores/:store_id/assertions/:authorization_model_id",
            axum::routing::put(write_assertions::<S>).get(read_assertions::<S>),
        )
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

/// OpenFGA-compatible error codes.
///
/// These error codes match the OpenFGA protobuf specification and ensure full API
/// compatibility. Each code maps to a specific HTTP status code via [`ApiError::into_response`].
///
/// # Error Code Categories
///
/// ## 404 Not Found
/// - [`STORE_ID_NOT_FOUND`] - Store with given ID does not exist
/// - [`AUTHORIZATION_MODEL_NOT_FOUND`] - Authorization model with given ID not found
/// - [`LATEST_AUTHORIZATION_MODEL_NOT_FOUND`] - No authorization models exist in store
/// - [`ASSERTION_NOT_FOUND`] - Assertions for given model not found
///
/// ## 400 Bad Request
/// - [`VALIDATION_ERROR`] - Generic input validation failure (format, missing fields)
/// - [`TYPE_NOT_FOUND`] - Type not defined in authorization model
/// - [`RELATION_NOT_FOUND`] - Relation not defined on type in authorization model
/// - [`TYPE_DEFINITIONS_TOO_FEW_ITEMS`] - type_definitions array is empty
/// - [`INVALID_WRITE_INPUT`] - Invalid tuple write request
/// - [`CANNOT_ALLOW_DUPLICATE_TUPLES_IN_ONE_REQUEST`] - Duplicate tuples in batch write
/// - [`CANNOT_ALLOW_DUPLICATE_TYPES_IN_ONE_REQUEST`] - Duplicate types in model definition
/// - [`INVALID_CONTINUATION_TOKEN`] - Invalid pagination token
/// - [`AUTHORIZATION_MODEL_RESOLUTION_TOO_COMPLEX`] - Resolution exceeded depth/complexity limits
///
/// ## 409 Conflict
/// - [`WRITE_FAILED_DUE_TO_INVALID_INPUT`] - Write conflict (tuple exists, condition mismatch)
///
/// ## 5xx Server Errors
/// - [`INTERNAL_ERROR`] - Unexpected internal error
/// - [`TIMEOUT`] - Operation timed out
/// - [`SERVICE_UNAVAILABLE`] - Service temporarily unavailable
/// - [`RESOURCE_EXHAUSTED`] - Resource limit reached (rate limiting)
/// - [`PAYLOAD_TOO_LARGE`] - Request body exceeds size limit
///
/// # Usage
///
/// Use the corresponding [`ApiError`] constructor methods rather than these constants directly:
///
/// ```ignore
/// // Preferred: Use ApiError constructors
/// ApiError::store_not_found("store not found")
/// ApiError::type_not_found("type 'foo' not found in authorization model")
///
/// // Avoid: Direct constant usage (for internal use only)
/// ApiError::new(error_codes::STORE_ID_NOT_FOUND, "message")
/// ```
///
/// # Compatibility
///
/// These codes are validated against OpenFGA's behavior in Phase 0 compatibility tests
/// (see `crates/compatibility-tests/tests/test_section_17_error_format.rs`).
pub mod error_codes {
    // 404 Not Found codes
    /// Store with the specified ID does not exist.
    pub const STORE_ID_NOT_FOUND: &str = "store_id_not_found";
    /// Authorization model with the specified ID not found in store.
    pub const AUTHORIZATION_MODEL_NOT_FOUND: &str = "authorization_model_not_found";
    /// No authorization models exist in the store.
    pub const LATEST_AUTHORIZATION_MODEL_NOT_FOUND: &str = "latest_authorization_model_not_found";
    /// Assertions for the specified authorization model not found.
    pub const ASSERTION_NOT_FOUND: &str = "assertion_not_found";

    // 400 Bad Request codes
    /// Generic input validation error (invalid format, missing required fields).
    pub const VALIDATION_ERROR: &str = "validation_error";
    /// Invalid write request format or content.
    pub const INVALID_WRITE_INPUT: &str = "invalid_write_input";
    /// type_definitions array must contain at least one type definition.
    pub const TYPE_DEFINITIONS_TOO_FEW_ITEMS: &str = "type_definitions_too_few_items";
    /// Cannot include duplicate tuples in a single write request.
    pub const CANNOT_ALLOW_DUPLICATE_TUPLES_IN_ONE_REQUEST: &str =
        "cannot_allow_duplicate_tuples_in_one_request";
    /// Cannot include duplicate type names in authorization model.
    pub const CANNOT_ALLOW_DUPLICATE_TYPES_IN_ONE_REQUEST: &str =
        "cannot_allow_duplicate_types_in_one_request";
    /// Pagination continuation token is invalid or expired.
    pub const INVALID_CONTINUATION_TOKEN: &str = "invalid_continuation_token";
    /// Authorization model resolution exceeded complexity limits (depth, cycles).
    pub const AUTHORIZATION_MODEL_RESOLUTION_TOO_COMPLEX: &str =
        "authorization_model_resolution_too_complex";
    /// Type not defined in the authorization model.
    pub const TYPE_NOT_FOUND: &str = "type_not_found";
    /// Relation not defined on type in the authorization model.
    pub const RELATION_NOT_FOUND: &str = "relation_not_found";

    // 409 Conflict codes
    /// Write failed due to conflict (tuple already exists or condition mismatch).
    pub const WRITE_FAILED_DUE_TO_INVALID_INPUT: &str = "write_failed_due_to_invalid_input";

    // 5xx codes
    /// Unexpected internal server error.
    pub const INTERNAL_ERROR: &str = "internal_error";
    /// Operation timed out before completion.
    pub const TIMEOUT: &str = "timeout";
    /// Service temporarily unavailable (storage backend issues).
    pub const SERVICE_UNAVAILABLE: &str = "service_unavailable";
    /// Resource limit reached (e.g., rate limiting).
    pub const RESOURCE_EXHAUSTED: &str = "resource_exhausted";
    /// Request body exceeds maximum allowed size.
    pub const PAYLOAD_TOO_LARGE: &str = "payload_too_large";
}

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

    /// Creates a store not found error (404).
    pub fn store_not_found(message: impl Into<String>) -> Self {
        Self::new(error_codes::STORE_ID_NOT_FOUND, message)
    }

    /// Creates an authorization model not found error (404).
    pub fn authorization_model_not_found(message: impl Into<String>) -> Self {
        Self::new(error_codes::AUTHORIZATION_MODEL_NOT_FOUND, message)
    }

    /// Creates a latest authorization model not found error (404).
    pub fn latest_authorization_model_not_found(message: impl Into<String>) -> Self {
        Self::new(error_codes::LATEST_AUTHORIZATION_MODEL_NOT_FOUND, message)
    }

    /// Creates an assertion not found error (404).
    pub fn assertion_not_found(message: impl Into<String>) -> Self {
        Self::new(error_codes::ASSERTION_NOT_FOUND, message)
    }

    /// Creates a validation error (400).
    pub fn validation_error(message: impl Into<String>) -> Self {
        Self::new(error_codes::VALIDATION_ERROR, message)
    }

    /// Creates an invalid write input error (400).
    pub fn invalid_write_input(message: impl Into<String>) -> Self {
        Self::new(error_codes::INVALID_WRITE_INPUT, message)
    }

    /// Creates a type definitions too few items error (400).
    pub fn type_definitions_too_few_items(message: impl Into<String>) -> Self {
        Self::new(error_codes::TYPE_DEFINITIONS_TOO_FEW_ITEMS, message)
    }

    /// Creates a duplicate tuples error (400).
    pub fn duplicate_tuples(message: impl Into<String>) -> Self {
        Self::new(
            error_codes::CANNOT_ALLOW_DUPLICATE_TUPLES_IN_ONE_REQUEST,
            message,
        )
    }

    /// Creates a duplicate types error (400).
    pub fn duplicate_types(message: impl Into<String>) -> Self {
        Self::new(
            error_codes::CANNOT_ALLOW_DUPLICATE_TYPES_IN_ONE_REQUEST,
            message,
        )
    }

    /// Creates an authorization model resolution too complex error (400).
    pub fn resolution_too_complex(message: impl Into<String>) -> Self {
        Self::new(
            error_codes::AUTHORIZATION_MODEL_RESOLUTION_TOO_COMPLEX,
            message,
        )
    }

    /// Creates a type not found error (400).
    pub fn type_not_found(message: impl Into<String>) -> Self {
        Self::new(error_codes::TYPE_NOT_FOUND, message)
    }

    /// Creates a relation not found error (400).
    pub fn relation_not_found(message: impl Into<String>) -> Self {
        Self::new(error_codes::RELATION_NOT_FOUND, message)
    }

    /// Creates an invalid continuation token error (400).
    pub fn invalid_continuation_token(message: impl Into<String>) -> Self {
        Self::new(error_codes::INVALID_CONTINUATION_TOKEN, message)
    }

    /// Creates an internal error (500).
    pub fn internal_error(message: impl Into<String>) -> Self {
        Self::new(error_codes::INTERNAL_ERROR, message)
    }

    /// Creates a conflict error (409 Conflict).
    /// Used for duplicate resources or condition conflicts.
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(error_codes::WRITE_FAILED_DUE_TO_INVALID_INPUT, message)
    }

    /// Creates a timeout error (504 Gateway Timeout).
    /// Used when an operation exceeds its time limit.
    pub fn gateway_timeout(message: impl Into<String>) -> Self {
        Self::new(error_codes::TIMEOUT, message)
    }

    /// Creates a service unavailable error (503 Service Unavailable).
    /// Used when the service is temporarily unavailable.
    pub fn service_unavailable(message: impl Into<String>) -> Self {
        Self::new(error_codes::SERVICE_UNAVAILABLE, message)
    }

    /// Creates a resource exhausted error (429 Too Many Requests).
    /// Used when a resource limit has been reached.
    pub fn resource_exhausted(message: impl Into<String>) -> Self {
        Self::new(error_codes::RESOURCE_EXHAUSTED, message)
    }

    // Legacy methods for backward compatibility - deprecated in favor of specific methods
    // TODO: Remove in v2.0.0 - tracked in issue #270
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        use error_codes::*;

        let status = match self.code.as_str() {
            // 404 Not Found
            STORE_ID_NOT_FOUND
            | AUTHORIZATION_MODEL_NOT_FOUND
            | LATEST_AUTHORIZATION_MODEL_NOT_FOUND
            | ASSERTION_NOT_FOUND => StatusCode::NOT_FOUND,

            // 400 Bad Request
            VALIDATION_ERROR
            | INVALID_WRITE_INPUT
            | TYPE_DEFINITIONS_TOO_FEW_ITEMS
            | CANNOT_ALLOW_DUPLICATE_TUPLES_IN_ONE_REQUEST
            | CANNOT_ALLOW_DUPLICATE_TYPES_IN_ONE_REQUEST
            | INVALID_CONTINUATION_TOKEN
            | AUTHORIZATION_MODEL_RESOLUTION_TOO_COMPLEX
            | TYPE_NOT_FOUND
            | RELATION_NOT_FOUND => StatusCode::BAD_REQUEST,

            // 409 Conflict
            WRITE_FAILED_DUE_TO_INVALID_INPUT => StatusCode::CONFLICT,

            // 504 Gateway Timeout
            TIMEOUT => StatusCode::GATEWAY_TIMEOUT,

            // 413 Payload Too Large
            PAYLOAD_TOO_LARGE => StatusCode::PAYLOAD_TOO_LARGE,

            // 503 Service Unavailable
            SERVICE_UNAVAILABLE => StatusCode::SERVICE_UNAVAILABLE,

            // 429 Too Many Requests
            RESOURCE_EXHAUSTED => StatusCode::TOO_MANY_REQUESTS,

            // Default: 500 Internal Server Error
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(self)).into_response()
    }
}

impl From<StorageError> for ApiError {
    fn from(err: StorageError) -> Self {
        match &err {
            // 404 Not Found: store or model doesn't exist
            StorageError::StoreNotFound { .. } => ApiError::store_not_found("store not found"),
            StorageError::ModelNotFound { .. } => {
                ApiError::authorization_model_not_found("authorization model not found")
            }
            // 400 Bad Request: validation errors
            StorageError::InvalidInput { message } => {
                // Check for specific error types that have dedicated error codes
                if message.contains("continuation_token") {
                    ApiError::invalid_continuation_token(message)
                } else {
                    ApiError::validation_error(message)
                }
            }
            StorageError::InvalidFilter { message } => ApiError::validation_error(message),
            // 409 Conflict: duplicate tuple or condition conflict
            StorageError::DuplicateTuple { .. } => {
                ApiError::conflict("cannot write a tuple which already exists")
            }
            StorageError::ConditionConflict(_) => {
                ApiError::conflict("tuple exists with different condition")
            }
            // 503 Service Unavailable: connection errors, health check failures
            StorageError::ConnectionError { .. } | StorageError::HealthCheckFailed { .. } => {
                error!("Storage unavailable: {}", err);
                ApiError::service_unavailable("storage backend unavailable")
            }
            // 504 Gateway Timeout: query timeout
            StorageError::QueryTimeout { .. } => {
                error!("Query timeout: {}", err);
                ApiError::gateway_timeout("storage operation timed out")
            }
            _ => {
                error!("Storage error: {}", err);
                ApiError::internal_error(err.to_string())
            }
        }
    }
}

impl From<DomainError> for ApiError {
    fn from(err: DomainError) -> Self {
        // Map domain errors to specific OpenFGA error codes
        match &err {
            // Store not found errors
            DomainError::StoreNotFound { .. } => ApiError::store_not_found("store not found"),
            // Type not found in authorization model
            DomainError::TypeNotFound { .. } => {
                ApiError::type_not_found("type not found in authorization model")
            }
            // Relation not found on type
            DomainError::RelationNotFound { .. } => {
                ApiError::relation_not_found("relation not found on type")
            }
            // Depth limit exceeded - resolution too complex
            DomainError::DepthLimitExceeded { .. } => {
                ApiError::resolution_too_complex("authorization model resolution too complex")
            }
            // Cycle detected - resolution too complex
            DomainError::CycleDetected { .. } => {
                ApiError::resolution_too_complex("cycle detected in authorization model")
            }
            // Timeouts
            DomainError::Timeout { .. } | DomainError::OperationTimeout { .. } => {
                error!("Domain timeout: {}", err);
                ApiError::gateway_timeout("authorization check timeout")
            }
            // Invalid format errors - provide field-specific messages
            DomainError::InvalidUserFormat { value } => {
                ApiError::validation_error(format!("invalid user format: {}", value))
            }
            DomainError::InvalidObjectFormat { value } => {
                ApiError::validation_error(format!("invalid object format: {}", value))
            }
            DomainError::InvalidRelationFormat { value } => {
                ApiError::validation_error(format!("invalid relation format: {}", value))
            }
            // Structured resolver error variants (no string parsing required)
            DomainError::AuthorizationModelNotFound { .. } => {
                ApiError::latest_authorization_model_not_found("no authorization model found")
            }
            DomainError::MissingContextKey { .. } => {
                ApiError::validation_error("missing required context parameter")
            }
            DomainError::ConditionParseError { .. } => {
                ApiError::validation_error("invalid condition expression")
            }
            DomainError::ConditionEvalError { .. } => {
                ApiError::validation_error("condition evaluation failed")
            }
            DomainError::InvalidParameter { .. } => ApiError::validation_error(err.to_string()),
            DomainError::InvalidFilter { .. } => ApiError::validation_error(err.to_string()),
            DomainError::StorageOperationFailed { reason } => {
                error!("Storage operation failed: {}", reason);
                ApiError::internal_error("internal error during authorization check")
            }
            // Legacy resolver error - kept for backwards compatibility during transition
            // TODO: Remove in v1.0.0 when all usages are migrated to structured variants
            DomainError::ResolverError { message } => {
                error!("Legacy resolver error: {}", message);
                ApiError::internal_error("internal error during authorization check")
            }
            // Condition-related errors
            DomainError::ConditionNotFound { .. } => {
                ApiError::validation_error("condition not found in authorization model")
            }
            // Model parse/validation errors
            DomainError::ModelParseError { .. } => {
                ApiError::validation_error("failed to parse authorization model")
            }
            DomainError::ModelValidationError { .. } => {
                ApiError::validation_error("authorization model validation failed")
            }
        }
    }
}

/// Converts a BatchCheckError to an ApiError.
///
/// Logs internal errors with structured context to aid debugging while
/// returning sanitized error messages to clients.
fn batch_check_error_to_api_error(err: rsfga_server::handlers::batch::BatchCheckError) -> ApiError {
    use rsfga_server::handlers::batch::BatchCheckError;
    match err {
        BatchCheckError::EmptyBatch => ApiError::validation_error("batch request cannot be empty"),
        BatchCheckError::BatchTooLarge { size, max } => {
            ApiError::validation_error(format!("batch size {size} exceeds maximum allowed {max}"))
        }
        BatchCheckError::InvalidCheck { index, message } => {
            ApiError::validation_error(format!("invalid check at index {index}: {message}"))
        }
        BatchCheckError::DomainError(msg) => {
            // Log full error details for debugging - DO NOT expose to clients
            tracing::error!(error = %msg, "Domain error in HTTP batch check");
            // Return sanitized message to prevent information leakage
            ApiError::internal_error("internal error during authorization check")
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

/// Query parameters for listing stores.
///
/// # Validation Rules
///
/// - `page_size`: Optional. Defaults to 50. Must be positive (> 0).
///   Values exceeding 50 are clamped to the maximum (50).
/// - `continuation_token`: Optional. Base64-encoded token from a previous
///   response to fetch the next page of results.
#[derive(Debug, Deserialize)]
pub struct ListStoresQuery {
    #[serde(default)]
    pub page_size: Option<u32>,
    #[serde(default)]
    pub continuation_token: Option<String>,
}

/// Response for listing stores.
///
/// # Fields
///
/// - `stores`: Array of store objects in the current page.
/// - `continuation_token`: Present when more results are available.
///   Pass this token in the next request to fetch the next page.
///   Format: Base64-encoded pagination state.
#[derive(Debug, Serialize)]
pub struct ListStoresResponse {
    pub stores: Vec<StoreResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuation_token: Option<String>,
}

/// Default and maximum page size for listing stores (OpenFGA limit).
const DEFAULT_STORES_PAGE_SIZE: u32 = 50;

async fn create_store<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    JsonBadRequest(body): JsonBadRequest<CreateStoreRequest>,
) -> ApiResult<impl IntoResponse> {
    let id = ulid::Ulid::new().to_string();
    let store = state.storage.create_store(&id, &body.name).await?;

    Ok((StatusCode::CREATED, Json(StoreResponse::from(store))))
}

async fn get_store<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(store_id): Path<String>,
) -> ApiResult<impl IntoResponse> {
    // OpenFGA returns 404 for any non-existent store, regardless of ID format.
    // No format validation - just let storage return "not found" for invalid IDs.
    let store = state.storage.get_store(&store_id).await?;
    Ok(Json(StoreResponse::from(store)))
}

async fn list_stores<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    axum::extract::Query(query): axum::extract::Query<ListStoresQuery>,
) -> ApiResult<impl IntoResponse> {
    // Validate and clamp page_size
    let page_size = query.page_size.unwrap_or(DEFAULT_STORES_PAGE_SIZE);
    if page_size == 0 {
        return Err(ApiError::validation_error("page_size must be positive"));
    }
    let page_size = page_size.min(DEFAULT_STORES_PAGE_SIZE);

    let pagination = PaginationOptions {
        page_size: Some(page_size),
        continuation_token: query.continuation_token,
    };

    let result = state.storage.list_stores_paginated(&pagination).await?;
    let stores: Vec<StoreResponse> = result.items.into_iter().map(StoreResponse::from).collect();

    Ok(Json(ListStoresResponse {
        stores,
        continuation_token: result.continuation_token,
    }))
}

/// Request body for updating a store.
#[derive(Debug, Deserialize)]
pub struct UpdateStoreRequest {
    pub name: String,
}

async fn update_store<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(store_id): Path<String>,
    JsonBadRequest(body): JsonBadRequest<UpdateStoreRequest>,
) -> ApiResult<impl IntoResponse> {
    // OpenFGA returns 404 for any non-existent store, regardless of ID format.
    let store = state.storage.update_store(&store_id, &body.name).await?;
    Ok(Json(StoreResponse::from(store)))
}

/// Delete a store and all associated data (DELETE).
///
/// # Cleanup Behavior
///
/// When a store is deleted, this handler also cleans up all in-memory assertions
/// associated with any authorization model in that store. The cleanup is performed
/// atomically using `retain` to avoid race conditions.
///
/// # Cleanup Order
///
/// Assertions are cleaned up *before* storage deletion. This ensures that if
/// storage deletion fails, we haven't leaked assertion data. The reverse order
/// (storage first) could leave orphaned assertions if the request fails partway.
///
/// # Performance
///
/// The assertion cleanup iterates all entries in the assertions map (O(n) where
/// n is the total number of assertion entries across all stores/models). For
/// production deployments with many assertion entries, consider the performance
/// implications.
async fn delete_store<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(store_id): Path<String>,
) -> ApiResult<impl IntoResponse> {
    // OpenFGA returns 404 for any non-existent store, regardless of ID format.
    // Clean up assertions FIRST, before storage deletion.
    // This ensures we don't leak assertions if storage deletion fails.
    // Using retain for atomic cleanup - no race condition window.
    state.assertions.retain(|key, _| key.0 != store_id);

    // Now delete from storage
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

/// Maximum size for authorization model JSON (1MB, similar to OpenFGA's ~256KB but more lenient).
/// This is validated at the HTTP layer before storage to prevent oversized payloads.
const MAX_AUTHORIZATION_MODEL_SIZE: usize = 1024 * 1024; // 1MB

/// Maximum number of (store_id, model_id) assertion entries to prevent unbounded memory growth.
/// This limits the total number of unique store/model pairs that can have assertions.
/// Typical production usage: < 100 models with assertions.
const MAX_ASSERTION_ENTRIES: usize = 10_000;

/// Warning threshold for assertion entries (80% of max).
const ASSERTION_ENTRIES_WARNING_THRESHOLD: usize = MAX_ASSERTION_ENTRIES * 80 / 100;

async fn write_authorization_model<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(store_id): Path<String>,
    JsonBadRequest(body): JsonBadRequest<WriteAuthorizationModelRequest>,
) -> ApiResult<impl IntoResponse> {
    // OpenFGA returns 404 for any non-existent store, regardless of ID format.
    // Validate type_definitions is not empty (OpenFGA requirement)
    if body.type_definitions.is_empty() {
        return Err(ApiError::type_definitions_too_few_items(
            "type_definitions requires at least 1 item",
        ));
    }

    // Serialize the model data to JSON for validation and storage
    let mut model_json = serde_json::json!({
        "type_definitions": body.type_definitions,
    });
    // Only include conditions if present and not null (OpenFGA compatibility)
    if let Some(ref conditions) = body.conditions {
        if !conditions.is_null() {
            model_json["conditions"] = conditions.clone();
        }
    }

    // Validate model semantics (duplicates, undefined refs, CEL syntax, etc.)
    // This is critical for API compatibility - OpenFGA returns 400 for invalid models
    crate::adapters::validate_authorization_model_json(&model_json, &body.schema_version)
        .map_err(|e| ApiError::validation_error(e.to_string()))?;

    // Validate model size before storage
    let model_json_str = model_json.to_string();
    if model_json_str.len() > MAX_AUTHORIZATION_MODEL_SIZE {
        return Err(ApiError::validation_error(format!(
            "authorization model exceeds maximum size of {MAX_AUTHORIZATION_MODEL_SIZE} bytes"
        )));
    }

    // Generate a new ULID for the model
    let model_id = ulid::Ulid::new().to_string();

    let model =
        StoredAuthorizationModel::new(&model_id, &store_id, &body.schema_version, model_json_str);

    // CRITICAL: Invalidate caches BEFORE writing the model to prevent race conditions.
    // If we invalidate after writing, concurrent requests could:
    // 1. Read the new model from storage
    // 2. Use cached CEL expressions or check results from the old model
    // 3. Return incorrect authorization decisions (security vulnerability)
    //
    // By invalidating first, any concurrent request will re-evaluate with fresh data.
    global_cache().invalidate_all();
    state.cache.invalidate_store(&store_id).await;

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
    // OpenFGA returns 404 for any non-existent store, regardless of ID format.
    let model = state
        .storage
        .get_authorization_model(&path.store_id, &path.authorization_model_id)
        .await
        .map_err(|e| match e {
            StorageError::ModelNotFound { .. } => {
                ApiError::authorization_model_not_found("authorization model not found")
            }
            other => ApiError::from(other),
        })?;

    let response = AuthorizationModelResponse::try_from(model)?;
    Ok(Json(serde_json::json!({
        "authorization_model": response
    })))
}

/// Delete an authorization model (DELETE).
///
/// Deletes the specified authorization model from the store.
///
/// # Cleanup Behavior
///
/// When an authorization model is deleted, this handler also cleans up any
/// in-memory assertions associated with that specific model. The cleanup uses
/// DashMap's atomic `remove` operation (O(1)).
///
/// # Cleanup Order
///
/// Assertions are cleaned up *before* storage deletion. This ensures consistent
/// behavior with `delete_store` and prevents assertion leaks if storage deletion
/// fails.
async fn delete_authorization_model<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(path): Path<AuthorizationModelPath>,
) -> ApiResult<impl IntoResponse> {
    // OpenFGA returns 404 for any non-existent store/model, regardless of ID format.
    // Clean up assertions FIRST, before storage deletion.
    // Using atomic remove - O(1) operation on DashMap.
    let key = (path.store_id.clone(), path.authorization_model_id.clone());
    state.assertions.remove(&key);

    // Now delete from storage
    state
        .storage
        .delete_authorization_model(&path.store_id, &path.authorization_model_id)
        .await
        .map_err(|e| match e {
            StorageError::ModelNotFound { .. } => {
                ApiError::authorization_model_not_found("authorization model not found")
            }
            other => ApiError::from(other),
        })?;

    Ok(StatusCode::NO_CONTENT)
}

/// Default and maximum page size for listing authorization models (OpenFGA limit).
const DEFAULT_AUTHORIZATION_MODELS_PAGE_SIZE: u32 = 50;

async fn list_authorization_models<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(store_id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<ListAuthorizationModelsQuery>,
) -> ApiResult<impl IntoResponse> {
    // OpenFGA returns 404 for any non-existent store, regardless of ID format.
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
    /// CEL evaluation context for condition evaluation.
    /// Contains values that will be accessible as `request.<key>` in CEL expressions.
    #[serde(default)]
    pub context: Option<std::collections::HashMap<String, serde_json::Value>>,
}

/// Relationship condition for conditional tuples.
#[derive(Debug, Deserialize)]
pub struct RelationshipConditionBody {
    /// The name of the condition (must match a condition defined in the model).
    pub name: String,
    /// Optional context parameters for the condition.
    #[serde(default)]
    pub context: Option<std::collections::HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
pub struct TupleKeyBody {
    pub user: String,
    pub relation: String,
    pub object: String,
    /// Optional condition for conditional relationships.
    #[serde(default)]
    pub condition: Option<RelationshipConditionBody>,
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
    JsonBadRequest(body): JsonBadRequest<CheckRequestBody>,
) -> ApiResult<impl IntoResponse> {
    // OpenFGA returns 404 for any non-existent store, regardless of ID format.
    // If a specific authorization_model_id is provided, validate it exists
    if let Some(ref model_id) = body.authorization_model_id {
        state
            .storage
            .get_authorization_model(&store_id, model_id)
            .await
            .map_err(|e| match e {
                rsfga_storage::StorageError::ModelNotFound { .. } => {
                    ApiError::validation_error(format!("authorization model not found: {model_id}"))
                }
                other => ApiError::from(other),
            })?;
    }

    // Convert contextual tuples from HTTP format to domain format
    let contextual_tuples: Vec<ContextualTuple> = body
        .contextual_tuples
        .map(|ct| {
            ct.tuple_keys
                .into_iter()
                .map(|tk| {
                    if let Some(condition) = tk.condition {
                        ContextualTuple::with_condition(
                            &tk.user,
                            &tk.relation,
                            &tk.object,
                            &condition.name,
                            condition.context,
                        )
                    } else {
                        ContextualTuple::new(&tk.user, &tk.relation, &tk.object)
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    // Create domain check request with context and optional model ID
    let check_request = DomainCheckRequest::with_model_id(
        store_id,
        body.tuple_key.user,
        body.tuple_key.relation,
        body.tuple_key.object,
        contextual_tuples,
        body.context.unwrap_or_default(),
        body.authorization_model_id,
    );

    // Delegate to GraphResolver for full graph traversal
    let result = state.resolver.check(&check_request).await?;

    Ok(Json(CheckResponseBody {
        allowed: result.allowed,
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
    /// CEL evaluation context for condition evaluation.
    #[serde(default)]
    pub context: Option<std::collections::HashMap<String, serde_json::Value>>,
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
    JsonBadRequest(body): JsonBadRequest<BatchCheckRequestBody>,
) -> ApiResult<impl IntoResponse> {
    use rsfga_server::handlers::batch::{
        BatchCheckItem as ServerBatchCheckItem, BatchCheckRequest as ServerBatchCheckRequest,
    };

    // OpenFGA returns 404 for any non-existent store, regardless of ID format.
    // Validate store exists
    let _ = state.storage.get_store(&store_id).await?;

    // Store correlation_ids for mapping back to response
    // We need to maintain the order since BatchCheckHandler returns results in order
    let correlation_ids: Vec<String> = body
        .checks
        .iter()
        .map(|item| item.correlation_id.clone())
        .collect();

    // Convert HTTP request to server-layer request
    let server_checks: Vec<ServerBatchCheckItem> = body
        .checks
        .into_iter()
        .map(|item| ServerBatchCheckItem {
            user: item.tuple_key.user,
            relation: item.tuple_key.relation,
            object: item.tuple_key.object,
            context: item.context.unwrap_or_default(),
        })
        .collect();

    let server_request = ServerBatchCheckRequest::new(store_id, server_checks);

    // Delegate to BatchCheckHandler for parallel execution with deduplication
    let server_response = state
        .batch_handler
        .check(server_request)
        .await
        .map_err(batch_check_error_to_api_error)?;

    // Convert server response back to HTTP response format
    let mut result_map = std::collections::HashMap::new();
    for (correlation_id, item_result) in correlation_ids
        .into_iter()
        .zip(server_response.results.into_iter())
    {
        result_map.insert(
            correlation_id,
            BatchCheckSingleResultBody {
                allowed: item_result.allowed,
                error: item_result.error.map(|msg| BatchCheckErrorBody {
                    // Map error kind to appropriate HTTP status code
                    // Validation errors (type/relation not found, invalid input) → 400
                    // Internal errors (resolver errors, timeout) → 500
                    code: item_result
                        .error_kind
                        .map(|k| k.http_status_code())
                        .unwrap_or(500),
                    message: msg,
                }),
            },
        );
    }

    Ok(Json(BatchCheckResponseBody { result: result_map }))
}

// ============================================================
// Expand Operation
// ============================================================

/// Tuple key for expand operation (no user required).
#[derive(Debug, Deserialize)]
pub struct ExpandTupleKeyBody {
    pub relation: String,
    pub object: String,
}

/// Request body for expand operation.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ExpandRequestBody {
    pub tuple_key: ExpandTupleKeyBody,
    #[serde(default)]
    pub authorization_model_id: Option<String>,
}

/// Response for expand operation.
///
/// OpenFGA returns a nested structure with `tree.root` containing the expansion.
#[derive(Debug, Serialize)]
pub struct ExpandResponseBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tree: Option<ExpandTreeBody>,
}

/// Tree wrapper containing the root node.
///
/// This matches OpenFGA's response format where the tree has a `root` property.
#[derive(Debug, Serialize)]
pub struct ExpandTreeBody {
    pub root: ExpandNodeBody,
}

/// A node in the expansion tree.
#[derive(Debug, Serialize, Default)]
pub struct ExpandNodeBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leaf: Option<ExpandLeafBody>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub union: Option<ExpandNodesBody>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intersection: Option<ExpandNodesBody>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub difference: Option<ExpandDifferenceBody>,
}

impl ExpandNodeBody {
    fn new_leaf(name: String, leaf: ExpandLeafBody) -> Self {
        Self {
            name: Some(name),
            leaf: Some(leaf),
            ..Default::default()
        }
    }

    fn new_union(name: String, nodes: Vec<ExpandNodeBody>) -> Self {
        Self {
            name: Some(name),
            union: Some(ExpandNodesBody { nodes }),
            ..Default::default()
        }
    }

    fn new_intersection(name: String, nodes: Vec<ExpandNodeBody>) -> Self {
        Self {
            name: Some(name),
            intersection: Some(ExpandNodesBody { nodes }),
            ..Default::default()
        }
    }

    fn new_difference(name: String, base: ExpandNodeBody, subtract: ExpandNodeBody) -> Self {
        Self {
            name: Some(name),
            difference: Some(ExpandDifferenceBody {
                base: Box::new(base),
                subtract: Box::new(subtract),
            }),
            ..Default::default()
        }
    }
}

/// Container for child nodes in union/intersection.
#[derive(Debug, Serialize)]
pub struct ExpandNodesBody {
    pub nodes: Vec<ExpandNodeBody>,
}

/// Difference (exclusion) node structure.
#[derive(Debug, Serialize)]
pub struct ExpandDifferenceBody {
    pub base: Box<ExpandNodeBody>,
    pub subtract: Box<ExpandNodeBody>,
}

/// A leaf node containing users or references.
#[derive(Debug, Serialize, Default)]
pub struct ExpandLeafBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub users: Option<ExpandUsersBody>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub computed: Option<ExpandComputedBody>,
    #[serde(rename = "tupleToUserset", skip_serializing_if = "Option::is_none")]
    pub tuple_to_userset: Option<ExpandTupleToUsersetBody>,
}

impl ExpandLeafBody {
    fn new_users(users: Vec<String>) -> Self {
        Self {
            users: Some(ExpandUsersBody { users }),
            ..Default::default()
        }
    }

    fn new_computed(userset: String) -> Self {
        Self {
            computed: Some(ExpandComputedBody { userset }),
            ..Default::default()
        }
    }

    fn new_tuple_to_userset(
        tupleset: ExpandObjectRelationBody,
        computed_userset: ExpandObjectRelationBody,
    ) -> Self {
        Self {
            tuple_to_userset: Some(ExpandTupleToUsersetBody {
                tupleset,
                computed_userset,
            }),
            ..Default::default()
        }
    }
}

/// Direct users in a leaf node.
#[derive(Debug, Serialize)]
pub struct ExpandUsersBody {
    pub users: Vec<String>,
}

/// Computed userset reference.
#[derive(Debug, Serialize)]
pub struct ExpandComputedBody {
    pub userset: String,
}

/// Tuple-to-userset reference.
/// Note: Uses ObjectRelation-like structure to match OpenFGA's format.
#[derive(Debug, Serialize)]
pub struct ExpandTupleToUsersetBody {
    pub tupleset: ExpandObjectRelationBody,
    #[serde(rename = "computedUserset")]
    pub computed_userset: ExpandObjectRelationBody,
}

/// Object relation reference (matches OpenFGA's ObjectRelation).
#[derive(Debug, Serialize)]
pub struct ExpandObjectRelationBody {
    pub object: String,
    pub relation: String,
}

/// Converts a domain ExpandNode to an HTTP response body.
fn expand_node_to_body(node: rsfga_domain::resolver::ExpandNode) -> ExpandNodeBody {
    use rsfga_domain::resolver::{ExpandLeafValue, ExpandNode};

    match node {
        ExpandNode::Leaf(leaf) => {
            let leaf_body = match leaf.value {
                ExpandLeafValue::Users(users) => ExpandLeafBody::new_users(users),
                ExpandLeafValue::Computed { userset } => ExpandLeafBody::new_computed(userset),
                ExpandLeafValue::TupleToUserset {
                    tupleset,
                    computed_userset,
                } => {
                    // Extract object from leaf.name (format: "type:id#relation")
                    // The tupleset relation is on the same object being expanded
                    let object_for_tupleset = leaf.name.split('#').next().unwrap_or("").to_string();
                    ExpandLeafBody::new_tuple_to_userset(
                        ExpandObjectRelationBody {
                            object: object_for_tupleset,
                            relation: tupleset,
                        },
                        ExpandObjectRelationBody {
                            // computed_userset object is unknown without further resolution
                            object: String::new(),
                            relation: computed_userset,
                        },
                    )
                }
            };
            ExpandNodeBody::new_leaf(leaf.name, leaf_body)
        }
        ExpandNode::Union { name, nodes } => {
            let child_nodes = nodes.into_iter().map(expand_node_to_body).collect();
            ExpandNodeBody::new_union(name, child_nodes)
        }
        ExpandNode::Intersection { name, nodes } => {
            let child_nodes = nodes.into_iter().map(expand_node_to_body).collect();
            ExpandNodeBody::new_intersection(name, child_nodes)
        }
        ExpandNode::Difference {
            name,
            base,
            subtract,
        } => ExpandNodeBody::new_difference(
            name,
            expand_node_to_body(*base),
            expand_node_to_body(*subtract),
        ),
    }
}

async fn expand<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(store_id): Path<String>,
    JsonBadRequest(body): JsonBadRequest<ExpandRequestBody>,
) -> ApiResult<impl IntoResponse> {
    // OpenFGA returns 404 for any non-existent store, regardless of ID format.
    use rsfga_domain::resolver::ExpandRequest;

    // Create domain expand request
    let expand_request =
        ExpandRequest::new(&store_id, &body.tuple_key.relation, &body.tuple_key.object);

    // Delegate to GraphResolver for expansion
    let result = state.resolver.expand(&expand_request).await?;

    // Convert domain result to HTTP response
    // Wrap the root node in ExpandTreeBody to match OpenFGA's response format
    Ok(Json(ExpandResponseBody {
        tree: Some(ExpandTreeBody {
            root: expand_node_to_body(result.tree.root),
        }),
    }))
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
    JsonBadRequest(body): JsonBadRequest<WriteRequestBody>,
) -> ApiResult<impl IntoResponse> {
    use rsfga_storage::StoredTuple;

    // OpenFGA returns 404 for any non-existent store, regardless of ID format.
    // Validate tuple count before processing (OpenFGA limit: 100 tuples per write)
    let write_count = body.writes.as_ref().map_or(0, |w| w.tuple_keys.len());
    let delete_count = body.deletes.as_ref().map_or(0, |d| d.tuple_keys.len());
    let total_count = write_count + delete_count;
    if let Some(err) = validate_tuple_count(total_count) {
        return Err(ApiError::validation_error(err));
    }

    // Validate user/object ID lengths before processing (OpenFGA limits)
    if let Some(ref writes) = body.writes {
        for (i, tk) in writes.tuple_keys.iter().enumerate() {
            if let Some(err) = validate_user_id_length(&tk.user) {
                return Err(ApiError::validation_error(format!(
                    "write tuple at index {}: {}",
                    i, err
                )));
            }
            if let Some(err) = validate_object_id_length(&tk.object) {
                return Err(ApiError::validation_error(format!(
                    "write tuple at index {}: {}",
                    i, err
                )));
            }
        }
    }
    if let Some(ref deletes) = body.deletes {
        for (i, tk) in deletes.tuple_keys.iter().enumerate() {
            if let Some(err) = validate_user_id_length(&tk.user) {
                return Err(ApiError::validation_error(format!(
                    "delete tuple at index {}: {}",
                    i, err
                )));
            }
            if let Some(err) = validate_object_id_length(&tk.object) {
                return Err(ApiError::validation_error(format!(
                    "delete tuple at index {}: {}",
                    i, err
                )));
            }
        }
    }

    // Validate store exists
    let _ = state.storage.get_store(&store_id).await?;

    // Get the latest authorization model to validate tuples against
    // OpenFGA requires tuples to reference types/relations defined in the model
    let stored_model = state
        .storage
        .get_latest_authorization_model(&store_id)
        .await
        .map_err(|e| match e {
            StorageError::ModelNotFound { .. } => ApiError::validation_error(
                "cannot write tuples: no authorization model exists for this store",
            ),
            other => ApiError::from(other),
        })?;

    let model =
        crate::adapters::parse_model_json(&stored_model.model_json, &stored_model.schema_version)
            .map_err(|e| {
            // Log full error for debugging but don't leak internal details to client
            error!(
                "Failed to parse stored authorization model for store {}: {e}",
                store_id
            );
            ApiError::internal_error("failed to parse authorization model")
        })?;

    // Convert write tuples - fail if any tuple key is invalid
    // No clones in happy path - error contains user/object for messages
    let writes: Vec<StoredTuple> = body
        .writes
        .map(|w| {
            w.tuple_keys
                .into_iter()
                .enumerate()
                .map(|(i, tk)| {
                    parse_tuple_key(tk).map_err(|e| {
                        ApiError::validation_error(format!(
                            "invalid tuple at index {i}: user={}, object={}, reason={}",
                            e.user, e.object, e.reason
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
                        ApiError::validation_error(format!(
                            "invalid tuple at index {i}: user={}, object={}, reason=invalid format",
                            tk.user, tk.object
                        ))
                    })
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();

    // Validate all tuples against the authorization model
    // OpenFGA returns 400 if tuples reference undefined types, relations, or conditions
    crate::adapters::validate_tuples_batch(
        &model,
        writes.iter().enumerate().map(|(i, t)| {
            (
                i,
                t.object_type.as_str(),
                t.relation.as_str(),
                t.condition_name.as_deref(),
            )
        }),
        false,
    )
    .map_err(|e| ApiError::validation_error(e.to_string()))?;

    crate::adapters::validate_tuples_batch(
        &model,
        deletes.iter().enumerate().map(|(i, t)| {
            (
                i,
                t.object_type.as_str(),
                t.relation.as_str(),
                t.condition_name.as_deref(),
            )
        }),
        true,
    )
    .map_err(|e| ApiError::validation_error(e.to_string()))?;

    state
        .storage
        .write_tuples(&store_id, writes, deletes)
        .await?;

    // Invalidate cache for this store to prevent stale auth decisions.
    // This is a coarse-grained stopgap until fine-grained invalidation is wired.
    state.cache.invalidate_store(&store_id).await;

    Ok(Json(serde_json::json!({})))
}

// Use shared validation functions from the validation module
use crate::validation::{
    is_valid_condition_name, json_exceeds_max_depth, validate_object_id_length,
    validate_tuple_count, validate_user_id_length, MAX_CONDITION_CONTEXT_SIZE,
};

/// Error returned when tuple key parsing fails.
/// Contains the original user/object strings for error messages (avoids cloning in happy path).
struct TupleKeyParseError {
    user: String,
    object: String,
    reason: &'static str,
}

/// Parses a tuple key into a StoredTuple (takes ownership to avoid clones).
///
/// Includes condition parsing for conditional relationships.
/// Returns `Err` with the original user/object for error messages (avoids cloning in happy path).
fn parse_tuple_key(tk: TupleKeyBody) -> Result<rsfga_storage::StoredTuple, TupleKeyParseError> {
    // Parse user: "user:alice" or "team:eng#member"
    let (user_type, user_id, user_relation) =
        parse_user(&tk.user).ok_or_else(|| TupleKeyParseError {
            user: tk.user.clone(),
            object: tk.object.clone(),
            reason: "invalid user format",
        })?;

    // Parse object: "document:readme" - use parse_object for consistent validation
    let (object_type, object_id) = parse_object(&tk.object).ok_or_else(|| TupleKeyParseError {
        user: tk.user.clone(),
        object: tk.object.clone(),
        reason: "invalid object format",
    })?;

    // Parse and validate condition if present
    let (condition_name, condition_context) = if let Some(cond) = tk.condition {
        if cond.name.is_empty() {
            (None, None)
        } else {
            // Validate condition name format (security constraint I4)
            if !is_valid_condition_name(&cond.name) {
                return Err(TupleKeyParseError {
                    user: tk.user,
                    object: tk.object,
                    reason: "invalid condition name: must be alphanumeric/underscore/hyphen, max 256 chars",
                });
            }

            // Validate context if present (constraint C11)
            if let Some(ref ctx) = cond.context {
                // Check depth limit to prevent stack overflow
                if ctx.values().any(|v| json_exceeds_max_depth(v, 1)) {
                    return Err(TupleKeyParseError {
                        user: tk.user,
                        object: tk.object,
                        reason: "condition context exceeds maximum nesting depth (10 levels)",
                    });
                }

                // Validate size limit
                let estimated_size: usize =
                    ctx.iter().map(|(k, v)| k.len() + v.to_string().len()).sum();
                if estimated_size > MAX_CONDITION_CONTEXT_SIZE {
                    return Err(TupleKeyParseError {
                        user: tk.user,
                        object: tk.object,
                        reason: "condition context exceeds maximum size (10KB)",
                    });
                }
            }

            (Some(cond.name), cond.context)
        }
    } else {
        (None, None)
    };

    Ok(rsfga_storage::StoredTuple {
        object_type: object_type.to_string(),
        object_id: object_id.to_string(),
        relation: tk.relation,
        user_type: user_type.to_string(),
        user_id: user_id.to_string(),
        user_relation: user_relation.map(|s| s.to_string()),
        condition_name,
        condition_context,
        // Set created_at at write time to ensure consistent timestamps
        // This prevents inconsistent timestamps when reading from memory backend
        created_at: Some(Utc::now()),
    })
}

/// Parses tuple fields directly into a StoredTuple (without condition).
///
/// This is used for delete operations where conditions are not applicable.
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
        condition_name: None,
        condition_context: None,
        // Set created_at at write time (note: deletes don't use this timestamp)
        created_at: Some(Utc::now()),
    })
}

// ============================================================
// Read Operation
// ============================================================

/// Tuple key filter for read operations.
///
/// Unlike `TupleKeyWithoutConditionBody` used for writes (where all fields are required),
/// the read filter allows partial matching - any field can be omitted to match all values.
///
/// This matches OpenFGA's behavior where:
/// - `{}` returns all tuples
/// - `{"user": "user:alice"}` returns all tuples for alice
/// - `{"object": "document:"}` returns all tuples for documents (type prefix)
/// - `{"user": "user:alice", "relation": "viewer"}` returns alice's viewer tuples
#[derive(Debug, Default, Deserialize)]
pub struct ReadTupleKeyFilter {
    /// Optional user filter. If empty string, treated as no filter.
    #[serde(default)]
    pub user: String,
    /// Optional relation filter. If empty string, treated as no filter.
    #[serde(default)]
    pub relation: String,
    /// Optional object filter. Supports both full object ("type:id") and type prefix ("type:").
    /// If empty string, treated as no filter.
    #[serde(default)]
    pub object: String,
}

/// Request body for read operation.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ReadRequestBody {
    #[serde(default)]
    pub tuple_key: Option<ReadTupleKeyFilter>,
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
    /// Condition for conditional relationships (OpenFGA compatibility I2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<RelationshipConditionResponseBody>,
}

/// Response body for relationship condition.
#[derive(Debug, Serialize)]
pub struct RelationshipConditionResponseBody {
    /// The name of the condition.
    pub name: String,
    /// Optional context parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<std::collections::HashMap<String, serde_json::Value>>,
}

async fn read_tuples<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(store_id): Path<String>,
    JsonBadRequest(body): JsonBadRequest<ReadRequestBody>,
) -> ApiResult<impl IntoResponse> {
    use rsfga_storage::TupleFilter;

    // OpenFGA returns 404 for any non-existent store, regardless of ID format.
    // Validate store exists
    let _ = state.storage.get_store(&store_id).await?;

    // Build filter from request
    // For Read API, we support type-prefix filtering (e.g., "document:" matches all documents)
    // Invalid object format is treated as "no object filter" rather than an error
    // since read is a query operation, not a write
    let filter = if let Some(tk) = body.tuple_key {
        // Parse object filter - supports both full object ("type:id") and type prefix ("type:")
        let (object_type, object_id) = if !tk.object.is_empty() {
            if let Some((obj_type, obj_id)) = tk.object.split_once(':') {
                if obj_type.is_empty() {
                    // Invalid format like ":id" - treat as no filter
                    (None, None)
                } else if obj_id.is_empty() {
                    // Type prefix like "document:" - filter by type only
                    (Some(obj_type.to_string()), None)
                } else {
                    // Full object like "document:doc1"
                    (Some(obj_type.to_string()), Some(obj_id.to_string()))
                }
            } else {
                // No colon - invalid format, treat as no filter
                (None, None)
            }
        } else {
            (None, None)
        };

        TupleFilter {
            object_type,
            object_id,
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

    // Build pagination options from request
    // Validate page_size is positive before casting to u32 (negative i32 wraps to huge u32)
    let page_size = match body.page_size {
        Some(s) if s > 0 => Some(s as u32),
        Some(_) => return Err(ApiError::validation_error("page_size must be positive")),
        None => None,
    };
    let pagination = rsfga_storage::PaginationOptions {
        page_size,
        continuation_token: body.continuation_token,
    };

    let result = state
        .storage
        .read_tuples_paginated(&store_id, &filter, &pagination)
        .await?;

    // Convert to response format, including conditions (OpenFGA compatibility I2)
    let response_tuples: Vec<TupleResponseBody> = result
        .items
        .into_iter()
        .map(|t| TupleResponseBody {
            key: TupleKeyResponseBody {
                user: format_user(&t.user_type, &t.user_id, t.user_relation.as_deref()),
                relation: t.relation,
                object: format!("{}:{}", t.object_type, t.object_id),
                condition: t
                    .condition_name
                    .map(|name| RelationshipConditionResponseBody {
                        name,
                        context: t.condition_context,
                    }),
            },
            timestamp: t.created_at.map(|dt| dt.to_rfc3339()),
        })
        .collect();

    Ok(Json(ReadResponseBody {
        tuples: response_tuples,
        continuation_token: result.continuation_token,
    }))
}

// ============================================================
// Read Changes Operation
// ============================================================

/// Query parameters for read changes operation.
#[derive(Debug, Deserialize)]
pub struct ReadChangesQuery {
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub page_size: Option<i32>,
    #[serde(default)]
    pub continuation_token: Option<String>,
}

/// Response for read changes operation.
#[derive(Debug, Serialize)]
pub struct ReadChangesResponseBody {
    pub changes: Vec<TupleChangeBody>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuation_token: Option<String>,
}

/// A tuple change (write or delete).
#[derive(Debug, Serialize)]
pub struct TupleChangeBody {
    pub tuple_key: TupleKeyResponseBody,
    pub operation: String,
    pub timestamp: String,
}

async fn read_changes<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(store_id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<ReadChangesQuery>,
) -> ApiResult<impl IntoResponse> {
    // OpenFGA returns 404 for any non-existent store, regardless of ID format.
    // Validate store exists
    let _ = state.storage.get_store(&store_id).await?;

    // Build filter for ReadChanges
    let filter = rsfga_storage::ReadChangesFilter {
        object_type: query.r#type.clone(),
    };

    // Validate page_size is positive before casting to u32 (negative i32 wraps to huge u32)
    let page_size = match query.page_size {
        Some(s) if s > 0 => Some(s as u32),
        Some(_) => return Err(ApiError::validation_error("page_size must be positive")),
        None => None,
    };
    let pagination = rsfga_storage::PaginationOptions {
        page_size,
        continuation_token: query.continuation_token,
    };

    // Read changes from changelog (ordered chronologically)
    let result = state
        .storage
        .read_changes(&store_id, &filter, &pagination)
        .await?;

    // Convert TupleChange to response body
    let changes: Vec<TupleChangeBody> = result
        .items
        .into_iter()
        .map(|change| TupleChangeBody {
            tuple_key: TupleKeyResponseBody {
                user: format_user(
                    &change.tuple.user_type,
                    &change.tuple.user_id,
                    change.tuple.user_relation.as_deref(),
                ),
                relation: change.tuple.relation,
                object: format!("{}:{}", change.tuple.object_type, change.tuple.object_id),
                condition: change.tuple.condition_name.map(|name| {
                    RelationshipConditionResponseBody {
                        name,
                        context: change.tuple.condition_context,
                    }
                }),
            },
            operation: change.operation.to_string(),
            timestamp: change.timestamp.to_rfc3339(),
        })
        .collect();

    Ok(Json(ReadChangesResponseBody {
        changes,
        continuation_token: result.continuation_token,
    }))
}

// ============================================================
// Assertions API
// ============================================================

/// Request body for write assertions operation.
#[derive(Debug, Deserialize)]
pub struct WriteAssertionsRequestBody {
    pub assertions: Vec<AssertionBody>,
}

/// A single assertion in the request/response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssertionBody {
    pub tuple_key: AssertionTupleKeyBody,
    pub expectation: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contextual_tuples: Option<AssertionContextualTuplesBody>,
}

/// Tuple key for assertions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssertionTupleKeyBody {
    pub user: String,
    pub relation: String,
    pub object: String,
}

/// Contextual tuples for assertions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssertionContextualTuplesBody {
    pub tuple_keys: Vec<AssertionTupleKeyBody>,
}

/// Response body for read assertions operation.
#[derive(Debug, Serialize)]
pub struct ReadAssertionsResponseBody {
    pub assertions: Vec<AssertionBody>,
}

/// Write assertions for an authorization model (PUT).
async fn write_assertions<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path((store_id, authorization_model_id)): Path<(String, String)>,
    JsonBadRequest(body): JsonBadRequest<WriteAssertionsRequestBody>,
) -> ApiResult<impl IntoResponse> {
    // OpenFGA returns 404 for any non-existent store, regardless of ID format.
    // Validate store exists
    let _ = state.storage.get_store(&store_id).await?;

    // Validate model exists
    let _ = state
        .storage
        .get_authorization_model(&store_id, &authorization_model_id)
        .await?;

    // Check assertion entry capacity to prevent unbounded memory growth.
    // Only check for NEW keys - updates to existing keys don't increase count.
    let assertion_key = (store_id.clone(), authorization_model_id.clone());
    let current_count = state.assertions.len();
    let is_new_key = !state.assertions.contains_key(&assertion_key);

    if is_new_key {
        if current_count >= MAX_ASSERTION_ENTRIES {
            tracing::error!(
                current_count = current_count,
                max = MAX_ASSERTION_ENTRIES,
                store_id = %store_id,
                model_id = %authorization_model_id,
                "Assertion storage capacity exceeded"
            );
            return Err(ApiError::resource_exhausted(format!(
                "assertion storage limit reached ({} entries), delete unused stores to free space",
                MAX_ASSERTION_ENTRIES
            )));
        }

        if current_count >= ASSERTION_ENTRIES_WARNING_THRESHOLD {
            tracing::warn!(
                current_count = current_count,
                threshold = ASSERTION_ENTRIES_WARNING_THRESHOLD,
                max = MAX_ASSERTION_ENTRIES,
                "Assertion storage nearing capacity"
            );
        }
    }

    // Convert assertions to stored format
    use super::state::{AssertionTupleKey, ContextualTuplesWrapper, StoredAssertion};

    let stored_assertions: Vec<StoredAssertion> = body
        .assertions
        .into_iter()
        .map(|a| StoredAssertion {
            tuple_key: AssertionTupleKey {
                user: a.tuple_key.user,
                relation: a.tuple_key.relation,
                object: a.tuple_key.object,
                condition: None, // HTTP API doesn't support conditions in assertions yet
            },
            expectation: a.expectation,
            contextual_tuples: a.contextual_tuples.map(|ct| ContextualTuplesWrapper {
                tuple_keys: ct
                    .tuple_keys
                    .into_iter()
                    .map(|tk| AssertionTupleKey {
                        user: tk.user,
                        relation: tk.relation,
                        object: tk.object,
                        condition: None, // HTTP API doesn't support conditions in assertions yet
                    })
                    .collect(),
            }),
        })
        .collect();

    // Store assertions (replaces existing)
    let key = (store_id, authorization_model_id);
    state.assertions.insert(key, stored_assertions);

    Ok(Json(serde_json::json!({})))
}

/// Read assertions for an authorization model (GET).
async fn read_assertions<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path((store_id, authorization_model_id)): Path<(String, String)>,
) -> ApiResult<impl IntoResponse> {
    // OpenFGA returns 404 for any non-existent store, regardless of ID format.
    // Validate store exists
    let _ = state.storage.get_store(&store_id).await?;

    // Validate model exists
    let _ = state
        .storage
        .get_authorization_model(&store_id, &authorization_model_id)
        .await?;

    // Read assertions
    let key = (store_id, authorization_model_id);
    let stored_assertions = state.assertions.get(&key);

    let assertions: Vec<AssertionBody> = stored_assertions
        .map(|sa| {
            sa.value()
                .iter()
                .map(|a| AssertionBody {
                    tuple_key: AssertionTupleKeyBody {
                        user: a.tuple_key.user.clone(),
                        relation: a.tuple_key.relation.clone(),
                        object: a.tuple_key.object.clone(),
                    },
                    expectation: a.expectation,
                    contextual_tuples: a.contextual_tuples.as_ref().map(|ct| {
                        AssertionContextualTuplesBody {
                            tuple_keys: ct
                                .tuple_keys
                                .iter()
                                .map(|tk| AssertionTupleKeyBody {
                                    user: tk.user.clone(),
                                    relation: tk.relation.clone(),
                                    object: tk.object.clone(),
                                })
                                .collect(),
                        }
                    }),
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(Json(ReadAssertionsResponseBody { assertions }))
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
    #[serde(default)]
    pub context: Option<std::collections::HashMap<String, serde_json::Value>>,
}

/// Response for list objects operation (stub).
#[derive(Debug, Serialize)]
pub struct ListObjectsResponseBody {
    pub objects: Vec<String>,
    pub truncated: bool,
}

async fn list_objects<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(store_id): Path<String>,
    JsonBadRequest(body): JsonBadRequest<ListObjectsRequestBody>,
) -> ApiResult<impl IntoResponse> {
    use crate::validation::{
        estimate_context_size, json_exceeds_max_depth, validate_relation_format,
        validate_user_format, MAX_CONDITION_CONTEXT_SIZE, MAX_JSON_DEPTH,
        MAX_LIST_OBJECTS_CANDIDATES,
    };
    use rsfga_domain::resolver::ListObjectsRequest;
    use rsfga_storage::traits::validate_object_type;
    use tracing::warn;

    // OpenFGA returns 404 for any non-existent store, regardless of ID format.
    // Validate input format (API layer validation)
    validate_object_type(&body.r#type)?;

    // Validate user format
    if let Some(err) = validate_user_format(&body.user) {
        return Err(ApiError::validation_error(err));
    }

    // Validate relation format
    if let Some(err) = validate_relation_format(&body.relation) {
        return Err(ApiError::validation_error(err));
    }

    // Validate context if provided (DoS protection)
    if let Some(ctx) = &body.context {
        if estimate_context_size(ctx) > MAX_CONDITION_CONTEXT_SIZE {
            return Err(ApiError::validation_error(format!(
                "context size exceeds maximum of {MAX_CONDITION_CONTEXT_SIZE} bytes"
            )));
        }

        // Check nesting depth for each value in context map.
        // We pass depth=2 because context is already at depth 1 (the map itself),
        // so values start at depth 2. MAX_JSON_DEPTH (5) limits total nesting from the root.
        for value in ctx.values() {
            if json_exceeds_max_depth(value, 2) {
                return Err(ApiError::validation_error(format!(
                    "context nested too deeply (max depth {MAX_JSON_DEPTH})"
                )));
            }
        }
    }

    // Convert contextual tuples if provided
    let contextual_tuples = body
        .contextual_tuples
        .map(|ct| {
            ct.tuple_keys
                .into_iter()
                .filter_map(|tk| {
                    let user = parse_user(&tk.user);
                    if user.is_none() {
                        warn!("Invalid user format in contextual tuple: {}", tk.user);
                        return None;
                    }
                    let (user_type, user_id, user_relation) = user.unwrap();

                    let object = parse_object(&tk.object);
                    if object.is_none() {
                        warn!("Invalid object format in contextual tuple: {}", tk.object);
                        return None;
                    }
                    let (object_type, object_id) = object.unwrap();

                    let user_str = format_user(user_type, user_id, user_relation);
                    let object_str = format!("{object_type}:{object_id}");

                    // Preserve condition if present
                    if let Some(condition) = tk.condition {
                        Some(rsfga_domain::resolver::ContextualTuple::with_condition(
                            user_str,
                            tk.relation,
                            object_str,
                            condition.name,
                            condition.context,
                        ))
                    } else {
                        Some(rsfga_domain::resolver::ContextualTuple::new(
                            user_str,
                            tk.relation,
                            object_str,
                        ))
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    // Create domain request
    let list_request = ListObjectsRequest::with_context(
        store_id,
        body.user,
        body.relation,
        body.r#type,
        contextual_tuples,
        body.context.unwrap_or_default(),
    );

    // Call the resolver with DoS protection limit
    let result = state
        .resolver
        .list_objects(&list_request, MAX_LIST_OBJECTS_CANDIDATES)
        .await?;

    Ok(Json(ListObjectsResponseBody {
        objects: result.objects,
        truncated: result.truncated,
    }))
}

// ============================================================
// List Users Operation
// ============================================================

/// Request body for list users operation.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ListUsersRequestBody {
    /// The object to check permissions for.
    pub object: ObjectBody,
    /// The relation to check.
    pub relation: String,
    /// Filter for user types to return.
    pub user_filters: Vec<UserFilterBody>,
    #[serde(default)]
    pub authorization_model_id: Option<String>,
    #[serde(default)]
    pub contextual_tuples: Option<ContextualTuplesBody>,
    #[serde(default)]
    pub context: Option<std::collections::HashMap<String, serde_json::Value>>,
}

/// Object reference in ListUsers request.
#[derive(Debug, Deserialize)]
pub struct ObjectBody {
    pub r#type: String,
    pub id: String,
}

/// User filter in ListUsers request.
#[derive(Debug, Deserialize)]
pub struct UserFilterBody {
    pub r#type: String,
    #[serde(default)]
    pub relation: Option<String>,
}

/// Response for list users operation.
#[derive(Debug, Serialize)]
pub struct ListUsersResponseBody {
    pub users: Vec<UserResultBody>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_users: Vec<UserResultBody>,
}

/// A user result in the response.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum UserResultBody {
    Object { object: UserObjectBody },
    Userset { userset: UserUsersetBody },
    Wildcard { wildcard: UserWildcardBody },
}

#[derive(Debug, Serialize)]
pub struct UserObjectBody {
    pub r#type: String,
    pub id: String,
}

#[derive(Debug, Serialize)]
pub struct UserUsersetBody {
    pub r#type: String,
    pub id: String,
    pub relation: String,
}

#[derive(Debug, Serialize)]
pub struct UserWildcardBody {
    pub r#type: String,
}

async fn list_users<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(store_id): Path<String>,
    JsonBadRequest(body): JsonBadRequest<ListUsersRequestBody>,
) -> ApiResult<impl IntoResponse> {
    use crate::validation::{
        estimate_context_size, json_exceeds_max_depth, validate_relation_format,
        MAX_CONDITION_CONTEXT_SIZE, MAX_JSON_DEPTH,
    };
    use rsfga_domain::resolver::{ListUsersRequest, UserFilter, UserResult};
    use rsfga_storage::traits::validate_object_type;
    use tracing::warn;

    // OpenFGA returns 404 for any non-existent store, regardless of ID format.
    // Validate object type format
    validate_object_type(&body.object.r#type)?;

    // Validate full object reference format (not just empty ID)
    let object_str = format!("{}:{}", body.object.r#type, body.object.id);
    if parse_object(&object_str).is_none() {
        return Err(ApiError::validation_error(format!(
            "object has invalid format: {}",
            object_str
        )));
    }

    // Validate relation format
    if let Some(err) = validate_relation_format(&body.relation) {
        return Err(ApiError::validation_error(err));
    }

    // Validate user_filters not empty
    if body.user_filters.is_empty() {
        return Err(ApiError::validation_error("user_filters cannot be empty"));
    }

    // Validate user filter types and relations
    for filter in &body.user_filters {
        if filter.r#type.is_empty() {
            return Err(ApiError::validation_error(
                "user_filters type cannot be empty",
            ));
        }
        // If relation is provided, validate its format
        if let Some(ref rel) = filter.relation {
            if rel.is_empty() {
                return Err(ApiError::validation_error(
                    "user_filters relation cannot be empty",
                ));
            }
            if let Some(err) = validate_relation_format(rel) {
                return Err(ApiError::validation_error(err));
            }
        }
    }

    // Validate context if provided (DoS protection)
    if let Some(ctx) = &body.context {
        if estimate_context_size(ctx) > MAX_CONDITION_CONTEXT_SIZE {
            return Err(ApiError::validation_error(format!(
                "context size exceeds maximum of {MAX_CONDITION_CONTEXT_SIZE} bytes"
            )));
        }

        for value in ctx.values() {
            if json_exceeds_max_depth(value, 2) {
                return Err(ApiError::validation_error(format!(
                    "context nested too deeply (max depth {MAX_JSON_DEPTH})"
                )));
            }
        }
    }

    // Convert user_filters
    let user_filters: Vec<UserFilter> = body
        .user_filters
        .into_iter()
        .map(|f| {
            if let Some(rel) = f.relation {
                UserFilter::with_relation(f.r#type, rel)
            } else {
                UserFilter::new(f.r#type)
            }
        })
        .collect();

    // Convert contextual tuples if provided
    let contextual_tuples = body
        .contextual_tuples
        .map(|ct| {
            ct.tuple_keys
                .into_iter()
                .filter_map(|tk| {
                    let user = parse_user(&tk.user);
                    if user.is_none() {
                        warn!("Invalid user format in contextual tuple: {}", tk.user);
                        return None;
                    }
                    let (user_type, user_id, user_relation) = user.unwrap();

                    let object = parse_object(&tk.object);
                    if object.is_none() {
                        warn!("Invalid object format in contextual tuple: {}", tk.object);
                        return None;
                    }
                    let (object_type, object_id) = object.unwrap();

                    let user_str = format_user(user_type, user_id, user_relation);
                    let object_str = format!("{object_type}:{object_id}");

                    // Preserve condition if present
                    if let Some(condition) = tk.condition {
                        Some(rsfga_domain::resolver::ContextualTuple::with_condition(
                            user_str,
                            tk.relation,
                            object_str,
                            condition.name,
                            condition.context,
                        ))
                    } else {
                        Some(rsfga_domain::resolver::ContextualTuple::new(
                            user_str,
                            tk.relation,
                            object_str,
                        ))
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    // Create domain request (object_str already validated above)
    let list_request = ListUsersRequest::with_context(
        store_id,
        object_str,
        body.relation,
        user_filters,
        contextual_tuples,
        body.context.unwrap_or_default(),
    );

    // Call the resolver with default max results for DoS protection.
    // OpenFGA's API doesn't support pagination for ListUsers, so we use an internal limit.
    const DEFAULT_MAX_RESULTS: usize = 1000;
    let result = state
        .resolver
        .list_users(&list_request, DEFAULT_MAX_RESULTS)
        .await?;

    // Helper to convert domain result to API response body
    fn to_user_result_body(user: UserResult) -> UserResultBody {
        match user {
            UserResult::Object { user_type, user_id } => UserResultBody::Object {
                object: UserObjectBody {
                    r#type: user_type,
                    id: user_id,
                },
            },
            UserResult::Userset {
                userset_type,
                userset_id,
                relation,
            } => UserResultBody::Userset {
                userset: UserUsersetBody {
                    r#type: userset_type,
                    id: userset_id,
                    relation,
                },
            },
            UserResult::Wildcard { wildcard_type } => UserResultBody::Wildcard {
                wildcard: UserWildcardBody {
                    r#type: wildcard_type,
                },
            },
        }
    }

    // Convert domain results to response format
    let users: Vec<UserResultBody> = result.users.into_iter().map(to_user_result_body).collect();

    let excluded_users: Vec<UserResultBody> = result
        .excluded_users
        .into_iter()
        .map(to_user_result_body)
        .collect();

    Ok(Json(ListUsersResponseBody {
        users,
        excluded_users,
    }))
}
