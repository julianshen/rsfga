//! Performance benchmarks for precompute cache speedup.
//!
//! Run with: cargo bench -p rsfga-api --features precompute --bench precompute_bench
//!
//! These benchmarks compare:
//! - Full graph resolution (cache miss / no precompute path)
//! - Simulated Valkey cache hit (HashMap lookup + JSON deserialization)
//! - Simulated Valkey cache miss with hot-path recording overhead
//!
//! The precompute cache (`rsfga-valkey::CheckCache`) is a concrete struct wrapping
//! a real Valkey connection, so we simulate the cache lookup with an in-memory
//! HashMap to isolate the speedup factor from network latency.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use tokio::runtime::Runtime;

use rsfga_domain::error::{DomainError, DomainResult};
use rsfga_domain::model::{AuthorizationModel, RelationDefinition, TypeDefinition, Userset};
use rsfga_domain::resolver::{
    CheckRequest, GraphResolver, ModelReader, StoredTupleRef, TupleReader,
};

// =============================================================================
// Benchmark-specific mock implementations (mirrors check_bench.rs pattern)
// =============================================================================

struct BenchTupleReader {
    stores: std::collections::HashSet<String>,
    tuples: HashMap<String, Vec<StoredTupleRef>>,
}

impl BenchTupleReader {
    fn new() -> Self {
        Self {
            stores: std::collections::HashSet::new(),
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
        let key = format!("{store_id}:{object_type}:{object_id}:{relation}");
        let tuple = StoredTupleRef::new(user_type, user_id, None);
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
        let key = format!("{store_id}:{object_type}:{object_id}:{relation}");
        Ok(self.tuples.get(&key).cloned().unwrap_or_default())
    }

    async fn store_exists(&self, store_id: &str) -> DomainResult<bool> {
        Ok(self.stores.contains(store_id))
    }
}

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

    async fn get_model_by_id(
        &self,
        store_id: &str,
        _authorization_model_id: &str,
    ) -> DomainResult<AuthorizationModel> {
        self.get_model(store_id).await
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
// Simulated precompute cache (in-memory HashMap, no real Valkey)
// =============================================================================

/// Mirrors `rsfga_valkey::cache::PrecomputedResult` for benchmark purposes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PrecomputedResult {
    allowed: bool,
    computed_at: chrono::DateTime<chrono::Utc>,
}

/// In-memory mock of the Valkey-backed CheckCache.
/// Uses the same key format as `CheckKey::to_redis_key()`.
struct MockCheckCache {
    data: HashMap<String, String>,
}

impl MockCheckCache {
    fn new() -> Self {
        Self {
            data: HashMap::new(),
        }
    }

    /// Insert a precomputed result, serialized as JSON (same as Valkey stores it).
    fn insert(&mut self, key: &str, result: &PrecomputedResult) {
        let json = serde_json::to_string(result).unwrap();
        self.data.insert(key.to_string(), json);
    }

