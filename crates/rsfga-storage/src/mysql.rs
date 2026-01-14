//! MySQL/MariaDB storage implementation.
//!
//! This module provides MySQL and MariaDB support for RSFGA storage.
//! TiDB is also compatible as it uses the MySQL wire protocol.

use async_trait::async_trait;

/// Maximum number of tuples to insert in a single batch INSERT statement.
/// MySQL has a limit on the number of placeholders (~65535). With 7 columns
/// per tuple, this gives us room for ~9000 tuples, but we use 1000 for safety
/// and to avoid excessively large queries that may cause timeouts.
const WRITE_BATCH_SIZE: usize = 1000;

/// Health check timeout in seconds.
/// Uses a shorter timeout than regular queries since health checks should be fast.
const HEALTH_CHECK_TIMEOUT_SECS: u64 = 5;
use sqlx::mysql::{MySqlPool, MySqlPoolOptions};
use sqlx::Row;
use tracing::{debug, instrument};

use crate::error::{HealthStatus, PoolStats, StorageError, StorageResult};
use crate::traits::{
    parse_continuation_token, parse_user_filter, validate_store_id, validate_store_name,
    validate_tuple, DataStore, PaginatedResult, PaginationOptions, Store, StoredAuthorizationModel,
    StoredTuple, TupleFilter,
};

/// MySQL configuration options.
#[derive(Clone)]
pub struct MySQLConfig {
    /// Database connection URL.
    pub database_url: String,
    /// Maximum number of connections in the pool.
    pub max_connections: u32,
    /// Minimum number of connections in the pool.
    pub min_connections: u32,
    /// Pool acquire timeout in seconds.
    ///
    /// This controls how long to wait when acquiring a connection from the pool,
    /// including time spent waiting for an available slot and establishing new
    /// connections. Note: This is passed to SQLx's `acquire_timeout()`, not
    /// TCP connect timeout.
    pub connect_timeout_secs: u64,
    /// Query timeout in seconds.
    ///
    /// Maximum time to wait for a query to complete. If a query exceeds this
    /// duration, it will be cancelled and return `StorageError::QueryTimeout`.
    /// Default: 30 seconds.
    pub query_timeout_secs: u64,
}

// Custom Debug implementation to hide credentials in database_url
impl std::fmt::Debug for MySQLConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MySQLConfig")
            .field("database_url", &"[REDACTED]")
            .field("max_connections", &self.max_connections)
            .field("min_connections", &self.min_connections)
            .field("connect_timeout_secs", &self.connect_timeout_secs)
            .field("query_timeout_secs", &self.query_timeout_secs)
            .finish()
    }
}

impl Default for MySQLConfig {
    fn default() -> Self {
        Self {
            database_url: "mysql://localhost/rsfga".to_string(),
            max_connections: 10,
            min_connections: 1,
            connect_timeout_secs: 30,
            query_timeout_secs: 30,
        }
    }
}

/// MySQL implementation of DataStore.
///
/// Supports MySQL 8.0+, MariaDB 10.5+, and TiDB.
///
/// # Pagination
///
/// This implementation uses OFFSET-based pagination with continuation tokens that
/// represent the offset position. Limitations to be aware of:
///
/// - **Performance**: Large offsets (e.g., page 1000) require scanning all preceding rows
/// - **Consistency**: Concurrent inserts/deletes may cause duplicates or gaps across pages
/// - **Ordering**: Results are ordered by `(created_at DESC, id DESC)` for deterministic pagination
///
/// For high-volume production use with deep pagination needs, consider cursor-based
/// pagination using the `id` column as a cursor (not yet implemented).
///
/// # Transactions
///
/// Individual transaction control (`begin_transaction`, `commit_transaction`, `rollback_transaction`)
/// is not supported. However, `write_tuples` operations are atomic internally - all writes
/// and deletes in a single call succeed or fail together.
pub struct MySQLDataStore {
    pool: MySqlPool,
    /// Query timeout duration.
    query_timeout: std::time::Duration,
}

impl MySQLDataStore {
    /// Creates a new MySQL data store from a connection pool.
    ///
    /// Uses default query timeout of 30 seconds.
    pub fn new(pool: MySqlPool) -> Self {
        Self {
            pool,
            query_timeout: std::time::Duration::from_secs(30),
        }
    }

    /// Creates a new MySQL data store from a connection pool with custom timeout.
    pub fn with_timeout(pool: MySqlPool, query_timeout: std::time::Duration) -> Self {
        Self {
            pool,
            query_timeout,
        }
    }

    /// Creates a new MySQL data store with the given configuration.
    #[instrument(skip(config))]
    pub async fn from_config(config: &MySQLConfig) -> StorageResult<Self> {
        let pool = MySqlPoolOptions::new()
            .max_connections(config.max_connections)
            .min_connections(config.min_connections)
            .acquire_timeout(std::time::Duration::from_secs(config.connect_timeout_secs))
            .connect(&config.database_url)
            .await
            .map_err(|e| StorageError::ConnectionError {
                message: e.to_string(),
            })?;

        Ok(Self {
            pool,
            query_timeout: std::time::Duration::from_secs(config.query_timeout_secs),
        })
    }

    /// Creates a new MySQL data store from a database URL.
    pub async fn from_url(database_url: &str) -> StorageResult<Self> {
        let config = MySQLConfig {
            database_url: database_url.to_string(),
            ..Default::default()
        };
        Self::from_config(&config).await
    }

