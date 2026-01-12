//! gRPC service implementation for OpenFGA-compatible API.

use std::sync::Arc;

use tonic::{Request, Response, Status};

use rsfga_domain::cache::{CheckCache, CheckCacheConfig};
use rsfga_domain::cel::global_cache;
use rsfga_domain::resolver::GraphResolver;
use rsfga_server::handlers::batch::BatchCheckHandler;
use rsfga_storage::{DataStore, DateTime, StorageError, StoredTuple, TupleFilter, Utc};

use crate::adapters::{DataStoreModelReader, DataStoreTupleReader};
use crate::utils::{format_user, parse_object, parse_user};

use crate::proto::openfga::v1::{
    open_fga_service_server::OpenFgaService, BatchCheckRequest, BatchCheckResponse,
    BatchCheckSingleResult, CheckError, CheckRequest, CheckResponse, CreateStoreRequest,
    CreateStoreResponse, DeleteStoreRequest, DeleteStoreResponse, ErrorCode, ExpandRequest,
    ExpandResponse, GetStoreRequest, GetStoreResponse, ListObjectsRequest, ListObjectsResponse,
    ListStoresRequest, ListStoresResponse, ListUsersRequest, ListUsersResponse,
    ReadAssertionsRequest, ReadAssertionsResponse, ReadAuthorizationModelRequest,
    ReadAuthorizationModelResponse, ReadAuthorizationModelsRequest,
    ReadAuthorizationModelsResponse, ReadChangesRequest, ReadChangesResponse, ReadRequest,
    ReadResponse, Store, Tuple, TupleKey, UpdateStoreRequest, UpdateStoreResponse,
    WriteAssertionsRequest, WriteAssertionsResponse, WriteAuthorizationModelRequest,
    WriteAuthorizationModelResponse, WriteRequest, WriteResponse,
};

/// gRPC service implementation for OpenFGA.
///
/// This service implements the OpenFGA gRPC API with support for parallel
/// batch checks through the integrated BatchCheckHandler.
pub struct OpenFgaGrpcService<S: DataStore> {
    storage: Arc<S>,
    /// The batch check handler with parallel execution and deduplication.
    batch_handler: Arc<BatchCheckHandler<DataStoreTupleReader<S>, DataStoreModelReader<S>>>,
}

impl<S: DataStore> OpenFgaGrpcService<S> {
    /// Creates a new gRPC service instance with default cache configuration.
    pub fn new(storage: Arc<S>) -> Self {
        Self::with_cache_config(storage, CheckCacheConfig::default())
    }

    /// Creates a new gRPC service instance with custom cache configuration.
    pub fn with_cache_config(storage: Arc<S>, cache_config: CheckCacheConfig) -> Self {
        // Create adapters to bridge storage to domain traits
        let tuple_reader = Arc::new(DataStoreTupleReader::new(Arc::clone(&storage)));
        let model_reader = Arc::new(DataStoreModelReader::new(Arc::clone(&storage)));

        // Create the graph resolver
        let resolver = Arc::new(GraphResolver::new(
            Arc::clone(&tuple_reader),
            Arc::clone(&model_reader),
        ));

        // Create the check cache
        let cache = Arc::new(CheckCache::new(cache_config));

        // Create the batch handler
        let batch_handler = Arc::new(BatchCheckHandler::new(
            Arc::clone(&resolver),
            Arc::clone(&cache),
        ));

        Self {
            storage,
            batch_handler,
        }
    }
}

/// Converts a StorageError to a tonic Status.
fn storage_error_to_status(err: StorageError) -> Status {
    match &err {
        StorageError::StoreNotFound { store_id } => {
            Status::not_found(format!("store not found: {}", store_id))
        }
        StorageError::TupleNotFound { .. } => Status::not_found(err.to_string()),
        StorageError::InvalidInput { message } => Status::invalid_argument(message),
        StorageError::DuplicateTuple { .. } => Status::already_exists(err.to_string()),
        _ => Status::internal(err.to_string()),
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
        BatchCheckError::BatchTooLarge { size, max } => Status::invalid_argument(format!(
            "batch size {} exceeds maximum allowed {}",
            size, max
        )),
        BatchCheckError::InvalidCheck { index, message } => {
            Status::invalid_argument(format!("invalid check at index {}: {}", index, message))
        }
        BatchCheckError::DomainError(msg) => {
            // Log internal errors for debugging - these indicate unexpected failures
            tracing::error!(error = %msg, "Domain error in gRPC batch check");
            Status::internal(format!("check error: {}", msg))
        }
    }
}

/// Converts a TupleKey proto to StoredTuple.
///
/// Uses `parse_user` and `parse_object` for consistent validation across all handlers.
fn tuple_key_to_stored(tk: &TupleKey) -> Option<StoredTuple> {
    let (user_type, user_id, user_relation) = parse_user(&tk.user)?;
    // Use parse_object for consistent validation (rejects empty type or id)
    let (object_type, object_id) = parse_object(&tk.object)?;

    Some(StoredTuple {
        object_type: object_type.to_string(),
        object_id: object_id.to_string(),
        relation: tk.relation.clone(),
        user_type: user_type.to_string(),
        user_id: user_id.to_string(),
        user_relation: user_relation.map(|s| s.to_string()),
        // TODO(#84): Parse condition from request when API support is added
        condition_name: None,
        condition_context: None,
    })
}

