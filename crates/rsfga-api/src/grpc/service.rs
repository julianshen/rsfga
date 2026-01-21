//! gRPC service implementation for OpenFGA-compatible API.

use std::sync::Arc;

use dashmap::DashMap;
use tonic::{Request, Response, Status};

use rsfga_domain::cache::{CheckCache, CheckCacheConfig};
use rsfga_domain::cel::global_cache;
use rsfga_domain::error::DomainError;
use rsfga_domain::resolver::{
    CheckRequest as DomainCheckRequest, ContextualTuple, ExpandRequest as DomainExpandRequest,
    GraphResolver, ResolverConfig,
};
use rsfga_server::handlers::batch::BatchCheckHandler;
use rsfga_storage::{
    DataStore, PaginationOptions, StorageError, StoredAuthorizationModel, StoredTuple, TupleFilter,
    Utc,
};

use crate::adapters::{DataStoreModelReader, DataStoreTupleReader};
use crate::grpc::conversion::{
    condition_to_json, datetime_to_timestamp, expand_node_to_proto, hashmap_to_prost_struct,
    prost_struct_to_hashmap, stored_model_to_proto, tuple_key_to_stored, type_definition_to_json,
};
use crate::http::{
    AssertionCondition, AssertionKey, AssertionTupleKey, ContextualTuplesWrapper, StoredAssertion,
};
use crate::utils::{format_user, parse_object, parse_user};

use crate::proto::openfga::v1::{
    open_fga_service_server::OpenFgaService, user, Assertion, AuthorizationModel,
    BatchCheckRequest, BatchCheckResponse, BatchCheckSingleResult, CheckError, CheckRequest,
    CheckResponse, CreateStoreRequest, CreateStoreResponse, DeleteStoreRequest,
    DeleteStoreResponse, ErrorCode, ExpandRequest, ExpandResponse, FgaObject, GetStoreRequest,
    GetStoreResponse, ListObjectsRequest, ListObjectsResponse, ListStoresRequest,
    ListStoresResponse, ListUsersRequest, ListUsersResponse, ReadAssertionsRequest,
    ReadAssertionsResponse, ReadAuthorizationModelRequest, ReadAuthorizationModelResponse,
    ReadAuthorizationModelsRequest, ReadAuthorizationModelsResponse, ReadChangesRequest,
    ReadChangesResponse, ReadRequest, ReadResponse, RelationshipCondition, Store, Tuple,
    TupleChange, TupleKey, TupleOperation, TypedWildcard, UpdateStoreRequest, UpdateStoreResponse,
    User, UsersetTree, UsersetUser, WriteAssertionsRequest, WriteAssertionsResponse,
    WriteAuthorizationModelRequest, WriteAuthorizationModelResponse, WriteRequest, WriteResponse,
};

/// Maximum allowed length for correlation IDs to prevent DoS attacks.
/// OpenFGA uses UUIDs (36 chars), so 256 provides generous headroom.
const MAX_CORRELATION_ID_LENGTH: usize = 256;

/// Maximum number of (store_id, model_id) assertion entries to prevent unbounded memory growth.
/// This limits the total number of unique store/model pairs that can have assertions.
/// Typical production usage: < 100 models with assertions.
const MAX_ASSERTION_ENTRIES: usize = 10_000;

/// Warning threshold for assertion entries (80% of max).
const ASSERTION_ENTRIES_WARNING_THRESHOLD: usize = MAX_ASSERTION_ENTRIES * 80 / 100;

/// gRPC service implementation for OpenFGA.
///
/// This service implements the OpenFGA gRPC API with support for parallel
/// batch checks through the integrated BatchCheckHandler.
///
/// # Cache Safety
///
/// By default, caching is **disabled** for safety. Cached positive authorization
/// decisions can serve stale results after tuple writes/deletes. To enable caching,
/// explicitly set `CheckCacheConfig::enabled` to `true`:
///
/// ```rust,ignore
/// let cache_config = CheckCacheConfig::default().with_enabled(true);
/// let service = OpenFgaGrpcService::with_cache_config(storage, cache_config);
/// ```
///
/// # Memory Usage Characteristics
///
/// Assertions are stored in-memory using a `DashMap`. This design provides:
/// - Fast read/write access for assertions
/// - No database storage overhead
///
/// **Memory Limits:**
/// - Maximum 10,000 unique (store_id, model_id) pairs with assertions
/// - Warning logged at 80% capacity (8,000 entries)
/// - New writes rejected with ResourceExhausted error when limit reached
///
/// **Cleanup Behavior:**
/// - Assertions are cleaned up when stores are deleted
/// - Memory is NOT recovered when individual models are deleted (only store deletion)
/// - WriteAssertions replaces all assertions for a key (not append)
///
/// **Production Considerations:**
/// - Monitor assertion entry count via logs
/// - Plan store lifecycle to ensure cleanup
/// - Typical usage: ~100-1000 models with assertions
///
/// OpenFGA also stores assertions in-memory, so this matches their behavior.
pub struct OpenFgaGrpcService<S: DataStore> {
    storage: Arc<S>,
    /// The graph resolver for single checks.
    resolver: Arc<GraphResolver<DataStoreTupleReader<S>, DataStoreModelReader<S>>>,
    /// The batch check handler with parallel execution and deduplication.
    batch_handler: Arc<BatchCheckHandler<DataStoreTupleReader<S>, DataStoreModelReader<S>>>,
    /// The check result cache, stored for future invalidation in write handlers.
    cache: Arc<CheckCache>,
    /// In-memory assertions storage keyed by (store_id, model_id).
    ///
    /// # Memory Management
    /// - Entries are removed when stores are deleted via `delete_store`
    /// - WriteAssertions replaces all assertions for a key (not append)
    /// - Typical usage: ~100-1000 assertions per model for testing scenarios
    assertions: Arc<DashMap<AssertionKey, Vec<StoredAssertion>>>,
}