    /// Wraps an async operation with a timeout.
    ///
    /// If the operation exceeds the configured query timeout, returns
    /// `StorageError::QueryTimeout` with details about the operation.
    async fn execute_with_timeout<T, F>(&self, operation: &str, future: F) -> StorageResult<T>
    where
        F: std::future::Future<Output = StorageResult<T>>,
    {
        match tokio::time::timeout(self.query_timeout, future).await {
            Ok(result) => result,
            Err(_elapsed) => Err(StorageError::QueryTimeout {
                operation: operation.to_string(),
                timeout: self.query_timeout,
            }),
        }
    }

    /// Runs database migrations to create required tables.
    ///
    /// This creates the stores and tuples tables with MySQL-specific syntax.
    /// The schema uses:
    /// - `BIGINT AUTO_INCREMENT` instead of PostgreSQL's `BIGSERIAL`
    /// - `ON UPDATE CURRENT_TIMESTAMP` for automatic updated_at tracking
    /// - Generated column for the unique index to handle NULL user_relation
    #[instrument(skip(self))]
    pub async fn run_migrations(&self) -> StorageResult<()> {
        debug!("Running MySQL database migrations");

        // Create stores table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS stores (
                id VARCHAR(255) PRIMARY KEY,
                name VARCHAR(255) NOT NULL,
                created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StorageError::QueryError {
            message: format!("Failed to create stores table: {e}"),
        })?;

        // Create tuples table
        // Note: MySQL requires a generated column to use COALESCE in a unique index.
        // The `user_relation_key` column is generated as COALESCE(user_relation, '').
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS tuples (
                id BIGINT AUTO_INCREMENT PRIMARY KEY,
                store_id VARCHAR(255) NOT NULL,
                object_type VARCHAR(255) NOT NULL,
                object_id VARCHAR(255) NOT NULL,
                relation VARCHAR(255) NOT NULL,
                user_type VARCHAR(255) NOT NULL,
                user_id VARCHAR(255) NOT NULL,
                user_relation VARCHAR(255) DEFAULT NULL,
                user_relation_key VARCHAR(255) AS (COALESCE(user_relation, '')) STORED,
                created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (store_id) REFERENCES stores(id) ON DELETE CASCADE
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StorageError::QueryError {
            message: format!("Failed to create tuples table: {e}"),
        })?;

        // Create unique index to enforce tuple uniqueness
        // Uses the generated column `user_relation_key` to handle NULL values
        self.create_index_if_not_exists(
            "idx_tuples_unique",
            "tuples",
            "(store_id, object_type, object_id, relation, user_type, user_id, user_relation_key)",
            true,
        )
        .await?;

        // Index Strategy for Tuple Queries
        //
        // These indexes are designed to optimize the most common authorization query patterns:
        //
        // idx_tuples_store: Base index for store-scoped queries. Used when listing all tuples
        // in a store or as a fallback when more specific indexes don't apply.
        //
        // idx_tuples_object: Optimizes "who has access to this object?" queries.
        // Common in Check operations when resolving direct object relationships.
        // Example: SELECT * FROM tuples WHERE store_id=? AND object_type=? AND object_id=?
        //
        // idx_tuples_user: Optimizes "what can this user access?" queries.
        // Used in ListObjects and reverse lookup operations.
        // Example: SELECT * FROM tuples WHERE store_id=? AND user_type=? AND user_id=?
        //
        // idx_tuples_relation: Optimizes queries filtering by relation on a specific object.
        // Critical for Check operations resolving specific relations.
        // Example: SELECT * FROM tuples WHERE store_id=? AND object_type=? AND object_id=? AND relation=?
        //
        // idx_tuples_store_relation: Optimizes queries that filter by relation across all objects.
        // Used for analytics and bulk operations on specific relation types.
        // Example: SELECT * FROM tuples WHERE store_id=? AND relation=?
        //
        // Note: MySQL doesn't support partial indexes, so no condition_name index is created here.
        // Conditional tuple queries may be slower on MySQL compared to PostgreSQL.
        self.create_index_if_not_exists("idx_tuples_store", "tuples", "(store_id)", false)
            .await?;

        self.create_index_if_not_exists(
            "idx_tuples_object",
            "tuples",
            "(store_id, object_type, object_id)",
            false,
        )
        .await?;

        self.create_index_if_not_exists(
            "idx_tuples_user",
            "tuples",
            "(store_id, user_type, user_id)",
            false,
        )
        .await?;

        self.create_index_if_not_exists(
            "idx_tuples_relation",
            "tuples",
            "(store_id, object_type, object_id, relation)",
            false,
        )
        .await?;

        self.create_index_if_not_exists(
            "idx_tuples_store_relation",
            "tuples",
            "(store_id, relation)",
            false,
        )
        .await?;

