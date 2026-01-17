//! gRPC service implementation for OpenFGA-compatible API.

use std::sync::Arc;

use tonic::{Request, Response, Status};

use rsfga_domain::cache::{CheckCache, CheckCacheConfig};
use rsfga_domain::cel::global_cache;
use rsfga_domain::error::DomainError;
use rsfga_domain::resolver::{
    CheckRequest as DomainCheckRequest, ContextualTuple, ExpandRequest as DomainExpandRequest,
    GraphResolver, ResolverConfig,
};
use rsfga_server::handlers::batch::BatchCheckHandler;
use rsfga_storage::{DataStore, DateTime, StorageError, StoredTuple, TupleFilter, Utc};

use crate::adapters::{DataStoreModelReader, DataStoreTupleReader};
use crate::utils::{format_user, parse_object, parse_user};

use crate::proto::openfga::v1::{
    open_fga_service_server::OpenFgaService, BatchCheckRequest, BatchCheckResponse,
    BatchCheckSingleResult, CheckError, CheckRequest, CheckResponse, Computed, CreateStoreRequest,
    CreateStoreResponse, DeleteStoreRequest, DeleteStoreResponse, ErrorCode, ExpandRequest,
    ExpandResponse, GetStoreRequest, GetStoreResponse, Leaf, ListObjectsRequest,
    ListObjectsResponse, ListStoresRequest, ListStoresResponse, ListUsersRequest,
    ListUsersResponse, Node, Nodes, ObjectRelation, ReadAssertionsRequest, ReadAssertionsResponse,
    ReadAuthorizationModelRequest, ReadAuthorizationModelResponse, ReadAuthorizationModelsRequest,
    ReadAuthorizationModelsResponse, ReadChangesRequest, ReadChangesResponse, ReadRequest,
    ReadResponse, RelationshipCondition, Store, Tuple, TupleKey, TupleToUserset,
    UpdateStoreRequest, UpdateStoreResponse, Users, UsersetTree, WriteAssertionsRequest,
    WriteAssertionsResponse, WriteAuthorizationModelRequest, WriteAuthorizationModelResponse,
    WriteRequest, WriteResponse,
};

/// Maximum allowed length for correlation IDs to prevent DoS attacks.
/// OpenFGA uses UUIDs (36 chars), so 256 provides generous headroom.
const MAX_CORRELATION_ID_LENGTH: usize = 256;

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
pub struct OpenFgaGrpcService<S: DataStore> {
    storage: Arc<S>,
    /// The graph resolver for single checks.
    resolver: Arc<GraphResolver<DataStoreTupleReader<S>, DataStoreModelReader<S>>>,
    /// The batch check handler with parallel execution and deduplication.
    batch_handler: Arc<BatchCheckHandler<DataStoreTupleReader<S>, DataStoreModelReader<S>>>,
    /// The check result cache, stored for future invalidation in write handlers.
    cache: Arc<CheckCache>,
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
        StorageError::TupleNotFound { .. } => Status::not_found(err.to_string()),
        StorageError::InvalidInput { message } => Status::invalid_argument(message),
        StorageError::DuplicateTuple { .. } => Status::already_exists(err.to_string()),
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

/// Converts a prost_types::Value to serde_json::Value (takes ownership to avoid clones).
///
/// Returns `Err` if the value contains NaN or Infinity (invalid JSON numbers).
fn prost_value_to_json(value: prost_types::Value) -> Result<serde_json::Value, &'static str> {
    use prost_types::value::Kind;

    match value.kind {
        Some(Kind::NullValue(_)) => Ok(serde_json::Value::Null),
        Some(Kind::NumberValue(n)) => {
            // Reject NaN and Infinity as they are not valid JSON values.
            // Note: Protobuf NumberValue uses f64, which loses precision for
            // integers > 2^53. This is a known limitation of the protocol.
            if n.is_nan() || n.is_infinite() {
                return Err("NaN and Infinity are not valid JSON numbers");
            }
            Ok(serde_json::json!(n))
        }
        Some(Kind::StringValue(s)) => Ok(serde_json::Value::String(s)),
        Some(Kind::BoolValue(b)) => Ok(serde_json::Value::Bool(b)),
        Some(Kind::StructValue(s)) => prost_struct_to_json(s),
        Some(Kind::ListValue(l)) => {
            let values: Result<Vec<_>, _> = l.values.into_iter().map(prost_value_to_json).collect();
            Ok(serde_json::Value::Array(values?))
        }
        None => Ok(serde_json::Value::Null),
    }
}

