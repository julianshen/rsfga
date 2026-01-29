//! DataStore trait definition.

use async_trait::async_trait;

use crate::error::{HealthStatus, StorageError, StorageResult};

// Per-field maximum lengths matching OpenFGA specification.
// These limits ensure API compatibility (Invariant I2).

/// Maximum length for type names (object_type, user_type).
/// OpenFGA spec: 254 characters.
pub const MAX_TYPE_LENGTH: usize = 254;

/// Maximum length for object IDs.
/// OpenFGA spec: 256 characters.
pub const MAX_OBJECT_ID_LENGTH: usize = 256;

/// Maximum length for relation names.
/// OpenFGA spec: 50 characters.
pub const MAX_RELATION_LENGTH: usize = 50;

/// Maximum length for user identifiers.
/// OpenFGA spec: 512 bytes (for full "type:id" or "type:id#relation" format).
/// This applies to user_id field which holds just the ID part.
pub const MAX_USER_ID_LENGTH: usize = 512;

/// Maximum length for store IDs (ULID format is 26 chars).
pub const MAX_STORE_ID_LENGTH: usize = 26;

/// Maximum length for store names.
pub const MAX_STORE_NAME_LENGTH: usize = 256;

/// Maximum length for condition names.
pub const MAX_CONDITION_NAME_LENGTH: usize = 256;

/// Validate a store ID.
///
/// # Errors
/// Returns `StorageError::InvalidInput` if the store ID is empty or too long.
pub fn validate_store_id(store_id: &str) -> StorageResult<()> {
    if store_id.is_empty() {
        return Err(StorageError::InvalidInput {
            message: "store_id cannot be empty".to_string(),
        });
    }
    if store_id.len() > MAX_STORE_ID_LENGTH {
        return Err(StorageError::InvalidInput {
            message: format!("store_id exceeds maximum length of {MAX_STORE_ID_LENGTH} characters"),
        });
    }
    Ok(())
}

/// Parse a continuation token into an offset value.
///
/// # Arguments
/// * `token` - Optional continuation token string (should be a numeric offset)
///
/// # Returns
/// * `Ok(0)` if token is None (start from beginning)
/// * `Ok(offset)` if token is a valid non-negative integer
/// * `Err(StorageError::InvalidInput)` if token is present but invalid
///
/// # Errors
/// Returns `StorageError::InvalidInput` if the token is not a valid non-negative integer.
pub fn parse_continuation_token(token: &Option<String>) -> StorageResult<i64> {
    match token {
        None => Ok(0),
        Some(t) if t.is_empty() => Ok(0),
        Some(t) => t
            .parse::<i64>()
            .map_err(|_| StorageError::InvalidInput {
                message: format!(
                    "invalid continuation_token: '{t}' (must be a non-negative integer)"
                ),
            })
            .and_then(|v| {
                if v < 0 {
                    Err(StorageError::InvalidInput {
                        message: format!(
                            "invalid continuation_token: '{t}' (must be non-negative)"
                        ),
                    })
                } else {
                    Ok(v)
                }
            }),
    }
}

/// Result of get_objects_with_parents containing object ID and optional condition info.
///
/// This struct is returned by `get_objects_with_parents` to enable proper condition
/// evaluation for TupleToUserset relations. Without condition info, the ReverseExpand
/// algorithm would grant access to objects whose tupleset tuples have failing conditions.
///
/// # Authorization Correctness (Invariant I1)
///
/// When a tuple has a condition (e.g., `document:123#parent@folder:456[valid_until: {...}]`),
/// the condition must be evaluated before granting access. Returning only object IDs would
/// bypass condition evaluation, potentially granting unauthorized access.
#[derive(Debug, Clone)]
pub struct ObjectWithCondition {
    /// The object ID (e.g., "doc1" not "document:doc1").
    pub object_id: String,
    /// Optional condition name that must be satisfied for this tuple.
    pub condition_name: Option<String>,
    /// Optional condition context (parameters) as JSON key-value pairs.
    pub condition_context: Option<std::collections::HashMap<String, serde_json::Value>>,
}

impl ObjectWithCondition {
    /// Creates a new ObjectWithCondition without a condition.
    pub fn new(object_id: impl Into<String>) -> Self {
        Self {
            object_id: object_id.into(),
            condition_name: None,
            condition_context: None,
        }
    }

    /// Creates a new ObjectWithCondition with a condition.
    pub fn with_condition(
        object_id: impl Into<String>,
        condition_name: impl Into<String>,
        condition_context: Option<std::collections::HashMap<String, serde_json::Value>>,
    ) -> Self {
        Self {
            object_id: object_id.into(),
            condition_name: Some(condition_name.into()),
            condition_context,
        }
    }
}

/// Cursor for tuple pagination using composite key.
///
/// Enables efficient cursor-based pagination by encoding the last tuple's
/// sort key. This allows O(log N) binary search to find the starting
/// position instead of O(N) offset-based skip.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TupleCursor {
    pub object_type: String,
    pub object_id: String,
    pub relation: String,
    pub user_type: String,
    pub user_id: String,
}

impl TupleCursor {
    /// Creates a cursor from a stored tuple.
    pub fn from_tuple(tuple: &StoredTuple) -> Self {
        Self {
            object_type: tuple.object_type.clone(),
            object_id: tuple.object_id.clone(),
            relation: tuple.relation.clone(),
            user_type: tuple.user_type.clone(),
            user_id: tuple.user_id.clone(),
        }
    }

