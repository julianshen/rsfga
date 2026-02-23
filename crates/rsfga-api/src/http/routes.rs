//! HTTP route definitions and handlers.

use std::sync::Arc;

use axum::{
    async_trait,
    extract::{FromRequest, Path, Request, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tower_http::limit::RequestBodyLimitLayer;
use tracing::{error, trace};

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
    let router = Router::new()
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
        );

    // Add async write routes when NATS feature is enabled
    #[cfg(feature = "nats")]
    let router = router
        .route(
            "/async/stores/:store_id/write",
            post(async_write_tuples::<S>),
        )
        .route(
            "/async/stores/:store_id/authorization-models",
            post(async_write_authorization_model::<S>),
        );

    router
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
    /// Entity count exceeds the allowed limit (e.g., too many tuples in a write).
    pub const EXCEEDED_ENTITY_LIMIT: &str = "exceeded_entity_limit";
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

    /// Creates an exceeded entity limit error (400).
    /// Used when the number of entities exceeds the allowed limit.
    pub fn exceeded_entity_limit(message: impl Into<String>) -> Self {
        Self::new(error_codes::EXCEEDED_ENTITY_LIMIT, message)
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
            | RELATION_NOT_FOUND
            | EXCEEDED_ENTITY_LIMIT => StatusCode::BAD_REQUEST,

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
    // OpenFGA requires store name to be at least 3 characters
    if body.name.len() < 3 {
        return Err(ApiError::validation_error(
            "store name must be at least 3 characters",
        ));
    }
    let id = ulid::Ulid::new().to_string();
    let store = state.storage.create_store(&id, &body.name).await?;

    Ok((StatusCode::CREATED, Json(StoreResponse::from(store))))
}

async fn get_store<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(store_id): Path<String>,
) -> ApiResult<impl IntoResponse> {
    // OpenFGA validates ULID format first → 400 for invalid, 404 for missing.
    if let Some(err) = crate::validation::validate_store_id_format(&store_id) {
        return Err(ApiError::validation_error(err));
    }
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
    if let Some(err) = crate::validation::validate_store_id_format(&store_id) {
        return Err(ApiError::validation_error(err));
    }
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
    if let Some(err) = crate::validation::validate_store_id_format(&store_id) {
        return Err(ApiError::validation_error(err));
    }
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

/// Consistency preferences for read operations.
///
/// Supports two formats for API compatibility:
/// - **OpenFGA string format**: `"MINIMIZE_LATENCY"`, `"HIGHER_CONSISTENCY"` (accepted and mapped)
/// - **RYOW object format**: `{"minimize_latency": true, "write_ticket": {...}}` (for async writes)
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ConsistencyPreference {
    /// If true, skip RYOW wait to minimize latency (eventual consistency).
    pub minimize_latency: bool,
    /// Write ticket from a previous async write operation.
    /// If present, the server will wait for this write to be committed
    /// before executing the read operation.
    pub write_ticket: Option<WriteTicketParam>,
}

impl<'de> serde::Deserialize<'de> for ConsistencyPreference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de;

        struct ConsistencyVisitor;

        impl<'de> de::Visitor<'de> for ConsistencyVisitor {
            type Value = ConsistencyPreference;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str(
                    "a consistency string (\"MINIMIZE_LATENCY\", \"HIGHER_CONSISTENCY\") or an object with optional minimize_latency and write_ticket fields",
                )
            }

            // Accept OpenFGA string format: "MINIMIZE_LATENCY", "HIGHER_CONSISTENCY", etc.
            fn visit_str<E>(self, value: &str) -> Result<ConsistencyPreference, E>
            where
                E: de::Error,
            {
                match value {
                    "MINIMIZE_LATENCY" => Ok(ConsistencyPreference {
                        minimize_latency: true,
                        write_ticket: None,
                    }),
                    // For any other string (including "HIGHER_CONSISTENCY",
                    // "UNSPECIFIED"), treat as default (no special handling needed)
                    _ => Ok(ConsistencyPreference::default()),
                }
            }

            // Accept RYOW object format: {"minimize_latency": true, "write_ticket": {...}}
            fn visit_map<M>(self, map: M) -> Result<ConsistencyPreference, M::Error>
            where
                M: de::MapAccess<'de>,
            {
                #[derive(Deserialize)]
                struct ConsistencyFields {
                    #[serde(default)]
                    minimize_latency: bool,
                    #[serde(default)]
                    write_ticket: Option<WriteTicketParam>,
                }

                let fields =
                    ConsistencyFields::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(ConsistencyPreference {
                    minimize_latency: fields.minimize_latency,
                    write_ticket: fields.write_ticket,
                })
            }
        }

        deserializer.deserialize_any(ConsistencyVisitor)
    }
}

/// Write ticket parameter for RYOW consistency.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WriteTicketParam {
    pub store_id: String,
    pub sequence: u64,
}

/// Tuple key for check requests with lenient deserialization.
///
/// Unlike `TupleKeyBody` (used by write/contextual tuples), this struct
/// uses `#[serde(default)]` so that missing fields deserialize as empty
/// strings instead of causing a JSON parse error. This allows the check
/// handler to report ALL missing fields at once (e.g. "relation, object")
/// rather than only the first one serde encounters.
#[derive(Debug, Deserialize)]
pub struct CheckTupleKeyBody {
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub relation: String,
    #[serde(default)]
    pub object: String,
    #[serde(default)]
    pub condition: Option<RelationshipConditionBody>,
}

/// Request body for check operation.
// Fields will be used when full resolver is integrated.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct CheckRequestBody {
    pub tuple_key: CheckTupleKeyBody,
    #[serde(default)]
    pub authorization_model_id: Option<String>,
    #[serde(default)]
    pub contextual_tuples: Option<ContextualTuplesBody>,
    /// CEL evaluation context for condition evaluation.
    /// Contains values that will be accessible as `request.<key>` in CEL expressions.
    #[serde(default)]
    pub context: Option<std::collections::HashMap<String, serde_json::Value>>,
    /// Consistency preferences for RYOW support.
    #[serde(default)]
    pub consistency: Option<ConsistencyPreference>,
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

// Implement TupleKeyLike for TupleKeyBody to enable shared validation
impl crate::validation::TupleKeyLike for TupleKeyBody {
    fn user(&self) -> &str {
        &self.user
    }
    fn object(&self) -> &str {
        &self.object
    }
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

/// Consistency level extracted from the `X-Consistency` HTTP header.
///
/// Provides an alternative to the body-level `consistency` field:
/// - `eventual`: Skip RYOW wait (minimize latency)
/// - `strong`: Wait for the latest committed sequence at call time
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConsistencyLevel {
    Eventual,
    Strong,
}

/// Extract consistency preference from the `X-Consistency` HTTP header.
///
/// Recognized values (case-insensitive): `eventual`, `strong`.
/// Unknown or absent values return `None`.
fn extract_consistency_header(headers: &HeaderMap) -> Option<ConsistencyLevel> {
    let level = headers
        .get("x-consistency")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            if v.eq_ignore_ascii_case("eventual") {
                Some(ConsistencyLevel::Eventual)
            } else if v.eq_ignore_ascii_case("strong") {
                Some(ConsistencyLevel::Strong)
            } else {
                trace!(value = %v, "Unknown X-Consistency header value, ignoring");
                None
            }
        });
    if let Some(ref l) = level {
        trace!(consistency = ?l, "X-Consistency header extracted");
    }
    level
}