impl<S: DataStore> OpenFgaGrpcService<S> {
    /// Creates a new gRPC service instance with default cache configuration.
    ///
    /// Note: Caching is **disabled** by default for safety. Use `with_cache_config`
    /// with an explicitly enabled config to enable caching.
    pub fn new(storage: Arc<S>) -> Self {
        Self::with_cache_config(storage, CheckCacheConfig::default())
    }

    /// Creates a new gRPC service instance with custom cache configuration.
    ///
    /// # Cache Safety
    ///
    /// If `cache_config.enabled` is `false` (the default), the cache will be created
    /// but NOT attached to the resolver. This means authorization checks will always
    /// hit storage, ensuring fresh results.
    ///
    /// If `cache_config.enabled` is `true`, the cache will be attached to the resolver.
    /// This improves performance but cached results may be stale until TTL expires
    /// or invalidation occurs.
    pub fn with_cache_config(storage: Arc<S>, cache_config: CheckCacheConfig) -> Self {
        // Create adapters to bridge storage to domain traits
        let tuple_reader = Arc::new(DataStoreTupleReader::new(Arc::clone(&storage)));
        let model_reader = Arc::new(DataStoreModelReader::new(Arc::clone(&storage)));

        // Create the check cache
        let cache = Arc::new(CheckCache::new(cache_config.clone()));

        // Create the graph resolver - only attach cache if explicitly enabled
        let resolver_config = if cache_config.enabled {
            ResolverConfig::default().with_cache(Arc::clone(&cache))
        } else {
            ResolverConfig::default()
        };
        let resolver = Arc::new(GraphResolver::with_config(
            Arc::clone(&tuple_reader),
            Arc::clone(&model_reader),
            resolver_config,
        ));

        // Create the batch handler
        let batch_handler = Arc::new(BatchCheckHandler::new(
            Arc::clone(&resolver),
            Arc::clone(&cache),
        ));

        Self {
            storage,
            resolver,
            batch_handler,
            cache,
            assertions: Arc::new(DashMap::new()),
        }
    }

    /// Returns a reference to the check cache for invalidation.
    pub fn cache(&self) -> &Arc<CheckCache> {
        &self.cache
    }
}

/// Converts a StorageError to a tonic Status.
fn storage_error_to_status(err: StorageError) -> Status {
    match &err {
        StorageError::StoreNotFound { store_id } => {
            Status::not_found(format!("store not found: {store_id}"))
        }
        StorageError::ModelNotFound { model_id } => {
            Status::not_found(format!("authorization model not found: {model_id}"))
        }
        StorageError::TupleNotFound { .. } => Status::not_found(err.to_string()),
        StorageError::InvalidInput { message } => Status::invalid_argument(message),
        // ALREADY_EXISTS (6): duplicate tuple or condition conflict
        StorageError::DuplicateTuple { .. } => Status::already_exists(err.to_string()),
        StorageError::ConditionConflict(conflict) => {
            Status::already_exists(format!("tuple exists with different condition: {conflict}"))
        }
        // UNAVAILABLE (14): connection errors, health check failures
        StorageError::ConnectionError { message } => {
            tracing::error!("Storage connection error: {}", message);
            Status::unavailable("storage backend unavailable")
        }
        StorageError::HealthCheckFailed { message } => {
            tracing::error!("Storage health check failed: {}", message);
            Status::unavailable("storage backend unavailable")
        }
        // DEADLINE_EXCEEDED (4): query timeout
        StorageError::QueryTimeout { .. } => {
            tracing::error!("Storage query timeout: {}", err);
            Status::deadline_exceeded("storage operation timed out")
        }
        _ => Status::internal(err.to_string()),
    }
}

/// Converts a DomainError to a tonic Status.
fn domain_error_to_status(err: DomainError) -> Status {
    use crate::errors::{classify_domain_error, DomainErrorKind};

    match classify_domain_error(&err) {
        DomainErrorKind::InvalidInput(msg) => Status::invalid_argument(msg),
        DomainErrorKind::NotFound(msg) => Status::not_found(msg),
        DomainErrorKind::Timeout(msg) => {
            tracing::error!("{}", msg);
            Status::deadline_exceeded("authorization check timeout")
        }
        DomainErrorKind::Internal(msg) => {
            tracing::error!("Domain error: {}", msg);
            Status::internal("internal error during authorization check")
        }
    }
}

/// Converts a BatchCheckError to a tonic Status.
///
/// Logs internal errors with structured context to aid debugging while
/// returning sanitized error messages to clients.
fn batch_check_error_to_status(err: rsfga_server::handlers::batch::BatchCheckError) -> Status {
    use rsfga_server::handlers::batch::BatchCheckError;
    match err {
        BatchCheckError::EmptyBatch => Status::invalid_argument("batch request cannot be empty"),
        BatchCheckError::BatchTooLarge { size, max } => {
            Status::invalid_argument(format!("batch size {size} exceeds maximum allowed {max}"))
        }
        BatchCheckError::InvalidCheck { index, message } => {
            Status::invalid_argument(format!("invalid check at index {index}: {message}"))
        }
        BatchCheckError::DomainError(msg) => {
            // Log full error details for debugging - DO NOT expose to clients
            tracing::error!(error = %msg, "Domain error in gRPC batch check");
            // Return sanitized message to prevent information leakage
            Status::internal("internal error during authorization check")
        }
    }
}

/// Maximum size for authorization model JSON (1MB).
/// This matches the OpenFGA default limit for model size.
const MAX_AUTHORIZATION_MODEL_SIZE: usize = 1024 * 1024;

/// Default and maximum page size for listing authorization models.
/// Matches OpenFGA's default behavior to limit memory usage and response size.
const DEFAULT_PAGE_SIZE: i32 = 50;

