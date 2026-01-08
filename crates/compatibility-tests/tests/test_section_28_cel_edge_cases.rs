mod common;

use anyhow::Result;
use common::{create_authorization_model, create_test_store, get_openfga_url, shared_client};
use serde_json::json;

// ============================================================================
// Milestone 0.8: CEL Condition Compatibility Tests
// Section 28: CEL Expression Edge Cases
// ============================================================================

/// Test: Empty context with no required params succeeds
#[tokio::test]
async fn test_empty_context_no_required_params() -> Result<()> {
    let store_id = create_test_store().await?;
    let client = shared_client();

    // Create model with condition that doesn't require any params
    // Expression always evaluates to true
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
                    }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [
                                {
                                    "type": "user",
                                    "condition": "always_true"
                                }
                            ]
                        }
                    }
                }
            }
        ],
        "conditions": {
            "always_true": {
                "name": "always_true",
                "expression": "true",
                "parameters": {}
            }
        }
    });

    create_authorization_model(&store_id, model).await?;

    // Write tuple with condition
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:alice",
                    "relation": "viewer",
                    "object": "document:doc1",
                    "condition": {
                        "name": "always_true"
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

    // Check without context (should succeed since no params required)
    let check_request = json!({
        "tuple_key": {
            "user": "user:alice",
            "relation": "viewer",
            "object": "document:doc1"
        }
        // No context provided
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    let body: serde_json::Value = response.json().await?;

    // Should succeed with empty context when no params required
    assert!(
        response.status().is_success() && body["allowed"].as_bool() == Some(true),
        "Check with no required params should return allowed: true"
    );

    Ok(())
}

/// Test: Null values in context handled correctly
#[tokio::test]
async fn test_null_values_in_context() -> Result<()> {
    let store_id = create_test_store().await?;
    let client = shared_client();

    // Create model with nullable condition check
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
                    }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [
                                {
                                    "type": "user",
                                    "condition": "nullable_check"
                                }
                            ]
                        }
                    }
                }
            }
        ],
        "conditions": {
            "nullable_check": {
                "name": "nullable_check",
                "expression": "request.value == 'expected'",
                "parameters": {
                    "value": {
                        "type_name": "TYPE_NAME_STRING"
                    }
                }
            }
        }
    });

    create_authorization_model(&store_id, model).await?;

    // Write tuple with condition
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:bob",
                    "relation": "viewer",
                    "object": "document:doc2",
                    "condition": {
                        "name": "nullable_check"
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

    // Check with null value in context
    let check_request = json!({
        "tuple_key": {
            "user": "user:bob",
            "relation": "viewer",
            "object": "document:doc2"
        },
        "context": {
            "value": null
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    // Document behavior - null handling may vary
    let status = response.status();
    let body: serde_json::Value = response.json().await?;

    // Null should either cause error, return false, or be handled gracefully
    assert!(
        status.is_success() || status.as_u16() == 400,
        "Null value in context should be handled, got status: {}, body: {:?}",
        status,
        body
    );

    Ok(())
}

/// Test: Very long string values in context
#[tokio::test]
async fn test_very_long_string_values_in_context() -> Result<()> {
    let store_id = create_test_store().await?;
    let client = shared_client();

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
                    }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [
                                {
                                    "type": "user",
                                    "condition": "string_check"
                                }
                            ]
                        }
                    }
                }
            }
        ],
        "conditions": {
            "string_check": {
                "name": "string_check",
                "expression": "request.data.size() > 0",
                "parameters": {
                    "data": {
                        "type_name": "TYPE_NAME_STRING"
                    }
                }
            }
        }
    });

    create_authorization_model(&store_id, model).await?;

    // Write tuple
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:charlie",
                    "relation": "viewer",
                    "object": "document:doc3",
                    "condition": {
                        "name": "string_check"
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

    // Create a very long string (10KB)
    let long_string = "a".repeat(10 * 1024);

    let check_request = json!({
        "tuple_key": {
            "user": "user:charlie",
            "relation": "viewer",
            "object": "document:doc3"
        },
        "context": {
            "data": long_string
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    // Long strings should be handled (either succeed or fail gracefully)
    let status = response.status();
    assert!(
        status.is_success() || status.as_u16() == 400,
        "Very long string should be handled gracefully, got: {}",
        status
    );

    Ok(())
}

/// Test: Large list values in context
#[tokio::test]
async fn test_large_list_values_in_context() -> Result<()> {
    let store_id = create_test_store().await?;
    let client = shared_client();

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
                    }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [
                                {
                                    "type": "user",
                                    "condition": "list_check"
                                }
                            ]
                        }
                    }
                }
            }
        ],
        "conditions": {
            "list_check": {
                "name": "list_check",
                "expression": "'target' in request.items",
                "parameters": {
                    "items": {
                        "type_name": "TYPE_NAME_LIST",
                        "generic_types": [
                            {
                                "type_name": "TYPE_NAME_STRING"
                            }
                        ]
                    }
                }
            }
        }
    });

    create_authorization_model(&store_id, model).await?;

    // Write tuple
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:dave",
                    "relation": "viewer",
                    "object": "document:doc4",
                    "condition": {
                        "name": "list_check"
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

    // Create large list (1000 items)
    let large_list: Vec<String> = (0..1000).map(|i| format!("item-{}", i)).collect();
    // Add target to the list
    let mut items = large_list;
    items.push("target".to_string());

    let check_request = json!({
        "tuple_key": {
            "user": "user:dave",
            "relation": "viewer",
            "object": "document:doc4"
        },
        "context": {
            "items": items
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    let status = response.status();
    assert!(
        status.is_success() || status.as_u16() == 400,
        "Large list should be handled gracefully, got: {}",
        status
    );

    if status.is_success() {
        let body: serde_json::Value = response.json().await?;
        assert_eq!(
            body["allowed"].as_bool(),
            Some(true),
            "Should find target in large list"
        );
    }

    Ok(())
}

/// Test: Deeply nested map values in context
#[tokio::test]
async fn test_deeply_nested_map_values_in_context() -> Result<()> {
    let store_id = create_test_store().await?;
    let client = shared_client();

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
                    }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [
                                {
                                    "type": "user",
                                    "condition": "map_check"
                                }
                            ]
                        }
                    }
                }
            }
        ],
        "conditions": {
            "map_check": {
                "name": "map_check",
                "expression": "request.metadata.level1.level2.value == 'expected'",
                "parameters": {
                    "metadata": {
                        "type_name": "TYPE_NAME_MAP",
                        "generic_types": [
                            {
                                "type_name": "TYPE_NAME_STRING"
                            },
                            {
                                "type_name": "TYPE_NAME_ANY"
                            }
                        ]
                    }
                }
            }
        }
    });

    create_authorization_model(&store_id, model).await?;

    // Write tuple
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:eve",
                    "relation": "viewer",
                    "object": "document:doc5",
                    "condition": {
                        "name": "map_check"
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

    // Check with deeply nested map
    let check_request = json!({
        "tuple_key": {
            "user": "user:eve",
            "relation": "viewer",
            "object": "document:doc5"
        },
        "context": {
            "metadata": {
                "level1": {
                    "level2": {
                        "value": "expected"
                    }
                }
            }
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    // Document behavior - nested map access may or may not be supported
    let status = response.status();
    assert!(
        status.is_success() || status.as_u16() == 400,
        "Nested map should be handled, got: {}",
        status
    );

    Ok(())
}

/// Test: CEL expression timeout behavior (if any)
#[tokio::test]
async fn test_cel_expression_timeout_behavior() -> Result<()> {
    let store_id = create_test_store().await?;
    let client = shared_client();

    // Note: CEL is designed to be non-Turing complete and should not allow infinite loops
    // This test documents behavior for complex but valid expressions
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
                    }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
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
                "expression": "request.items.filter(x, x > 50).size() > 10",
                "parameters": {
                    "items": {
                        "type_name": "TYPE_NAME_LIST",
                        "generic_types": [
                            {
                                "type_name": "TYPE_NAME_INT"
                            }
                        ]
                    }
                }
            }
        }
    });

    create_authorization_model(&store_id, model).await?;

    // Write tuple
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:frank",
                    "relation": "viewer",
                    "object": "document:doc6",
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

    // Check with moderately large list (should complete quickly due to CEL's design)
    let items: Vec<i32> = (1..=200).collect();
    let check_request = json!({
        "tuple_key": {
            "user": "user:frank",
            "relation": "viewer",
            "object": "document:doc6"
        },
        "context": {
            "items": items
        }
    });

    let start = std::time::Instant::now();
    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;
    let elapsed = start.elapsed();

    // CEL should complete quickly (within reasonable time)
    assert!(
        elapsed.as_secs() < 5,
        "CEL expression should complete quickly, took: {:?}",
        elapsed
    );

    let status = response.status();
    assert!(
        status.is_success(),
        "Complex CEL expression should succeed, got: {}",
        status
    );

    Ok(())
}

