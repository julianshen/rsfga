//! Application state for HTTP handlers.

use std::sync::Arc;

use rsfga_domain::cache::{CheckCache, CheckCacheConfig};
use rsfga_domain::resolver::GraphResolver;
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
#[derive(Clone)]
pub struct AppState<S: DataStore> {
    /// The storage backend.
    pub storage: Arc<S>,
    /// The batch check handler with parallel execution and deduplication.
    pub batch_handler: Arc<BatchCheckHandler<DataStoreTupleReader<S>, DataStoreModelReader<S>>>,
    /// The graph resolver for single checks.
    pub resolver: Arc<GraphResolver<DataStoreTupleReader<S>, DataStoreModelReader<S>>>,
    /// The check result cache.
    pub cache: Arc<CheckCache>,
}

impl<S: DataStore> AppState<S> {
    /// Creates a new application state with default cache configuration.
    pub fn new(storage: Arc<S>) -> Self {
        Self::with_cache_config(storage, CheckCacheConfig::default())
    }

    /// Creates a new application state with custom cache configuration.
    pub fn with_cache_config(storage: Arc<S>, cache_config: CheckCacheConfig) -> Self {
        // Create adapters to bridge storage to domain traits
        let tuple_reader = Arc::new(DataStoreTupleReader::new(Arc::clone(&storage)));
        let model_reader = Arc::new(DataStoreModelReader::new(Arc::clone(&storage)));

        // Create the graph resolver
        let resolver = Arc::new(GraphResolver::new(
            Arc::clone(&tuple_reader),
            Arc::clone(&model_reader),
        ));

        // Create the check cache
        let cache = Arc::new(CheckCache::new(cache_config));

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