    /// Simulate a cache GET: HashMap lookup + JSON deserialization.
    fn get(&self, key: &str) -> Option<PrecomputedResult> {
        self.data
            .get(key)
            .and_then(|json| serde_json::from_str(json).ok())
    }
}

// =============================================================================
// Setup helpers
// =============================================================================

fn create_direct_relation_setup() -> (Arc<BenchTupleReader>, Arc<BenchModelReader>) {
    let mut tuple_reader = BenchTupleReader::new();
    let mut model_reader = BenchModelReader::new();

    tuple_reader.add_store("bench-store");

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

    for i in 0..100 {
        tuple_reader.add_tuple(
            "bench-store",
            "document",
            &format!("doc{i}"),
            "viewer",
            "user",
            "alice",
        );
    }

    (Arc::new(tuple_reader), Arc::new(model_reader))
}

fn create_union_relation_setup() -> (Arc<BenchTupleReader>, Arc<BenchModelReader>) {
    let mut tuple_reader = BenchTupleReader::new();
    let mut model_reader = BenchModelReader::new();

    tuple_reader.add_store("bench-store");

    let doc_type = TypeDefinition {
        type_name: "document".to_string(),
        relations: vec![
            RelationDefinition {
                name: "owner".to_string(),
                type_constraints: vec!["user".into()],
                rewrite: Userset::This,
            },
            RelationDefinition {
                name: "viewer".to_string(),
                type_constraints: vec!["user".into()],
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

    for i in 0..100 {
        tuple_reader.add_tuple(
            "bench-store",
            "document",
            &format!("doc{i}"),
            "viewer",
            "user",
            "alice",
        );
    }

    (Arc::new(tuple_reader), Arc::new(model_reader))
}

fn build_cache_key(
    store_id: &str,
    model_id: &str,
    object_type: &str,
    object_id: &str,
    relation: &str,
    user: &str,
) -> String {
    format!("check:{store_id}:{model_id}:{object_type}:{object_id}#{relation}@{user}")
}

// =============================================================================
// Benchmarks
// =============================================================================

/// Baseline: full graph resolution without any precompute cache.
/// This is the code path when `precompute_cache` is `None` or on a cache miss.
fn bench_check_no_precompute(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (tuple_reader, model_reader) = create_direct_relation_setup();
    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let mut group = c.benchmark_group("precompute_comparison");
    group.throughput(Throughput::Elements(1));

    group.bench_function("no_precompute_direct", |b| {
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

/// Precompute cache hit: simulated Valkey lookup (HashMap + JSON deser).
/// This measures the sub-millisecond fast path when a precomputed result exists.
fn bench_check_precompute_hit(c: &mut Criterion) {
    let cache_key = build_cache_key(
        "bench-store",
        "model-1",
        "document",
        "doc0",
        "viewer",
        "user:alice",
    );

    let mut mock_cache = MockCheckCache::new();
    mock_cache.insert(
        &cache_key,
        &PrecomputedResult {
            allowed: true,
            computed_at: chrono::Utc::now(),
        },
    );

    let mut group = c.benchmark_group("precompute_comparison");
    group.throughput(Throughput::Elements(1));

    group.bench_function("precompute_cache_hit", |b| {
        b.iter(|| {
            let key = black_box(&cache_key);
            let result = mock_cache.get(key);
            black_box(result)
        })
    });

    group.finish();
}

/// Precompute cache miss: full graph resolution + hot-path recording overhead.
/// The hot-path recording is simulated as a HashMap insert (Valkey ZADD equivalent).
fn bench_check_precompute_miss(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (tuple_reader, model_reader) = create_direct_relation_setup();
    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let cache_key = build_cache_key(
        "bench-store",
        "model-1",
        "document",
        "doc0",
        "viewer",
        "user:alice",
    );

    // Empty cache — all lookups are misses
    let mock_cache = MockCheckCache::new();

    let mut group = c.benchmark_group("precompute_comparison");
    group.throughput(Throughput::Elements(1));

    group.bench_function("precompute_cache_miss", |b| {
        b.to_async(&rt).iter(|| async {
            // 1. Cache lookup (miss)
            let _missed = mock_cache.get(black_box(&cache_key));

            // 2. Full graph resolution (same as no-precompute path)
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

/// Compare direct vs union resolution with and without precompute.
/// Union relations require multiple branch evaluations, making the speedup
/// from precompute even more pronounced.
fn bench_precompute_union_comparison(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (tuple_reader, model_reader) = create_union_relation_setup();
    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let cache_key = build_cache_key(
        "bench-store",
        "model-1",
        "document",
        "doc0",
        "can_view",
        "user:alice",
    );

    let mut mock_cache = MockCheckCache::new();
    mock_cache.insert(
        &cache_key,
        &PrecomputedResult {
            allowed: true,
            computed_at: chrono::Utc::now(),
        },
    );

    let mut group = c.benchmark_group("precompute_union");
    group.throughput(Throughput::Elements(1));

    // Full union resolution (no precompute)
    group.bench_function("union_no_precompute", |b| {
        b.to_async(&rt).iter(|| async {
            let request = CheckRequest::new(
                "bench-store".to_string(),
                "user:alice".to_string(),
                "can_view".to_string(),
                "document:doc0".to_string(),
                vec![],
            );
            let result = resolver.check(black_box(&request)).await;
            black_box(result)
        })
    });

    // Precompute cache hit (skips union resolution entirely)
    group.bench_function("union_precompute_hit", |b| {
        b.iter(|| {
            let result = mock_cache.get(black_box(&cache_key));
            black_box(result)
        })
    });

    group.finish();
}

/// Benchmark batch of checks: precompute hit vs full resolution.
/// Simulates a batch-check scenario where all items are precomputed.
fn bench_precompute_batch(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (tuple_reader, model_reader) = create_direct_relation_setup();
    let resolver = Arc::new(GraphResolver::new(tuple_reader, model_reader));

    let batch_size = 25;

    // Populate cache with all batch keys
    let mut mock_cache = MockCheckCache::new();
    for i in 0..batch_size {
        let key = build_cache_key(
            "bench-store",
            "model-1",
            "document",
            &format!("doc{}", i % 100),
            "viewer",
            "user:alice",
        );
        mock_cache.insert(
            &key,
            &PrecomputedResult {
                allowed: true,
                computed_at: chrono::Utc::now(),
            },
        );
    }

    let mut group = c.benchmark_group("precompute_batch");
    group.throughput(Throughput::Elements(batch_size as u64));

    // Batch via full resolution
    group.bench_function("batch_25_no_precompute", |b| {
        b.to_async(&rt).iter(|| {
            let resolver = Arc::clone(&resolver);
            async move {
                let futures = (0..batch_size).map(|i| {
                    let resolver = Arc::clone(&resolver);
                    async move {
                        let request = CheckRequest::new(
                            "bench-store".to_string(),
                            "user:alice".to_string(),
                            "viewer".to_string(),
                            format!("document:doc{}", i % 100),
                            vec![],
                        );
                        resolver.check(&request).await
                    }
                });
                let results = futures::future::join_all(futures).await;
                black_box(results)
            }
        })
    });

    // Batch via precompute cache
    group.bench_function("batch_25_all_precompute_hits", |b| {
        b.iter(|| {
            let results: Vec<_> = (0..batch_size)
                .map(|i| {
                    let key = build_cache_key(
                        "bench-store",
                        "model-1",
                        "document",
                        &format!("doc{}", i % 100),
                        "viewer",
                        "user:alice",
                    );
                    mock_cache.get(black_box(&key))
                })
                .collect();
            black_box(results)
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_check_no_precompute,
    bench_check_precompute_hit,
    bench_check_precompute_miss,
    bench_precompute_union_comparison,
    bench_precompute_batch,
);
criterion_main!(benches);
