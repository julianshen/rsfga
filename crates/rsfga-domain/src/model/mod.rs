//! Authorization model types and DSL parser.
//!
//! This module contains:
//! - Core type definitions (User, Object, Relation, Tuple)
//! - Authorization model structures
//! - DSL parser for OpenFGA model format
//! - Type system for efficient lookups with caching

mod parser;
mod type_system;
mod types;
#[cfg(test)]
mod types_proptest;

pub use parser::{parse, ParserError, ParserResult};
pub use type_system::TypeSystem;
pub use types::{
    AuthorizationModel, Object, Relation, RelationDefinition, Store, Tuple, TypeDefinition, User,
    Userset,
};