/// Converts a chrono DateTime<Utc> to a prost_types::Timestamp.
fn datetime_to_timestamp(dt: DateTime<Utc>) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: dt.timestamp(),
        nanos: dt.timestamp_subsec_nanos() as i32,
    }
}

#[tonic::async_trait]
impl<S: DataStore> OpenFgaService for OpenFgaGrpcService<S> {
    async fn check(
        &self,
        request: Request<CheckRequest>,
    ) -> Result<Response<CheckResponse>, Status> {
        let req = request.into_inner();

        // Validate store exists
        let _ = self
            .storage
            .get_store(&req.store_id)
            .await
            .map_err(storage_error_to_status)?;

        let tuple_key = req
            .tuple_key
            .ok_or_else(|| Status::invalid_argument("tuple_key is required"))?;

        // Validate object format (required: "type:id")
        let (object_type, object_id) = parse_object(&tuple_key.object).ok_or_else(|| {
            Status::invalid_argument(format!(
                "invalid object format '{}': expected 'type:id'",
                tuple_key.object
            ))
        })?;

        // Build filter for tuple lookup
        let filter = TupleFilter {
            object_type: Some(object_type.to_string()),
            object_id: Some(object_id.to_string()),
            relation: Some(tuple_key.relation),
            user: Some(tuple_key.user),
            condition_name: None,
        };

        let tuples = self
            .storage
            .read_tuples(&req.store_id, &filter)
            .await
            .map_err(storage_error_to_status)?;

        Ok(Response::new(CheckResponse {
            allowed: !tuples.is_empty(),
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
                    "tuple_key is required for check at index {}",
                    index
                ))
            })?;

            correlation_ids.push(item.correlation_id);
            server_checks.push(ServerBatchCheckItem {
                user: tuple_key.user,
                relation: tuple_key.relation,
                object: tuple_key.object,
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
        let writes: Vec<StoredTuple> = req
            .writes
            .map(|w| {
                w.tuple_keys
                    .iter()
                    .enumerate()
                    .map(|(i, tk)| {
                        tuple_key_to_stored(tk).ok_or_else(|| {
                            Status::invalid_argument(format!(
                                "invalid tuple_key at writes index {}: user='{}', object='{}'",
                                i, tk.user, tk.object
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?
            .unwrap_or_default();

        // Convert deletes - fail if any tuple key is invalid
        let deletes: Vec<StoredTuple> = req
            .deletes
            .map(|d| {
                d.tuple_keys
                    .iter()
                    .enumerate()
                    .map(|(i, tk)| {
                        tuple_key_to_stored(&TupleKey {
                            user: tk.user.clone(),
                            relation: tk.relation.clone(),
                            object: tk.object.clone(),
                            condition: None,
                        })
                        .ok_or_else(|| {
                            Status::invalid_argument(format!(
                                "invalid tuple_key at deletes index {}: user='{}', object='{}'",
                                i, tk.user, tk.object
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

        // Convert to response format
        let response_tuples: Vec<Tuple> = tuples
            .into_iter()
            .map(|t| Tuple {
                key: Some(TupleKey {
                    user: format_user(&t.user_type, &t.user_id, t.user_relation.as_deref()),
                    relation: t.relation,
                    object: format!("{}:{}", t.object_type, t.object_id),
                    condition: None,
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

        // Validate store exists
        let _ = self
            .storage
            .get_store(&req.store_id)
            .await
            .map_err(storage_error_to_status)?;

        // Expand not yet implemented
        Ok(Response::new(ExpandResponse { tree: None }))
    }

    async fn list_objects(
        &self,
        request: Request<ListObjectsRequest>,
    ) -> Result<Response<ListObjectsResponse>, Status> {
        let req = request.into_inner();

        // Validate store exists
        let _ = self
            .storage
            .get_store(&req.store_id)
            .await
            .map_err(storage_error_to_status)?;

        // ListObjects not yet implemented
        Ok(Response::new(ListObjectsResponse { objects: vec![] }))
    }

    async fn list_users(
        &self,
        request: Request<ListUsersRequest>,
    ) -> Result<Response<ListUsersResponse>, Status> {
        let req = request.into_inner();

        // Validate store exists
        let _ = self
            .storage
            .get_store(&req.store_id)
            .await
            .map_err(storage_error_to_status)?;

        // ListUsers not yet implemented
        Ok(Response::new(ListUsersResponse {
            users: vec![],
            excluded_users: vec![],
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

    // Authorization model operations (stubs)
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

        Ok(Response::new(ReadAuthorizationModelsResponse {
            authorization_models: vec![],
            continuation_token: String::new(),
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

        Err(Status::not_found("authorization model not found"))
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

        // TODO: Persist the authorization model to storage when implemented

        // Invalidate CEL expression cache to ensure stale expressions from
        // previous models are not reused. This is critical for security:
        // old condition expressions must not be evaluated against new models.
        global_cache().invalidate_all();

        Ok(Response::new(WriteAuthorizationModelResponse {
            authorization_model_id: ulid::Ulid::new().to_string(),
        }))
    }

    // Assertions (stubs)
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

        Ok(Response::new(ReadAssertionsResponse {
            authorization_model_id: req.authorization_model_id,
            assertions: vec![],
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

        Ok(Response::new(WriteAssertionsResponse {}))
    }

    // Read changes (stub)
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

        Ok(Response::new(ReadChangesResponse {
            changes: vec![],
            continuation_token: String::new(),
        }))
    }
}
