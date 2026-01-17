//! Performance benchmarks for BatchCheckHandler deduplication.
//!
//! Run with: cargo bench -p rsfga-server
//!
//! These benchmarks measure:
//! - Intra-batch deduplication effectiveness
//! - Throughput with varying duplicate ratios (0%, 25%, 50%, 75%)
//! - Comparison of deduplicated vs non-deduplicated execution
//!
//! Target: >500 checks/s batch throughput with >90% deduplication effectiveness.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use tokio::runtime::Runtime;

use rsfga_domain::cache::{CheckCache, CheckCacheConfig};
use rsfga_domain::error::{DomainError, DomainResult};
use rsfga_domain::model::{AuthorizationModel, RelationDefinition, TypeDefinition, Userset};
use rsfga_domain::resolver::{GraphResolver, ModelReader, StoredTupleRef, TupleReader};
use rsfga_server::handlers::batch::{BatchCheckHandler, BatchCheckItem, BatchCheckRequest};

// =============================================================================
// Benchmark-specific mock implementations
// =============================================================================

/// Fast in-memory tuple reader for benchmarks.
struct BenchTupleReader {
    tuples: HashMap<String, Vec<StoredTupleRef>>,
}

impl BenchTupleReader {
    fn new() -> Self {
        Self {
            tuples: HashMap::new(),
        }
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

    async fn store_exists(&self, _store_id: &str) -> DomainResult<bool> {
        Ok(true)
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

/// Create a setup for batch check benchmarks with direct relations.
fn create_batch_check_setup() -> (Arc<BenchTupleReader>, Arc<BenchModelReader>) {
    let mut tuple_reader = BenchTupleReader::new();
    let mut model_reader = BenchModelReader::new();

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

    // Add user type
    let user_type = TypeDefinition {
        type_name: "user".to_string(),
        relations: vec![],
    };
    model_reader.add_type("bench-store", user_type);

    // Add tuples for multiple users and documents
    for doc_id in 0..100 {
        for user_id in 0..10 {
            tuple_reader.add_tuple(
                "bench-store",
                "document",
                &format!("doc{doc_id}"),
                "viewer",
                "user",
                &format!("user{user_id}"),
            );
        }
    }

    (Arc::new(tuple_reader), Arc::new(model_reader))
}

/// Generate batch check items with a specified duplicate ratio.
///
/// # Arguments
/// * `batch_size` - Total number of items in the batch
/// * `duplicate_ratio` - Ratio of duplicates (0.0 = no duplicates, 1.0 = all same)
///
/// # Returns
/// A vector of batch check items with the specified duplicate pattern.
fn generate_batch_with_duplicates(batch_size: usize, duplicate_ratio: f64) -> Vec<BatchCheckItem> {
    let unique_count = ((1.0 - duplicate_ratio) * batch_size as f64).ceil() as usize;
    let unique_count = unique_count.max(1); // At least 1 unique

    (0..batch_size)
        .map(|i| {
            let doc_idx = i % unique_count;
            BatchCheckItem { context: std::collections::HashMap::new(),
                user: "user:user0".to_string(),
                relation: "viewer".to_string(),
                object: format!("document:doc{doc_idx}"),
            }
        })
        .collect()
}

/// Count unique checks in a batch (for verification).
fn count_unique_checks(checks: &[BatchCheckItem]) -> usize {
    use std::collections::HashSet;
    let unique: HashSet<_> = checks
        .iter()
        .map(|c| (&c.user, &c.relation, &c.object))
        .collect();
    unique.len()
}

// =============================================================================
// Benchmarks
// =============================================================================

/// Benchmark BatchCheckHandler with varying duplicate ratios.
///
/// This measures the effectiveness of intra-batch deduplication by comparing
/// throughput with different ratios of duplicate checks within a batch.
fn bench_batch_handler_deduplication(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (tuple_reader, model_reader) = create_batch_check_setup();
    let resolver = Arc::new(GraphResolver::new(tuple_reader, model_reader));
    let cache = Arc::new(CheckCache::new(CheckCacheConfig::default()));
    let handler = BatchCheckHandler::new(Arc::clone(&resolver), cache);

    let batch_size = 50; // MAX_BATCH_SIZE

    let mut group = c.benchmark_group("batch_handler_deduplication");
    group.throughput(Throughput::Elements(batch_size as u64));

    // Test different duplicate ratios
    for (label, ratio) in [
        ("0%_duplicates", 0.0),
        ("25%_duplicates", 0.25),
        ("50%_duplicates", 0.50),
        ("75%_duplicates", 0.75),
    ] {
        let checks = generate_batch_with_duplicates(batch_size, ratio);
        let unique_count = count_unique_checks(&checks);

        // Report deduplication effectiveness
        let dedup_effectiveness = 1.0 - (unique_count as f64 / batch_size as f64);
        println!(
            "{}: {} total, {} unique, {:.1}% dedup effectiveness",
            label,
            batch_size,
            unique_count,
            dedup_effectiveness * 100.0
        );

        group.bench_with_input(
            BenchmarkId::new("throughput", label),
            &checks,
            |b, checks| {
                b.to_async(&rt).iter(|| async {
                    let request = BatchCheckRequest::new("bench-store", checks.clone());
                    let result = handler.check(black_box(request)).await;
                    black_box(result)
                })
            },
        );
    }

    group.finish();
}

/// Benchmark BatchCheckHandler with different batch sizes.
///
/// This measures how throughput scales with batch size, demonstrating
/// the amortized cost of deduplication overhead.
fn bench_batch_handler_scaling(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (tuple_reader, model_reader) = create_batch_check_setup();
    let resolver = Arc::new(GraphResolver::new(tuple_reader, model_reader));
    let cache = Arc::new(CheckCache::new(CheckCacheConfig::default()));
    let handler = BatchCheckHandler::new(Arc::clone(&resolver), cache);

    let mut group = c.benchmark_group("batch_handler_scaling");

    // Test different batch sizes with 50% duplicates
    for batch_size in [10, 25, 50] {
        let checks = generate_batch_with_duplicates(batch_size, 0.50);

        group.throughput(Throughput::Elements(batch_size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(batch_size),
            &checks,
            |b, checks| {
                b.to_async(&rt).iter(|| async {
                    let request = BatchCheckRequest::new("bench-store", checks.clone());
                    let result = handler.check(black_box(request)).await;
                    black_box(result)
                })
            },
        );
    }

    group.finish();
}

/// Benchmark to compare with and without deduplication benefits.
///
/// This demonstrates the throughput improvement from deduplication by
/// comparing batches with all unique checks vs. all duplicate checks.
fn bench_deduplication_benefit(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (tuple_reader, model_reader) = create_batch_check_setup();
    let resolver = Arc::new(GraphResolver::new(tuple_reader, model_reader));
    let cache = Arc::new(CheckCache::new(CheckCacheConfig::default()));
    let handler = BatchCheckHandler::new(Arc::clone(&resolver), cache);

    let batch_size = 50;

    let mut group = c.benchmark_group("deduplication_benefit");
    group.throughput(Throughput::Elements(batch_size as u64));

    // All unique checks (worst case for deduplication)
    let unique_checks: Vec<_> = (0..batch_size)
        .map(|i| BatchCheckItem { context: std::collections::HashMap::new(),
            user: format!("user:user{}", i % 10),
            relation: "viewer".to_string(),
            object: format!("document:doc{i}"),
        })
        .collect();

    group.bench_function("all_unique", |b| {
        b.to_async(&rt).iter(|| async {
            let request = BatchCheckRequest::new("bench-store", unique_checks.clone());
            let result = handler.check(black_box(request)).await;
            black_box(result)
        })
    });

    // All duplicate checks (best case for deduplication)
    let duplicate_checks: Vec<_> = (0..batch_size)
        .map(|_| BatchCheckItem { context: std::collections::HashMap::new(),
            user: "user:user0".to_string(),
            relation: "viewer".to_string(),
            object: "document:doc0".to_string(),
        })
        .collect();

    group.bench_function("all_duplicates", |b| {
        b.to_async(&rt).iter(|| async {
            let request = BatchCheckRequest::new("bench-store", duplicate_checks.clone());
            let result = handler.check(black_box(request)).await;
            black_box(result)
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_batch_handler_deduplication,
    bench_batch_handler_scaling,
    bench_deduplication_benefit,
);
criterion_main!(benches);
