//! Performance benchmarks for Valkey key construction overhead.
//!
//! Run with: cargo bench -p rsfga-valkey --bench key_construction_bench
//!
//! Benchmarks:
//! - `check_key_new`: CheckKey::new() — 6 String allocations
//! - `check_key_to_redis_key`: to_redis_key() — format! + percent-encoding
//! - `hotpath_member`: hotpath_member() — format! + percent-encoding
//! - `full_miss_overhead`: All key construction combined (total CPU cost per miss)
//! - `special_chars`: Same operations with inputs containing #/@/% (triggering encoding)

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};

use rsfga_valkey::keys::{hotpath_key, hotpath_member, CheckKey};

// =============================================================================
// Constants — typical real-world field values
// =============================================================================

const STORE_ID: &str = "01HXYZ1234567890ABCDEF";
const MODEL_ID: &str = "01HXYZ9876543210FEDCBA";
const OBJECT_TYPE: &str = "document";
const OBJECT_ID: &str = "readme";
const RELATION: &str = "viewer";
const USER: &str = "user:alice";

// Inputs with special characters that trigger percent-encoding
const OBJECT_ID_SPECIAL: &str = "my#doc@v2%final";
const USER_SPECIAL: &str = "group:eng#member@corp%25";

// =============================================================================
// Benchmarks
// =============================================================================

fn bench_check_key_new(c: &mut Criterion) {
    let mut group = c.benchmark_group("key_construction");
    group.throughput(Throughput::Elements(1));

    group.bench_function("check_key_new", |b| {
        b.iter(|| {
            CheckKey::new(
                black_box(STORE_ID),
                black_box(MODEL_ID),
                black_box(OBJECT_TYPE),
                black_box(OBJECT_ID),
                black_box(RELATION),
                black_box(USER),
            )
        })
    });

    group.finish();
}

fn bench_check_key_to_redis_key(c: &mut Criterion) {
    let key = CheckKey::new(STORE_ID, MODEL_ID, OBJECT_TYPE, OBJECT_ID, RELATION, USER);

    let mut group = c.benchmark_group("key_construction");
    group.throughput(Throughput::Elements(1));

    group.bench_function("check_key_to_redis_key", |b| {
        b.iter(|| black_box(&key).to_redis_key())
    });

    group.finish();
}

fn bench_hotpath_member(c: &mut Criterion) {
    let mut group = c.benchmark_group("key_construction");
    group.throughput(Throughput::Elements(1));

    group.bench_function("hotpath_member", |b| {
        b.iter(|| {
            hotpath_member(
                black_box(OBJECT_TYPE),
                black_box(OBJECT_ID),
                black_box(RELATION),
                black_box(USER),
            )
        })
    });

    group.bench_function("hotpath_key", |b| {
        b.iter(|| hotpath_key(black_box(STORE_ID)))
    });

    group.finish();
}

fn bench_full_miss_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("key_construction");
    group.throughput(Throughput::Elements(1));

    group.bench_function("full_miss_overhead", |b| {
        b.iter(|| {
            // On a cache miss the server constructs both keys:
            // 1. CheckKey for the cache lookup
            let ck = CheckKey::new(
                black_box(STORE_ID),
                black_box(MODEL_ID),
                black_box(OBJECT_TYPE),
                black_box(OBJECT_ID),
                black_box(RELATION),
                black_box(USER),
            );
            let redis_key = ck.to_redis_key();
            // 2. Hot-path member for recording the access pattern
            let hp = hotpath_member(
                black_box(OBJECT_TYPE),
                black_box(OBJECT_ID),
                black_box(RELATION),
                black_box(USER),
            );
            let hk = hotpath_key(black_box(STORE_ID));
            black_box((redis_key, hp, hk))
        })
    });

    group.finish();
}

fn bench_special_chars(c: &mut Criterion) {
    let mut group = c.benchmark_group("key_construction_special");
    group.throughput(Throughput::Elements(1));

    group.bench_function("check_key_new_special", |b| {
        b.iter(|| {
            CheckKey::new(
                black_box(STORE_ID),
                black_box(MODEL_ID),
                black_box(OBJECT_TYPE),
                black_box(OBJECT_ID_SPECIAL),
                black_box(RELATION),
                black_box(USER_SPECIAL),
            )
        })
    });

    let key_special = CheckKey::new(
        STORE_ID,
        MODEL_ID,
        OBJECT_TYPE,
        OBJECT_ID_SPECIAL,
        RELATION,
        USER_SPECIAL,
    );

    group.bench_function("to_redis_key_special", |b| {
        b.iter(|| black_box(&key_special).to_redis_key())
    });

    group.bench_function("hotpath_member_special", |b| {
        b.iter(|| {
            hotpath_member(
                black_box(OBJECT_TYPE),
                black_box(OBJECT_ID_SPECIAL),
                black_box(RELATION),
                black_box(USER_SPECIAL),
            )
        })
    });

    group.bench_function("full_miss_overhead_special", |b| {
        b.iter(|| {
            let ck = CheckKey::new(
                black_box(STORE_ID),
                black_box(MODEL_ID),
                black_box(OBJECT_TYPE),
                black_box(OBJECT_ID_SPECIAL),
                black_box(RELATION),
                black_box(USER_SPECIAL),
            );
            let redis_key = ck.to_redis_key();
            let hp = hotpath_member(
                black_box(OBJECT_TYPE),
                black_box(OBJECT_ID_SPECIAL),
                black_box(RELATION),
                black_box(USER_SPECIAL),
            );
            let hk = hotpath_key(black_box(STORE_ID));
            black_box((redis_key, hp, hk))
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_check_key_new,
    bench_check_key_to_redis_key,
    bench_hotpath_member,
    bench_full_miss_overhead,
    bench_special_chars,
);
criterion_main!(benches);
