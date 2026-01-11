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
use sqlx::mysql::{MySqlPool, MySqlPoolOptions};
use sqlx::Row;
use tracing::{debug, instrument};

use crate::error::{StorageError, StorageResult};
use crate::traits::{
    parse_user_filter, validate_store_id, validate_store_name, validate_tuple, DataStore,
    PaginatedResult, PaginationOptions, Store, StoredTuple, TupleFilter,
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
}

// Custom Debug implementation to hide credentials in database_url
impl std::fmt::Debug for MySQLConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MySQLConfig")
            .field("database_url", &"[REDACTED]")
            .field("max_connections", &self.max_connections)
            .field("min_connections", &self.min_connections)
            .field("connect_timeout_secs", &self.connect_timeout_secs)
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
}

impl MySQLDataStore {
    /// Creates a new MySQL data store from a connection pool.
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
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

        Ok(Self { pool })
    }

    /// Creates a new MySQL data store from a database URL.
    pub async fn from_url(database_url: &str) -> StorageResult<Self> {
        let config = MySQLConfig {
            database_url: database_url.to_string(),
            ..Default::default()
        };
        Self::from_config(&config).await
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
            message: format!("Failed to create stores table: {}", e),
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
            message: format!("Failed to create tuples table: {}", e),
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

        // Create indexes for common query patterns
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
                message: format!("Invalid index name: {}", index_name),
            });
        }
        if !is_safe_identifier(table_name) {
            return Err(StorageError::QueryError {
                message: format!("Invalid table name: {}", table_name),
            });
        }
        if !is_safe_columns(columns) {
            return Err(StorageError::QueryError {
                message: format!("Invalid column specification: {}", columns),
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
            message: format!("Failed to check index existence: {}", e),
        })?;

        if !exists {
            let unique_clause = if unique { "UNIQUE" } else { "" };
            let query = format!(
                "CREATE {} INDEX {} ON {} {}",
                unique_clause, index_name, table_name, columns
            );
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
                        message: format!("Failed to create index {}: {}", index_name, e),
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

        sqlx::query(
            r#"
            INSERT INTO stores (id, name, created_at, updated_at)
            VALUES (?, ?, ?, ?)
            "#,
        )
        .bind(id)
        .bind(name)
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
                            store_id: id.to_string(),
                        };
                    }
                }
            }
            StorageError::QueryError {
                message: format!("Failed to create store: {}", e),
            }
        })?;

        Ok(Store {
            id: id.to_string(),
            name: name.to_string(),
            created_at: now,
            updated_at: now,
        })
    }

    #[instrument(skip(self))]
    async fn get_store(&self, id: &str) -> StorageResult<Store> {
        let row = sqlx::query(
            r#"
            SELECT id, name, created_at, updated_at
            FROM stores
            WHERE id = ?
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| StorageError::QueryError {
            message: format!("Failed to get store: {}", e),
        })?;

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
        let result = sqlx::query(
            r#"
            DELETE FROM stores WHERE id = ?
            "#,
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| StorageError::QueryError {
            message: format!("Failed to delete store: {}", e),
        })?;

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
        // atomicity between UPDATE and SELECT
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| StorageError::QueryError {
                message: format!("Failed to begin transaction: {}", e),
            })?;

        let result = sqlx::query(
            r#"
            UPDATE stores
            SET name = ?, updated_at = NOW()
            WHERE id = ?
            "#,
        )
        .bind(name)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| StorageError::QueryError {
            message: format!("Failed to update store: {}", e),
        })?;

        if result.rows_affected() == 0 {
            tx.rollback().await.ok();
            return Err(StorageError::StoreNotFound {
                store_id: id.to_string(),
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
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| StorageError::QueryError {
            message: format!("Failed to fetch updated store: {}", e),
        })?;

        tx.commit().await.map_err(|e| StorageError::QueryError {
            message: format!("Failed to commit transaction: {}", e),
        })?;

        Ok(Store {
            id: row.get("id"),
            name: row.get("name"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
        })
    }

    #[instrument(skip(self))]
    async fn list_stores(&self) -> StorageResult<Vec<Store>> {
        let rows = sqlx::query(
            r#"
            SELECT id, name, created_at, updated_at
            FROM stores
            ORDER BY created_at DESC, id DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StorageError::QueryError {
            message: format!("Failed to list stores: {}", e),
        })?;

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
        let offset: i64 = pagination
            .continuation_token
            .as_ref()
            .and_then(|t| t.parse().ok())
            .unwrap_or(0);

        let rows = sqlx::query(
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
            message: format!("Failed to list stores: {}", e),
        })?;

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
            message: format!("Failed to check store existence: {}", e),
        })?;

        if !store_exists {
            return Err(StorageError::StoreNotFound {
                store_id: store_id.to_string(),
            });
        }

        // Start a transaction
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| StorageError::TransactionError {
                message: format!("Failed to begin transaction: {}", e),
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
                    message: format!("Failed to batch delete tuples: {}", e),
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
                    message: format!("Failed to batch write tuples: {}", e),
                })?;
        }

        // Commit the transaction
        tx.commit()
            .await
            .map_err(|e| StorageError::TransactionError {
                message: format!("Failed to commit transaction: {}", e),
            })?;

        Ok(())
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
            message: format!("Failed to check store existence: {}", e),
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

        let rows =
            builder
                .build()
                .fetch_all(&self.pool)
                .await
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to read tuples: {}", e),
                })?;

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
            message: format!("Failed to check store existence: {}", e),
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

        let page_size = pagination.page_size.unwrap_or(100) as i64;
        let offset: i64 = pagination
            .continuation_token
            .as_ref()
            .and_then(|t| t.parse().ok())
            .unwrap_or(0);

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

        let rows =
            builder
                .build()
                .fetch_all(&self.pool)
                .await
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to read tuples: {}", e),
                })?;

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
