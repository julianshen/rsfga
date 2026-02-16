# Precomputed Check (v3.0.0) Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Sub-millisecond check operations by precomputing hot-path authorization results in Valkey, with event-driven invalidation from NATS.

**Architecture:** New `rsfga-precompute` binary consumes NATS committed events, classifies changes, identifies affected hot-path checks via a Valkey-stored registry, re-resolves them using the existing graph resolver, and writes results to Valkey. The existing RSFGA API is modified (behind a `precompute` feature flag) to try Valkey before falling back to graph resolution.

**Tech Stack:** Rust, `redis` crate (Valkey/Redis client), `async-nats` (existing), `rsfga-domain` graph resolver (existing), `tokio` async runtime.

**Design Doc:** `docs/plans/2026-02-16-precomputed-check-design.md`

---

## Task 1: Add `redis` workspace dependency and register new crates

**Files:**
- Modify: `Cargo.toml` (workspace root, lines 3-11 for members, line 25+ for deps)

**Step 1: Add `redis` to workspace dependencies and register new crates**

Add to `[workspace.dependencies]` section in `/Users/julianshen/prj/rsfga/Cargo.toml`:

```toml
# Valkey/Redis client for precomputed check cache
redis = { version = "0.27", features = ["tokio-comp", "connection-manager"] }
```

Add new crate members to `[workspace]` members list:

```toml
members = [
    "crates/rsfga-api",
    "crates/rsfga-server",
    "crates/rsfga-domain",
    "crates/rsfga-storage",
    "crates/rsfga-nats",
    "crates/rsfga-writer",
    "crates/rsfga-edge",
    "crates/rsfga-precompute",
    "crates/rsfga-valkey",
    "crates/compatibility-tests",
]
```

**Step 2: Verify workspace parses**

Run: `cargo check --workspace 2>&1 | head -5`
Expected: Errors about missing crate directories (not parse errors). This is expected — we'll create the crates next.

**Step 3: Commit**

```bash
git add Cargo.toml
git commit -m "[STRUCTURAL] Register rsfga-valkey and rsfga-precompute crates, add redis dependency"
```

---

## Task 2: Scaffold `rsfga-valkey` crate — Cargo.toml and module stubs

**Files:**
- Create: `crates/rsfga-valkey/Cargo.toml`
- Create: `crates/rsfga-valkey/src/lib.rs`
- Create: `crates/rsfga-valkey/src/client.rs`
- Create: `crates/rsfga-valkey/src/keys.rs`
- Create: `crates/rsfga-valkey/src/cache.rs`
- Create: `crates/rsfga-valkey/src/error.rs`

**Step 1: Create Cargo.toml**

Create `/Users/julianshen/prj/rsfga/crates/rsfga-valkey/Cargo.toml`:

```toml
[package]
name = "rsfga-valkey"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
authors.workspace = true
license.workspace = true
description = "Valkey/Redis client for RSFGA precomputed check cache"

[dependencies]
redis = { workspace = true }
tokio = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }
chrono = { workspace = true }

[dev-dependencies]
tokio = { workspace = true, features = ["test-util", "macros", "rt-multi-thread"] }
```

**Step 2: Create module stubs**

Create `crates/rsfga-valkey/src/lib.rs`:

```rust
//! Valkey/Redis client for RSFGA precomputed check cache.
//!
//! Shared between the RSFGA API server (reads precomputed results, records hot-path)
//! and the rsfga-precompute worker (writes precomputed results, reads hot-path).

pub mod cache;
pub mod client;
pub mod error;
pub mod keys;

pub use client::ValkeyClient;
pub use error::{ValkeyError, Result};
pub use keys::CheckKey;
```

Create `crates/rsfga-valkey/src/error.rs`:

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ValkeyError {
    #[error("Valkey connection error: {0}")]
    Connection(#[from] redis::RedisError),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Valkey unavailable")]
    Unavailable,
}

pub type Result<T> = std::result::Result<T, ValkeyError>;
```

Create `crates/rsfga-valkey/src/client.rs`:

```rust
//! Valkey connection client with pooling.

use redis::aio::ConnectionManager;
use tracing::info;

use crate::error::{Result, ValkeyError};

/// Configuration for the Valkey client.
#[derive(Debug, Clone)]
pub struct ValkeyConfig {
    /// Redis URL (e.g., "redis://localhost:6379")
    pub url: String,
    /// TTL for precomputed check results (seconds)
    pub result_ttl_secs: u64,
    /// TTL for hot-path registry entries (seconds)
    pub hotpath_ttl_secs: u64,
}

impl Default for ValkeyConfig {
    fn default() -> Self {
        Self {
            url: "redis://localhost:6379".to_string(),
            result_ttl_secs: 300,
            hotpath_ttl_secs: 3600,
        }
    }
}

/// Valkey client with connection pooling via ConnectionManager.
///
/// ConnectionManager automatically reconnects on failure, making it
/// suitable for long-lived server processes.
#[derive(Clone)]
pub struct ValkeyClient {
    conn: ConnectionManager,
    config: ValkeyConfig,
}

impl ValkeyClient {
    /// Connect to Valkey with the given configuration.
    pub async fn connect(config: ValkeyConfig) -> Result<Self> {
        let client = redis::Client::open(config.url.as_str())
            .map_err(ValkeyError::Connection)?;
        let conn = ConnectionManager::new(client)
            .await
            .map_err(ValkeyError::Connection)?;

        info!(url = %redact_url(&config.url), "Valkey connection established");

        Ok(Self { conn, config })
    }

    /// Get a clone of the connection manager (cheap, uses Arc internally).
    pub fn connection(&self) -> ConnectionManager {
        self.conn.clone()
    }

    /// Get the configured result TTL in seconds.
    pub fn result_ttl_secs(&self) -> u64 {
        self.config.result_ttl_secs
    }

    /// Get the configured hot-path TTL in seconds.
    pub fn hotpath_ttl_secs(&self) -> u64 {
        self.config.hotpath_ttl_secs
    }
}

/// Redact credentials from URL for safe logging.
fn redact_url(url_str: &str) -> String {
    match url::Url::parse(url_str) {
        Ok(mut url) => {
            if !url.password().unwrap_or_default().is_empty() {
                let _ = url.set_password(Some("[REDACTED]"));
            }
            url.to_string()
        }
        Err(_) => "[invalid URL]".to_string(),
    }
}
```

Create `crates/rsfga-valkey/src/keys.rs`:

```rust
//! Key format conventions for Valkey storage.
//!
//! All keys follow a namespaced format to avoid collisions.

/// A check cache key in Valkey.
///
/// Format: `check:{store_id}:{model_id}:{object_type}:{object_id}#{relation}@{user}`
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CheckKey {
    pub store_id: String,
    pub model_id: String,
    pub object_type: String,
    pub object_id: String,
    pub relation: String,
    pub user: String,
}

impl CheckKey {
    /// Create a new check key.
    pub fn new(
        store_id: impl Into<String>,
        model_id: impl Into<String>,
        object_type: impl Into<String>,
        object_id: impl Into<String>,
        relation: impl Into<String>,
        user: impl Into<String>,
    ) -> Self {
        Self {
            store_id: store_id.into(),
            model_id: model_id.into(),
            object_type: object_type.into(),
            object_id: object_id.into(),
            relation: relation.into(),
            user: user.into(),
        }
    }

    /// Serialize to the Valkey key string.
    pub fn to_redis_key(&self) -> String {
        format!(
            "check:{}:{}:{}:{}#{}@{}",
            self.store_id,
            self.model_id,
            self.object_type,
            self.object_id,
            self.relation,
            self.user,
        )
    }

    /// Parse from a Valkey key string. Returns None if format is invalid.
    pub fn from_redis_key(key: &str) -> Option<Self> {
        let rest = key.strip_prefix("check:")?;
        let mut parts = rest.splitn(4, ':');
        let store_id = parts.next()?;
        let model_id = parts.next()?;
        let object_type = parts.next()?;
        let remainder = parts.next()?; // "object_id#relation@user"

        let hash_pos = remainder.find('#')?;
        let object_id = &remainder[..hash_pos];
        let after_hash = &remainder[hash_pos + 1..];

        let at_pos = after_hash.find('@')?;
        let relation = &after_hash[..at_pos];
        let user = &after_hash[at_pos + 1..];

        Some(Self {
            store_id: store_id.to_string(),
            model_id: model_id.to_string(),
            object_type: object_type.to_string(),
            object_id: object_id.to_string(),
            relation: relation.to_string(),
            user: user.to_string(),
        })
    }
}

/// Hot-path registry key.
///
/// Format: `hotpath:{store_id}`
/// Members are: `{object_type}:{object_id}#{relation}@{user}`
/// Scores are: Unix timestamp (seconds) of last check request
pub fn hotpath_key(store_id: &str) -> String {
    format!("hotpath:{store_id}")
}

/// Build a hot-path member string from check components.
pub fn hotpath_member(
    object_type: &str,
    object_id: &str,
    relation: &str,
    user: &str,
) -> String {
    format!("{object_type}:{object_id}#{relation}@{user}")
}

/// Build a pattern for ZSCAN matching on (object_type, relation).
/// Uses Redis glob-style patterns: `{object_type}:*#{relation}@*`
pub fn hotpath_pattern(object_type: &str, relation: &str) -> String {
    format!("{object_type}:*#{relation}@*")
}

/// Key prefix for invalidating all check results for a store.
/// Used with SCAN to find all keys matching `check:{store_id}:*`
pub fn store_check_prefix(store_id: &str) -> String {
    format!("check:{store_id}:")
}
```

Create `crates/rsfga-valkey/src/cache.rs`:

```rust
//! Precomputed check result cache operations.

use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::client::ValkeyClient;
use crate::error::Result;
use crate::keys::{self, CheckKey};

/// A precomputed check result stored in Valkey.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrecomputedResult {
    /// Whether the check is allowed.
    pub allowed: bool,
    /// Timestamp when this result was computed.
    pub computed_at: chrono::DateTime<chrono::Utc>,
}

