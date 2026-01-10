//! MySQL/MariaDB storage implementation.
//!
//! This module provides MySQL and MariaDB support for RSFGA storage.
//! TiDB is also compatible as it uses the MySQL wire protocol.

use async_trait::async_trait;
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
    /// Connection timeout in seconds.
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
    async fn create_index_if_not_exists(
        &self,
        index_name: &str,
        table_name: &str,
        columns: &str,
        unique: bool,
    ) -> StorageResult<()> {
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
            sqlx::query(&query).execute(&self.pool).await.map_err(|e| {
                StorageError::QueryError {
                    message: format!("Failed to create index {}: {}", index_name, e),
                }
            })?;
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
            if let sqlx::Error::Database(ref db_err) = e {
                // MySQL error code 1062 is duplicate entry (ER_DUP_ENTRY)
                if db_err.code().as_deref() == Some("23000") {
                    return StorageError::StoreAlreadyExists {
                        store_id: id.to_string(),
                    };
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
    async fn list_stores(&self) -> StorageResult<Vec<Store>> {
        let rows = sqlx::query(
            r#"
            SELECT id, name, created_at, updated_at
            FROM stores
            ORDER BY created_at DESC
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
            ORDER BY created_at DESC
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

        // Delete tuples one by one (MySQL doesn't support UNNEST)
        // For better performance with large batches, consider using temp tables
        for tuple in &deletes {
            sqlx::query(
                r#"
                DELETE FROM tuples
                WHERE store_id = ?
                  AND object_type = ?
                  AND object_id = ?
                  AND relation = ?
                  AND user_type = ?
                  AND user_id = ?
                  AND COALESCE(user_relation, '') = COALESCE(?, '')
                "#,
            )
            .bind(store_id)
            .bind(&tuple.object_type)
            .bind(&tuple.object_id)
            .bind(&tuple.relation)
            .bind(&tuple.user_type)
            .bind(&tuple.user_id)
            .bind(&tuple.user_relation)
            .execute(&mut *tx)
            .await
            .map_err(|e| StorageError::QueryError {
                message: format!("Failed to delete tuple: {}", e),
            })?;
        }

        // Insert tuples using batch INSERT with multiple VALUES
        // ON DUPLICATE KEY UPDATE id = id is a no-op for idempotency
        if !writes.is_empty() {
            // Build batch insert query
            let mut query = String::from(
                "INSERT INTO tuples (store_id, object_type, object_id, relation, user_type, user_id, user_relation) VALUES ",
            );

            let placeholders: Vec<String> = writes
                .iter()
                .map(|_| "(?, ?, ?, ?, ?, ?, ?)".to_string())
                .collect();
            query.push_str(&placeholders.join(", "));
            query.push_str(" ON DUPLICATE KEY UPDATE id = id");

            let mut query_builder = sqlx::query(&query);

            for tuple in &writes {
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

        builder.push(" ORDER BY created_at DESC");

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

        builder.push(" ORDER BY created_at DESC LIMIT ");
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
        // Note: Individual transaction control is not supported.
        // Transactions are managed internally per write_tuples call.
        // Use write_tuples with both writes and deletes to get atomic behavior.
        Ok(())
    }

    async fn commit_transaction(&self) -> StorageResult<()> {
        // Note: Individual transaction control is not supported.
        // Transactions are committed automatically in write_tuples.
        Ok(())
    }

    async fn rollback_transaction(&self) -> StorageResult<()> {
        // Note: Individual transaction control is not supported.
        // Transactions are rolled back automatically on error in write_tuples.
        Ok(())
    }

    fn supports_transactions(&self) -> bool {
        // Returns false because individual transaction control (begin/commit/rollback)
        // is not supported. However, write_tuples operations are atomic internally.
        // Users should use write_tuples with both writes and deletes for atomic operations.
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
