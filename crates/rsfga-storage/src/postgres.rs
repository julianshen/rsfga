//! PostgreSQL storage implementation.

use async_trait::async_trait;
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::Row;
use std::collections::HashMap;
use tokio::sync::OnceCell;
use tracing::{debug, instrument};

use crate::error::{HealthStatus, PoolStats, StorageError, StorageResult};
use crate::traits::{
    parse_continuation_token, parse_user_filter, validate_store_id, validate_store_name,
    validate_tuple, DataStore, PaginatedResult, PaginationOptions, Store, StoredAuthorizationModel,
    StoredTuple, TupleFilter,
};

/// Maximum size of condition_context JSON in bytes (64 KB).
/// OpenFGA has a 512 KB overall request limit but no specific context limit.
/// We use 64 KB as a reasonable per-tuple limit for condition context.
const MAX_CONDITION_CONTEXT_SIZE: usize = 64 * 1024;

/// Default health check timeout in seconds.
/// Uses a shorter timeout than regular queries since health checks should be fast.
const DEFAULT_HEALTH_CHECK_TIMEOUT_SECS: u64 = 5;

/// Default query timeout in seconds.
const DEFAULT_QUERY_TIMEOUT_SECS: u64 = 30;

/// Validate condition_context size to prevent DoS via large JSON payloads.
fn validate_condition_context_size(tuple: &StoredTuple) -> StorageResult<()> {
    if let Some(ctx) = &tuple.condition_context {
        let json_str =
            serde_json::to_string(ctx).map_err(|e| StorageError::SerializationError {
                message: format!("Failed to serialize condition_context: {e}"),
            })?;
        if json_str.len() > MAX_CONDITION_CONTEXT_SIZE {
            return Err(StorageError::InvalidInput {
                message: format!(
                    "condition_context exceeds maximum size of {} bytes (actual: {} bytes)",
                    MAX_CONDITION_CONTEXT_SIZE,
                    json_str.len()
                ),
            });
        }
    }
    Ok(())
}

/// Parse condition_context JSONB value into a HashMap.
/// Returns an error if the JSON is present but malformed.
fn parse_condition_context(
    value: Option<serde_json::Value>,
) -> StorageResult<Option<HashMap<String, serde_json::Value>>> {
    match value {
        None => Ok(None),
        Some(v) => serde_json::from_value(v)
            .map(Some)
            .map_err(|e| StorageError::QueryError {
                message: format!("Failed to deserialize condition_context: {e}"),
            }),
    }
}

/// Parse a database row into a StoredTuple.
/// Shared between read_tuples and read_tuples_paginated.
fn row_to_stored_tuple(row: sqlx::postgres::PgRow) -> StorageResult<StoredTuple> {
    let condition_context: Option<serde_json::Value> = row.get("condition_context");
    Ok(StoredTuple {
        object_type: row.get("object_type"),
        object_id: row.get("object_id"),
        relation: row.get("relation"),
        user_type: row.get("user_type"),
        user_id: row.get("user_id"),
        user_relation: row.get("user_relation"),
        condition_name: row.get("condition_name"),
        condition_context: parse_condition_context(condition_context)?,
    })
}

/// Apply filter conditions to a query builder.
/// Shared between read_tuples and read_tuples_paginated.
fn apply_tuple_filters<'a>(
    builder: &mut sqlx::QueryBuilder<'a, sqlx::Postgres>,
    filter: &'a TupleFilter,
    user_filter: Option<(String, String, Option<String>)>,
) {
    if let Some(ref object_type) = filter.object_type {
        builder.push(" AND object_type = ");
        builder.push_bind(object_type.clone());
    }

    if let Some(ref object_id) = filter.object_id {
        builder.push(" AND object_id = ");
        builder.push_bind(object_id.clone());
    }

    if let Some(ref relation) = filter.relation {
        builder.push(" AND relation = ");
        builder.push_bind(relation.clone());
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

    if let Some(ref condition_name) = filter.condition_name {
        builder.push(" AND condition_name = ");
        builder.push_bind(condition_name.clone());
    }
}

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
    /// Default query timeout in seconds.
    ///
    /// Maximum time to wait for a query to complete. If a query exceeds this
    /// duration, it will be cancelled and return `StorageError::QueryTimeout`.
    /// Default: 30 seconds.
    pub query_timeout_secs: u64,
    /// Timeout for read operations in seconds.
    ///
    /// Applies to: `read_tuples`, `list_stores`, `get_store`, `get_authorization_model`, etc.
    /// Falls back to `query_timeout_secs` if not set.
    pub read_timeout_secs: Option<u64>,
    /// Timeout for write operations in seconds.
    ///
    /// Applies to: `create_store`, `write_tuples`, `delete_store`, etc.
    /// Falls back to `query_timeout_secs` if not set.
    pub write_timeout_secs: Option<u64>,
    /// Timeout for health checks in seconds.
    ///
    /// Should be shorter than query timeout for fast Kubernetes probe responses.
    /// Default: 5 seconds.
    pub health_check_timeout_secs: u64,
}

// Custom Debug implementation to hide credentials in database_url
impl std::fmt::Debug for PostgresConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostgresConfig")
            .field("database_url", &"[REDACTED]")
            .field("max_connections", &self.max_connections)
            .field("min_connections", &self.min_connections)
            .field("connect_timeout_secs", &self.connect_timeout_secs)
            .field("query_timeout_secs", &self.query_timeout_secs)
            .field("read_timeout_secs", &self.read_timeout_secs)
            .field("write_timeout_secs", &self.write_timeout_secs)
            .field("health_check_timeout_secs", &self.health_check_timeout_secs)
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
            query_timeout_secs: DEFAULT_QUERY_TIMEOUT_SECS,
            read_timeout_secs: None,
            write_timeout_secs: None,
            health_check_timeout_secs: DEFAULT_HEALTH_CHECK_TIMEOUT_SECS,
        }
    }
}

