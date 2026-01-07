//! Tests for the graph resolver module.
//!
//! Organized by functionality:
//! - Direct tuple resolution
//! - Computed relations (tuple-to-userset)
//! - Union relations
//! - Intersection relations
//! - Exclusion relations
//! - Contextual tuples
//! - Safety features (depth limiting, cycle detection, timeouts)
//! - Edge cases and type constraints

mod mocks;

#[cfg(test)]
mod resolver_tests;
