//! Application state for HTTP handlers.

use std::sync::Arc;

use rsfga_storage::DataStore;

/// Application state shared across all HTTP handlers.
///
/// This struct holds all the dependencies needed by the HTTP handlers.
/// For now, it focuses on storage operations. The full resolver and cache
/// integration will be added when needed.
#[derive(Clone)]
pub struct AppState<S: DataStore> {
    /// The storage backend.
    pub storage: Arc<S>,
}

impl<S: DataStore> AppState<S> {
    /// Creates a new application state.
    pub fn new(storage: Arc<S>) -> Self {
        Self { storage }
    }
}