/// Read and write precomputed check results.
#[derive(Clone)]
pub struct CheckCache {
    client: ValkeyClient,
}

impl CheckCache {
    pub fn new(client: ValkeyClient) -> Self {
        Self { client }
    }

    /// Look up a precomputed check result.
    /// Returns None on cache miss or Valkey error (graceful degradation).
    pub async fn get(&self, key: &CheckKey) -> Option<PrecomputedResult> {
        let redis_key = key.to_redis_key();
        let mut conn = self.client.connection();

        match conn.get::<_, Option<String>>(&redis_key).await {
            Ok(Some(json)) => match serde_json::from_str(&json) {
                Ok(result) => {
                    debug!(key = %redis_key, "Precomputed cache hit");
                    Some(result)
                }
                Err(e) => {
                    warn!(key = %redis_key, error = %e, "Failed to deserialize cached result");
                    None
                }
            },
            Ok(None) => None,
            Err(e) => {
                warn!(key = %redis_key, error = %e, "Valkey GET failed, falling back");
                None
            }
        }
    }

    /// Store a precomputed check result with TTL.
    pub async fn set(&self, key: &CheckKey, result: &PrecomputedResult) -> Result<()> {
        let redis_key = key.to_redis_key();
        let json = serde_json::to_string(result)?;
        let ttl = self.client.result_ttl_secs();
        let mut conn = self.client.connection();

        conn.set_ex::<_, _, ()>(&redis_key, &json, ttl).await?;

        debug!(key = %redis_key, ttl_secs = ttl, "Stored precomputed result");
        Ok(())
    }

    /// Record a check request in the hot-path registry.
    /// Uses ZADD with GT flag — only updates score if new score > existing.
    pub async fn record_hotpath(
        &self,
        store_id: &str,
        object_type: &str,
        object_id: &str,
        relation: &str,
        user: &str,
    ) -> Result<()> {
        let key = keys::hotpath_key(store_id);
        let member = keys::hotpath_member(object_type, object_id, relation, user);
        let score = chrono::Utc::now().timestamp() as f64;
        let mut conn = self.client.connection();

        // ZADD with GT: only update score if new > current (or member doesn't exist)
        redis::cmd("ZADD")
            .arg(&key)
            .arg("GT")
            .arg(score)
            .arg(&member)
            .exec_async(&mut conn)
            .await?;

        Ok(())
    }

    /// Scan hot-path entries matching a pattern for a given store.
    /// Returns (object_type, object_id, relation, user) tuples.
    pub async fn scan_hotpath(
        &self,
        store_id: &str,
        pattern: &str,
    ) -> Result<Vec<String>> {
        let key = keys::hotpath_key(store_id);
        let mut conn = self.client.connection();
        let mut results = Vec::new();
        let mut cursor: u64 = 0;

        loop {
            let (new_cursor, members): (u64, Vec<String>) = redis::cmd("ZSCAN")
                .arg(&key)
                .arg(cursor)
                .arg("MATCH")
                .arg(pattern)
                .arg("COUNT")
                .arg(100)
                .query_async(&mut conn)
                .await?;

            // ZSCAN returns alternating member, score pairs
            for (i, item) in members.iter().enumerate() {
                if i % 2 == 0 {
                    results.push(item.clone());
                }
            }

            cursor = new_cursor;
            if cursor == 0 {
                break;
            }
        }

        Ok(results)
    }

    /// Get all hot-path entries for a store (for full recomputation on model change).
    pub async fn get_all_hotpath(&self, store_id: &str) -> Result<Vec<String>> {
        let key = keys::hotpath_key(store_id);
        let mut conn = self.client.connection();

        let members: Vec<String> = conn.zrange(&key, 0, -1).await?;
        Ok(members)
    }

    /// Remove expired hot-path entries older than the configured TTL.
    pub async fn evict_stale_hotpath(&self, store_id: &str) -> Result<u64> {
        let key = keys::hotpath_key(store_id);
        let cutoff = chrono::Utc::now().timestamp() as f64
            - self.client.hotpath_ttl_secs() as f64;
        let mut conn = self.client.connection();

        let removed: u64 = conn
            .zrembyscore(&key, f64::NEG_INFINITY, cutoff)
            .await?;

        if removed > 0 {
            debug!(store_id, removed, "Evicted stale hot-path entries");
        }

        Ok(removed)
    }

    /// Delete all precomputed results for a store (used on model change).
    /// Uses SCAN + DEL to avoid blocking with KEYS command.
    pub async fn invalidate_store(&self, store_id: &str) -> Result<u64> {
        let prefix = keys::store_check_prefix(store_id);
        let mut conn = self.client.connection();
        let mut deleted: u64 = 0;
        let mut cursor: u64 = 0;

        loop {
            let (new_cursor, batch_keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(format!("{prefix}*"))
                .arg("COUNT")
                .arg(100)
                .query_async(&mut conn)
                .await?;

            if !batch_keys.is_empty() {
                let count: u64 = redis::cmd("DEL")
                    .arg(&batch_keys)
                    .query_async(&mut conn)
                    .await?;
                deleted += count;
            }

            cursor = new_cursor;
            if cursor == 0 {
                break;
            }
        }

        debug!(store_id, deleted, "Invalidated store check results");
        Ok(deleted)
    }
}
```

**Step 3: Verify compilation**

Run: `cargo check -p rsfga-valkey`
Expected: Compiles without errors.

**Step 4: Commit**

```bash
git add crates/rsfga-valkey/
git commit -m "[BEHAVIORAL] Scaffold rsfga-valkey crate with client, keys, cache, and error modules"
```

---

## Task 3: Unit tests for `rsfga-valkey` key formatting

**Files:**
- Modify: `crates/rsfga-valkey/src/keys.rs` (add tests module)

**Step 1: Write failing tests**

Add to the bottom of `crates/rsfga-valkey/src/keys.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_key_roundtrip() {
        let key = CheckKey::new("store1", "model1", "document", "readme", "viewer", "user:alice");
        let redis_key = key.to_redis_key();
        assert_eq!(redis_key, "check:store1:model1:document:readme#viewer@user:alice");

        let parsed = CheckKey::from_redis_key(&redis_key).unwrap();
        assert_eq!(parsed, key);
    }

    #[test]
    fn test_check_key_with_special_characters() {
        let key = CheckKey::new("store1", "model1", "document", "doc-with-dashes", "can_view", "user:bob");
        let redis_key = key.to_redis_key();
        let parsed = CheckKey::from_redis_key(&redis_key).unwrap();
        assert_eq!(parsed, key);
    }

    #[test]
    fn test_check_key_from_invalid_format() {
        assert!(CheckKey::from_redis_key("invalid").is_none());
        assert!(CheckKey::from_redis_key("check:").is_none());
        assert!(CheckKey::from_redis_key("check:s:m:t:obj").is_none()); // missing #
        assert!(CheckKey::from_redis_key("wrong_prefix:s:m:t:o#r@u").is_none());
    }

    #[test]
    fn test_hotpath_key_format() {
        assert_eq!(hotpath_key("store123"), "hotpath:store123");
    }

    #[test]
    fn test_hotpath_member_format() {
        let member = hotpath_member("document", "readme", "viewer", "user:alice");
        assert_eq!(member, "document:readme#viewer@user:alice");
    }

