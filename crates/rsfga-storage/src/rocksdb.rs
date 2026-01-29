//! RocksDB storage implementation for RSFGA.
//!
//! This module provides an embedded storage backend using RocksDB, optimized for:
//! - High write throughput (LSM tree sequential writes vs B-tree random I/O)
//! - Low latency reads (no network roundtrip, direct key lookups)
//! - Single-node deployments (edge/embedded use cases)
//!
//! # Key Schema
//!
//! ```text
//! Stores:    s:{store_id}                           -> JSON(Store)
//! Tuples:    t:{store_id}:{obj_type}:{obj_id}:{rel}:{user_type}:{user_id}:{user_rel?} -> JSON(TupleValue)
//! Models:    m:{store_id}:{model_id}                -> JSON(StoredAuthorizationModel)
//! Changes:   c:{store_id}:{ulid}                    -> JSON(TupleChange)
//! ```
//!
//! # Performance Characteristics
//!
//! - **Write**: O(1) amortized (memtable insert, background compaction)
//! - **Point read**: O(log N) where N is number of SST files
//! - **Range scan**: O(K + log N) where K is result set size
//! - **Delete**: O(1) (tombstone write, compaction cleanup)
//!
//! # Async Safety
//!
//! All RocksDB operations are wrapped in `tokio::task::spawn_blocking` to avoid
//! blocking the async runtime. This ensures the Tokio executor threads remain
//! responsive even during heavy I/O operations.
//!
//! # Limitations
//!
//! - Single-node only (no replication)
//! - Compaction can cause latency spikes (configurable)
//! - No SQL query optimizer (complex filters require full scans)

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use rust_rocksdb::{Options, WriteBatch, DB};
use serde::{Deserialize, Serialize};
use tracing::{instrument, warn};
use ulid::Ulid;

use crate::error::{HealthStatus, StorageError, StorageResult};
use crate::traits::{
    parse_continuation_token, parse_user_filter, validate_object_type, validate_store_id,
    validate_store_name, validate_tuple, DataStore, ObjectWithCondition, PaginatedResult,
    PaginationOptions, ReadChangesFilter, Store, StoredAuthorizationModel, StoredTuple,
    TupleChange, TupleFilter, TupleOperation,
};

/// Configuration for RocksDB storage backend.
#[derive(Debug, Clone)]
pub struct RocksDBConfig {
    /// Path to the RocksDB database directory.
    pub path: String,
    /// Whether to create the database if it doesn't exist.
    pub create_if_missing: bool,
    /// Write buffer size in bytes (default: 64MB).
    /// Larger values improve write throughput but use more memory.
    pub write_buffer_size: usize,
    /// Maximum number of write buffers (default: 3).
    pub max_write_buffer_number: i32,
    /// Block cache size in bytes (default: 128MB).
    /// Larger values improve read performance for hot data.
    pub block_cache_size: usize,
    /// Whether to enable compression (default: true).
    /// Reduces disk usage but adds CPU overhead.
    pub enable_compression: bool,
    /// Maximum number of background compaction threads (default: 4).
    pub max_background_jobs: i32,
}

impl Default for RocksDBConfig {
    fn default() -> Self {
        Self {
            path: "./rsfga-data".to_string(),
            create_if_missing: true,
            write_buffer_size: 64 * 1024 * 1024, // 64MB
            max_write_buffer_number: 3,
            block_cache_size: 128 * 1024 * 1024, // 128MB
            enable_compression: true,
            max_background_jobs: 4,
        }
    }
}

/// Key prefixes for different data types.
mod prefix {
    pub const STORE: &str = "s";
    pub const TUPLE: &str = "t";
    pub const MODEL: &str = "m";
    pub const CHANGE: &str = "c";
}

/// Separator used in composite keys.
const KEY_SEP: char = ':';

/// Internal representation of tuple value stored in RocksDB.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TupleValue {
    pub condition_name: Option<String>,
    pub condition_context: Option<HashMap<String, serde_json::Value>>,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Internal representation of change entry stored in RocksDB.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChangeValue {
    pub tuple_key: String,
    pub tuple_value: TupleValue,
    pub operation: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// RocksDB implementation of DataStore.
///
/// Thread-safe embedded storage optimized for write-heavy workloads.
/// All blocking I/O operations are offloaded to a blocking thread pool
/// via `tokio::task::spawn_blocking`.
pub struct RocksDBDataStore {
    db: Arc<DB>,
    #[allow(dead_code)]
    config: RocksDBConfig,
}

impl std::fmt::Debug for RocksDBDataStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RocksDBDataStore")
            .field("path", &self.config.path)
            .finish()
    }
}

impl RocksDBDataStore {
    /// Creates a new RocksDB data store with the given configuration.
    ///
    /// # Errors
    ///
    /// Returns `StorageError::ConnectionError` if the database cannot be opened.
    pub fn new(config: RocksDBConfig) -> StorageResult<Self> {
        let mut opts = Options::default();
        opts.create_if_missing(config.create_if_missing);
        opts.set_write_buffer_size(config.write_buffer_size);
        opts.set_max_write_buffer_number(config.max_write_buffer_number);
        opts.set_max_background_jobs(config.max_background_jobs);

        if config.enable_compression {
            opts.set_compression_type(rust_rocksdb::DBCompressionType::Lz4);
        }

        // Set up block cache
        let mut block_opts = rust_rocksdb::BlockBasedOptions::default();
        let cache = rust_rocksdb::Cache::new_lru_cache(config.block_cache_size);
        block_opts.set_block_cache(&cache);
        block_opts.set_bloom_filter(10.0, false);
        opts.set_block_based_table_factory(&block_opts);

        let db = DB::open(&opts, &config.path).map_err(|e| StorageError::ConnectionError {
            message: format!("Failed to open RocksDB at {}: {}", config.path, e),
        })?;

        Ok(Self {
            db: Arc::new(db),
            config,
        })
    }

    /// Creates a new RocksDB data store at the given path with default configuration.
    pub fn open<P: AsRef<Path>>(path: P) -> StorageResult<Self> {
        Self::new(RocksDBConfig {
            path: path.as_ref().to_string_lossy().to_string(),
            ..Default::default()
        })
    }

    /// Returns the configuration for this store.
    pub fn config(&self) -> &RocksDBConfig {
        &self.config
    }

    // =========================================================================
    // Key Construction Helpers
    // =========================================================================

    fn store_key(store_id: &str) -> String {
        format!("{}{KEY_SEP}{store_id}", prefix::STORE)
    }

    fn store_prefix() -> String {
        format!("{}{KEY_SEP}", prefix::STORE)
    }

    fn tuple_key(
        store_id: &str,
        object_type: &str,
        object_id: &str,
        relation: &str,
        user_type: &str,
        user_id: &str,
        user_relation: Option<&str>,
    ) -> String {
        let user_rel = user_relation.unwrap_or("");
        format!(
            "{}{KEY_SEP}{store_id}{KEY_SEP}{object_type}{KEY_SEP}{object_id}{KEY_SEP}{relation}{KEY_SEP}{user_type}{KEY_SEP}{user_id}{KEY_SEP}{user_rel}",
            prefix::TUPLE
        )
    }