/// PostgreSQL implementation of DataStore.
pub struct PostgresDataStore {
    pool: PgPool,
    /// Default query timeout duration.
    query_timeout: std::time::Duration,
    /// Read operation timeout duration.
    read_timeout: std::time::Duration,
    /// Write operation timeout duration.
    write_timeout: std::time::Duration,
    /// Health check timeout duration.
    health_check_timeout: std::time::Duration,
    /// Cached result of CockroachDB detection.
    /// Uses OnceCell to detect once and cache for the lifetime of the store.
    is_cockroachdb_cache: OnceCell<bool>,
}

/// Parses a database version string to detect if it's CockroachDB.
///
/// CockroachDB version strings typically look like:
/// - "CockroachDB CCL v23.1.0 (x86_64-pc-linux-gnu, built 2023/04/26 14:41:24, go1.19.6)"
/// - "CockroachDB CCL v25.4.0-beta..."
///
/// PostgreSQL version strings look like:
/// - "PostgreSQL 15.2 on x86_64-pc-linux-gnu, compiled by gcc ..."
/// - "PostgreSQL 14.0 (Debian 14.0-1.pgdg110+1) on x86_64-pc-linux-gnu..."
///
/// This function performs case-insensitive matching for robustness.
fn is_cockroachdb_version(version_string: &str) -> bool {
    version_string.to_lowercase().contains("cockroachdb")
}

/// Helper to build UNNEST clauses for batch operations.
/// Handles the syntax difference between CockroachDB (ROWS FROM) and PostgreSQL (multi-arg UNNEST).
fn build_unnest_clause(
    use_rows_from: bool,
    alias: &str,
    columns: &[(&str, &str)], // (sql_type_suffix, column_name)
) -> String {
    let col_names = columns
        .iter()
        .map(|(_, name)| *name)
        .collect::<Vec<_>>()
        .join(", ");

    if use_rows_from {
        // CockroachDB syntax: ROWS FROM (UNNEST($2...), UNNEST($3...)...) AS alias(cols)
        let unnest_calls = columns
            .iter()
            .enumerate()
            .map(|(i, (ty, _))| format!("UNNEST(${}{})", i + 2, ty))
            .collect::<Vec<_>>()
            .join(",\n            ");
            
        format!("ROWS FROM (\n            {}\n        ) AS {}({})", unnest_calls, alias, col_names)
    } else {
        // PostgreSQL syntax: UNNEST($2..., $3...) AS alias(cols)
        let args = columns
            .iter()
            .enumerate()
            .map(|(i, (ty, _))| format!("${}{}", i + 2, ty))
            .collect::<Vec<_>>()
            .join(", ");
            
        format!("UNNEST({}) AS {}({})", args, alias, col_names)
    }
}

/// Builds the UNNEST clause for batch delete operations.
///
/// CockroachDB requires `ROWS FROM (UNNEST(...), UNNEST(...))` syntax while
/// PostgreSQL uses `UNNEST(..., ...) AS t(...)`.
///
/// The 6-column format (without condition columns) is used for delete operations.
fn build_delete_unnest(use_rows_from: bool) -> String {
    let columns = [
        ("::text[]", "object_type"),
        ("::text[]", "object_id"),
        ("::text[]", "relation"),
        ("::text[]", "user_type"),
        ("::text[]", "user_id"),
        ("::text[]", "user_relation"),
    ];
    build_unnest_clause(use_rows_from, "d", &columns)
}

/// Builds the UNNEST clause for batch insert/conflict-check operations.
///
/// CockroachDB requires `ROWS FROM` syntax while PostgreSQL uses multi-array UNNEST.
/// The 8-column format (with condition columns) is used for insert and conflict check operations.
fn build_insert_unnest(use_rows_from: bool, alias: &str) -> String {
    let columns = [
        ("::text[]", "object_type"),
        ("::text[]", "object_id"),
        ("::text[]", "relation"),
        ("::text[]", "user_type"),
        ("::text[]", "user_id"),
        ("::text[]", "user_relation"),
        ("::text[]", "condition_name"),
        ("::jsonb[]", "condition_context"),
    ];
    build_unnest_clause(use_rows_from, alias, &columns)
}

impl PostgresDataStore {
    /// Creates a new PostgreSQL data store from a connection pool.
    ///
    /// Uses default query timeout of 30 seconds for all operations.
    pub fn new(pool: PgPool) -> Self {
        let default_timeout = std::time::Duration::from_secs(DEFAULT_QUERY_TIMEOUT_SECS);
        Self {
            pool,
            query_timeout: default_timeout,
            read_timeout: default_timeout,
            write_timeout: default_timeout,
            health_check_timeout: std::time::Duration::from_secs(DEFAULT_HEALTH_CHECK_TIMEOUT_SECS),
            is_cockroachdb_cache: OnceCell::new(),
        }
    }

    /// Creates a new PostgreSQL data store from a connection pool with custom timeout.
    ///
    /// The provided timeout is used for all operation types.
    pub fn with_timeout(pool: PgPool, query_timeout: std::time::Duration) -> Self {
        Self {
            pool,
            query_timeout,
            read_timeout: query_timeout,
            write_timeout: query_timeout,
            health_check_timeout: std::time::Duration::from_secs(DEFAULT_HEALTH_CHECK_TIMEOUT_SECS),
            is_cockroachdb_cache: OnceCell::new(),
        }
    }

