//! Performance benchmarks for check operations.
//!
//! Run with: cargo bench -p rsfga-domain
//!
//! These benchmarks measure:
//! - Direct relation check throughput
//! - Cache hit ratio and performance
//! - Computed relation (union) performance
//! - Batch check performance
//!
//! Target baselines (from OpenFGA):
//! - Direct check: ~500 req/s
//! - RSFGA target: 1000+ req/s (2x improvement)

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use tokio::runtime::Runtime;

use rsfga_domain::error::{DomainError, DomainResult};
use rsfga_domain::model::{AuthorizationModel, RelationDefinition, TypeDefinition, Userset};
use rsfga_domain::resolver::{
    CheckRequest, GraphResolver, ModelReader, ResolverConfig, StoredTupleRef, TupleReader,
};

// =============================================================================
// Benchmark-specific mock implementations
// =============================================================================

/// Fast in-memory tuple reader for benchmarks.
/// Uses pre-populated HashMaps for O(1) lookups.
struct BenchTupleReader {
    stores: HashSet<String>,
    tuples: HashMap<String, Vec<StoredTupleRef>>,
}

impl BenchTupleReader {
    fn new() -> Self {
        Self {
            stores: HashSet::new(),
            tuples: HashMap::new(),
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
        let key = format!("{}:{}:{}:{}", store_id, object_type, object_id, relation);
        let tuple = StoredTupleRef {
            user_type: user_type.to_string(),
            user_id: user_id.to_string(),
            user_relation: None,
        };
        self.tuples.entry(key).or_default().push(tuple);
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
        let key = format!("{}:{}:{}:{}", store_id, object_type, object_id, relation);
        Ok(self.tuples.get(&key).cloned().unwrap_or_default())
    }

    async fn store_exists(&self, store_id: &str) -> DomainResult<bool> {
        Ok(self.stores.contains(store_id))
    }
}

/// Fast in-memory model reader for benchmarks.
struct BenchModelReader {
    type_definitions: HashMap<String, TypeDefinition>,
}

impl BenchModelReader {
    fn new() -> Self {
        Self {
            type_definitions: HashMap::new(),
        }
    }

    fn add_type(&mut self, store_id: &str, type_def: TypeDefinition) {
        let key = format!("{}:{}", store_id, type_def.type_name);
        self.type_definitions.insert(key, type_def);
    }
}

#[async_trait]
impl ModelReader for BenchModelReader {
    async fn get_model(&self, _store_id: &str) -> DomainResult<AuthorizationModel> {
        Ok(AuthorizationModel::new("1.1"))
    }