    fn tuple_prefix(store_id: &str) -> String {
        format!("{}{KEY_SEP}{store_id}{KEY_SEP}", prefix::TUPLE)
    }

    fn tuple_prefix_by_type(store_id: &str, object_type: &str) -> String {
        format!(
            "{}{KEY_SEP}{store_id}{KEY_SEP}{object_type}{KEY_SEP}",
            prefix::TUPLE
        )
    }

    fn model_key(store_id: &str, model_id: &str) -> String {
        format!("{}{KEY_SEP}{store_id}{KEY_SEP}{model_id}", prefix::MODEL)
    }

    fn model_prefix(store_id: &str) -> String {
        format!("{}{KEY_SEP}{store_id}{KEY_SEP}", prefix::MODEL)
    }

    fn change_key(store_id: &str, ulid: &str) -> String {
        format!("{}{KEY_SEP}{store_id}{KEY_SEP}{ulid}", prefix::CHANGE)
    }

    fn change_prefix(store_id: &str) -> String {
        format!("{}{KEY_SEP}{store_id}{KEY_SEP}", prefix::CHANGE)
    }

    // =========================================================================
    // Key Parsing Helpers
    // =========================================================================

    /// Parses a tuple key back into its components.
    fn parse_tuple_key(
        key: &str,
    ) -> Option<(String, String, String, String, String, Option<String>)> {
        let parts: Vec<&str> = key.split(KEY_SEP).collect();
        if parts.len() < 8 || parts[0] != prefix::TUPLE {
            return None;
        }
        // parts: [prefix, store_id, obj_type, obj_id, relation, user_type, user_id, user_rel]
        let user_relation = if parts.len() > 7 && !parts[7].is_empty() {
            Some(parts[7].to_string())
        } else {
            None
        };
        Some((
            parts[2].to_string(), // object_type
            parts[3].to_string(), // object_id
            parts[4].to_string(), // relation
            parts[5].to_string(), // user_type
            parts[6].to_string(), // user_id
            user_relation,
        ))
    }

    // =========================================================================
    // Internal Helpers
    // =========================================================================

    /// Generates a unique change ID using ULID.
    /// ULIDs are lexicographically sortable and unique across restarts/processes.
    fn generate_change_id() -> String {
        Ulid::new().to_string()
    }

    /// Helper to convert spawn_blocking JoinError to StorageError.
    fn join_error(e: tokio::task::JoinError) -> StorageError {
        StorageError::InternalError {
            message: format!("Task join error: {e}"),
        }
    }
}

#[async_trait]
impl DataStore for RocksDBDataStore {
    // =========================================================================
    // Store Operations
    // =========================================================================