    /// Creates a new PostgreSQL data store with the given configuration.
    #[instrument(skip(config))]
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

        let default_timeout = std::time::Duration::from_secs(config.query_timeout_secs);

        Ok(Self {
            pool,
            query_timeout: default_timeout,
            read_timeout: config
                .read_timeout_secs
                .map(std::time::Duration::from_secs)
                .unwrap_or(default_timeout),
            write_timeout: config
                .write_timeout_secs
                .map(std::time::Duration::from_secs)
                .unwrap_or(default_timeout),
            health_check_timeout: std::time::Duration::from_secs(config.health_check_timeout_secs),
            is_cockroachdb_cache: OnceCell::new(),
        })
    }

    /// Creates a new PostgreSQL data store from a database URL.
    pub async fn from_url(database_url: &str) -> StorageResult<Self> {
        let config = PostgresConfig {
            database_url: database_url.to_string(),
            ..Default::default()
        };
        Self::from_config(&config).await
    }

    /// Detects if the connected database is CockroachDB.
    ///
    /// CockroachDB uses the PostgreSQL wire protocol but has different SQL
    /// capabilities. This detection is used to select compatible migration
    /// strategies (e.g., CockroachDB doesn't support ALTER TABLE in functions
    /// but does support ADD COLUMN IF NOT EXISTS at the DDL level).
    ///
    /// The result is cached using `OnceCell` to avoid repeated database queries
    /// since the database type won't change during the lifetime of the store.
    async fn is_cockroachdb(&self) -> StorageResult<bool> {
        // Use get_or_try_init to cache the result and avoid repeated queries
        self.is_cockroachdb_cache
            .get_or_try_init(|| async {
                let row: (String,) = sqlx::query_as("SELECT version()")
                    .fetch_one(&self.pool)
                    .await
                    .map_err(|e| StorageError::QueryError {
                        message: format!(
                            "Failed to detect database version: {e}. \
                             Ensure the database connection is valid and the user has \
                             permission to execute SELECT version()."
                        ),
                    })?;
                let is_crdb = is_cockroachdb_version(&row.0);
                debug!(
                    version = %row.0,
                    is_cockroachdb = is_crdb,
                    "Detected database type"
                );
                Ok(is_crdb)
            })
            .await
            .copied()
    }

    /// Wraps an async operation with a timeout and records metrics.
    ///
    /// If the operation exceeds the specified timeout, returns
    /// `StorageError::QueryTimeout` with details about the operation.
    ///
    /// # Arguments
    /// * `operation` - Human-readable name of the operation (for error messages and metrics)
    /// * `timeout` - Maximum duration to wait for the operation
    /// * `future` - The async operation to execute
    ///
    /// # Metrics
    /// - `rsfga_storage_query_duration_seconds` - Histogram of query durations
    /// - `rsfga_storage_query_timeout_total` - Counter of timeout events
    async fn execute_with_timeout_and_metrics<T, F>(
        &self,
        operation: &str,
        timeout: std::time::Duration,
        future: F,
    ) -> StorageResult<T>
    where
        F: std::future::Future<Output = StorageResult<T>>,
    {
        let start = std::time::Instant::now();
        let result = tokio::time::timeout(timeout, future).await;
        let duration = start.elapsed().as_secs_f64();

        let (status, final_result) = match result {
            Ok(Ok(value)) => ("success", Ok(value)),
            Ok(Err(StorageError::QueryTimeout { .. })) => (
                "timeout",
                Err(StorageError::QueryTimeout {
                    operation: operation.to_string(),
                    timeout,
                }),
            ),
            Ok(Err(e)) => ("error", Err(e)),
            Err(_elapsed) => (
                "timeout",
                Err(StorageError::QueryTimeout {
                    operation: operation.to_string(),
                    timeout,
                }),
            ),
        };

        // Record duration histogram
        metrics::histogram!(
            "rsfga_storage_query_duration_seconds",
            "operation" => operation.to_string(),
            "backend" => "postgres",
            "status" => status.to_string()
        )
        .record(duration);

        // Record timeout counter if applicable
        if status == "timeout" {
            metrics::counter!(
                "rsfga_storage_query_timeout_total",
                "operation" => operation.to_string(),
                "backend" => "postgres"
            )
            .increment(1);
        }

        final_result
    }

    /// Executes a read operation with the configured read timeout.
    #[allow(dead_code)] // Will be used when operations are migrated
    async fn execute_read<T, F>(&self, operation: &str, future: F) -> StorageResult<T>
    where
        F: std::future::Future<Output = StorageResult<T>>,
    {
        self.execute_with_timeout_and_metrics(operation, self.read_timeout, future)
            .await
    }

    /// Executes a write operation with the configured write timeout.
    #[allow(dead_code)] // Will be used when operations are migrated
    async fn execute_write<T, F>(&self, operation: &str, future: F) -> StorageResult<T>
    where
        F: std::future::Future<Output = StorageResult<T>>,
    {
        self.execute_with_timeout_and_metrics(operation, self.write_timeout, future)
            .await
    }

    /// Wraps an async operation with default query timeout.
    ///
    /// This method is kept for backwards compatibility and operations that
    /// don't fit clearly into read or write categories.
    async fn execute_with_timeout<T, F>(&self, operation: &str, future: F) -> StorageResult<T>
    where
        F: std::future::Future<Output = StorageResult<T>>,
    {
        self.execute_with_timeout_and_metrics(operation, self.query_timeout, future)
            .await
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
            message: format!("Failed to create stores table: {e}"),
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
                condition_name VARCHAR(255),
                condition_context JSONB,
                created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
                FOREIGN KEY (store_id) REFERENCES stores(id) ON DELETE CASCADE
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StorageError::QueryError {
            message: format!("Failed to create tuples table: {e}"),
        })?;

        // Add condition columns if they don't exist (for existing databases)
        // These are idempotent migrations that add columns only if missing.
        //
        // CockroachDB doesn't support ALTER TABLE inside PL/pgSQL functions,
        // but it does support ADD COLUMN IF NOT EXISTS at the DDL level.
        // PostgreSQL doesn't support IF NOT EXISTS for ADD COLUMN in older versions,
        // so we use a DO block for PostgreSQL and direct DDL for CockroachDB.
        if self.is_cockroachdb().await? {
            // CockroachDB: Use ADD COLUMN IF NOT EXISTS (supported at DDL level)
            // Combine both columns in a single statement for efficiency and atomicity.
            //
            // Note on atomicity: CockroachDB executes multi-column ALTER TABLE as a
            // single schema change operation. If one column addition fails, the entire
            // statement is rolled back, ensuring the schema remains consistent.
            // Tested with CockroachDB v23.x - v25.x.
            sqlx::query(
                "ALTER TABLE tuples \
                 ADD COLUMN IF NOT EXISTS condition_name VARCHAR(255), \
                 ADD COLUMN IF NOT EXISTS condition_context JSONB",
            )
            .execute(&self.pool)
            .await
            .map_err(|e| StorageError::QueryError {
                message: format!(
                    "Failed to add condition columns on CockroachDB: {e}. \
                     Ensure the database user has ALTER TABLE privileges on the 'tuples' table."
                ),
            })?;
        } else {
            // PostgreSQL: Use DO block for conditional column addition
            // Note: We filter by table_schema = current_schema() to ensure we check
            // the correct table in the current search_path, avoiding false matches
            // from tables with the same name in other schemas.
            sqlx::query(
                r#"
                DO $$
                BEGIN
                    IF NOT EXISTS (
                        SELECT 1 FROM information_schema.columns
                        WHERE table_schema = current_schema()
                          AND table_name = 'tuples'
                          AND column_name = 'condition_name'
                    ) THEN
                        ALTER TABLE tuples ADD COLUMN condition_name VARCHAR(255);
                    END IF;
                    IF NOT EXISTS (
                        SELECT 1 FROM information_schema.columns
                        WHERE table_schema = current_schema()
                          AND table_name = 'tuples'
                          AND column_name = 'condition_context'
                    ) THEN
                        ALTER TABLE tuples ADD COLUMN condition_context JSONB;
                    END IF;
                END $$;
                "#,
            )
            .execute(&self.pool)
            .await
            .map_err(|e| StorageError::QueryError {
                message: format!(
                    "Failed to add condition columns on PostgreSQL: {e}. \
                     Ensure the database user has ALTER TABLE privileges and \
                     the 'plpgsql' extension is available."
                ),
            })?;
        }

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
            message: format!("Failed to create unique index: {e}"),
        })?;

        // Index Strategy for Tuple Queries
        //
        // These indexes are designed to optimize the most common authorization query patterns:
        //
        // idx_tuples_store: Base index for store-scoped queries. Used when listing all tuples
        // in a store or as a fallback when more specific indexes don't apply.
        //
        // idx_tuples_object: Optimizes "who has access to this object?" queries.
        // Common in Check operations when resolving direct object relationships.
        // Example: SELECT * FROM tuples WHERE store_id=$1 AND object_type=$2 AND object_id=$3
        //
        // idx_tuples_user: Optimizes "what can this user access?" queries.
        // Used in ListObjects and reverse lookup operations.
        // Example: SELECT * FROM tuples WHERE store_id=$1 AND user_type=$2 AND user_id=$3
        //
        // idx_tuples_relation: Optimizes queries filtering by relation on a specific object.
        // Critical for Check operations resolving specific relations.
        // Example: SELECT * FROM tuples WHERE store_id=$1 AND object_type=$2 AND object_id=$3 AND relation=$4
        //
        // idx_tuples_store_relation: Optimizes queries that filter by relation across all objects.
        // Used for analytics and bulk operations on specific relation types.
        // Example: SELECT * FROM tuples WHERE store_id=$1 AND relation=$2
        //
        // idx_tuples_condition: Partial index for conditional tuples (CEL conditions).
        // Only indexes rows with non-NULL condition_name to save space.
        // Used when evaluating or auditing conditional relationships.
        let indexes = [
            "CREATE INDEX IF NOT EXISTS idx_tuples_store ON tuples(store_id)",
            "CREATE INDEX IF NOT EXISTS idx_tuples_object ON tuples(store_id, object_type, object_id)",
            "CREATE INDEX IF NOT EXISTS idx_tuples_user ON tuples(store_id, user_type, user_id)",
            "CREATE INDEX IF NOT EXISTS idx_tuples_relation ON tuples(store_id, object_type, object_id, relation)",
            "CREATE INDEX IF NOT EXISTS idx_tuples_store_relation ON tuples(store_id, relation)",
            "CREATE INDEX IF NOT EXISTS idx_tuples_condition ON tuples(store_id, condition_name) WHERE condition_name IS NOT NULL",
        ];

        for index_sql in indexes {
            sqlx::query(index_sql)
                .execute(&self.pool)
                .await
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to create index: {e}"),
                })?;
        }

        // Create authorization_models table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS authorization_models (
                id VARCHAR(255) PRIMARY KEY,
                store_id VARCHAR(255) NOT NULL,
                schema_version VARCHAR(50) NOT NULL,
                model_json TEXT NOT NULL,
                created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
                FOREIGN KEY (store_id) REFERENCES stores(id) ON DELETE CASCADE
            )
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
        // Ordering: (store_id, created_at DESC, id DESC) ensures:
        //   1. Store-scoped queries are efficient (leading store_id column)
        //   2. Results are returned newest-first without additional sorting
        //   3. Deterministic ordering when timestamps are identical (id DESC tiebreaker)
        //   4. Efficient OFFSET/LIMIT pagination as rows are pre-sorted
        //
        // Used by: list_authorization_models, get_latest_authorization_model
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_authorization_models_store ON authorization_models(store_id, created_at DESC, id DESC)",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StorageError::QueryError {
            message: format!("Failed to create authorization_models index: {e}"),
        })?;

        debug!("Database migrations completed successfully");
        Ok(())
    }

    /// Returns the connection pool for testing or advanced usage.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