/// Wait for a write ticket to be committed if RYOW consistency is requested.
///
/// Supports two consistency mechanisms:
/// 1. **Body-level** (`consistency.write_ticket`): Waits for a specific sequence (RYOW)
/// 2. **Header-level** (`X-Consistency: strong`): Waits for the latest committed sequence
///
/// Body-level consistency takes precedence over header-level when both are present.
///
/// If `X-Consistency: eventual` is set and no body-level consistency is provided,
/// consistency checks are skipped entirely for lower latency.
///
/// This is a no-op if the NATS feature is disabled or no WriteTracker is configured.
///
/// Returns 504 Gateway Timeout if the write is not committed within the timeout.
/// Returns 400 Bad Request if the write ticket's store_id doesn't match the request.
#[allow(unused_variables)]
async fn wait_for_consistency<S: DataStore>(
    state: &AppState<S>,
    consistency: Option<&ConsistencyPreference>,
    header_consistency: Option<ConsistencyLevel>,
    expected_store_id: &str,
) -> ApiResult<()> {
    #[cfg(feature = "nats")]
    {
        // Body-level consistency takes precedence over header-level
        if let Some(pref) = consistency {
            // minimize_latency in body suppresses header-level strong consistency
            if pref.minimize_latency {
                trace!(store_id = %expected_store_id, "Body minimize_latency suppresses header consistency");
                return Ok(());
            }

            if let Some(ticket) = &pref.write_ticket {
                // Validate that the write ticket's store_id matches the request's store_id
                // to prevent cross-store ticket attacks
                if ticket.store_id != expected_store_id {
                    return Err(ApiError::validation_error(format!(
                        "write_ticket.store_id '{}' does not match request store_id '{}'",
                        ticket.store_id, expected_store_id
                    )));
                }

                if let Some(tracker) = state.write_tracker() {
                    let timeout_secs = state.ryow_timeout_secs();
                    let start = std::time::Instant::now();

                    metrics::counter!("rsfga_api_ryow_wait_total").increment(1);

                    match tracker
                        .wait_for_commit(
                            &ticket.store_id,
                            ticket.sequence,
                            std::time::Duration::from_secs(timeout_secs),
                        )
                        .await
                    {
                        Ok(()) => {
                            let elapsed_ms = start.elapsed().as_millis() as f64;
                            metrics::histogram!("rsfga_api_ryow_wait_duration_ms")
                                .record(elapsed_ms);
                            metrics::counter!("rsfga_api_ryow_wait_success_total").increment(1);
                        }
                        Err(e) => {
                            metrics::counter!("rsfga_api_ryow_wait_timeout_total").increment(1);
                            return Err(ApiError::gateway_timeout(format!(
                                "RYOW timeout: write not yet committed (store_id: {}, sequence: {})",
                                e.store_id, e.sequence
                            )));
                        }
                    }
                }
                return Ok(());
            }
        }

        // Header-level consistency (only if no body-level write_ticket)
        if let Some(ConsistencyLevel::Strong) = header_consistency {
            trace!(store_id = %expected_store_id, "Header-level strong consistency requested");
            if let Some(tracker) = state.write_tracker() {
                if let Some(sequence) = tracker.get_committed_sequence(expected_store_id) {
                    if sequence > 0 {
                        let timeout_secs = state.ryow_timeout_secs();
                        let start = std::time::Instant::now();

                        metrics::counter!("rsfga_api_strong_consistency_wait_total").increment(1);

                        match tracker
                            .wait_for_commit(
                                expected_store_id,
                                sequence,
                                std::time::Duration::from_secs(timeout_secs),
                            )
                            .await
                        {
                            Ok(()) => {
                                let elapsed_ms = start.elapsed().as_millis() as f64;
                                metrics::histogram!(
                                    "rsfga_api_strong_consistency_wait_duration_ms"
                                )
                                .record(elapsed_ms);
                            }
                            Err(e) => {
                                metrics::counter!(
                                    "rsfga_api_strong_consistency_wait_timeout_total"
                                )
                                .increment(1);
                                return Err(ApiError::gateway_timeout(format!(
                                    "Strong consistency timeout (store_id: {}, sequence: {})",
                                    e.store_id, e.sequence
                                )));
                            }
                        }
                    }
                }
            }
        }
        // ConsistencyLevel::Eventual or None: no-op
    }
    Ok(())
}

