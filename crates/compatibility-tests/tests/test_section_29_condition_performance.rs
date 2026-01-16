mod common;

use anyhow::Result;
use common::{create_authorization_model, create_test_store, get_openfga_url, shared_client};
use serde_json::json;
use std::time::Instant;

// ============================================================================
// Milestone 0.8: CEL Condition Compatibility Tests
// Section 29: Condition Performance Baselines
// ============================================================================

/// Helper to create a model with various condition types for performance testing
async fn create_performance_test_model(store_id: &str) -> Result<String> {
    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            {
                "type": "user"
            },
            {
                "type": "document",
                "relations": {
                    "viewer": {
                        "this": {}
                    },
                    "conditional_viewer": {
                        "this": {}
                    }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [
                                {"type": "user"}
                            ]
                        },
                        "conditional_viewer": {
                            "directly_related_user_types": [
                                {
                                    "type": "user",
                                    "condition": "simple_condition"
                                }
                            ]
                        }
                    }
                }
            }
        ],
        "conditions": {
            "simple_condition": {
                "name": "simple_condition",
                "expression": "request.enabled == true",
                "parameters": {
                    "enabled": {
                        "type_name": "TYPE_NAME_BOOL"
                    }
                }
            },
            "complex_condition": {
                "name": "complex_condition",
                "expression": "request.role in request.allowed_roles && request.level >= request.min_level && request.department == request.required_department",
                "parameters": {
                    "role": {
                        "type_name": "TYPE_NAME_STRING"
                    },
                    "allowed_roles": {
                        "type_name": "TYPE_NAME_LIST",
                        "generic_types": [{"type_name": "TYPE_NAME_STRING"}]
                    },
                    "level": {
                        "type_name": "TYPE_NAME_INT"
                    },
                    "min_level": {
                        "type_name": "TYPE_NAME_INT"
                    },
                    "department": {
                        "type_name": "TYPE_NAME_STRING"
                    },
                    "required_department": {
                        "type_name": "TYPE_NAME_STRING"
                    }
                }
            }
        }
    });

    create_authorization_model(store_id, model).await
}

/// Test: Measure check latency with simple condition
/// Baseline measurement for RSFGA comparison
#[tokio::test]
async fn test_measure_check_latency_simple_condition() -> Result<()> {
    let store_id = create_test_store().await?;
    let _model_id = create_performance_test_model(&store_id).await?;
    let client = shared_client();

    // Write tuple with simple condition
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:perf-alice",
                    "relation": "conditional_viewer",
                    "object": "document:perf-doc1",
                    "condition": {
                        "name": "simple_condition"
                    }
                }
            ]
        }
    });

    client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&write_request)
        .send()
        .await?;

    // Warm up
    let check_request = json!({
        "tuple_key": {
            "user": "user:perf-alice",
            "relation": "conditional_viewer",
            "object": "document:perf-doc1"
        },
        "context": {
            "enabled": true
        }
    });

    for _ in 0..5 {
        client
            .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
            .json(&check_request)
            .send()
            .await?;
    }

    // Measure latency over 20 iterations
    let iterations = 20;
    let mut latencies = Vec::with_capacity(iterations);

    for _ in 0..iterations {
        let start = Instant::now();
        let response = client
            .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
            .json(&check_request)
            .send()
            .await?;
        let elapsed = start.elapsed();
        assert!(response.status().is_success());
        latencies.push(elapsed.as_micros() as f64);
    }

    // Calculate statistics
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let avg = latencies.iter().sum::<f64>() / iterations as f64;
    let p50 = latencies[iterations / 2];
    let p95 = latencies[(iterations as f64 * 0.95) as usize];
    let p99 = latencies[(iterations as f64 * 0.99) as usize];

    println!("\n=== Simple Condition Check Latency ===");
    println!("Iterations: {}", iterations);
    println!("Average: {:.0}µs", avg);
    println!("P50: {:.0}µs", p50);
    println!("P95: {:.0}µs", p95);
    println!("P99: {:.0}µs", p99);
    println!("======================================\n");

    // Baseline assertion - should be reasonably fast
    assert!(
        avg < 100_000.0, // 100ms average as upper bound
        "Simple condition check should be fast, got avg: {:.0}µs",
        avg
    );

    Ok(())
}