    /// Encodes the cursor as a base64 string for use as continuation_token.
    pub fn encode(&self) -> String {
        use base64::Engine;
        let json = serde_json::to_string(self).expect("cursor serialization should not fail");
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json.as_bytes())
    }

    /// Decodes a cursor from a base64-encoded continuation_token.
    pub fn decode(token: &str) -> StorageResult<Self> {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(token)
            .map_err(|_| StorageError::InvalidInput {
                message: "invalid continuation_token: base64 decode failed".to_string(),
            })?;
        serde_json::from_slice(&bytes).map_err(|_| StorageError::InvalidInput {
            message: "invalid continuation_token: JSON decode failed".to_string(),
        })
    }

    /// Returns the sort key tuple for comparison.
    pub fn sort_key(&self) -> (&str, &str, &str, &str, &str) {
        (
            &self.object_type,
            &self.object_id,
            &self.relation,
            &self.user_type,
            &self.user_id,
        )
    }
}

/// Parse a cursor-based continuation token for tuples.
///
/// Returns None if the token is empty (start from beginning).
pub fn parse_tuple_cursor(token: &Option<String>) -> StorageResult<Option<TupleCursor>> {
    match token {
        None => Ok(None),
        Some(t) if t.is_empty() => Ok(None),
        Some(t) => TupleCursor::decode(t).map(Some),
    }
}

/// Validate a store name.
///
/// # Errors
/// Returns `StorageError::InvalidInput` if the name is empty or too long.
pub fn validate_store_name(name: &str) -> StorageResult<()> {
    if name.is_empty() {
        return Err(StorageError::InvalidInput {
            message: "store name cannot be empty".to_string(),
        });
    }
    if name.len() > MAX_STORE_NAME_LENGTH {
        return Err(StorageError::InvalidInput {
            message: format!(
                "store name exceeds maximum length of {MAX_STORE_NAME_LENGTH} characters"
            ),
        });
    }
    Ok(())
}

/// Validate object type format.
///
/// # Errors
/// Returns `StorageError::InvalidInput` if the type is empty or too long.
pub fn validate_object_type(object_type: &str) -> StorageResult<()> {
    if object_type.is_empty() {
        return Err(StorageError::InvalidInput {
            message: "object_type cannot be empty".to_string(),
        });
    }
    if object_type.len() > MAX_TYPE_LENGTH {
        return Err(StorageError::InvalidInput {
            message: format!("object_type exceeds maximum length of {MAX_TYPE_LENGTH} characters"),
        });
    }
    // SQL Injection prevention: ensure no colons or special characters
    // Only allow alphanumeric, underscore, and dash
    if object_type.contains(':')
        || !object_type
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(StorageError::InvalidInput {
            message: format!("object_type contains invalid characters: {}", object_type),
        });
    }
    Ok(())
}

/// Validate a stored tuple at the storage layer.
///
/// This performs **structural validation** only:
/// - Field presence (required fields must not be empty)
/// - Field length (per-field limits matching OpenFGA specification)
///
/// # Per-Field Limits (OpenFGA Specification)
///
/// | Field          | Max Length | Notes                        |
/// |----------------|------------|------------------------------|
/// | object_type    | 254        | Type name limit              |
/// | object_id      | 256        | Object identifier limit      |
/// | relation       | 50         | Relation name limit          |
/// | user_type      | 254        | Type name limit              |
/// | user_id        | 512        | User identifier limit        |
/// | user_relation  | 50         | Relation name limit          |
/// | condition_name | 256        | Condition name limit         |
///
/// # Validation Strategy
///
/// Condition names are validated in two stages:
///
/// 1. **Write-time (this function)**: Validates that `condition_name` is non-empty
///    if provided and doesn't exceed length limits. Does NOT verify the condition
///    exists in the authorization model.
///
/// 2. **Check-time (graph_resolver)**: When evaluating permissions, the resolver
///    looks up the condition by name. If not found, returns an error. This allows
///    tuples to be written before conditions are defined, supporting flexible
///    deployment ordering.
///
/// This two-stage approach matches OpenFGA's behavior where:
/// - Tuples can reference conditions not yet in the model
/// - Errors surface at authorization check time, not write time
/// - Model updates don't require tuple rewrites
///
/// # Errors
///
/// Returns `StorageError::InvalidInput` if any field is empty or too long.
pub fn validate_tuple(tuple: &StoredTuple) -> StorageResult<()> {
    validate_object_type(&tuple.object_type)?;

    if tuple.object_id.is_empty() {
        return Err(StorageError::InvalidInput {
            message: "object_id cannot be empty".to_string(),
        });
    }
    if tuple.object_id.len() > MAX_OBJECT_ID_LENGTH {
        return Err(StorageError::InvalidInput {
            message: format!(
                "object_id exceeds maximum length of {MAX_OBJECT_ID_LENGTH} characters"
            ),
        });
    }
    if tuple.relation.is_empty() {
        return Err(StorageError::InvalidInput {
            message: "relation cannot be empty".to_string(),
        });
    }
    if tuple.relation.len() > MAX_RELATION_LENGTH {
        return Err(StorageError::InvalidInput {
            message: format!("relation exceeds maximum length of {MAX_RELATION_LENGTH} characters"),
        });
    }
    if tuple.user_type.is_empty() {
        return Err(StorageError::InvalidInput {
            message: "user_type cannot be empty".to_string(),
        });
    }
    if tuple.user_type.len() > MAX_TYPE_LENGTH {
        return Err(StorageError::InvalidInput {
            message: format!("user_type exceeds maximum length of {MAX_TYPE_LENGTH} characters"),
        });
    }
    if tuple.user_id.is_empty() {
        return Err(StorageError::InvalidInput {
            message: "user_id cannot be empty".to_string(),
        });
    }
    if tuple.user_id.len() > MAX_USER_ID_LENGTH {
        return Err(StorageError::InvalidInput {
            message: format!("user_id exceeds maximum length of {MAX_USER_ID_LENGTH} characters"),
        });
    }
    if let Some(ref user_relation) = tuple.user_relation {
        if user_relation.is_empty() {
            return Err(StorageError::InvalidInput {
                message: "user_relation cannot be empty if provided".to_string(),
            });
        }
        if user_relation.len() > MAX_RELATION_LENGTH {
            return Err(StorageError::InvalidInput {
                message: format!(
                    "user_relation exceeds maximum length of {MAX_RELATION_LENGTH} characters"
                ),
            });
        }
    }
    if let Some(ref condition_name) = tuple.condition_name {
        if condition_name.is_empty() {
            return Err(StorageError::InvalidInput {
                message: "condition_name cannot be empty if provided".to_string(),
            });
        }
        if condition_name.len() > MAX_CONDITION_NAME_LENGTH {
            return Err(StorageError::InvalidInput {
                message: format!(
                    "condition_name exceeds maximum length of {MAX_CONDITION_NAME_LENGTH} characters"
                ),
            });
        }
    }
    Ok(())
}

