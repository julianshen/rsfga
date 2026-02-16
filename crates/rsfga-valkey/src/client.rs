use redis::aio::ConnectionManager;
use tracing::info;

use crate::error::{Result, ValkeyError};

#[derive(Debug, Clone)]
pub struct ValkeyConfig {
    pub url: String,
    pub result_ttl_secs: u64,
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

#[derive(Clone)]
pub struct ValkeyClient {
    conn: ConnectionManager,
    config: ValkeyConfig,
}

impl ValkeyClient {
    pub async fn connect(config: ValkeyConfig) -> Result<Self> {
        let client = redis::Client::open(config.url.as_str()).map_err(ValkeyError::Connection)?;
        let conn = ConnectionManager::new(client)
            .await
            .map_err(ValkeyError::Connection)?;
        info!(url = %redact_url(&config.url), "Valkey connection established");
        Ok(Self { conn, config })
    }

    pub fn connection(&self) -> ConnectionManager {
        self.conn.clone()
    }

    pub fn result_ttl_secs(&self) -> u64 {
        self.config.result_ttl_secs
    }

    pub fn hotpath_ttl_secs(&self) -> u64 {
        self.config.hotpath_ttl_secs
    }
}

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
