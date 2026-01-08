mod common;

use anyhow::Result;
use common::{create_authorization_model, create_test_store, get_openfga_url, shared_client};
use reqwest::StatusCode;
use serde_json::json;

// ============================================================================
// Milestone 0.8: CEL Condition Compatibility Tests
// Section 24: Authorization Model with Conditions
// ============================================================================

/// Test: Can create model with condition definitions
#[tokio::test]
async fn test_can_create_model_with_conditions() -> Result<()> {
    let store_id = create_test_store().await?;

    // Create model with a simple condition
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
                                    "condition": "valid_time"
                                }
                            ]
                        }
                    }
                }
            }
        ],
        "conditions": {
            "valid_time": {
                "name": "valid_time",
                "expression": "request.current_time < request.expires_at",
                "parameters": {
                    "current_time": {
                        "type_name": "TYPE_NAME_TIMESTAMP"
                    },
                    "expires_at": {
                        "type_name": "TYPE_NAME_TIMESTAMP"
                    }
                }
            }
        }
    });

    let model_id = create_authorization_model(&store_id, model).await?;

    assert!(
        !model_id.is_empty(),
        "Model with conditions should be created successfully"
    );

    Ok(())
}

/// Test: Condition has name, expression, and parameters
#[tokio::test]
async fn test_condition_structure_name_expression_parameters() -> Result<()> {
    let store_id = create_test_store().await?;
    let client = shared_client();

    // Create model with fully specified condition
    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            {
                "type": "user"
            },
            {
                "type": "resource",
                "relations": {
                    "reader": {
                        "this": {}
                    }
                },
                "metadata": {
                    "relations": {
                        "reader": {
                            "directly_related_user_types": [
                                {
                                    "type": "user",
                                    "condition": "ip_check"
                                }
                            ]
                        }
                    }
                }
            }
        ],
        "conditions": {
            "ip_check": {
                "name": "ip_check",
                "expression": "request.client_ip in request.allowed_ips",
                "parameters": {
                    "client_ip": {
                        "type_name": "TYPE_NAME_STRING"
                    },
                    "allowed_ips": {
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

    let model_id = create_authorization_model(&store_id, model).await?;

    // Retrieve the model and verify condition structure
    let response = client
        .get(format!(
            "{}/stores/{}/authorization-models/{}",
            get_openfga_url(),
            store_id,
            model_id
        ))
        .send()
        .await?;

    assert!(response.status().is_success());

    let body: serde_json::Value = response.json().await?;
    let model = &body["authorization_model"];

    // Verify conditions are returned
    let conditions = &model["conditions"];
    assert!(
        conditions.is_object(),
        "Conditions should be present in retrieved model"
    );

    let ip_check = &conditions["ip_check"];
    assert_eq!(
        ip_check["name"].as_str(),
        Some("ip_check"),
        "Condition should have name"
    );
    assert!(
        ip_check["expression"].is_string(),
        "Condition should have expression"
    );
    assert!(
        ip_check["parameters"].is_object(),
        "Condition should have parameters"
    );

    Ok(())
}

/// Test: Condition expression uses CEL syntax
#[tokio::test]
async fn test_condition_uses_cel_syntax() -> Result<()> {
    let store_id = create_test_store().await?;

    // Create model with various CEL syntax elements
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
                "expression": "request.role == 'admin' && request.level >= 5 && request.department in ['engineering', 'product']",
                "parameters": {
                    "role": {
                        "type_name": "TYPE_NAME_STRING"
                    },
                    "level": {
                        "type_name": "TYPE_NAME_INT"
                    },
                    "department": {
                        "type_name": "TYPE_NAME_STRING"
                    }
                }
            }
        }
    });

    let model_id = create_authorization_model(&store_id, model).await?;

    assert!(
        !model_id.is_empty(),
        "Model with complex CEL expression should be created"
    );

    Ok(())
}

/// Test: Condition parameters have type definitions (string, int, bool, timestamp, duration, list, map)
#[tokio::test]
async fn test_condition_parameter_types() -> Result<()> {
    let store_id = create_test_store().await?;

    // Create model with various parameter types
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
                                    "condition": "multi_type_check"
                                }
                            ]
                        }
                    }
                }
            }
        ],
        "conditions": {
            "multi_type_check": {
                "name": "multi_type_check",
                "expression": "request.enabled == true && request.count > 0",
                "parameters": {
                    "enabled": {
                        "type_name": "TYPE_NAME_BOOL"
                    },
                    "count": {
                        "type_name": "TYPE_NAME_INT"
                    },
                    "name": {
                        "type_name": "TYPE_NAME_STRING"
                    },
                    "created_at": {
                        "type_name": "TYPE_NAME_TIMESTAMP"
                    },
                    "timeout": {
                        "type_name": "TYPE_NAME_DURATION"
                    },
                    "tags": {
                        "type_name": "TYPE_NAME_LIST",
                        "generic_types": [
                            {
                                "type_name": "TYPE_NAME_STRING"
                            }
                        ]
                    },
                    "metadata": {
                        "type_name": "TYPE_NAME_MAP",
                        "generic_types": [
                            {
                                "type_name": "TYPE_NAME_STRING"
                            },
                            {
                                "type_name": "TYPE_NAME_STRING"
                            }
                        ]
                    }
                }
            }
        }
    });

    let model_id = create_authorization_model(&store_id, model).await?;

    assert!(
        !model_id.is_empty(),
        "Model with various parameter types should be created"
    );

    Ok(())
}

