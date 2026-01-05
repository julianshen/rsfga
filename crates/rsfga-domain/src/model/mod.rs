//! Authorization model types and DSL parser.
//!
//! This module contains:
//! - Core type definitions (User, Object, Relation, Tuple)
//! - Authorization model structures
//! - DSL parser for OpenFGA model format

mod parser;
mod types;
#[cfg(test)]
mod types_proptest;

pub use parser::{parse, ParserError, ParserResult};
pub use types::*;
