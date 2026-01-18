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
//! - ListUsers API

mod mocks;

#[cfg(test)]
mod list_users_tests;
#[cfg(test)]
mod resolver_tests;
