//! Traits for storage operations needed by the resolver.

use async_trait::async_trait;

use crate::error::DomainResult;
use crate::model::{AuthorizationModel, RelationDefinition, TypeDefinition};

use super::types::{ObjectTupleInfo, StoredTupleRef};

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

    /// Gets all object IDs of a given type in a store, limited by max_count.
    ///
    /// Used by ListObjects to get candidate objects for permission checks.
    /// The limit parameter provides DoS protection (constraint C11).
    ///
    /// Default implementation returns an empty list. Override to enable ListObjects.
    async fn get_objects_of_type(
        &self,
        _store_id: &str,
        _object_type: &str,
        _max_count: usize,
    ) -> DomainResult<Vec<String>> {
        // Default: return empty list (ListObjects not supported)
        Ok(Vec::new())
    }

    /// Gets objects where the specified user has a direct relation.
    ///
    /// This is a reverse lookup: instead of checking "can user X access object Y",
    /// it finds "all objects Y where user X has relation R".
    ///
    /// Used by ListObjects for efficient permission resolution - starts from user's
    /// direct assignments rather than checking all objects.
    ///
    /// Returns ObjectTupleInfo for the specified user and object_type, including
    /// condition info for tuples that have conditions.
    /// The limit parameter provides DoS protection (constraint C11).
    ///
    /// Default implementation returns an empty list. Override to enable efficient ListObjects.
    async fn get_objects_for_user(
        &self,
        _store_id: &str,
        _user: &str,
        _object_type: &str,
        _relation: Option<&str>,
        _max_count: usize,
    ) -> DomainResult<Vec<ObjectTupleInfo>> {
        // Default: return empty list (reverse lookup not supported)
        Ok(Vec::new())
    }

    /// Finds objects that reference any of the given parent objects via a specific relation.
    ///
    /// This is the core query for the ReverseExpand algorithm. For TupleToUserset relations,
    /// after finding which parent objects the user can access, this method efficiently finds
    /// all child objects that have those parents.
    ///
    /// # Arguments
    ///
    /// * `store_id` - The store to query
    /// * `object_type` - The type of objects to find (e.g., "document")
    /// * `tupleset_relation` - The relation that references parents (e.g., "parent")
    /// * `parent_type` - The type of the parent objects (e.g., "folder")
    /// * `parent_ids` - The IDs of parent objects to search for
    /// * `max_count` - Maximum number of results (DoS protection)
    ///
    /// # Returns
    ///
    /// A vector of object IDs (not full type:id, just the ID part) that reference any
    /// of the given parents via the tupleset relation.
    ///
    /// # Example
    ///
    /// For a model with `define viewer: viewer from parent` on documents:
    /// - User has viewer access to folder:reports and folder:archives
    /// - This method finds all documents with parent = folder:reports OR parent = folder:archives
    async fn get_objects_with_parents(
        &self,
        _store_id: &str,
        _object_type: &str,
        _tupleset_relation: &str,
        _parent_type: &str,
        _parent_ids: &[String],
        _max_count: usize,
    ) -> DomainResult<Vec<String>> {
        // Default: return empty list (ReverseExpand not supported, falls back to forward-scan)
        Ok(Vec::new())
    }
}

/// Trait for authorization model operations needed by the resolver.
#[async_trait]
pub trait ModelReader: Send + Sync {
    /// Gets the latest authorization model for a store.
    async fn get_model(&self, store_id: &str) -> DomainResult<AuthorizationModel>;

    /// Gets a specific authorization model by its ID.
    ///
    /// Returns an error if the model with the given ID is not found.
    async fn get_model_by_id(
        &self,
        store_id: &str,
        authorization_model_id: &str,
    ) -> DomainResult<AuthorizationModel>;

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

    /// Gets a relation definition from a specific model version.
    ///
    /// If `authorization_model_id` is `None`, uses the latest model.
    /// Otherwise, fetches the specific model version and extracts the relation.
    async fn get_relation_definition_with_model_id(
        &self,
        store_id: &str,
        type_name: &str,
        relation: &str,
        authorization_model_id: Option<&str>,
    ) -> DomainResult<RelationDefinition> {
        // Default implementation: fetch model and extract relation
        let model = match authorization_model_id {
            Some(model_id) => self.get_model_by_id(store_id, model_id).await?,
            None => self.get_model(store_id).await?,
        };

        let type_def = model
            .type_definitions
            .into_iter()
            .find(|td| td.type_name == type_name)
            .ok_or_else(|| crate::error::DomainError::TypeNotFound {
                type_name: type_name.to_string(),
            })?;

        type_def
            .relations
            .into_iter()
            .find(|r| r.name == relation)
            .ok_or_else(|| crate::error::DomainError::RelationNotFound {
                type_name: type_name.to_string(),
                relation: relation.to_string(),
            })
    }
}
