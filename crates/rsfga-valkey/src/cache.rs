use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::client::ValkeyClient;
use crate::error::Result;
use crate::keys::{self, CheckKey};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrecomputedResult {
    pub allowed: bool,
    pub computed_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone)]
pub struct CheckCache {
    client: ValkeyClient,
}

impl CheckCache {
    pub fn new(client: ValkeyClient) -> Self {
        Self { client }
    }

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

    pub async fn set(&self, key: &CheckKey, result: &PrecomputedResult) -> Result<()> {
        let redis_key = key.to_redis_key();
        let json = serde_json::to_string(result)?;
        let ttl = self.client.result_ttl_secs();
        let mut conn = self.client.connection();
        conn.set_ex::<_, _, ()>(&redis_key, &json, ttl).await?;
        debug!(key = %redis_key, ttl_secs = ttl, "Stored precomputed result");
        Ok(())
    }

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
        redis::cmd("ZADD")
            .arg(&key)
            .arg("GT")
            .arg(score)
            .arg(&member)
            .exec_async(&mut conn)
            .await?;
        Ok(())
    }

    pub async fn scan_hotpath(&self, store_id: &str, pattern: &str) -> Result<Vec<String>> {
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

    pub async fn get_all_hotpath(&self, store_id: &str) -> Result<Vec<String>> {
        let key = keys::hotpath_key(store_id);
        let mut conn = self.client.connection();
        let mut results = Vec::new();
        let mut cursor: u64 = 0;
        loop {
            let (new_cursor, members): (u64, Vec<String>) = redis::cmd("ZSCAN")
                .arg(&key)
                .arg(cursor)
                .arg("COUNT")
                .arg(100)
                .query_async(&mut conn)
                .await?;
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

    pub async fn evict_stale_hotpath(&self, store_id: &str) -> Result<u64> {
        let key = keys::hotpath_key(store_id);
        let cutoff = chrono::Utc::now().timestamp() as f64 - self.client.hotpath_ttl_secs() as f64;
        let mut conn = self.client.connection();
        let removed: u64 = conn.zrembyscore(&key, f64::NEG_INFINITY, cutoff).await?;
        if removed > 0 {
            debug!(store_id, removed, "Evicted stale hot-path entries");
        }
        Ok(removed)
    }

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
