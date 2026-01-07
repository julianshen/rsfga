//! Traits for storage operations needed by the resolver.

use async_trait::async_trait;

use crate::error::DomainResult;
use crate::model::{AuthorizationModel, RelationDefinition, TypeDefinition};

use super::types::StoredTupleRef;

/// Trait for tuple storage operations needed by the resolver.
#[async_trait]
pub trait TupleReader: Send + Sync {
    /// Reads tuples matching the given criteria.
    async fn read_tuples(
        &self,
        store_id: &str,
        object_type: &str,
        object_id: &str,
        relation: &str,
    ) -> DomainResult<Vec<StoredTupleRef>>;

    /// Checks if a store exists.
    async fn store_exists(&self, store_id: &str) -> DomainResult<bool>;
}

/// Trait for authorization model operations needed by the resolver.
#[async_trait]
pub trait ModelReader: Send + Sync {
    /// Gets the authorization model for a store.
    async fn get_model(&self, store_id: &str) -> DomainResult<AuthorizationModel>;

    /// Gets a type definition by name.
    async fn get_type_definition(
        &self,
        store_id: &str,
        type_name: &str,
    ) -> DomainResult<TypeDefinition>;

    /// Gets a relation definition from a type.
    async fn get_relation_definition(
        &self,
        store_id: &str,
        type_name: &str,
        relation: &str,
    ) -> DomainResult<RelationDefinition>;
}
