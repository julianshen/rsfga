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
    DataStore, DateTime, PaginationOptions, StorageError, StoredAuthorizationModel, StoredTuple,
    TupleFilter, Utc,
};

use crate::adapters::{DataStoreModelReader, DataStoreTupleReader};
use crate::http::state::{
    AssertionCondition, AssertionKey, AssertionTupleKey, ContextualTuplesWrapper, StoredAssertion,
};
use crate::utils::{format_user, parse_object, parse_user};

use crate::proto::openfga::v1::{
    open_fga_service_server::OpenFgaService, user, Assertion, AuthorizationModel,
    BatchCheckRequest, BatchCheckResponse, BatchCheckSingleResult, CheckError, CheckRequest,
    CheckResponse, Computed, Condition, ConditionMetadata, ConditionParamTypeRef,
    CreateStoreRequest, CreateStoreResponse, DeleteStoreRequest, DeleteStoreResponse, Difference,
    DirectUserset, ErrorCode, ExpandRequest, ExpandResponse, FgaObject, GetStoreRequest,
    GetStoreResponse, Leaf, ListObjectsRequest, ListObjectsResponse, ListStoresRequest,
    ListStoresResponse, ListUsersRequest, ListUsersResponse, Metadata, Node, Nodes, ObjectRelation,
    ReadAssertionsRequest, ReadAssertionsResponse, ReadAuthorizationModelRequest,
    ReadAuthorizationModelResponse, ReadAuthorizationModelsRequest,
    ReadAuthorizationModelsResponse, ReadChangesRequest, ReadChangesResponse, ReadRequest,
    ReadResponse, RelationMetadata, RelationReference, RelationshipCondition, Store, Tuple,
    TupleKey, TupleToUserset, TypeDefinition, TypeName, TypedWildcard, UpdateStoreRequest,
    UpdateStoreResponse, User, Users, Userset, UsersetTree, UsersetUser, Usersets, Wildcard,
    WriteAssertionsRequest, WriteAssertionsResponse, WriteAuthorizationModelRequest,
    WriteAuthorizationModelResponse, WriteRequest, WriteResponse,
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
        created_at: None,
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

// ============================================================
// Proto-JSON Conversion Functions for Authorization Models
// ============================================================
// These functions convert between proto TypeDefinition/Condition and JSON
// to maintain storage format compatibility with the HTTP layer.

/// Converts a proto TypeDefinition to JSON for storage.
fn type_definition_to_json(td: &TypeDefinition) -> serde_json::Value {
    let mut obj = serde_json::json!({
        "type": td.r#type,
    });

    // Convert relations map
    if !td.relations.is_empty() {
        let relations: serde_json::Map<String, serde_json::Value> = td
            .relations
            .iter()
            .map(|(k, v)| (k.clone(), userset_to_json(v)))
            .collect();
        obj["relations"] = serde_json::Value::Object(relations);
    }

    // Convert metadata if present
    if let Some(ref metadata) = td.metadata {
        obj["metadata"] = metadata_to_json(metadata);
    }

    obj
}

/// Converts a proto Userset to JSON.
fn userset_to_json(us: &Userset) -> serde_json::Value {
    use crate::proto::openfga::v1::userset::Userset as US;
    match &us.userset {
        Some(US::This(_)) => serde_json::json!({ "this": {} }),
        Some(US::ComputedUserset(or)) => serde_json::json!({
            "computedUserset": {
                "relation": or.relation
            }
        }),
        Some(US::TupleToUserset(ttu)) => {
            let mut obj = serde_json::json!({});
            if let Some(ref ts) = ttu.tupleset {
                obj["tupleset"] = serde_json::json!({ "relation": ts.relation });
            }
            if let Some(ref cus) = ttu.computed_userset {
                obj["computedUserset"] = serde_json::json!({ "relation": cus.relation });
            }
            serde_json::json!({ "tupleToUserset": obj })
        }
        Some(US::Union(children)) => serde_json::json!({
            "union": {
                "child": children.child.iter().map(userset_to_json).collect::<Vec<_>>()
            }
        }),
        Some(US::Intersection(children)) => serde_json::json!({
            "intersection": {
                "child": children.child.iter().map(userset_to_json).collect::<Vec<_>>()
            }
        }),
        Some(US::Difference(diff)) => {
            let mut obj = serde_json::json!({});
            if let Some(ref base) = diff.base {
                obj["base"] = userset_to_json(base);
            }
            if let Some(ref subtract) = diff.subtract {
                obj["subtract"] = userset_to_json(subtract);
            }
            serde_json::json!({ "difference": obj })
        }
        None => serde_json::json!({}),
    }
}

/// Converts proto Metadata to JSON.
fn metadata_to_json(md: &Metadata) -> serde_json::Value {
    let mut obj = serde_json::json!({});
    if !md.relations.is_empty() {
        let relations: serde_json::Map<String, serde_json::Value> = md
            .relations
            .iter()
            .map(|(k, v)| (k.clone(), relation_metadata_to_json(v)))
            .collect();
        obj["relations"] = serde_json::Value::Object(relations);
    }
    if !md.module.is_empty() {
        obj["module"] = serde_json::Value::String(md.module.clone());
    }
    if !md.source_info.is_empty() {
        obj["source_info"] = serde_json::Value::String(md.source_info.clone());
    }
    obj
}

/// Converts proto RelationMetadata to JSON.
fn relation_metadata_to_json(rm: &RelationMetadata) -> serde_json::Value {
    let mut obj = serde_json::json!({});
    if !rm.directly_related_user_types.is_empty() {
        obj["directly_related_user_types"] = serde_json::json!(rm
            .directly_related_user_types
            .iter()
            .map(relation_reference_to_json)
            .collect::<Vec<_>>());
    }
    if !rm.module.is_empty() {
        obj["module"] = serde_json::Value::String(rm.module.clone());
    }
    if !rm.source_info.is_empty() {
        obj["source_info"] = serde_json::Value::String(rm.source_info.clone());
    }
    obj
}

/// Converts proto RelationReference to JSON.
fn relation_reference_to_json(rr: &RelationReference) -> serde_json::Value {
    use crate::proto::openfga::v1::relation_reference::RelationOrWildcard;
    let mut obj = serde_json::json!({ "type": rr.r#type });
    match &rr.relation_or_wildcard {
        Some(RelationOrWildcard::Relation(rel)) => {
            obj["relation"] = serde_json::Value::String(rel.clone());
        }
        Some(RelationOrWildcard::Wildcard(_)) => {
            obj["wildcard"] = serde_json::json!({});
        }
        None => {}
    }
    if !rr.condition.is_empty() {
        obj["condition"] = serde_json::Value::String(rr.condition.clone());
    }
    obj
}

/// Converts a proto Condition to JSON for storage.
fn condition_to_json(cond: &Condition) -> serde_json::Value {
    let mut obj = serde_json::json!({
        "name": cond.name,
        "expression": cond.expression,
    });
    if !cond.parameters.is_empty() {
        let params: serde_json::Map<String, serde_json::Value> = cond
            .parameters
            .iter()
            .map(|(k, v)| (k.clone(), condition_param_to_json(v)))
            .collect();
        obj["parameters"] = serde_json::Value::Object(params);
    }
    if let Some(ref md) = cond.metadata {
        let mut md_obj = serde_json::json!({});
        if !md.module.is_empty() {
            md_obj["module"] = serde_json::Value::String(md.module.clone());
        }
        if !md.source_info.is_empty() {
            md_obj["source_info"] = serde_json::Value::String(md.source_info.clone());
        }
        obj["metadata"] = md_obj;
    }
    obj
}

/// Converts proto ConditionParamTypeRef to JSON.
fn condition_param_to_json(cp: &ConditionParamTypeRef) -> serde_json::Value {
    let type_name = type_name_to_string(cp.type_name);
    let mut obj = serde_json::json!({ "type_name": type_name });
    if !cp.generic_types.is_empty() {
        obj["generic_types"] = serde_json::json!(cp
            .generic_types
            .iter()
            .map(|t| type_name_to_string(*t))
            .collect::<Vec<_>>());
    }
    obj
}

/// Converts proto TypeName enum to string.
fn type_name_to_string(tn: i32) -> String {
    match TypeName::try_from(tn) {
        Ok(TypeName::Unspecified) => "TYPE_NAME_UNSPECIFIED",
        Ok(TypeName::Any) => "TYPE_NAME_ANY",
        Ok(TypeName::Bool) => "TYPE_NAME_BOOL",
        Ok(TypeName::String) => "TYPE_NAME_STRING",
        Ok(TypeName::Int) => "TYPE_NAME_INT",
        Ok(TypeName::Uint) => "TYPE_NAME_UINT",
        Ok(TypeName::Double) => "TYPE_NAME_DOUBLE",
        Ok(TypeName::Duration) => "TYPE_NAME_DURATION",
        Ok(TypeName::Timestamp) => "TYPE_NAME_TIMESTAMP",
        Ok(TypeName::Map) => "TYPE_NAME_MAP",
        Ok(TypeName::List) => "TYPE_NAME_LIST",
        Ok(TypeName::Ipaddress) => "TYPE_NAME_IPADDRESS",
        Err(_) => "TYPE_NAME_UNSPECIFIED",
    }
    .to_string()
}

/// Parses a JSON type definition into a proto TypeDefinition.
fn json_to_type_definition(json: &serde_json::Value) -> Option<TypeDefinition> {
    let type_name = json.get("type")?.as_str()?.to_string();
    let relations = json
        .get("relations")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| json_to_userset(v).map(|us| (k.clone(), us)))
                .collect()
        })
        .unwrap_or_default();

    let metadata = json.get("metadata").and_then(json_to_metadata);

    Some(TypeDefinition {
        r#type: type_name,
        relations,
        metadata,
    })
}