    #[test]
    fn test_hotpath_pattern_format() {
        let pattern = hotpath_pattern("document", "viewer");
        assert_eq!(pattern, "document:*#viewer@*");
    }

    #[test]
    fn test_store_check_prefix() {
        assert_eq!(store_check_prefix("store1"), "check:store1:");
    }
}
```

**Step 2: Run tests**

Run: `cargo test -p rsfga-valkey -- --nocapture`
Expected: All 7 tests pass.

**Step 3: Commit**

```bash
git add crates/rsfga-valkey/src/keys.rs
git commit -m "[TEST] Add unit tests for Valkey key formatting and parsing"
```

---

## Task 4: Scaffold `rsfga-precompute` crate — Cargo.toml and module stubs

**Files:**
- Create: `crates/rsfga-precompute/Cargo.toml`
- Create: `crates/rsfga-precompute/src/lib.rs`
- Create: `crates/rsfga-precompute/src/main.rs`
- Create: `crates/rsfga-precompute/src/config.rs`
- Create: `crates/rsfga-precompute/src/classifier.rs`
- Create: `crates/rsfga-precompute/src/impact.rs`
- Create: `crates/rsfga-precompute/src/worker.rs`
- Create: `crates/rsfga-precompute/src/metrics.rs`
- Create: `crates/rsfga-precompute/src/error.rs`

**Step 1: Create Cargo.toml**

Create `/Users/julianshen/prj/rsfga/crates/rsfga-precompute/Cargo.toml`:

```toml
[package]
name = "rsfga-precompute"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
authors.workspace = true
license.workspace = true
description = "Precomputation worker daemon for RSFGA sub-millisecond check operations"

[lib]
name = "rsfga_precompute"
path = "src/lib.rs"

[[bin]]
name = "rsfga-precompute"
path = "src/main.rs"

[dependencies]
# Async runtime
tokio = { workspace = true }
futures = { workspace = true }
async-nats = { workspace = true }

# Internal crates
rsfga-nats = { path = "../rsfga-nats" }
rsfga-storage = { path = "../rsfga-storage" }
rsfga-domain = { path = "../rsfga-domain" }
rsfga-valkey = { path = "../rsfga-valkey" }

# Serialization
serde = { workspace = true }
serde_json = { workspace = true }

# Error handling
thiserror = { workspace = true }
anyhow = { workspace = true }

# Tracing and observability
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
metrics = { workspace = true }
metrics-exporter-prometheus = { workspace = true }

# CLI
clap = { workspace = true }

# Utilities
url = { workspace = true }
chrono = { workspace = true }
ulid = { workspace = true }
dashmap = { workspace = true }

[dev-dependencies]
tokio = { workspace = true, features = ["test-util", "macros", "rt-multi-thread"] }
tempfile = { workspace = true }
```

**Step 2: Create module stubs**

Create `crates/rsfga-precompute/src/lib.rs`:

```rust
//! RSFGA Precomputation Worker
//!
//! Consumes committed events from NATS JetStream, classifies changes,
//! identifies affected hot-path checks, and precomputes results into Valkey.

pub mod classifier;
pub mod config;
pub mod error;
pub mod impact;
pub mod metrics;
pub mod worker;
```

Create `crates/rsfga-precompute/src/error.rs`:

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PrecomputeError {
    #[error("NATS error: {0}")]
    Nats(#[from] rsfga_nats::NatsError),

    #[error("Valkey error: {0}")]
    Valkey(#[from] rsfga_valkey::ValkeyError),

    #[error("Storage error: {0}")]
    Storage(#[from] rsfga_storage::StorageError),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("{0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, PrecomputeError>;
```

Create `crates/rsfga-precompute/src/config.rs`:

```rust
use std::time::Duration;

/// Configuration for the precompute worker.
#[derive(Debug, Clone)]
pub struct PrecomputeConfig {
    /// NATS consumer name (should be unique per instance).
    pub consumer_name: String,
    /// Number of async workers for check resolution.
    pub workers: usize,
    /// Debounce window: batch changes to the same (object_type, relation)
    /// within this duration before scheduling recomputation.
    pub batch_window: Duration,
    /// Maximum depth of the recomputation job queue.
    /// Jobs beyond this limit are dropped (oldest hot-path entries first).
    pub max_queue_depth: usize,
    /// Fetch timeout for NATS pull consumer.
    pub fetch_timeout: Duration,
}

impl Default for PrecomputeConfig {
    fn default() -> Self {
        Self {
            consumer_name: "rsfga-precompute-1".to_string(),
            workers: 4,
            batch_window: Duration::from_millis(50),
            max_queue_depth: 10_000,
            fetch_timeout: Duration::from_millis(500),
        }
    }
}
```

Create `crates/rsfga-precompute/src/classifier.rs`:

```rust
//! Change classification for NATS committed events.

use rsfga_nats::CommittedEvent;

/// Classified change from a committed event.
#[derive(Debug, Clone)]
pub enum ChangeType {
    /// A tuple was written or deleted. Contains (store_id, object_type, relation).
    TupleChange {
        store_id: String,
        object_type: String,
        relation: String,
    },
    /// An authorization model changed. Requires full store invalidation.
    ModelChange {
        store_id: String,
    },
}

/// Classify a committed event into change types for impact analysis.
pub fn classify(event: &CommittedEvent) -> Vec<ChangeType> {
    let mut changes = Vec::new();

    // Extract unique (object_type, relation) pairs from writes
    for write in &event.writes {
        if let Some((obj_type, _)) = write.object.split_once(':') {
            changes.push(ChangeType::TupleChange {
                store_id: event.store_id.clone(),
                object_type: obj_type.to_string(),
                relation: write.relation.clone(),
            });
        }
    }

    // Extract unique (object_type, relation) pairs from deletes
    for delete in &event.deletes {
        if let Some((obj_type, _)) = delete.object.split_once(':') {
            changes.push(ChangeType::TupleChange {
                store_id: event.store_id.clone(),
                object_type: obj_type.to_string(),
                relation: delete.relation.clone(),
            });
        }
    }

    // Deduplicate by (store_id, object_type, relation)
    changes.dedup_by(|a, b| match (a, b) {
        (
            ChangeType::TupleChange {
                store_id: s1,
                object_type: t1,
                relation: r1,
            },
            ChangeType::TupleChange {
                store_id: s2,
                object_type: t2,
                relation: r2,
            },
        ) => s1 == s2 && t1 == t2 && r1 == r2,
        _ => false,
    });

    changes
}
```

Create `crates/rsfga-precompute/src/impact.rs`:

```rust
//! Impact analysis: determine which hot-path checks need recomputation.

// Will be implemented in Task 8
```

Create `crates/rsfga-precompute/src/worker.rs`:

```rust
//! Precomputation worker pool.

// Will be implemented in Task 10
```

Create `crates/rsfga-precompute/src/metrics.rs`:

```rust
//! Prometheus metrics for the precompute worker.

use metrics::{counter, gauge, histogram};

/// Record a received event.
pub fn record_event_received(event_type: &str) {
    counter!("precompute_events_received_total", "event_type" => event_type.to_string())
        .increment(1);
}

/// Record a queued job.
pub fn record_job_queued() {
    counter!("precompute_jobs_queued_total").increment(1);
}

/// Record a completed job.
pub fn record_job_completed(result: &str) {
    counter!("precompute_jobs_completed_total", "result" => result.to_string()).increment(1);
}

/// Record check resolution duration.
pub fn record_resolution_duration(duration_secs: f64) {
    histogram!("precompute_resolution_duration_seconds").record(duration_secs);
}

/// Record Valkey write duration.
pub fn record_valkey_write_duration(duration_secs: f64) {
    histogram!("precompute_valkey_write_duration_seconds").record(duration_secs);
}

/// Update the queue depth gauge.
pub fn update_queue_depth(depth: f64) {
    gauge!("precompute_queue_depth").set(depth);
}
```

Create `crates/rsfga-precompute/src/main.rs`:

```rust
//! RSFGA Precompute Worker Binary
//!
//! Consumes committed events from NATS, identifies affected hot-path checks,
//! resolves them, and stores precomputed results in Valkey.

fn main() {
    // Placeholder — will be implemented in Task 12
    println!("rsfga-precompute: not yet implemented");
}
```

**Step 3: Verify compilation**

Run: `cargo check -p rsfga-precompute`
Expected: Compiles without errors.

**Step 4: Commit**

```bash
git add crates/rsfga-precompute/
git commit -m "[BEHAVIORAL] Scaffold rsfga-precompute crate with classifier, config, metrics, and error modules"
```

---

## Task 5: Change classifier — tests and implementation

**Files:**
- Modify: `crates/rsfga-precompute/src/classifier.rs`

