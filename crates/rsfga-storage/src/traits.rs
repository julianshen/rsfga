//! DataStore trait definition.

use async_trait::async_trait;

use crate::error::{StorageError, StorageResult};

/// Maximum length for string fields.
const MAX_FIELD_LENGTH: usize = 255;

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
    if store_id.len() > MAX_FIELD_LENGTH {
        return Err(StorageError::InvalidInput {
            message: format!(
                "store_id exceeds maximum length of {} characters",
                MAX_FIELD_LENGTH
            ),
        });
    }
    Ok(())
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
    if name.len() > MAX_FIELD_LENGTH {
        return Err(StorageError::InvalidInput {
            message: format!(
                "store name exceeds maximum length of {} characters",
                MAX_FIELD_LENGTH
            ),
        });
    }
    Ok(())
}

/// Validate a stored tuple.
///
/// # Errors
/// Returns `StorageError::InvalidInput` if any field is empty or too long.
pub fn validate_tuple(tuple: &StoredTuple) -> StorageResult<()> {
    if tuple.object_type.is_empty() {
        return Err(StorageError::InvalidInput {
            message: "object_type cannot be empty".to_string(),
        });
    }
    if tuple.object_type.len() > MAX_FIELD_LENGTH {
        return Err(StorageError::InvalidInput {
            message: format!(
                "object_type exceeds maximum length of {} characters",
                MAX_FIELD_LENGTH
            ),
        });
    }
    if tuple.object_id.is_empty() {
        return Err(StorageError::InvalidInput {
            message: "object_id cannot be empty".to_string(),
        });
    }
    if tuple.object_id.len() > MAX_FIELD_LENGTH {
        return Err(StorageError::InvalidInput {
            message: format!(
                "object_id exceeds maximum length of {} characters",
                MAX_FIELD_LENGTH
            ),
        });
    }
    if tuple.relation.is_empty() {
        return Err(StorageError::InvalidInput {
            message: "relation cannot be empty".to_string(),
        });
    }
    if tuple.relation.len() > MAX_FIELD_LENGTH {
        return Err(StorageError::InvalidInput {
            message: format!(
                "relation exceeds maximum length of {} characters",
                MAX_FIELD_LENGTH
            ),
        });
    }
    if tuple.user_type.is_empty() {
        return Err(StorageError::InvalidInput {
            message: "user_type cannot be empty".to_string(),
        });
    }
    if tuple.user_type.len() > MAX_FIELD_LENGTH {
        return Err(StorageError::InvalidInput {
            message: format!(
                "user_type exceeds maximum length of {} characters",
                MAX_FIELD_LENGTH
            ),
        });
    }
    if tuple.user_id.is_empty() {
        return Err(StorageError::InvalidInput {
            message: "user_id cannot be empty".to_string(),
        });
    }
    if tuple.user_id.len() > MAX_FIELD_LENGTH {
        return Err(StorageError::InvalidInput {
            message: format!(
                "user_id exceeds maximum length of {} characters",
                MAX_FIELD_LENGTH
            ),
        });
    }
    if let Some(ref user_relation) = tuple.user_relation {
        if user_relation.is_empty() {
            return Err(StorageError::InvalidInput {
                message: "user_relation cannot be empty if provided".to_string(),
            });
        }
        if user_relation.len() > MAX_FIELD_LENGTH {
            return Err(StorageError::InvalidInput {
                message: format!(
                    "user_relation exceeds maximum length of {} characters",
                    MAX_FIELD_LENGTH
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
                    "Invalid user filter format: '{}'. Expected 'type:id#relation'",
                    user
                ),
            });
        }
        let user_parts: Vec<&str> = parts[0].split(':').collect();
        if user_parts.len() != 2 || user_parts[0].is_empty() || user_parts[1].is_empty() {
            return Err(StorageError::InvalidFilter {
                message: format!(
                    "Invalid user filter format: '{}'. Expected 'type:id#relation'",
                    user
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
                message: format!("Invalid user filter format: '{}'. Expected 'type:id'", user),
            });
        }
        Ok((user_parts[0].to_string(), user_parts[1].to_string(), None))
    }
}

/// A stored tuple.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StoredTuple {
    pub object_type: String,
    pub object_id: String,
    pub relation: String,
    pub user_type: String,
    pub user_id: String,
    pub user_relation: Option<String>,
}

/// Store metadata.
#[derive(Debug, Clone)]
pub struct Store {
    pub id: String,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Options for paginated queries.
#[derive(Debug, Clone, Default)]
pub struct PaginationOptions {
    /// Maximum number of results to return.
    pub page_size: Option<u32>,
    /// Continuation token from a previous query.
    pub continuation_token: Option<String>,
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
#[async_trait]
pub trait DataStore: Send + Sync + 'static {
    // Store operations

    /// Creates a new store.
    async fn create_store(&self, id: &str, name: &str) -> StorageResult<Store>;

    /// Gets a store by ID.
    async fn get_store(&self, id: &str) -> StorageResult<Store>;

    /// Deletes a store.
    async fn delete_store(&self, id: &str) -> StorageResult<()>;

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
            let tuple = StoredTuple {
                object_type: "doc".to_string(),
                object_id: "1".to_string(),
                relation: "viewer".to_string(),
                user_type: "user".to_string(),
                user_id: "alice".to_string(),
                user_relation: None,
            };
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
            let tuple = StoredTuple {
                object_type: "doc".to_string(),
                object_id: "1".to_string(),
                relation: "viewer".to_string(),
                user_type: "user".to_string(),
                user_id: "alice".to_string(),
                user_relation: None,
            };
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
        let tuple = StoredTuple {
            object_type: "document".to_string(),
            object_id: "doc1".to_string(),
            relation: "viewer".to_string(),
            user_type: "user".to_string(),
            user_id: "alice".to_string(),
            user_relation: None,
        };

        assert_eq!(tuple.object_type, "document");
        assert_eq!(tuple.object_id, "doc1");
        assert_eq!(tuple.relation, "viewer");
        assert_eq!(tuple.user_type, "user");
        assert_eq!(tuple.user_id, "alice");
        assert!(tuple.user_relation.is_none());
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
}