/// Parses JSON into a proto Userset.
fn json_to_userset(json: &serde_json::Value) -> Option<Userset> {
    use crate::proto::openfga::v1::userset::Userset as US;

    if json.get("this").is_some() {
        return Some(Userset {
            userset: Some(US::This(DirectUserset {})),
        });
    }

    if let Some(cu) = json.get("computedUserset") {
        let relation = cu.get("relation")?.as_str()?.to_string();
        return Some(Userset {
            userset: Some(US::ComputedUserset(ObjectRelation {
                object: String::new(),
                relation,
            })),
        });
    }

    if let Some(ttu) = json.get("tupleToUserset") {
        let tupleset = ttu.get("tupleset").and_then(|ts| {
            Some(ObjectRelation {
                object: String::new(),
                relation: ts.get("relation")?.as_str()?.to_string(),
            })
        });
        let computed_userset = ttu.get("computedUserset").and_then(|cus| {
            Some(ObjectRelation {
                object: String::new(),
                relation: cus.get("relation")?.as_str()?.to_string(),
            })
        });
        return Some(Userset {
            userset: Some(US::TupleToUserset(TupleToUserset {
                tupleset,
                computed_userset,
            })),
        });
    }

    if let Some(union) = json.get("union") {
        let children = union
            .get("child")
            .and_then(|c| c.as_array())
            .map(|arr| arr.iter().filter_map(json_to_userset).collect())
            .unwrap_or_default();
        return Some(Userset {
            userset: Some(US::Union(Usersets { child: children })),
        });
    }

    if let Some(intersection) = json.get("intersection") {
        let children = intersection
            .get("child")
            .and_then(|c| c.as_array())
            .map(|arr| arr.iter().filter_map(json_to_userset).collect())
            .unwrap_or_default();
        return Some(Userset {
            userset: Some(US::Intersection(Usersets { child: children })),
        });
    }

    if let Some(diff) = json.get("difference") {
        let base = diff.get("base").and_then(json_to_userset).map(Box::new);
        let subtract = diff.get("subtract").and_then(json_to_userset).map(Box::new);
        return Some(Userset {
            userset: Some(US::Difference(Box::new(Difference { base, subtract }))),
        });
    }

    None
}