        // Create authorization_models table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS authorization_models (
                id VARCHAR(255) PRIMARY KEY,
                store_id VARCHAR(255) NOT NULL,
                schema_version VARCHAR(50) NOT NULL,
                model_json MEDIUMTEXT NOT NULL,
                created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (store_id) REFERENCES stores(id) ON DELETE CASCADE
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StorageError::QueryError {
            message: format!("Failed to create authorization_models table: {e}"),
        })?;

        // Index Strategy for Authorization Models
        //
        // idx_authorization_models_store: Composite index for efficient model listing.
        // Ordering: (store_id, created_at, id) ensures:
        //   1. Store-scoped queries are efficient (leading store_id column)
        //   2. Results are returned in timestamp order (created_at)
        //   3. Deterministic ordering when timestamps are identical (id tiebreaker)
        //   4. Efficient LIMIT/OFFSET pagination as rows are indexed in order
        //
        // Note: MySQL orders ASC by default; ORDER BY ... DESC in queries will use
        // a backwards index scan which is still efficient.
        //
        // Used by: list_authorization_models, get_latest_authorization_model
        self.create_index_if_not_exists(
            "idx_authorization_models_store",
            "authorization_models",
            "(store_id, created_at, id)",
            false,
        )
        .await?;

        debug!("MySQL database migrations completed successfully");
        Ok(())
    }

    /// Helper to create an index if it doesn't exist.
    /// MySQL doesn't have CREATE INDEX IF NOT EXISTS, so we check first.
    ///
    /// # Safety
    ///
    /// This function uses string formatting to build the CREATE INDEX query because
    /// SQL identifiers (table names, index names, columns) cannot be parameterized.
    /// All parameters are validated to contain only safe characters to prevent SQL injection.
    ///
    /// # Arguments
    ///
    /// * `index_name` - Must contain only alphanumeric characters and underscores
    /// * `table_name` - Must contain only alphanumeric characters and underscores
    /// * `columns` - Must contain only alphanumeric chars, underscores, parentheses, commas, spaces
    /// * `unique` - Whether to create a UNIQUE index
    async fn create_index_if_not_exists(
        &self,
        index_name: &str,
        table_name: &str,
        columns: &str,
        unique: bool,
    ) -> StorageResult<()> {
        // Validate identifiers to prevent SQL injection
        // Only alphanumeric and underscore allowed for names
        fn is_safe_identifier(s: &str) -> bool {
            !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }

        // Columns can also have parentheses, commas, and spaces for composite indexes
        fn is_safe_columns(s: &str) -> bool {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '(' | ')' | ',' | ' '))
        }

        if !is_safe_identifier(index_name) {
            return Err(StorageError::QueryError {
                message: format!("Invalid index name: {index_name}"),
            });
        }
        if !is_safe_identifier(table_name) {
            return Err(StorageError::QueryError {
                message: format!("Invalid table name: {table_name}"),
            });
        }
        if !is_safe_columns(columns) {
            return Err(StorageError::QueryError {
                message: format!("Invalid column specification: {columns}"),
            });
        }

        // Check if index exists
        let exists: bool = sqlx::query_scalar(
            r#"
            SELECT COUNT(*) > 0
            FROM information_schema.statistics
            WHERE table_schema = DATABASE()
              AND table_name = ?
              AND index_name = ?
            "#,
        )
        .bind(table_name)
        .bind(index_name)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| StorageError::QueryError {
            message: format!("Failed to check index existence: {e}"),
        })?;

        if !exists {
            let unique_clause = if unique { "UNIQUE" } else { "" };
            let query =
                format!("CREATE {unique_clause} INDEX {index_name} ON {table_name} {columns}");
            if let Err(e) = sqlx::query(&query).execute(&self.pool).await {
                // Handle TOCTOU race: ignore duplicate index error (1061 = ER_DUP_KEYNAME)
                // This can happen if concurrent migrations try to create the same index
                let is_duplicate_index = if let sqlx::Error::Database(ref db_err) = e {
                    db_err
                        .try_downcast_ref::<sqlx::mysql::MySqlDatabaseError>()
                        .is_some_and(|mysql_err| mysql_err.number() == 1061)
                } else {
                    false
                };

                if !is_duplicate_index {
                    return Err(StorageError::QueryError {
                        message: format!("Failed to create index {index_name}: {e}"),
                    });
                }
            }
        }

        Ok(())
    }

    /// Returns the connection pool for testing or advanced usage.
    pub fn pool(&self) -> &MySqlPool {
        &self.pool
    }
}