/// Filter for reading tuples.
#[derive(Debug, Clone, Default)]
pub struct TupleFilter {
    /// Filter by object type.
    pub object_type: Option<String>,
    /// Filter by object ID.
    pub object_id: Option<String>,
    /// Filter by relation.
    pub relation: Option<String>,
    /// Filter by user.
    ///
    /// Expected format: `"type:id"` for direct users, or `"type:id#relation"` for usersets.
    ///
    /// # Examples
    /// - `"user:alice"` - Direct user reference
    /// - `"group:engineering#member"` - Userset reference (members of engineering group)
    ///
    /// # Errors
    /// Invalid formats will result in `StorageError::InvalidFilter` when the filter is applied.
    pub user: Option<String>,
    /// Filter by condition name.
    ///
    /// When set, only returns tuples that have this specific condition attached.
    pub condition_name: Option<String>,
}

/// Parse user filter string into (user_type, user_id, Option<user_relation>).
///
/// # Format
/// - `"type:id"` for direct users
/// - `"type:id#relation"` for usersets
///
/// # Errors
/// Returns `StorageError::InvalidFilter` if the format is invalid.
///
/// # Examples
/// ```
/// use rsfga_storage::traits::parse_user_filter;
///
/// // Direct user
/// let (user_type, user_id, relation) = parse_user_filter("user:alice").unwrap();
/// assert_eq!(user_type, "user");
/// assert_eq!(user_id, "alice");
/// assert!(relation.is_none());
///
/// // Userset
/// let (user_type, user_id, relation) = parse_user_filter("group:eng#member").unwrap();
/// assert_eq!(user_type, "group");
/// assert_eq!(user_id, "eng");
/// assert_eq!(relation, Some("member".to_string()));
/// ```
pub fn parse_user_filter(user: &str) -> StorageResult<(String, String, Option<String>)> {
    if user.contains('#') {
        let parts: Vec<&str> = user.split('#').collect();
        if parts.len() != 2 || parts[1].is_empty() {
            return Err(StorageError::InvalidFilter {
                message: format!(
                    "Invalid user filter format: '{user}'. Expected 'type:id#relation'"
                ),
            });
        }
        let user_parts: Vec<&str> = parts[0].split(':').collect();
        if user_parts.len() != 2 || user_parts[0].is_empty() || user_parts[1].is_empty() {
            return Err(StorageError::InvalidFilter {
                message: format!(
                    "Invalid user filter format: '{user}'. Expected 'type:id#relation'"
                ),
            });
        }
        Ok((
            user_parts[0].to_string(),
            user_parts[1].to_string(),
            Some(parts[1].to_string()),
        ))
    } else {
        let user_parts: Vec<&str> = user.split(':').collect();
        if user_parts.len() != 2 || user_parts[0].is_empty() || user_parts[1].is_empty() {
            return Err(StorageError::InvalidFilter {
                message: format!("Invalid user filter format: '{user}'. Expected 'type:id'"),
            });
        }
        Ok((user_parts[0].to_string(), user_parts[1].to_string(), None))
    }
}

