//! Performance benchmarks for ListUsers operations.
//!
//! Run with: cargo bench -p rsfga-domain --bench list_users_bench
//!
//! These benchmarks measure:
//! - Direct user list performance (small and large sets)
//! - Wildcard user expansion performance
//! - Userset (group#member) resolution performance
//! - CEL condition evaluation impact
//! - Truncation behavior overhead
//!
//! ListUsers is the inverse of ListObjects - it returns all users who have
//! a specified relationship with an object.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use tokio::runtime::Runtime;

use rsfga_domain::error::{DomainError, DomainResult};
use rsfga_domain::model::{
    AuthorizationModel, Condition, ConditionParameter, RelationDefinition, TypeDefinition, Userset,
};
use rsfga_domain::resolver::{
    GraphResolver, ListUsersRequest, ModelReader, StoredTupleRef, TupleReader, UserFilter,
};

// =============================================================================
// Benchmark-specific mock implementations
// =============================================================================

/// Fast in-memory tuple reader for benchmarks.
/// Uses pre-populated HashMaps for O(1) lookups.
struct BenchTupleReader {
    stores: HashSet<String>,
    /// Tuples keyed by "store:object_type:object_id:relation"
    tuples: HashMap<String, Vec<StoredTupleRef>>,
    /// Objects keyed by "store:type"
    objects: HashMap<String, Vec<String>>,
}

impl BenchTupleReader {
    fn new() -> Self {
        Self {
            stores: HashSet::new(),
            tuples: HashMap::new(),
            objects: HashMap::new(),
        }
    }

    fn add_store(&mut self, store_id: &str) {
        self.stores.insert(store_id.to_string());
    }

