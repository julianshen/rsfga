//! Performance benchmarks for precompute pipeline components.
//!
//! Run with: cargo bench -p rsfga-precompute --bench precompute_components_bench
//!
//! Benchmarks:
//! - `classifier`: Event classification throughput (events/sec)
//! - `classifier_scaling`: Classification with 1/10/100/1000 tuple writes
//! - `impact_deps`: Relation dependency graph build time
//! - `impact_deps_scaling`: Dependency build with varying model complexity

use std::collections::HashMap;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use rsfga_nats::{CommittedEvent, TupleOperation};
use rsfga_precompute::classifier::{classify, ChangeType};
use rsfga_precompute::impact::build_relation_dependencies;

// =============================================================================
// Helpers
// =============================================================================

/// Create a CommittedEvent with N unique tuple writes across distinct
/// (object_type, relation) pairs.
fn event_with_writes(n: usize) -> CommittedEvent {
    let writes: Vec<TupleOperation> = (0..n)
        .map(|i| {
            TupleOperation::new(
                format!("user:user_{i}"),
                format!("rel_{}", i % 5),
                format!("type_{}:obj_{i}", i % 3),
            )
        })
        .collect();
    CommittedEvent::new("bench-store", 1).with_writes(writes)
}

/// Create a CommittedEvent with a model change flag set.
fn event_model_change() -> CommittedEvent {
    CommittedEvent::new("bench-store", 1).with_model_changed()
}

/// Build a flat type definition map: one type with `n` independent relations.
fn flat_type_defs(n: usize) -> HashMap<String, HashMap<String, Vec<String>>> {
    let mut type_defs = HashMap::new();
    let mut rels = HashMap::new();
    for i in 0..n {
        rels.insert(format!("rel_{i}"), vec![]);
    }
    type_defs.insert("document".to_string(), rels);
    type_defs
}

/// Build a chain type definition map: rel_0 → rel_1 → ... → rel_{n-1}.
/// Each relation references the next, forming a linear dependency chain.
fn chain_type_defs(n: usize) -> HashMap<String, HashMap<String, Vec<String>>> {
    let mut type_defs = HashMap::new();
    let mut rels = HashMap::new();
    for i in 0..n {
        let refs = if i + 1 < n {
            vec![format!("rel_{}", i + 1)]
        } else {
            vec![]
        };
        rels.insert(format!("rel_{i}"), refs);
    }
    type_defs.insert("document".to_string(), rels);
    type_defs
}

/// Build a diamond type definition map:
/// `viewer` → [editor, commenter], `editor` → [owner], `commenter` → [owner],
/// plus (n - 4) additional leaf relations.
fn diamond_type_defs(n: usize) -> HashMap<String, HashMap<String, Vec<String>>> {
    let mut type_defs = HashMap::new();
    let mut rels = HashMap::new();
    rels.insert(
        "viewer".to_string(),
        vec!["editor".to_string(), "commenter".to_string()],
    );
    rels.insert("editor".to_string(), vec!["owner".to_string()]);
    rels.insert("commenter".to_string(), vec!["owner".to_string()]);
    rels.insert("owner".to_string(), vec![]);
    // Add extra leaf relations to reach target count
    for i in 4..n {
        rels.insert(format!("extra_rel_{i}"), vec![]);
    }
    type_defs.insert("document".to_string(), rels);
    type_defs
}

// =============================================================================
// Benchmarks
// =============================================================================

fn bench_classifier(c: &mut Criterion) {
    let mut group = c.benchmark_group("classifier");

    // Single tuple write classification
    let event_1 = event_with_writes(1);
    group.throughput(Throughput::Elements(1));
    group.bench_function("classify_1_write", |b| {
        b.iter(|| classify(black_box(&event_1)))
    });

    // Model change classification
    let model_event = event_model_change();
    group.bench_function("classify_model_change", |b| {
        b.iter(|| classify(black_box(&model_event)))
    });

    group.finish();
}

fn bench_classifier_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("classifier_scaling");

    for &n in &[1, 10, 100, 1000] {
        let event = event_with_writes(n);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::new("classify", n), &event, |b, event| {
            b.iter(|| classify(black_box(event)))
        });
    }

    group.finish();
}

fn bench_impact_deps(c: &mut Criterion) {
    let mut group = c.benchmark_group("impact_deps");

    // Flat: 20 independent relations (no dependencies)
    let flat = flat_type_defs(20);
    group.bench_function("flat_20_relations", |b| {
        b.iter(|| build_relation_dependencies(black_box(&flat)))
    });

    // Chain: 10 relations in a linear chain
    let chain = chain_type_defs(10);
    group.bench_function("chain_10_relations", |b| {
        b.iter(|| build_relation_dependencies(black_box(&chain)))
    });

    // Diamond: 4-node diamond pattern
    let diamond = diamond_type_defs(4);
    group.bench_function("diamond_4_relations", |b| {
        b.iter(|| build_relation_dependencies(black_box(&diamond)))
    });

    group.finish();
}

fn bench_impact_deps_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("impact_deps_scaling");

    // Flat scaling: 5, 20, 50 relations
    for &n in &[5, 20, 50] {
        let defs = flat_type_defs(n);
        group.bench_with_input(BenchmarkId::new("flat", n), &defs, |b, defs| {
            b.iter(|| build_relation_dependencies(black_box(defs)))
        });
    }

    // Chain scaling: 5, 20, 50 relations
    for &n in &[5, 20, 50] {
        let defs = chain_type_defs(n);
        group.bench_with_input(BenchmarkId::new("chain", n), &defs, |b, defs| {
            b.iter(|| build_relation_dependencies(black_box(defs)))
        });
    }

    // Diamond scaling: 5, 20, 50 relations (diamond base + extra leaves)
    for &n in &[5, 20, 50] {
        let defs = diamond_type_defs(n);
        group.bench_with_input(BenchmarkId::new("diamond", n), &defs, |b, defs| {
            b.iter(|| build_relation_dependencies(black_box(defs)))
        });
    }

    group.finish();
}

// Verify that classify produces expected output shapes (sanity check in bench)
fn bench_classifier_result_verification(c: &mut Criterion) {
    let mut group = c.benchmark_group("classifier_verify");

    // Mixed event: model change + 50 tuple writes
    let mixed = CommittedEvent::new("bench-store", 1)
        .with_model_changed()
        .with_writes(
            (0..50)
                .map(|i| {
                    TupleOperation::new(
                        format!("user:user_{i}"),
                        format!("rel_{}", i % 3),
                        format!("doc:doc_{i}"),
                    )
                })
                .collect(),
        );

    // Sanity check: verify the event produces expected output before benchmarking
    let sanity = classify(&mixed);
    assert!(!sanity.is_empty());
    assert!(sanity
        .iter()
        .any(|c| matches!(c, ChangeType::ModelChange { .. })));

    group.throughput(Throughput::Elements(1));
    group.bench_function("classify_mixed_model_and_50_writes", |b| {
        b.iter(|| classify(black_box(&mixed)))
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_classifier,
    bench_classifier_scaling,
    bench_impact_deps,
    bench_impact_deps_scaling,
    bench_classifier_result_verification,
);
criterion_main!(benches);