/// A stored tuple.
///
/// Tuples can optionally have a condition name and condition context.
/// When a condition is specified, the tuple is only considered valid
/// if the condition evaluates to true during authorization checks.
///
/// Note: Hash is implemented manually because HashMap<String, serde_json::Value>
/// doesn't implement Hash. The condition_context is included in both PartialEq
/// and Hash - tuples with different contexts are distinct and both stored.
#[derive(Debug, Clone)]
pub struct StoredTuple {
    pub object_type: String,
    pub object_id: String,
    pub relation: String,
    pub user_type: String,
    pub user_id: String,
    pub user_relation: Option<String>,
    /// Optional condition name that must be satisfied for this tuple.
    pub condition_name: Option<String>,
    /// Optional condition context (parameters) as JSON key-value pairs.
    /// Only meaningful when condition_name is set.
    pub condition_context: Option<std::collections::HashMap<String, serde_json::Value>>,
    /// Timestamp when this tuple was created (OpenFGA compatibility).
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl StoredTuple {
    /// Creates a new StoredTuple without condition.
    pub fn new(
        object_type: impl Into<String>,
        object_id: impl Into<String>,
        relation: impl Into<String>,
        user_type: impl Into<String>,
        user_id: impl Into<String>,
        user_relation: Option<String>,
    ) -> Self {
        Self {
            object_type: object_type.into(),
            object_id: object_id.into(),
            relation: relation.into(),
            user_type: user_type.into(),
            user_id: user_id.into(),
            user_relation,
            condition_name: None,
            condition_context: None,
            created_at: None,
        }
    }

    /// Creates a new StoredTuple with a condition.
    #[allow(clippy::too_many_arguments)]
    pub fn with_condition(
        object_type: impl Into<String>,
        object_id: impl Into<String>,
        relation: impl Into<String>,
        user_type: impl Into<String>,
        user_id: impl Into<String>,
        user_relation: Option<String>,
        condition_name: impl Into<String>,
        condition_context: Option<std::collections::HashMap<String, serde_json::Value>>,
    ) -> Self {
        Self {
            object_type: object_type.into(),
            object_id: object_id.into(),
            relation: relation.into(),
            user_type: user_type.into(),
            user_id: user_id.into(),
            user_relation,
            condition_name: Some(condition_name.into()),
            condition_context,
            created_at: None,
        }
    }

    /// Returns the tuple key (excludes condition context for comparison).
    fn key(&self) -> (&str, &str, &str, &str, &str, Option<&str>, Option<&str>) {
        (
            &self.object_type,
            &self.object_id,
            &self.relation,
            &self.user_type,
            &self.user_id,
            self.user_relation.as_deref(),
            self.condition_name.as_deref(),
        )
    }
}

impl PartialEq for StoredTuple {
    fn eq(&self, other: &Self) -> bool {
        // Include condition_context in equality check to prevent data loss
        // when tuples have same key but different condition parameters
        self.key() == other.key() && self.condition_context == other.condition_context
    }
}

impl Eq for StoredTuple {}

impl std::hash::Hash for StoredTuple {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.object_type.hash(state);
        self.object_id.hash(state);
        self.relation.hash(state);
        self.user_type.hash(state);
        self.user_id.hash(state);
        self.user_relation.hash(state);
        self.condition_name.hash(state);
        // Hash condition_context by serializing to canonical JSON string with sorted keys
        // Using BTreeMap ensures deterministic key ordering for consistent hashing
        // We use explicit discriminant to maintain Hash/PartialEq contract
        match &self.condition_context {
            None => {
                0u8.hash(state);
            }
            Some(ctx) => {
                1u8.hash(state);
                let sorted: std::collections::BTreeMap<_, _> = ctx.iter().collect();
                let json_str = serde_json::to_string(&sorted)
                    .expect("serde_json::Value should always be serializable");
                json_str.hash(state);
            }
        }
    }
}

/// Store metadata.
#[derive(Debug, Clone)]
pub struct Store {
    pub id: String,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Stored authorization model.
///
/// Authorization models define the type system and relations for a store.
/// The model data is stored as a JSON string to preserve the complex nested
/// structure of type definitions and conditions.
#[derive(Debug, Clone)]
pub struct StoredAuthorizationModel {
    /// Unique identifier for this model (ULID format).
    pub id: String,
    /// ID of the store this model belongs to.
    pub store_id: String,
    /// Schema version (e.g., "1.1").
    pub schema_version: String,
    /// JSON-serialized type definitions and conditions.
    /// This preserves the full OpenFGA model structure.
    pub model_json: String,
    /// When this model was created.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl StoredAuthorizationModel {
    /// Creates a new StoredAuthorizationModel.
    pub fn new(
        id: impl Into<String>,
        store_id: impl Into<String>,
        schema_version: impl Into<String>,
        model_json: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            store_id: store_id.into(),
            schema_version: schema_version.into(),
            model_json: model_json.into(),
            created_at: chrono::Utc::now(),
        }
    }
}

/// Options for paginated queries.
#[derive(Debug, Clone, Default)]
pub struct PaginationOptions {
    /// Maximum number of results to return.
    pub page_size: Option<u32>,
    /// Continuation token from a previous query.
    pub continuation_token: Option<String>,
}

/// Operation type for tuple changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TupleOperation {
    /// Tuple was written (added).
    Write,
    /// Tuple was deleted.
    Delete,
}

impl TupleOperation {
    /// Returns the OpenFGA-compatible string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            TupleOperation::Write => "TUPLE_OPERATION_WRITE",
            TupleOperation::Delete => "TUPLE_OPERATION_DELETE",
        }
    }
}