#[async_trait]
impl DataStore for MySQLDataStore {
    #[instrument(skip(self))]
    async fn create_store(&self, id: &str, name: &str) -> StorageResult<Store> {
        // Validate inputs
        validate_store_id(id)?;
        validate_store_name(name)?;

        let now = chrono::Utc::now();
        let id_owned = id.to_string();
        let name_owned = name.to_string();

        self.execute_with_timeout("create_store", async {
            sqlx::query(
                r#"
                INSERT INTO stores (id, name, created_at, updated_at)
                VALUES (?, ?, ?, ?)
                "#,
            )
            .bind(&id_owned)
            .bind(&name_owned)
            .bind(now)
            .bind(now)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                if let sqlx::Error::Database(db_err) = &e {
                    // Check for MySQL-specific duplicate entry error (1062 = ER_DUP_ENTRY)
                    if let Some(mysql_err) =
                        db_err.try_downcast_ref::<sqlx::mysql::MySqlDatabaseError>()
                    {
                        if mysql_err.number() == 1062 {
                            return StorageError::StoreAlreadyExists {
                                store_id: id_owned.clone(),
                            };
                        }
                    }
                }
                StorageError::QueryError {
                    message: format!("Failed to create store: {e}"),
                }
            })
        })
        .await?;

        Ok(Store {
            id: id.to_string(),
            name: name.to_string(),
            created_at: now,
            updated_at: now,
        })
    }

    #[instrument(skip(self))]
    async fn get_store(&self, id: &str) -> StorageResult<Store> {
        let id_owned = id.to_string();
        let row = self
            .execute_with_timeout("get_store", async {
                sqlx::query(
                    r#"
                    SELECT id, name, created_at, updated_at
                    FROM stores
                    WHERE id = ?
                    "#,
                )
                .bind(&id_owned)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to get store: {e}"),
                })
            })
            .await?;

        match row {
            Some(row) => Ok(Store {
                id: row.get("id"),
                name: row.get("name"),
                created_at: row.get("created_at"),
                updated_at: row.get("updated_at"),
            }),
            None => Err(StorageError::StoreNotFound {
                store_id: id.to_string(),
            }),
        }
    }

    #[instrument(skip(self))]
    async fn delete_store(&self, id: &str) -> StorageResult<()> {
        let id_owned = id.to_string();
        let result = self
            .execute_with_timeout("delete_store", async {
                sqlx::query(
                    r#"
                    DELETE FROM stores WHERE id = ?
                    "#,
                )
                .bind(&id_owned)
                .execute(&self.pool)
                .await
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to delete store: {e}"),
                })
            })
            .await?;

        if result.rows_affected() == 0 {
            return Err(StorageError::StoreNotFound {
                store_id: id.to_string(),
            });
        }

        Ok(())
    }

    #[instrument(skip(self))]
    async fn update_store(&self, id: &str, name: &str) -> StorageResult<Store> {
        // Validate inputs
        validate_store_id(id)?;
        validate_store_name(name)?;

        // MySQL doesn't support RETURNING, so we use a transaction to ensure
        // atomicity between UPDATE and SELECT.
        // Execute transaction with timeout protection for DoS prevention.
        let id_owned = id.to_string();
        let name_owned = name.to_string();
        let tx_result = tokio::time::timeout(self.query_timeout, async {
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|e| StorageError::TransactionError {
                    message: format!("Failed to begin transaction: {e}"),
                })?;

            let result = sqlx::query(
                r#"
                UPDATE stores
                SET name = ?, updated_at = NOW()
                WHERE id = ?
                "#,
            )
            .bind(&name_owned)
            .bind(&id_owned)
            .execute(&mut *tx)
            .await
            .map_err(|e| StorageError::QueryError {
                message: format!("Failed to update store: {e}"),
            })?;

            if result.rows_affected() == 0 {
                tx.rollback().await.ok();
                return Err(StorageError::StoreNotFound {
                    store_id: id_owned.clone(),
                });
            }

            // Fetch the updated store within the same transaction
            let row = sqlx::query(
                r#"
                SELECT id, name, created_at, updated_at
                FROM stores
                WHERE id = ?
                "#,
            )
            .bind(&id_owned)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| StorageError::QueryError {
                message: format!("Failed to fetch updated store: {e}"),
            })?;

            tx.commit()
                .await
                .map_err(|e| StorageError::TransactionError {
                    message: format!("Failed to commit transaction: {e}"),
                })?;

            Ok(Store {
                id: row.get("id"),
                name: row.get("name"),
                created_at: row.get("created_at"),
                updated_at: row.get("updated_at"),
            })
        })
        .await;

        match tx_result {
            Ok(result) => result,
            Err(_elapsed) => Err(StorageError::QueryTimeout {
                operation: "update_store_transaction".to_string(),
                timeout: self.query_timeout,
            }),
        }
    }

    #[instrument(skip(self))]
    async fn list_stores(&self) -> StorageResult<Vec<Store>> {
        let rows = self
            .execute_with_timeout("list_stores", async {
                sqlx::query(
                    r#"
                    SELECT id, name, created_at, updated_at
                    FROM stores
                    ORDER BY created_at DESC, id DESC
                    "#,
                )
                .fetch_all(&self.pool)
                .await
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to list stores: {e}"),
                })
            })
            .await?;

        Ok(rows
            .into_iter()
            .map(|row| Store {
                id: row.get("id"),
                name: row.get("name"),
                created_at: row.get("created_at"),
                updated_at: row.get("updated_at"),
            })
            .collect())
    }

    #[instrument(skip(self, pagination))]
    async fn list_stores_paginated(
        &self,
        pagination: &PaginationOptions,
    ) -> StorageResult<PaginatedResult<Store>> {
        let page_size = pagination.page_size.unwrap_or(100) as i64;
        let offset = parse_continuation_token(&pagination.continuation_token)?;

        let rows = self
            .execute_with_timeout("list_stores_paginated", async {
                sqlx::query(
                    r#"
                    SELECT id, name, created_at, updated_at
                    FROM stores
                    ORDER BY created_at DESC, id DESC
                    LIMIT ? OFFSET ?
                    "#,
                )
                .bind(page_size)
                .bind(offset)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to list stores: {e}"),
                })
            })
            .await?;

        let items: Vec<Store> = rows
            .into_iter()
            .map(|row| Store {
                id: row.get("id"),
                name: row.get("name"),
                created_at: row.get("created_at"),
                updated_at: row.get("updated_at"),
            })
            .collect();

        let next_offset = offset + items.len() as i64;
        let continuation_token = if items.len() == page_size as usize {
            Some(next_offset.to_string())
        } else {
            None
        };

        Ok(PaginatedResult {
            items,
            continuation_token,
        })
    }

    #[instrument(skip(self, writes, deletes))]
    async fn write_tuples(
        &self,
        store_id: &str,
        writes: Vec<StoredTuple>,
        deletes: Vec<StoredTuple>,
    ) -> StorageResult<()> {
        // Validate inputs
        validate_store_id(store_id)?;
        for tuple in &writes {
            validate_tuple(tuple)?;
        }
        for tuple in &deletes {
            validate_tuple(tuple)?;
        }

        // Verify store exists (with timeout protection)
        let store_id_owned = store_id.to_string();
        let store_exists: bool = self
            .execute_with_timeout("write_tuples_store_check", async {
                sqlx::query_scalar(
                    r#"
                    SELECT EXISTS(SELECT 1 FROM stores WHERE id = ?)
                    "#,
                )
                .bind(&store_id_owned)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to check store existence: {e}"),
                })
            })
            .await?;

        if !store_exists {
            return Err(StorageError::StoreNotFound {
                store_id: store_id.to_string(),
            });
        }

        // Execute transaction with timeout protection.
        // The entire transaction block is wrapped in a timeout to prevent DoS attacks
        // from slow queries. On timeout, the transaction is automatically rolled back
        // when `tx` is dropped.
        let tx_result = tokio::time::timeout(self.query_timeout, async {
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|e| StorageError::TransactionError {
                    message: format!("Failed to begin transaction: {e}"),
                })?;

            // Batch delete using tuple comparison with IN clause.
            // This reduces N deletes from N round trips to ceil(N/BATCH_SIZE) round trips.
            // We use `user_relation_key` (generated column) for NULL-safe comparison.
            for chunk in deletes.chunks(WRITE_BATCH_SIZE) {
                // Build batch DELETE query with tuple comparison
                let mut query = String::from(
                    "DELETE FROM tuples WHERE store_id = ? AND (object_type, object_id, relation, user_type, user_id, user_relation_key) IN (",
                );

                let placeholders: Vec<String> = chunk
                    .iter()
                    .map(|_| "(?, ?, ?, ?, ?, ?)".to_string())
                    .collect();
                query.push_str(&placeholders.join(", "));
                query.push(')');

                let mut query_builder = sqlx::query(&query);
                query_builder = query_builder.bind(store_id);

                for tuple in chunk {
                    // Use empty string for NULL user_relation to match generated column
                    let user_relation_key = tuple.user_relation.as_deref().unwrap_or("");
                    query_builder = query_builder
                        .bind(&tuple.object_type)
                        .bind(&tuple.object_id)
                        .bind(&tuple.relation)
                        .bind(&tuple.user_type)
                        .bind(&tuple.user_id)
                        .bind(user_relation_key);
                }

                query_builder
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| StorageError::QueryError {
                        message: format!("Failed to batch delete tuples: {e}"),
                    })?;
            }

            // Insert tuples using batch INSERT with multiple VALUES, chunked to avoid
            // exceeding MySQL's placeholder limit (~65535) and to prevent timeout issues.
            // ON DUPLICATE KEY UPDATE id = id is a no-op for idempotency.
            for chunk in writes.chunks(WRITE_BATCH_SIZE) {
                // Build batch insert query for this chunk
                let mut query = String::from(
                    "INSERT INTO tuples (store_id, object_type, object_id, relation, user_type, user_id, user_relation) VALUES ",
                );

                let placeholders: Vec<String> = chunk
                    .iter()
                    .map(|_| "(?, ?, ?, ?, ?, ?, ?)".to_string())
                    .collect();
                query.push_str(&placeholders.join(", "));
                query.push_str(" ON DUPLICATE KEY UPDATE id = id");

                let mut query_builder = sqlx::query(&query);

                for tuple in chunk {
                    query_builder = query_builder
                        .bind(store_id)
                        .bind(&tuple.object_type)
                        .bind(&tuple.object_id)
                        .bind(&tuple.relation)
                        .bind(&tuple.user_type)
                        .bind(&tuple.user_id)
                        .bind(&tuple.user_relation);
                }

                query_builder
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| StorageError::QueryError {
                        message: format!("Failed to batch write tuples: {e}"),
                    })?;
            }

            // Commit the transaction
            tx.commit()
                .await
                .map_err(|e| StorageError::TransactionError {
                    message: format!("Failed to commit transaction: {e}"),
                })?;

            Ok(())
        })
        .await;

        match tx_result {
            Ok(result) => result,
            Err(_elapsed) => Err(StorageError::QueryTimeout {
                operation: "write_tuples_transaction".to_string(),
                timeout: self.query_timeout,
            }),
        }
    }

    #[instrument(skip(self, filter))]
    async fn read_tuples(
        &self,
        store_id: &str,
        filter: &TupleFilter,
    ) -> StorageResult<Vec<StoredTuple>> {
        // Verify store exists
        let store_exists: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS(SELECT 1 FROM stores WHERE id = ?)
            "#,
        )
        .bind(store_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| StorageError::QueryError {
            message: format!("Failed to check store existence: {e}"),
        })?;

        if !store_exists {
            return Err(StorageError::StoreNotFound {
                store_id: store_id.to_string(),
            });
        }

        // Condition filtering is not yet supported
        if filter.condition_name.is_some() {
            return Err(StorageError::QueryError {
                message: "Filtering by condition_name is not yet supported in MySQL storage"
                    .to_string(),
            });
        }

        // Parse user filter upfront to validate and extract components
        let user_filter = if let Some(ref user) = filter.user {
            Some(parse_user_filter(user)?)
        } else {
            None
        };

        // Use sqlx::QueryBuilder for safe dynamic query construction
        let mut builder: sqlx::QueryBuilder<sqlx::MySql> = sqlx::QueryBuilder::new(
            "SELECT object_type, object_id, relation, user_type, user_id, user_relation FROM tuples WHERE store_id = ",
        );
        builder.push_bind(store_id);

        if let Some(ref object_type) = filter.object_type {
            builder.push(" AND object_type = ");
            builder.push_bind(object_type);
        }

        if let Some(ref object_id) = filter.object_id {
            builder.push(" AND object_id = ");
            builder.push_bind(object_id);
        }

        if let Some(ref relation) = filter.relation {
            builder.push(" AND relation = ");
            builder.push_bind(relation);
        }

        if let Some((user_type, user_id, user_relation)) = user_filter {
            builder.push(" AND user_type = ");
            builder.push_bind(user_type);
            builder.push(" AND user_id = ");
            builder.push_bind(user_id);
            if let Some(rel) = user_relation {
                builder.push(" AND user_relation = ");
                builder.push_bind(rel);
            } else {
                builder.push(" AND user_relation IS NULL");
            }
        }

        builder.push(" ORDER BY created_at DESC, id DESC");

        // Wrap the main query with timeout protection
        let pool = &self.pool;
        let rows = self
            .execute_with_timeout("read_tuples", async {
                builder
                    .build()
                    .fetch_all(pool)
                    .await
                    .map_err(|e| StorageError::QueryError {
                        message: format!("Failed to read tuples: {e}"),
                    })
            })
            .await?;

        Ok(rows
            .into_iter()
            .map(|row| StoredTuple {
                object_type: row.get("object_type"),
                object_id: row.get("object_id"),
                relation: row.get("relation"),
                user_type: row.get("user_type"),
                user_id: row.get("user_id"),
                user_relation: row.get("user_relation"),
                condition_name: None,
                condition_context: None,
            })
            .collect())
    }

    #[instrument(skip(self, filter, pagination))]
    async fn read_tuples_paginated(
        &self,
        store_id: &str,
        filter: &TupleFilter,
        pagination: &PaginationOptions,
    ) -> StorageResult<PaginatedResult<StoredTuple>> {
        // Verify store exists (with timeout protection)
        let store_id_owned = store_id.to_string();
        let store_exists: bool = self
            .execute_with_timeout("read_tuples_paginated_store_check", async {
                sqlx::query_scalar(
                    r#"
                    SELECT EXISTS(SELECT 1 FROM stores WHERE id = ?)
                    "#,
                )
                .bind(&store_id_owned)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to check store existence: {e}"),
                })
            })
            .await?;

        if !store_exists {
            return Err(StorageError::StoreNotFound {
                store_id: store_id.to_string(),
            });
        }

        // Condition filtering is not yet supported
        if filter.condition_name.is_some() {
            return Err(StorageError::QueryError {
                message: "Filtering by condition_name is not yet supported in MySQL storage"
                    .to_string(),
            });
        }

        // Parse user filter upfront to validate and extract components
        let user_filter = if let Some(ref user) = filter.user {
            Some(parse_user_filter(user)?)
        } else {
            None
        };

        let page_size = pagination.page_size.unwrap_or(100) as i64;
        let offset = parse_continuation_token(&pagination.continuation_token)?;

        // Use sqlx::QueryBuilder for safe dynamic query construction
        let mut builder: sqlx::QueryBuilder<sqlx::MySql> = sqlx::QueryBuilder::new(
            "SELECT object_type, object_id, relation, user_type, user_id, user_relation FROM tuples WHERE store_id = ",
        );
        builder.push_bind(store_id);

        if let Some(ref object_type) = filter.object_type {
            builder.push(" AND object_type = ");
            builder.push_bind(object_type);
        }

        if let Some(ref object_id) = filter.object_id {
            builder.push(" AND object_id = ");
            builder.push_bind(object_id);
        }

        if let Some(ref relation) = filter.relation {
            builder.push(" AND relation = ");
            builder.push_bind(relation);
        }

        if let Some((user_type, user_id, user_relation)) = user_filter {
            builder.push(" AND user_type = ");
            builder.push_bind(user_type);
            builder.push(" AND user_id = ");
            builder.push_bind(user_id);
            if let Some(rel) = user_relation {
                builder.push(" AND user_relation = ");
                builder.push_bind(rel);
            } else {
                builder.push(" AND user_relation IS NULL");
            }
        }

        builder.push(" ORDER BY created_at DESC, id DESC LIMIT ");
        builder.push_bind(page_size);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        // Execute main query with timeout protection
        let rows = self
            .execute_with_timeout("read_tuples_paginated", async {
                builder
                    .build()
                    .fetch_all(&self.pool)
                    .await
                    .map_err(|e| StorageError::QueryError {
                        message: format!("Failed to read tuples: {e}"),
                    })
            })
            .await?;

        let items: Vec<StoredTuple> = rows
            .into_iter()
            .map(|row| StoredTuple {
                object_type: row.get("object_type"),
                object_id: row.get("object_id"),
                relation: row.get("relation"),
                user_type: row.get("user_type"),
                user_id: row.get("user_id"),
                user_relation: row.get("user_relation"),
                condition_name: None,
                condition_context: None,
            })
            .collect();

        let next_offset = offset + items.len() as i64;
        let continuation_token = if items.len() == page_size as usize {
            Some(next_offset.to_string())
        } else {
            None
        };

        Ok(PaginatedResult {
            items,
            continuation_token,
        })
    }

    async fn begin_transaction(&self) -> StorageResult<()> {
        // No-op: Individual transaction control is not supported.
        // Callers MUST check supports_transactions() before calling this method.
        // Transactions are managed internally per write_tuples call.
        // Use write_tuples with both writes and deletes to get atomic behavior.
        Ok(())
    }

    async fn commit_transaction(&self) -> StorageResult<()> {
        // No-op: Individual transaction control is not supported.
        // Callers MUST check supports_transactions() before calling this method.
        // Transactions are committed automatically in write_tuples.
        Ok(())
    }

    async fn rollback_transaction(&self) -> StorageResult<()> {
        // No-op: Individual transaction control is not supported.
        // Callers MUST check supports_transactions() before calling this method.
        // Transactions are rolled back automatically on error in write_tuples.
        Ok(())
    }

    fn supports_transactions(&self) -> bool {
        // Returns false because individual transaction control (begin/commit/rollback)
        // is not supported. However, write_tuples operations are atomic internally.
        // Users should use write_tuples with both writes and deletes for atomic operations.
        // Callers MUST check this method before calling begin/commit/rollback_transaction.
        false
    }

    // Authorization model operations
    //
    // Note on SQL injection safety: All queries use sqlx parameterized bindings (?)
    // which prevent SQL injection by design. Input validation below ensures bounds checking
    // for defense-in-depth and consistent error handling.

    #[instrument(skip(self, model))]
    async fn write_authorization_model(
        &self,
        model: StoredAuthorizationModel,
    ) -> StorageResult<StoredAuthorizationModel> {
        // Validate input bounds (defense-in-depth, sqlx params prevent SQL injection)
        validate_store_id(&model.store_id)?;

        // Verify store exists (with timeout protection)
        let store_id = model.store_id.clone();
        let store_exists: bool = self
            .execute_with_timeout("write_authorization_model_store_check", async {
                sqlx::query_scalar(
                    r#"
                    SELECT EXISTS(SELECT 1 FROM stores WHERE id = ?)
                    "#,
                )
                .bind(&store_id)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to check store existence: {e}"),
                })
            })
            .await?;

        if !store_exists {
            return Err(StorageError::StoreNotFound {
                store_id: model.store_id.clone(),
            });
        }

        // Insert the model (with timeout protection)
        self.execute_with_timeout("write_authorization_model", async {
            sqlx::query(
                r#"
                INSERT INTO authorization_models (id, store_id, schema_version, model_json, created_at)
                VALUES (?, ?, ?, ?, ?)
                "#,
            )
            .bind(&model.id)
            .bind(&model.store_id)
            .bind(&model.schema_version)
            .bind(&model.model_json)
            .bind(model.created_at)
            .execute(&self.pool)
            .await
            .map_err(|e| StorageError::QueryError {
                message: format!("Failed to write authorization model: {e}"),
            })
        })
        .await?;

        Ok(model)
    }

    #[instrument(skip(self))]
    async fn get_authorization_model(
        &self,
        store_id: &str,
        model_id: &str,
    ) -> StorageResult<StoredAuthorizationModel> {
        // Validate input bounds
        validate_store_id(store_id)?;

        // Verify store exists (with timeout protection)
        let store_id_owned = store_id.to_string();
        let store_exists: bool = self
            .execute_with_timeout("get_authorization_model_store_check", async {
                sqlx::query_scalar(
                    r#"
                    SELECT EXISTS(SELECT 1 FROM stores WHERE id = ?)
                    "#,
                )
                .bind(&store_id_owned)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to check store existence: {e}"),
                })
            })
            .await?;

        if !store_exists {
            return Err(StorageError::StoreNotFound {
                store_id: store_id.to_string(),
            });
        }

        // Fetch the model (with timeout protection)
        let model_id_owned = model_id.to_string();
        let row = self
            .execute_with_timeout("get_authorization_model", async {
                sqlx::query(
                    r#"
                    SELECT id, store_id, schema_version, model_json, created_at
                    FROM authorization_models
                    WHERE store_id = ? AND id = ?
                    "#,
                )
                .bind(&store_id_owned)
                .bind(&model_id_owned)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to get authorization model: {e}"),
                })
            })
            .await?;

        match row {
            Some(row) => Ok(StoredAuthorizationModel {
                id: row.get("id"),
                store_id: row.get("store_id"),
                schema_version: row.get("schema_version"),
                model_json: row.get("model_json"),
                created_at: row.get("created_at"),
            }),
            None => Err(StorageError::ModelNotFound {
                model_id: model_id.to_string(),
            }),
        }
    }

    #[instrument(skip(self))]
    async fn list_authorization_models(
        &self,
        store_id: &str,
    ) -> StorageResult<Vec<StoredAuthorizationModel>> {
        // Validate input bounds
        validate_store_id(store_id)?;

        // Verify store exists (with timeout protection)
        let store_id_owned = store_id.to_string();
        let store_exists: bool = self
            .execute_with_timeout("list_authorization_models_store_check", async {
                sqlx::query_scalar(
                    r#"
                    SELECT EXISTS(SELECT 1 FROM stores WHERE id = ?)
                    "#,
                )
                .bind(&store_id_owned)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to check store existence: {e}"),
                })
            })
            .await?;

        if !store_exists {
            return Err(StorageError::StoreNotFound {
                store_id: store_id.to_string(),
            });
        }

        // Fetch all models for the store (with timeout protection)
        let rows = self
            .execute_with_timeout("list_authorization_models", async {
                sqlx::query(
                    r#"
                    SELECT id, store_id, schema_version, model_json, created_at
                    FROM authorization_models
                    WHERE store_id = ?
                    ORDER BY created_at DESC, id DESC
                    "#,
                )
                .bind(&store_id_owned)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to list authorization models: {e}"),
                })
            })
            .await?;

        Ok(rows
            .into_iter()
            .map(|row| StoredAuthorizationModel {
                id: row.get("id"),
                store_id: row.get("store_id"),
                schema_version: row.get("schema_version"),
                model_json: row.get("model_json"),
                created_at: row.get("created_at"),
            })
            .collect())
    }

    #[instrument(skip(self, pagination))]
    async fn list_authorization_models_paginated(
        &self,
        store_id: &str,
        pagination: &PaginationOptions,
    ) -> StorageResult<PaginatedResult<StoredAuthorizationModel>> {
        // Validate input bounds
        validate_store_id(store_id)?;

        // Verify store exists (with timeout protection)
        let store_id_owned = store_id.to_string();
        let store_exists: bool = self
            .execute_with_timeout("list_authorization_models_paginated_store_check", async {
                sqlx::query_scalar(
                    r#"
                    SELECT EXISTS(SELECT 1 FROM stores WHERE id = ?)
                    "#,
                )
                .bind(&store_id_owned)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to check store existence: {e}"),
                })
            })
            .await?;

        if !store_exists {
            return Err(StorageError::StoreNotFound {
                store_id: store_id.to_string(),
            });
        }

        let page_size = pagination.page_size.unwrap_or(100) as i64;
        let offset = parse_continuation_token(&pagination.continuation_token)?;

        // Fetch models with pagination (with timeout protection)
        let rows = self
            .execute_with_timeout("list_authorization_models_paginated", async {
                sqlx::query(
                    r#"
                    SELECT id, store_id, schema_version, model_json, created_at
                    FROM authorization_models
                    WHERE store_id = ?
                    ORDER BY created_at DESC, id DESC
                    LIMIT ? OFFSET ?
                    "#,
                )
                .bind(&store_id_owned)
                .bind(page_size)
                .bind(offset)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to list authorization models: {e}"),
                })
            })
            .await?;

        let items: Vec<StoredAuthorizationModel> = rows
            .into_iter()
            .map(|row| StoredAuthorizationModel {
                id: row.get("id"),
                store_id: row.get("store_id"),
                schema_version: row.get("schema_version"),
                model_json: row.get("model_json"),
                created_at: row.get("created_at"),
            })
            .collect();

        let next_offset = offset + items.len() as i64;
        let continuation_token = if items.len() == page_size as usize {
            Some(next_offset.to_string())
        } else {
            None
        };

        Ok(PaginatedResult {
            items,
            continuation_token,
        })
    }

    #[instrument(skip(self))]
    async fn get_latest_authorization_model(
        &self,
        store_id: &str,
    ) -> StorageResult<StoredAuthorizationModel> {
        // Validate input bounds
        validate_store_id(store_id)?;

        // Verify store exists (with timeout protection)
        let store_id_owned = store_id.to_string();
        let store_exists: bool = self
            .execute_with_timeout("get_latest_authorization_model_store_check", async {
                sqlx::query_scalar(
                    r#"
                    SELECT EXISTS(SELECT 1 FROM stores WHERE id = ?)
                    "#,
                )
                .bind(&store_id_owned)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to check store existence: {e}"),
                })
            })
            .await?;

        if !store_exists {
            return Err(StorageError::StoreNotFound {
                store_id: store_id.to_string(),
            });
        }

        // Fetch the most recent model (with timeout protection)
        let row = self
            .execute_with_timeout("get_latest_authorization_model", async {
                sqlx::query(
                    r#"
                    SELECT id, store_id, schema_version, model_json, created_at
                    FROM authorization_models
                    WHERE store_id = ?
                    ORDER BY created_at DESC, id DESC
                    LIMIT 1
                    "#,
                )
                .bind(&store_id_owned)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to get latest authorization model: {e}"),
                })
            })
            .await?;

        match row {
            Some(row) => Ok(StoredAuthorizationModel {
                id: row.get("id"),
                store_id: row.get("store_id"),
                schema_version: row.get("schema_version"),
                model_json: row.get("model_json"),
                created_at: row.get("created_at"),
            }),
            None => Err(StorageError::ModelNotFound {
                model_id: format!("latest (no models exist for store {store_id})"),
            }),
        }
    }

    #[instrument(skip(self))]
    async fn health_check(&self) -> StorageResult<HealthStatus> {
        let start = std::time::Instant::now();
        let health_timeout = std::time::Duration::from_secs(HEALTH_CHECK_TIMEOUT_SECS);

        // Execute a simple query to verify database connectivity
        // Uses a shorter dedicated timeout (5s) since health checks should be fast
        match tokio::time::timeout(health_timeout, async {
            sqlx::query("SELECT 1")
                .execute(&self.pool)
                .await
                .map_err(|e| StorageError::HealthCheckFailed {
                    message: format!("database ping failed: {e}"),
                })
        })
        .await
        {
            Ok(result) => {
                result?;
            }
            Err(_elapsed) => {
                return Err(StorageError::QueryTimeout {
                    operation: "health_check".to_string(),
                    timeout: health_timeout,
                });
            }
        }

        let latency = start.elapsed();

        // Get pool statistics
        // Note: pool.size() returns total connections, so active = size - idle
        let total_connections = self.pool.size();
        let idle_connections = self.pool.num_idle() as u32;
        let pool_stats = Some(PoolStats {
            active_connections: total_connections.saturating_sub(idle_connections),
            idle_connections,
            max_connections: self.pool.options().get_max_connections(),
        });

        Ok(HealthStatus {
            healthy: true,
            latency,
            pool_stats,
            message: Some("mysql".to_string()),
        })
    }
}

