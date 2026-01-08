//! rsfga-domain: Core authorization domain logic
//!
//! This crate contains the core authorization logic including:
//! - Type system and authorization model parsing
//! - Graph resolver for permission checks
//! - Check result caching
//! - Model validation
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │                rsfga-domain                  │
//! ├─────────────────────────────────────────────┤
//! │  model/      - Type system & DSL parser     │
//! │  resolver/   - Graph resolution engine      │
//! │  cache/      - Check result caching         │
//! │  validation/ - Model validation             │
//! └─────────────────────────────────────────────┘
//! ```

pub mod cache;
pub mod cel;
pub mod error;
pub mod model;
pub mod resolver;
pub mod validation;

// Re-export commonly used types at the crate root
pub use cache::{CacheKey, CheckCache, CheckCacheConfig};
pub use error::{DomainError, DomainResult};