impl std::fmt::Display for TupleOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A tuple change entry in the changelog.
#[derive(Debug, Clone)]
pub struct TupleChange {
    /// The tuple that was changed.
    pub tuple: StoredTuple,
    /// The operation performed (write or delete).
    pub operation: TupleOperation,
    /// When the change occurred.
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl TupleChange {
    /// Creates a new write change entry.
    pub fn write(tuple: StoredTuple, timestamp: chrono::DateTime<chrono::Utc>) -> Self {
        Self {
            tuple,
            operation: TupleOperation::Write,
            timestamp,
        }
    }

    /// Creates a new delete change entry.
    pub fn delete(tuple: StoredTuple, timestamp: chrono::DateTime<chrono::Utc>) -> Self {
        Self {
            tuple,
            operation: TupleOperation::Delete,
            timestamp,
        }
    }
}

/// Filter options for reading changes.
#[derive(Debug, Clone, Default)]
pub struct ReadChangesFilter {
    /// Filter by object type.
    pub object_type: Option<String>,
}

/// Paginated query result.
#[derive(Debug, Clone)]
pub struct PaginatedResult<T> {
    /// The results.
    pub items: Vec<T>,
    /// Token for fetching the next page, if there are more results.
    pub continuation_token: Option<String>,
}

/// Abstract storage interface for authorization data.
///
/// Implementations must be thread-safe (Send + Sync) and support
/// async operations.
///
/// - **Validation**: Implementations should enforce data integrity (e.g., max lengths, allowed characters).
///   Note: This validation is a safety net for persistence. User-facing validation (400 Bad Request)
///   should primarily happen at the API layer to provide helpful error messages.
#[async_trait]
pub trait DataStore: Send + Sync + 'static {
    // Store operations

    /// Creates a new store.
    async fn create_store(&self, id: &str, name: &str) -> StorageResult<Store>;

    /// Gets a store by ID.
    async fn get_store(&self, id: &str) -> StorageResult<Store>;

    /// Deletes a store.
    async fn delete_store(&self, id: &str) -> StorageResult<()>;

    /// Updates a store's metadata (name).
    ///
    /// Returns the updated store with the new `updated_at` timestamp.
    ///
    /// # Atomicity
    ///
    /// Implementations must ensure atomicity: the returned `Store` reflects the
    /// state immediately after the update. For database backends, this typically
    /// requires using transactions or `RETURNING` clauses to prevent race
    /// conditions between the update and subsequent read.
    ///
    /// # Errors
    ///
    /// Returns `StorageError::StoreNotFound` if the store doesn't exist.
    /// Returns `StorageError::InvalidInput` if the new name is invalid.
    async fn update_store(&self, id: &str, name: &str) -> StorageResult<Store>;

    /// Lists all stores.
    async fn list_stores(&self) -> StorageResult<Vec<Store>>;

    /// Lists stores with pagination support.
    async fn list_stores_paginated(
        &self,
        pagination: &PaginationOptions,
    ) -> StorageResult<PaginatedResult<Store>>;

    // Tuple operations

    /// Writes a single tuple to storage.
    async fn write_tuple(&self, store_id: &str, tuple: StoredTuple) -> StorageResult<()> {
        self.write_tuples(store_id, vec![tuple], vec![]).await
    }

    /// Writes tuples to storage (insert and delete).
    async fn write_tuples(
        &self,
        store_id: &str,
        writes: Vec<StoredTuple>,
        deletes: Vec<StoredTuple>,
    ) -> StorageResult<()>;

    /// Reads tuples matching the filter.
    async fn read_tuples(
        &self,
        store_id: &str,
        filter: &TupleFilter,
    ) -> StorageResult<Vec<StoredTuple>>;

    /// Reads tuples matching the filter with pagination support.
    async fn read_tuples_paginated(
        &self,
        store_id: &str,
        filter: &TupleFilter,
        pagination: &PaginationOptions,
    ) -> StorageResult<PaginatedResult<StoredTuple>>;

    /// Deletes a single tuple from storage.
    async fn delete_tuple(&self, store_id: &str, tuple: StoredTuple) -> StorageResult<()> {
        self.write_tuples(store_id, vec![], vec![tuple]).await
    }

    /// Deletes multiple tuples from storage.
    async fn delete_tuples(&self, store_id: &str, tuples: Vec<StoredTuple>) -> StorageResult<()> {
        self.write_tuples(store_id, vec![], tuples).await
    }

    /// Lists objects of specific type.
    ///
    /// Used by ListObjects API to get candidate objects.
    /// Should implement DISTINCT logic to return unique object IDs.
    ///
    /// # Design Rationale (ADR: ListObjects Storage Query)
    ///
    /// Each storage backend must implement this efficiently using DISTINCT queries
    /// rather than fetching all tuples and deduplicating in Rust. This prevents:
    /// - O(n) memory usage for stores with many tuples per object type
    /// - CPU overhead of in-memory deduplication
    /// - Network bandwidth waste transferring duplicate data
    ///
    /// PostgreSQL and MySQL implementations use `SELECT DISTINCT object_id ... LIMIT ?`
    /// which pushes deduplication to the database engine where it's most efficient.
    ///
    /// # Errors
    /// Returns `StorageError::StoreNotFound` if the store doesn't exist.
    async fn list_objects_by_type(
        &self,
        _store_id: &str,
        _object_type: &str,
        _limit: usize,
    ) -> StorageResult<Vec<String>> {
        // Default implementation (Safe fail):
        // Return error to force stores to implement efficient logic.
        // Prevents OOM crashes from fetching all tuples.
        Err(StorageError::InternalError {
            message: "list_objects_by_type not implemented for this store".to_string(),
        })
    }

