//! Storage layer benchmarks.
//!
//! Measures performance of storage operations to:
//! 1. Establish baseline before optimizations
//! 2. Validate O(1) vs O(N) complexity after changes
//! 3. Catch performance regressions
//!
//! Run with: cargo bench -p rsfga-storage

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use rand::Rng;
use rsfga_storage::memory::MemoryDataStore;
use rsfga_storage::traits::{DataStore, PaginationOptions, StoredTuple, TupleFilter};
use std::sync::Arc;
use tokio::runtime::Runtime;

/// Creates a store pre-populated with N tuples for benchmarking reads.
async fn setup_store_with_tuples(n: usize) -> Arc<MemoryDataStore> {
    let store = Arc::new(MemoryDataStore::new());
    store.create_store("bench-store", "Benchmark").await.unwrap();

    // Write tuples in batches to avoid O(N^2) setup time
    let batch_size = 1000;
    for batch_start in (0..n).step_by(batch_size) {
        let batch_end = (batch_start + batch_size).min(n);
        let tuples: Vec<StoredTuple> = (batch_start..batch_end)
            .map(|i| StoredTuple::new("document", format!("doc{i}"), "viewer", "user", format!("user{i}"), None))
            .collect();
        store.write_tuples("bench-store", tuples, vec![]).await.unwrap();
    }

    store
}

/// Benchmark: Write single tuple to store with N existing tuples.
/// Current implementation is O(N) due to HashSet creation for duplicate check.
fn bench_write_single_tuple(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("write_single_tuple");
    group.sample_size(50);

    for n in [100, 1_000, 10_000] {
        group.throughput(Throughput::Elements(1));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let store = rt.block_on(setup_store_with_tuples(n));
            let mut counter = n;

            b.iter(|| {
                counter += 1;
                let tuple = StoredTuple::new(
                    "document",
                    format!("new_doc{counter}"),
                    "viewer",
                    "user",
                    format!("new_user{counter}"),
                    None,
                );
                rt.block_on(store.write_tuple("bench-store", tuple)).unwrap();
            });
        });
    }
    group.finish();
}

/// Benchmark: Write batch of 100 tuples to store with N existing tuples.
fn bench_write_batch(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("write_batch_100");
    group.sample_size(30);

    for n in [100, 1_000, 10_000] {
        group.throughput(Throughput::Elements(100));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let store = rt.block_on(setup_store_with_tuples(n));
            let mut counter = n;

            b.iter(|| {
                let batch: Vec<StoredTuple> = (0..100)
                    .map(|i| {
                        counter += 1;
                        StoredTuple::new(
                            "document",
                            format!("batch_doc{counter}_{i}"),
                            "viewer",
                            "user",
                            format!("batch_user{counter}_{i}"),
                            None,
                        )
                    })
                    .collect();
                rt.block_on(store.write_tuples("bench-store", batch, vec![])).unwrap();
            });
        });
    }
    group.finish();
}

/// Benchmark: Read tuples with object_type filter from store with N tuples.
fn bench_read_by_object_type(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("read_by_object_type");
    group.sample_size(100);

    for n in [100, 1_000, 10_000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let store = rt.block_on(setup_store_with_tuples(n));

            let filter = TupleFilter {
                object_type: Some("document".to_string()),
                ..Default::default()
            };

            b.iter(|| {
                let result = rt.block_on(store.read_tuples("bench-store", black_box(&filter))).unwrap();
                black_box(result);
            });
        });
    }
    group.finish();
}

/// Benchmark: Read tuples with specific object filter from store with N tuples.
/// This should ideally be O(1) with proper indexing.
fn bench_read_specific_object(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("read_specific_object");
    group.sample_size(100);

    for n in [100, 1_000, 10_000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let store = rt.block_on(setup_store_with_tuples(n));
            let mut rng = rand::thread_rng();

            b.iter(|| {
                // Random object lookup
                let idx = rng.gen_range(0..n);
                let filter = TupleFilter {
                    object_type: Some("document".to_string()),
                    object_id: Some(format!("doc{idx}")),
                    ..Default::default()
                };
                let result = rt.block_on(store.read_tuples("bench-store", black_box(&filter))).unwrap();
                black_box(result);
            });
        });
    }
    group.finish();
}

/// Benchmark: Read tuples with user filter from store with N tuples.
fn bench_read_by_user(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("read_by_user");
    group.sample_size(100);

    for n in [100, 1_000, 10_000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let store = rt.block_on(setup_store_with_tuples(n));
            let mut rng = rand::thread_rng();

            b.iter(|| {
                let idx = rng.gen_range(0..n);
                let filter = TupleFilter {
                    user: Some(format!("user:user{idx}")),
                    ..Default::default()
                };
                let result = rt.block_on(store.read_tuples("bench-store", black_box(&filter))).unwrap();
                black_box(result);
            });
        });
    }
    group.finish();
}

/// Benchmark: Delete single tuple from store with N tuples.
/// Current implementation is O(N) due to linear scan.
fn bench_delete_single_tuple(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("delete_single_tuple");
    group.sample_size(50);

    for n in [100, 1_000, 10_000] {
        group.throughput(Throughput::Elements(1));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            // Setup fresh store for each benchmark run
            b.iter_custom(|iters| {
                let mut total_time = std::time::Duration::ZERO;

                for _ in 0..iters {
                    // Fresh store for each iteration
                    let store = rt.block_on(setup_store_with_tuples(n));
                    let mut rng = rand::thread_rng();
                    let idx = rng.gen_range(0..n);

                    let tuple = StoredTuple::new(
                        "document",
                        format!("doc{idx}"),
                        "viewer",
                        "user",
                        format!("user{idx}"),
                        None,
                    );

                    let start = std::time::Instant::now();
                    rt.block_on(store.delete_tuple("bench-store", tuple)).unwrap();
                    total_time += start.elapsed();
                }

                total_time
            });
        });
    }
    group.finish();
}

/// Benchmark: Paginated read with page size 100 from store with N tuples.
fn bench_read_paginated(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("read_paginated_100");
    group.sample_size(100);

    for n in [100, 1_000, 10_000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let store = rt.block_on(setup_store_with_tuples(n));

            let filter = TupleFilter::default();
            let pagination = PaginationOptions {
                page_size: Some(100),
                continuation_token: None,
            };

            b.iter(|| {
                let result = rt.block_on(store.read_tuples_paginated(
                    "bench-store",
                    black_box(&filter),
                    black_box(&pagination),
                ))
                .unwrap();
                black_box(result);
            });
        });
    }
    group.finish();
}

/// Benchmark: Check if tuple exists (write duplicate).
/// Tests the duplicate detection path which is O(N) in current impl.
fn bench_write_duplicate_check(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("write_duplicate_check");
    group.sample_size(100);

    for n in [100, 1_000, 10_000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let store = rt.block_on(setup_store_with_tuples(n));

            // Try to write a tuple that already exists
            let tuple = StoredTuple::new("document", "doc0", "viewer", "user", "user0", None);

            b.iter(|| {
                rt.block_on(store.write_tuple("bench-store", black_box(tuple.clone()))).unwrap();
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_write_single_tuple,
    bench_write_batch,
    bench_read_by_object_type,
    bench_read_specific_object,
    bench_read_by_user,
    bench_delete_single_tuple,
    bench_read_paginated,
    bench_write_duplicate_check,
);

criterion_main!(benches);