**Step 1: Write failing tests**

Add to the bottom of `crates/rsfga-precompute/src/classifier.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rsfga_nats::{CommittedEvent, TupleKey, TupleOperation};

    fn make_event(writes: Vec<TupleOperation>, deletes: Vec<TupleKey>) -> CommittedEvent {
        CommittedEvent::new("store1", 1)
            .with_writes(writes)
            .with_deletes(deletes)
    }

    fn tuple_op(user: &str, relation: &str, object: &str) -> TupleOperation {
        TupleOperation {
            user: user.to_string(),
            relation: relation.to_string(),
            object: object.to_string(),
            condition: None,
        }
    }

    fn tuple_key(user: &str, relation: &str, object: &str) -> TupleKey {
        TupleKey {
            user: user.to_string(),
            relation: relation.to_string(),
            object: object.to_string(),
        }
    }

    #[test]
    fn test_classify_single_tuple_write() {
        let event = make_event(
            vec![tuple_op("user:alice", "viewer", "document:readme")],
            vec![],
        );
        let changes = classify(&event);
        assert_eq!(changes.len(), 1);
        match &changes[0] {
            ChangeType::TupleChange { store_id, object_type, relation } => {
                assert_eq!(store_id, "store1");
                assert_eq!(object_type, "document");
                assert_eq!(relation, "viewer");
            }
            _ => panic!("Expected TupleChange"),
        }
    }

    #[test]
    fn test_classify_tuple_delete() {
        let event = make_event(
            vec![],
            vec![tuple_key("user:alice", "viewer", "document:readme")],
        );
        let changes = classify(&event);
        assert_eq!(changes.len(), 1);
        match &changes[0] {
            ChangeType::TupleChange { object_type, relation, .. } => {
                assert_eq!(object_type, "document");
                assert_eq!(relation, "viewer");
            }
            _ => panic!("Expected TupleChange"),
        }
    }

    #[test]
    fn test_classify_deduplicates_same_type_relation() {
        let event = make_event(
            vec![
                tuple_op("user:alice", "viewer", "document:doc1"),
                tuple_op("user:bob", "viewer", "document:doc2"),
            ],
            vec![],
        );
        let changes = classify(&event);
        // Both are (document, viewer) — should deduplicate to 1
        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn test_classify_different_relations_not_deduped() {
        let event = make_event(
            vec![
                tuple_op("user:alice", "viewer", "document:doc1"),
                tuple_op("user:alice", "editor", "document:doc1"),
            ],
            vec![],
        );
        let changes = classify(&event);
        assert_eq!(changes.len(), 2);
    }

    #[test]
    fn test_classify_empty_event() {
        let event = make_event(vec![], vec![]);
        let changes = classify(&event);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_classify_invalid_object_format_skipped() {
        // Object without ':' separator should be skipped
        let event = CommittedEvent::new("store1", 1).with_writes(vec![TupleOperation {
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "invalidobject".to_string(), // No type:id separator
            condition: None,
        }]);
        let changes = classify(&event);
        assert!(changes.is_empty());
    }
}
```

**Step 2: Run tests**

Run: `cargo test -p rsfga-precompute -- --nocapture`
Expected: All 6 tests pass.

**Step 3: Commit**

```bash
git add crates/rsfga-precompute/src/classifier.rs
git commit -m "[TEST] Add classifier tests for change classification from committed events"
```

---

## Task 6: Valkey integration tests with testcontainers (or mock)

**Files:**
- Create: `crates/rsfga-valkey/tests/integration_test.rs`

**Note:** These tests require a running Redis/Valkey instance. Use `#[ignore]` for CI environments without Redis. Alternatively, test with a real Redis if available, or use the `redis-test` crate.

**Step 1: Create integration test**

Create `crates/rsfga-valkey/tests/integration_test.rs`:

```rust
//! Integration tests for rsfga-valkey.
//!
//! These tests require a running Redis/Valkey instance at localhost:6379.
//! Run with: cargo test -p rsfga-valkey --test integration_test -- --ignored
//!
//! To start a local Redis for testing:
//!   docker run --rm -p 6379:6379 redis:7-alpine

use rsfga_valkey::{
    cache::{CheckCache, PrecomputedResult},
    client::{ValkeyClient, ValkeyConfig},
    keys::CheckKey,
};

async fn setup() -> CheckCache {
    let config = ValkeyConfig {
        url: "redis://localhost:6379".to_string(),
        result_ttl_secs: 10,
        hotpath_ttl_secs: 60,
    };
    let client = ValkeyClient::connect(config)
        .await
        .expect("Failed to connect to Redis — is it running on localhost:6379?");
    CheckCache::new(client)
}

#[tokio::test]
#[ignore = "requires running Redis on localhost:6379"]
async fn test_set_and_get_precomputed_result() {
    let cache = setup().await;
    let key = CheckKey::new("test-store", "model1", "document", "readme", "viewer", "user:alice");

    let result = PrecomputedResult {
        allowed: true,
        computed_at: chrono::Utc::now(),
    };

    cache.set(&key, &result).await.unwrap();

    let cached = cache.get(&key).await;
    assert!(cached.is_some());
    assert_eq!(cached.unwrap().allowed, true);
}

#[tokio::test]
#[ignore = "requires running Redis on localhost:6379"]
async fn test_get_returns_none_on_cache_miss() {
    let cache = setup().await;
    let key = CheckKey::new("nonexistent", "model1", "doc", "x", "viewer", "user:nobody");

    let cached = cache.get(&key).await;
    assert!(cached.is_none());
}

#[tokio::test]
#[ignore = "requires running Redis on localhost:6379"]
async fn test_hotpath_record_and_scan() {
    let cache = setup().await;
    let store_id = "test-hotpath-store";

    // Record some hot-path entries
    cache.record_hotpath(store_id, "document", "doc1", "viewer", "user:alice").await.unwrap();
    cache.record_hotpath(store_id, "document", "doc2", "viewer", "user:bob").await.unwrap();
    cache.record_hotpath(store_id, "folder", "f1", "owner", "user:alice").await.unwrap();

    // Scan for document:*#viewer@* pattern
    let matches = cache.scan_hotpath(store_id, "document:*#viewer@*").await.unwrap();
    assert_eq!(matches.len(), 2);

    // Scan for folder:*#owner@* pattern
    let matches = cache.scan_hotpath(store_id, "folder:*#owner@*").await.unwrap();
    assert_eq!(matches.len(), 1);
}

#[tokio::test]
#[ignore = "requires running Redis on localhost:6379"]
async fn test_invalidate_store_deletes_all_check_keys() {
    let cache = setup().await;
    let store_id = "test-invalidate-store";

    let key1 = CheckKey::new(store_id, "m1", "doc", "d1", "viewer", "user:alice");
    let key2 = CheckKey::new(store_id, "m1", "doc", "d2", "editor", "user:bob");

    let result = PrecomputedResult {
        allowed: true,
        computed_at: chrono::Utc::now(),
    };

    cache.set(&key1, &result).await.unwrap();
    cache.set(&key2, &result).await.unwrap();

    // Both should be cached
    assert!(cache.get(&key1).await.is_some());
    assert!(cache.get(&key2).await.is_some());

    // Invalidate the store
    let deleted = cache.invalidate_store(store_id).await.unwrap();
    assert!(deleted >= 2);

    // Both should now be gone
    assert!(cache.get(&key1).await.is_none());
    assert!(cache.get(&key2).await.is_none());
}

#[tokio::test]
#[ignore = "requires running Redis on localhost:6379"]
async fn test_get_all_hotpath() {
    let cache = setup().await;
    let store_id = "test-all-hotpath";

    cache.record_hotpath(store_id, "doc", "d1", "viewer", "user:a").await.unwrap();
    cache.record_hotpath(store_id, "doc", "d2", "editor", "user:b").await.unwrap();

    let all = cache.get_all_hotpath(store_id).await.unwrap();
    assert_eq!(all.len(), 2);
}
```

**Step 2: Run tests (if Redis is available)**

Run: `cargo test -p rsfga-valkey --test integration_test -- --ignored --nocapture`
Expected: All 5 tests pass (requires Redis running).

**Step 3: Commit**

```bash
git add crates/rsfga-valkey/tests/
git commit -m "[TEST] Add Valkey integration tests for cache and hot-path registry"
```

---

## Task 7: Model dependency graph for impact analysis

**Files:**
- Modify: `crates/rsfga-precompute/src/impact.rs`

**Context:** The impact analyzer needs to understand model dependencies. For example, if `viewer` is defined as `[user] or editor`, then a change to an `editor` tuple also affects `viewer` checks. This requires reading the authorization model and building a dependency graph.