#[tonic::async_trait]
impl<S: DataStore> OpenFgaService for OpenFgaGrpcService<S> {
    async fn check(
        &self,
        request: Request<CheckRequest>,
    ) -> Result<Response<CheckResponse>, Status> {
        let req = request.into_inner();

        let tuple_key = req
            .tuple_key
            .ok_or_else(|| Status::invalid_argument("tuple_key is required"))?;

        // Convert contextual tuples from gRPC format to domain format
        let contextual_tuples: Vec<ContextualTuple> = req
            .contextual_tuples
            .map(|ct| {
                ct.tuple_keys
                    .into_iter()
                    .map(|tk| {
                        if let Some(condition) = tk.condition {
                            let context = condition
                                .context
                                .map(prost_struct_to_hashmap)
                                .transpose()
                                .map_err(|e| {
                                    Status::invalid_argument(format!(
                                        "invalid condition context for tuple '{}#{}@{}': {}",
                                        tk.object, tk.relation, tk.user, e
                                    ))
                                })?;

                            Ok(ContextualTuple::with_condition(
                                &tk.user,
                                &tk.relation,
                                &tk.object,
                                &condition.name,
                                context,
                            ))
                        } else {
                            Ok(ContextualTuple::new(&tk.user, &tk.relation, &tk.object))
                        }
                    })
                    .collect::<Result<Vec<_>, Status>>()
            })
            .transpose()?
            .unwrap_or_default();

        // Create domain check request
        let check_request = DomainCheckRequest::new(
            req.store_id,
            tuple_key.user,
            tuple_key.relation,
            tuple_key.object,
            contextual_tuples,
        );

        // Delegate to GraphResolver for full graph traversal
        let result = self
            .resolver
            .check(&check_request)
            .await
            .map_err(domain_error_to_status)?;

        Ok(Response::new(CheckResponse {
            allowed: result.allowed,
            resolution: String::new(),
        }))
    }

    async fn batch_check(
        &self,
        request: Request<BatchCheckRequest>,
    ) -> Result<Response<BatchCheckResponse>, Status> {
        use rsfga_server::handlers::batch::{
            BatchCheckItem as ServerBatchCheckItem, BatchCheckRequest as ServerBatchCheckRequest,
        };

        let req = request.into_inner();

        // Validate store exists
        let _ = self
            .storage
            .get_store(&req.store_id)
            .await
            .map_err(storage_error_to_status)?;

        // Convert gRPC request to server-layer request, validating each check
        // We process correlation_ids and server_checks in lockstep to ensure correct mapping
        let mut correlation_ids: Vec<String> = Vec::with_capacity(req.checks.len());
        let mut server_checks: Vec<ServerBatchCheckItem> = Vec::with_capacity(req.checks.len());

        for (index, item) in req.checks.into_iter().enumerate() {
            let tuple_key = item.tuple_key.ok_or_else(|| {
                Status::invalid_argument(format!(
                    "tuple_key is required for check at index {index}"
                ))
            })?;

            // Validate correlation_id length to prevent DoS via excessive memory allocation
            if item.correlation_id.len() > MAX_CORRELATION_ID_LENGTH {
                return Err(Status::invalid_argument(format!(
                    "correlation_id at index {index} exceeds maximum length of {MAX_CORRELATION_ID_LENGTH} bytes"
                )));
            }

            // Parse context from gRPC Struct if present
            let context = if let Some(ctx) = item.context {
                prost_struct_to_hashmap(ctx).map_err(|_| {
                    Status::invalid_argument(format!(
                        "check at index {index} has invalid context (NaN or Infinity values not allowed)"
                    ))
                })?
            } else {
                std::collections::HashMap::new()
            };

            correlation_ids.push(item.correlation_id);
            server_checks.push(ServerBatchCheckItem {
                user: tuple_key.user,
                relation: tuple_key.relation,
                object: tuple_key.object,
                context,
            });
        }

        let server_request = ServerBatchCheckRequest::new(req.store_id, server_checks);

        // Delegate to BatchCheckHandler for parallel execution with deduplication
        let server_response = self
            .batch_handler
            .check(server_request)
            .await
            .map_err(batch_check_error_to_status)?;

        // Convert server response back to gRPC response format
        // SAFETY: Validate that results match input to prevent correlation ID mismatch.
        // This is a critical invariant - mismatched results would cause authorization
        // decisions to be mapped to wrong correlation IDs (violates Invariant I1).
        if server_response.results.len() != correlation_ids.len() {
            tracing::error!(
                expected = correlation_ids.len(),
                actual = server_response.results.len(),
                "batch check result count mismatch - this indicates a bug in BatchCheckHandler"
            );
            return Err(Status::internal(
                "internal error: batch check result count mismatch",
            ));
        }

        let mut result_map = std::collections::HashMap::new();
        for (correlation_id, item_result) in correlation_ids
            .into_iter()
            .zip(server_response.results.into_iter())
        {
            result_map.insert(
                correlation_id,
                BatchCheckSingleResult {
                    allowed: item_result.allowed,
                    error: item_result.error.map(|msg| CheckError {
                        code: ErrorCode::ValidationError as i32,
                        message: msg,
                    }),
                },
            );
        }

        Ok(Response::new(BatchCheckResponse { result: result_map }))
    }