    async fn get_type_definition(
        &self,
        store_id: &str,
        type_name: &str,
    ) -> DomainResult<TypeDefinition> {
        let key = format!("{}:{}", store_id, type_name);
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

/// Create a simple document/user setup with direct relations.
fn create_direct_relation_setup() -> (Arc<BenchTupleReader>, Arc<BenchModelReader>) {
    let mut tuple_reader = BenchTupleReader::new();
    let mut model_reader = BenchModelReader::new();

    tuple_reader.add_store("bench-store");

    // Add document type with viewer relation (direct via This)
    let doc_type = TypeDefinition {
        type_name: "document".to_string(),
        relations: vec![RelationDefinition {
            name: "viewer".to_string(),
            type_constraints: vec!["user".to_string()],
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

    // Add some tuples (user:alice is viewer of document:doc1, doc2, ..., doc100)
    for i in 0..100 {
        tuple_reader.add_tuple(
            "bench-store",
            "document",
            &format!("doc{}", i),
            "viewer",
            "user",
            "alice",
        );
    }

    (Arc::new(tuple_reader), Arc::new(model_reader))
}

/// Create setup with computed union relations.
fn create_union_relation_setup() -> (Arc<BenchTupleReader>, Arc<BenchModelReader>) {
    let mut tuple_reader = BenchTupleReader::new();
    let mut model_reader = BenchModelReader::new();

    tuple_reader.add_store("bench-store");

    // Document type with can_view = owner or viewer
    let doc_type = TypeDefinition {
        type_name: "document".to_string(),
        relations: vec![
            RelationDefinition {
                name: "owner".to_string(),
                type_constraints: vec!["user".to_string()],
                rewrite: Userset::This,
            },
            RelationDefinition {
                name: "viewer".to_string(),
                type_constraints: vec!["user".to_string()],
                rewrite: Userset::This,
            },
            RelationDefinition {
                name: "can_view".to_string(),
                type_constraints: vec![],
                rewrite: Userset::Union {
                    children: vec![
                        Userset::ComputedUserset {
                            relation: "owner".to_string(),
                        },
                        Userset::ComputedUserset {
                            relation: "viewer".to_string(),
                        },
                    ],
                },
            },
        ],
    };
    model_reader.add_type("bench-store", doc_type);

    let user_type = TypeDefinition {
        type_name: "user".to_string(),
        relations: vec![],
    };
    model_reader.add_type("bench-store", user_type);

    // Alice is viewer (but not owner)
    for i in 0..100 {
        tuple_reader.add_tuple(
            "bench-store",
            "document",
            &format!("doc{}", i),
            "viewer",
            "user",
            "alice",
        );
    }

    (Arc::new(tuple_reader), Arc::new(model_reader))
}

// =============================================================================
// Benchmarks
// =============================================================================

/// Benchmark direct relation checks (tuple lookup).
fn bench_direct_check(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (tuple_reader, model_reader) = create_direct_relation_setup();
    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let mut group = c.benchmark_group("direct_check");
    group.throughput(Throughput::Elements(1));

    group.bench_function("direct_check_allowed", |b| {
        b.to_async(&rt).iter(|| async {
            let request = CheckRequest::new(
                "bench-store".to_string(),
                "user:alice".to_string(),
                "viewer".to_string(),
                "document:doc0".to_string(),
                vec![],
            );
            let result = resolver.check(black_box(&request)).await;
            black_box(result)
        })
    });

    group.bench_function("direct_check_denied", |b| {
        b.to_async(&rt).iter(|| async {
            let request = CheckRequest::new(
                "bench-store".to_string(),
                "user:bob".to_string(), // Bob has no tuples
                "viewer".to_string(),
                "document:doc0".to_string(),
                vec![],
            );
            let result = resolver.check(black_box(&request)).await;
            black_box(result)
        })
    });

    group.finish();
}

/// Benchmark computed union relation checks.
fn bench_union_check(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (tuple_reader, model_reader) = create_union_relation_setup();
    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let mut group = c.benchmark_group("union_check");
    group.throughput(Throughput::Elements(1));

    group.bench_function("union_check_allowed", |b| {
        b.to_async(&rt).iter(|| async {
            let request = CheckRequest::new(
                "bench-store".to_string(),
                "user:alice".to_string(),
                "can_view".to_string(), // Computed from owner OR viewer
                "document:doc0".to_string(),
                vec![],
            );
            let result = resolver.check(black_box(&request)).await;
            black_box(result)
        })
    });

    group.finish();
}

/// Benchmark cache performance.
fn bench_cache_operations(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (tuple_reader, model_reader) = create_direct_relation_setup();

    // Create resolver with cache enabled
    let config = ResolverConfig::default();
    let resolver = GraphResolver::with_config(tuple_reader, model_reader, config);

    let mut group = c.benchmark_group("cache");
    group.throughput(Throughput::Elements(1));

    // First, warm up the cache
    rt.block_on(async {
        for i in 0..100 {
            let request = CheckRequest::new(
                "bench-store".to_string(),
                "user:alice".to_string(),
                "viewer".to_string(),
                format!("document:doc{}", i),
                vec![],
            );
            let _ = resolver.check(&request).await;
        }
    });

    // Benchmark cache hits (same requests)
    group.bench_function("cache_hit", |b| {
        b.to_async(&rt).iter(|| async {
            let request = CheckRequest::new(
                "bench-store".to_string(),
                "user:alice".to_string(),
                "viewer".to_string(),
                "document:doc0".to_string(),
                vec![],
            );
            let result = resolver.check(black_box(&request)).await;
            black_box(result)
        })
    });

    group.finish();
}

/// Benchmark raw check throughput for batch-sized workloads.
///
/// Note: This measures sequential check performance for batch-sized workloads,
/// not the actual `BatchCheckHandler` with deduplication and singleflight.
/// For batch handler benchmarks with deduplication, see rsfga-server benches.
///
/// This benchmark is useful for:
/// - Measuring baseline check throughput
/// - Comparing cache hit performance across batch sizes
/// - Validating resolver scalability under sequential load
fn bench_batch_check(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (tuple_reader, model_reader) = create_direct_relation_setup();
    let resolver = Arc::new(GraphResolver::new(tuple_reader, model_reader));

    let mut group = c.benchmark_group("batch_check");

    for batch_size in [10, 25, 50] {
        group.throughput(Throughput::Elements(batch_size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(batch_size),
            &batch_size,
            |b, &size| {
                b.to_async(&rt).iter(|| {
                    let resolver = Arc::clone(&resolver);
                    async move {
                        let mut results = Vec::with_capacity(size);
                        for i in 0..size {
                            let request = CheckRequest::new(
                                "bench-store".to_string(),
                                "user:alice".to_string(),
                                "viewer".to_string(),
                                format!("document:doc{}", i % 100),
                                vec![],
                            );
                            results.push(resolver.check(&request).await);
                        }
                        black_box(results)
                    }
                })
            },
        );
    }

    group.finish();
}

/// Benchmark varying tuple counts to test scalability.
fn bench_tuple_count_scalability(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("tuple_scalability");
    group.throughput(Throughput::Elements(1));

    for tuple_count in [10, 100, 1000] {
        let (tuple_reader, model_reader) = {
            let mut tuple_reader = BenchTupleReader::new();
            let mut model_reader = BenchModelReader::new();

            tuple_reader.add_store("bench-store");

            let doc_type = TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".to_string()],
                    rewrite: Userset::This,
                }],
            };
            model_reader.add_type("bench-store", doc_type);

            let user_type = TypeDefinition {
                type_name: "user".to_string(),
                relations: vec![],
            };
            model_reader.add_type("bench-store", user_type);

            // Add tuples
            for i in 0..tuple_count {
                tuple_reader.add_tuple(
                    "bench-store",
                    "document",
                    &format!("doc{}", i),
                    "viewer",
                    "user",
                    "alice",
                );
            }

            (Arc::new(tuple_reader), Arc::new(model_reader))
        };

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        group.bench_with_input(
            BenchmarkId::from_parameter(tuple_count),
            &tuple_count,
            |b, _| {
                b.to_async(&rt).iter(|| async {
                    let request = CheckRequest::new(
                        "bench-store".to_string(),
                        "user:alice".to_string(),
                        "viewer".to_string(),
                        "document:doc0".to_string(),
                        vec![],
                    );
                    let result = resolver.check(black_box(&request)).await;
                    black_box(result)
                })
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_direct_check,
    bench_union_check,
    bench_cache_operations,
    bench_batch_check,
    bench_tuple_count_scalability,
);
criterion_main!(benches);