/// Test: Measure check latency with complex condition (multiple operators)
#[tokio::test]
async fn test_measure_check_latency_complex_condition() -> Result<()> {
    let store_id = create_test_store().await?;
    let client = shared_client();

    // Create model with complex condition
    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            {
                "type": "user"
            },
            {
                "type": "resource",
                "relations": {
                    "access": {
                        "this": {}
                    }
                },
                "metadata": {
                    "relations": {
                        "access": {
                            "directly_related_user_types": [
                                {
                                    "type": "user",
                                    "condition": "complex_check"
                                }
                            ]
                        }
                    }
                }
            }
        ],
        "conditions": {
            "complex_check": {
                "name": "complex_check",
                "expression": "request.role in request.allowed_roles && request.level >= request.min_level && (request.department == 'engineering' || request.is_admin == true)",
                "parameters": {
                    "role": {"type_name": "TYPE_NAME_STRING"},
                    "allowed_roles": {"type_name": "TYPE_NAME_LIST", "generic_types": [{"type_name": "TYPE_NAME_STRING"}]},
                    "level": {"type_name": "TYPE_NAME_INT"},
                    "min_level": {"type_name": "TYPE_NAME_INT"},
                    "department": {"type_name": "TYPE_NAME_STRING"},
                    "is_admin": {"type_name": "TYPE_NAME_BOOL"}
                }
            }
        }
    });

    create_authorization_model(&store_id, model).await?;

    // Write tuple with complex condition
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:perf-bob",
                    "relation": "access",
                    "object": "resource:perf-res1",
                    "condition": {
                        "name": "complex_check"
                    }
                }
            ]
        }
    });

    client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&write_request)
        .send()
        .await?;

    // Check with complex context
    let check_request = json!({
        "tuple_key": {
            "user": "user:perf-bob",
            "relation": "access",
            "object": "resource:perf-res1"
        },
        "context": {
            "role": "engineer",
            "allowed_roles": ["admin", "engineer", "manager"],
            "level": 7,
            "min_level": 5,
            "department": "engineering",
            "is_admin": false
        }
    });

    // Warm up
    for _ in 0..5 {
        client
            .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
            .json(&check_request)
            .send()
            .await?;
    }

    // Measure latency
    let iterations = 20;
    let mut latencies = Vec::with_capacity(iterations);

    for _ in 0..iterations {
        let start = Instant::now();
        let response = client
            .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
            .json(&check_request)
            .send()
            .await?;
        let elapsed = start.elapsed();
        assert!(response.status().is_success());
        latencies.push(elapsed.as_micros() as f64);
    }

    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let avg = latencies.iter().sum::<f64>() / iterations as f64;
    let p50 = latencies[iterations / 2];
    let p95 = latencies[(iterations as f64 * 0.95) as usize];
    let p99 = latencies[(iterations as f64 * 0.99) as usize];

    println!("\n=== Complex Condition Check Latency ===");
    println!("Iterations: {}", iterations);
    println!("Average: {:.0}µs", avg);
    println!("P50: {:.0}µs", p50);
    println!("P95: {:.0}µs", p95);
    println!("P99: {:.0}µs", p99);
    println!("=======================================\n");

    // Baseline assertion
    assert!(
        avg < 100_000.0,
        "Complex condition check should be reasonably fast, got avg: {:.0}µs",
        avg
    );

    Ok(())
}