    fn add_tuple(
        &mut self,
        store_id: &str,
        object_type: &str,
        object_id: &str,
        relation: &str,
        user_type: &str,
        user_id: &str,
    ) {
        let key = format!("{store_id}:{object_type}:{object_id}:{relation}");
        let tuple = StoredTupleRef::new(user_type, user_id, None);
        self.tuples.entry(key).or_default().push(tuple);

        // Track objects for get_objects_of_type
        let obj_key = format!("{store_id}:{object_type}");
        let objects = self.objects.entry(obj_key).or_default();
        if !objects.contains(&object_id.to_string()) {
            objects.push(object_id.to_string());
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add_tuple_with_userset(
        &mut self,
        store_id: &str,
        object_type: &str,
        object_id: &str,
        relation: &str,
        user_type: &str,
        user_id: &str,
        user_relation: &str,
    ) {
        let key = format!("{store_id}:{object_type}:{object_id}:{relation}");
        let tuple = StoredTupleRef::new(user_type, user_id, Some(user_relation.to_string()));
        self.tuples.entry(key).or_default().push(tuple);

        // Track objects
        let obj_key = format!("{store_id}:{object_type}");
        let objects = self.objects.entry(obj_key).or_default();
        if !objects.contains(&object_id.to_string()) {
            objects.push(object_id.to_string());
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add_tuple_with_condition(
        &mut self,
        store_id: &str,
        object_type: &str,
        object_id: &str,
        relation: &str,
        user_type: &str,
        user_id: &str,
        condition_name: &str,
        condition_context: Option<HashMap<String, serde_json::Value>>,
    ) {
        let key = format!("{store_id}:{object_type}:{object_id}:{relation}");
        let tuple = StoredTupleRef::with_condition(
            user_type,
            user_id,
            None,
            condition_name,
            condition_context,
        );
        self.tuples.entry(key).or_default().push(tuple);

        // Track objects
        let obj_key = format!("{store_id}:{object_type}");
        let objects = self.objects.entry(obj_key).or_default();
        if !objects.contains(&object_id.to_string()) {
            objects.push(object_id.to_string());
        }
    }

    fn add_wildcard_tuple(
        &mut self,
        store_id: &str,
        object_type: &str,
        object_id: &str,
        relation: &str,
        user_type: &str,
    ) {
        let key = format!("{store_id}:{object_type}:{object_id}:{relation}");
        let tuple = StoredTupleRef::new(user_type, "*", None);
        self.tuples.entry(key).or_default().push(tuple);

        // Track objects
        let obj_key = format!("{store_id}:{object_type}");
        let objects = self.objects.entry(obj_key).or_default();
        if !objects.contains(&object_id.to_string()) {
            objects.push(object_id.to_string());
        }
    }
}

#[async_trait]
impl TupleReader for BenchTupleReader {
    async fn read_tuples(
        &self,
        store_id: &str,
        object_type: &str,
        object_id: &str,
        relation: &str,
    ) -> DomainResult<Vec<StoredTupleRef>> {
        let key = format!("{store_id}:{object_type}:{object_id}:{relation}");
        Ok(self.tuples.get(&key).cloned().unwrap_or_default())
    }

    async fn store_exists(&self, store_id: &str) -> DomainResult<bool> {
        Ok(self.stores.contains(store_id))
    }

    async fn get_objects_of_type(
        &self,
        store_id: &str,
        object_type: &str,
        _limit: usize,
    ) -> DomainResult<Vec<String>> {
        let key = format!("{store_id}:{object_type}");
        Ok(self.objects.get(&key).cloned().unwrap_or_default())
    }
}

/// Fast in-memory model reader for benchmarks.
struct BenchModelReader {
    type_definitions: HashMap<String, TypeDefinition>,
    conditions: HashMap<String, Condition>,
}

impl BenchModelReader {
    fn new() -> Self {
        Self {
            type_definitions: HashMap::new(),
            conditions: HashMap::new(),
        }
    }

    fn add_type(&mut self, store_id: &str, type_def: TypeDefinition) {
        let key = format!("{}:{}", store_id, type_def.type_name);
        self.type_definitions.insert(key, type_def);
    }

    fn add_condition(&mut self, condition: Condition) {
        self.conditions.insert(condition.name.clone(), condition);
    }
}

#[async_trait]
impl ModelReader for BenchModelReader {
    async fn get_model(&self, _store_id: &str) -> DomainResult<AuthorizationModel> {
        let mut model = AuthorizationModel::new("1.1");
        for condition in self.conditions.values() {
            model.add_condition(condition.clone());
        }
        Ok(model)
    }

    async fn get_type_definition(
        &self,
        store_id: &str,
        type_name: &str,
    ) -> DomainResult<TypeDefinition> {
        let key = format!("{store_id}:{type_name}");
        self.type_definitions
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

// =============================================================================
// Setup helpers
// =============================================================================

/// Create a setup with direct users (specified count).
fn create_direct_users_setup(user_count: usize) -> (Arc<BenchTupleReader>, Arc<BenchModelReader>) {
    let mut tuple_reader = BenchTupleReader::new();
    let mut model_reader = BenchModelReader::new();

    tuple_reader.add_store("bench-store");

    // Add document type with viewer relation (direct via This)
    let doc_type = TypeDefinition {
        type_name: "document".to_string(),
        relations: vec![RelationDefinition {
            name: "viewer".to_string(),
            type_constraints: vec!["user".into()],
            rewrite: Userset::This,
        }],
    };
    model_reader.add_type("bench-store", doc_type);

    // Add user type (for reference)
    let user_type = TypeDefinition {
        type_name: "user".to_string(),
        relations: vec![],
    };
    model_reader.add_type("bench-store", user_type);

    // Add users as viewers of doc1
    for i in 0..user_count {
        tuple_reader.add_tuple(
            "bench-store",
            "document",
            "doc1",
            "viewer",
            "user",
            &format!("user{i}"),
        );
    }

    (Arc::new(tuple_reader), Arc::new(model_reader))
}

/// Create a setup with wildcards mixed with direct users.
fn create_wildcard_setup() -> (Arc<BenchTupleReader>, Arc<BenchModelReader>) {
    let mut tuple_reader = BenchTupleReader::new();
    let mut model_reader = BenchModelReader::new();

    tuple_reader.add_store("bench-store");

    // Add document type with viewer relation that accepts wildcards
    let doc_type = TypeDefinition {
        type_name: "document".to_string(),
        relations: vec![RelationDefinition {
            name: "viewer".to_string(),
            type_constraints: vec!["user".into()],
            rewrite: Userset::This,
        }],
    };
    model_reader.add_type("bench-store", doc_type);

    let user_type = TypeDefinition {
        type_name: "user".to_string(),
        relations: vec![],
    };
    model_reader.add_type("bench-store", user_type);

    // Add wildcard for public access
    tuple_reader.add_wildcard_tuple("bench-store", "document", "doc1", "viewer", "user");

    // Add some direct users too
    for i in 0..10 {
        tuple_reader.add_tuple(
            "bench-store",
            "document",
            "doc1",
            "viewer",
            "user",
            &format!("user{i}"),
        );
    }

    (Arc::new(tuple_reader), Arc::new(model_reader))
}

/// Create a setup with userset references (group#member).
fn create_userset_setup() -> (Arc<BenchTupleReader>, Arc<BenchModelReader>) {
    let mut tuple_reader = BenchTupleReader::new();
    let mut model_reader = BenchModelReader::new();

    tuple_reader.add_store("bench-store");

    // Add user type
    let user_type = TypeDefinition {
        type_name: "user".to_string(),
        relations: vec![],
    };
    model_reader.add_type("bench-store", user_type);

    // Add group type with member relation
    let group_type = TypeDefinition {
        type_name: "group".to_string(),
        relations: vec![RelationDefinition {
            name: "member".to_string(),
            type_constraints: vec!["user".into()],
            rewrite: Userset::This,
        }],
    };
    model_reader.add_type("bench-store", group_type);

    // Add document type with viewer that accepts both user and group#member
    let doc_type = TypeDefinition {
        type_name: "document".to_string(),
        relations: vec![RelationDefinition {
            name: "viewer".to_string(),
            type_constraints: vec!["user".into(), "group#member".into()],
            rewrite: Userset::This,
        }],
    };
    model_reader.add_type("bench-store", doc_type);

    // Add group members
    for i in 0..20 {
        tuple_reader.add_tuple(
            "bench-store",
            "group",
            "engineering",
            "member",
            "user",
            &format!("user{i}"),
        );
    }

    // Grant group access to document via userset
    tuple_reader.add_tuple_with_userset(
        "bench-store",
        "document",
        "doc1",
        "viewer",
        "group",
        "engineering",
        "member",
    );

    // Add some direct users too
    for i in 20..25 {
        tuple_reader.add_tuple(
            "bench-store",
            "document",
            "doc1",
            "viewer",
            "user",
            &format!("user{i}"),
        );
    }

    (Arc::new(tuple_reader), Arc::new(model_reader))
}

/// Create a setup with CEL conditions.
fn create_condition_setup() -> (Arc<BenchTupleReader>, Arc<BenchModelReader>) {
    let mut tuple_reader = BenchTupleReader::new();
    let mut model_reader = BenchModelReader::new();

    tuple_reader.add_store("bench-store");

    // Add time_condition to the model
    model_reader.add_condition(Condition {
        name: "time_condition".to_string(),
        expression: "request.current_time < request.expiry".to_string(),
        parameters: vec![
            ConditionParameter {
                name: "current_time".to_string(),
                type_name: "int".to_string(),
            },
            ConditionParameter {
                name: "expiry".to_string(),
                type_name: "int".to_string(),
            },
        ],
    });

    // Add document type with viewer relation
    let doc_type = TypeDefinition {
        type_name: "document".to_string(),
        relations: vec![RelationDefinition {
            name: "viewer".to_string(),
            type_constraints: vec!["user".into()],
            rewrite: Userset::This,
        }],
    };
    model_reader.add_type("bench-store", doc_type);

    let user_type = TypeDefinition {
        type_name: "user".to_string(),
        relations: vec![],
    };
    model_reader.add_type("bench-store", user_type);

    // Add users with conditions
    for i in 0..20 {
        let mut ctx = HashMap::new();
        ctx.insert("expiry".to_string(), serde_json::json!(1000000 + i));
        tuple_reader.add_tuple_with_condition(
            "bench-store",
            "document",
            "doc1",
            "viewer",
            "user",
            &format!("user{i}"),
            "time_condition",
            Some(ctx),
        );
    }

    (Arc::new(tuple_reader), Arc::new(model_reader))
}

// =============================================================================
// Benchmarks
// =============================================================================

/// Benchmark ListUsers with small number of direct users (10).
fn bench_list_users_direct_small(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (tuple_reader, model_reader) = create_direct_users_setup(10);
    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let mut group = c.benchmark_group("list_users_direct");
    group.throughput(Throughput::Elements(1));

    group.bench_function("small_10_users", |b| {
        b.to_async(&rt).iter(|| async {
            let request = ListUsersRequest::new(
                "bench-store",
                "document:doc1",
                "viewer",
                vec![UserFilter::new("user")],
            );
            let result = resolver.list_users(black_box(&request), 1000).await;
            black_box(result)
        })
    });

    group.finish();
}

/// Benchmark ListUsers with large number of direct users (1000).
fn bench_list_users_direct_large(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (tuple_reader, model_reader) = create_direct_users_setup(1000);
    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let mut group = c.benchmark_group("list_users_direct");
    group.throughput(Throughput::Elements(1));

    group.bench_function("large_1000_users", |b| {
        b.to_async(&rt).iter(|| async {
            let request = ListUsersRequest::new(
                "bench-store",
                "document:doc1",
                "viewer",
                vec![UserFilter::new("user")],
            );
            let result = resolver.list_users(black_box(&request), 2000).await;
            black_box(result)
        })
    });

    group.finish();
}

/// Benchmark ListUsers with wildcards.
fn bench_list_users_wildcards(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (tuple_reader, model_reader) = create_wildcard_setup();
    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let mut group = c.benchmark_group("list_users_wildcards");
    group.throughput(Throughput::Elements(1));

    group.bench_function("mixed_wildcard_and_direct", |b| {
        b.to_async(&rt).iter(|| async {
            let request = ListUsersRequest::new(
                "bench-store",
                "document:doc1",
                "viewer",
                vec![UserFilter::new("user")],
            );
            let result = resolver.list_users(black_box(&request), 1000).await;
            black_box(result)
        })
    });

    group.finish();
}

/// Benchmark ListUsers with userset references (group#member).
fn bench_list_users_usersets(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (tuple_reader, model_reader) = create_userset_setup();
    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let mut group = c.benchmark_group("list_users_usersets");
    group.throughput(Throughput::Elements(1));

    // Benchmark with user filter (returns direct users)
    group.bench_function("user_filter", |b| {
        b.to_async(&rt).iter(|| async {
            let request = ListUsersRequest::new(
                "bench-store",
                "document:doc1",
                "viewer",
                vec![UserFilter::new("user")],
            );
            let result = resolver.list_users(black_box(&request), 1000).await;
            black_box(result)
        })
    });

    // Benchmark with userset filter (returns group#member references)
    group.bench_function("userset_filter", |b| {
        b.to_async(&rt).iter(|| async {
            let request = ListUsersRequest::new(
                "bench-store",
                "document:doc1",
                "viewer",
                vec![UserFilter::with_relation("group", "member")],
            );
            let result = resolver.list_users(black_box(&request), 1000).await;
            black_box(result)
        })
    });

    group.finish();
}

/// Benchmark ListUsers with CEL condition evaluation.
fn bench_list_users_conditions(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (tuple_reader, model_reader) = create_condition_setup();
    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let mut group = c.benchmark_group("list_users_conditions");
    group.throughput(Throughput::Elements(1));

    group.bench_function("with_cel_conditions", |b| {
        b.to_async(&rt).iter(|| async {
            // Create request with context for condition evaluation
            let mut context = HashMap::new();
            context.insert("current_time".to_string(), serde_json::json!(500));

            let request = ListUsersRequest::with_context(
                "bench-store",
                "document:doc1",
                "viewer",
                vec![UserFilter::new("user")],
                vec![],
                context,
            );
            let result = resolver.list_users(black_box(&request), 1000).await;
            black_box(result)
        })
    });

    group.finish();
}

/// Benchmark ListUsers truncation behavior.
fn bench_list_users_truncation(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (tuple_reader, model_reader) = create_direct_users_setup(100);
    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let mut group = c.benchmark_group("list_users_truncation");
    group.throughput(Throughput::Elements(1));

    // Compare performance with different max_results limits
    for max_results in [10, 50, 100] {
        group.bench_with_input(
            BenchmarkId::from_parameter(max_results),
            &max_results,
            |b, &limit| {
                b.to_async(&rt).iter(|| async {
                    let request = ListUsersRequest::new(
                        "bench-store",
                        "document:doc1",
                        "viewer",
                        vec![UserFilter::new("user")],
                    );
                    let result = resolver.list_users(black_box(&request), limit).await;
                    black_box(result)
                })
            },
        );
    }

    group.finish();
}

/// Benchmark varying user counts to test scalability.
fn bench_list_users_scalability(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("list_users_scalability");
    group.throughput(Throughput::Elements(1));

    for user_count in [10, 100, 500, 1000] {
        let (tuple_reader, model_reader) = create_direct_users_setup(user_count);
        let resolver = GraphResolver::new(tuple_reader, model_reader);

        group.bench_with_input(
            BenchmarkId::from_parameter(user_count),
            &user_count,
            |b, _| {
                b.to_async(&rt).iter(|| async {
                    let request = ListUsersRequest::new(
                        "bench-store",
                        "document:doc1",
                        "viewer",
                        vec![UserFilter::new("user")],
                    );
                    let result = resolver.list_users(black_box(&request), 2000).await;
                    black_box(result)
                })
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_list_users_direct_small,
    bench_list_users_direct_large,
    bench_list_users_wildcards,
    bench_list_users_usersets,
    bench_list_users_conditions,
    bench_list_users_truncation,
    bench_list_users_scalability,
);
criterion_main!(benches);
