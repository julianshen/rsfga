//! RocksDB storage layer benchmarks.
//!
//! Measures performance of RocksDB storage operations to validate targets:
//! - Write: 2,000+ req/s (vs PostgreSQL 459 req/s)
//! - Write latency: <10ms p95 (vs PostgreSQL 33ms p95)
//! - Check (cold): ~3ms (vs ~5ms)
//!
//! Run with: cargo bench -p rsfga-storage --features rocksdb -- rocksdb
//!
//! Prerequisites:
//! - clang or gcc with development headers
//! - On Fedora/RHEL: sudo dnf install clang-devel
//! - On Ubuntu/Debian: sudo apt-get install clang libclang-dev
//! - On macOS: xcode-select --install

#![cfg(feature = "rocksdb")]

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use rand::Rng;
use rsfga_storage::rocksdb::{RocksDBConfig, RocksDBDataStore};
use rsfga_storage::traits::{DataStore, PaginationOptions, StoredTuple, TupleFilter};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::runtime::Runtime;

/// Creates a RocksDB store in a temporary directory, pre-populated with N tuples.
fn setup_rocksdb_store_with_tuples(rt: &Runtime, n: usize) -> (Arc<RocksDBDataStore>, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let config = RocksDBConfig {
        path: temp_dir.path().to_string_lossy().to_string(),
        ..Default::default()
    };
    let store = Arc::new(RocksDBDataStore::new(config).expect("Failed to create RocksDB store"));

    rt.block_on(async {
        store
            .create_store("bench-store", "Benchmark")
            .await
            .unwrap();

        // Write tuples in batches
        let batch_size = 1000;
        for batch_start in (0..n).step_by(batch_size) {
            let batch_end = (batch_start + batch_size).min(n);
            let tuples: Vec<StoredTuple> = (batch_start..batch_end)
                .map(|i| {
                    StoredTuple::new(
                        "document",
                        format!("doc{i}"),
                        "viewer",
                        "user",
                        format!("user{i}"),
                        None,
                    )
                })
                .collect();
            store
                .write_tuples("bench-store", tuples, vec![])
                .await
                .unwrap();
        }
    });

    (store, temp_dir)
}

/// Benchmark: Write single tuple to RocksDB with N existing tuples.
fn bench_rocksdb_write_single_tuple(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("rocksdb_write_single_tuple");
    group.sample_size(50);

    for n in [100, 1_000, 10_000] {
        group.throughput(Throughput::Elements(1));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let (store, _temp_dir) = setup_rocksdb_store_with_tuples(&rt, n);
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
                rt.block_on(store.write_tuple("bench-store", tuple))
                    .unwrap();
            });
        });
    }
    group.finish();
}

/// Benchmark: Write batch of 100 tuples to RocksDB.
fn bench_rocksdb_write_batch(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("rocksdb_write_batch_100");
    group.sample_size(30);

    for n in [100, 1_000, 10_000] {
        group.throughput(Throughput::Elements(100));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let (store, _temp_dir) = setup_rocksdb_store_with_tuples(&rt, n);
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
                rt.block_on(store.write_tuples("bench-store", batch, vec![]))
                    .unwrap();
            });
        });
    }
    group.finish();
}

/// Benchmark: Read tuples by object_type from RocksDB.
fn bench_rocksdb_read_by_object_type(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("rocksdb_read_by_object_type");
    group.sample_size(100);

    for n in [100, 1_000, 10_000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let (store, _temp_dir) = setup_rocksdb_store_with_tuples(&rt, n);

            let filter = TupleFilter {
                object_type: Some("document".to_string()),
                ..Default::default()
            };

            b.iter(|| {
                let result = rt
                    .block_on(store.read_tuples("bench-store", black_box(&filter)))
                    .unwrap();
                black_box(result);
            });
        });
    }
    group.finish();
}

/// Benchmark: Read specific object from RocksDB (point lookup).
fn bench_rocksdb_read_specific_object(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("rocksdb_read_specific_object");
    group.sample_size(100);

    for n in [100, 1_000, 10_000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let (store, _temp_dir) = setup_rocksdb_store_with_tuples(&rt, n);
            let mut rng = rand::thread_rng();

            b.iter(|| {
                let idx = rng.gen_range(0..n);
                let filter = TupleFilter {
                    object_type: Some("document".to_string()),
                    object_id: Some(format!("doc{idx}")),
                    ..Default::default()
                };
                let result = rt
                    .block_on(store.read_tuples("bench-store", black_box(&filter)))
                    .unwrap();
                black_box(result);
            });
        });
    }
    group.finish();
}

