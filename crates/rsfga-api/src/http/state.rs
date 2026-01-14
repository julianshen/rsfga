//! Application state for HTTP handlers.

use std::sync::Arc;

use rsfga_domain::cache::{CheckCache, CheckCacheConfig};
use rsfga_domain::resolver::{GraphResolver, ResolverConfig};
use rsfga_server::handlers::batch::BatchCheckHandler;
use rsfga_storage::DataStore;

use crate::adapters::{DataStoreModelReader, DataStoreTupleReader};

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
        // Create adapters to bridge storage to domain traits
        let tuple_reader = Arc::new(DataStoreTupleReader::new(Arc::clone(&storage)));
        let model_reader = Arc::new(DataStoreModelReader::new(Arc::clone(&storage)));

        // Create the check cache
        let cache = Arc::new(CheckCache::new(cache_config.clone()));

        // Create the graph resolver - only attach cache if explicitly enabled
        let resolver_config = if cache_config.enabled {
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
        }
    }
}