    /// Finds objects that reference any of the given parent objects via a specific relation.
    ///
    /// This is the core query for the ReverseExpand algorithm used by ListObjects API.
    /// Instead of checking every object individually (O(n) permission checks), this method
    /// efficiently finds all objects that could have inherited access from the given parents.
    ///
    /// # Use Case: TupleToUserset Resolution
    ///
    /// For a relation like `define viewer: viewer from parent`, to find all documents
    /// a user can view:
    /// 1. Find all folders where the user has `viewer` access
    /// 2. Use this method to find all documents with those folders as their `parent`
    ///
    /// # Arguments
    ///
    /// * `store_id` - The store to query
    /// * `object_type` - The type of objects to find (e.g., "document")
    /// * `relation` - The relation that references parents (e.g., "parent")
    /// * `parent_type` - The type of the parent objects (e.g., "folder")
    /// * `parent_ids` - The IDs of parent objects to search for
    /// * `limit` - Maximum number of results to return (DoS protection)
    ///
    /// # Returns
    ///
    /// A vector of `ObjectWithCondition` containing object IDs and optional condition
    /// metadata. Condition info is critical for authorization correctness (Invariant I1)
    /// - tuples with conditions must be evaluated before granting access.
    ///
    /// # Performance
    ///
    /// This query uses the (store_id, object_type, relation, user_type, user_id) index
    /// to efficiently find matching tuples. The IN clause on parent_ids is optimized
    /// by most database engines.
    ///
    /// # Errors
    ///
    /// Returns `StorageError::StoreNotFound` if the store doesn't exist.
    async fn get_objects_with_parents(
        &self,
        _store_id: &str,
        _object_type: &str,
        _relation: &str,
        _parent_type: &str,
        _parent_ids: &[String],
        _limit: usize,
    ) -> StorageResult<Vec<ObjectWithCondition>> {
        // Default implementation returns empty - ReverseExpand will fall back to
        // forward-scan when this is not implemented.
        Ok(Vec::new())
    }

    // Transaction support
    //
    // Note: Individual transaction control is NOT currently supported by any implementation.
    // These methods exist for future extensibility. For atomic operations, use `write_tuples`
    // which provides atomic behavior for writes and deletes within a single call.
    //
    // Callers should check `supports_transactions()` before relying on transaction methods.

    /// Begins a transaction.
    ///
    /// **Note**: Currently returns `Ok(())` as a no-op for all implementations.
    /// Check `supports_transactions()` to determine if explicit transaction control is available.
    ///
    /// For atomic write operations, use `write_tuples` which handles transactions internally.
    async fn begin_transaction(&self) -> StorageResult<()> {
        // Default: no-op - no implementations currently support explicit transaction control
        Ok(())
    }

    /// Commits the current transaction.
    ///
    /// **Note**: Currently returns `Ok(())` as a no-op for all implementations.
    /// Check `supports_transactions()` to determine if explicit transaction control is available.
    ///
    /// For atomic write operations, use `write_tuples` which commits transactions internally.
    async fn commit_transaction(&self) -> StorageResult<()> {
        // Default: no-op - no implementations currently support explicit transaction control
        Ok(())
    }

    /// Rolls back the current transaction.
    ///
    /// **Note**: Currently returns `Ok(())` as a no-op for all implementations.
    /// Check `supports_transactions()` to determine if explicit transaction control is available.
    ///
    /// For atomic write operations, use `write_tuples` which rolls back on error internally.
    async fn rollback_transaction(&self) -> StorageResult<()> {
        // Default: no-op - no implementations currently support explicit transaction control
        Ok(())
    }

    /// Checks if the store supports explicit transaction control.
    ///
    /// Returns `true` if `begin_transaction`, `commit_transaction`, and `rollback_transaction`
    /// are functional. Returns `false` if these methods are no-ops.
    ///
    /// **Note**: Currently returns `false` for all implementations. For atomic operations,
    /// use `write_tuples` which provides internal transaction handling.
    fn supports_transactions(&self) -> bool {
        false
    }

    // Authorization model operations

    /// Writes an authorization model to storage.
    ///
    /// # Errors
    ///
    /// Returns `StorageError::StoreNotFound` if the store doesn't exist.
    async fn write_authorization_model(
        &self,
        model: StoredAuthorizationModel,
    ) -> StorageResult<StoredAuthorizationModel>;

    /// Gets an authorization model by ID.
    ///
    /// # Errors
    ///
    /// Returns `StorageError::StoreNotFound` if the store doesn't exist.
    /// Returns `StorageError::ModelNotFound` if the model doesn't exist.
    async fn get_authorization_model(
        &self,
        store_id: &str,
        model_id: &str,
    ) -> StorageResult<StoredAuthorizationModel>;