/// Test: Measure batch check with conditions
#[tokio::test]
async fn test_measure_batch_check_with_conditions() -> Result<()> {
    let store_id = create_test_store().await?;
    let _model_id = create_performance_test_model(&store_id).await?;
    let client = shared_client();

    // Write multiple tuples with conditions
    let users: Vec<String> = (0..10).map(|i| format!("user:batch-user-{}", i)).collect();
    let tuple_keys: Vec<serde_json::Value> = users
        .iter()
        .map(|user| {
            json!({
                "user": user,
                "relation": "conditional_viewer",
                "object": "document:batch-doc",
                "condition": {
                    "name": "simple_condition"
                }
            })
        })
        .collect();

    let write_request = json!({
        "writes": {
            "tuple_keys": tuple_keys
        }
    });

    client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&write_request)
        .send()
        .await?;

    // Create batch check request
    let checks: Vec<serde_json::Value> = users
        .iter()
        .enumerate()
        .map(|(i, user)| {
            json!({
                "tuple_key": {
                    "user": user,
                    "relation": "conditional_viewer",
                    "object": "document:batch-doc"
                },
                "context": {
                    "enabled": true
                },
                "correlation_id": format!("check-{}", i)
            })
        })
        .collect();

    let batch_request = json!({
        "checks": checks
    });

    // Warm up
    for _ in 0..3 {
        client
            .post(format!(
                "{}/stores/{}/batch-check",
                get_openfga_url(),
                store_id
            ))
            .json(&batch_request)
            .send()
            .await?;
    }

    // Measure latency
    let iterations = 10;
    let mut latencies = Vec::with_capacity(iterations);

    for _ in 0..iterations {
        let start = Instant::now();
        let response = client
            .post(format!(
                "{}/stores/{}/batch-check",
                get_openfga_url(),
                store_id
            ))
            .json(&batch_request)
            .send()
            .await?;
        let elapsed = start.elapsed();
        assert!(response.status().is_success());
        latencies.push(elapsed.as_micros() as f64);
    }

    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let avg = latencies.iter().sum::<f64>() / iterations as f64;
    let p50 = latencies[iterations / 2];
    let checks_per_batch = 10;
    let avg_per_check = avg / checks_per_batch as f64;

    println!("\n=== Batch Check with Conditions Latency ===");
    println!("Batch size: {} checks", checks_per_batch);
    println!("Iterations: {}", iterations);
    println!("Average (total): {:.0}µs", avg);
    println!("Average (per check): {:.0}µs", avg_per_check);
    println!("P50 (total): {:.0}µs", p50);
    println!("===========================================\n");

    // Baseline assertion
    assert!(
        avg < 500_000.0, // 500ms for batch of 10
        "Batch check with conditions should be reasonably fast, got avg: {:.0}µs",
        avg
    );

    Ok(())
}

/// Test: Compare conditional vs non-conditional check latency
#[tokio::test]
async fn test_compare_conditional_vs_non_conditional_latency() -> Result<()> {
    let store_id = create_test_store().await?;
    let _model_id = create_performance_test_model(&store_id).await?;
    let client = shared_client();

    // Write both conditional and non-conditional tuples
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:compare-user",
                    "relation": "viewer",
                    "object": "document:non-conditional"
                },
                {
                    "user": "user:compare-user",
                    "relation": "conditional_viewer",
                    "object": "document:conditional",
                    "condition": {
                        "name": "simple_condition"
                    }
                }
            ]
        }
    });

    client
        .post(format!("{}/stores/{}/write", get_openfga_url(), store_id))
        .json(&write_request)
        .send()
        .await?;

    let iterations = 20;

    // Measure non-conditional check
    let non_conditional_request = json!({
        "tuple_key": {
            "user": "user:compare-user",
            "relation": "viewer",
            "object": "document:non-conditional"
        }
    });

    // Warm up
    for _ in 0..5 {
        client
            .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
            .json(&non_conditional_request)
            .send()
            .await?;
    }

    let mut non_conditional_latencies = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        let response = client
            .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
            .json(&non_conditional_request)
            .send()
            .await?;
        let elapsed = start.elapsed();
        assert!(response.status().is_success());
        non_conditional_latencies.push(elapsed.as_micros() as f64);
    }

    // Measure conditional check
    let conditional_request = json!({
        "tuple_key": {
            "user": "user:compare-user",
            "relation": "conditional_viewer",
            "object": "document:conditional"
        },
        "context": {
            "enabled": true
        }
    });

    // Warm up
    for _ in 0..5 {
        client
            .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
            .json(&conditional_request)
            .send()
            .await?;
    }

    let mut conditional_latencies = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        let response = client
            .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
            .json(&conditional_request)
            .send()
            .await?;
        let elapsed = start.elapsed();
        assert!(response.status().is_success());
        conditional_latencies.push(elapsed.as_micros() as f64);
    }

    // Calculate statistics
    non_conditional_latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    conditional_latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let non_cond_avg = non_conditional_latencies.iter().sum::<f64>() / iterations as f64;
    let cond_avg = conditional_latencies.iter().sum::<f64>() / iterations as f64;
    let overhead = ((cond_avg - non_cond_avg) / non_cond_avg * 100.0).max(0.0);

    println!("\n=== Conditional vs Non-Conditional Check Comparison ===");
    println!("Iterations: {}", iterations);
    println!("Non-conditional average: {:.0}µs", non_cond_avg);
    println!("Conditional average: {:.0}µs", cond_avg);
    println!("Condition overhead: {:.1}%", overhead);
    println!("======================================================\n");

    // Document the overhead - CEL evaluation adds some latency
    // This is expected and acceptable for the flexibility it provides
    assert!(
        cond_avg < 100_000.0,
        "Conditional check should still be fast, got avg: {:.0}µs",
        cond_avg
    );

    Ok(())
}
