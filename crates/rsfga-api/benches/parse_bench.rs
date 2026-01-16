//! Performance benchmarks for relation parsing.
//!
//! Run with: cargo bench -p rsfga-api
//!
//! These benchmarks measure:
//! - Direct relation parsing throughput
//! - Nested union/intersection parsing performance
//! - Deeply nested structure parsing (stress test)
//! - Type constraint parsing performance

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use serde_json::json;

// Import the actual parse_userset function from rsfga-api
use rsfga_api::adapters::parse_userset;

// =============================================================================
// Helper functions for generating test structures
// =============================================================================

/// Generate a simple direct relation definition
fn simple_direct() -> serde_json::Value {
    json!({
        "this": {}
    })
}

/// Generate a computed userset relation definition
fn simple_computed(relation: &str) -> serde_json::Value {
    json!({
        "computedUserset": {
            "relation": relation
        }
    })
}

/// Generate a tuple-to-userset relation definition
fn simple_ttu(tupleset_relation: &str, computed_relation: &str) -> serde_json::Value {
    json!({
        "tupleToUserset": {
            "tupleset": {
                "relation": tupleset_relation
            },
            "computedUserset": {
                "relation": computed_relation
            }
        }
    })
}

/// Generate a union of n direct relations
fn flat_union(n: usize) -> serde_json::Value {
    let children: Vec<serde_json::Value> = (0..n).map(|_| simple_direct()).collect();
    json!({
        "union": {
            "child": children
        }
    })
}

/// Generate an intersection of n direct relations
fn flat_intersection(n: usize) -> serde_json::Value {
    let children: Vec<serde_json::Value> = (0..n).map(|_| simple_direct()).collect();
    json!({
        "intersection": {
            "child": children
        }
    })
}

/// Generate a nested union structure of given depth
/// Each level contains a union of 2 children: one direct and one nested union
fn nested_union(depth: usize) -> serde_json::Value {
    if depth == 0 {
        simple_direct()
    } else {
        json!({
            "union": {
                "child": [
                    simple_direct(),
                    nested_union(depth - 1)
                ]
            }
        })
    }
}

/// Generate a nested intersection structure of given depth
fn nested_intersection(depth: usize) -> serde_json::Value {
    if depth == 0 {
        simple_direct()
    } else {
        json!({
            "intersection": {
                "child": [
                    simple_direct(),
                    nested_intersection(depth - 1)
                ]
            }
        })
    }
}

/// Generate alternating union/intersection nesting
fn nested_alternating(depth: usize, use_union: bool) -> serde_json::Value {
    if depth == 0 {
        simple_direct()
    } else if use_union {
        json!({
            "union": {
                "child": [
                    simple_direct(),
                    nested_alternating(depth - 1, false)
                ]
            }
        })
    } else {
        json!({
            "intersection": {
                "child": [
                    simple_direct(),
                    nested_alternating(depth - 1, true)
                ]
            }
        })
    }
}

/// Generate a wide union with nested children
fn wide_nested_union(width: usize, depth: usize) -> serde_json::Value {
    let children: Vec<serde_json::Value> = (0..width).map(|_| nested_union(depth)).collect();
    json!({
        "union": {
            "child": children
        }
    })
}

/// Generate a complex model with type constraints
fn model_with_constraints(num_types: usize) -> serde_json::Value {
    let type_restrictions: Vec<serde_json::Value> = (0..num_types)
        .map(|i| {
            json!({
                "type": format!("type_{}", i)
            })
        })
        .collect();

    json!({
        "this": {},
        "metadata": {
            "directly_related_user_types": type_restrictions
        }
    })
}

// =============================================================================
// Benchmarks
// =============================================================================