/// Test: Relation can reference condition via directly_related_user_types[].condition
#[tokio::test]
async fn test_relation_references_condition() -> Result<()> {
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
                    "restricted_viewer": {
                        "this": {}
                    }
                },
                "metadata": {
                    "relations": {
                        "restricted_viewer": {
                            "directly_related_user_types": [
                                {
                                    "type": "user",
                                    "condition": "time_restriction"
                                }
                            ]
                        }
                    }
                }
            }
        ],
        "conditions": {
            "time_restriction": {
                "name": "time_restriction",
                "expression": "request.hour >= 9 && request.hour <= 17",
                "parameters": {
                    "hour": {
                        "type_name": "TYPE_NAME_INT"
                    }
                }
            }
        }
    });

    let model_id = create_authorization_model(&store_id, model).await?;

    // Verify the model was created with condition reference
    let response = client
        .get(format!(
            "{}/stores/{}/authorization-models/{}",
            get_openfga_url(),
            store_id,
            model_id
        ))
        .send()
        .await?;

    let body: serde_json::Value = response.json().await?;
    let model = &body["authorization_model"];

    // Navigate to check condition reference in metadata
    let doc_type = model["type_definitions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["type"] == "document")
        .unwrap();

    let user_types =
        &doc_type["metadata"]["relations"]["restricted_viewer"]["directly_related_user_types"];
    let condition_ref = user_types
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["type"] == "user");

    assert!(
        condition_ref.is_some(),
        "Should find user type in directly_related_user_types"
    );
    assert_eq!(
        condition_ref.unwrap()["condition"].as_str(),
        Some("time_restriction"),
        "Relation should reference condition"
    );

    Ok(())
}

/// Test: Invalid condition expression returns 400
#[tokio::test]
async fn test_invalid_condition_expression_returns_400() -> Result<()> {
    let store_id = create_test_store().await?;
    let client = shared_client();

    // Create model with invalid CEL expression (syntax error)
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
                                    "condition": "bad_expr"
                                }
                            ]
                        }
                    }
                }
            }
        ],
        "conditions": {
            "bad_expr": {
                "name": "bad_expr",
                "expression": "this is not valid cel syntax !@#$",
                "parameters": {}
            }
        }
    });

    let response = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model)
        .send()
        .await?;

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "Invalid CEL expression should return 400"
    );

    Ok(())
}

/// Test: Undefined condition reference returns 400
#[tokio::test]
async fn test_undefined_condition_reference_returns_400() -> Result<()> {
    let store_id = create_test_store().await?;
    let client = shared_client();

    // Create model that references a condition that doesn't exist
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
                                    "condition": "nonexistent_condition"
                                }
                            ]
                        }
                    }
                }
            }
        ]
        // Note: no "conditions" block defined
    });

    let response = client
        .post(format!(
            "{}/stores/{}/authorization-models",
            get_openfga_url(),
            store_id
        ))
        .json(&model)
        .send()
        .await?;

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "Undefined condition reference should return 400"
    );

    Ok(())
}

/// Test: Can retrieve model with conditions via GET
#[tokio::test]
async fn test_can_retrieve_model_with_conditions() -> Result<()> {
    let store_id = create_test_store().await?;
    let client = shared_client();

    // Create model with conditions
    let model = json!({
        "schema_version": "1.1",
        "type_definitions": [
            {
                "type": "user"
            },
            {
                "type": "document",
                "relations": {
                    "editor": {
                        "this": {}
                    }
                },
                "metadata": {
                    "relations": {
                        "editor": {
                            "directly_related_user_types": [
                                {
                                    "type": "user",
                                    "condition": "department_match"
                                }
                            ]
                        }
                    }
                }
            }
        ],
        "conditions": {
            "department_match": {
                "name": "department_match",
                "expression": "request.user_dept == request.doc_dept",
                "parameters": {
                    "user_dept": {
                        "type_name": "TYPE_NAME_STRING"
                    },
                    "doc_dept": {
                        "type_name": "TYPE_NAME_STRING"
                    }
                }
            }
        }
    });

    let model_id = create_authorization_model(&store_id, model).await?;

    // GET the model back
    let response = client
        .get(format!(
            "{}/stores/{}/authorization-models/{}",
            get_openfga_url(),
            store_id,
            model_id
        ))
        .send()
        .await?;

    assert!(
        response.status().is_success(),
        "Should be able to retrieve model with conditions"
    );

    let body: serde_json::Value = response.json().await?;
    let returned_model = &body["authorization_model"];

    // Verify conditions are present in response
    assert!(
        returned_model["conditions"].is_object(),
        "Retrieved model should include conditions"
    );

    let dept_match = &returned_model["conditions"]["department_match"];
    assert!(
        dept_match.is_object(),
        "department_match condition should be present"
    );
    assert_eq!(
        dept_match["name"].as_str(),
        Some("department_match"),
        "Condition name should match"
    );
    assert!(
        dept_match["expression"].is_string(),
        "Condition expression should be present"
    );
    assert!(
        dept_match["parameters"].is_object(),
        "Condition parameters should be present"
    );

    Ok(())
}