**Step 1: Write tests and implementation**

Replace `crates/rsfga-precompute/src/impact.rs`:

```rust
//! Impact analysis: determine which hot-path checks need recomputation.
//!
//! Given a committed event and the hot-path registry, this module identifies
//! which specific check results need to be recomputed.

use std::collections::{HashMap, HashSet};

use rsfga_valkey::cache::CheckCache;
use rsfga_valkey::keys;
use tracing::{debug, warn};

use crate::classifier::ChangeType;
use crate::error::Result;

/// A job to recompute a specific check result.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct RecomputeJob {
    pub store_id: String,
    pub object_type: String,
    pub object_id: String,
    pub relation: String,
    pub user: String,
}

/// Build a map of relation dependencies from an authorization model's type definitions.
///
/// For each (type, relation), returns the set of other relations that could be affected
/// when tuples with (type, relation) change.
///
/// Example: If `viewer` is defined as `[user] or editor`, then `editor` affects `viewer`.
/// So `dependent_relations[("document", "editor")]` includes `("document", "viewer")`.
pub fn build_relation_dependencies(
    type_definitions: &HashMap<String, HashMap<String, Vec<String>>>,
) -> HashMap<(String, String), HashSet<(String, String)>> {
    let mut deps: HashMap<(String, String), HashSet<(String, String)>> = HashMap::new();

    for (type_name, relations) in type_definitions {
        for (relation_name, referenced_relations) in relations {
            for ref_rel in referenced_relations {
                // If relation_name references ref_rel, then changes to ref_rel
                // affect relation_name's check results
                deps.entry((type_name.clone(), ref_rel.clone()))
                    .or_default()
                    .insert((type_name.clone(), relation_name.clone()));
            }
        }
    }

    deps
}

/// Determine which hot-path checks need recomputation based on classified changes.
///
/// For each `TupleChange`, queries the hot-path registry for matching entries
/// and any entries affected through computed relation dependencies.
///
/// For `ModelChange`, returns ALL hot-path entries for the store.
pub async fn find_affected_checks(
    changes: &[ChangeType],
    cache: &CheckCache,
    relation_deps: &HashMap<(String, String), HashSet<(String, String)>>,
) -> Result<Vec<RecomputeJob>> {
    let mut jobs = HashSet::new();

    for change in changes {
        match change {
            ChangeType::TupleChange {
                store_id,
                object_type,
                relation,
            } => {
                // Direct: find hot-path entries for this (object_type, relation)
                let pattern = keys::hotpath_pattern(object_type, relation);
                let members = cache.scan_hotpath(store_id, &pattern).await?;
                for member in &members {
                    if let Some(job) = parse_hotpath_member(store_id, member) {
                        jobs.insert(job);
                    }
                }

                // Indirect: find computed relations that depend on this relation
                let key = (object_type.clone(), relation.clone());
                if let Some(dependents) = relation_deps.get(&key) {
                    for (dep_type, dep_rel) in dependents {
                        let dep_pattern = keys::hotpath_pattern(dep_type, dep_rel);
                        let dep_members = cache.scan_hotpath(store_id, &dep_pattern).await?;
                        for member in &dep_members {
                            if let Some(job) = parse_hotpath_member(store_id, member) {
                                jobs.insert(job);
                            }
                        }
                    }
                }

                debug!(
                    store_id,
                    object_type,
                    relation,
                    job_count = jobs.len(),
                    "Found affected checks for tuple change"
                );
            }
            ChangeType::ModelChange { store_id } => {
                // Model change: ALL hot-path entries must be recomputed
                let all_members = cache.get_all_hotpath(store_id).await?;
                for member in &all_members {
                    if let Some(job) = parse_hotpath_member(store_id, member) {
                        jobs.insert(job);
                    }
                }

                debug!(
                    store_id,
                    job_count = jobs.len(),
                    "Found affected checks for model change"
                );
            }
        }
    }

    Ok(jobs.into_iter().collect())
}

/// Parse a hot-path member string into a RecomputeJob.
///
/// Format: `{object_type}:{object_id}#{relation}@{user}`
fn parse_hotpath_member(store_id: &str, member: &str) -> Option<RecomputeJob> {
    let colon_pos = member.find(':')?;
    let object_type = &member[..colon_pos];
    let after_colon = &member[colon_pos + 1..];

    let hash_pos = after_colon.find('#')?;
    let object_id = &after_colon[..hash_pos];
    let after_hash = &after_colon[hash_pos + 1..];

    let at_pos = after_hash.find('@')?;
    let relation = &after_hash[..at_pos];
    let user = &after_hash[at_pos + 1..];

    Some(RecomputeJob {
        store_id: store_id.to_string(),
        object_type: object_type.to_string(),
        object_id: object_id.to_string(),
        relation: relation.to_string(),
        user: user.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hotpath_member_valid() {
        let job = parse_hotpath_member("store1", "document:readme#viewer@user:alice").unwrap();
        assert_eq!(job.store_id, "store1");
        assert_eq!(job.object_type, "document");
        assert_eq!(job.object_id, "readme");
        assert_eq!(job.relation, "viewer");
        assert_eq!(job.user, "user:alice");
    }

    #[test]
    fn test_parse_hotpath_member_invalid() {
        assert!(parse_hotpath_member("s", "invalid").is_none());
        assert!(parse_hotpath_member("s", "no_hash@user").is_none());
        assert!(parse_hotpath_member("s", "type:id#no_at").is_none());
    }

    #[test]
    fn test_build_relation_dependencies() {
        // Model: document type with viewer = [user] or editor
        let mut type_defs: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();
        let mut doc_relations = HashMap::new();
        doc_relations.insert("viewer".to_string(), vec!["editor".to_string()]); // viewer references editor
        doc_relations.insert("editor".to_string(), vec![]); // editor has no references
        type_defs.insert("document".to_string(), doc_relations);

        let deps = build_relation_dependencies(&type_defs);

        // editor changes should affect viewer
        let affected = deps.get(&("document".to_string(), "editor".to_string()));
        assert!(affected.is_some());
        assert!(affected.unwrap().contains(&("document".to_string(), "viewer".to_string())));

        // viewer changes should NOT affect editor (no reverse dep)
        let affected = deps.get(&("document".to_string(), "viewer".to_string()));
        assert!(affected.is_none());
    }

    #[test]
    fn test_build_relation_dependencies_chain() {
        // Model: viewer references editor, editor references owner
        let mut type_defs: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();
        let mut doc_relations = HashMap::new();
        doc_relations.insert("viewer".to_string(), vec!["editor".to_string()]);
        doc_relations.insert("editor".to_string(), vec!["owner".to_string()]);
        doc_relations.insert("owner".to_string(), vec![]);
        type_defs.insert("document".to_string(), doc_relations);

        let deps = build_relation_dependencies(&type_defs);

        // owner changes affect editor
        assert!(deps[&("document".to_string(), "owner".to_string())]
            .contains(&("document".to_string(), "editor".to_string())));

        // editor changes affect viewer
        assert!(deps[&("document".to_string(), "editor".to_string())]
            .contains(&("document".to_string(), "viewer".to_string())));
    }
}
```

**Step 2: Run tests**

Run: `cargo test -p rsfga-precompute -- --nocapture`
Expected: All tests pass (classifier tests + impact tests).

**Step 3: Commit**

```bash
git add crates/rsfga-precompute/src/impact.rs
git commit -m "[BEHAVIORAL] Implement impact analyzer with relation dependency graph and hot-path matching"
```

---

## Task 8: Precomputation worker pool

**Files:**
- Modify: `crates/rsfga-precompute/src/worker.rs`

**Step 1: Implement worker pool**

Replace `crates/rsfga-precompute/src/worker.rs`:

```rust
//! Precomputation worker pool.
//!
//! Receives recomputation jobs, resolves checks using the graph resolver,
//! and writes results to Valkey.

use std::sync::Arc;
use std::time::Instant;

use rsfga_domain::resolver::{CheckRequest, GraphResolver};
use rsfga_storage::DataStore;
use rsfga_valkey::cache::{CheckCache, PrecomputedResult};
use rsfga_valkey::keys::CheckKey;
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

use crate::impact::RecomputeJob;
use crate::metrics;

/// A worker pool that processes recomputation jobs concurrently.
pub struct WorkerPool<S: DataStore> {
    sender: mpsc::Sender<RecomputeJob>,
    _handles: Vec<tokio::task::JoinHandle<()>>,
    _phantom: std::marker::PhantomData<S>,
}

impl<S: DataStore + Send + Sync + 'static> WorkerPool<S> {
    /// Create a new worker pool with the given number of workers.
    ///
    /// # Arguments
    /// - `workers`: Number of concurrent workers
    /// - `max_queue_depth`: Maximum number of pending jobs
    /// - `resolver`: Shared graph resolver for check operations
    /// - `cache`: Shared Valkey cache for writing results
    /// - `model_id_fn`: Function to get the latest model ID for a store
    pub fn new(
        workers: usize,
        max_queue_depth: usize,
        resolver: Arc<GraphResolver<S>>,
        cache: Arc<CheckCache>,
        storage: Arc<S>,
    ) -> Self {
        let (sender, receiver) = mpsc::channel::<RecomputeJob>(max_queue_depth);
        let receiver = Arc::new(tokio::sync::Mutex::new(receiver));

        let mut handles = Vec::with_capacity(workers);

        for worker_id in 0..workers {
            let rx = Arc::clone(&receiver);
            let resolver = Arc::clone(&resolver);
            let cache = Arc::clone(&cache);
            let storage = Arc::clone(&storage);

            let handle = tokio::spawn(async move {
                loop {
                    let job = {
                        let mut guard = rx.lock().await;
                        guard.recv().await
                    };

                    match job {
                        Some(job) => {
                            process_job(worker_id, &job, &resolver, &cache, &storage).await;
                        }
                        None => {
                            debug!(worker_id, "Worker channel closed, shutting down");
                            break;
                        }
                    }
                }
            });

            handles.push(handle);
        }

        Self {
            sender,
            _handles: handles,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Submit a job for precomputation.
    /// Returns false if the queue is full (backpressure).
    pub fn submit(&self, job: RecomputeJob) -> bool {
        match self.sender.try_send(job) {
            Ok(_) => {
                metrics::record_job_queued();
                true
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                warn!("Precompute job queue full, dropping job");
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                error!("Precompute worker pool shut down");
                false
            }
        }
    }

    /// Get the current queue depth.
    pub fn queue_depth(&self) -> usize {
        self.sender.max_capacity() - self.sender.capacity()
    }
}

/// Process a single recomputation job.
async fn process_job<S: DataStore>(
    worker_id: usize,
    job: &RecomputeJob,
    resolver: &GraphResolver<S>,
    cache: &CheckCache,
    storage: &S,
) {
    let start = Instant::now();

    // Get the latest authorization model ID for this store
    let model_id = match storage.get_latest_authorization_model(&job.store_id).await {
        Ok(Some(model)) => model.id,
        Ok(None) => {
            debug!(
                worker_id,
                store_id = %job.store_id,
                "No authorization model found, skipping job"
            );
            metrics::record_job_completed("skipped");
            return;
        }
        Err(e) => {
            warn!(
                worker_id,
                store_id = %job.store_id,
                error = %e,
                "Failed to get latest model, skipping job"
            );
            metrics::record_job_completed("error");
            return;
        }
    };

    // Build check request
    let check_request = CheckRequest::new(
        job.store_id.clone(),
        job.user.clone(),
        job.relation.clone(),
        format!("{}:{}", job.object_type, job.object_id),
    );

    // Resolve the check
    let check_result = match resolver.check(&check_request).await {
        Ok(result) => result,
        Err(e) => {
            warn!(
                worker_id,
                store_id = %job.store_id,
                object = %format!("{}:{}", job.object_type, job.object_id),
                relation = %job.relation,
                user = %job.user,
                error = %e,
                "Check resolution failed"
            );
            metrics::record_job_completed("error");
            return;
        }
    };

    let resolution_duration = start.elapsed();
    metrics::record_resolution_duration(resolution_duration.as_secs_f64());

    // Write result to Valkey
    let valkey_start = Instant::now();
    let key = CheckKey::new(
        &job.store_id,
        &model_id,
        &job.object_type,
        &job.object_id,
        &job.relation,
        &job.user,
    );
    let precomputed = PrecomputedResult {
        allowed: check_result.allowed,
        computed_at: chrono::Utc::now(),
    };

    if let Err(e) = cache.set(&key, &precomputed).await {
        warn!(
            worker_id,
            key = %key.to_redis_key(),
            error = %e,
            "Failed to write precomputed result to Valkey"
        );
        metrics::record_job_completed("error");
        return;
    }

    metrics::record_valkey_write_duration(valkey_start.elapsed().as_secs_f64());
    metrics::record_job_completed("success");

    debug!(
        worker_id,
        key = %key.to_redis_key(),
        allowed = check_result.allowed,
        resolution_ms = resolution_duration.as_millis(),
        "Precomputed check result"
    );
}

#[cfg(test)]
mod tests {
    // Worker pool integration tests will be added in Task 12 (requires storage + resolver setup)
}
```

**Step 2: Verify compilation**

Run: `cargo check -p rsfga-precompute`
Expected: Compiles. Note: `CheckRequest::new` and `GraphResolver::check` signatures need to match existing domain crate API. Adjust field names if compilation fails — check `crates/rsfga-domain/src/resolver/types.rs` and `crates/rsfga-domain/src/resolver/graph_resolver.rs` for exact signatures.

**Step 3: Commit**

```bash
git add crates/rsfga-precompute/src/worker.rs
git commit -m "[BEHAVIORAL] Implement precomputation worker pool with mpsc job queue and backpressure"
```

---

## Task 9: `rsfga-precompute` binary — main loop, config, graceful shutdown

**Files:**
- Modify: `crates/rsfga-precompute/src/main.rs`

**Step 1: Implement binary entry point**

Replace `crates/rsfga-precompute/src/main.rs`:

```rust
//! RSFGA Precompute Worker Binary
//!
//! Consumes committed events from NATS JetStream, classifies changes,
//! identifies affected hot-path checks, and precomputes results into Valkey.
//!
//! # Usage
//!
//! ```bash
//! rsfga-precompute --nats-url nats://localhost:4222 --valkey-url redis://localhost:6379
//! ```

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use futures::StreamExt;
use rsfga_nats::{CommittedEvent, JetStreamManager, NatsClient, NatsConfig};
use rsfga_precompute::classifier;
use rsfga_precompute::config::PrecomputeConfig;
use rsfga_precompute::impact;
use rsfga_precompute::metrics;
use rsfga_precompute::worker::WorkerPool;
use rsfga_storage::{DataStore, MemoryDataStore, PostgresConfig, PostgresDataStore};
use rsfga_valkey::cache::CheckCache;
use rsfga_valkey::client::{ValkeyClient, ValkeyConfig};
use tracing::{error, info, warn};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// RSFGA Precompute Worker — precomputes authorization check results
#[derive(Parser, Debug)]
#[command(name = "rsfga-precompute")]
#[command(version, about)]
struct Args {
    /// NATS server URL
    #[arg(long, env = "NATS_URL", default_value = "nats://localhost:4222")]
    nats_url: String,

    /// Unique consumer name for this instance
    #[arg(long, env = "CONSUMER_NAME", default_value = "rsfga-precompute-1")]
    consumer_name: String,

    /// Valkey/Redis URL
    #[arg(long, env = "VALKEY_URL", default_value = "redis://localhost:6379")]
    valkey_url: String,

    /// TTL for precomputed results (seconds)
    #[arg(long, env = "RESULT_TTL_SECS", default_value = "300")]
    result_ttl_secs: u64,

    /// TTL for hot-path registry entries (seconds)
    #[arg(long, env = "HOTPATH_TTL_SECS", default_value = "3600")]
    hotpath_ttl_secs: u64,

    /// Number of precompute workers
    #[arg(long, env = "WORKERS", default_value = "4")]
    workers: usize,

    /// Maximum job queue depth
    #[arg(long, env = "MAX_QUEUE_DEPTH", default_value = "10000")]
    max_queue_depth: usize,

    /// Debounce window (ms) for batching changes to the same (type, relation)
    #[arg(long, env = "BATCH_WINDOW_MS", default_value = "50")]
    batch_window_ms: u64,

    /// Storage backend: memory or postgres
    #[arg(long, env = "STORAGE_BACKEND", default_value = "memory")]
    storage_backend: String,

    /// Database URL (required for postgres backend)
    #[arg(long, env = "DATABASE_URL")]
    database_url: Option<String>,

    /// Metrics port
    #[arg(long, env = "METRICS_PORT", default_value = "9093")]
    metrics_port: u16,

    /// Log level
    #[arg(long, env = "LOG_LEVEL", default_value = "info")]
    log_level: String,

    /// NATS fetch timeout (ms)
    #[arg(long, env = "FETCH_TIMEOUT_MS", default_value = "500")]
    fetch_timeout_ms: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize tracing
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&args.log_level));
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(env_filter)
        .init();

    info!(
        version = env!("CARGO_PKG_VERSION"),
        consumer_name = %args.consumer_name,
        nats_url = %redact_url(&args.nats_url),
        valkey_url = %redact_url(&args.valkey_url),
        workers = args.workers,
        result_ttl_secs = args.result_ttl_secs,
        hotpath_ttl_secs = args.hotpath_ttl_secs,
        "Starting RSFGA Precompute worker"
    );

    // Initialize metrics server
    let builder = metrics_exporter_prometheus::PrometheusBuilder::new();
    let handle = builder
        .with_http_listener(([0, 0, 0, 0], args.metrics_port))
        .install_recorder()?;
    info!(port = args.metrics_port, "Metrics server started");

    // Connect to Valkey
    let valkey_config = ValkeyConfig {
        url: args.valkey_url.clone(),
        result_ttl_secs: args.result_ttl_secs,
        hotpath_ttl_secs: args.hotpath_ttl_secs,
    };
    let valkey_client = ValkeyClient::connect(valkey_config).await?;
    let cache = Arc::new(CheckCache::new(valkey_client));

    // Connect to NATS
    let nats_config = NatsConfig {
        servers: vec![args.nats_url.clone()],
        ..Default::default()
    };
    let nats_client = NatsClient::connect(nats_config).await?;

    // Ensure JetStream streams exist
    let js_manager = JetStreamManager::new(nats_client.clone());
    js_manager.setup_streams().await?;

    // Create storage backend for graph resolution
    let storage: Arc<dyn DataStore> = match args.storage_backend.as_str() {
        "memory" => {
            info!("Using in-memory storage backend");
            Arc::new(MemoryDataStore::new())
        }
        "postgres" => {
            let database_url = args
                .database_url
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("DATABASE_URL required for postgres backend"))?;
            info!("Connecting to PostgreSQL");
            let pg_config = PostgresConfig {
                database_url: database_url.clone(),
                ..Default::default()
            };
            let storage = PostgresDataStore::from_config(&pg_config).await?;
            Arc::new(storage)
        }
        other => anyhow::bail!("Unknown storage backend: {}", other),
    };

    // Create graph resolver (reuses rsfga-domain)
    let resolver = Arc::new(rsfga_domain::resolver::GraphResolver::new(
        Arc::clone(&storage),
    ));

    // Create worker pool
    let config = PrecomputeConfig {
        consumer_name: args.consumer_name.clone(),
        workers: args.workers,
        batch_window: Duration::from_millis(args.batch_window_ms),
        max_queue_depth: args.max_queue_depth,
        fetch_timeout: Duration::from_millis(args.fetch_timeout_ms),
    };

    let pool = WorkerPool::new(
        config.workers,
        config.max_queue_depth,
        resolver,
        Arc::clone(&cache),
        Arc::clone(&storage),
    );

    // Set up graceful shutdown
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            info!("Received shutdown signal");
            let _ = shutdown_tx.send(true);
        }
    });

    // Run the main consumer loop
    info!("Precompute worker ready — consuming from RSFGA_EVENTS");
    run_consumer_loop(nats_client, cache, pool, config, shutdown_rx).await?;

    info!("Precompute worker stopped");
    Ok(())
}