/// Converts a prost_types::Struct to serde_json::Value (takes ownership to avoid clones).
///
/// Returns `Err` if any value contains NaN or Infinity.
fn prost_struct_to_json(s: prost_types::Struct) -> Result<serde_json::Value, &'static str> {
    let mut map = serde_json::Map::new();
    for (k, v) in s.fields {
        map.insert(k, prost_value_to_json(v)?);
    }
    Ok(serde_json::Value::Object(map))
}

/// Converts a prost_types::Struct to HashMap<String, serde_json::Value> (takes ownership).
///
/// Returns `Err` if any value contains NaN or Infinity.
fn prost_struct_to_hashmap(
    s: prost_types::Struct,
) -> Result<std::collections::HashMap<String, serde_json::Value>, &'static str> {
    s.fields
        .into_iter()
        .map(|(k, v)| Ok((k, prost_value_to_json(v)?)))
        .collect()
}

/// Converts a serde_json::Value to prost_types::Value (for Read response).
fn json_to_prost_value(value: serde_json::Value) -> prost_types::Value {
    use prost_types::value::Kind;

    prost_types::Value {
        kind: Some(match value {
            serde_json::Value::Null => Kind::NullValue(0),
            serde_json::Value::Bool(b) => Kind::BoolValue(b),
            serde_json::Value::Number(n) => Kind::NumberValue(n.as_f64().unwrap_or(0.0)),
            serde_json::Value::String(s) => Kind::StringValue(s),
            serde_json::Value::Array(arr) => Kind::ListValue(prost_types::ListValue {
                values: arr.into_iter().map(json_to_prost_value).collect(),
            }),
            serde_json::Value::Object(obj) => Kind::StructValue(json_map_to_prost_struct(obj)),
        }),
    }
}

/// Converts a serde_json Map to prost_types::Struct (for Read response).
fn json_map_to_prost_struct(
    map: serde_json::Map<String, serde_json::Value>,
) -> prost_types::Struct {
    prost_types::Struct {
        fields: map
            .into_iter()
            .map(|(k, v)| (k, json_to_prost_value(v)))
            .collect(),
    }
}

/// Converts a HashMap<String, serde_json::Value> to prost_types::Struct (for Read response).
fn hashmap_to_prost_struct(
    map: std::collections::HashMap<String, serde_json::Value>,
) -> prost_types::Struct {
    prost_types::Struct {
        fields: map
            .into_iter()
            .map(|(k, v)| (k, json_to_prost_value(v)))
            .collect(),
    }
}

// Use shared validation functions from the validation module
use crate::validation::{
    is_valid_condition_name, prost_value_exceeds_max_depth, MAX_CONDITION_CONTEXT_SIZE,
};

/// Error returned when tuple key parsing fails.
/// Contains the original user/object strings for error messages (avoids cloning in happy path).
struct TupleKeyParseError {
    user: String,
    object: String,
    reason: &'static str,
}

/// Converts a TupleKey proto to StoredTuple (takes ownership to avoid clones).
///
/// Uses `parse_user` and `parse_object` for consistent validation across all handlers.
/// Parses the optional condition field including name and context.
///
/// Returns `Err` with the original user/object for error messages (avoids cloning in happy path).
fn tuple_key_to_stored(tk: TupleKey) -> Result<StoredTuple, TupleKeyParseError> {
    let (user_type, user_id, user_relation) =
        parse_user(&tk.user).ok_or_else(|| TupleKeyParseError {
            user: tk.user.clone(),
            object: tk.object.clone(),
            reason: "invalid user format",
        })?;

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

            // Convert and validate context (constraint C11)
            let context = if let Some(ctx) = cond.context {
                // Check depth limit to prevent stack overflow
                if ctx
                    .fields
                    .values()
                    .any(|v| prost_value_exceeds_max_depth(v, 1))
                {
                    return Err(TupleKeyParseError {
                        user: tk.user,
                        object: tk.object,
                        reason: "condition context exceeds maximum nesting depth (10 levels)",
                    });
                }

                // Convert prost Struct to HashMap, rejecting NaN/Infinity values
                let hashmap = prost_struct_to_hashmap(ctx).map_err(|_| TupleKeyParseError {
                    user: tk.user.clone(),
                    object: tk.object.clone(),
                    reason: "condition context contains invalid number (NaN or Infinity)",
                })?;

                // Estimate serialized size to enforce bounds
                let estimated_size: usize = hashmap
                    .iter()
                    .map(|(k, v)| k.len() + v.to_string().len())
                    .sum();
                if estimated_size > MAX_CONDITION_CONTEXT_SIZE {
                    return Err(TupleKeyParseError {
                        user: tk.user,
                        object: tk.object,
                        reason: "condition context exceeds maximum size (10KB)",
                    });
                }
                Some(hashmap)
            } else {
                None
            };

            (Some(cond.name), context)
        }
    } else {
        (None, None)
    };

    Ok(StoredTuple {
        object_type: object_type.to_string(),
        object_id: object_id.to_string(),
        relation: tk.relation,
        user_type: user_type.to_string(),
        user_id: user_id.to_string(),
        user_relation: user_relation.map(|s| s.to_string()),
        condition_name,
        condition_context,
    })
}