#[async_trait]
impl DataStore for PostgresDataStore {
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
                VALUES ($1, $2, $3, $4)
                "#,
            )
            .bind(&id_owned)
            .bind(&name_owned)
            .bind(now)
            .bind(now)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                if let sqlx::Error::Database(ref db_err) = e {
                    if db_err.constraint() == Some("stores_pkey") {
                        return StorageError::StoreAlreadyExists {
                            store_id: id_owned.clone(),
                        };
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
                    WHERE id = $1
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
                    DELETE FROM stores WHERE id = $1
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

        let id_owned = id.to_string();
        let name_owned = name.to_string();
        let row = self
            .execute_with_timeout("update_store", async {
                sqlx::query(
                    r#"
                    UPDATE stores
                    SET name = $2, updated_at = NOW()
                    WHERE id = $1
                    RETURNING id, name, created_at, updated_at
                    "#,
                )
                .bind(&id_owned)
                .bind(&name_owned)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to update store: {e}"),
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
    async fn list_stores(&self) -> StorageResult<Vec<Store>> {
        let rows = self
            .execute_with_timeout("list_stores", async {
                sqlx::query(
                    r#"
                    SELECT id, name, created_at, updated_at
                    FROM stores
                    ORDER BY created_at DESC
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
                    ORDER BY created_at DESC
                    LIMIT $1 OFFSET $2
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
            validate_condition_context_size(tuple)?;
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
                    SELECT EXISTS(SELECT 1 FROM stores WHERE id = $1)
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

        // Detect database type for SQL syntax compatibility.
        // CockroachDB uses ROWS FROM (UNNEST(...), UNNEST(...)) while
        // PostgreSQL uses UNNEST(..., ...) AS t(...).
        let use_rows_from_syntax = self.is_cockroachdb().await?;

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

            // Batch delete using UNNEST arrays (fixes N+1 query problem)
            if !deletes.is_empty() {
                let object_types: Vec<&str> = deletes.iter().map(|t| t.object_type.as_str()).collect();
                let object_ids: Vec<&str> = deletes.iter().map(|t| t.object_id.as_str()).collect();
                let relations: Vec<&str> = deletes.iter().map(|t| t.relation.as_str()).collect();
                let user_types: Vec<&str> = deletes.iter().map(|t| t.user_type.as_str()).collect();
                let user_ids: Vec<&str> = deletes.iter().map(|t| t.user_id.as_str()).collect();
                let user_relations: Vec<Option<&str>> =
                    deletes.iter().map(|t| t.user_relation.as_deref()).collect();

                // CockroachDB requires ROWS FROM syntax; PostgreSQL uses multi-array UNNEST
                let unnest_clause = build_delete_unnest(use_rows_from_syntax);
                let delete_query = format!(
                    r#"
                    DELETE FROM tuples t
                    USING (
                        SELECT * FROM {unnest_clause}
                    ) AS del
                    WHERE t.store_id = $1
                      AND t.object_type = del.object_type
                      AND t.object_id = del.object_id
                      AND t.relation = del.relation
                      AND t.user_type = del.user_type
                      AND t.user_id = del.user_id
                      AND COALESCE(t.user_relation, '') = COALESCE(del.user_relation, '')
                    "#
                );
                sqlx::query(&delete_query)
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
                    message: format!("Failed to batch delete tuples: {e}"),
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
                let condition_names: Vec<Option<&str>> =
                    writes.iter().map(|t| t.condition_name.as_deref()).collect();
                // Serialize condition_context to JSON strings for JSONB insertion
                let condition_contexts: Vec<Option<serde_json::Value>> = writes
                    .iter()
                    .map(|t| {
                        t.condition_context
                            .as_ref()
                            .map(|ctx| serde_json::json!(ctx))
                    })
                    .collect();

                // Check for condition conflicts with FOR UPDATE to prevent race conditions.
                // OpenFGA returns 409 Conflict when writing a tuple that exists with a different
                // condition. You must delete and re-create to change conditions.
                //
                // FOR UPDATE locks matching rows, preventing concurrent transactions from
                // inserting conflicting tuples until this transaction completes.
                //
                // Performance impact:
                // - Best case (no existing tuples): ~0.1-0.5ms overhead for the JOIN query
                // - Typical case (few matches): ~0.5-2ms with row locking
                // - Post-insert verification only runs when rows_affected < input count
                // - The LIMIT 1 ensures we fail fast on first conflict
                // Trade-off: Correctness over raw throughput (Invariant I1)
                let unnest_clause = build_insert_unnest(use_rows_from_syntax, "x");
                let conflict_check_query = format!(
                    r#"
                    SELECT
                        t.object_type, t.object_id, t.relation,
                        t.user_type, t.user_id, t.user_relation,
                        t.condition_name AS existing_condition,
                        new.condition_name AS new_condition
                    FROM tuples t
                    INNER JOIN (
                        SELECT * FROM {unnest_clause}
                    ) AS new
                    ON t.store_id = $1
                       AND t.object_type = new.object_type
                       AND t.object_id = new.object_id
                       AND t.relation = new.relation
                       AND t.user_type = new.user_type
                       AND t.user_id = new.user_id
                       AND COALESCE(t.user_relation, '') = COALESCE(new.user_relation, '')
                    WHERE (t.condition_name IS DISTINCT FROM new.condition_name)
                       OR (t.condition_context IS DISTINCT FROM new.condition_context)
                    FOR UPDATE OF t
                    LIMIT 1
                    "#
                );
                let conflict_check = sqlx::query(&conflict_check_query)
                .bind(store_id)
                .bind(&object_types)
                .bind(&object_ids)
                .bind(&relations)
                .bind(&user_types)
                .bind(&user_ids)
                .bind(&user_relations)
                .bind(&condition_names)
                .bind(&condition_contexts)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to check for condition conflicts: {e}"),
                })?;

                if let Some(row) = conflict_check {
                    let user = format!(
                        "{}:{}{}",
                        row.get::<String, _>("user_type"),
                        row.get::<String, _>("user_id"),
                        row.get::<Option<String>, _>("user_relation")
                            .map(|r| format!("#{r}"))
                            .unwrap_or_default()
                    );
                    return Err(StorageError::ConditionConflict(Box::new(
                        crate::error::ConditionConflictError {
                            store_id: store_id.to_string(),
                            object_type: row.get("object_type"),
                            object_id: row.get("object_id"),
                            relation: row.get("relation"),
                            user,
                            existing_condition: row.get("existing_condition"),
                            new_condition: row.get("new_condition"),
                        },
                    )));
                }

                // Insert with ON CONFLICT DO NOTHING for truly identical tuples (idempotent).
                // We've already verified no condition conflicts exist above, and FOR UPDATE
                // locks prevent concurrent inserts with different conditions.
                let unnest_clause = build_insert_unnest(use_rows_from_syntax, "t");
                let insert_query = format!(
                    r#"
                    INSERT INTO tuples (store_id, object_type, object_id, relation, user_type, user_id, user_relation, condition_name, condition_context)
                    SELECT $1, object_type, object_id, relation, user_type, user_id, user_relation, condition_name, condition_context
                    FROM {unnest_clause}
                    ON CONFLICT (store_id, object_type, object_id, relation, user_type, user_id, (COALESCE(user_relation, '')))
                    DO NOTHING
                    "#
                );
                let insert_result = sqlx::query(&insert_query)
                .bind(store_id)
                .bind(&object_types)
                .bind(&object_ids)
                .bind(&relations)
                .bind(&user_types)
                .bind(&user_ids)
                .bind(&user_relations)
                .bind(&condition_names)
                .bind(&condition_contexts)
                .execute(&mut *tx)
                .await
                .map_err(|e| StorageError::QueryError {
                    message: format!("Failed to batch write tuples: {e}"),
                })?;

                // Post-insert verification: if fewer rows were inserted than expected,
                // a concurrent transaction may have inserted conflicting tuples.
                // Re-check for conflicts to ensure we didn't miss any due to race.
                let inserted_count = insert_result.rows_affected() as usize;
                if inserted_count < writes.len() {
                    // Some rows weren't inserted - could be duplicates or race condition.
                    // Re-run conflict check to detect any condition conflicts that occurred.
                    let unnest_clause = build_insert_unnest(use_rows_from_syntax, "x");
                    let post_conflict_query = format!(
                        r#"
                        SELECT
                            t.object_type, t.object_id, t.relation,
                            t.user_type, t.user_id, t.user_relation,
                            t.condition_name AS existing_condition,
                            new.condition_name AS new_condition
                        FROM tuples t
                        INNER JOIN (
                            SELECT * FROM {unnest_clause}
                        ) AS new
                        ON t.store_id = $1
                           AND t.object_type = new.object_type
                           AND t.object_id = new.object_id
                           AND t.relation = new.relation
                           AND t.user_type = new.user_type
                           AND t.user_id = new.user_id
                           AND COALESCE(t.user_relation, '') = COALESCE(new.user_relation, '')
                        WHERE (t.condition_name IS DISTINCT FROM new.condition_name)
                           OR (t.condition_context IS DISTINCT FROM new.condition_context)
                        LIMIT 1
                        "#
                    );
                    let post_conflict = sqlx::query(&post_conflict_query)
                    .bind(store_id)
                    .bind(&object_types)
                    .bind(&object_ids)
                    .bind(&relations)
                    .bind(&user_types)
                    .bind(&user_ids)
                    .bind(&user_relations)
                    .bind(&condition_names)
                    .bind(&condition_contexts)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(|e| StorageError::QueryError {
                        message: format!("Failed to verify condition conflicts: {e}"),
                    })?;

                    if let Some(row) = post_conflict {
                        let user = format!(
                            "{}:{}{}",
                            row.get::<String, _>("user_type"),
                            row.get::<String, _>("user_id"),
                            row.get::<Option<String>, _>("user_relation")
                                .map(|r| format!("#{r}"))
                                .unwrap_or_default()
                        );
                        return Err(StorageError::ConditionConflict(Box::new(
                            crate::error::ConditionConflictError {
                                store_id: store_id.to_string(),
                                object_type: row.get("object_type"),
                                object_id: row.get("object_id"),
                                relation: row.get("relation"),
                                user,
                                existing_condition: row.get("existing_condition"),
                                new_condition: row.get("new_condition"),
                            },
                        )));
                    }
                    // If no conflicts found, the skipped rows were true duplicates (same condition).
                }
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
            SELECT EXISTS(SELECT 1 FROM stores WHERE id = $1)
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

        // Parse user filter upfront to validate and extract components
        let user_filter = if let Some(ref user) = filter.user {
            Some(parse_user_filter(user)?)
        } else {
            None
        };

        // Build query with shared filter logic
        let mut builder: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
            "SELECT object_type, object_id, relation, user_type, user_id, user_relation, condition_name, condition_context FROM tuples WHERE store_id = ",
        );
        builder.push_bind(store_id);
        apply_tuple_filters(&mut builder, filter, user_filter);
        builder.push(" ORDER BY created_at DESC");

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

        // Parse rows using shared helper
        rows.into_iter().map(row_to_stored_tuple).collect()
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
                    SELECT EXISTS(SELECT 1 FROM stores WHERE id = $1)
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

        // Parse user filter upfront to validate and extract components
        let user_filter = if let Some(ref user) = filter.user {
            Some(parse_user_filter(user)?)
        } else {
            None
        };

        let page_size = pagination.page_size.unwrap_or(100) as i64;
        let offset = parse_continuation_token(&pagination.continuation_token)?;

        // Build query with shared filter logic
        let mut builder: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
            "SELECT object_type, object_id, relation, user_type, user_id, user_relation, condition_name, condition_context FROM tuples WHERE store_id = ",
        );
        builder.push_bind(store_id);
        apply_tuple_filters(&mut builder, filter, user_filter);
        builder.push(" ORDER BY created_at DESC LIMIT ");
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

        // Parse rows using shared helper
        let items: Vec<StoredTuple> = rows
            .into_iter()
            .map(row_to_stored_tuple)
            .collect::<StorageResult<Vec<_>>>()?;

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

    // Authorization model operations
    //
    // Note on SQL injection safety: All queries use sqlx parameterized bindings ($1, $2, etc.)
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
                    SELECT EXISTS(SELECT 1 FROM stores WHERE id = $1)
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
                VALUES ($1, $2, $3, $4, $5)
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
                    SELECT EXISTS(SELECT 1 FROM stores WHERE id = $1)
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
                    WHERE store_id = $1 AND id = $2
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
                    SELECT EXISTS(SELECT 1 FROM stores WHERE id = $1)
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
                    WHERE store_id = $1
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
                    SELECT EXISTS(SELECT 1 FROM stores WHERE id = $1)
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
                    WHERE store_id = $1
                    ORDER BY created_at DESC, id DESC
                    LIMIT $2 OFFSET $3
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
                    SELECT EXISTS(SELECT 1 FROM stores WHERE id = $1)
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
                    WHERE store_id = $1
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

        // Execute a simple query to verify database connectivity
        // Uses a shorter dedicated timeout since health checks should be fast
        let check_result = tokio::time::timeout(self.health_check_timeout, async {
            sqlx::query("SELECT 1")
                .execute(&self.pool)
                .await
                .map_err(|e| StorageError::HealthCheckFailed {
                    message: format!("database ping failed: {e}"),
                })
        })
        .await;

        let latency = start.elapsed();

        // Determine status for metrics
        let status = match &check_result {
            Ok(Ok(_)) => "success",
            Ok(Err(_)) => "error",
            Err(_) => "timeout",
        };

        // Record health check duration
        metrics::histogram!(
            "rsfga_storage_health_check_duration_seconds",
            "backend" => "postgres",
            "status" => status.to_string()
        )
        .record(latency.as_secs_f64());

        // Handle the result after recording metrics
        match check_result {
            Ok(result) => {
                result?;
            }
            Err(_elapsed) => {
                metrics::counter!(
                    "rsfga_storage_query_timeout_total",
                    "operation" => "health_check",
                    "backend" => "postgres"
                )
                .increment(1);
                return Err(StorageError::QueryTimeout {
                    operation: "health_check".to_string(),
                    timeout: self.health_check_timeout,
                });
            }
        }

        // Get pool statistics
        // Note: pool.size() returns total connections, so active = size - idle
        let total_connections = self.pool.size();
        let idle_connections = self.pool.num_idle() as u32;
        let active_connections = total_connections.saturating_sub(idle_connections);
        let max_connections = self.pool.options().get_max_connections();

        // Record pool connection gauges
        metrics::gauge!(
            "rsfga_storage_pool_connections",
            "backend" => "postgres",
            "state" => "active"
        )
        .set(active_connections as f64);

        metrics::gauge!(
            "rsfga_storage_pool_connections",
            "backend" => "postgres",
            "state" => "idle"
        )
        .set(idle_connections as f64);

        metrics::gauge!(
            "rsfga_storage_pool_connections",
            "backend" => "postgres",
            "state" => "max"
        )
        .set(max_connections as f64);

        let pool_stats = Some(PoolStats {
            active_connections,
            idle_connections,
            max_connections,
        });

        Ok(HealthStatus {
            healthy: true,
            latency,
            pool_stats,
            message: Some("postgresql".to_string()),
        })
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
        assert_eq!(config.query_timeout_secs, DEFAULT_QUERY_TIMEOUT_SECS);
        assert_eq!(config.read_timeout_secs, None);
        assert_eq!(config.write_timeout_secs, None);
        assert_eq!(
            config.health_check_timeout_secs,
            DEFAULT_HEALTH_CHECK_TIMEOUT_SECS
        );
    }

    #[test]
    fn test_postgres_config_with_custom_timeouts() {
        let config = PostgresConfig {
            database_url: "postgres://localhost/test".to_string(),
            query_timeout_secs: 60,
            read_timeout_secs: Some(10),
            write_timeout_secs: Some(120),
            health_check_timeout_secs: 3,
            ..Default::default()
        };

        assert_eq!(config.query_timeout_secs, 60);
        assert_eq!(config.read_timeout_secs, Some(10));
        assert_eq!(config.write_timeout_secs, Some(120));
        assert_eq!(config.health_check_timeout_secs, 3);
    }

    #[test]
    fn test_postgres_config_debug_redacts_url() {
        let config = PostgresConfig {
            database_url: "postgres://user:password@localhost/db".to_string(),
            ..Default::default()
        };

        let debug_str = format!("{config:?}");
        assert!(debug_str.contains("[REDACTED]"));
        assert!(!debug_str.contains("password"));
        // Should include new timeout fields
        assert!(debug_str.contains("read_timeout_secs"));
        assert!(debug_str.contains("write_timeout_secs"));
        assert!(debug_str.contains("health_check_timeout_secs"));
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

    // =========================================================================
    // CockroachDB Detection Tests
    // =========================================================================

    #[test]
    fn test_is_cockroachdb_version_detects_cockroachdb() {
        // Standard CockroachDB version strings
        assert!(is_cockroachdb_version(
            "CockroachDB CCL v23.1.0 (x86_64-pc-linux-gnu, built 2023/04/26 14:41:24, go1.19.6)"
        ));
        assert!(is_cockroachdb_version(
            "CockroachDB CCL v25.4.0-beta.1 (x86_64-unknown-linux-gnu)"
        ));
        assert!(is_cockroachdb_version("CockroachDB v21.2.0"));
    }

    #[test]
    fn test_is_cockroachdb_version_case_insensitive() {
        // Should work regardless of case
        assert!(is_cockroachdb_version("cockroachdb v23.1.0"));
        assert!(is_cockroachdb_version("COCKROACHDB v23.1.0"));
        assert!(is_cockroachdb_version("CockroachDB v23.1.0"));
        assert!(is_cockroachdb_version("cOcKrOaChDb v23.1.0"));
    }

    #[test]
    fn test_is_cockroachdb_version_rejects_postgresql() {
        // Standard PostgreSQL version strings
        assert!(!is_cockroachdb_version(
            "PostgreSQL 15.2 on x86_64-pc-linux-gnu, compiled by gcc (GCC) 11.3.0, 64-bit"
        ));
        assert!(!is_cockroachdb_version(
            "PostgreSQL 14.0 (Debian 14.0-1.pgdg110+1) on x86_64-pc-linux-gnu"
        ));
        assert!(!is_cockroachdb_version("PostgreSQL 16.0"));
    }

    #[test]
    fn test_is_cockroachdb_version_handles_edge_cases() {
        // Empty string
        assert!(!is_cockroachdb_version(""));

        // Random strings
        assert!(!is_cockroachdb_version("MySQL 8.0"));
        assert!(!is_cockroachdb_version("SQLite 3.39.0"));

        // Partial matches that shouldn't match
        assert!(!is_cockroachdb_version("cockroach"));
        assert!(!is_cockroachdb_version("roachdb"));

        // Must contain full "cockroachdb"
        assert!(is_cockroachdb_version("Something CockroachDB something"));
    }

    #[test]
    fn test_is_cockroachdb_version_embedded_in_text() {
        // CockroachDB mentioned in longer version info
        assert!(is_cockroachdb_version(
            "Database: CockroachDB CCL v23.1.0 running on cluster xyz"
        ));
    }

    // =========================================================================
    // UNNEST Helper Function Tests
    // =========================================================================

    #[test]
    fn test_build_delete_unnest_postgres_syntax() {
        let result = build_delete_unnest(false);
        // PostgreSQL uses multi-array UNNEST
        assert!(result.contains("UNNEST($2::text[], $3::text[]"));
        assert!(result.contains("AS d(object_type, object_id"));
        // Should NOT use ROWS FROM
        assert!(!result.contains("ROWS FROM"));
    }

    #[test]
    fn test_build_delete_unnest_cockroachdb_syntax() {
        let result = build_delete_unnest(true);
        // CockroachDB uses ROWS FROM with individual UNNESTs
        assert!(result.contains("ROWS FROM"));
        assert!(result.contains("UNNEST($2::text[])"));
        assert!(result.contains("UNNEST($3::text[])"));
        assert!(result.contains("AS d(object_type, object_id"));
    }

    #[test]
    fn test_build_insert_unnest_postgres_syntax() {
        let result = build_insert_unnest(false, "t");
        // PostgreSQL uses multi-array UNNEST
        assert!(result.contains("UNNEST($2::text[], $3::text[]"));
        assert!(result.contains("AS t(object_type, object_id"));
        assert!(result.contains("condition_name, condition_context)"));
        // Should NOT use ROWS FROM
        assert!(!result.contains("ROWS FROM"));
    }

    #[test]
    fn test_build_insert_unnest_cockroachdb_syntax() {
        let result = build_insert_unnest(true, "t");
        // CockroachDB uses ROWS FROM with individual UNNESTs
        assert!(result.contains("ROWS FROM"));
        assert!(result.contains("UNNEST($2::text[])"));
        assert!(result.contains("UNNEST($9::jsonb[])"));
        assert!(result.contains("AS t(object_type, object_id"));
        assert!(result.contains("condition_name, condition_context)"));
    }

    #[test]
    fn test_build_insert_unnest_custom_alias() {
        // Test that the alias parameter is used correctly
        let result_x = build_insert_unnest(false, "x");
        assert!(result_x.contains("AS x("));

        let result_new = build_insert_unnest(true, "new_alias");
        assert!(result_new.contains("AS new_alias("));
    }
}