/// Test: CEL expression with undefined variable access
#[tokio::test]
async fn test_cel_expression_undefined_variable() -> Result<()> {
    let store_id = create_test_store().await?;
    let client = shared_client();

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
                    }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [
                                {
                                    "type": "user",
                                    "condition": "var_check"
                                }
                            ]
                        }
                    }
                }
            }
        ],
        "conditions": {
            "var_check": {
                "name": "var_check",
                "expression": "request.defined_var == 'test'",
                "parameters": {
                    "defined_var": {
                        "type_name": "TYPE_NAME_STRING"
                    }
                }
            }
        }
    });

    create_authorization_model(&store_id, model).await?;

    // Write tuple
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:grace",
                    "relation": "viewer",
                    "object": "document:doc7",
                    "condition": {
                        "name": "var_check"
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

    // Check WITHOUT providing the required variable
    let check_request = json!({
        "tuple_key": {
            "user": "user:grace",
            "relation": "viewer",
            "object": "document:doc7"
        },
        "context": {
            // Note: "defined_var" is NOT provided
            "other_var": "something"
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    let status = response.status();
    let body: serde_json::Value = response.json().await?;

    // Undefined variable should cause error or return false
    assert!(
        status.as_u16() == 400 || body["allowed"].as_bool() == Some(false),
        "Undefined variable access should error or return false, got status: {}, body: {:?}",
        status,
        body
    );

    Ok(())
}

/// Test: CEL expression division by zero
#[tokio::test]
async fn test_cel_expression_division_by_zero() -> Result<()> {
    let store_id = create_test_store().await?;
    let client = shared_client();

    // Create model with division expression
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
                    }
                },
                "metadata": {
                    "relations": {
                        "viewer": {
                            "directly_related_user_types": [
                                {
                                    "type": "user",
                                    "condition": "div_check"
                                }
                            ]
                        }
                    }
                }
            }
        ],
        "conditions": {
            "div_check": {
                "name": "div_check",
                "expression": "(request.numerator / request.denominator) > 1",
                "parameters": {
                    "numerator": {
                        "type_name": "TYPE_NAME_INT"
                    },
                    "denominator": {
                        "type_name": "TYPE_NAME_INT"
                    }
                }
            }
        }
    });

    create_authorization_model(&store_id, model).await?;

    // Write tuple
    let write_request = json!({
        "writes": {
            "tuple_keys": [
                {
                    "user": "user:henry",
                    "relation": "viewer",
                    "object": "document:doc8",
                    "condition": {
                        "name": "div_check"
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

    // Check with division by zero
    let check_request = json!({
        "tuple_key": {
            "user": "user:henry",
            "relation": "viewer",
            "object": "document:doc8"
        },
        "context": {
            "numerator": 10,
            "denominator": 0
        }
    });

    let response = client
        .post(format!("{}/stores/{}/check", get_openfga_url(), store_id))
        .json(&check_request)
        .send()
        .await?;

    let status = response.status();
    let body: serde_json::Value = response.json().await?;

    // Division by zero should be handled gracefully (error or false)
    assert!(
        status.as_u16() == 400
            || status.as_u16() == 500
            || body["allowed"].as_bool() == Some(false),
        "Division by zero should be handled gracefully, got status: {}, body: {:?}",
        status,
        body
    );

    Ok(())
}