    /// Lists all authorization models for a store.
    ///
    /// Returns models ordered by `created_at DESC, id DESC` (newest first, deterministic).
    ///
    /// # Errors
    ///
    /// Returns `StorageError::StoreNotFound` if the store doesn't exist.
    async fn list_authorization_models(
        &self,
        store_id: &str,
    ) -> StorageResult<Vec<StoredAuthorizationModel>>;

    /// Lists authorization models with pagination support.
    ///
    /// Returns models ordered by `created_at DESC, id DESC` (newest first, deterministic).
    ///
    /// # Errors
    ///
    /// Returns `StorageError::StoreNotFound` if the store doesn't exist.
    async fn list_authorization_models_paginated(
        &self,
        store_id: &str,
        pagination: &PaginationOptions,
    ) -> StorageResult<PaginatedResult<StoredAuthorizationModel>>;

    /// Gets the latest authorization model for a store.
    ///
    /// Returns the most recently created model (ordered by `created_at DESC, id DESC`
    /// for deterministic results).
    ///
    /// # Errors
    ///
    /// Returns `StorageError::StoreNotFound` if the store doesn't exist.
    /// Returns `StorageError::ModelNotFound` if no models exist.
    async fn get_latest_authorization_model(
        &self,
        store_id: &str,
    ) -> StorageResult<StoredAuthorizationModel>;

    /// Deletes an authorization model by ID.
    ///
    /// # Behavior
    ///
    /// - Deletes only the model metadata; existing tuples remain accessible.
    /// - Deleted models will not appear in `list_authorization_models` results.
    /// - Attempts to `get_authorization_model` for a deleted model return `ModelNotFound`.
    /// - If the deleted model was the latest, `get_latest_authorization_model` returns
    ///   the next most recent model (or `ModelNotFound` if no models remain).
    ///
    /// # Concurrency
    ///
    /// - Concurrent deletions of the same model are safe; one will succeed and others
    ///   will return `ModelNotFound`.
    /// - Concurrent deletion and read operations are safe; reads may see the model
    ///   before or after deletion depending on timing.
    /// - Implementations must ensure atomicity of the delete operation itself.
    ///
    /// # Errors
    ///
    /// Returns `StorageError::StoreNotFound` if the store doesn't exist.
    /// Returns `StorageError::ModelNotFound` if the model doesn't exist.
    async fn delete_authorization_model(&self, store_id: &str, model_id: &str)
        -> StorageResult<()>;

    // Changelog operations

    /// Reads tuple changes from the changelog.
    ///
    /// Returns changes in chronological order (ascending by timestamp).
    /// Each change includes the tuple, operation type (write/delete), and timestamp.
    ///
    /// # Parameters
    ///
    /// - `store_id`: The store to read changes from.
    /// - `filter`: Optional filter by object type.
    /// - `pagination`: Pagination options (page_size, continuation_token).
    ///
    /// # Errors
    ///
    /// Returns `StorageError::StoreNotFound` if the store doesn't exist.
    async fn read_changes(
        &self,
        store_id: &str,
        filter: &ReadChangesFilter,
        pagination: &PaginationOptions,
    ) -> StorageResult<PaginatedResult<TupleChange>>;

    // Health check operations

    /// Checks the health of the storage backend.
    ///
    /// Returns detailed health status including:
    /// - Whether the backend is healthy and ready to serve requests
    /// - Latency of a health check ping (e.g., `SELECT 1` for databases)
    /// - Connection pool statistics (active/idle/max connections)
    ///
    /// # Use Cases
    ///
    /// - **Kubernetes liveness/readiness probes**: `/health` endpoint calls this
    /// - **Startup validation**: Verify database is ready before accepting traffic
    /// - **Monitoring**: Expose pool health metrics to observability systems
    ///
    /// # Errors
    ///
    /// Returns `StorageError::HealthCheckFailed` if the backend is unreachable
    /// or unhealthy.
    async fn health_check(&self) -> StorageResult<HealthStatus>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // Test: DataStore trait compiles
    #[test]
    fn test_datastore_trait_compiles() {
        // This test passes if the code compiles
        // The DataStore trait is defined above and used throughout
        fn _assert_trait_exists<T: DataStore>() {}
    }

    // Test: DataStore trait is Send + Sync
    #[test]
    fn test_datastore_trait_is_send_sync() {
        fn _assert_send_sync<T: Send + Sync + 'static>() {}

        // This verifies the trait bounds at compile time
        fn _check_trait_bounds<T: DataStore>() {
            _assert_send_sync::<T>();
        }
    }

    // Test: Can define write_tuple method signature
    #[test]
    fn test_write_tuple_method_signature() {
        // Verify the method exists with correct signature by checking
        // that the code compiles. Uses a helper async fn to verify signature.
        async fn _verify_signature<T: DataStore>(store: &T) {
            let tuple = StoredTuple::new("doc", "1", "viewer", "user", "alice", None);
            let _: StorageResult<()> = store.write_tuple("store", tuple).await;
        }
    }

    // Test: Can define read_tuples method signature
    #[test]
    fn test_read_tuples_method_signature() {
        // Verify the method exists with correct signature
        async fn _verify_signature<T: DataStore>(store: &T) {
            let filter = TupleFilter::default();
            let _: StorageResult<Vec<StoredTuple>> = store.read_tuples("store", &filter).await;
        }
    }

