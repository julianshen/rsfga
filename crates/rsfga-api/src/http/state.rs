//! Application state for HTTP handlers.

use std::sync::Arc;

use dashmap::DashMap;
use rsfga_domain::cache::{CheckCache, CheckCacheConfig};
use rsfga_domain::resolver::{GraphResolver, ResolverConfig};
use rsfga_server::handlers::batch::BatchCheckHandler;
use rsfga_storage::DataStore;

use crate::adapters::{DataStoreModelReader, DataStoreTupleReader};

/// Stored assertion for testing authorization models.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct StoredAssertion {
    pub tuple_key: AssertionTupleKey,
    pub expectation: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contextual_tuples: Option<ContextualTuplesWrapper>,
}

/// Tuple key for an assertion.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AssertionTupleKey {
    pub user: String,
    pub relation: String,
    pub object: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<AssertionCondition>,
}

/// Condition for an assertion tuple.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AssertionCondition {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<std::collections::HashMap<String, serde_json::Value>>,
}

/// Wrapper for contextual tuples in assertions.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ContextualTuplesWrapper {
    pub tuple_keys: Vec<AssertionTupleKey>,
}

/// Application state shared across all HTTP handlers.
///
/// This struct holds all the dependencies needed by the HTTP handlers,
/// including storage, resolver, cache, and the batch check handler.
///
/// # Type Parameters
///
/// * `S` - The storage backend implementing `DataStore`
///
/// # Architecture
///
/// The state uses adapters to bridge the storage layer to the domain layer:
/// - `DataStoreTupleReader<S>` implements `TupleReader` using `DataStore`
/// - `DataStoreModelReader<S>` implements `ModelReader` using `DataStore`
///
/// This allows the `GraphResolver` and `BatchCheckHandler` to work with
/// any storage backend that implements `DataStore`.
///
/// # Cache Safety
///
/// By default, caching is **disabled** for safety. Cached positive authorization
/// decisions can serve stale results after tuple writes/deletes. To enable caching,
/// explicitly set `CheckCacheConfig::enabled` to `true`:
///
/// ```rust,ignore
/// let cache_config = CheckCacheConfig::default().with_enabled(true);
/// let state = AppState::with_cache_config(storage, cache_config);
/// ```
/// Key for assertions storage: (store_id, authorization_model_id).
pub type AssertionKey = (String, String);

#[derive(Clone)]
pub struct AppState<S: DataStore> {
    /// The storage backend.
    pub storage: Arc<S>,
    /// The batch check handler with parallel execution and deduplication.
    pub batch_handler: Arc<BatchCheckHandler<DataStoreTupleReader<S>, DataStoreModelReader<S>>>,
    /// The graph resolver for single checks.
    pub resolver: Arc<GraphResolver<DataStoreTupleReader<S>, DataStoreModelReader<S>>>,
    /// The check result cache (always created, but only attached to resolver if enabled).
    pub cache: Arc<CheckCache>,
    /// In-memory assertions storage (store_id, model_id) -> assertions.
    pub assertions: Arc<DashMap<AssertionKey, Vec<StoredAssertion>>>,
}

impl<S: DataStore> AppState<S> {
    /// Creates a new application state with default cache configuration.
    ///
    /// Note: Caching is **disabled** by default for safety. Use `with_cache_config`
    /// with an explicitly enabled config to enable caching.
    pub fn new(storage: Arc<S>) -> Self {
        Self::with_cache_config(storage, CheckCacheConfig::default())
    }

    /// Creates a new application state with custom cache configuration.
    ///
    /// # Cache Safety
    ///
    /// If `cache_config.enabled` is `false` (the default), the cache will be created
    /// but NOT attached to the resolver. This means authorization checks will always
    /// hit storage, ensuring fresh results.
    ///
    /// If `cache_config.enabled` is `true`, the cache will be attached to the resolver.
    /// This improves performance but cached results may be stale until TTL expires
    /// or invalidation occurs.
    pub fn with_cache_config(storage: Arc<S>, cache_config: CheckCacheConfig) -> Self {
        let cache = Arc::new(CheckCache::new(cache_config));
        Self::from_storage_and_cache(storage, cache)
    }

    /// Creates a new application state with a shared cache.
    ///
    /// This constructor allows multiple `AppState` instances to share the same
    /// cache, which is essential for testing cache invalidation behavior.
    ///
    /// # Cache Attachment
    ///
    /// The resolver will only use the shared cache if `cache.is_enabled()` returns
    /// `true`. If the cache is disabled, the resolver will bypass the cache entirely
    /// and always hit storage for fresh results.
    ///
    /// # Arguments
    ///
    /// * `storage` - The storage backend
    /// * `cache` - A shared cache instance
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let cache = Arc::new(CheckCache::new(CheckCacheConfig::default().with_enabled(true)));
    /// let state1 = AppState::with_shared_cache(storage.clone(), cache.clone());
    /// let state2 = AppState::with_shared_cache(storage.clone(), cache.clone());
    /// // Both state1 and state2 share the same cache
    /// ```
    pub fn with_shared_cache(storage: Arc<S>, cache: Arc<CheckCache>) -> Self {
        Self::from_storage_and_cache(storage, cache)
    }

    /// Internal helper to construct AppState from storage and cache.
    ///
    /// This extracts the common logic between `with_cache_config` and `with_shared_cache`.
    fn from_storage_and_cache(storage: Arc<S>, cache: Arc<CheckCache>) -> Self {
        // Create adapters to bridge storage to domain traits
        let tuple_reader = Arc::new(DataStoreTupleReader::new(Arc::clone(&storage)));
        let model_reader = Arc::new(DataStoreModelReader::new(Arc::clone(&storage)));

        // Create the graph resolver - only attach cache if explicitly enabled
        let resolver_config = if cache.is_enabled() {
            ResolverConfig::default().with_cache(Arc::clone(&cache))
        } else {
            ResolverConfig::default()
        };
        let resolver = Arc::new(GraphResolver::with_config(
            Arc::clone(&tuple_reader),
            Arc::clone(&model_reader),
            resolver_config,
        ));

        // Create the batch handler
        let batch_handler = Arc::new(BatchCheckHandler::new(
            Arc::clone(&resolver),
            Arc::clone(&cache),
        ));

        Self {
            storage,
            batch_handler,
            resolver,
            cache,
            assertions: Arc::new(DashMap::new()),
        }
    }
}
