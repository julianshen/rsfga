//! HTTP REST API endpoints.
//!
//! Implements OpenFGA-compatible REST API using Axum.
//!
//! # OpenFGA API Endpoints
//!
//! | Endpoint | Method | Description |
//! |----------|--------|-------------|
//! | `/stores/{store_id}/check` | POST | Permission check |
//! | `/stores/{store_id}/batch-check` | POST | Batch permission check |
//! | `/stores/{store_id}/expand` | POST | Expand userset |
//! | `/stores/{store_id}/write` | POST | Write tuples |
//! | `/stores/{store_id}/read` | POST | Read tuples |
//! | `/stores/{store_id}/list-objects` | POST | List accessible objects |
//! | `/stores` | POST | Create store |
//! | `/stores` | GET | List stores |
//! | `/stores/{store_id}` | GET | Get store |
//! | `/stores/{store_id}` | DELETE | Delete store |

pub mod routes;
pub mod state;

pub use routes::{
    create_router, create_router_with_body_limit, create_router_with_observability,
    create_router_with_observability_and_limit, DEFAULT_BODY_LIMIT,
};
pub use state::{
    AppState, AssertionCondition, AssertionKey, AssertionTupleKey, ContextualTuplesWrapper,
    StoredAssertion,
};

#[cfg(test)]
mod tests;