/// Benchmark: Read tuples by user from RocksDB.
fn bench_rocksdb_read_by_user(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("rocksdb_read_by_user");
    group.sample_size(100);

    for n in [100, 1_000, 10_000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let (store, _temp_dir) = setup_rocksdb_store_with_tuples(&rt, n);
            let mut rng = rand::thread_rng();

            b.iter(|| {
                let idx = rng.gen_range(0..n);
                let filter = TupleFilter {
                    user: Some(format!("user:user{idx}")),
                    ..Default::default()
                };
                let result = rt
                    .block_on(store.read_tuples("bench-store", black_box(&filter)))
                    .unwrap();
                black_box(result);
            });
        });
    }
    group.finish();
}

/// Benchmark: Delete single tuple from RocksDB.
fn bench_rocksdb_delete_single_tuple(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("rocksdb_delete_single_tuple");
    group.sample_size(50);

    for n in [100, 1_000, 10_000] {
        group.throughput(Throughput::Elements(1));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter_custom(|iters| {
                let mut total_time = std::time::Duration::ZERO;

                for _ in 0..iters {
                    let (store, _temp_dir) = setup_rocksdb_store_with_tuples(&rt, n);
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
                    rt.block_on(store.delete_tuple("bench-store", tuple))
                        .unwrap();
                    total_time += start.elapsed();
                }

                total_time
            });
        });
    }
    group.finish();
}

/// Benchmark: Paginated read from RocksDB.
fn bench_rocksdb_read_paginated(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("rocksdb_read_paginated_100");
    group.sample_size(100);

    for n in [100, 1_000, 10_000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let (store, _temp_dir) = setup_rocksdb_store_with_tuples(&rt, n);

            let filter = TupleFilter::default();
            let pagination = PaginationOptions {
                page_size: Some(100),
                continuation_token: None,
            };

            b.iter(|| {
                let result = rt
                    .block_on(store.read_tuples_paginated(
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

/// Benchmark: High-throughput write test (validates 2,000+ req/s target).
fn bench_rocksdb_write_throughput(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("rocksdb_write_throughput");
    group.sample_size(20);
    group.throughput(Throughput::Elements(1000));

    group.bench_function("1000_writes", |b| {
        b.iter_custom(|iters| {
            let mut total_time = std::time::Duration::ZERO;

            for _ in 0..iters {
                let temp_dir = TempDir::new().unwrap();
                let config = RocksDBConfig {
                    path: temp_dir.path().to_string_lossy().to_string(),
                    ..Default::default()
                };
                let store = RocksDBDataStore::new(config).unwrap();

                rt.block_on(async {
                    store
                        .create_store("bench-store", "Benchmark")
                        .await
                        .unwrap();

                    let start = std::time::Instant::now();

                    // Write 1000 tuples individually
                    for i in 0..1000 {
                        let tuple = StoredTuple::new(
                            "document",
                            format!("doc{i}"),
                            "viewer",
                            "user",
                            format!("user{i}"),
                            None,
                        );
                        store.write_tuple("bench-store", tuple).await.unwrap();
                    }

                    total_time += start.elapsed();
                });
            }

            total_time
        });
    });

    group.finish();
}

/// Benchmark: Batch write throughput (validates high-throughput batch operations).
fn bench_rocksdb_batch_write_throughput(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("rocksdb_batch_write_throughput");
    group.sample_size(20);
    group.throughput(Throughput::Elements(10000));

    group.bench_function("10000_tuples_in_batches", |b| {
        b.iter_custom(|iters| {
            let mut total_time = std::time::Duration::ZERO;

            for _ in 0..iters {
                let temp_dir = TempDir::new().unwrap();
                let config = RocksDBConfig {
                    path: temp_dir.path().to_string_lossy().to_string(),
                    ..Default::default()
                };
                let store = RocksDBDataStore::new(config).unwrap();

                rt.block_on(async {
                    store
                        .create_store("bench-store", "Benchmark")
                        .await
                        .unwrap();

                    let start = std::time::Instant::now();

                    // Write 10,000 tuples in batches of 100
                    for batch_idx in 0..100 {
                        let tuples: Vec<StoredTuple> = (0..100)
                            .map(|i| {
                                let idx = batch_idx * 100 + i;
                                StoredTuple::new(
                                    "document",
                                    format!("doc{idx}"),
                                    "viewer",
                                    "user",
                                    format!("user{idx}"),
                                    None,
                                )
                            })
                            .collect();
                        store
                            .write_tuples("bench-store", tuples, vec![])
                            .await
                            .unwrap();
                    }

                    total_time += start.elapsed();
                });
            }

            total_time
        });
    });

    group.finish();
}

criterion_group!(
    rocksdb_benches,
    bench_rocksdb_write_single_tuple,
    bench_rocksdb_write_batch,
    bench_rocksdb_read_by_object_type,
    bench_rocksdb_read_specific_object,
    bench_rocksdb_read_by_user,
    bench_rocksdb_delete_single_tuple,
    bench_rocksdb_read_paginated,
    bench_rocksdb_write_throughput,
    bench_rocksdb_batch_write_throughput,
);

criterion_main!(rocksdb_benches);