    async fn write(
        &self,
        request: Request<WriteRequest>,
    ) -> Result<Response<WriteResponse>, Status> {
        let req = request.into_inner();

        // Validate store exists
        let _ = self
            .storage
            .get_store(&req.store_id)
            .await
            .map_err(storage_error_to_status)?;

        // Convert writes - fail if any tuple key is invalid
        // No clones in happy path - error contains user/object for messages
        let writes: Vec<StoredTuple> = req
            .writes
            .map(|w| {
                w.tuple_keys
                    .into_iter()
                    .enumerate()
                    .map(|(i, tk)| {
                        tuple_key_to_stored(tk).map_err(|e| {
                            Status::invalid_argument(format!(
                                "invalid tuple_key at writes index {i}: user='{}', object='{}': {}",
                                e.user, e.object, e.reason
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?
            .unwrap_or_default();

        // Convert deletes - fail if any tuple key is invalid
        // Deletes use TupleKeyWithoutCondition, convert to TupleKey
        let deletes: Vec<StoredTuple> = req
            .deletes
            .map(|d| {
                d.tuple_keys
                    .into_iter()
                    .enumerate()
                    .map(|(i, tk)| {
                        tuple_key_to_stored(TupleKey {
                            user: tk.user,
                            relation: tk.relation,
                            object: tk.object,
                            condition: None,
                        })
                        .map_err(|e| {
                            Status::invalid_argument(format!(
                                "invalid tuple_key at deletes index {i}: user='{}', object='{}': {}",
                                e.user, e.object, e.reason
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?
            .unwrap_or_default();

        self.storage
            .write_tuples(&req.store_id, writes, deletes)
            .await
            .map_err(storage_error_to_status)?;

        // Invalidate cache for this store to prevent stale auth decisions.
        // This is a coarse-grained stopgap until fine-grained invalidation is wired.
        self.cache.invalidate_store(&req.store_id).await;

        Ok(Response::new(WriteResponse {}))
    }

    async fn read(&self, request: Request<ReadRequest>) -> Result<Response<ReadResponse>, Status> {
        let req = request.into_inner();

        // Validate store exists
        let _ = self
            .storage
            .get_store(&req.store_id)
            .await
            .map_err(storage_error_to_status)?;

        // Build filter - use parse_object for consistent validation
        // Invalid object format is treated as "no object filter" rather than an error
        // since read is a query operation, not a write
        let filter = if let Some(tk) = req.tuple_key {
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

        let tuples = self
            .storage
            .read_tuples(&req.store_id, &filter)
            .await
            .map_err(storage_error_to_status)?;

        // Convert to response format, including conditions (OpenFGA compatibility I2)
        let response_tuples: Vec<Tuple> = tuples
            .into_iter()
            .map(|t| Tuple {
                key: Some(TupleKey {
                    user: format_user(&t.user_type, &t.user_id, t.user_relation.as_deref()),
                    relation: t.relation,
                    object: format!("{}:{}", t.object_type, t.object_id),
                    condition: t.condition_name.map(|name| RelationshipCondition {
                        name,
                        context: t.condition_context.map(hashmap_to_prost_struct),
                    }),
                }),
                timestamp: None,
            })
            .collect();

        Ok(Response::new(ReadResponse {
            tuples: response_tuples,
            continuation_token: String::new(),
        }))
    }

    async fn expand(
        &self,
        request: Request<ExpandRequest>,
    ) -> Result<Response<ExpandResponse>, Status> {
        let req = request.into_inner();

        let tuple_key = req
            .tuple_key
            .ok_or_else(|| Status::invalid_argument("tuple_key is required"))?;

        // Create domain expand request
        let expand_request =
            DomainExpandRequest::new(&req.store_id, &tuple_key.relation, &tuple_key.object);

        // Delegate to GraphResolver for expansion
        let result = self
            .resolver
            .expand(&expand_request)
            .await
            .map_err(domain_error_to_status)?;

        // Convert domain result to proto response
        Ok(Response::new(ExpandResponse {
            tree: Some(UsersetTree {
                root: Some(expand_node_to_proto(result.tree.root)),
            }),
        }))
    }
    async fn list_objects(
        &self,
        request: Request<ListObjectsRequest>,
    ) -> Result<Response<ListObjectsResponse>, Status> {
        use crate::validation::{
            estimate_context_size, json_exceeds_max_depth, validate_relation_format,
            validate_user_format, MAX_CONDITION_CONTEXT_SIZE, MAX_JSON_DEPTH,
            MAX_LIST_OBJECTS_CANDIDATES,
        };
        use rsfga_domain::resolver::ListObjectsRequest as DomainListObjectsRequest;
        use rsfga_storage::traits::validate_object_type;

        let req = request.into_inner();

        // Validate object type format
        if let Err(e) = validate_object_type(&req.r#type) {
            return Err(Status::invalid_argument(e.to_string()));
        }

        // Validate user format
        if let Some(err) = validate_user_format(&req.user) {
            return Err(Status::invalid_argument(err));
        }

        // Validate relation format
        if let Some(err) = validate_relation_format(&req.relation) {
            return Err(Status::invalid_argument(err));
        }
        let mut validated_context_map = None;
        if let Some(context_struct) = &req.context {
            // Check size (approximate from Struct)
            // Note: This is an estimation. For strict limits, we might want to check the byte size of the request.
            // But checking the converted JSON size is consistent with HTTP.
            // We convert to JSON map early to check.
            let context_map = prost_struct_to_hashmap(context_struct.clone())
                .map_err(|e| Status::invalid_argument(format!("invalid context: {}", e)))?;

            if estimate_context_size(&context_map) > MAX_CONDITION_CONTEXT_SIZE {
                return Err(Status::invalid_argument(format!(
                    "context size exceeds maximum of {MAX_CONDITION_CONTEXT_SIZE} bytes"
                )));
            }

            // Check nesting depth
            for value in context_map.values() {
                if json_exceeds_max_depth(value, 2) {
                    return Err(Status::invalid_argument(format!(
                        "context nested too deeply (max depth {MAX_JSON_DEPTH})"
                    )));
                }
            }

            validated_context_map = Some(context_map);
        }

        // Convert contextual tuples if provided, logging any that are skipped due to invalid format
        let contextual_tuples = req
            .contextual_tuples
            .map(|ct| {
                ct.tuple_keys
                    .into_iter()
                    .filter_map(|tk| {
                        let user = parse_user(&tk.user);
                        if user.is_none() {
                            tracing::warn!("Invalid user format in contextual tuple: {}", tk.user);
                            return None;
                        }
                        let (user_type, user_id, user_relation) = user.unwrap();

                        let object = parse_object(&tk.object);
                        if object.is_none() {
                            tracing::warn!(
                                "Invalid object format in contextual tuple: {}",
                                tk.object
                            );
                            return None;
                        }
                        let (object_type, object_id) = object.unwrap();

                        Some(rsfga_domain::resolver::ContextualTuple::new(
                            format_user(user_type, user_id, user_relation),
                            tk.relation,
                            format!("{object_type}:{object_id}"),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Create domain request using validated context if available
        let context = validated_context_map.unwrap_or_default();

        let list_request = DomainListObjectsRequest::with_context(
            req.store_id,
            req.user,
            req.relation,
            req.r#type,
            contextual_tuples,
            context,
        );

        // Call the resolver with DoS protection limit
        let result = self
            .resolver
            .list_objects(&list_request, MAX_LIST_OBJECTS_CANDIDATES)
            .await
            .map_err(domain_error_to_status)?;

        Ok(Response::new(ListObjectsResponse {
            objects: result.objects,
            truncated: result.truncated,
        }))
    }

    async fn list_users(
        &self,
        request: Request<ListUsersRequest>,
    ) -> Result<Response<ListUsersResponse>, Status> {
        use crate::validation::{
            estimate_context_size, json_exceeds_max_depth, validate_relation_format,
            MAX_CONDITION_CONTEXT_SIZE, MAX_JSON_DEPTH,
        };
        use rsfga_domain::resolver::{
            ListUsersRequest as DomainListUsersRequest, UserFilter, UserResult,
        };
        use rsfga_storage::traits::validate_object_type;

        let req = request.into_inner();

        // Validate store exists
        let _ = self
            .storage
            .get_store(&req.store_id)
            .await
            .map_err(storage_error_to_status)?;

        // Extract object from request
        let object = req
            .object
            .ok_or_else(|| Status::invalid_argument("object is required"))?;

        if object.r#type.is_empty() {
            return Err(Status::invalid_argument("object.type cannot be empty"));
        }
        if object.id.is_empty() {
            return Err(Status::invalid_argument("object.id cannot be empty"));
        }

        // Validate object type format
        if let Err(e) = validate_object_type(&object.r#type) {
            return Err(Status::invalid_argument(e.to_string()));
        }

        // Validate relation
        if req.relation.is_empty() {
            return Err(Status::invalid_argument("relation cannot be empty"));
        }

        // Validate relation format
        if let Some(err) = validate_relation_format(&req.relation) {
            return Err(Status::invalid_argument(err));
        }

        // Validate user_filters not empty
        if req.user_filters.is_empty() {
            return Err(Status::invalid_argument("user_filters cannot be empty"));
        }

        // Validate context if provided (DoS protection)
        if let Some(ref ctx) = req.context {
            let context_map = prost_struct_to_hashmap(ctx.clone())
                .map_err(|e| Status::invalid_argument(format!("invalid context: {e}")))?;

            if estimate_context_size(&context_map) > MAX_CONDITION_CONTEXT_SIZE {
                return Err(Status::invalid_argument(format!(
                    "context size exceeds maximum of {MAX_CONDITION_CONTEXT_SIZE} bytes"
                )));
            }

            // Check nesting depth
            for value in context_map.values() {
                if json_exceeds_max_depth(value, 2) {
                    return Err(Status::invalid_argument(format!(
                        "context nested too deeply (max depth {MAX_JSON_DEPTH})"
                    )));
                }
            }
        }

        // Convert user_filters
        let user_filters: Vec<UserFilter> = req
            .user_filters
            .into_iter()
            .map(|f| {
                if f.relation.is_empty() {
                    UserFilter::new(f.r#type)
                } else {
                    UserFilter::with_relation(f.r#type, f.relation)
                }
            })
            .collect();

        // Convert contextual tuples
        let contextual_tuples: Vec<rsfga_domain::resolver::ContextualTuple> = req
            .contextual_tuples
            .map(|ct| {
                ct.tuple_keys
                    .into_iter()
                    .filter_map(|tk| {
                        let user = parse_user(&tk.user);
                        if user.is_none() {
                            tracing::warn!("Invalid user format in contextual tuple: {}", tk.user);
                            return None;
                        }
                        let (user_type, user_id, user_relation) = user.unwrap();

                        let object = parse_object(&tk.object);
                        if object.is_none() {
                            tracing::warn!(
                                "Invalid object format in contextual tuple: {}",
                                tk.object
                            );
                            return None;
                        }
                        let (object_type, object_id) = object.unwrap();

                        let user_str = format_user(user_type, user_id, user_relation);
                        let object_str = format!("{object_type}:{object_id}");

                        // Preserve condition if present
                        if let Some(condition) = tk.condition {
                            let condition_context = condition
                                .context
                                .and_then(|ctx| prost_struct_to_hashmap(ctx).ok());
                            Some(rsfga_domain::resolver::ContextualTuple::with_condition(
                                user_str,
                                tk.relation,
                                object_str,
                                condition.name,
                                condition_context,
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

        // Convert context
        let context = req
            .context
            .map(prost_struct_to_hashmap)
            .transpose()
            .map_err(|e| Status::invalid_argument(format!("invalid context: {}", e)))?
            .unwrap_or_default();

        // Create domain request
        let object_str = format!("{}:{}", object.r#type, object.id);
        let list_request = DomainListUsersRequest::with_context(
            req.store_id,
            object_str,
            req.relation,
            user_filters,
            contextual_tuples,
            context,
        );

        // Call the resolver with default max results for DoS protection.
        // OpenFGA's proto doesn't support pagination for ListUsers, so we use an internal limit.
        const DEFAULT_MAX_RESULTS: usize = 1000;
        let result = self
            .resolver
            .list_users(&list_request, DEFAULT_MAX_RESULTS)
            .await
            .map_err(domain_error_to_status)?;

        // Convert domain results to proto format
        let users: Vec<User> = result
            .users
            .into_iter()
            .map(|u| match u {
                UserResult::Object { user_type, user_id } => User {
                    user: Some(user::User::Object(FgaObject {
                        r#type: user_type,
                        id: user_id,
                    })),
                },
                UserResult::Userset {
                    userset_type,
                    userset_id,
                    relation,
                } => User {
                    user: Some(user::User::Userset(UsersetUser {
                        r#type: userset_type,
                        id: userset_id,
                        relation,
                    })),
                },
                UserResult::Wildcard { wildcard_type } => User {
                    user: Some(user::User::Wildcard(TypedWildcard {
                        r#type: wildcard_type,
                    })),
                },
            })
            .collect();

        // Note: excluded_users in OpenFGA proto is Vec<String> not Vec<User>
        // We format as strings for compatibility
        let excluded_users: Vec<String> = result
            .excluded_users
            .into_iter()
            .map(|u| match u {
                UserResult::Object { user_type, user_id } => format!("{user_type}:{user_id}"),
                UserResult::Userset {
                    userset_type,
                    userset_id,
                    relation,
                } => format!("{userset_type}:{userset_id}#{relation}"),
                UserResult::Wildcard { wildcard_type } => format!("{wildcard_type}:*"),
            })
            .collect();

        Ok(Response::new(ListUsersResponse {
            users,
            excluded_users,
        }))
    }

    async fn create_store(
        &self,
        request: Request<CreateStoreRequest>,
    ) -> Result<Response<CreateStoreResponse>, Status> {
        let req = request.into_inner();
        let id = ulid::Ulid::new().to_string();

        let store = self
            .storage
            .create_store(&id, &req.name)
            .await
            .map_err(storage_error_to_status)?;

        Ok(Response::new(CreateStoreResponse {
            id: store.id,
            name: store.name,
            created_at: Some(datetime_to_timestamp(store.created_at)),
            updated_at: Some(datetime_to_timestamp(store.updated_at)),
        }))
    }

    async fn update_store(
        &self,
        request: Request<UpdateStoreRequest>,
    ) -> Result<Response<UpdateStoreResponse>, Status> {
        let req = request.into_inner();

        let store = self
            .storage
            .update_store(&req.store_id, &req.name)
            .await
            .map_err(storage_error_to_status)?;

        Ok(Response::new(UpdateStoreResponse {
            id: store.id,
            name: store.name,
            created_at: Some(datetime_to_timestamp(store.created_at)),
            updated_at: Some(datetime_to_timestamp(store.updated_at)),
        }))
    }

    async fn delete_store(
        &self,
        request: Request<DeleteStoreRequest>,
    ) -> Result<Response<DeleteStoreResponse>, Status> {
        let req = request.into_inner();

        self.storage
            .delete_store(&req.store_id)
            .await
            .map_err(storage_error_to_status)?;

        // Clean up assertions for this store to prevent memory leak.
        // Assertions are keyed by (store_id, model_id), so we remove all entries
        // where the store_id matches the deleted store.
        self.assertions
            .retain(|(store_id, _model_id), _| store_id != &req.store_id);

        Ok(Response::new(DeleteStoreResponse {}))
    }

    async fn get_store(
        &self,
        request: Request<GetStoreRequest>,
    ) -> Result<Response<GetStoreResponse>, Status> {
        let req = request.into_inner();

        let store = self
            .storage
            .get_store(&req.store_id)
            .await
            .map_err(storage_error_to_status)?;

        Ok(Response::new(GetStoreResponse {
            id: store.id,
            name: store.name,
            created_at: Some(datetime_to_timestamp(store.created_at)),
            updated_at: Some(datetime_to_timestamp(store.updated_at)),
        }))
    }

    async fn list_stores(
        &self,
        _request: Request<ListStoresRequest>,
    ) -> Result<Response<ListStoresResponse>, Status> {
        let stores = self
            .storage
            .list_stores()
            .await
            .map_err(storage_error_to_status)?;

        let response_stores: Vec<Store> = stores
            .into_iter()
            .map(|s| Store {
                id: s.id,
                name: s.name,
                created_at: Some(datetime_to_timestamp(s.created_at)),
                updated_at: Some(datetime_to_timestamp(s.updated_at)),
                deleted_at: None,
            })
            .collect();

        Ok(Response::new(ListStoresResponse {
            stores: response_stores,
            continuation_token: String::new(),
        }))
    }

    // Authorization model operations
    async fn read_authorization_models(
        &self,
        request: Request<ReadAuthorizationModelsRequest>,
    ) -> Result<Response<ReadAuthorizationModelsResponse>, Status> {
        let req = request.into_inner();

        // Validate store exists
        let _ = self
            .storage
            .get_store(&req.store_id)
            .await
            .map_err(storage_error_to_status)?;

        // Parse pagination from request, capping at DEFAULT_PAGE_SIZE
        let page_size = req
            .page_size
            .map(|ps| ps.min(DEFAULT_PAGE_SIZE))
            .unwrap_or(DEFAULT_PAGE_SIZE);
        let pagination = PaginationOptions {
            page_size: Some(page_size as u32),
            continuation_token: if req.continuation_token.is_empty() {
                None
            } else {
                Some(req.continuation_token)
            },
        };

        // Fetch models from storage with pagination
        let result = self
            .storage
            .list_authorization_models_paginated(&req.store_id, &pagination)
            .await
            .map_err(storage_error_to_status)?;

        // Convert stored models to proto AuthorizationModel.
        // Log warnings for malformed models instead of silently ignoring them.
        let authorization_models: Vec<AuthorizationModel> = result
            .items
            .into_iter()
            .filter_map(|stored| match stored_model_to_proto(&stored) {
                Some(model) => Some(model),
                None => {
                    tracing::warn!(
                        model_id = %stored.id,
                        store_id = %stored.store_id,
                        "Failed to parse stored authorization model - possible data corruption"
                    );
                    None
                }
            })
            .collect();

        Ok(Response::new(ReadAuthorizationModelsResponse {
            authorization_models,
            continuation_token: result.continuation_token.unwrap_or_default(),
        }))
    }

    async fn read_authorization_model(
        &self,
        request: Request<ReadAuthorizationModelRequest>,
    ) -> Result<Response<ReadAuthorizationModelResponse>, Status> {
        let req = request.into_inner();

        // Validate store exists
        let _ = self
            .storage
            .get_store(&req.store_id)
            .await
            .map_err(storage_error_to_status)?;

        // Fetch the specific model from storage
        let stored = self
            .storage
            .get_authorization_model(&req.store_id, &req.id)
            .await
            .map_err(storage_error_to_status)?;

        // Convert to proto AuthorizationModel
        let authorization_model = stored_model_to_proto(&stored)
            .ok_or_else(|| Status::internal("failed to parse stored authorization model"))?;

        Ok(Response::new(ReadAuthorizationModelResponse {
            authorization_model: Some(authorization_model),
        }))
    }

    async fn write_authorization_model(
        &self,
        request: Request<WriteAuthorizationModelRequest>,
    ) -> Result<Response<WriteAuthorizationModelResponse>, Status> {
        let req = request.into_inner();

        // Validate store exists
        let _ = self
            .storage
            .get_store(&req.store_id)
            .await
            .map_err(storage_error_to_status)?;

        // Validate type_definitions is not empty (OpenFGA requirement)
        if req.type_definitions.is_empty() {
            return Err(Status::invalid_argument("type_definitions cannot be empty"));
        }

        // Generate a new ULID for the model
        let model_id = ulid::Ulid::new().to_string();

        // Convert proto TypeDefinitions to JSON for storage
        let type_definitions_json: Vec<serde_json::Value> = req
            .type_definitions
            .iter()
            .map(type_definition_to_json)
            .collect();

        // Build model JSON (same format as HTTP layer)
        let mut model_json = serde_json::json!({
            "type_definitions": type_definitions_json,
        });

        // Convert conditions if present
        if !req.conditions.is_empty() {
            let conditions_json: serde_json::Map<String, serde_json::Value> = req
                .conditions
                .iter()
                .map(|(k, v)| (k.clone(), condition_to_json(v)))
                .collect();
            model_json["conditions"] = serde_json::Value::Object(conditions_json);
        }

        // Validate model size before storage
        let model_json_str = model_json.to_string();
        if model_json_str.len() > MAX_AUTHORIZATION_MODEL_SIZE {
            return Err(Status::invalid_argument(format!(
                "authorization model exceeds maximum size of {MAX_AUTHORIZATION_MODEL_SIZE} bytes"
            )));
        }

        // Create stored model and persist
        let stored_model = StoredAuthorizationModel::new(
            &model_id,
            &req.store_id,
            &req.schema_version,
            model_json_str,
        );

        self.storage
            .write_authorization_model(stored_model)
            .await
            .map_err(storage_error_to_status)?;

        // Invalidate CEL expression cache to ensure stale expressions from
        // previous models are not reused. This is critical for security:
        // old condition expressions must not be evaluated against new models.
        global_cache().invalidate_all();

        Ok(Response::new(WriteAuthorizationModelResponse {
            authorization_model_id: model_id,
        }))
    }

    // Assertions operations
    async fn read_assertions(
        &self,
        request: Request<ReadAssertionsRequest>,
    ) -> Result<Response<ReadAssertionsResponse>, Status> {
        let req = request.into_inner();

        // Validate store exists
        let _ = self
            .storage
            .get_store(&req.store_id)
            .await
            .map_err(storage_error_to_status)?;

        // Validate model exists.
        // Note: This fetches the full model to validate existence. A lightweight
        // model_exists() method on DataStore could optimize this, but the current
        // approach is correct and assertions are typically low-traffic operations.
        let _ = self
            .storage
            .get_authorization_model(&req.store_id, &req.authorization_model_id)
            .await
            .map_err(storage_error_to_status)?;

        // Read assertions from in-memory storage
        // Preserves conditions on tuple keys to avoid data loss.
        let key = (req.store_id.clone(), req.authorization_model_id.clone());
        let assertions: Vec<Assertion> = self
            .assertions
            .get(&key)
            .map(|stored| {
                stored
                    .value()
                    .iter()
                    .map(|sa| Assertion {
                        tuple_key: Some(TupleKey {
                            user: sa.tuple_key.user.clone(),
                            relation: sa.tuple_key.relation.clone(),
                            object: sa.tuple_key.object.clone(),
                            condition: sa.tuple_key.condition.as_ref().map(|c| {
                                RelationshipCondition {
                                    name: c.name.clone(),
                                    context: c
                                        .context
                                        .as_ref()
                                        .map(|ctx| hashmap_to_prost_struct(ctx.clone())),
                                }
                            }),
                        }),
                        expectation: sa.expectation,
                        contextual_tuples: sa
                            .contextual_tuples
                            .as_ref()
                            .map(|ct| {
                                ct.tuple_keys
                                    .iter()
                                    .map(|tk| TupleKey {
                                        user: tk.user.clone(),
                                        relation: tk.relation.clone(),
                                        object: tk.object.clone(),
                                        condition: tk.condition.as_ref().map(|c| {
                                            RelationshipCondition {
                                                name: c.name.clone(),
                                                context: c.context.as_ref().map(|ctx| {
                                                    hashmap_to_prost_struct(ctx.clone())
                                                }),
                                            }
                                        }),
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(Response::new(ReadAssertionsResponse {
            authorization_model_id: req.authorization_model_id,
            assertions,
        }))
    }

    async fn write_assertions(
        &self,
        request: Request<WriteAssertionsRequest>,
    ) -> Result<Response<WriteAssertionsResponse>, Status> {
        let req = request.into_inner();

        // Validate store exists
        let _ = self
            .storage
            .get_store(&req.store_id)
            .await
            .map_err(storage_error_to_status)?;

        // Validate model exists.
        // Note: This fetches the full model to validate existence. A lightweight
        // model_exists() method on DataStore could optimize this, but the current
        // approach is correct and assertions are typically low-traffic operations.
        let _ = self
            .storage
            .get_authorization_model(&req.store_id, &req.authorization_model_id)
            .await
            .map_err(storage_error_to_status)?;

        // Check assertion entry capacity to prevent unbounded memory growth.
        // Only check for NEW keys - updates to existing keys don't increase count.
        let assertion_key = (req.store_id.clone(), req.authorization_model_id.clone());
        let current_count = self.assertions.len();
        let is_new_key = !self.assertions.contains_key(&assertion_key);

        if is_new_key {
            if current_count >= MAX_ASSERTION_ENTRIES {
                tracing::error!(
                    current_count = current_count,
                    max = MAX_ASSERTION_ENTRIES,
                    store_id = %req.store_id,
                    model_id = %req.authorization_model_id,
                    "Assertion storage capacity exceeded"
                );
                return Err(Status::resource_exhausted(format!(
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

        // Convert proto assertions to stored format (same format as HTTP layer)
        // Preserves conditions on tuple keys. Rejects requests with invalid context data
        // (e.g., NaN/Infinity values) to prevent silent data loss.
        let mut stored_assertions: Vec<StoredAssertion> = Vec::with_capacity(req.assertions.len());

        for (assertion_idx, a) in req.assertions.into_iter().enumerate() {
            let tuple_key = match a.tuple_key {
                Some(tk) => tk,
                None => continue, // Skip assertions without tuple_key
            };

            // Convert condition, failing on invalid context
            let condition = match tuple_key.condition {
                Some(c) => {
                    let context = match c.context {
                        Some(ctx) => Some(prost_struct_to_hashmap(ctx).map_err(|e| {
                            Status::invalid_argument(format!(
                                "assertion at index {}: condition '{}' has invalid context: {}",
                                assertion_idx, c.name, e
                            ))
                        })?),
                        None => None,
                    };
                    Some(AssertionCondition {
                        name: c.name,
                        context,
                    })
                }
                None => None,
            };

            // Convert contextual tuples, failing on invalid context
            let contextual_tuples = if a.contextual_tuples.is_empty() {
                None
            } else {
                let mut tuple_keys = Vec::with_capacity(a.contextual_tuples.len());
                for (ct_idx, tk) in a.contextual_tuples.into_iter().enumerate() {
                    let ct_condition = match tk.condition {
                        Some(c) => {
                            let context = match c.context {
                                Some(ctx) => {
                                    Some(prost_struct_to_hashmap(ctx).map_err(|e| {
                                        Status::invalid_argument(format!(
                                            "assertion at index {}: contextual tuple at index {}: condition '{}' has invalid context: {}",
                                            assertion_idx, ct_idx, c.name, e
                                        ))
                                    })?)
                                }
                                None => None,
                            };
                            Some(AssertionCondition {
                                name: c.name,
                                context,
                            })
                        }
                        None => None,
                    };
                    tuple_keys.push(AssertionTupleKey {
                        user: tk.user,
                        relation: tk.relation,
                        object: tk.object,
                        condition: ct_condition,
                    });
                }
                Some(ContextualTuplesWrapper { tuple_keys })
            };

            stored_assertions.push(StoredAssertion {
                tuple_key: AssertionTupleKey {
                    user: tuple_key.user,
                    relation: tuple_key.relation,
                    object: tuple_key.object,
                    condition,
                },
                expectation: a.expectation,
                contextual_tuples,
            });
        }

        // Store assertions (replaces existing)
        let key = (req.store_id, req.authorization_model_id);
        self.assertions.insert(key, stored_assertions);

        Ok(Response::new(WriteAssertionsResponse {}))
    }

    // Read changes
    //
    // Returns tuple changes (writes) from the store. This simplified implementation
    // returns current tuples as "writes" but does not track actual change history
    // or delete operations. For full OpenFGA compatibility, a changelog table would
    // be needed in the storage layer.
    async fn read_changes(
        &self,
        request: Request<ReadChangesRequest>,
    ) -> Result<Response<ReadChangesResponse>, Status> {
        let req = request.into_inner();

        // Validate store exists
        let _ = self
            .storage
            .get_store(&req.store_id)
            .await
            .map_err(storage_error_to_status)?;

        // Build filter - optionally filter by object type
        let filter = TupleFilter {
            object_type: if req.r#type.is_empty() {
                None
            } else {
                Some(req.r#type.clone())
            },
            ..Default::default()
        };

        // Validate and parse page_size
        let page_size = req.page_size.map(|v| v as u32);

        let pagination = PaginationOptions {
            page_size,
            continuation_token: if req.continuation_token.is_empty() {
                None
            } else {
                Some(req.continuation_token.clone())
            },
        };

        // Read tuples from storage
        let result = self
            .storage
            .read_tuples_paginated(&req.store_id, &filter, &pagination)
            .await
            .map_err(storage_error_to_status)?;

        // Convert tuples to changes (all as writes)
        let changes: Vec<TupleChange> = result
            .items
            .into_iter()
            .map(|t| {
                let user = format_user(&t.user_type, &t.user_id, t.user_relation.as_deref());
                let object = format!("{}:{}", t.object_type, t.object_id);

                // Build condition if present
                let condition = t.condition_name.map(|name| RelationshipCondition {
                    name,
                    context: t.condition_context.map(hashmap_to_prost_struct),
                });

                TupleChange {
                    tuple_key: Some(TupleKey {
                        user,
                        relation: t.relation,
                        object,
                        condition,
                    }),
                    operation: TupleOperation::Write as i32,
                    // Use tuple's created_at timestamp or current time as fallback
                    timestamp: Some(datetime_to_timestamp(t.created_at.unwrap_or_else(Utc::now))),
                }
            })
            .collect();

        Ok(Response::new(ReadChangesResponse {
            changes,
            continuation_token: result.continuation_token.unwrap_or_default(),
        }))
    }
}