    // Test: Can define delete_tuple method signature
    #[test]
    fn test_delete_tuple_method_signature() {
        // Verify the method exists with correct signature
        async fn _verify_signature<T: DataStore>(store: &T) {
            let tuple = StoredTuple::new("doc", "1", "viewer", "user", "alice", None);
            let _: StorageResult<()> = store.delete_tuple("store", tuple).await;
        }
    }

    // Test: Can define transaction support methods
    #[test]
    fn test_transaction_support_methods() {
        // Verify transaction-related methods exist
        fn _check_supports_transactions<T: DataStore>(store: &T) -> bool {
            store.supports_transactions()
        }

        // Verify transaction method signatures compile
        async fn _verify_transaction_signatures<T: DataStore>(store: &T) {
            let _: StorageResult<()> = store.begin_transaction().await;
            let _: StorageResult<()> = store.commit_transaction().await;
            let _: StorageResult<()> = store.rollback_transaction().await;
        }
    }

    // Test: DataStore can be used with Arc (object safety check)
    #[test]
    fn test_datastore_object_safety() {
        // DataStore should be object-safe for use with dyn
        fn _assert_object_safe(_: &dyn DataStore) {}

        // And work with Arc
        fn _assert_arc_compatible(_: Arc<dyn DataStore>) {}
    }

    // Test: StoredTuple fields are correct
    #[test]
    fn test_stored_tuple_struct() {
        let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);

        assert_eq!(tuple.object_type, "document");
        assert_eq!(tuple.object_id, "doc1");
        assert_eq!(tuple.relation, "viewer");
        assert_eq!(tuple.user_type, "user");
        assert_eq!(tuple.user_id, "alice");
        assert!(tuple.user_relation.is_none());
        assert!(tuple.condition_name.is_none());
        assert!(tuple.condition_context.is_none());
    }

    // Test: StoredTuple with condition
    #[test]
    fn test_stored_tuple_with_condition() {
        let mut context = std::collections::HashMap::new();
        context.insert(
            "expires_at".to_string(),
            serde_json::json!("2024-12-31T23:59:59Z"),
        );

        let tuple = StoredTuple::with_condition(
            "document",
            "doc1",
            "viewer",
            "user",
            "alice",
            None,
            "time_bound",
            Some(context),
        );

        assert_eq!(tuple.object_type, "document");
        assert_eq!(tuple.condition_name, Some("time_bound".to_string()));
        assert!(tuple.condition_context.is_some());
    }

    // Test: TupleFilter default is empty
    #[test]
    fn test_tuple_filter_default() {
        let filter = TupleFilter::default();

        assert!(filter.object_type.is_none());
        assert!(filter.object_id.is_none());
        assert!(filter.relation.is_none());
        assert!(filter.user.is_none());
    }

    // Test: Store struct fields
    #[test]
    fn test_store_struct() {
        let now = chrono::Utc::now();
        let store = Store {
            id: "store-1".to_string(),
            name: "Test Store".to_string(),
            created_at: now,
            updated_at: now,
        };

        assert_eq!(store.id, "store-1");
        assert_eq!(store.name, "Test Store");
        assert_eq!(store.created_at, now);
        assert_eq!(store.updated_at, now);
    }

    // Test: PaginationOptions default
    #[test]
    fn test_pagination_options_default() {
        let opts = PaginationOptions::default();

        assert!(opts.page_size.is_none());
        assert!(opts.continuation_token.is_none());
    }

    // Test: PaginatedResult struct
    #[test]
    fn test_paginated_result_struct() {
        let result: PaginatedResult<String> = PaginatedResult {
            items: vec!["item1".to_string(), "item2".to_string()],
            continuation_token: Some("next-page".to_string()),
        };

        assert_eq!(result.items.len(), 2);
        assert_eq!(result.continuation_token, Some("next-page".to_string()));
    }

    // Test: parse_user_filter works with Unicode characters
    #[test]
    fn test_parse_user_filter_unicode() {
        // Chinese characters
        let (user_type, user_id, relation) = parse_user_filter("user:用户123").unwrap();
        assert_eq!(user_type, "user");
        assert_eq!(user_id, "用户123");
        assert!(relation.is_none());

        // Japanese characters
        let (user_type, user_id, relation) = parse_user_filter("user:ユーザー").unwrap();
        assert_eq!(user_type, "user");
        assert_eq!(user_id, "ユーザー");
        assert!(relation.is_none());

        // Emoji
        let (user_type, user_id, relation) = parse_user_filter("user:👤").unwrap();
        assert_eq!(user_type, "user");
        assert_eq!(user_id, "👤");
        assert!(relation.is_none());

        // Arabic characters
        let (user_type, user_id, relation) = parse_user_filter("user:مستخدم").unwrap();
        assert_eq!(user_type, "user");
        assert_eq!(user_id, "مستخدم");
        assert!(relation.is_none());

        // Accented Latin characters
        let (user_type, user_id, relation) = parse_user_filter("user:Müller").unwrap();
        assert_eq!(user_type, "user");
        assert_eq!(user_id, "Müller");
        assert!(relation.is_none());

        // Unicode in userset format
        let (user_type, user_id, relation) = parse_user_filter("group:团队#成员").unwrap();
        assert_eq!(user_type, "group");
        assert_eq!(user_id, "团队");
        assert_eq!(relation, Some("成员".to_string()));
    }
}
