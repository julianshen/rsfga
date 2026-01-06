//! PostgreSQL storage implementation.

use async_trait::async_trait;
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::Row;
use tracing::{debug, instrument};

use crate::error::{StorageError, StorageResult};
use crate::traits::{
    DataStore, PaginatedResult, PaginationOptions, Store, StoredTuple, TupleFilter,
};

/// PostgreSQL configuration options.
#[derive(Clone)]
pub struct PostgresConfig {
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
impl std::fmt::Debug for PostgresConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostgresConfig")
            .field("database_url", &"[REDACTED]")
            .field("max_connections", &self.max_connections)
            .field("min_connections", &self.min_connections)
            .field("connect_timeout_secs", &self.connect_timeout_secs)
            .finish()
    }
}

impl Default for PostgresConfig {
    fn default() -> Self {
        Self {
            database_url: "postgres://localhost/rsfga".to_string(),
            max_connections: 10,
            min_connections: 1,
            connect_timeout_secs: 30,
        }
    }
}

/// PostgreSQL implementation of DataStore.
pub struct PostgresDataStore {
    pool: PgPool,
}

impl PostgresDataStore {
    /// Creates a new PostgreSQL data store from a connection pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Creates a new PostgreSQL data store with the given configuration.
    #[instrument(skip(config), fields(database_url = %config.database_url))]
    pub async fn from_config(config: &PostgresConfig) -> StorageResult<Self> {
        let pool = PgPoolOptions::new()
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

    /// Creates a new PostgreSQL data store from a database URL.
    pub async fn from_url(database_url: &str) -> StorageResult<Self> {
        let config = PostgresConfig {
            database_url: database_url.to_string(),
            ..Default::default()
        };
        Self::from_config(&config).await
    }

    /// Runs database migrations to create required tables.
    #[instrument(skip(self))]
    pub async fn run_migrations(&self) -> StorageResult<()> {
        debug!("Running database migrations");

        // Create stores table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS stores (
                id VARCHAR(255) PRIMARY KEY,
                name VARCHAR(255) NOT NULL,
                created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StorageError::QueryError {
            message: format!("Failed to create stores table: {}", e),
        })?;

        // Create tuples table
        // Note: We use a surrogate primary key and a unique index instead of a composite
        // PRIMARY KEY with COALESCE, since PostgreSQL doesn't allow expressions in PKs.
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS tuples (
                id BIGSERIAL PRIMARY KEY,
                store_id VARCHAR(255) NOT NULL,
                object_type VARCHAR(255) NOT NULL,
                object_id VARCHAR(255) NOT NULL,
                relation VARCHAR(255) NOT NULL,
                user_type VARCHAR(255) NOT NULL,
                user_id VARCHAR(255) NOT NULL,
                user_relation VARCHAR(255),
                created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
                FOREIGN KEY (store_id) REFERENCES stores(id) ON DELETE CASCADE
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StorageError::QueryError {
            message: format!("Failed to create tuples table: {}", e),
        })?;

        // Create unique index to enforce tuple uniqueness (handles NULL user_relation correctly)
        sqlx::query(
            r#"
            CREATE UNIQUE INDEX IF NOT EXISTS idx_tuples_unique
            ON tuples (store_id, object_type, object_id, relation, user_type, user_id, COALESCE(user_relation, ''))
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StorageError::QueryError {
            message: format!("Failed to create unique index: {}", e),
        })?;

        // Create indexes for common query patterns
        let indexes = [
            "CREATE INDEX IF NOT EXISTS idx_tuples_store ON tuples(store_id)",
            "CREATE INDEX IF NOT EXISTS idx_tuples_object ON tuples(store_id, object_type, object_id)",
            "CREATE INDEX IF NOT EXISTS idx_tuples_user ON tuples(store_id, user_type, user_id)",
            "CREATE INDEX IF NOT EXISTS idx_tuples_relation ON tuples(store_id, object_type, object_id, relation)",
        ];

        for index_sql in indexes {
            sqlx::query(index_sql)
                .execute(&self.pool)
                .await
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to create index: {}", e),
                })?;
        }

        debug!("Database migrations completed successfully");
        Ok(())
    }

    /// Returns the connection pool for testing or advanced usage.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Parse user filter string into (user_type, user_id, Option<user_relation>).
    /// Format: "type:id" or "type:id#relation"
    fn parse_user_filter(user: &str) -> StorageResult<(String, String, Option<String>)> {
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
}

