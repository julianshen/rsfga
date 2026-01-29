//! Mock implementations for resolver testing.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::error::{DomainError, DomainResult};
use crate::model::{AuthorizationModel, Condition, RelationDefinition, TypeDefinition};
use crate::resolver::{ModelReader, ObjectTupleInfo, StoredTupleRef, TupleReader};

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
        let key = format!("{store_id}:{object_type}:{object_id}:{relation}");
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
        let key = format!("{store_id}:{object_type}:{object_id}:{relation}");
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
        let key = format!("{store_id}:{object_type}:{object_id}:{relation}");
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
        let key = format!("{store_id}:{object_type}:{object_id}:{relation}");
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

    async fn get_objects_of_type(
        &self,
        store_id: &str,
        object_type: &str,
        max_count: usize,
    ) -> DomainResult<Vec<String>> {
        // Extract unique object IDs for the given type from stored tuples
        let tuples = self.tuples.read().await;
        let prefix = format!("{store_id}:{object_type}:");
        let mut objects = HashSet::new();

        for key in tuples.keys() {
            if key.starts_with(&prefix) {
                // Key format: "store_id:object_type:object_id:relation"
                let parts: Vec<&str> = key.splitn(4, ':').collect();
                if parts.len() >= 3 {
                    objects.insert(parts[2].to_string());
                }
            }
        }

        let mut result: Vec<String> = objects.into_iter().collect();
        result.truncate(max_count);
        Ok(result)
    }

    async fn get_objects_for_user(
        &self,
        store_id: &str,
        user: &str,
        object_type: &str,
        relation: Option<&str>,
        max_count: usize,
    ) -> DomainResult<Vec<ObjectTupleInfo>> {
        // Parse the user to get user_type:user_id
        let (user_type, user_id) = if let Some((t, id)) = user.split_once(':') {
            (t, id)
        } else {
            return Ok(Vec::new());
        };

        // Search through all tuples to find matching ones
        let tuples = self.tuples.read().await;
        let prefix = format!("{store_id}:{object_type}:");
        let mut results = Vec::new();

        for (key, stored_tuples) in tuples.iter() {
            if !key.starts_with(&prefix) {
                continue;
            }

            // Key format: "store_id:object_type:object_id:relation"
            let parts: Vec<&str> = key.splitn(4, ':').collect();
            if parts.len() < 4 {
                continue;
            }

            let tuple_object_id = parts[2];
            let tuple_relation = parts[3];

            // Filter by relation if specified
            if let Some(rel) = relation {
                if tuple_relation != rel {
                    continue;
                }
            }

            // Check if user matches any of the stored tuples
            for tuple in stored_tuples {
                if tuple.user_type == user_type && tuple.user_id == user_id {
                    let info = if let Some(ref cond_name) = tuple.condition_name {
                        ObjectTupleInfo::with_condition(
                            tuple_object_id.to_string(),
                            tuple_relation.to_string(),
                            cond_name.clone(),
                            tuple.condition_context.clone(),
                        )
                    } else {
                        ObjectTupleInfo::new(
                            tuple_object_id.to_string(),
                            tuple_relation.to_string(),
                        )
                    };
                    results.push(info);
                    if results.len() >= max_count {
                        return Ok(results);
                    }
                }
            }
        }

        Ok(results)
    }

    async fn get_objects_with_parents(
        &self,
        store_id: &str,
        object_type: &str,
        tupleset_relation: &str,
        parent_type: &str,
        parent_ids: &[String],
        max_count: usize,
    ) -> DomainResult<Vec<String>> {
        if parent_ids.is_empty() {
            return Ok(Vec::new());
        }

        let parent_id_set: HashSet<&str> = parent_ids.iter().map(|s| s.as_str()).collect();

        // Search through all tuples to find objects that have the given parents
        let tuples = self.tuples.read().await;
        let prefix = format!("{store_id}:{object_type}:");
        let mut results = HashSet::new();

        for (key, stored_tuples) in tuples.iter() {
            if !key.starts_with(&prefix) {
                continue;
            }

            // Key format: "store_id:object_type:object_id:relation"
            let parts: Vec<&str> = key.splitn(4, ':').collect();
            if parts.len() < 4 {
                continue;
            }

            let tuple_object_id = parts[2];
            let tuple_relation = parts[3];

            // Check if this is the tupleset relation we're looking for
            if tuple_relation != tupleset_relation {
                continue;
            }

            // Check if any of the tuples reference one of our parent objects
            for tuple in stored_tuples {
                if tuple.user_type == parent_type && parent_id_set.contains(tuple.user_id.as_str())
                {
                    results.insert(tuple_object_id.to_string());
                    if results.len() >= max_count {
                        return Ok(results.into_iter().collect());
                    }
                }
            }
        }

        Ok(results.into_iter().collect())
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
        let type_defs = self.type_definitions.read().await;
        for (key, type_def) in type_defs.iter() {
            if let Some(key_store_id) = key.split(':').next() {
                if key_store_id == store_id {
                    model.type_definitions.push(type_def.clone());
                }
            }
        }
        Ok(model)
    }

    async fn get_model_by_id(
        &self,
        store_id: &str,
        _authorization_model_id: &str,
    ) -> DomainResult<AuthorizationModel> {
        // Mock implementation: just return the same model as get_model
        // In real tests, specific model versions would be handled differently
        self.get_model(store_id).await
    }

    async fn get_type_definition(
        &self,
        store_id: &str,
        type_name: &str,
    ) -> DomainResult<TypeDefinition> {
        let key = format!("{store_id}:{type_name}");
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
