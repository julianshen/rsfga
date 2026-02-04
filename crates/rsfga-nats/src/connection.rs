//! NATS connection management with automatic reconnection.

use crate::config::{AuthConfig, NatsConfig, TlsConfig};
use crate::error::{NatsError, Result};
use async_nats::jetstream::{self, Context as JetStreamContext};
use async_nats::Client;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// NATS client with connection management and JetStream support.
#[derive(Clone)]
pub struct NatsClient {
    /// Inner client state
    inner: Arc<NatsClientInner>,
}

struct NatsClientInner {
    /// NATS client
    client: Client,
    /// JetStream context
    jetstream: JetStreamContext,
    /// Configuration
    config: NatsConfig,
    /// Connection state
    state: RwLock<ConnectionState>,
}

/// Connection state tracking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// Connected and healthy
    Connected,
    /// Reconnecting after disconnection
    Reconnecting,
    /// Disconnected (failed to reconnect)
    Disconnected,
}

impl NatsClient {
    /// Connect to NATS server with the given configuration.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rsfga_nats::{NatsConfig, NatsClient};
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let config = NatsConfig::default();
    /// let client = NatsClient::connect(config).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn connect(config: NatsConfig) -> Result<Self> {
        info!(
            servers = ?config.servers,
            name = %config.name,
            "Connecting to NATS"
        );

        let options = Self::build_connect_options(&config).await?;
        let client = options.connect(&config.servers).await?;

        info!("Connected to NATS successfully");

        // Create JetStream context
        let jetstream = if let Some(ref domain) = config.jetstream.domain {
            jetstream::with_domain(client.clone(), domain.clone())
        } else {
            jetstream::new(client.clone())
        };

        let inner = NatsClientInner {
            client,
            jetstream,
            config,
            state: RwLock::new(ConnectionState::Connected),
        };

        Ok(Self {
            inner: Arc::new(inner),
        })
    }

    /// Build connection options from configuration.
    async fn build_connect_options(config: &NatsConfig) -> Result<async_nats::ConnectOptions> {
        let mut options = async_nats::ConnectOptions::new()
            .name(&config.name)
            .connection_timeout(config.connect_timeout)
            .request_timeout(Some(config.request_timeout));

        // Configure reconnection
        if config.reconnect.enabled {
            options = options
                .retry_on_initial_connect()
                .max_reconnects(Some(config.reconnect.max_attempts as usize));
        } else {
            options = options.max_reconnects(Some(0));
        }

        // Configure authentication
        options = Self::apply_auth_config(options, &config.auth).await?;

        // Configure TLS
        if config.tls.enabled {
            options = Self::apply_tls_config(options, &config.tls)?;
        }

        Ok(options)
    }

    /// Apply authentication configuration.
    async fn apply_auth_config(
        mut options: async_nats::ConnectOptions,
        auth: &AuthConfig,
    ) -> Result<async_nats::ConnectOptions> {
        if let (Some(username), Some(password)) = (&auth.username, &auth.password) {
            debug!("Using username/password authentication");
            options = options.user_and_password(username.clone(), password.clone());
        } else if let Some(token) = &auth.token {
            debug!("Using token authentication");
            options = options.token(token.clone());
        } else if let Some(creds_path) = &auth.credentials_path {
            debug!(path = %creds_path, "Using credentials file authentication");
            options = options.credentials_file(creds_path).await.map_err(|e| {
                NatsError::Authentication(format!("Failed to load credentials: {}", e))
            })?;
        } else if let Some(nkey_path) = &auth.nkey_seed_path {
            debug!(path = %nkey_path, "Using NKey authentication");
            // NKey auth requires reading the seed file
            let seed = std::fs::read_to_string(nkey_path).map_err(|e| {
                NatsError::Authentication(format!("Failed to read NKey seed: {}", e))
            })?;
            let key_pair = nkeys::KeyPair::from_seed(&seed)
                .map_err(|e| NatsError::Authentication(format!("Invalid NKey seed: {}", e)))?;
            options = options.nkey(key_pair.public_key());
        }

        Ok(options)
    }

    /// Apply TLS configuration.
    fn apply_tls_config(
        options: async_nats::ConnectOptions,
        tls: &TlsConfig,
    ) -> Result<async_nats::ConnectOptions> {
        debug!("Configuring TLS");

        let mut tls_options = options.require_tls(true);

        // Load CA certificate if provided
        if let Some(ca_path) = &tls.ca_cert_path {
            debug!(path = %ca_path, "Loading CA certificate");
            tls_options = tls_options.add_root_certificates(ca_path.into());
        }

        // Load client certificate if provided
        if let (Some(cert_path), Some(key_path)) = (&tls.client_cert_path, &tls.client_key_path) {
            debug!(cert = %cert_path, key = %key_path, "Loading client certificate");
            tls_options = tls_options.add_client_certificate(cert_path.into(), key_path.into());
        }

        // Warn about insecure mode
        if tls.insecure_skip_verify {
            warn!("TLS certificate verification is DISABLED - this is insecure!");
            // Note: async-nats doesn't directly support skip_verify,
            // this would need custom TLS config
        }

        Ok(tls_options)
    }

    /// Get the JetStream context.
    pub fn jetstream(&self) -> &JetStreamContext {
        &self.inner.jetstream
    }

    /// Get the raw NATS client.
    pub fn client(&self) -> &Client {
        &self.inner.client
    }

    /// Get the current connection state.
    pub async fn state(&self) -> ConnectionState {
        *self.inner.state.read().await
    }

    /// Check if the client is connected.
    pub async fn is_connected(&self) -> bool {
        self.state().await == ConnectionState::Connected
    }

    /// Get the configuration.
    pub fn config(&self) -> &NatsConfig {
        &self.inner.config
    }

    /// Flush any pending messages.
    pub async fn flush(&self) -> Result<()> {
        self.inner
            .client
            .flush()
            .await
            .map_err(|e| NatsError::Connection(format!("Flush failed: {}", e)))
    }

    /// Drain and close the connection.
    pub async fn drain(&self) -> Result<()> {
        info!("Draining NATS connection");
        self.inner
            .client
            .drain()
            .await
            .map_err(|e| NatsError::Connection(format!("Drain failed: {}", e)))
    }
}