#[async_trait]
impl DataStore for PostgresDataStore {
    #[instrument(skip(self))]
    async fn create_store(&self, id: &str, name: &str) -> StorageResult<Store> {
        let now = chrono::Utc::now();

        sqlx::query(
            r#"
            INSERT INTO stores (id, name, created_at, updated_at)
            VALUES ($1, $2, $3, $4)
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
                if db_err.constraint() == Some("stores_pkey") {
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
            WHERE id = $1
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
            DELETE FROM stores WHERE id = $1
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
            LIMIT $1 OFFSET $2
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
        // Verify store exists
        let store_exists: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS(SELECT 1 FROM stores WHERE id = $1)
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

        // Batch delete using UNNEST arrays (fixes N+1 query problem)
        if !deletes.is_empty() {
            let object_types: Vec<&str> = deletes.iter().map(|t| t.object_type.as_str()).collect();
            let object_ids: Vec<&str> = deletes.iter().map(|t| t.object_id.as_str()).collect();
            let relations: Vec<&str> = deletes.iter().map(|t| t.relation.as_str()).collect();
            let user_types: Vec<&str> = deletes.iter().map(|t| t.user_type.as_str()).collect();
            let user_ids: Vec<&str> = deletes.iter().map(|t| t.user_id.as_str()).collect();
            let user_relations: Vec<Option<&str>> =
                deletes.iter().map(|t| t.user_relation.as_deref()).collect();

            sqlx::query(
                r#"
                DELETE FROM tuples t
                USING (
                    SELECT * FROM UNNEST($2::text[], $3::text[], $4::text[], $5::text[], $6::text[], $7::text[])
                    AS d(object_type, object_id, relation, user_type, user_id, user_relation)
                ) AS del
                WHERE t.store_id = $1
                  AND t.object_type = del.object_type
                  AND t.object_id = del.object_id
                  AND t.relation = del.relation
                  AND t.user_type = del.user_type
                  AND t.user_id = del.user_id
                  AND COALESCE(t.user_relation, '') = COALESCE(del.user_relation, '')
                "#,
            )
            .bind(store_id)
            .bind(&object_types)
            .bind(&object_ids)
            .bind(&relations)
            .bind(&user_types)
            .bind(&user_ids)
            .bind(&user_relations)
            .execute(&mut *tx)
            .await
            .map_err(|e| StorageError::QueryError {
                message: format!("Failed to batch delete tuples: {}", e),
            })?;
        }

        // Batch insert using UNNEST arrays with ON CONFLICT (fixes N+1 query problem)
        if !writes.is_empty() {
            let object_types: Vec<&str> = writes.iter().map(|t| t.object_type.as_str()).collect();
            let object_ids: Vec<&str> = writes.iter().map(|t| t.object_id.as_str()).collect();
            let relations: Vec<&str> = writes.iter().map(|t| t.relation.as_str()).collect();
            let user_types: Vec<&str> = writes.iter().map(|t| t.user_type.as_str()).collect();
            let user_ids: Vec<&str> = writes.iter().map(|t| t.user_id.as_str()).collect();
            let user_relations: Vec<Option<&str>> =
                writes.iter().map(|t| t.user_relation.as_deref()).collect();

            sqlx::query(
                r#"
                INSERT INTO tuples (store_id, object_type, object_id, relation, user_type, user_id, user_relation)
                SELECT $1, object_type, object_id, relation, user_type, user_id, user_relation
                FROM UNNEST($2::text[], $3::text[], $4::text[], $5::text[], $6::text[], $7::text[])
                AS t(object_type, object_id, relation, user_type, user_id, user_relation)
                ON CONFLICT (store_id, object_type, object_id, relation, user_type, user_id, (COALESCE(user_relation, '')))
                DO NOTHING
                "#,
            )
            .bind(store_id)
            .bind(&object_types)
            .bind(&object_ids)
            .bind(&relations)
            .bind(&user_types)
            .bind(&user_ids)
            .bind(&user_relations)
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
            SELECT EXISTS(SELECT 1 FROM stores WHERE id = $1)
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

        // Parse user filter upfront to validate and extract components
        let user_filter = if let Some(ref user) = filter.user {
            Some(Self::parse_user_filter(user)?)
        } else {
            None
        };

        // Use sqlx::QueryBuilder for safe dynamic query construction
        let mut builder: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
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
            SELECT EXISTS(SELECT 1 FROM stores WHERE id = $1)
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

        // Parse user filter upfront to validate and extract components
        let user_filter = if let Some(ref user) = filter.user {
            Some(Self::parse_user_filter(user)?)
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
        let mut builder: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
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

impl std::fmt::Debug for PostgresDataStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostgresDataStore")
            .field("pool", &"PgPool")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: These tests require a running PostgreSQL instance.
    // They are marked as ignored by default and should be run with:
    // cargo test -p rsfga-storage --lib postgres -- --ignored
    //
    // For CI, use testcontainers to spin up PostgreSQL.

    #[test]
    fn test_postgres_config_default() {
        let config = PostgresConfig::default();
        assert_eq!(config.max_connections, 10);
        assert_eq!(config.min_connections, 1);
        assert_eq!(config.connect_timeout_secs, 30);
    }

    #[test]
    fn test_postgres_datastore_debug() {
        // This tests that the Debug trait is properly implemented
        // (we can't actually create a PostgresDataStore without a database)
        fn _assert_debug<T: std::fmt::Debug>() {}
        _assert_debug::<PostgresDataStore>();
    }

    #[test]
    fn test_postgres_datastore_implements_datastore() {
        // Verify PostgresDataStore implements DataStore trait
        fn _assert_datastore<T: DataStore>() {}
        _assert_datastore::<PostgresDataStore>();
    }

    #[test]
    fn test_postgres_datastore_is_send_sync() {
        fn _assert_send_sync<T: Send + Sync>() {}
        _assert_send_sync::<PostgresDataStore>();
    }
}