    #[instrument(skip(self), fields(store_id = %id))]
    async fn create_store(&self, id: &str, name: &str) -> StorageResult<Store> {
        validate_store_id(id)?;
        validate_store_name(name)?;

        let db = Arc::clone(&self.db);
        let id_owned = id.to_string();
        let name_owned = name.to_string();

        tokio::task::spawn_blocking(move || {
            let key = Self::store_key(&id_owned);

            // Check if store already exists
            if db
                .get(key.as_bytes())
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to check store existence: {e}"),
                })?
                .is_some()
            {
                return Err(StorageError::StoreAlreadyExists { store_id: id_owned });
            }

            let now = chrono::Utc::now();
            let store = Store {
                id: id_owned,
                name: name_owned,
                created_at: now,
                updated_at: now,
            };

            let value =
                serde_json::to_vec(&store).map_err(|e| StorageError::SerializationError {
                    message: format!("Failed to serialize store: {e}"),
                })?;

            db.put(key.as_bytes(), &value)
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to create store: {e}"),
                })?;

            Ok(store)
        })
        .await
        .map_err(Self::join_error)?
    }

    #[instrument(skip(self), fields(store_id = %id))]
    async fn get_store(&self, id: &str) -> StorageResult<Store> {
        let db = Arc::clone(&self.db);
        let id_owned = id.to_string();

        tokio::task::spawn_blocking(move || {
            let key = Self::store_key(&id_owned);

            match db.get(key.as_bytes()) {
                Ok(Some(value)) => {
                    serde_json::from_slice(&value).map_err(|e| StorageError::SerializationError {
                        message: format!("Failed to deserialize store: {e}"),
                    })
                }
                Ok(None) => Err(StorageError::StoreNotFound { store_id: id_owned }),
                Err(e) => Err(StorageError::QueryError {
                    message: format!("Failed to get store: {e}"),
                }),
            }
        })
        .await
        .map_err(Self::join_error)?
    }

    #[instrument(skip(self), fields(store_id = %id))]
    async fn delete_store(&self, id: &str) -> StorageResult<()> {
        let db = Arc::clone(&self.db);
        let id_owned = id.to_string();

        tokio::task::spawn_blocking(move || {
            let store_key = Self::store_key(&id_owned);

            // Check if store exists
            if db
                .get(store_key.as_bytes())
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to check store existence: {e}"),
                })?
                .is_none()
            {
                return Err(StorageError::StoreNotFound {
                    store_id: id_owned.clone(),
                });
            }

            let mut batch = WriteBatch::default();

            // Delete the store record
            batch.delete(store_key.as_bytes());

            // Delete all tuples for this store
            let tuple_prefix = Self::tuple_prefix(&id_owned);
            let iter = db.prefix_iterator(tuple_prefix.as_bytes());
            for item in iter {
                match item {
                    Ok((key, _)) => {
                        let key_str = String::from_utf8_lossy(&key);
                        if !key_str.starts_with(&tuple_prefix) {
                            break;
                        }
                        batch.delete(&key);
                    }
                    Err(e) => {
                        return Err(StorageError::QueryError {
                            message: format!("Failed to iterate tuples for deletion: {e}"),
                        });
                    }
                }
            }

            // Delete all models for this store
            let model_prefix = Self::model_prefix(&id_owned);
            let iter = db.prefix_iterator(model_prefix.as_bytes());
            for item in iter {
                match item {
                    Ok((key, _)) => {
                        let key_str = String::from_utf8_lossy(&key);
                        if !key_str.starts_with(&model_prefix) {
                            break;
                        }
                        batch.delete(&key);
                    }
                    Err(e) => {
                        return Err(StorageError::QueryError {
                            message: format!("Failed to iterate models for deletion: {e}"),
                        });
                    }
                }
            }

            // Delete all changes for this store
            let change_prefix = Self::change_prefix(&id_owned);
            let iter = db.prefix_iterator(change_prefix.as_bytes());
            for item in iter {
                match item {
                    Ok((key, _)) => {
                        let key_str = String::from_utf8_lossy(&key);
                        if !key_str.starts_with(&change_prefix) {
                            break;
                        }
                        batch.delete(&key);
                    }
                    Err(e) => {
                        return Err(StorageError::QueryError {
                            message: format!("Failed to iterate changes for deletion: {e}"),
                        });
                    }
                }
            }

            db.write(batch).map_err(|e| StorageError::QueryError {
                message: format!("Failed to delete store: {e}"),
            })?;

            Ok(())
        })
        .await
        .map_err(Self::join_error)?
    }

    #[instrument(skip(self), fields(store_id = %id))]
    async fn update_store(&self, id: &str, name: &str) -> StorageResult<Store> {
        validate_store_id(id)?;
        validate_store_name(name)?;

        let db = Arc::clone(&self.db);
        let id_owned = id.to_string();
        let name_owned = name.to_string();

        tokio::task::spawn_blocking(move || {
            let key = Self::store_key(&id_owned);

            // Get existing store
            let mut store: Store = match db.get(key.as_bytes()) {
                Ok(Some(value)) => serde_json::from_slice(&value).map_err(|e| {
                    StorageError::SerializationError {
                        message: format!("Failed to deserialize store: {e}"),
                    }
                })?,
                Ok(None) => return Err(StorageError::StoreNotFound { store_id: id_owned }),
                Err(e) => {
                    return Err(StorageError::QueryError {
                        message: format!("Failed to get store: {e}"),
                    })
                }
            };

            // Update fields
            store.name = name_owned;
            store.updated_at = chrono::Utc::now();

            let value =
                serde_json::to_vec(&store).map_err(|e| StorageError::SerializationError {
                    message: format!("Failed to serialize store: {e}"),
                })?;

            db.put(key.as_bytes(), &value)
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to update store: {e}"),
                })?;

            Ok(store)
        })
        .await
        .map_err(Self::join_error)?
    }

    async fn list_stores(&self) -> StorageResult<Vec<Store>> {
        let db = Arc::clone(&self.db);

        tokio::task::spawn_blocking(move || {
            let prefix = Self::store_prefix();
            let mut stores = Vec::new();

            let iter = db.prefix_iterator(prefix.as_bytes());
            for item in iter {
                match item {
                    Ok((key, value)) => {
                        let key_str = String::from_utf8_lossy(&key);
                        if !key_str.starts_with(&prefix) {
                            break;
                        }
                        match serde_json::from_slice::<Store>(&value) {
                            Ok(store) => stores.push(store),
                            Err(e) => {
                                warn!(key = %key_str, error = %e, "Failed to deserialize store, skipping");
                            }
                        }
                    }
                    Err(e) => {
                        return Err(StorageError::QueryError {
                            message: format!("Failed to iterate stores: {e}"),
                        });
                    }
                }
            }

            // Sort by created_at descending for consistency
            stores.sort_by(|a, b| b.created_at.cmp(&a.created_at));

            Ok(stores)
        })
        .await
        .map_err(Self::join_error)?
    }

    async fn list_stores_paginated(
        &self,
        pagination: &PaginationOptions,
    ) -> StorageResult<PaginatedResult<Store>> {
        let mut stores = self.list_stores().await?;

        let page_size = pagination.page_size.unwrap_or(100) as usize;
        let offset = parse_continuation_token(&pagination.continuation_token)? as usize;

        let items: Vec<Store> = stores.drain(..).skip(offset).take(page_size).collect();

        let next_offset = offset + items.len();
        let continuation_token = if items.len() == page_size {
            Some(next_offset.to_string())
        } else {
            None
        };

        Ok(PaginatedResult {
            items,
            continuation_token,
        })
    }

    // =========================================================================
    // Tuple Operations
    // =========================================================================

    #[instrument(skip(self, writes, deletes), fields(store_id = %store_id, writes = writes.len(), deletes = deletes.len()))]
    async fn write_tuples(
        &self,
        store_id: &str,
        writes: Vec<StoredTuple>,
        deletes: Vec<StoredTuple>,
    ) -> StorageResult<()> {
        validate_store_id(store_id)?;
        for tuple in &writes {
            validate_tuple(tuple)?;
        }
        for tuple in &deletes {
            validate_tuple(tuple)?;
        }

        let db = Arc::clone(&self.db);
        let store_id_owned = store_id.to_string();

        tokio::task::spawn_blocking(move || {
            // Check store exists
            let store_key = Self::store_key(&store_id_owned);
            if db
                .get(store_key.as_bytes())
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to check store existence: {e}"),
                })?
                .is_none()
            {
                return Err(StorageError::StoreNotFound {
                    store_id: store_id_owned,
                });
            }

            let mut batch = WriteBatch::default();
            let now = chrono::Utc::now();

            // Process deletes first
            for tuple in deletes {
                let key = Self::tuple_key(
                    &store_id_owned,
                    &tuple.object_type,
                    &tuple.object_id,
                    &tuple.relation,
                    &tuple.user_type,
                    &tuple.user_id,
                    tuple.user_relation.as_deref(),
                );
                batch.delete(key.as_bytes());

                // Write change entry
                let change_id = Self::generate_change_id();
                let change_key = Self::change_key(&store_id_owned, &change_id);
                let change_value = ChangeValue {
                    tuple_key: key.clone(),
                    tuple_value: TupleValue {
                        condition_name: tuple.condition_name.clone(),
                        condition_context: tuple.condition_context.clone(),
                        created_at: tuple.created_at,
                    },
                    operation: TupleOperation::Delete.as_str().to_string(),
                    timestamp: chrono::Utc::now(),
                };
                if let Ok(json) = serde_json::to_vec(&change_value) {
                    batch.put(change_key.as_bytes(), &json);
                }
            }

            // Process writes
            for tuple in writes {
                let key = Self::tuple_key(
                    &store_id_owned,
                    &tuple.object_type,
                    &tuple.object_id,
                    &tuple.relation,
                    &tuple.user_type,
                    &tuple.user_id,
                    tuple.user_relation.as_deref(),
                );

                let value = TupleValue {
                    condition_name: tuple.condition_name.clone(),
                    condition_context: tuple.condition_context.clone(),
                    created_at: Some(tuple.created_at.unwrap_or(now)),
                };

                let json =
                    serde_json::to_vec(&value).map_err(|e| StorageError::SerializationError {
                        message: format!("Failed to serialize tuple: {e}"),
                    })?;

                batch.put(key.as_bytes(), &json);

                // Write change entry
                let change_id = Self::generate_change_id();
                let change_key = Self::change_key(&store_id_owned, &change_id);
                let change_value = ChangeValue {
                    tuple_key: key,
                    tuple_value: value,
                    operation: TupleOperation::Write.as_str().to_string(),
                    timestamp: chrono::Utc::now(),
                };
                if let Ok(json) = serde_json::to_vec(&change_value) {
                    batch.put(change_key.as_bytes(), &json);
                }
            }

            db.write(batch).map_err(|e| StorageError::QueryError {
                message: format!("Failed to write tuples: {e}"),
            })?;

            Ok(())
        })
        .await
        .map_err(Self::join_error)?
    }

    async fn read_tuples(
        &self,
        store_id: &str,
        filter: &TupleFilter,
    ) -> StorageResult<Vec<StoredTuple>> {
        // Validate and parse filters upfront (before spawn_blocking)
        let store_id_owned = store_id.to_string();

        // Check store exists
        let store = self.get_store(store_id).await;
        if let Err(StorageError::StoreNotFound { .. }) = store {
            return Err(StorageError::StoreNotFound {
                store_id: store_id_owned,
            });
        }
        store?;

        // Parse user filter upfront
        let user_filter = if let Some(ref user) = filter.user {
            Some(parse_user_filter(user)?)
        } else {
            None
        };

        let db = Arc::clone(&self.db);
        let filter_owned = filter.clone();

        tokio::task::spawn_blocking(move || {
            // Determine the best prefix to scan based on filters
            let prefix = if let Some(ref object_type) = filter_owned.object_type {
                Self::tuple_prefix_by_type(&store_id_owned, object_type)
            } else {
                Self::tuple_prefix(&store_id_owned)
            };

            let mut tuples = Vec::new();
            let iter = db.prefix_iterator(prefix.as_bytes());

            for item in iter {
                match item {
                    Ok((key, value)) => {
                        let key_str = String::from_utf8_lossy(&key);
                        if !key_str.starts_with(&prefix) {
                            break;
                        }

                        // Parse the key
                        let Some((
                            object_type,
                            object_id,
                            relation,
                            user_type,
                            user_id,
                            user_relation,
                        )) = Self::parse_tuple_key(&key_str)
                        else {
                            continue;
                        };

                        // Apply filters
                        if let Some(ref ot) = filter_owned.object_type {
                            if &object_type != ot {
                                continue;
                            }
                        }
                        if let Some(ref oi) = filter_owned.object_id {
                            if &object_id != oi {
                                continue;
                            }
                        }
                        if let Some(ref r) = filter_owned.relation {
                            if &relation != r {
                                continue;
                            }
                        }
                        if let Some((ref ut, ref ui, ref ur)) = user_filter {
                            if &user_type != ut || &user_id != ui || &user_relation != ur {
                                continue;
                            }
                        }

                        // Parse the value
                        let tuple_value: TupleValue = match serde_json::from_slice(&value) {
                            Ok(v) => v,
                            Err(e) => {
                                warn!(key = %key_str, error = %e, "Failed to deserialize tuple value, skipping");
                                continue;
                            }
                        };

                        // Apply condition filter
                        if let Some(ref cn) = filter_owned.condition_name {
                            if tuple_value.condition_name.as_ref() != Some(cn) {
                                continue;
                            }
                        }

                        tuples.push(StoredTuple {
                            object_type,
                            object_id,
                            relation,
                            user_type,
                            user_id,
                            user_relation,
                            condition_name: tuple_value.condition_name,
                            condition_context: tuple_value.condition_context,
                            created_at: tuple_value.created_at,
                        });
                    }
                    Err(e) => {
                        return Err(StorageError::QueryError {
                            message: format!("Failed to iterate tuples: {e}"),
                        });
                    }
                }
            }

            Ok(tuples)
        })
        .await
        .map_err(Self::join_error)?
    }

    async fn read_tuples_paginated(
        &self,
        store_id: &str,
        filter: &TupleFilter,
        pagination: &PaginationOptions,
    ) -> StorageResult<PaginatedResult<StoredTuple>> {
        // Get all matching tuples
        let mut tuples = self.read_tuples(store_id, filter).await?;

        // Sort for consistent pagination (by object_type, object_id, relation, user_type, user_id)
        tuples.sort_by(|a, b| {
            (
                &a.object_type,
                &a.object_id,
                &a.relation,
                &a.user_type,
                &a.user_id,
            )
                .cmp(&(
                    &b.object_type,
                    &b.object_id,
                    &b.relation,
                    &b.user_type,
                    &b.user_id,
                ))
        });

        let page_size = pagination.page_size.unwrap_or(100) as usize;
        let offset = parse_continuation_token(&pagination.continuation_token)? as usize;

        let items: Vec<StoredTuple> = tuples.drain(..).skip(offset).take(page_size).collect();

        let next_offset = offset + items.len();
        let continuation_token = if items.len() == page_size {
            Some(next_offset.to_string())
        } else {
            None
        };

        Ok(PaginatedResult {
            items,
            continuation_token,
        })
    }

    // =========================================================================
    // Query Optimization Methods
    // =========================================================================

    async fn list_objects_by_type(
        &self,
        store_id: &str,
        object_type: &str,
        limit: usize,
    ) -> StorageResult<Vec<String>> {
        validate_store_id(store_id)?;
        validate_object_type(object_type)?;

        // Check store exists
        self.get_store(store_id).await?;

        let db = Arc::clone(&self.db);
        let store_id_owned = store_id.to_string();
        let object_type_owned = object_type.to_string();

        tokio::task::spawn_blocking(move || {
            let prefix = Self::tuple_prefix_by_type(&store_id_owned, &object_type_owned);
            let mut unique_ids: HashSet<String> = HashSet::new();

            let iter = db.prefix_iterator(prefix.as_bytes());
            for item in iter {
                match item {
                    Ok((key, _)) => {
                        let key_str = String::from_utf8_lossy(&key);
                        if !key_str.starts_with(&prefix) {
                            break;
                        }

                        if let Some((_, object_id, _, _, _, _)) = Self::parse_tuple_key(&key_str) {
                            unique_ids.insert(object_id);
                            if unique_ids.len() >= limit {
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        return Err(StorageError::QueryError {
                            message: format!("Failed to list objects: {e}"),
                        });
                    }
                }
            }

            // Sort for deterministic results
            let mut result: Vec<String> = unique_ids.into_iter().collect();
            result.sort();
            result.truncate(limit);

            Ok(result)
        })
        .await
        .map_err(Self::join_error)?
    }

    async fn get_objects_with_parents(
        &self,
        store_id: &str,
        object_type: &str,
        relation: &str,
        parent_type: &str,
        parent_ids: &[String],
        limit: usize,
    ) -> StorageResult<Vec<ObjectWithCondition>> {
        if parent_ids.is_empty() {
            return Ok(Vec::new());
        }

        // Bounds check for consistency with other backends
        const MAX_PARENT_IDS: usize = 1000;
        if parent_ids.len() > MAX_PARENT_IDS {
            return Err(StorageError::InvalidInput {
                message: format!(
                    "too many parent IDs: {} (max {})",
                    parent_ids.len(),
                    MAX_PARENT_IDS
                ),
            });
        }

        validate_store_id(store_id)?;
        validate_object_type(object_type)?;

        // Check store exists
        self.get_store(store_id).await?;

        let db = Arc::clone(&self.db);
        let store_id_owned = store_id.to_string();
        let object_type_owned = object_type.to_string();
        let relation_owned = relation.to_string();
        let parent_type_owned = parent_type.to_string();
        let parent_ids_owned: HashSet<String> = parent_ids.iter().cloned().collect();

        tokio::task::spawn_blocking(move || {
            let prefix = Self::tuple_prefix_by_type(&store_id_owned, &object_type_owned);
            let mut results: Vec<ObjectWithCondition> = Vec::new();
            let mut seen: HashSet<String> = HashSet::new();

            let iter = db.prefix_iterator(prefix.as_bytes());
            for item in iter {
                if results.len() >= limit {
                    break;
                }

                match item {
                    Ok((key, value)) => {
                        let key_str = String::from_utf8_lossy(&key);
                        if !key_str.starts_with(&prefix) {
                            break;
                        }

                        if let Some((_, object_id, rel, user_type, user_id, _)) =
                            Self::parse_tuple_key(&key_str)
                        {
                            // Check if this tuple matches our criteria
                            if rel == relation_owned
                                && user_type == parent_type_owned
                                && parent_ids_owned.contains(&user_id)
                                && seen.insert(object_id.clone())
                            {
                                // Parse value for condition info
                                let tuple_value: TupleValue = match serde_json::from_slice(&value) {
                                    Ok(v) => v,
                                    Err(e) => {
                                        warn!(key = %key_str, error = %e, "Failed to deserialize tuple value, skipping");
                                        continue;
                                    }
                                };

                                results.push(ObjectWithCondition {
                                    object_id,
                                    condition_name: tuple_value.condition_name,
                                    condition_context: tuple_value.condition_context,
                                });
                            }
                        }
                    }
                    Err(e) => {
                        return Err(StorageError::QueryError {
                            message: format!("Failed to get objects with parents: {e}"),
                        });
                    }
                }
            }

            // Sort for deterministic results
            results.sort_by(|a, b| a.object_id.cmp(&b.object_id));

            Ok(results)
        })
        .await
        .map_err(Self::join_error)?
    }

    // =========================================================================
    // Authorization Model Operations
    // =========================================================================

    async fn write_authorization_model(
        &self,
        model: StoredAuthorizationModel,
    ) -> StorageResult<StoredAuthorizationModel> {
        validate_store_id(&model.store_id)?;

        // Check store exists
        self.get_store(&model.store_id).await?;

        let db = Arc::clone(&self.db);

        tokio::task::spawn_blocking(move || {
            let key = Self::model_key(&model.store_id, &model.id);
            let value =
                serde_json::to_vec(&model).map_err(|e| StorageError::SerializationError {
                    message: format!("Failed to serialize model: {e}"),
                })?;

            db.put(key.as_bytes(), &value)
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to write model: {e}"),
                })?;

            Ok(model)
        })
        .await
        .map_err(Self::join_error)?
    }

    async fn get_authorization_model(
        &self,
        store_id: &str,
        model_id: &str,
    ) -> StorageResult<StoredAuthorizationModel> {
        validate_store_id(store_id)?;

        // Check store exists
        self.get_store(store_id).await?;

        let db = Arc::clone(&self.db);
        let store_id_owned = store_id.to_string();
        let model_id_owned = model_id.to_string();

        tokio::task::spawn_blocking(move || {
            let key = Self::model_key(&store_id_owned, &model_id_owned);

            match db.get(key.as_bytes()) {
                Ok(Some(value)) => {
                    serde_json::from_slice(&value).map_err(|e| StorageError::SerializationError {
                        message: format!("Failed to deserialize model: {e}"),
                    })
                }
                Ok(None) => Err(StorageError::ModelNotFound {
                    model_id: model_id_owned,
                }),
                Err(e) => Err(StorageError::QueryError {
                    message: format!("Failed to get model: {e}"),
                }),
            }
        })
        .await
        .map_err(Self::join_error)?
    }

    async fn list_authorization_models(
        &self,
        store_id: &str,
    ) -> StorageResult<Vec<StoredAuthorizationModel>> {
        validate_store_id(store_id)?;

        // Check store exists
        self.get_store(store_id).await?;

        let db = Arc::clone(&self.db);
        let store_id_owned = store_id.to_string();

        tokio::task::spawn_blocking(move || {
            let prefix = Self::model_prefix(&store_id_owned);
            let mut models = Vec::new();

            let iter = db.prefix_iterator(prefix.as_bytes());
            for item in iter {
                match item {
                    Ok((key, value)) => {
                        let key_str = String::from_utf8_lossy(&key);
                        if !key_str.starts_with(&prefix) {
                            break;
                        }
                        match serde_json::from_slice::<StoredAuthorizationModel>(&value) {
                            Ok(model) => models.push(model),
                            Err(e) => {
                                warn!(key = %key_str, error = %e, "Failed to deserialize model, skipping");
                            }
                        }
                    }
                    Err(e) => {
                        return Err(StorageError::QueryError {
                            message: format!("Failed to iterate models: {e}"),
                        });
                    }
                }
            }

            // Sort by created_at DESC, id DESC (newest first, deterministic)
            models.sort_by(|a, b| {
                b.created_at
                    .cmp(&a.created_at)
                    .then_with(|| b.id.cmp(&a.id))
            });

            Ok(models)
        })
        .await
        .map_err(Self::join_error)?
    }

    async fn list_authorization_models_paginated(
        &self,
        store_id: &str,
        pagination: &PaginationOptions,
    ) -> StorageResult<PaginatedResult<StoredAuthorizationModel>> {
        let mut models = self.list_authorization_models(store_id).await?;

        let page_size = pagination.page_size.unwrap_or(100) as usize;
        let offset = parse_continuation_token(&pagination.continuation_token)? as usize;

        let items: Vec<StoredAuthorizationModel> =
            models.drain(..).skip(offset).take(page_size).collect();

        let next_offset = offset + items.len();
        let continuation_token = if items.len() == page_size {
            Some(next_offset.to_string())
        } else {
            None
        };

        Ok(PaginatedResult {
            items,
            continuation_token,
        })
    }

    async fn get_latest_authorization_model(
        &self,
        store_id: &str,
    ) -> StorageResult<StoredAuthorizationModel> {
        let models = self.list_authorization_models(store_id).await?;

        models
            .into_iter()
            .next()
            .ok_or_else(|| StorageError::ModelNotFound {
                model_id: format!("latest (no models exist for store {store_id})"),
            })
    }

    async fn delete_authorization_model(
        &self,
        store_id: &str,
        model_id: &str,
    ) -> StorageResult<()> {
        validate_store_id(store_id)?;

        // Check store exists
        self.get_store(store_id).await?;

        let db = Arc::clone(&self.db);
        let store_id_owned = store_id.to_string();
        let model_id_owned = model_id.to_string();

        tokio::task::spawn_blocking(move || {
            let key = Self::model_key(&store_id_owned, &model_id_owned);

            // Check if model exists
            if db
                .get(key.as_bytes())
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to check model: {e}"),
                })?
                .is_none()
            {
                return Err(StorageError::ModelNotFound {
                    model_id: model_id_owned,
                });
            }

            db.delete(key.as_bytes())
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to delete model: {e}"),
                })?;

            Ok(())
        })
        .await
        .map_err(Self::join_error)?
    }

    // =========================================================================
    // Changelog Operations
    // =========================================================================

    async fn read_changes(
        &self,
        store_id: &str,
        filter: &ReadChangesFilter,
        pagination: &PaginationOptions,
    ) -> StorageResult<PaginatedResult<TupleChange>> {
        // Check store exists
        self.get_store(store_id).await?;

        let db = Arc::clone(&self.db);
        let store_id_owned = store_id.to_string();
        let filter_owned = filter.clone();
        let offset = parse_continuation_token(&pagination.continuation_token)? as usize;
        let page_size = pagination.page_size.unwrap_or(50) as usize;

        tokio::task::spawn_blocking(move || {
            let prefix = Self::change_prefix(&store_id_owned);

            let mut changes: Vec<TupleChange> = Vec::new();
            let mut count = 0;
            let mut skipped = 0;

            let iter = db.prefix_iterator(prefix.as_bytes());
            for item in iter {
                match item {
                    Ok((key, value)) => {
                        let key_str = String::from_utf8_lossy(&key);
                        if !key_str.starts_with(&prefix) {
                            break;
                        }

                        let change_value: ChangeValue = match serde_json::from_slice(&value) {
                            Ok(v) => v,
                            Err(e) => {
                                warn!(key = %key_str, error = %e, "Failed to deserialize change value, skipping");
                                continue;
                            }
                        };

                        // Parse the tuple from the change
                        let Some((
                            object_type,
                            object_id,
                            relation,
                            user_type,
                            user_id,
                            user_relation,
                        )) = Self::parse_tuple_key(&change_value.tuple_key)
                        else {
                            continue;
                        };

                        // Apply type filter
                        if let Some(ref filter_type) = filter_owned.object_type {
                            if &object_type != filter_type {
                                continue;
                            }
                        }

                        // Apply offset
                        if skipped < offset {
                            skipped += 1;
                            continue;
                        }

                        let tuple = StoredTuple {
                            object_type,
                            object_id,
                            relation,
                            user_type,
                            user_id,
                            user_relation,
                            condition_name: change_value.tuple_value.condition_name,
                            condition_context: change_value.tuple_value.condition_context,
                            created_at: change_value.tuple_value.created_at,
                        };

                        let operation =
                            if change_value.operation == TupleOperation::Write.as_str() {
                                TupleOperation::Write
                            } else {
                                TupleOperation::Delete
                            };

                        changes.push(TupleChange {
                            tuple,
                            operation,
                            timestamp: change_value.timestamp,
                        });

                        count += 1;
                        if count > page_size {
                            break;
                        }
                    }
                    Err(e) => {
                        return Err(StorageError::QueryError {
                            message: format!("Failed to read changes: {e}"),
                        });
                    }
                }
            }

            let has_more = changes.len() > page_size;
            let items: Vec<TupleChange> = changes.into_iter().take(page_size).collect();

            let continuation_token = if has_more {
                Some((offset + page_size).to_string())
            } else {
                None
            };

            Ok(PaginatedResult {
                items,
                continuation_token,
            })
        })
        .await
        .map_err(Self::join_error)?
    }

    // =========================================================================
    // Health Check
    // =========================================================================

    async fn health_check(&self) -> StorageResult<HealthStatus> {
        let db = Arc::clone(&self.db);

        tokio::task::spawn_blocking(move || {
            let start = Instant::now();

            // Simple health check: write and read a test key
            let test_key = b"__health_check__";
            let test_value = b"ok";

            db.put(test_key, test_value)
                .map_err(|e| StorageError::HealthCheckFailed {
                    message: format!("Health check write failed: {e}"),
                })?;

            match db.get(test_key) {
                Ok(Some(value)) if value == test_value => {}
                Ok(_) => {
                    return Err(StorageError::HealthCheckFailed {
                        message: "Health check read returned unexpected value".to_string(),
                    });
                }
                Err(e) => {
                    return Err(StorageError::HealthCheckFailed {
                        message: format!("Health check read failed: {e}"),
                    });
                }
            }

            // Clean up test key
            let _ = db.delete(test_key);

            let latency = start.elapsed();

            Ok(HealthStatus {
                healthy: true,
                latency,
                pool_stats: None, // RocksDB doesn't have a connection pool
                message: Some("rocksdb".to_string()),
            })
        })
        .await
        .map_err(Self::join_error)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Creates a test RocksDB store in a temporary directory.
    fn create_test_store() -> (RocksDBDataStore, TempDir) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config = RocksDBConfig {
            path: temp_dir.path().to_string_lossy().to_string(),
            ..Default::default()
        };
        let store = RocksDBDataStore::new(config).expect("Failed to create store");
        (store, temp_dir)
    }

    // ==========================================================================
    // Store Operations Tests
    // ==========================================================================

    #[tokio::test]
    async fn test_create_and_get_store() {
        let (store, _temp) = create_test_store();

        let created = store.create_store("test-id", "Test Store").await.unwrap();
        assert_eq!(created.id, "test-id");
        assert_eq!(created.name, "Test Store");

        let retrieved = store.get_store("test-id").await.unwrap();
        assert_eq!(retrieved.id, created.id);
        assert_eq!(retrieved.name, created.name);
    }

    #[tokio::test]
    async fn test_get_nonexistent_store() {
        let (store, _temp) = create_test_store();

        let result = store.get_store("nonexistent").await;
        assert!(matches!(result, Err(StorageError::StoreNotFound { .. })));
    }

    #[tokio::test]
    async fn test_create_duplicate_store_fails() {
        let (store, _temp) = create_test_store();

        store.create_store("test-id", "Test Store").await.unwrap();
        let result = store.create_store("test-id", "Another Store").await;
        assert!(matches!(
            result,
            Err(StorageError::StoreAlreadyExists { .. })
        ));
    }

    #[tokio::test]
    async fn test_delete_store() {
        let (store, _temp) = create_test_store();

        store.create_store("test-id", "Test Store").await.unwrap();
        store.delete_store("test-id").await.unwrap();

        let result = store.get_store("test-id").await;
        assert!(matches!(result, Err(StorageError::StoreNotFound { .. })));
    }

    #[tokio::test]
    async fn test_update_store() {
        let (store, _temp) = create_test_store();

        store.create_store("test-id", "Old Name").await.unwrap();
        let updated = store.update_store("test-id", "New Name").await.unwrap();

        assert_eq!(updated.name, "New Name");

        let retrieved = store.get_store("test-id").await.unwrap();
        assert_eq!(retrieved.name, "New Name");
    }

    #[tokio::test]
    async fn test_list_stores() {
        let (store, _temp) = create_test_store();

        store.create_store("store1", "Store 1").await.unwrap();
        store.create_store("store2", "Store 2").await.unwrap();
        store.create_store("store3", "Store 3").await.unwrap();

        let stores = store.list_stores().await.unwrap();
        assert_eq!(stores.len(), 3);
    }

    // ==========================================================================
    // Tuple Operations Tests
    // ==========================================================================

    #[tokio::test]
    async fn test_write_and_read_tuple() {
        let (store, _temp) = create_test_store();
        store.create_store("test-store", "Test").await.unwrap();

        let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);
        store
            .write_tuple("test-store", tuple.clone())
            .await
            .unwrap();

        let filter = TupleFilter {
            object_type: Some("document".to_string()),
            ..Default::default()
        };
        let tuples = store.read_tuples("test-store", &filter).await.unwrap();

        assert_eq!(tuples.len(), 1);
        assert_eq!(tuples[0].object_id, "doc1");
        assert_eq!(tuples[0].user_id, "alice");
    }

    #[tokio::test]
    async fn test_write_is_idempotent() {
        let (store, _temp) = create_test_store();
        store.create_store("test-store", "Test").await.unwrap();

        let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);

        // Write same tuple twice
        store
            .write_tuple("test-store", tuple.clone())
            .await
            .unwrap();
        store.write_tuple("test-store", tuple).await.unwrap();

        let tuples = store
            .read_tuples("test-store", &TupleFilter::default())
            .await
            .unwrap();
        assert_eq!(tuples.len(), 1);
    }

    #[tokio::test]
    async fn test_delete_tuple() {
        let (store, _temp) = create_test_store();
        store.create_store("test-store", "Test").await.unwrap();

        let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);
        store
            .write_tuple("test-store", tuple.clone())
            .await
            .unwrap();

        // Delete the tuple
        store.delete_tuple("test-store", tuple).await.unwrap();

        let tuples = store
            .read_tuples("test-store", &TupleFilter::default())
            .await
            .unwrap();
        assert!(tuples.is_empty());
    }

    #[tokio::test]
    async fn test_filter_by_user() {
        let (store, _temp) = create_test_store();
        store.create_store("test-store", "Test").await.unwrap();

        let tuples = vec![
            StoredTuple::new("document", "doc1", "viewer", "user", "alice", None),
            StoredTuple::new("document", "doc2", "viewer", "user", "bob", None),
            StoredTuple::new("document", "doc3", "editor", "user", "alice", None),
        ];
        store
            .write_tuples("test-store", tuples, vec![])
            .await
            .unwrap();

        let filter = TupleFilter {
            user: Some("user:alice".to_string()),
            ..Default::default()
        };
        let result = store.read_tuples("test-store", &filter).await.unwrap();

        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|t| t.user_id == "alice"));
    }

    #[tokio::test]
    async fn test_tuple_with_condition() {
        let (store, _temp) = create_test_store();
        store.create_store("test-store", "Test").await.unwrap();

        let mut context = HashMap::new();
        context.insert("expires_at".to_string(), serde_json::json!("2024-12-31"));

        let tuple = StoredTuple::with_condition(
            "document",
            "doc1",
            "viewer",
            "user",
            "alice",
            None,
            "time_bound",
            Some(context.clone()),
        );
        store.write_tuple("test-store", tuple).await.unwrap();

        let tuples = store
            .read_tuples("test-store", &TupleFilter::default())
            .await
            .unwrap();

        assert_eq!(tuples.len(), 1);
        assert_eq!(tuples[0].condition_name, Some("time_bound".to_string()));
        assert!(tuples[0].condition_context.is_some());
    }

    #[tokio::test]
    async fn test_filter_by_condition_name() {
        let (store, _temp) = create_test_store();
        store.create_store("test-store", "Test").await.unwrap();

        let tuples = vec![
            StoredTuple::new("document", "doc1", "viewer", "user", "alice", None),
            StoredTuple::with_condition(
                "document",
                "doc2",
                "viewer",
                "user",
                "bob",
                None,
                "time_bound",
                None,
            ),
            StoredTuple::with_condition(
                "document",
                "doc3",
                "viewer",
                "user",
                "charlie",
                None,
                "region_check",
                None,
            ),
        ];
        store
            .write_tuples("test-store", tuples, vec![])
            .await
            .unwrap();

        let filter = TupleFilter {
            condition_name: Some("time_bound".to_string()),
            ..Default::default()
        };
        let result = store.read_tuples("test-store", &filter).await.unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].user_id, "bob");
    }

    // ==========================================================================
    // Query Optimization Tests
    // ==========================================================================

    #[tokio::test]
    async fn test_list_objects_by_type() {
        let (store, _temp) = create_test_store();
        store.create_store("test-store", "Test").await.unwrap();

        let tuples = vec![
            StoredTuple::new("document", "doc1", "viewer", "user", "alice", None),
            StoredTuple::new("document", "doc1", "editor", "user", "bob", None), // duplicate object
            StoredTuple::new("document", "doc2", "viewer", "user", "charlie", None),
            StoredTuple::new("folder", "folder1", "viewer", "user", "alice", None), // different type
        ];
        store
            .write_tuples("test-store", tuples, vec![])
            .await
            .unwrap();

        let objects = store
            .list_objects_by_type("test-store", "document", 100)
            .await
            .unwrap();

        assert_eq!(objects.len(), 2); // doc1 and doc2 (deduplicated)
        assert!(objects.contains(&"doc1".to_string()));
        assert!(objects.contains(&"doc2".to_string()));
    }

    #[tokio::test]
    async fn test_list_objects_by_type_with_limit() {
        let (store, _temp) = create_test_store();
        store.create_store("test-store", "Test").await.unwrap();

        for i in 0..10 {
            let tuple = StoredTuple::new(
                "document",
                format!("doc{i}"),
                "viewer",
                "user",
                "alice",
                None,
            );
            store.write_tuple("test-store", tuple).await.unwrap();
        }

        let objects = store
            .list_objects_by_type("test-store", "document", 5)
            .await
            .unwrap();
        assert_eq!(objects.len(), 5);
    }

    #[tokio::test]
    async fn test_get_objects_with_parents() {
        let (store, _temp) = create_test_store();
        store.create_store("test-store", "Test").await.unwrap();

        // Create documents with parent folders
        let tuples = vec![
            StoredTuple::new("document", "doc1", "parent", "folder", "folder1", None),
            StoredTuple::new("document", "doc2", "parent", "folder", "folder1", None),
            StoredTuple::new("document", "doc3", "parent", "folder", "folder2", None),
            StoredTuple::new("document", "doc4", "parent", "folder", "folder3", None),
        ];
        store
            .write_tuples("test-store", tuples, vec![])
            .await
            .unwrap();

        let results = store
            .get_objects_with_parents(
                "test-store",
                "document",
                "parent",
                "folder",
                &["folder1".to_string(), "folder2".to_string()],
                100,
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 3);
        let object_ids: Vec<_> = results.iter().map(|r| r.object_id.as_str()).collect();
        assert!(object_ids.contains(&"doc1"));
        assert!(object_ids.contains(&"doc2"));
        assert!(object_ids.contains(&"doc3"));
        assert!(!object_ids.contains(&"doc4"));
    }

    // ==========================================================================
    // Authorization Model Tests
    // ==========================================================================

    #[tokio::test]
    async fn test_write_and_get_model() {
        let (store, _temp) = create_test_store();
        store.create_store("test-store", "Test").await.unwrap();

        let model = StoredAuthorizationModel::new(
            "model-1",
            "test-store",
            "1.1",
            r#"{"type_definitions": []}"#,
        );
        store.write_authorization_model(model).await.unwrap();

        let retrieved = store
            .get_authorization_model("test-store", "model-1")
            .await
            .unwrap();
        assert_eq!(retrieved.id, "model-1");
        assert_eq!(retrieved.schema_version, "1.1");
    }

    #[tokio::test]
    async fn test_get_latest_model() {
        let (store, _temp) = create_test_store();
        store.create_store("test-store", "Test").await.unwrap();

        for i in 1..=3 {
            let model = StoredAuthorizationModel::new(
                format!("model-{i}"),
                "test-store",
                "1.1",
                r#"{"type_definitions": []}"#,
            );
            store.write_authorization_model(model).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let latest = store
            .get_latest_authorization_model("test-store")
            .await
            .unwrap();
        assert_eq!(latest.id, "model-3");
    }

    #[tokio::test]
    async fn test_delete_model() {
        let (store, _temp) = create_test_store();
        store.create_store("test-store", "Test").await.unwrap();

        let model = StoredAuthorizationModel::new(
            "model-1",
            "test-store",
            "1.1",
            r#"{"type_definitions": []}"#,
        );
        store.write_authorization_model(model).await.unwrap();

        store
            .delete_authorization_model("test-store", "model-1")
            .await
            .unwrap();

        let result = store.get_authorization_model("test-store", "model-1").await;
        assert!(matches!(result, Err(StorageError::ModelNotFound { .. })));
    }

    #[tokio::test]
    async fn test_list_models() {
        let (store, _temp) = create_test_store();
        store.create_store("test-store", "Test").await.unwrap();

        for i in 1..=3 {
            let model = StoredAuthorizationModel::new(
                format!("model-{i}"),
                "test-store",
                "1.1",
                r#"{"type_definitions": []}"#,
            );
            store.write_authorization_model(model).await.unwrap();
        }

        let models = store.list_authorization_models("test-store").await.unwrap();
        assert_eq!(models.len(), 3);
    }

    // ==========================================================================
    // Changelog Tests
    // ==========================================================================

    #[tokio::test]
    async fn test_read_changes() {
        let (store, _temp) = create_test_store();
        store.create_store("test-store", "Test").await.unwrap();

        // Write some tuples
        let tuple1 = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);
        let tuple2 = StoredTuple::new("document", "doc2", "viewer", "user", "bob", None);
        store
            .write_tuple("test-store", tuple1.clone())
            .await
            .unwrap();
        store.write_tuple("test-store", tuple2).await.unwrap();

        // Delete one
        store.delete_tuple("test-store", tuple1).await.unwrap();

        let changes = store
            .read_changes(
                "test-store",
                &ReadChangesFilter::default(),
                &PaginationOptions::default(),
            )
            .await
            .unwrap();

        assert_eq!(changes.items.len(), 3); // 2 writes + 1 delete
    }

    #[tokio::test]
    async fn test_read_changes_with_filter() {
        let (store, _temp) = create_test_store();
        store.create_store("test-store", "Test").await.unwrap();

        let tuples = vec![
            StoredTuple::new("document", "doc1", "viewer", "user", "alice", None),
            StoredTuple::new("folder", "folder1", "viewer", "user", "bob", None),
        ];
        store
            .write_tuples("test-store", tuples, vec![])
            .await
            .unwrap();

        let filter = ReadChangesFilter {
            object_type: Some("document".to_string()),
        };
        let changes = store
            .read_changes("test-store", &filter, &PaginationOptions::default())
            .await
            .unwrap();

        assert_eq!(changes.items.len(), 1);
        assert_eq!(changes.items[0].tuple.object_type, "document");
    }

    // ==========================================================================
    // Health Check Tests
    // ==========================================================================

    #[tokio::test]
    async fn test_health_check() {
        let (store, _temp) = create_test_store();

        let status = store.health_check().await.unwrap();

        assert!(status.healthy);
        assert_eq!(status.message, Some("rocksdb".to_string()));
        assert!(status.pool_stats.is_none());
    }

    // ==========================================================================
    // Pagination Tests
    // ==========================================================================

    #[tokio::test]
    async fn test_read_tuples_paginated() {
        let (store, _temp) = create_test_store();
        store.create_store("test-store", "Test").await.unwrap();

        // Write 10 tuples
        for i in 0..10 {
            let tuple = StoredTuple::new(
                "document",
                format!("doc{i:02}"),
                "viewer",
                "user",
                format!("user{i}"),
                None,
            );
            store.write_tuple("test-store", tuple).await.unwrap();
        }

        // First page
        let pagination = PaginationOptions {
            page_size: Some(3),
            continuation_token: None,
        };
        let result = store
            .read_tuples_paginated("test-store", &TupleFilter::default(), &pagination)
            .await
            .unwrap();
        assert_eq!(result.items.len(), 3);
        assert!(result.continuation_token.is_some());

        // Second page
        let pagination = PaginationOptions {
            page_size: Some(3),
            continuation_token: result.continuation_token,
        };
        let result = store
            .read_tuples_paginated("test-store", &TupleFilter::default(), &pagination)
            .await
            .unwrap();
        assert_eq!(result.items.len(), 3);
        assert!(result.continuation_token.is_some());
    }

    #[tokio::test]
    async fn test_list_stores_paginated() {
        let (store, _temp) = create_test_store();

        for i in 0..5 {
            store
                .create_store(&format!("store{i}"), &format!("Store {i}"))
                .await
                .unwrap();
        }

        let pagination = PaginationOptions {
            page_size: Some(2),
            continuation_token: None,
        };
        let result = store.list_stores_paginated(&pagination).await.unwrap();
        assert_eq!(result.items.len(), 2);
        assert!(result.continuation_token.is_some());

        // Continue to get all stores
        let mut all_stores = result.items;
        let mut token = result.continuation_token;

        while token.is_some() {
            let pagination = PaginationOptions {
                page_size: Some(2),
                continuation_token: token,
            };
            let result = store.list_stores_paginated(&pagination).await.unwrap();
            all_stores.extend(result.items);
            token = result.continuation_token;
        }

        assert_eq!(all_stores.len(), 5);
    }

    // ==========================================================================
    // Error Handling Tests
    // ==========================================================================

    #[tokio::test]
    async fn test_write_to_nonexistent_store_fails() {
        let (store, _temp) = create_test_store();

        let tuple = StoredTuple::new("document", "doc1", "viewer", "user", "alice", None);
        let result = store.write_tuple("nonexistent", tuple).await;

        assert!(matches!(result, Err(StorageError::StoreNotFound { .. })));
    }

    #[tokio::test]
    async fn test_invalid_store_id_fails() {
        let (store, _temp) = create_test_store();

        let result = store.create_store("", "Test").await;
        assert!(matches!(result, Err(StorageError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn test_invalid_user_filter_returns_error() {
        let (store, _temp) = create_test_store();
        store.create_store("test-store", "Test").await.unwrap();

        let filter = TupleFilter {
            user: Some("invalid".to_string()),
            ..Default::default()
        };
        let result = store.read_tuples("test-store", &filter).await;

        assert!(matches!(result, Err(StorageError::InvalidFilter { .. })));
    }

    // ==========================================================================
    // ULID Change ID Tests
    // ==========================================================================

    #[tokio::test]
    async fn test_change_ids_are_unique() {
        let (store, _temp) = create_test_store();
        store.create_store("test-store", "Test").await.unwrap();

        // Write multiple tuples rapidly
        for i in 0..100 {
            let tuple = StoredTuple::new(
                "document",
                format!("doc{i}"),
                "viewer",
                "user",
                "alice",
                None,
            );
            store.write_tuple("test-store", tuple).await.unwrap();
        }

        // Read all changes and verify no duplicate IDs
        let changes = store
            .read_changes(
                "test-store",
                &ReadChangesFilter::default(),
                &PaginationOptions {
                    page_size: Some(200),
                    continuation_token: None,
                },
            )
            .await
            .unwrap();

        assert_eq!(changes.items.len(), 100);

        // Verify all changes have unique tuples (no duplicates)
        let mut seen_tuples = std::collections::HashSet::new();
        for change in &changes.items {
            let tuple_key = format!(
                "{}:{}#{}@{}:{}",
                change.tuple.object_type,
                change.tuple.object_id,
                change.tuple.relation,
                change.tuple.user_type,
                change.tuple.user_id
            );
            assert!(
                seen_tuples.insert(tuple_key.clone()),
                "Duplicate tuple found: {tuple_key}"
            );
        }
    }
}