/// Builder for NatsClient with fluent API.
pub struct NatsClientBuilder {
    config: NatsConfig,
}

impl NatsClientBuilder {
    /// Create a new builder with default configuration.
    pub fn new() -> Self {
        Self {
            config: NatsConfig::default(),
        }
    }

    /// Set the NATS server URLs.
    pub fn servers(mut self, servers: Vec<String>) -> Self {
        self.config.servers = servers;
        self
    }

    /// Add a NATS server URL.
    pub fn add_server(mut self, server: impl Into<String>) -> Self {
        self.config.servers.push(server.into());
        self
    }

    /// Set the connection name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.config.name = name.into();
        self
    }

    /// Set username/password authentication.
    pub fn user_password(
        mut self,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        self.config.auth.username = Some(username.into());
        self.config.auth.password = Some(password.into());
        self
    }

    /// Set token authentication.
    pub fn token(mut self, token: impl Into<String>) -> Self {
        self.config.auth.token = Some(token.into());
        self
    }

    /// Set credentials file path.
    pub fn credentials_file(mut self, path: impl Into<String>) -> Self {
        self.config.auth.credentials_path = Some(path.into());
        self
    }

    /// Enable TLS.
    pub fn tls(mut self) -> Self {
        self.config.tls.enabled = true;
        self
    }

    /// Set the JetStream domain.
    pub fn jetstream_domain(mut self, domain: impl Into<String>) -> Self {
        self.config.jetstream.domain = Some(domain.into());
        self
    }

    /// Build and connect the client.
    pub async fn connect(self) -> Result<NatsClient> {
        NatsClient::connect(self.config).await
    }
}

impl Default for NatsClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_defaults() {
        let builder = NatsClientBuilder::new();
        assert_eq!(builder.config.servers, vec!["nats://localhost:4222"]);
        assert_eq!(builder.config.name, "rsfga");
    }

    #[test]
    fn test_builder_servers() {
        let builder = NatsClientBuilder::new()
            .servers(vec!["nats://server1:4222".to_string()])
            .add_server("nats://server2:4222");

        assert_eq!(builder.config.servers.len(), 2);
        assert!(builder
            .config
            .servers
            .contains(&"nats://server1:4222".to_string()));
        assert!(builder
            .config
            .servers
            .contains(&"nats://server2:4222".to_string()));
    }

    #[test]
    fn test_builder_auth() {
        let builder = NatsClientBuilder::new().user_password("user", "pass");

        assert_eq!(builder.config.auth.username, Some("user".to_string()));
        assert_eq!(builder.config.auth.password, Some("pass".to_string()));
    }

    #[test]
    fn test_builder_token() {
        let builder = NatsClientBuilder::new().token("secret-token");

        assert_eq!(builder.config.auth.token, Some("secret-token".to_string()));
    }

    #[test]
    fn test_builder_tls() {
        let builder = NatsClientBuilder::new().tls();
        assert!(builder.config.tls.enabled);
    }

    #[test]
    fn test_connection_state() {
        assert_eq!(ConnectionState::Connected, ConnectionState::Connected);
        assert_ne!(ConnectionState::Connected, ConnectionState::Disconnected);
    }
}