/// Parses JSON into proto Metadata.
fn json_to_metadata(json: &serde_json::Value) -> Option<Metadata> {
    let relations = json
        .get("relations")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| json_to_relation_metadata(v).map(|rm| (k.clone(), rm)))
                .collect()
        })
        .unwrap_or_default();

    let module = json
        .get("module")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let source_info = json
        .get("source_info")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Some(Metadata {
        relations,
        module,
        source_info,
    })
}

/// Parses JSON into proto RelationMetadata.
fn json_to_relation_metadata(json: &serde_json::Value) -> Option<RelationMetadata> {
    let directly_related_user_types = json
        .get("directly_related_user_types")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(json_to_relation_reference).collect())
        .unwrap_or_default();

    let module = json
        .get("module")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let source_info = json
        .get("source_info")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Some(RelationMetadata {
        directly_related_user_types,
        module,
        source_info,
    })
}

/// Parses JSON into proto RelationReference.
fn json_to_relation_reference(json: &serde_json::Value) -> Option<RelationReference> {
    use crate::proto::openfga::v1::relation_reference::RelationOrWildcard;

    let type_name = json.get("type")?.as_str()?.to_string();
    let relation_or_wildcard = if json.get("wildcard").is_some() {
        Some(RelationOrWildcard::Wildcard(Wildcard {}))
    } else {
        json.get("relation")
            .and_then(|r| r.as_str())
            .map(|s| RelationOrWildcard::Relation(s.to_string()))
    };
    let condition = json
        .get("condition")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Some(RelationReference {
        r#type: type_name,
        relation_or_wildcard,
        condition,
    })
}

