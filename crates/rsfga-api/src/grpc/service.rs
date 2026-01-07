//! gRPC service implementation for OpenFGA-compatible API.

use std::sync::Arc;

use tonic::{Request, Response, Status};

use rsfga_storage::{DataStore, StorageError, StoredTuple, TupleFilter};

use crate::utils::{format_user, parse_object, parse_user, MAX_BATCH_SIZE};

use crate::proto::openfga::v1::{
    open_fga_service_server::OpenFgaService, BatchCheckItem, BatchCheckRequest, BatchCheckResponse,
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
pub struct OpenFgaGrpcService<S: DataStore> {
    storage: Arc<S>,
}

impl<S: DataStore> OpenFgaGrpcService<S> {
    /// Creates a new gRPC service instance.
    pub fn new(storage: Arc<S>) -> Self {
        Self { storage }
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
    })
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

    // NOTE: This implementation performs sequential tuple lookups for simplicity.
    // In Milestone 1.8 (Server Integration), this will delegate to BatchCheckHandler
    // in rsfga-server to leverage parallel execution, request deduplication, and
    // singleflight optimizations. See plan.md for the integration roadmap.
    async fn batch_check(
        &self,
        request: Request<BatchCheckRequest>,
    ) -> Result<Response<BatchCheckResponse>, Status> {
        let req = request.into_inner();

        // Validate store exists
        let _ = self
            .storage
            .get_store(&req.store_id)
            .await
            .map_err(storage_error_to_status)?;

        // Validate batch size
        if req.checks.is_empty() {
            return Err(Status::invalid_argument("batch request cannot be empty"));
        }
        if req.checks.len() > MAX_BATCH_SIZE {
            return Err(Status::invalid_argument(format!(
                "batch size {} exceeds maximum allowed {}",
                req.checks.len(),
                MAX_BATCH_SIZE
            )));
        }

        // Process each check independently - errors are captured per-item, not propagated
        // This matches OpenFGA behavior where each check in a batch is independent
        let mut result_map = std::collections::HashMap::new();
        for item in req.checks.into_iter() {
            let result = self.process_batch_check_item(&req.store_id, &item).await;
            result_map.insert(item.correlation_id, result);
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
        let id = uuid::Uuid::new_v4().to_string();

        let store = self
            .storage
            .create_store(&id, &req.name)
            .await
            .map_err(storage_error_to_status)?;

        Ok(Response::new(CreateStoreResponse {
            id: store.id,
            name: store.name,
            created_at: Some(prost_types::Timestamp {
                seconds: store.created_at.timestamp(),
                nanos: store.created_at.timestamp_subsec_nanos() as i32,
            }),
            updated_at: Some(prost_types::Timestamp {
                seconds: store.updated_at.timestamp(),
                nanos: store.updated_at.timestamp_subsec_nanos() as i32,
            }),
        }))
    }

    async fn update_store(
        &self,
        request: Request<UpdateStoreRequest>,
    ) -> Result<Response<UpdateStoreResponse>, Status> {
        let req = request.into_inner();

        // Validate store exists
        let _ = self
            .storage
            .get_store(&req.store_id)
            .await
            .map_err(storage_error_to_status)?;

        // TODO: UpdateStore requires storage layer support (update_store method)
        // Currently returns UNIMPLEMENTED as the storage trait doesn't have update_store.
        // This will be implemented when storage layer adds update_store support.
        Err(Status::unimplemented(
            "update_store is not yet implemented - storage layer support required",
        ))
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
            created_at: Some(prost_types::Timestamp {
                seconds: store.created_at.timestamp(),
                nanos: store.created_at.timestamp_subsec_nanos() as i32,
            }),
            updated_at: Some(prost_types::Timestamp {
                seconds: store.updated_at.timestamp(),
                nanos: store.updated_at.timestamp_subsec_nanos() as i32,
            }),
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
                created_at: Some(prost_types::Timestamp {
                    seconds: s.created_at.timestamp(),
                    nanos: s.created_at.timestamp_subsec_nanos() as i32,
                }),
                updated_at: Some(prost_types::Timestamp {
                    seconds: s.updated_at.timestamp(),
                    nanos: s.updated_at.timestamp_subsec_nanos() as i32,
                }),
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

        Ok(Response::new(WriteAuthorizationModelResponse {
            authorization_model_id: uuid::Uuid::new_v4().to_string(),
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

impl<S: DataStore> OpenFgaGrpcService<S> {
    /// Helper to process a single batch check item.
    /// Returns a result with either allowed status or error - errors are per-item, not propagated.
    async fn process_batch_check_item(
        &self,
        store_id: &str,
        item: &BatchCheckItem,
    ) -> BatchCheckSingleResult {
        let tuple_key = match item.tuple_key.as_ref() {
            Some(tk) => tk,
            None => {
                return BatchCheckSingleResult {
                    allowed: false,
                    error: Some(CheckError {
                        code: ErrorCode::ValidationError as i32,
                        message: "tuple_key is required".to_string(),
                    }),
                };
            }
        };

        // Validate object format (required: "type:id")
        let (object_type, object_id) = match parse_object(&tuple_key.object) {
            Some((t, i)) => (t, i),
            None => {
                return BatchCheckSingleResult {
                    allowed: false,
                    error: Some(CheckError {
                        code: ErrorCode::InvalidObjectFormat as i32,
                        message: format!(
                            "invalid object format '{}': expected 'type:id'",
                            tuple_key.object
                        ),
                    }),
                };
            }
        };

        // Validate user format
        if parse_user(&tuple_key.user).is_none() {
            return BatchCheckSingleResult {
                allowed: false,
                error: Some(CheckError {
                    code: ErrorCode::ValidationError as i32,
                    message: format!(
                        "invalid user format '{}': expected 'type:id' or 'type:id#relation'",
                        tuple_key.user
                    ),
                }),
            };
        }

        let filter = TupleFilter {
            object_type: Some(object_type.to_string()),
            object_id: Some(object_id.to_string()),
            relation: Some(tuple_key.relation.clone()),
            user: Some(tuple_key.user.clone()),
        };

        match self.storage.read_tuples(store_id, &filter).await {
            Ok(tuples) => BatchCheckSingleResult {
                allowed: !tuples.is_empty(),
                error: None,
            },
            Err(e) => BatchCheckSingleResult {
                allowed: false,
                error: Some(CheckError {
                    code: ErrorCode::ValidationError as i32,
                    message: format!("storage error: {}", e),
                }),
            },
        }
    }
}