fn bench_simple_parsing(c: &mut Criterion) {
    let mut group = c.benchmark_group("simple_parsing");

    // Direct relation
    let direct = simple_direct();
    group.bench_function("direct_this", |b| {
        b.iter(|| parse_userset(black_box(&direct), "document", "viewer"))
    });

    // Computed userset
    let computed = simple_computed("editor");
    group.bench_function("computed_userset", |b| {
        b.iter(|| parse_userset(black_box(&computed), "document", "viewer"))
    });

    // Tuple to userset
    let ttu = simple_ttu("parent", "viewer");
    group.bench_function("tuple_to_userset", |b| {
        b.iter(|| parse_userset(black_box(&ttu), "document", "viewer"))
    });

    group.finish();
}

fn bench_flat_structures(c: &mut Criterion) {
    let mut group = c.benchmark_group("flat_structures");

    for size in [2, 5, 10, 20] {
        let union = flat_union(size);
        group.bench_with_input(BenchmarkId::new("union", size), &union, |b, input| {
            b.iter(|| parse_userset(black_box(input), "document", "viewer"))
        });

        let intersection = flat_intersection(size);
        group.bench_with_input(
            BenchmarkId::new("intersection", size),
            &intersection,
            |b, input| b.iter(|| parse_userset(black_box(input), "document", "viewer")),
        );
    }

    group.finish();
}

fn bench_nested_structures(c: &mut Criterion) {
    let mut group = c.benchmark_group("nested_structures");

    // Test increasing nesting depths
    for depth in [1, 5, 10, 15, 20, 24] {
        let nested = nested_union(depth);
        group.bench_with_input(
            BenchmarkId::new("nested_union", depth),
            &nested,
            |b, input| b.iter(|| parse_userset(black_box(input), "document", "viewer")),
        );

        let nested_int = nested_intersection(depth);
        group.bench_with_input(
            BenchmarkId::new("nested_intersection", depth),
            &nested_int,
            |b, input| b.iter(|| parse_userset(black_box(input), "document", "viewer")),
        );

        let alternating = nested_alternating(depth, true);
        group.bench_with_input(
            BenchmarkId::new("alternating_union_intersection", depth),
            &alternating,
            |b, input| b.iter(|| parse_userset(black_box(input), "document", "viewer")),
        );
    }

    group.finish();
}

fn bench_wide_nested(c: &mut Criterion) {
    let mut group = c.benchmark_group("wide_nested");

    // Test combinations of width and depth
    for (width, depth) in [(2, 10), (5, 5), (10, 3), (20, 2)] {
        let wide = wide_nested_union(width, depth);
        group.bench_with_input(
            BenchmarkId::new(format!("{}x{}", width, depth), 1),
            &wide,
            |b, input| b.iter(|| parse_userset(black_box(input), "document", "viewer")),
        );
    }

    group.finish();
}

fn bench_type_constraints(c: &mut Criterion) {
    let mut group = c.benchmark_group("type_constraints");

    for num_types in [1, 5, 10, 20] {
        let model = model_with_constraints(num_types);
        group.bench_with_input(
            BenchmarkId::new("constraints", num_types),
            &model,
            |b, input| b.iter(|| parse_userset(black_box(input), "document", "viewer")),
        );
    }

    group.finish();
}

fn bench_depth_limit(c: &mut Criterion) {
    let mut group = c.benchmark_group("depth_limit");

    // Test at and near the depth limit (25)
    for depth in [20, 22, 24, 25] {
        let nested = nested_union(depth);
        group.bench_with_input(
            BenchmarkId::new("near_limit", depth),
            &nested,
            |b, input| b.iter(|| parse_userset(black_box(input), "document", "viewer")),
        );
    }

    // Also test that exceeding the limit fails quickly
    let over_limit = nested_union(26);
    group.bench_function("over_limit_26", |b| {
        b.iter(|| {
            let result = parse_userset(black_box(&over_limit), "document", "viewer");
            // Should return error, not hang
            assert!(result.is_err());
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_simple_parsing,
    bench_flat_structures,
    bench_nested_structures,
    bench_wide_nested,
    bench_type_constraints,
    bench_depth_limit,
);

criterion_main!(benches);
