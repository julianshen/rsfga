//! Mock implementations for resolver testing.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::error::{DomainError, DomainResult};
use crate::model::{AuthorizationModel, Condition, RelationDefinition, TypeDefinition};
use crate::resolver::{ModelReader, StoredTupleRef, TupleReader};

/// Mock tuple reader for testing.
pub struct MockTupleReader {
    stores: RwLock<HashSet<String>>,
    tuples: RwLock<HashMap<String, Vec<StoredTupleRef>>>,
}

impl MockTupleReader {
    pub fn new() -> Self {
        Self {
            stores: RwLock::new(HashSet::new()),
            tuples: RwLock::new(HashMap::new()),
        }
    }

    pub async fn add_store(&self, store_id: &str) {
        self.stores.write().await.insert(store_id.to_string());
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn add_tuple(
        &self,
        store_id: &str,
        object_type: &str,
        object_id: &str,
        relation: &str,
        user_type: &str,
        user_id: &str,
        user_relation: Option<&str>,
    ) {
        let key = format!("{}:{}:{}:{}", store_id, object_type, object_id, relation);
        let tuple = StoredTupleRef::new(user_type, user_id, user_relation.map(|s| s.to_string()));
        self.tuples
            .write()
            .await
            .entry(key)
            .or_default()
            .push(tuple);
    }

    #[allow(dead_code)]
    pub async fn remove_tuple(
        &self,
        store_id: &str,
        object_type: &str,
        object_id: &str,
        relation: &str,
        user_type: &str,
        user_id: &str,
    ) {
        let key = format!("{}:{}:{}:{}", store_id, object_type, object_id, relation);
        if let Some(tuples) = self.tuples.write().await.get_mut(&key) {
            tuples.retain(|t| t.user_type != user_type || t.user_id != user_id);
        }
    }

    /// Adds a tuple with a condition.
    #[allow(clippy::too_many_arguments)]
    pub async fn add_tuple_with_condition(
        &self,
        store_id: &str,
        object_type: &str,
        object_id: &str,
        relation: &str,
        user_type: &str,
        user_id: &str,
        user_relation: Option<&str>,
        condition_name: &str,
        condition_context: Option<HashMap<String, serde_json::Value>>,
    ) {
        let key = format!("{}:{}:{}:{}", store_id, object_type, object_id, relation);
        let tuple = StoredTupleRef::with_condition(
            user_type,
            user_id,
            user_relation.map(|s| s.to_string()),
            condition_name,
            condition_context,
        );
        self.tuples
            .write()
            .await
            .entry(key)
            .or_default()
            .push(tuple);
    }
}

#[async_trait]
impl TupleReader for MockTupleReader {
    async fn read_tuples(
        &self,
        store_id: &str,
        object_type: &str,
        object_id: &str,
        relation: &str,
    ) -> DomainResult<Vec<StoredTupleRef>> {
        let key = format!("{}:{}:{}:{}", store_id, object_type, object_id, relation);
        Ok(self
            .tuples
            .read()
            .await
            .get(&key)
            .cloned()
            .unwrap_or_default())
    }

    async fn store_exists(&self, store_id: &str) -> DomainResult<bool> {
        Ok(self.stores.read().await.contains(store_id))
    }
}

/// Mock model reader for testing.
pub struct MockModelReader {
    type_definitions: RwLock<HashMap<String, TypeDefinition>>,
    conditions: RwLock<HashMap<String, Vec<Condition>>>,
}

impl MockModelReader {
    pub fn new() -> Self {
        Self {
            type_definitions: RwLock::new(HashMap::new()),
            conditions: RwLock::new(HashMap::new()),
        }
    }

    pub async fn add_type(&self, store_id: &str, type_def: TypeDefinition) {
        let key = format!("{}:{}", store_id, type_def.type_name);
        self.type_definitions.write().await.insert(key, type_def);
    }

    /// Adds a condition definition to the store's model.
    pub async fn add_condition(&self, store_id: &str, condition: Condition) {
        self.conditions
            .write()
            .await
            .entry(store_id.to_string())
            .or_default()
            .push(condition);
    }
}

#[async_trait]
impl ModelReader for MockModelReader {
    async fn get_model(&self, store_id: &str) -> DomainResult<AuthorizationModel> {
        let mut model = AuthorizationModel::new("1.1");
        if let Some(conditions) = self.conditions.read().await.get(store_id) {
            for condition in conditions {
                model.add_condition(condition.clone());
            }
        }
        Ok(model)
    }

    async fn get_type_definition(
        &self,
        store_id: &str,
        type_name: &str,
    ) -> DomainResult<TypeDefinition> {
        let key = format!("{}:{}", store_id, type_name);
        self.type_definitions
            .read()
            .await
            .get(&key)
            .cloned()
            .ok_or_else(|| DomainError::TypeNotFound {
                type_name: type_name.to_string(),
            })
    }

    async fn get_relation_definition(
        &self,
        store_id: &str,
        type_name: &str,
        relation: &str,
    ) -> DomainResult<RelationDefinition> {
        let type_def = self.get_type_definition(store_id, type_name).await?;
        type_def
            .relations
            .into_iter()
            .find(|r| r.name == relation)
            .ok_or_else(|| DomainError::RelationNotFound {
                type_name: type_name.to_string(),
                relation: relation.to_string(),
            })
    }
}

/// Helper to create a new resolver with mocks.
#[allow(dead_code)]
pub fn create_resolver() -> (
    Arc<MockTupleReader>,
    Arc<MockModelReader>,
    crate::resolver::GraphResolver<MockTupleReader, MockModelReader>,
) {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());
    let resolver =
        crate::resolver::GraphResolver::new(Arc::clone(&tuple_reader), Arc::clone(&model_reader));
    (tuple_reader, model_reader, resolver)
}