/// Parses JSON into a proto Condition.
fn json_to_condition(json: &serde_json::Value) -> Option<Condition> {
    let name = json.get("name")?.as_str()?.to_string();
    let expression = json
        .get("expression")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let parameters = json
        .get("parameters")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| json_to_condition_param(v).map(|cp| (k.clone(), cp)))
                .collect()
        })
        .unwrap_or_default();

    let metadata = json.get("metadata").map(|md| ConditionMetadata {
        module: md
            .get("module")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        source_info: md
            .get("source_info")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    });

    Some(Condition {
        name,
        expression,
        parameters,
        metadata,
    })
}

/// Parses JSON into proto ConditionParamTypeRef.
fn json_to_condition_param(json: &serde_json::Value) -> Option<ConditionParamTypeRef> {
    let type_name = json
        .get("type_name")
        .and_then(|v| v.as_str())
        .map(string_to_type_name)
        .unwrap_or(TypeName::Unspecified as i32);
    let generic_types = json
        .get("generic_types")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(string_to_type_name)
                .collect()
        })
        .unwrap_or_default();

    Some(ConditionParamTypeRef {
        type_name,
        generic_types,
    })
}

/// Converts string to proto TypeName enum value.
fn string_to_type_name(s: &str) -> i32 {
    match s {
        "TYPE_NAME_ANY" => TypeName::Any as i32,
        "TYPE_NAME_BOOL" => TypeName::Bool as i32,
        "TYPE_NAME_STRING" => TypeName::String as i32,
        "TYPE_NAME_INT" => TypeName::Int as i32,
        "TYPE_NAME_UINT" => TypeName::Uint as i32,
        "TYPE_NAME_DOUBLE" => TypeName::Double as i32,
        "TYPE_NAME_DURATION" => TypeName::Duration as i32,
        "TYPE_NAME_TIMESTAMP" => TypeName::Timestamp as i32,
        "TYPE_NAME_MAP" => TypeName::Map as i32,
        "TYPE_NAME_LIST" => TypeName::List as i32,
        "TYPE_NAME_IPADDRESS" => TypeName::Ipaddress as i32,
        _ => TypeName::Unspecified as i32,
    }
}