/// Converts a chrono DateTime<Utc> to a prost_types::Timestamp.
fn datetime_to_timestamp(dt: DateTime<Utc>) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: dt.timestamp(),
        nanos: dt.timestamp_subsec_nanos() as i32,
    }
}

/// Converts a domain ExpandNode to a proto Node.
fn expand_node_to_proto(node: rsfga_domain::resolver::ExpandNode) -> Node {
    use rsfga_domain::resolver::{ExpandLeafValue, ExpandNode as DomainExpandNode};

    match node {
        DomainExpandNode::Leaf(leaf) => {
            // Extract object from leaf.name (format: "type:id#relation") before moving name
            // The tupleset relation is on the same object being expanded
            let object_for_tupleset = leaf.name.split('#').next().unwrap_or("").to_string();
            Node {
                name: leaf.name,
                value: Some(crate::proto::openfga::v1::node::Value::Leaf(
                    match leaf.value {
                        ExpandLeafValue::Users(users) => Leaf {
                            value: Some(crate::proto::openfga::v1::leaf::Value::Users(Users {
                                users,
                            })),
                        },
                        ExpandLeafValue::Computed { userset } => Leaf {
                            value: Some(crate::proto::openfga::v1::leaf::Value::Computed(
                                Computed { userset },
                            )),
                        },
                        ExpandLeafValue::TupleToUserset {
                            tupleset,
                            computed_userset,
                        } => Leaf {
                            value: Some(crate::proto::openfga::v1::leaf::Value::TupleToUserset(
                                TupleToUserset {
                                    tupleset: Some(ObjectRelation {
                                        object: object_for_tupleset,
                                        relation: tupleset,
                                    }),
                                    // computed_userset object is unknown without further resolution
                                    computed_userset: Some(ObjectRelation {
                                        object: String::new(),
                                        relation: computed_userset,
                                    }),
                                },
                            )),
                        },
                    },
                )),
            }
        }
        DomainExpandNode::Union { name, nodes } => Node {
            name,
            value: Some(crate::proto::openfga::v1::node::Value::Union(Nodes {
                nodes: nodes.into_iter().map(expand_node_to_proto).collect(),
            })),
        },
        DomainExpandNode::Intersection { name, nodes } => Node {
            name,
            value: Some(crate::proto::openfga::v1::node::Value::Intersection(
                Nodes {
                    nodes: nodes.into_iter().map(expand_node_to_proto).collect(),
                },
            )),
        },
        DomainExpandNode::Difference {
            name,
            base,
            subtract,
        } => {
            // Note: OpenFGA proto uses Nodes for difference (with 2 nodes: base, subtract)
            // This matches OpenFGA's representation where difference.nodes[0] is base
            // and difference.nodes[1] is subtract
            Node {
                name,
                value: Some(crate::proto::openfga::v1::node::Value::Difference(Nodes {
                    nodes: vec![expand_node_to_proto(*base), expand_node_to_proto(*subtract)],
                })),
            }
        }
    }
}

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
        use crate::validation::MAX_LIST_OBJECTS_CANDIDATES;
        use rsfga_domain::resolver::ListObjectsRequest as DomainListObjectsRequest;

        let req = request.into_inner();

        // Convert contextual tuples if provided
        let contextual_tuples = req
            .contextual_tuples
            .map(|ct| {
                ct.tuple_keys
                    .into_iter()
                    .filter_map(|tk| {
                        let (user_type, user_id, user_relation) = parse_user(&tk.user)?;
                        let (object_type, object_id) = parse_object(&tk.object)?;
                        Some(rsfga_domain::resolver::ContextualTuple::new(
                            format_user(user_type, user_id, user_relation),
                            tk.relation,
                            format!("{object_type}:{object_id}"),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Create domain request
        let context = req
            .context
            .map(prost_struct_to_hashmap)
            .transpose()
            .map_err(|e| Status::invalid_argument(format!("invalid context: {}", e)))?
            .unwrap_or_default();

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
        }))
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