/// Main consumer loop: fetches committed events from NATS, classifies, analyzes impact,
/// and submits recomputation jobs to the worker pool.
async fn run_consumer_loop<S: DataStore + Send + Sync + 'static>(
    nats_client: NatsClient,
    cache: Arc<CheckCache>,
    pool: WorkerPool<S>,
    config: PrecomputeConfig,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    use async_nats::jetstream::consumer::pull::Config as PullConfig;
    use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy};

    let js = nats_client.jetstream();
    let stream = js
        .get_stream(rsfga_nats::STREAM_EVENTS)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to get RSFGA_EVENTS stream: {}", e))?;

    let consumer_config = PullConfig {
        durable_name: Some(config.consumer_name.clone()),
        name: Some(config.consumer_name.clone()),
        description: Some("RSFGA precompute worker".to_string()),
        filter_subject: format!("{}.*.committed", rsfga_nats::SUBJECT_EVENTS_PREFIX),
        ack_policy: AckPolicy::Explicit,
        deliver_policy: DeliverPolicy::All,
        inactive_threshold: Duration::from_secs(3600),
        ..Default::default()
    };

    let consumer = stream
        .get_or_create_consumer(&config.consumer_name, consumer_config)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create event consumer: {}", e))?;

    // TODO: In a full implementation, build relation_deps from the authorization model
    // For now, use empty deps (only direct relation matching, no computed relation expansion)
    let relation_deps = std::collections::HashMap::new();

    let mut messages = consumer
        .messages()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start message stream: {}", e))?;

    loop {
        tokio::select! {
            msg_opt = messages.next() => {
                match msg_opt {
                    Some(Ok(msg)) => {
                        // Parse committed event
                        match CommittedEvent::from_bytes(&msg.payload) {
                            Ok(event) => {
                                metrics::record_event_received("committed");

                                // Classify changes
                                let changes = classifier::classify(&event);

                                // Find affected hot-path checks
                                match impact::find_affected_checks(&changes, &cache, &relation_deps).await {
                                    Ok(jobs) => {
                                        let job_count = jobs.len();
                                        for job in jobs {
                                            pool.submit(job);
                                        }
                                        metrics::update_queue_depth(pool.queue_depth() as f64);

                                        if job_count > 0 {
                                            info!(
                                                store_id = %event.store_id,
                                                sequence = event.sequence,
                                                jobs = job_count,
                                                "Scheduled recomputation jobs"
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        warn!(error = %e, "Failed to find affected checks");
                                    }
                                }

                                // Ack the message
                                if let Err(e) = msg.ack().await {
                                    warn!(error = %e, "Failed to ack committed event");
                                }
                            }
                            Err(e) => {
                                error!(error = %e, "Failed to parse committed event");
                                let _ = msg.ack().await;
                            }
                        }
                    }
                    Some(Err(e)) => {
                        warn!(error = %e, "Error receiving message");
                    }
                    None => {
                        warn!("Message stream ended, reconnecting in 1s");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        break;
                    }
                }
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("Shutdown signal received in consumer loop");
                    break;
                }
            }
        }
    }

    Ok(())
}