impl std::fmt::Debug for MySQLDataStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MySQLDataStore")
            .field("pool", &"MySqlPool")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: These tests require a running MySQL instance.
    // They are marked as ignored by default and should be run with:
    // cargo test -p rsfga-storage --lib mysql -- --ignored
    //
    // For CI, use testcontainers to spin up MySQL.

    #[test]
    fn test_mysql_config_default() {
        let config = MySQLConfig::default();
        assert_eq!(config.max_connections, 10);
        assert_eq!(config.min_connections, 1);
        assert_eq!(config.connect_timeout_secs, 30);
        assert_eq!(config.database_url, "mysql://localhost/rsfga");
    }

    #[test]
    fn test_mysql_config_debug_hides_credentials() {
        let config = MySQLConfig {
            database_url: "mysql://user:password@localhost/rsfga".to_string(),
            ..Default::default()
        };
        let debug_output = format!("{:?}", config);
        assert!(debug_output.contains("[REDACTED]"));
        assert!(!debug_output.contains("password"));
    }

    #[test]
    fn test_mysql_datastore_debug() {
        // This tests that the Debug trait is properly implemented
        // (we can't actually create a MySQLDataStore without a database)
        fn _assert_debug<T: std::fmt::Debug>() {}
        _assert_debug::<MySQLDataStore>();
    }

    #[test]
    fn test_mysql_datastore_implements_datastore() {
        // Verify MySQLDataStore implements DataStore trait
        fn _assert_datastore<T: DataStore>() {}
        _assert_datastore::<MySQLDataStore>();
    }

    #[test]
    fn test_mysql_datastore_is_send_sync() {
        fn _assert_send_sync<T: Send + Sync>() {}
        _assert_send_sync::<MySQLDataStore>();
    }
}