async fn check<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(store_id): Path<String>,
    headers: HeaderMap,
    JsonBadRequest(body): JsonBadRequest<CheckRequestBody>,
) -> ApiResult<impl IntoResponse> {
    // Validate required tuple_key fields — report ALL missing fields at once
    {
        let mut missing = Vec::new();
        if body.tuple_key.user.is_empty() {
            missing.push("user");
        }
        if body.tuple_key.relation.is_empty() {
            missing.push("relation");
        }
        if body.tuple_key.object.is_empty() {
            missing.push("object");
        }
        if !missing.is_empty() {
            return Err(ApiError::validation_error(format!(
                "tuple_key missing required fields: {}",
                missing.join(", ")
            )));
        }
    }

    // RYOW: Wait for write ticket if consistency preferences are specified.
    // Validates that the write ticket's store_id matches the request path's store_id
    // to prevent cross-store ticket attacks.
    let header_consistency = extract_consistency_header(&headers);
    wait_for_consistency(
        &state,
        body.consistency.as_ref(),
        header_consistency,
        &store_id,
    )
    .await?;

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

    // Validate contextual tuple count before allocation
    if let Some(ref ct) = body.contextual_tuples {
        if ct.tuple_keys.len() > crate::validation::MAX_CONTEXTUAL_TUPLES {
            return Err(ApiError::validation_error(format!(
                "too many contextual tuples: {} exceeds maximum of {}",
                ct.tuple_keys.len(),
                crate::validation::MAX_CONTEXTUAL_TUPLES
            )));
        }
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

    // Precomputed check: try Valkey first for sub-millisecond response.
    // Skip when contextual_tuples or context are present since the cache key
    // doesn't include them and the result could be different.
    #[cfg(feature = "precompute")]
    {
        let has_contextual_tuples = !check_request.contextual_tuples.is_empty();
        let has_context = !check_request.context.is_empty();

        if !has_contextual_tuples && !has_context {
            if let Some(ref precompute_cache) = state.precompute_cache {
                // Get the model ID for cache key construction
                let model_id_for_cache = if let Some(ref mid) = check_request.authorization_model_id
                {
                    Some(mid.clone())
                } else {
                    state
                        .storage
                        .get_latest_authorization_model(&check_request.store_id)
                        .await
                        .ok()
                        .map(|m| m.id)
                };

                if let Some(model_id) = model_id_for_cache {
                    // Parse object into type:id (validates non-empty parts)
                    if let Some((obj_type, obj_id)) = parse_object(&check_request.object) {
                        let cache_key = rsfga_valkey::CheckKey::new(
                            &check_request.store_id,
                            &model_id,
                            obj_type,
                            obj_id,
                            &check_request.relation,
                            &check_request.user,
                        );

                        if let Some(cached) = precompute_cache.get(&cache_key).await {
                            // Cache hit - return precomputed result
                            return Ok(Json(CheckResponseBody {
                                allowed: cached.allowed,
                                resolution: None,
                            }));
                        }

                        // Cache miss - record in hot-path registry for future precomputation
                        let _ = precompute_cache
                            .record_hotpath(
                                &check_request.store_id,
                                obj_type,
                                obj_id,
                                &check_request.relation,
                                &check_request.user,
                            )
                            .await;
                    }
                }
            }
        }
    }

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
    /// Consistency preferences for RYOW support.
    #[serde(default)]
    pub consistency: Option<ConsistencyPreference>,
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
/// The object field is optional because computedUserset in tupleToUserset
/// doesn't know its target object until the tupleset is resolved.
#[derive(Debug, Serialize)]
pub struct ExpandObjectRelationBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object: Option<String>,
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
                    //
                    // Error handling strategy: Log warning and continue with empty/malformed object.
                    // Rationale:
                    // 1. Malformed leaf.name indicates a bug in the domain resolver, not user input
                    // 2. Failing the entire expand request would be worse than returning partial data
                    // 3. Warning logs allow debugging while maintaining API availability
                    // 4. OpenFGA's behavior with malformed data is not well-documented, so we
                    //    err on the side of returning data rather than erroring
                    //
                    // Note: split('#').next() always returns Some since split returns at least one element
                    let object_part = leaf.name.split('#').next().unwrap_or_default();
                    if object_part.is_empty() && !leaf.name.is_empty() {
                        tracing::warn!(
                            leaf_name = %leaf.name,
                            "Expand leaf.name has empty object part before '#' - possible resolver bug"
                        );
                    }
                    ExpandLeafBody::new_tuple_to_userset(
                        ExpandObjectRelationBody {
                            object: Some(object_part.to_string()),
                            relation: tupleset,
                        },
                        ExpandObjectRelationBody {
                            // computed_userset object is unknown without further resolution
                            // This matches OpenFGA behavior where the target object is not known
                            // until the tupleset is resolved - omit object field
                            object: None,
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
    headers: HeaderMap,
    JsonBadRequest(body): JsonBadRequest<ExpandRequestBody>,
) -> ApiResult<impl IntoResponse> {
    // RYOW: Wait for write ticket if consistency preferences are specified.
    let header_consistency = extract_consistency_header(&headers);
    wait_for_consistency(
        &state,
        body.consistency.as_ref(),
        header_consistency,
        &store_id,
    )
    .await?;

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

// Implement TupleKeyLike for TupleKeyWithoutConditionBody to enable shared validation
impl crate::validation::TupleKeyLike for TupleKeyWithoutConditionBody {
    fn user(&self) -> &str {
        &self.user
    }
    fn object(&self) -> &str {
        &self.object
    }
}

async fn write_tuples<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(store_id): Path<String>,
    JsonBadRequest(body): JsonBadRequest<WriteRequestBody>,
) -> ApiResult<impl IntoResponse> {
    use rsfga_storage::StoredTuple;

    // Validate tuple count before processing (OpenFGA limit: 100 tuples per write)
    let write_count = body.writes.as_ref().map_or(0, |w| w.tuple_keys.len());
    let delete_count = body.deletes.as_ref().map_or(0, |d| d.tuple_keys.len());
    let total_count = write_count + delete_count;
    if let Some(err) = validate_tuple_count(total_count) {
        return Err(ApiError::exceeded_entity_limit(err));
    }

    // Validate user/object ID lengths before processing (OpenFGA limits)
    // Uses shared validation to eliminate DRY violation with async endpoint
    validate_tuple_id_lengths(
        body.writes.as_ref().map(|w| w.tuple_keys.as_slice()),
        body.deletes.as_ref().map(|d| d.tuple_keys.as_slice()),
    )
    .map_err(ApiError::validation_error)?;

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

    // Clone tuple data for NATS event publishing (only when feature enabled)
    #[cfg(feature = "nats")]
    let nats_event_data = if state.has_publisher() {
        Some(convert_tuples_for_nats_event(&writes, &deletes))
    } else {
        None
    };

    state
        .storage
        .write_tuples(&store_id, writes, deletes)
        .await?;

    // Invalidate cache for this store to prevent stale auth decisions.
    // This is a coarse-grained stopgap until fine-grained invalidation is wired.
    state.cache.invalidate_store(&store_id).await;

    // Fire-and-forget event publishing to RSFGA_EVENTS stream (Milestone 2.0.2.3)
    #[cfg(feature = "nats")]
    if let Some((nats_writes, nats_deletes)) = nats_event_data {
        if let Some(publisher) = state.publisher() {
            let publisher = Arc::clone(publisher);
            let store_id_clone = store_id.clone();
            tokio::spawn(async move {
                use rsfga_nats::CommittedEvent;

                // Sequence 0 for sync path - fire-and-forget notification,
                // not used for RYOW consistency (async path handles that)
                let event = CommittedEvent::new(&store_id_clone, 0)
                    .with_writes(nats_writes)
                    .with_deletes(nats_deletes);

                if let Err(e) = publisher.publish_committed_event(&event).await {
                    // Log error but don't fail the request (fire-and-forget)
                    tracing::warn!(
                        store_id = %store_id_clone,
                        error = %e,
                        "Failed to publish committed event to RSFGA_EVENTS (fire-and-forget)"
                    );
                    // Metrics are tracked inside the publisher
                }
            });
        }
    }

    Ok(Json(serde_json::json!({})))
}

// Use shared validation functions from the validation module
use crate::validation::{
    is_valid_condition_name, json_exceeds_max_depth, validate_tuple_count,
    validate_tuple_id_lengths, MAX_CONDITION_CONTEXT_SIZE,
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

/// Converts StoredTuples to NATS event types for sync path event publishing.
///
/// This function is feature-gated to only compile when the `nats` feature is enabled.
/// It converts the internal StoredTuple representation to the NATS TupleOperation/TupleKey types.
#[cfg(feature = "nats")]
fn convert_tuples_for_nats_event(
    writes: &[rsfga_storage::StoredTuple],
    deletes: &[rsfga_storage::StoredTuple],
) -> (Vec<rsfga_nats::TupleOperation>, Vec<rsfga_nats::TupleKey>) {
    let nats_writes: Vec<rsfga_nats::TupleOperation> = writes
        .iter()
        .map(|t| {
            // Format user as "type:id" or "type:id#relation" for usersets
            let user = if let Some(ref user_rel) = t.user_relation {
                format!("{}:{}#{}", t.user_type, t.user_id, user_rel)
            } else {
                format!("{}:{}", t.user_type, t.user_id)
            };

            // Format object as "type:id"
            let object = format!("{}:{}", t.object_type, t.object_id);

            // Create TupleOperation with optional condition
            let mut op = rsfga_nats::TupleOperation::new(user, t.relation.clone(), object);

            if let Some(ref cond_name) = t.condition_name {
                let condition = rsfga_nats::TupleCondition {
                    name: cond_name.clone(),
                    context: t.condition_context.clone().unwrap_or_default(),
                };
                op = op.with_condition(condition);
            }

            op
        })
        .collect();

    let nats_deletes: Vec<rsfga_nats::TupleKey> = deletes
        .iter()
        .map(|t| {
            // Format user as "type:id" or "type:id#relation" for usersets
            let user = if let Some(ref user_rel) = t.user_relation {
                format!("{}:{}#{}", t.user_type, t.user_id, user_rel)
            } else {
                format!("{}:{}", t.user_type, t.user_id)
            };

            // Format object as "type:id"
            let object = format!("{}:{}", t.object_type, t.object_id);

            rsfga_nats::TupleKey::new(user, t.relation.clone(), object)
        })
        .collect();

    (nats_writes, nats_deletes)
}

/// Converts NATS TupleOperations back to StoredTuples for sync fallback writes.
///
/// This is the reverse of `convert_tuples_for_nats_event`. Used when NATS is unavailable
/// and we need to fall back to direct storage writes (WriteMode::Auto).
///
/// Returns an error if any tuple cannot be parsed, rather than silently
/// dropping tuples (which could cause data loss).
#[cfg(feature = "nats")]
fn nats_tuples_to_stored(
    ops: &[rsfga_nats::TupleOperation],
) -> Result<Vec<rsfga_storage::StoredTuple>, ApiError> {
    ops.iter()
        .enumerate()
        .map(|(i, op)| {
            let mut tuple = parse_tuple_fields(&op.key.user, &op.key.relation, &op.key.object)
                .ok_or_else(|| {
                    ApiError::internal_error(format!(
                        "sync fallback: failed to parse tuple at index {i}: \
                             user={}, relation={}, object={}",
                        op.key.user, op.key.relation, op.key.object
                    ))
                })?;

            // Attach condition if present, with validation
            if let Some(ref cond) = op.condition {
                // Validate condition name length and format (same checks as sync path)
                if !crate::validation::is_valid_condition_name(&cond.name) {
                    return Err(ApiError::validation_error(format!(
                        "invalid condition name at index {i}: '{}' \
                         (must be non-empty, <= {} chars, alphanumeric/underscore/dash)",
                        cond.name,
                        crate::validation::MAX_CONDITION_NAME_LENGTH
                    )));
                }
                tuple.condition_name = Some(cond.name.clone());
                if !cond.context.is_empty() {
                    // Validate condition context depth to prevent DoS via deep nesting
                    let context_json = serde_json::to_value(&cond.context).map_err(|e| {
                        ApiError::internal_error(format!(
                            "sync fallback: failed to serialize condition context at index {i}: {e}"
                        ))
                    })?;
                    if crate::validation::json_exceeds_max_depth(&context_json, 0) {
                        return Err(ApiError::validation_error(format!(
                            "condition context at index {i} exceeds maximum nesting depth"
                        )));
                    }
                    tuple.condition_context = Some(cond.context.clone());
                }
            }

            Ok(tuple)
        })
        .collect()
}

/// Converts NATS TupleKeys to StoredTuples for sync fallback deletes.
///
/// Returns an error if any tuple key cannot be parsed, rather than silently
/// dropping it (which could cause partial deletes and data inconsistency).
#[cfg(feature = "nats")]
fn nats_keys_to_stored(
    keys: &[rsfga_nats::TupleKey],
) -> Result<Vec<rsfga_storage::StoredTuple>, ApiError> {
    keys.iter()
        .enumerate()
        .map(|(i, key)| {
            parse_tuple_fields(&key.user, &key.relation, &key.object).ok_or_else(|| {
                ApiError::internal_error(format!(
                    "sync fallback: failed to parse delete key at index {i}: \
                     user={}, relation={}, object={}",
                    key.user, key.relation, key.object
                ))
            })
        })
        .collect()
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

    // OpenFGA returns 204 No Content for successful assertion writes
    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
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
    /// Consistency preferences for RYOW support.
    #[serde(default)]
    pub consistency: Option<ConsistencyPreference>,
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
    headers: HeaderMap,
    JsonBadRequest(body): JsonBadRequest<ListObjectsRequestBody>,
) -> ApiResult<impl IntoResponse> {
    // RYOW: Wait for write ticket if consistency preferences are specified.
    // Validates that the write ticket's store_id matches the request path's store_id
    // to prevent cross-store ticket attacks.
    let header_consistency = extract_consistency_header(&headers);
    wait_for_consistency(
        &state,
        body.consistency.as_ref(),
        header_consistency,
        &store_id,
    )
    .await?;

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

    // Validate contextual tuple count before allocation
    if let Some(ref ct) = body.contextual_tuples {
        if ct.tuple_keys.len() > crate::validation::MAX_CONTEXTUAL_TUPLES {
            return Err(ApiError::validation_error(format!(
                "too many contextual tuples: {} exceeds maximum of {}",
                ct.tuple_keys.len(),
                crate::validation::MAX_CONTEXTUAL_TUPLES
            )));
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
    let mut list_request = ListObjectsRequest::with_context(
        store_id,
        body.user,
        body.relation,
        body.r#type,
        contextual_tuples,
        body.context.unwrap_or_default(),
    );
    list_request.authorization_model_id = body.authorization_model_id;

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
    /// Consistency preferences for RYOW support.
    #[serde(default)]
    pub consistency: Option<ConsistencyPreference>,
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
    headers: HeaderMap,
    JsonBadRequest(body): JsonBadRequest<ListUsersRequestBody>,
) -> ApiResult<impl IntoResponse> {
    // RYOW: Wait for write ticket if consistency preferences are specified.
    let header_consistency = extract_consistency_header(&headers);
    wait_for_consistency(
        &state,
        body.consistency.as_ref(),
        header_consistency,
        &store_id,
    )
    .await?;

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

    // Validate contextual tuple count before allocation
    if let Some(ref ct) = body.contextual_tuples {
        if ct.tuple_keys.len() > crate::validation::MAX_CONTEXTUAL_TUPLES {
            return Err(ApiError::validation_error(format!(
                "too many contextual tuples: {} exceeds maximum of {}",
                ct.tuple_keys.len(),
                crate::validation::MAX_CONTEXTUAL_TUPLES
            )));
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

// ============================================================================
// Async Write Operation (NATS-based)
// ============================================================================

/// Response body for async write operation.
///
/// The async write endpoint publishes the write request to NATS JetStream
/// instead of writing directly to storage. This provides:
/// - Lower latency (publish vs. database write)
/// - Higher throughput (batch processing)
/// - Decoupled storage processing
///
/// The write ticket can be used for read-your-own-writes (RYOW) consistency.
#[cfg(feature = "nats")]
#[derive(Debug, Serialize)]
pub struct AsyncWriteResponseBody {
    /// Unique request ID for this write operation.
    pub request_id: String,
    /// JetStream sequence number for ordering.
    pub sequence: u64,
    /// Write ticket for RYOW consistency.
    /// None when the write was handled via sync fallback.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub write_ticket: Option<WriteTicketBody>,
    /// Whether this write was handled via sync fallback (NATS was unavailable).
    #[serde(default, skip_serializing_if = "is_false")]
    pub fallback: bool,
}

#[cfg(feature = "nats")]
fn is_false(b: &bool) -> bool {
    !b
}

/// Write ticket for read-your-own-writes consistency.
#[cfg(feature = "nats")]
#[derive(Debug, Serialize)]
pub struct WriteTicketBody {
    /// Store ID.
    pub store_id: String,
    /// Expected sequence number after commit.
    pub sequence: u64,
    /// Request ID for correlation.
    pub request_id: String,
    /// ISO 8601 timestamp when the ticket expires.
    pub expires_at: String,
}

/// Async write endpoint that publishes to NATS JetStream.
///
/// This endpoint validates the write request (same as sync path) but instead
/// of writing directly to storage, it publishes a `WriteRequest` event to the
/// RSFGA_WRITES JetStream stream.
///
/// # Advantages
///
/// - **Low latency**: Returns in <5ms (vs. 15-20ms for sync writes)
/// - **High throughput**: JetStream buffers writes for batch processing
/// - **Decoupled**: Storage writes happen asynchronously
///
/// # Consistency
///
/// The response includes a `write_ticket` that can be used to wait for the
/// write to be committed (read-your-own-writes consistency).
///
/// # Errors
///
/// - 400: Invalid tuple format or validation error
/// - 404: Store not found
/// - 503: NATS publisher not configured or unavailable
///
/// # Performance Note
///
/// While this endpoint publishes to NATS for async processing, it still performs
/// synchronous validation (store existence, model fetch, tuple validation) before
/// publishing. This provides the same data integrity guarantees as the sync path.
///
/// For use cases requiring absolute minimal latency:
/// - Consider pre-validating tuples client-side
/// - Model caching (planned for future release) will reduce validation latency
/// - The consumer will still reject invalid tuples, but without client notification
///
/// Trade-off: We chose to maintain validation to prevent invalid data in the queue,
/// which would cause silent failures in the consumer.
#[cfg(feature = "nats")]
async fn async_write_tuples<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(store_id): Path<String>,
    JsonBadRequest(body): JsonBadRequest<WriteRequestBody>,
) -> ApiResult<impl IntoResponse> {
    use rsfga_nats::{TupleKey, TupleOperation, WriteRequest};

    // Check if NATS publisher is configured
    let publisher = state.publisher().ok_or_else(|| {
        ApiError::service_unavailable("async writes not available: NATS not configured")
    })?;

    // Validate tuple count (same as sync path)
    let write_count = body.writes.as_ref().map_or(0, |w| w.tuple_keys.len());
    let delete_count = body.deletes.as_ref().map_or(0, |d| d.tuple_keys.len());
    let total_count = write_count + delete_count;
    if let Some(err) = validate_tuple_count(total_count) {
        return Err(ApiError::exceeded_entity_limit(err));
    }

    // Validate user/object ID lengths (same as sync path)
    // Uses shared validation to eliminate DRY violation
    validate_tuple_id_lengths(
        body.writes.as_ref().map(|w| w.tuple_keys.as_slice()),
        body.deletes.as_ref().map(|d| d.tuple_keys.as_slice()),
    )
    .map_err(ApiError::validation_error)?;

    // Validate store exists
    let _ = state.storage.get_store(&store_id).await?;

    // Get the latest authorization model to validate tuples against
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
            error!(
                "Failed to parse stored authorization model for store {}: {e}",
                store_id
            );
            ApiError::internal_error("failed to parse authorization model")
        })?;

    // Convert and validate write tuples
    let writes: Vec<TupleOperation> = body
        .writes
        .map(|w| {
            w.tuple_keys
                .into_iter()
                .enumerate()
                .map(|(i, tk)| {
                    // Validate tuple format
                    parse_tuple_key_for_validation(&tk).map_err(|e| {
                        ApiError::validation_error(format!(
                            "invalid tuple at index {i}: user={}, object={}, reason={}",
                            e.user, e.object, e.reason
                        ))
                    })?;

                    // Convert to NATS TupleOperation
                    let mut op = TupleOperation::new(&tk.user, &tk.relation, &tk.object);
                    if let Some(cond) = tk.condition {
                        if !cond.name.is_empty() {
                            // Create condition with both name and context
                            let mut condition = rsfga_nats::TupleCondition::new(&cond.name);
                            if let Some(ctx) = cond.context {
                                for (key, value) in ctx {
                                    condition = condition.with_context(key, value);
                                }
                            }
                            op = op.with_condition(condition);
                        }
                    }
                    Ok(op)
                })
                .collect::<Result<Vec<_>, ApiError>>()
        })
        .transpose()?
        .unwrap_or_default();

    // Convert delete tuples
    let deletes: Vec<TupleKey> = body
        .deletes
        .map(|d| {
            d.tuple_keys
                .into_iter()
                .enumerate()
                .map(|(i, tk)| {
                    // Validate tuple format
                    let _ = parse_tuple_fields(&tk.user, &tk.relation, &tk.object).ok_or_else(
                        || {
                            ApiError::validation_error(format!(
                            "invalid tuple at index {i}: user={}, object={}, reason=invalid format",
                            tk.user, tk.object
                        ))
                        },
                    )?;
                    Ok(TupleKey::new(&tk.user, &tk.relation, &tk.object))
                })
                .collect::<Result<Vec<_>, ApiError>>()
        })
        .transpose()?
        .unwrap_or_default();

    // Validate all tuples against the authorization model (same as sync path)
    // Collect validation data first (object_type, relation, condition_name)
    let write_validation_data: Vec<(String, String, Option<String>)> = writes
        .iter()
        .map(|t| {
            let obj_type = t.key.object.split(':').next().unwrap_or("").to_string();
            let relation = t.key.relation.clone();
            let cond_name = t.condition.as_ref().map(|c| c.name.clone());
            (obj_type, relation, cond_name)
        })
        .collect();

    crate::adapters::validate_tuples_batch(
        &model,
        write_validation_data
            .iter()
            .enumerate()
            .map(|(i, (obj_type, relation, cond_name))| {
                (
                    i,
                    obj_type.as_str(),
                    relation.as_str(),
                    cond_name.as_deref(),
                )
            }),
        false,
    )
    .map_err(|e| ApiError::validation_error(e.to_string()))?;

    let delete_validation_data: Vec<(String, String)> = deletes
        .iter()
        .map(|t| {
            let obj_type = t.object.split(':').next().unwrap_or("").to_string();
            let relation = t.relation.clone();
            (obj_type, relation)
        })
        .collect();

    crate::adapters::validate_tuples_batch(
        &model,
        delete_validation_data
            .iter()
            .enumerate()
            .map(|(i, (obj_type, relation))| {
                (i, obj_type.as_str(), relation.as_str(), None::<&str>)
            }),
        true,
    )
    .map_err(|e| ApiError::validation_error(e.to_string()))?;

    // Build the write request
    let mut request = WriteRequest::new(&store_id);
    if let Some(model_id) = body.authorization_model_id {
        request = request.with_model_id(model_id);
    }
    request = request.writes(writes).deletes(deletes);

    // Check write mode BEFORE attempting NATS publish.
    // WriteMode::Direct should go straight to sync storage, never touching NATS.
    use rsfga_nats::config::WriteMode;
    if state.write_mode() == WriteMode::Direct {
        return sync_fallback_write_tuples(&state, &store_id, &request).await;
    }

    // Attempt NATS publish (WriteMode::Nats or WriteMode::Auto)
    match publisher.publish_write_request(&request).await {
        Ok(ticket) => {
            // NATS publish succeeded - return async response with write ticket
            Ok(Json(AsyncWriteResponseBody {
                request_id: request.request_id.clone(),
                sequence: ticket.sequence,
                write_ticket: Some(WriteTicketBody {
                    store_id: ticket.store_id.clone(),
                    sequence: ticket.sequence,
                    request_id: ticket.request_id.clone(),
                    expires_at: ticket.expires_at.to_rfc3339(),
                }),
                fallback: false,
            }))
        }
        Err(nats_err) => {
            if state.write_mode() == WriteMode::Auto {
                // Auto-fallback: try direct storage write
                tracing::warn!(
                    store_id = %store_id,
                    error = %nats_err,
                    "NATS publish failed, falling back to sync write"
                );
                sync_fallback_write_tuples(&state, &store_id, &request).await
            } else {
                // WriteMode::Nats: don't fall back, return error
                error!("Failed to publish write request to NATS: {nats_err}");
                Err(ApiError::service_unavailable(format!(
                    "failed to publish write request: {nats_err}"
                )))
            }
        }
    }
}

/// Shared sync fallback helper for tuple writes.
///
/// Converts NATS tuple operations to StoredTuples and writes them directly
/// to storage. Used by WriteMode::Direct and WriteMode::Auto fallback.
#[cfg(feature = "nats")]
async fn sync_fallback_write_tuples<S: DataStore>(
    state: &AppState<S>,
    store_id: &str,
    request: &rsfga_nats::WriteRequest,
) -> ApiResult<Json<AsyncWriteResponseBody>> {
    metrics::counter!("rsfga_api_write_fallback_total").increment(1);

    // Convert NATS tuples to StoredTuples for the sync path
    let stored_writes = nats_tuples_to_stored(&request.writes)?;
    let stored_deletes = nats_keys_to_stored(&request.deletes)?;

    state
        .storage
        .write_tuples(store_id, stored_writes, stored_deletes)
        .await
        .map_err(|e| {
            metrics::counter!("rsfga_api_write_fallback_failure_total").increment(1);
            error!(
                store_id = %store_id,
                error = %e,
                "Sync fallback write failed"
            );
            ApiError::from(e)
        })?;

    // Invalidate cache after successful write
    state.cache.invalidate_store(store_id).await;

    metrics::counter!("rsfga_api_write_fallback_success_total").increment(1);

    Ok(Json(AsyncWriteResponseBody {
        request_id: request.request_id.clone(),
        sequence: 0, // No NATS sequence for sync fallback
        write_ticket: None,
        fallback: true,
    }))
}

/// Helper to validate tuple key format without creating a StoredTuple.
/// Used by async write handler to validate tuples before publishing to NATS.
#[cfg(feature = "nats")]
fn parse_tuple_key_for_validation(tk: &TupleKeyBody) -> Result<(), TupleKeyParseError> {
    // Parse user
    let _ = parse_user(&tk.user).ok_or_else(|| TupleKeyParseError {
        user: tk.user.clone(),
        object: tk.object.clone(),
        reason: "invalid user format",
    })?;

    // Parse object
    let _ = parse_object(&tk.object).ok_or_else(|| TupleKeyParseError {
        user: tk.user.clone(),
        object: tk.object.clone(),
        reason: "invalid object format",
    })?;

    // Validate condition if present
    if let Some(ref cond) = tk.condition {
        if !cond.name.is_empty() && !is_valid_condition_name(&cond.name) {
            return Err(TupleKeyParseError {
                user: tk.user.clone(),
                object: tk.object.clone(),
                reason:
                    "invalid condition name: must be alphanumeric/underscore/hyphen, max 256 chars",
            });
        }
    }

    Ok(())
}

/// Response body for async model write operation.
#[cfg(feature = "nats")]
#[derive(Debug, Serialize)]
pub struct AsyncModelWriteResponseBody {
    /// Generated authorization model ID.
    pub authorization_model_id: String,
    /// Unique request ID for this write operation.
    pub request_id: String,
    /// JetStream sequence number for ordering.
    pub sequence: u64,
    /// Write ticket for RYOW consistency.
    /// None when the write was handled via sync fallback.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub write_ticket: Option<WriteTicketBody>,
    /// Whether this write was handled via sync fallback (NATS was unavailable).
    #[serde(default, skip_serializing_if = "is_false")]
    pub fallback: bool,
}

/// Async model write endpoint that publishes to NATS JetStream.
///
/// This endpoint validates the authorization model (same as sync path) but
/// instead of writing directly to storage, it publishes a `ModelWriteRequest`
/// event to the RSFGA_WRITES JetStream stream.
///
/// # Note
///
/// The model ID is generated upfront and included in the response. The actual
/// storage write happens asynchronously via the storage consumer.
///
/// # Errors
///
/// - 400: Invalid model format or validation error
/// - 404: Store not found
/// - 503: NATS publisher not configured or unavailable
#[cfg(feature = "nats")]
async fn async_write_authorization_model<S: DataStore>(
    State(state): State<Arc<AppState<S>>>,
    Path(store_id): Path<String>,
    JsonBadRequest(body): JsonBadRequest<WriteAuthorizationModelRequest>,
) -> ApiResult<impl IntoResponse> {
    use rsfga_nats::ModelWriteRequest;

    // Check if NATS publisher is configured
    let publisher = state.publisher().ok_or_else(|| {
        ApiError::service_unavailable("async writes not available: NATS not configured")
    })?;

    // Validate store exists
    let _ = state.storage.get_store(&store_id).await?;

    // Validate type_definitions is not empty (OpenFGA requirement)
    if body.type_definitions.is_empty() {
        return Err(ApiError::type_definitions_too_few_items(
            "type_definitions requires at least 1 item",
        ));
    }

    // Serialize the model data to JSON for validation
    let mut model_json = serde_json::json!({
        "type_definitions": body.type_definitions,
    });
    if let Some(ref conditions) = body.conditions {
        if !conditions.is_null() {
            model_json["conditions"] = conditions.clone();
        }
    }

    // Validate model semantics (same as sync path)
    crate::adapters::validate_authorization_model_json(&model_json, &body.schema_version)
        .map_err(|e| ApiError::validation_error(e.to_string()))?;

    // Validate model size
    let model_json_str = model_json.to_string();
    if model_json_str.len() > MAX_AUTHORIZATION_MODEL_SIZE {
        return Err(ApiError::validation_error(format!(
            "authorization model exceeds maximum size of {MAX_AUTHORIZATION_MODEL_SIZE} bytes"
        )));
    }

    // Generate a new ULID for the model (generated upfront for RYOW)
    let model_id = ulid::Ulid::new().to_string();

    // Build the model write request with pre-generated model_id for RYOW consistency
    let mut request = ModelWriteRequest::new(&store_id, &body.schema_version)
        .with_model_id(&model_id)
        .with_type_definitions(serde_json::json!(body.type_definitions));

    if let Some(ref conditions) = body.conditions {
        if !conditions.is_null() {
            request = request.with_conditions(conditions.clone());
        }
    }

    // Check write mode BEFORE attempting NATS publish.
    // WriteMode::Direct should go straight to sync storage, never touching NATS.
    use rsfga_nats::config::WriteMode;
    if state.write_mode() == WriteMode::Direct {
        return sync_fallback_write_model(&state, &store_id, &model_id, &model_json_str, &request)
            .await;
    }

    // Attempt NATS publish (WriteMode::Nats or WriteMode::Auto)
    match publisher.publish_model_write_request(&request).await {
        Ok(ticket) => {
            // NATS publish succeeded - return async response with write ticket
            Ok((
                StatusCode::CREATED,
                Json(AsyncModelWriteResponseBody {
                    authorization_model_id: model_id,
                    request_id: request.request_id.clone(),
                    sequence: ticket.sequence,
                    write_ticket: Some(WriteTicketBody {
                        store_id: ticket.store_id.clone(),
                        sequence: ticket.sequence,
                        request_id: ticket.request_id.clone(),
                        expires_at: ticket.expires_at.to_rfc3339(),
                    }),
                    fallback: false,
                }),
            ))
        }
        Err(nats_err) => {
            if state.write_mode() == WriteMode::Auto {
                // Auto-fallback: write model directly to storage
                tracing::warn!(
                    store_id = %store_id,
                    error = %nats_err,
                    "NATS model publish failed, falling back to sync write"
                );
                sync_fallback_write_model(&state, &store_id, &model_id, &model_json_str, &request)
                    .await
            } else {
                // WriteMode::Nats: don't fall back, return error
                error!("Failed to publish model write request to NATS: {nats_err}");
                Err(ApiError::service_unavailable(format!(
                    "failed to publish model write request: {nats_err}"
                )))
            }
        }
    }
}

/// Shared sync fallback helper for model writes.
///
/// Writes the authorization model directly to storage, bypassing NATS.
/// Used by WriteMode::Direct and WriteMode::Auto fallback.
#[cfg(feature = "nats")]
async fn sync_fallback_write_model<S: DataStore>(
    state: &AppState<S>,
    store_id: &str,
    model_id: &str,
    model_json_str: &str,
    request: &rsfga_nats::ModelWriteRequest,
) -> ApiResult<(StatusCode, Json<AsyncModelWriteResponseBody>)> {
    metrics::counter!("rsfga_api_model_write_fallback_total").increment(1);

    let model = StoredAuthorizationModel::new(
        model_id,
        store_id,
        &request.schema_version,
        model_json_str.to_string(),
    );

    // Invalidate caches BEFORE writing to prevent stale reads (same as sync path)
    global_cache().invalidate_all();
    state.cache.invalidate_store(store_id).await;

    state
        .storage
        .write_authorization_model(model)
        .await
        .map_err(|e| {
            metrics::counter!("rsfga_api_model_write_fallback_failure_total").increment(1);
            error!(
                store_id = %store_id,
                error = %e,
                "Sync fallback model write failed"
            );
            ApiError::from(e)
        })?;

    metrics::counter!("rsfga_api_model_write_fallback_success_total").increment(1);

    Ok((
        StatusCode::CREATED,
        Json(AsyncModelWriteResponseBody {
            authorization_model_id: model_id.to_string(),
            request_id: request.request_id.clone(),
            sequence: 0,
            write_ticket: None,
            fallback: true,
        }),
    ))
}

// ============================================================================
// Unit Tests (Issue #282 - Expand API response format)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rsfga_domain::resolver::{ExpandLeaf, ExpandLeafValue, ExpandNode};

    /// Test: expand_node_to_body correctly formats TupleToUserset with ObjectRelation
    ///
    /// Verifies that the expand response uses the correct structure with object and relation fields.
    #[test]
    fn test_expand_node_to_body_tuple_to_userset_format() {
        let node = ExpandNode::Leaf(ExpandLeaf {
            name: "document:doc1#viewer".to_string(),
            value: ExpandLeafValue::TupleToUserset {
                tupleset: "parent".to_string(),
                computed_userset: "viewer".to_string(),
            },
        });

        let body = expand_node_to_body(node);

        // Verify the structure
        assert!(body.leaf.is_some());
        let leaf = body.leaf.unwrap();
        assert!(leaf.tuple_to_userset.is_some());

        let ttu = leaf.tuple_to_userset.unwrap();

        // Tupleset should have object extracted from leaf.name
        assert_eq!(ttu.tupleset.object, Some("document:doc1".to_string()));
        assert_eq!(ttu.tupleset.relation, "parent");

        // Computed userset object is unknown, should be None (omitted in JSON)
        assert_eq!(ttu.computed_userset.object, None);
        assert_eq!(ttu.computed_userset.relation, "viewer");
    }

    /// Test: expand_node_to_body handles empty leaf.name gracefully
    ///
    /// Verifies that malformed leaf.name (empty) doesn't cause panic.
    #[test]
    fn test_expand_node_to_body_empty_leaf_name() {
        let node = ExpandNode::Leaf(ExpandLeaf {
            name: String::new(), // Empty name
            value: ExpandLeafValue::TupleToUserset {
                tupleset: "parent".to_string(),
                computed_userset: "viewer".to_string(),
            },
        });

        let body = expand_node_to_body(node);

        let leaf = body.leaf.unwrap();
        let ttu = leaf.tuple_to_userset.unwrap();

        // Should handle empty name gracefully (empty becomes Some(""))
        assert_eq!(ttu.tupleset.object, Some(String::new()));
        assert_eq!(ttu.tupleset.relation, "parent");
    }

    /// Test: expand_node_to_body handles leaf.name without hash gracefully
    ///
    /// Verifies that leaf.name without '#' separator is handled correctly.
    #[test]
    fn test_expand_node_to_body_name_without_hash() {
        let node = ExpandNode::Leaf(ExpandLeaf {
            name: "document:doc1".to_string(), // No hash separator
            value: ExpandLeafValue::TupleToUserset {
                tupleset: "parent".to_string(),
                computed_userset: "viewer".to_string(),
            },
        });

        let body = expand_node_to_body(node);

        let leaf = body.leaf.unwrap();
        let ttu = leaf.tuple_to_userset.unwrap();

        // Should use the entire name as object when no hash present
        assert_eq!(ttu.tupleset.object, Some("document:doc1".to_string()));
        assert_eq!(ttu.tupleset.relation, "parent");
    }

    /// Test: ExpandTupleToUsersetBody serializes with correct camelCase field names
    ///
    /// Verifies that JSON serialization uses 'tupleToUserset' and 'computedUserset'.
    #[test]
    fn test_expand_tuple_to_userset_serializes_camel_case() {
        let leaf = ExpandLeafBody {
            users: None,
            computed: None,
            tuple_to_userset: Some(ExpandTupleToUsersetBody {
                tupleset: ExpandObjectRelationBody {
                    object: Some("document:doc1".to_string()),
                    relation: "parent".to_string(),
                },
                computed_userset: ExpandObjectRelationBody {
                    object: None,
                    relation: "viewer".to_string(),
                },
            }),
        };

        let json = serde_json::to_string(&leaf).unwrap();

        // Verify camelCase field names
        assert!(
            json.contains("tupleToUserset"),
            "Should serialize as tupleToUserset, got: {}",
            json
        );
        assert!(
            json.contains("computedUserset"),
            "Should serialize as computedUserset, got: {}",
            json
        );

        // Verify ObjectRelation structure
        assert!(
            json.contains(r#""object":"document:doc1""#),
            "Should include object field"
        );
        assert!(
            json.contains(r#""relation":"parent""#),
            "Should include relation field"
        );
    }

    /// Test: ExpandLeafBody with users serializes correctly
    #[test]
    fn test_expand_leaf_users_format() {
        let node = ExpandNode::Leaf(ExpandLeaf {
            name: "document:doc1#viewer".to_string(),
            value: ExpandLeafValue::Users(vec!["user:alice".to_string(), "user:bob".to_string()]),
        });

        let body = expand_node_to_body(node);

        assert!(body.leaf.is_some());
        let leaf = body.leaf.unwrap();
        assert!(leaf.users.is_some());
        assert!(leaf.tuple_to_userset.is_none());

        let users = leaf.users.unwrap();
        assert_eq!(users.users.len(), 2);
        assert!(users.users.contains(&"user:alice".to_string()));
        assert!(users.users.contains(&"user:bob".to_string()));
    }

    /// Test: ExpandLeafBody with computed userset serializes correctly
    #[test]
    fn test_expand_leaf_computed_format() {
        let node = ExpandNode::Leaf(ExpandLeaf {
            name: "document:doc1#viewer".to_string(),
            value: ExpandLeafValue::Computed {
                userset: "document:doc1#editor".to_string(),
            },
        });

        let body = expand_node_to_body(node);

        assert!(body.leaf.is_some());
        let leaf = body.leaf.unwrap();
        assert!(leaf.computed.is_some());
        assert!(leaf.tuple_to_userset.is_none());

        let computed = leaf.computed.unwrap();
        assert_eq!(computed.userset, "document:doc1#editor");
    }

    /// Test: Union node serializes correctly
    #[test]
    fn test_expand_union_node_format() {
        let node = ExpandNode::Union {
            name: "document:doc1#viewer".to_string(),
            nodes: vec![
                ExpandNode::Leaf(ExpandLeaf {
                    name: "document:doc1#viewer".to_string(),
                    value: ExpandLeafValue::Users(vec!["user:alice".to_string()]),
                }),
                ExpandNode::Leaf(ExpandLeaf {
                    name: "document:doc1#editor".to_string(),
                    value: ExpandLeafValue::Users(vec!["user:bob".to_string()]),
                }),
            ],
        };

        let body = expand_node_to_body(node);

        assert!(body.union.is_some());
        assert_eq!(body.name.as_deref(), Some("document:doc1#viewer"));

        let union = body.union.unwrap();
        assert_eq!(union.nodes.len(), 2);
    }

    /// Test: Malformed leaf.name starting with '#' serializes with empty object
    ///
    /// Edge case: When leaf.name has format "#relation" (missing type:id),
    /// the object field should be Some("") (empty string), not None.
    #[test]
    fn test_expand_malformed_leaf_name_serialization() {
        let leaf = ExpandLeafBody {
            users: None,
            computed: None,
            tuple_to_userset: Some(ExpandTupleToUsersetBody {
                tupleset: ExpandObjectRelationBody {
                    object: Some(String::new()), // Empty from malformed "#relation" leaf name
                    relation: "parent".to_string(),
                },
                computed_userset: ExpandObjectRelationBody {
                    object: None,
                    relation: "viewer".to_string(),
                },
            }),
        };

        let json = serde_json::to_string(&leaf).unwrap();

        // Verify tupleset still has object field (even if empty)
        assert!(
            json.contains(r#""tupleset":{"object":"","relation":"parent"}"#),
            "Malformed tupleset should serialize with empty object, got: {}",
            json
        );

        // Verify computedUserset omits object field entirely (Option::None)
        assert!(
            json.contains(r#""computedUserset":{"relation":"viewer"}"#),
            "computedUserset should omit object field, got: {}",
            json
        );
    }

    // ====================================================================
    // Sync fallback conversion tests (Milestone 2.0.5)
    // ====================================================================

    #[cfg(feature = "nats")]
    mod nats_fallback_tests {
        use super::*;
        use rsfga_nats::{TupleCondition, TupleKey, TupleOperation};

        #[test]
        fn test_nats_tuples_to_stored_basic() {
            let ops = vec![TupleOperation {
                key: TupleKey::new("user:alice", "viewer", "document:readme"),
                condition: None,
            }];

            let stored = nats_tuples_to_stored(&ops).unwrap();
            assert_eq!(stored.len(), 1);
            assert_eq!(stored[0].user_type, "user");
            assert_eq!(stored[0].user_id, "alice");
            assert_eq!(stored[0].relation, "viewer");
            assert_eq!(stored[0].object_type, "document");
            assert_eq!(stored[0].object_id, "readme");
            assert!(stored[0].condition_name.is_none());
            assert!(stored[0].condition_context.is_none());
        }

        #[test]
        fn test_nats_tuples_to_stored_with_condition() {
            let ops = vec![TupleOperation {
                key: TupleKey::new("user:bob", "editor", "folder:docs"),
                condition: Some(
                    TupleCondition::new("time_bound")
                        .with_context("start_time", serde_json::json!("2024-01-01T00:00:00Z")),
                ),
            }];

            let stored = nats_tuples_to_stored(&ops).unwrap();
            assert_eq!(stored.len(), 1);
            assert_eq!(stored[0].condition_name.as_deref(), Some("time_bound"));
            assert!(stored[0].condition_context.is_some());
            let ctx = stored[0].condition_context.as_ref().unwrap();
            assert!(ctx.contains_key("start_time"));
        }

        #[test]
        fn test_nats_tuples_to_stored_with_empty_condition_context() {
            let ops = vec![TupleOperation {
                key: TupleKey::new("user:carol", "viewer", "document:report"),
                condition: Some(TupleCondition::new("always_true")),
            }];

            let stored = nats_tuples_to_stored(&ops).unwrap();
            assert_eq!(stored.len(), 1);
            assert_eq!(stored[0].condition_name.as_deref(), Some("always_true"));
            // Empty context should not be set
            assert!(stored[0].condition_context.is_none());
        }

        #[test]
        fn test_nats_tuples_to_stored_with_userset() {
            let ops = vec![TupleOperation {
                key: TupleKey::new("team:engineering#member", "viewer", "document:readme"),
                condition: None,
            }];

            let stored = nats_tuples_to_stored(&ops).unwrap();
            assert_eq!(stored.len(), 1);
            assert_eq!(stored[0].user_type, "team");
            assert_eq!(stored[0].user_id, "engineering");
            assert_eq!(stored[0].user_relation, Some("member".to_string()));
        }

        #[test]
        fn test_nats_tuples_to_stored_returns_error_on_invalid_tuple() {
            let ops = vec![TupleOperation {
                key: TupleKey::new("", "viewer", "document:readme"), // invalid: empty user
                condition: None,
            }];

            let result = nats_tuples_to_stored(&ops);
            assert!(result.is_err());
        }

        #[test]
        fn test_nats_keys_to_stored() {
            let keys = vec![
                TupleKey::new("user:alice", "viewer", "document:readme"),
                TupleKey::new("user:bob", "editor", "folder:docs"),
            ];

            let stored = nats_keys_to_stored(&keys).unwrap();
            assert_eq!(stored.len(), 2);
            assert_eq!(stored[0].user_id, "alice");
            assert_eq!(stored[1].user_id, "bob");
        }

        #[test]
        fn test_nats_keys_to_stored_returns_error_on_invalid_key() {
            let keys = vec![TupleKey::new("user:alice", "viewer", "invalid")]; // no colon

            let result = nats_keys_to_stored(&keys);
            assert!(result.is_err());
        }

        #[test]
        fn test_async_write_response_body_serialization_normal() {
            let body = AsyncWriteResponseBody {
                request_id: "req-1".to_string(),
                sequence: 42,
                write_ticket: Some(WriteTicketBody {
                    store_id: "store1".to_string(),
                    sequence: 42,
                    request_id: "req-1".to_string(),
                    expires_at: "2024-01-01T00:00:00Z".to_string(),
                }),
                fallback: false,
            };

            let json = serde_json::to_value(&body).unwrap();
            assert_eq!(json["request_id"], "req-1");
            assert_eq!(json["sequence"], 42);
            assert!(json["write_ticket"].is_object());
            // fallback=false should be skipped
            assert!(json.get("fallback").is_none());
        }

        #[test]
        fn test_async_write_response_body_serialization_fallback() {
            let body = AsyncWriteResponseBody {
                request_id: "req-2".to_string(),
                sequence: 0,
                write_ticket: None,
                fallback: true,
            };

            let json = serde_json::to_value(&body).unwrap();
            assert_eq!(json["request_id"], "req-2");
            assert_eq!(json["sequence"], 0);
            // write_ticket should be omitted when None
            assert!(json.get("write_ticket").is_none());
            // fallback=true should be present
            assert_eq!(json["fallback"], true);
        }

        #[test]
        fn test_async_model_write_response_body_serialization_normal() {
            let body = AsyncModelWriteResponseBody {
                authorization_model_id: "model-1".to_string(),
                request_id: "req-1".to_string(),
                sequence: 10,
                write_ticket: Some(WriteTicketBody {
                    store_id: "store1".to_string(),
                    sequence: 10,
                    request_id: "req-1".to_string(),
                    expires_at: "2024-01-01T00:00:00Z".to_string(),
                }),
                fallback: false,
            };

            let json = serde_json::to_value(&body).unwrap();
            assert_eq!(json["authorization_model_id"], "model-1");
            assert!(json["write_ticket"].is_object());
            assert!(json.get("fallback").is_none());
        }

        #[test]
        fn test_async_model_write_response_body_serialization_fallback() {
            let body = AsyncModelWriteResponseBody {
                authorization_model_id: "model-2".to_string(),
                request_id: "req-3".to_string(),
                sequence: 0,
                write_ticket: None,
                fallback: true,
            };

            let json = serde_json::to_value(&body).unwrap();
            assert_eq!(json["authorization_model_id"], "model-2");
            assert!(json.get("write_ticket").is_none());
            assert_eq!(json["fallback"], true);
        }
    }

    // ── X-Consistency Header Tests ──────────────────────────────────────

    #[test]
    fn test_extract_consistency_header_eventual() {
        let mut headers = HeaderMap::new();
        headers.insert("x-consistency", "eventual".parse().unwrap());
        assert_eq!(
            extract_consistency_header(&headers),
            Some(ConsistencyLevel::Eventual)
        );
    }

    #[test]
    fn test_extract_consistency_header_strong() {
        let mut headers = HeaderMap::new();
        headers.insert("x-consistency", "strong".parse().unwrap());
        assert_eq!(
            extract_consistency_header(&headers),
            Some(ConsistencyLevel::Strong)
        );
    }

    #[test]
    fn test_extract_consistency_header_case_insensitive() {
        for value in [
            "EVENTUAL", "Eventual", "STRONG", "Strong", "EvEnTuAl", "sTrOnG",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert("x-consistency", value.parse().unwrap());
            let result = extract_consistency_header(&headers);
            assert!(
                result.is_some(),
                "Expected Some for value '{}', got None",
                value
            );
        }
    }

    #[test]
    fn test_extract_consistency_header_unknown_value() {
        let mut headers = HeaderMap::new();
        headers.insert("x-consistency", "weak".parse().unwrap());
        assert_eq!(extract_consistency_header(&headers), None);
    }

    #[test]
    fn test_extract_consistency_header_absent() {
        let headers = HeaderMap::new();
        assert_eq!(extract_consistency_header(&headers), None);
    }

    #[test]
    fn test_extract_consistency_header_empty_value() {
        let mut headers = HeaderMap::new();
        headers.insert("x-consistency", "".parse().unwrap());
        assert_eq!(extract_consistency_header(&headers), None);
    }

    #[tokio::test]
    async fn test_wait_for_consistency_no_tracker_is_noop() {
        // Without NATS feature or without a tracker, all header values are no-ops
        let storage = std::sync::Arc::new(rsfga_storage::MemoryDataStore::new());
        let state = AppState::new(storage);

        // Strong header should not error when no tracker is configured
        let result =
            wait_for_consistency(&state, None, Some(ConsistencyLevel::Strong), "store-1").await;
        assert!(result.is_ok());

        // Eventual header is always a no-op
        let result =
            wait_for_consistency(&state, None, Some(ConsistencyLevel::Eventual), "store-1").await;
        assert!(result.is_ok());

        // No header is also fine
        let result = wait_for_consistency(&state, None, None, "store-1").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_wait_for_consistency_body_takes_precedence_over_header() {
        // When body-level consistency has a write_ticket, header is ignored.
        // We test this by providing an Eventual header (which would skip waits)
        // alongside a body-level write_ticket with a mismatched store_id (which should error).
        let storage = std::sync::Arc::new(rsfga_storage::MemoryDataStore::new());
        let state = AppState::new(storage);

        let pref = ConsistencyPreference {
            minimize_latency: false,
            write_ticket: Some(WriteTicketParam {
                store_id: "store-wrong".to_string(),
                sequence: 1,
            }),
        };

        // Even with Eventual header, body takes precedence → store_id mismatch error
        // Note: Without nats feature, this is a no-op so the error won't fire.
        // With nats feature, the body-level validation runs first.
        let _result = wait_for_consistency(
            &state,
            Some(&pref),
            Some(ConsistencyLevel::Eventual),
            "store-1",
        )
        .await;
        // Just verify it doesn't panic. The exact behavior depends on the nats feature.
    }

    #[cfg(feature = "nats")]
    #[tokio::test]
    async fn test_wait_for_consistency_body_precedence_over_header_with_nats() {
        // With nats feature, body write_ticket with mismatched store_id should error
        // even when header says Eventual
        let storage = std::sync::Arc::new(rsfga_storage::MemoryDataStore::new());
        let state = AppState::new(storage);

        let pref = ConsistencyPreference {
            minimize_latency: false,
            write_ticket: Some(WriteTicketParam {
                store_id: "store-wrong".to_string(),
                sequence: 1,
            }),
        };

        let result = wait_for_consistency(
            &state,
            Some(&pref),
            Some(ConsistencyLevel::Eventual),
            "store-1",
        )
        .await;
        assert!(result.is_err(), "Expected error for store_id mismatch");
    }

    #[tokio::test]
    async fn test_wait_for_consistency_minimize_latency_suppresses_strong_header() {
        // When body sets minimize_latency=true (no write_ticket),
        // header-level strong consistency should be suppressed.
        let storage = std::sync::Arc::new(rsfga_storage::MemoryDataStore::new());
        let state = AppState::new(storage);

        let pref = ConsistencyPreference {
            minimize_latency: true,
            write_ticket: None,
        };

        let result = wait_for_consistency(
            &state,
            Some(&pref),
            Some(ConsistencyLevel::Strong),
            "store-1",
        )
        .await;
        assert!(
            result.is_ok(),
            "minimize_latency should suppress header-level strong consistency"
        );
    }

    #[cfg(feature = "nats")]
    #[tokio::test]
    async fn test_wait_for_consistency_strong_header_no_tracker_is_noop() {
        // Strong header without a configured tracker should be a no-op
        let storage = std::sync::Arc::new(rsfga_storage::MemoryDataStore::new());
        let state = AppState::new(storage);

        let result =
            wait_for_consistency(&state, None, Some(ConsistencyLevel::Strong), "store-1").await;
        assert!(result.is_ok());
    }
}