/// Redact credentials from URL for safe logging.
fn redact_url(url_str: &str) -> String {
    match url::Url::parse(url_str) {
        Ok(mut url) => {
            if !url.password().unwrap_or_default().is_empty() {
                let _ = url.set_password(Some("[REDACTED]"));
            }
            url.to_string()
        }
        Err(_) => "[invalid URL]".to_string(),
    }
}
```

**Step 2: Verify compilation**

Run: `cargo check -p rsfga-precompute`
Expected: Compiles. If `GraphResolver::new` signature doesn't match, check `crates/rsfga-domain/src/resolver/graph_resolver.rs` for the exact constructor parameters and adjust.

**Step 3: Commit**

```bash
git add crates/rsfga-precompute/src/main.rs
git commit -m "[BEHAVIORAL] Implement rsfga-precompute binary with NATS consumer loop and graceful shutdown"
```

---

## Task 10: API-side hybrid resolution — feature-gated Valkey lookup

**Files:**
- Modify: `crates/rsfga-api/Cargo.toml` — add `rsfga-valkey` dependency behind `precompute` feature
- Modify: `crates/rsfga-api/src/http/state.rs` — add Valkey cache to AppState
- Modify: `crates/rsfga-api/src/http/routes.rs` — modify check handler

**Step 1: Add feature flag and dependency**

In `crates/rsfga-api/Cargo.toml`, add to `[features]`:

```toml
[features]
default = []
nats = ["rsfga-nats"]
precompute = ["rsfga-valkey"]
```

Add to `[dependencies]`:

```toml
rsfga-valkey = { path = "../rsfga-valkey", optional = true }
```

**Step 2: Extend AppState with Valkey cache**

In `crates/rsfga-api/src/http/state.rs`, add behind `#[cfg(feature = "precompute")]`:

```rust
#[cfg(feature = "precompute")]
pub precompute_cache: Option<Arc<rsfga_valkey::cache::CheckCache>>,
```

Add builder method:

```rust
#[cfg(feature = "precompute")]
pub fn with_precompute_cache(mut self, cache: Arc<rsfga_valkey::cache::CheckCache>) -> Self {
    self.precompute_cache = Some(cache);
    self
}
```

Initialize field to `None` in `new()` and other constructors.

**Step 3: Modify check handler**

In `crates/rsfga-api/src/http/routes.rs`, add Valkey lookup BEFORE the resolver call, inside the `check` handler. Add this block just before the `state.resolver.check(&check_request).await` line:

```rust
// Precomputed check: try Valkey first for sub-millisecond response
#[cfg(feature = "precompute")]
{
    if let Some(ref precompute_cache) = state.precompute_cache {
        // Get the model ID (either from request or latest)
        let model_id_for_cache = if let Some(ref mid) = body.authorization_model_id {
            Some(mid.clone())
        } else {
            state.storage.get_latest_authorization_model(&store_id)
                .await
                .ok()
                .flatten()
                .map(|m| m.id)
        };

        if let Some(model_id) = model_id_for_cache {
            // Parse object into type:id
            if let Some((obj_type, obj_id)) = check_request.object.split_once(':') {
                let cache_key = rsfga_valkey::CheckKey::new(
                    &store_id,
                    &model_id,
                    obj_type,
                    obj_id,
                    &check_request.relation,
                    &check_request.user,
                );

                if let Some(cached) = precompute_cache.get(&cache_key).await {
                    // Cache hit — return precomputed result
                    return Ok(Json(CheckResponseBody {
                        allowed: cached.allowed,
                        resolution: None,
                    })
                    .into_response());
                }

                // Cache miss — record in hot-path registry for future precomputation
                let _ = precompute_cache.record_hotpath(
                    &store_id,
                    obj_type,
                    obj_id,
                    &check_request.relation,
                    &check_request.user,
                ).await;
            }
        }
    }
}
```

**Step 4: Verify compilation (both with and without feature)**

Run: `cargo check -p rsfga-api` (without precompute)
Run: `cargo check -p rsfga-api --features precompute` (with precompute)
Expected: Both compile.

**Step 5: Commit**

```bash
git add crates/rsfga-api/
git commit -m "[BEHAVIORAL] Add precompute feature flag with Valkey hybrid resolution in check handler"
```

---

## Task 11: Docker and docker-compose setup for precompute

**Files:**
- Modify: `docker-compose.yaml` — add Valkey service and rsfga-precompute service
- Modify: `Dockerfile` — ensure rsfga-precompute binary builds with existing multi-binary pattern

**Step 1: Add Valkey and rsfga-precompute to docker-compose.yaml**

Add to `services:` section:

```yaml
  # Valkey (Redis-compatible) for precomputed check cache
  valkey:
    image: valkey/valkey:8-alpine
    ports:
      - "6379:6379"
    volumes:
      - valkey_data:/data
    healthcheck:
      test: ["CMD", "valkey-cli", "ping"]
      interval: 10s
      timeout: 5s
      retries: 5
      start_period: 5s
    profiles:
      - precompute

  # RSFGA Precompute Worker
  rsfga-precompute:
    build:
      context: .
      dockerfile: Dockerfile
      args:
        BINARY: rsfga-precompute
    environment:
      - NATS_URL=nats://nats:4222
      - CONSUMER_NAME=rsfga-precompute-1
      - VALKEY_URL=redis://valkey:6379
      - RESULT_TTL_SECS=300
      - HOTPATH_TTL_SECS=3600
      - WORKERS=4
      - MAX_QUEUE_DEPTH=10000
      - STORAGE_BACKEND=postgres
      - DATABASE_URL=postgres://rsfga:rsfga@postgres:5432/rsfga
      - METRICS_PORT=9093
    ports:
      - "9093:9093"
    depends_on:
      nats:
        condition: service_healthy
      valkey:
        condition: service_healthy
      postgres:
        condition: service_healthy
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:9093/metrics"]
      interval: 10s
      timeout: 5s
      retries: 5
      start_period: 10s
    profiles:
      - precompute
    restart: unless-stopped
```

Add to `volumes:` section:

```yaml
  valkey_data:
```

Update file header comments to include precompute profile:

```yaml
#   docker-compose --profile precompute up -d  # Start with precomputed checks
```

**Step 2: Verify Dockerfile handles rsfga-precompute**

The existing `Dockerfile` uses `ARG BINARY=rsfga` — the rsfga-precompute binary just needs the same build pattern. Verify the Dockerfile works by checking it builds `--bin $BINARY`.

**Step 3: Commit**

```bash
git add docker-compose.yaml
git commit -m "[BEHAVIORAL] Add Valkey and rsfga-precompute to docker-compose with precompute profile"
```

---

## Task 12: End-to-end integration test

**Files:**
- Create: `crates/rsfga-precompute/tests/integration_test.rs`

**Step 1: Write integration test**

This test requires NATS, Valkey, and RSFGA all running. Mark as `#[ignore]` for CI.

```rust
//! End-to-end integration test for precomputed check flow.
//!
//! Requirements:
//! - NATS running on localhost:4222 with JetStream
//! - Valkey/Redis running on localhost:6379
//! - RSFGA API running on localhost:8080 (with precompute feature)
//!
//! Run with: cargo test -p rsfga-precompute --test integration_test -- --ignored --nocapture

#[tokio::test]
#[ignore = "requires NATS + Valkey + RSFGA running"]
async fn test_precomputed_check_e2e() {
    // This test verifies the full flow:
    // 1. Create store and model
    // 2. Write a tuple (generates NATS event)
    // 3. Wait for precompute worker to process
    // 4. Verify result is in Valkey
    // 5. Check request hits Valkey cache

    // Implementation depends on running infrastructure.
    // Will be filled in during integration testing phase.
    todo!("Implement once infrastructure is running")
}
```

**Step 2: Commit**

```bash
git add crates/rsfga-precompute/tests/
git commit -m "[TEST] Add e2e integration test placeholder for precomputed check flow"
```

---

## Task 13: Update Dockerfile for rsfga-precompute binary

**Files:**
- Modify: `Dockerfile` (if needed)
- Modify: `.github/workflows/` (if release workflow needs updating)

**Step 1: Verify Dockerfile builds rsfga-precompute**

The existing Dockerfile uses `ARG BINARY=rsfga` and builds with `cargo build --release --bin $BINARY`. Test locally:

Run: `docker build --build-arg BINARY=rsfga-precompute -t rsfga-precompute:dev .`
Expected: Build succeeds.

If the `rsfga-precompute` binary requires the `precompute` feature, update the Dockerfile's cargo build line to include `--features precompute` when `BINARY=rsfga-precompute`.

**Step 2: Update release workflow matrix**

In `.github/workflows/release.yml` (or equivalent), add `rsfga-precompute` to the binary matrix. Follow the existing pattern for `rsfga-writer` and `rsfga-edge`.

**Step 3: Commit**

```bash
git add Dockerfile .github/
git commit -m "[CHORE] Add rsfga-precompute to Docker build and release workflow"
```

---

## Task 14: Quality gates and PR preparation

**Step 1: Run full quality gates**

```bash
cargo test --workspace
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
```

Fix any warnings or test failures.

**Step 2: Run rsfga-valkey key tests**

```bash
cargo test -p rsfga-valkey -- --nocapture
```

**Step 3: Run rsfga-precompute tests**

```bash
cargo test -p rsfga-precompute -- --nocapture
```

**Step 4: Commit any fixes**

```bash
git add -A
git commit -m "[CHORE] Fix clippy warnings and test issues"
```

**Step 5: Push and create PR**

```bash
git push origin feature/phase3-precomputed-check
gh pr create --title "[Phase 3] Precomputed Check - Core Infrastructure" --body "..."
```

---

## Summary

| Task | Description | Files | Est. Tests |
|------|-------------|-------|-----------|
| 1 | Workspace deps + crate registration | `Cargo.toml` | 0 |
| 2 | Scaffold `rsfga-valkey` crate | 6 files | 0 |
| 3 | Valkey key formatting tests | `keys.rs` | 7 |
| 4 | Scaffold `rsfga-precompute` crate | 9 files | 0 |
| 5 | Change classifier tests + impl | `classifier.rs` | 6 |
| 6 | Valkey integration tests | `integration_test.rs` | 5 |
| 7 | Impact analyzer + relation deps | `impact.rs` | 4 |
| 8 | Worker pool implementation | `worker.rs` | 0 (integration later) |
| 9 | Precompute binary (main loop) | `main.rs` | 0 |
| 10 | API hybrid resolution (feature-gated) | `state.rs`, `routes.rs` | 0 (manual test) |
| 11 | Docker + docker-compose | `docker-compose.yaml` | 0 |
| 12 | E2e integration test placeholder | `integration_test.rs` | 1 |
| 13 | Dockerfile + CI update | `Dockerfile`, workflows | 0 |
| 14 | Quality gates + PR | — | — |
| **Total** | | | **~23 unit + 6 integration** |

**Note:** The estimated ~140 tests from the design doc will be reached as integration tests are expanded during hands-on testing with real NATS + Valkey infrastructure (Tasks 6, 12), and as the impact analyzer and worker pool get more thorough test coverage in follow-up PRs.