/// Converts a StoredAuthorizationModel to a proto AuthorizationModel.
///
/// Logs warnings when individual type definitions or conditions fail to parse,
/// instead of silently dropping them. This helps diagnose data corruption issues.
fn stored_model_to_proto(stored: &StoredAuthorizationModel) -> Option<AuthorizationModel> {
    // Parse the stored JSON
    let parsed: serde_json::Value = match serde_json::from_str(&stored.model_json) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                model_id = %stored.id,
                store_id = %stored.store_id,
                error = %e,
                "Failed to parse authorization model JSON"
            );
            return None;
        }
    };

    // Extract and convert type_definitions with logging for failures
    let type_definitions: Vec<TypeDefinition> = parsed
        .get("type_definitions")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .enumerate()
                .filter_map(|(idx, td_json)| match json_to_type_definition(td_json) {
                    Some(td) => Some(td),
                    None => {
                        tracing::warn!(
                            model_id = %stored.id,
                            store_id = %stored.store_id,
                            type_def_index = idx,
                            type_def_json = %td_json,
                            "Failed to parse type definition in authorization model"
                        );
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    // Extract and convert conditions with logging for failures
    let conditions: std::collections::HashMap<String, Condition> = parsed
        .get("conditions")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| match json_to_condition(v) {
                    Some(c) => Some((k.clone(), c)),
                    None => {
                        tracing::warn!(
                            model_id = %stored.id,
                            store_id = %stored.store_id,
                            condition_name = %k,
                            condition_json = %v,
                            "Failed to parse condition in authorization model"
                        );
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    Some(AuthorizationModel {
        id: stored.id.clone(),
        schema_version: stored.schema_version.clone(),
        type_definitions,
        conditions,
    })
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

        // Validate model exists
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

        // Validate model exists
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
        // Preserves conditions on tuple keys to avoid data loss.
        let stored_assertions: Vec<StoredAssertion> = req
            .assertions
            .into_iter()
            .filter_map(|a| {
                let tuple_key = a.tuple_key?;
                Some(StoredAssertion {
                    tuple_key: AssertionTupleKey {
                        user: tuple_key.user,
                        relation: tuple_key.relation,
                        object: tuple_key.object,
                        condition: tuple_key.condition.map(|c| {
                            let condition_name = c.name;
                            let context = c
                                .context
                                .map(prost_struct_to_hashmap)
                                .transpose()
                                .map_err(|e| {
                                    tracing::warn!(
                                        condition_name = %condition_name,
                                        error = %e,
                                        "Failed to convert assertion condition context"
                                    );
                                })
                                .ok()
                                .flatten();
                            AssertionCondition {
                                name: condition_name,
                                context,
                            }
                        }),
                    },
                    expectation: a.expectation,
                    contextual_tuples: if a.contextual_tuples.is_empty() {
                        None
                    } else {
                        Some(ContextualTuplesWrapper {
                            tuple_keys: a
                                .contextual_tuples
                                .into_iter()
                                .map(|tk| AssertionTupleKey {
                                    user: tk.user,
                                    relation: tk.relation,
                                    object: tk.object,
                                    condition: tk.condition.map(|c| {
                                        let condition_name = c.name;
                                        let context = c
                                            .context
                                            .map(prost_struct_to_hashmap)
                                            .transpose()
                                            .map_err(|e| {
                                                tracing::warn!(
                                                    condition_name = %condition_name,
                                                    error = %e,
                                                    "Failed to convert contextual tuple condition context"
                                                );
                                            })
                                            .ok()
                                            .flatten();
                                        AssertionCondition {
                                            name: condition_name,
                                            context,
                                        }
                                    }),
                                })
                                .collect(),
                        })
                    },
                })
            })
            .collect();

        // Store assertions (replaces existing)
        let key = (req.store_id, req.authorization_model_id);
        self.assertions.insert(key, stored_assertions);

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
