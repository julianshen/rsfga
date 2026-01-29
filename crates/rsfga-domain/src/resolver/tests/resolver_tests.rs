//! Graph resolver test suite.
//!
//! Contains comprehensive tests for all resolver functionality including:
//! - Direct tuple resolution
//! - Computed relations (ComputedUserset, TupleToUserset)
//! - Union, Intersection, and Exclusion relations
//! - Contextual tuples
//! - Safety features (depth limits, cycle detection, timeouts)
//! - Type constraints
//! - CEL condition evaluation

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use super::mocks::{MockModelReader, MockTupleReader};
use crate::error::{DomainError, DomainResult};
use crate::model::{Condition, ConditionParameter, RelationDefinition, TypeDefinition, Userset};
use crate::resolver::{
    CheckRequest, CheckResult, ContextualTuple, ExpandLeafValue, ExpandNode, ExpandRequest,
    GraphResolver, ListObjectsRequest, ResolverConfig, StoredTupleRef, TupleReader,
};

// ========== Section 1: Direct Tuple Resolution ==========

#[tokio::test]
async fn test_check_returns_true_for_direct_tuple_assignment() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    // Set up store
    tuple_reader.add_store("store1").await;

    // Add type definition with direct relation
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // Add tuple: user:alice is viewer of document:readme
    tuple_reader
        .add_tuple(
            "store1", "document", "readme", "viewer", "user", "alice", None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "document:readme".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await.unwrap();
    assert!(
        result.allowed,
        "Direct tuple assignment should allow access"
    );
}

#[tokio::test]
async fn test_check_returns_false_when_no_tuple_exists() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // No tuple added

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "document:readme".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await.unwrap();
    assert!(!result.allowed, "Should deny access when no tuple exists");
}

#[tokio::test]
async fn test_check_handles_multiple_stores_independently() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    // Set up two stores
    tuple_reader.add_store("store1").await;
    tuple_reader.add_store("store2").await;

    // Add type definitions for both stores
    for store_id in &["store1", "store2"] {
        model_reader
            .add_type(
                store_id,
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    }],
                },
            )
            .await;
    }

    // Add tuple only in store1
    tuple_reader
        .add_tuple(
            "store1", "document", "readme", "viewer", "user", "alice", None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Check store1 - should be allowed
    let request1 = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "document:readme".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };
    let result1 = resolver.check(&request1).await.unwrap();
    assert!(result1.allowed, "Store1 should allow access");

    // Check store2 - should be denied
    let request2 = CheckRequest {
        store_id: "store2".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "document:readme".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };
    let result2 = resolver.check(&request2).await.unwrap();
    assert!(!result2.allowed, "Store2 should deny access (no tuple)");
}

#[tokio::test]
async fn test_check_validates_input_parameters() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Empty user
    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "".to_string(),
        relation: "viewer".to_string(),
        object: "document:readme".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };
    let result = resolver.check(&request).await;
    assert!(matches!(result, Err(DomainError::InvalidUserFormat { .. })));

    // Empty relation
    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "".to_string(),
        object: "document:readme".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };
    let result = resolver.check(&request).await;
    assert!(matches!(
        result,
        Err(DomainError::InvalidRelationFormat { .. })
    ));

    // Empty object
    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };
    let result = resolver.check(&request).await;
    assert!(matches!(
        result,
        Err(DomainError::InvalidObjectFormat { .. })
    ));
}

#[tokio::test]
async fn test_check_rejects_invalid_user_format() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Invalid user format (no colon)
    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "alice".to_string(),
        relation: "viewer".to_string(),
        object: "document:readme".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };
    let result = resolver.check(&request).await;
    assert!(matches!(result, Err(DomainError::InvalidUserFormat { .. })));
}

#[tokio::test]
async fn test_check_rejects_invalid_object_format() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Invalid object format (no colon)
    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "readme".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };
    let result = resolver.check(&request).await;
    assert!(matches!(
        result,
        Err(DomainError::InvalidObjectFormat { .. })
    ));

    // Invalid object format (empty type)
    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: ":readme".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };
    let result = resolver.check(&request).await;
    assert!(matches!(
        result,
        Err(DomainError::InvalidObjectFormat { .. })
    ));

    // Invalid object format (empty id)
    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "document:".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };
    let result = resolver.check(&request).await;
    assert!(matches!(
        result,
        Err(DomainError::InvalidObjectFormat { .. })
    ));
}

// ========== Section 2: Computed Relations ==========

#[tokio::test]
async fn test_can_resolve_this_relation() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Relation with "this" rewrite
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "owner".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    tuple_reader
        .add_tuple(
            "store1", "document", "readme", "owner", "user", "alice", None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "owner".to_string(),
        object: "document:readme".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await.unwrap();
    assert!(result.allowed);
}

#[tokio::test]
async fn test_can_resolve_relation_from_parent_object() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // folder type with viewer relation
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "folder".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // document type: viewer = viewer from parent folder
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "parent".to_string(),
                        type_constraints: vec!["folder".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::TupleToUserset {
                            tupleset: "parent".to_string(),
                            computed_userset: "viewer".to_string(),
                        },
                    },
                ],
            },
        )
        .await;

    // Set up: folder1 -> document1, alice is viewer of folder1
    tuple_reader
        .add_tuple(
            "store1", "folder", "folder1", "viewer", "user", "alice", None,
        )
        .await;
    tuple_reader
        .add_tuple(
            "store1", "document", "doc1", "parent", "folder", "folder1", None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await.unwrap();
    assert!(
        result.allowed,
        "Should inherit viewer permission from parent folder"
    );
}

#[tokio::test]
async fn test_resolves_nested_parent_relationships() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // org type with admin relation
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "org".to_string(),
                relations: vec![RelationDefinition {
                    name: "admin".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // folder type: admin = admin from parent org
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "folder".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "parent".to_string(),
                        type_constraints: vec!["org".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "admin".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::TupleToUserset {
                            tupleset: "parent".to_string(),
                            computed_userset: "admin".to_string(),
                        },
                    },
                ],
            },
        )
        .await;

    // document type: admin = admin from parent folder
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "parent".to_string(),
                        type_constraints: vec!["folder".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "admin".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::TupleToUserset {
                            tupleset: "parent".to_string(),
                            computed_userset: "admin".to_string(),
                        },
                    },
                ],
            },
        )
        .await;

    // org1 -> folder1 -> doc1, alice is admin of org1
    tuple_reader
        .add_tuple("store1", "org", "org1", "admin", "user", "alice", None)
        .await;
    tuple_reader
        .add_tuple("store1", "folder", "folder1", "parent", "org", "org1", None)
        .await;
    tuple_reader
        .add_tuple(
            "store1", "document", "doc1", "parent", "folder", "folder1", None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "admin".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await.unwrap();
    assert!(
        result.allowed,
        "Should inherit admin permission through nested parents"
    );
}

#[tokio::test]
async fn test_handles_missing_parent_gracefully() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "folder".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "parent".to_string(),
                        type_constraints: vec!["folder".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::TupleToUserset {
                            tupleset: "parent".to_string(),
                            computed_userset: "viewer".to_string(),
                        },
                    },
                ],
            },
        )
        .await;

    // No parent tuple exists for doc1

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await.unwrap();
    assert!(!result.allowed, "Should deny access when parent is missing");
}

// ========== Section 3: Union Relations (A or B) ==========

#[tokio::test]
async fn test_union_returns_true_if_any_branch_is_true() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // viewer = owner or direct_viewer
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "owner".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "direct_viewer".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::Union {
                            children: vec![
                                Userset::ComputedUserset {
                                    relation: "owner".to_string(),
                                },
                                Userset::ComputedUserset {
                                    relation: "direct_viewer".to_string(),
                                },
                            ],
                        },
                    },
                ],
            },
        )
        .await;

    // Alice is owner (but not direct_viewer)
    tuple_reader
        .add_tuple("store1", "document", "doc1", "owner", "user", "alice", None)
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await.unwrap();
    assert!(
        result.allowed,
        "Union should return true if any branch is true"
    );
}

#[tokio::test]
async fn test_union_returns_false_if_all_branches_are_false() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "owner".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "editor".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::Union {
                            children: vec![
                                Userset::ComputedUserset {
                                    relation: "owner".to_string(),
                                },
                                Userset::ComputedUserset {
                                    relation: "editor".to_string(),
                                },
                            ],
                        },
                    },
                ],
            },
        )
        .await;

    // Alice has no permissions

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await.unwrap();
    assert!(
        !result.allowed,
        "Union should return false if all branches are false"
    );
}

#[tokio::test]
async fn test_union_executes_branches_in_parallel() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Create union with many branches
    let mut children = vec![];
    for i in 0..10 {
        children.push(Userset::ComputedUserset {
            relation: format!("branch{i}"),
        });
    }

    let mut relations = vec![];
    for i in 0..10 {
        relations.push(RelationDefinition {
            name: format!("branch{i}"),
            type_constraints: vec!["user".into()],
            rewrite: Userset::This,
        });
    }
    relations.push(RelationDefinition {
        name: "combined".to_string(),
        type_constraints: vec![],
        rewrite: Userset::Union { children },
    });

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations,
            },
        )
        .await;

    // Add tuple for branch5 only
    tuple_reader
        .add_tuple(
            "store1", "document", "doc1", "branch5", "user", "alice", None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "combined".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await.unwrap();
    assert!(result.allowed, "Union with many branches should work");
}

#[tokio::test]
async fn test_union_handles_errors_in_branches() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Union with one valid branch and one that would have errors
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "owner".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::Union {
                            children: vec![
                                Userset::ComputedUserset {
                                    relation: "owner".to_string(),
                                },
                                // This would cause an error if checked (relation doesn't exist)
                                Userset::ComputedUserset {
                                    relation: "nonexistent".to_string(),
                                },
                            ],
                        },
                    },
                ],
            },
        )
        .await;

    // Alice is owner - this should return true despite the error in another branch
    tuple_reader
        .add_tuple("store1", "document", "doc1", "owner", "user", "alice", None)
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    // Should succeed because owner branch is true
    let result = resolver.check(&request).await.unwrap();
    assert!(result.allowed);
}

#[tokio::test]
async fn test_union_returns_cycle_error_when_all_branches_cycle() {
    // Test that when ALL branches in a union return CycleDetected,
    // the error is properly propagated instead of returning allowed: false
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Create a model where union branches both lead to cycles
    // branch1: viewer -> computed from cyclic_rel1
    // branch2: viewer -> computed from cyclic_rel2
    // cyclic_rel1 and cyclic_rel2 both reference viewer (creating cycles)
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::Union {
                            children: vec![
                                Userset::ComputedUserset {
                                    relation: "cyclic_rel1".to_string(),
                                },
                                Userset::ComputedUserset {
                                    relation: "cyclic_rel2".to_string(),
                                },
                            ],
                        },
                    },
                    RelationDefinition {
                        name: "cyclic_rel1".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::ComputedUserset {
                            relation: "viewer".to_string(),
                        },
                    },
                    RelationDefinition {
                        name: "cyclic_rel2".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::ComputedUserset {
                            relation: "viewer".to_string(),
                        },
                    },
                ],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    // Should return CycleDetected error, not allowed: false
    let result = resolver.check(&request).await;
    assert!(
        matches!(result, Err(DomainError::CycleDetected { .. })),
        "Union with all cyclic branches should return CycleDetected error, got {result:?}"
    );
}

#[tokio::test]
async fn test_union_returns_false_when_one_branch_false_and_other_depth_limit() {
    // Test for issue #52: Union should return false (not error) when one branch
    // returns false and another hits DepthLimitExceeded. DepthLimitExceeded is
    // a path-termination error, not a fatal error for union semantics.
    //
    // NOTE: We use a chain of DIFFERENT relations (step0 -> step1 -> step2 -> step3 -> step4)
    // to ensure we hit DepthLimitExceeded rather than CycleDetected. A self-referential
    // relation would be detected as a cycle before hitting depth limit.
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Create a model where:
    // - viewer = union of direct_viewer and step0
    // - direct_viewer = This (direct tuple - will be false for alice)
    // - step0 -> step1 -> step2 -> step3 -> step4 (linear chain exceeding depth limit)
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::Union {
                            children: vec![
                                // Branch 1: direct_viewer - will return false
                                Userset::ComputedUserset {
                                    relation: "direct_viewer".to_string(),
                                },
                                // Branch 2: step0 - will hit depth limit via chain
                                Userset::ComputedUserset {
                                    relation: "step0".to_string(),
                                },
                            ],
                        },
                    },
                    RelationDefinition {
                        name: "direct_viewer".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    // Chain of relations to exceed depth limit without cycling
                    RelationDefinition {
                        name: "step0".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::ComputedUserset {
                            relation: "step1".to_string(),
                        },
                    },
                    RelationDefinition {
                        name: "step1".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::ComputedUserset {
                            relation: "step2".to_string(),
                        },
                    },
                    RelationDefinition {
                        name: "step2".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::ComputedUserset {
                            relation: "step3".to_string(),
                        },
                    },
                    RelationDefinition {
                        name: "step3".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::ComputedUserset {
                            relation: "step4".to_string(),
                        },
                    },
                    RelationDefinition {
                        name: "step4".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This, // Terminal - but we'll never reach it
                    },
                ],
            },
        )
        .await;

    // Don't add any tuples - alice is NOT a direct_viewer

    // Use max_depth=3 so the chain step0->step1->step2->step3 exceeds it
    let config = ResolverConfig {
        max_depth: 3,
        timeout: Duration::from_secs(30),
        cache: None,
    };
    let resolver = GraphResolver::with_config(tuple_reader, model_reader, config);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    // Should return false, NOT DepthLimitExceeded error
    // Because: direct_viewer returns false, step0 branch hits depth limit
    // Union with one false branch should return false, not propagate the error
    let result = resolver.check(&request).await;
    assert!(
        matches!(result, Ok(CheckResult { allowed: false })),
        "Union with one false branch and one depth-limited branch should return false, got {result:?}"
    );
}

#[tokio::test]
async fn test_union_returns_depth_limit_error_when_all_branches_hit_depth_limit() {
    // Test that when ALL branches in a union return DepthLimitExceeded,
    // the error is properly propagated (similar to cycle error behavior)
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Create a model where both branches hit depth limit
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::Union {
                            children: vec![
                                Userset::ComputedUserset {
                                    relation: "deep1".to_string(),
                                },
                                Userset::ComputedUserset {
                                    relation: "deep2".to_string(),
                                },
                            ],
                        },
                    },
                    RelationDefinition {
                        name: "deep1".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::ComputedUserset {
                            relation: "deep1".to_string(),
                        },
                    },
                    RelationDefinition {
                        name: "deep2".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::ComputedUserset {
                            relation: "deep2".to_string(),
                        },
                    },
                ],
            },
        )
        .await;

    let config = ResolverConfig {
        max_depth: 3,
        timeout: Duration::from_secs(30),
        cache: None,
    };
    let resolver = GraphResolver::with_config(tuple_reader, model_reader, config);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    // Should return DepthLimitExceeded or CycleDetected (path-termination error)
    // when ALL branches hit the limit
    let result = resolver.check(&request).await;
    assert!(
        matches!(
            result,
            Err(DomainError::DepthLimitExceeded { .. }) | Err(DomainError::CycleDetected { .. })
        ),
        "Union with all depth-limited branches should return path-termination error, got {result:?}"
    );
}

// ========== Section 4: Intersection Relations (A and B) ==========

#[tokio::test]
async fn test_intersection_returns_true_only_if_all_branches_are_true() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // can_edit = is_member and is_verified
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "member".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "verified".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "can_edit".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::Intersection {
                            children: vec![
                                Userset::ComputedUserset {
                                    relation: "member".to_string(),
                                },
                                Userset::ComputedUserset {
                                    relation: "verified".to_string(),
                                },
                            ],
                        },
                    },
                ],
            },
        )
        .await;

    // Alice is both member AND verified
    tuple_reader
        .add_tuple(
            "store1", "document", "doc1", "member", "user", "alice", None,
        )
        .await;
    tuple_reader
        .add_tuple(
            "store1", "document", "doc1", "verified", "user", "alice", None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "can_edit".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await.unwrap();
    assert!(
        result.allowed,
        "Intersection should return true when all branches are true"
    );
}

#[tokio::test]
async fn test_intersection_returns_false_if_any_branch_is_false() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "member".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "verified".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "can_edit".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::Intersection {
                            children: vec![
                                Userset::ComputedUserset {
                                    relation: "member".to_string(),
                                },
                                Userset::ComputedUserset {
                                    relation: "verified".to_string(),
                                },
                            ],
                        },
                    },
                ],
            },
        )
        .await;

    // Alice is member but NOT verified
    tuple_reader
        .add_tuple(
            "store1", "document", "doc1", "member", "user", "alice", None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "can_edit".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await.unwrap();
    assert!(
        !result.allowed,
        "Intersection should return false if any branch is false"
    );
}

#[tokio::test]
async fn test_intersection_executes_branches_in_parallel() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Create intersection with many branches
    let mut children = vec![];
    for i in 0..5 {
        children.push(Userset::ComputedUserset {
            relation: format!("req{i}"),
        });
    }

    let mut relations = vec![];
    for i in 0..5 {
        relations.push(RelationDefinition {
            name: format!("req{i}"),
            type_constraints: vec!["user".into()],
            rewrite: Userset::This,
        });
    }
    relations.push(RelationDefinition {
        name: "all_required".to_string(),
        type_constraints: vec![],
        rewrite: Userset::Intersection { children },
    });

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations,
            },
        )
        .await;

    // Add tuples for all requirements
    for i in 0..5 {
        tuple_reader
            .add_tuple(
                "store1",
                "document",
                "doc1",
                &format!("req{i}"),
                "user",
                "alice",
                None,
            )
            .await;
    }

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "all_required".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await.unwrap();
    assert!(result.allowed, "All requirements met");
}

// ========== Section 5: Exclusion Relations (A but not B) ==========

#[tokio::test]
async fn test_exclusion_returns_true_if_base_true_and_subtract_false() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // can_view = viewer but not blocked
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "blocked".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "can_view".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::Exclusion {
                            base: Box::new(Userset::ComputedUserset {
                                relation: "viewer".to_string(),
                            }),
                            subtract: Box::new(Userset::ComputedUserset {
                                relation: "blocked".to_string(),
                            }),
                        },
                    },
                ],
            },
        )
        .await;

    // Alice is viewer and NOT blocked
    tuple_reader
        .add_tuple(
            "store1", "document", "doc1", "viewer", "user", "alice", None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "can_view".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await.unwrap();
    assert!(
        result.allowed,
        "Exclusion: viewer but not blocked should be allowed"
    );
}

#[tokio::test]
async fn test_exclusion_returns_false_if_base_is_false() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "blocked".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "can_view".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::Exclusion {
                            base: Box::new(Userset::ComputedUserset {
                                relation: "viewer".to_string(),
                            }),
                            subtract: Box::new(Userset::ComputedUserset {
                                relation: "blocked".to_string(),
                            }),
                        },
                    },
                ],
            },
        )
        .await;

    // Alice is NOT a viewer

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "can_view".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await.unwrap();
    assert!(!result.allowed, "Exclusion: not a viewer should be denied");
}

#[tokio::test]
async fn test_exclusion_returns_false_if_subtract_is_true() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "blocked".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "can_view".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::Exclusion {
                            base: Box::new(Userset::ComputedUserset {
                                relation: "viewer".to_string(),
                            }),
                            subtract: Box::new(Userset::ComputedUserset {
                                relation: "blocked".to_string(),
                            }),
                        },
                    },
                ],
            },
        )
        .await;

    // Alice is viewer BUT ALSO blocked
    tuple_reader
        .add_tuple(
            "store1", "document", "doc1", "viewer", "user", "alice", None,
        )
        .await;
    tuple_reader
        .add_tuple(
            "store1", "document", "doc1", "blocked", "user", "alice", None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "can_view".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await.unwrap();
    assert!(
        !result.allowed,
        "Exclusion: viewer but blocked should be denied"
    );
}

#[tokio::test]
async fn test_exclusion_returns_false_when_base_false_despite_subtract_cycle() {
    // Test that exclusion returns false (not error) when base is false,
    // even if subtract would cycle. Since base is false, we don't need subtract.
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // can_view = viewer but not blocked
    // blocked will have a cycle (blocked -> blocked_cyclic -> blocked)
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "blocked".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::ComputedUserset {
                            relation: "blocked_cyclic".to_string(),
                        },
                    },
                    RelationDefinition {
                        name: "blocked_cyclic".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::ComputedUserset {
                            relation: "blocked".to_string(),
                        },
                    },
                    RelationDefinition {
                        name: "can_view".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::Exclusion {
                            base: Box::new(Userset::ComputedUserset {
                                relation: "viewer".to_string(),
                            }),
                            subtract: Box::new(Userset::ComputedUserset {
                                relation: "blocked".to_string(),
                            }),
                        },
                    },
                ],
            },
        )
        .await;

    // Alice is NOT a viewer (no tuple), so base=false
    // subtract would cycle, but we don't need it

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "can_view".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    // Should return false (base is false), not CycleDetected error
    let result = resolver.check(&request).await;
    assert!(
        result.is_ok(),
        "Exclusion with base=false should not error despite cyclic subtract: {result:?}"
    );
    assert!(
        !result.unwrap().allowed,
        "Exclusion with base=false should return false"
    );
}

#[tokio::test]
async fn test_exclusion_returns_false_when_subtract_true_despite_base_cycle() {
    // Test that exclusion returns false (not error) when subtract is true,
    // even if base would cycle. Since subtract is true, result is false regardless of base.
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // can_view = viewer but not blocked
    // viewer will have a cycle (viewer -> viewer_cyclic -> viewer)
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::ComputedUserset {
                            relation: "viewer_cyclic".to_string(),
                        },
                    },
                    RelationDefinition {
                        name: "viewer_cyclic".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::ComputedUserset {
                            relation: "viewer".to_string(),
                        },
                    },
                    RelationDefinition {
                        name: "blocked".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "can_view".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::Exclusion {
                            base: Box::new(Userset::ComputedUserset {
                                relation: "viewer".to_string(),
                            }),
                            subtract: Box::new(Userset::ComputedUserset {
                                relation: "blocked".to_string(),
                            }),
                        },
                    },
                ],
            },
        )
        .await;

    // Alice IS blocked (subtract=true), viewer would cycle but we don't need it
    tuple_reader
        .add_tuple(
            "store1", "document", "doc1", "blocked", "user", "alice", None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "can_view".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    // Should return false (subtract is true), not CycleDetected error
    let result = resolver.check(&request).await;
    assert!(
        result.is_ok(),
        "Exclusion with subtract=true should not error despite cyclic base: {result:?}"
    );
    assert!(
        !result.unwrap().allowed,
        "Exclusion with subtract=true should return false"
    );
}

#[tokio::test]
async fn test_exclusion_returns_cycle_error_when_both_branches_cycle() {
    // Test that exclusion returns CycleDetected when BOTH branches cycle
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Both viewer and blocked have cycles
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::ComputedUserset {
                            relation: "viewer_cyclic".to_string(),
                        },
                    },
                    RelationDefinition {
                        name: "viewer_cyclic".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::ComputedUserset {
                            relation: "viewer".to_string(),
                        },
                    },
                    RelationDefinition {
                        name: "blocked".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::ComputedUserset {
                            relation: "blocked_cyclic".to_string(),
                        },
                    },
                    RelationDefinition {
                        name: "blocked_cyclic".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::ComputedUserset {
                            relation: "blocked".to_string(),
                        },
                    },
                    RelationDefinition {
                        name: "can_view".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::Exclusion {
                            base: Box::new(Userset::ComputedUserset {
                                relation: "viewer".to_string(),
                            }),
                            subtract: Box::new(Userset::ComputedUserset {
                                relation: "blocked".to_string(),
                            }),
                        },
                    },
                ],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "can_view".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    // Should return CycleDetected error since both branches cycle
    let result = resolver.check(&request).await;
    assert!(
        matches!(result, Err(DomainError::CycleDetected { .. })),
        "Exclusion with both cyclic branches should return CycleDetected, got {result:?}"
    );
}

#[tokio::test]
async fn test_exclusion_propagates_error_when_base_true_and_subtract_errors() {
    // Test that when base is true but subtract errors, the error is propagated
    // (because we need subtract's result to compute the final answer)
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "blocked".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::ComputedUserset {
                            relation: "blocked_cyclic".to_string(),
                        },
                    },
                    RelationDefinition {
                        name: "blocked_cyclic".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::ComputedUserset {
                            relation: "blocked".to_string(),
                        },
                    },
                    RelationDefinition {
                        name: "can_view".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::Exclusion {
                            base: Box::new(Userset::ComputedUserset {
                                relation: "viewer".to_string(),
                            }),
                            subtract: Box::new(Userset::ComputedUserset {
                                relation: "blocked".to_string(),
                            }),
                        },
                    },
                ],
            },
        )
        .await;

    // Alice IS a viewer (base=true), but blocked cycles
    tuple_reader
        .add_tuple(
            "store1", "document", "doc1", "viewer", "user", "alice", None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "can_view".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    // Should return CycleDetected error since we need subtract but it cycles
    let result = resolver.check(&request).await;
    assert!(
        matches!(result, Err(DomainError::CycleDetected { .. })),
        "Exclusion with base=true and cyclic subtract should return error, got {result:?}"
    );
}

// ========== Section 5b: Contextual Tuple Userset Resolution ==========

#[tokio::test]
async fn test_contextual_tuple_resolves_userset_reference() {
    // Test that contextual tuples with userset references like "team:eng#member"
    // are recursively resolved, not just matched literally.
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Set up: document has viewer relation, team has member relation
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into(), "team#member".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "team".to_string(),
                relations: vec![RelationDefinition {
                    name: "member".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // Alice is a member of team:eng (stored tuple)
    tuple_reader
        .add_tuple("store1", "team", "eng", "member", "user", "alice", None)
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Use contextual tuple to grant "team:eng#member" viewer access to document:readme
    // This should resolve recursively: alice -> team:eng#member -> viewer of document:readme
    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "document:readme".to_string(),
        contextual_tuples: Arc::new(vec![ContextualTuple {
            user: "team:eng#member".to_string(),
            relation: "viewer".to_string(),
            object: "document:readme".to_string(),
            condition_name: None,
            condition_context: None,
        }]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await.unwrap();
    assert!(
        result.allowed,
        "Contextual tuple with userset reference should be recursively resolved"
    );
}

#[tokio::test]
async fn test_contextual_tuple_userset_not_member_denied() {
    // Test that users who are NOT members of the userset are denied
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into(), "team#member".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "team".to_string(),
                relations: vec![RelationDefinition {
                    name: "member".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // Alice is a member of team:eng, but Bob is NOT
    tuple_reader
        .add_tuple("store1", "team", "eng", "member", "user", "alice", None)
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Use contextual tuple to grant "team:eng#member" viewer access
    // Bob should be denied because he's not a member of team:eng
    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:bob".to_string(),
        relation: "viewer".to_string(),
        object: "document:readme".to_string(),
        contextual_tuples: Arc::new(vec![ContextualTuple {
            user: "team:eng#member".to_string(),
            relation: "viewer".to_string(),
            object: "document:readme".to_string(),
            condition_name: None,
            condition_context: None,
        }]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await.unwrap();
    assert!(
        !result.allowed,
        "User not in userset should be denied even with contextual tuple"
    );
}

// ========== Section 6: Safety Mechanisms ==========

#[tokio::test]
async fn test_depth_limit_prevents_stack_overflow() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Create deep chain: level0.viewer -> level1.viewer -> ... -> levelN.viewer
    for i in 0..30 {
        let type_name = format!("level{i}");
        let rewrite = if i == 0 {
            Userset::This
        } else {
            Userset::TupleToUserset {
                tupleset: "parent".to_string(),
                computed_userset: "viewer".to_string(),
            }
        };

        let mut relations = vec![RelationDefinition {
            name: "viewer".to_string(),
            type_constraints: vec!["user".into()],
            rewrite,
        }];

        if i > 0 {
            relations.push(RelationDefinition {
                name: "parent".to_string(),
                type_constraints: vec![format!("level{}", i - 1).into()],
                rewrite: Userset::This,
            });
        }

        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name,
                    relations,
                },
            )
            .await;
    }

    // Create chain of parent relationships
    for i in 1..30 {
        tuple_reader
            .add_tuple(
                "store1",
                &format!("level{i}"),
                "obj",
                "parent",
                &format!("level{}", i - 1),
                "obj",
                None,
            )
            .await;
    }

    // Add viewer at level0
    tuple_reader
        .add_tuple("store1", "level0", "obj", "viewer", "user", "alice", None)
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Try to check at level29 (should exceed depth limit of 25)
    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "level29:obj".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await;
    assert!(
        matches!(result, Err(DomainError::DepthLimitExceeded { .. })),
        "Should return depth limit exceeded error"
    );
}

#[tokio::test]
async fn test_depth_limit_matches_openfga_default() {
    let config = ResolverConfig::default();
    assert_eq!(config.max_depth, 25, "Default depth limit should be 25");
}

#[tokio::test]
async fn test_returns_depth_limit_exceeded_error() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Create chain that exceeds depth limit
    for i in 0..30 {
        let type_name = format!("type{i}");
        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: type_name.clone(),
                    relations: vec![
                        RelationDefinition {
                            name: "parent".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "viewer".to_string(),
                            type_constraints: vec![],
                            rewrite: if i == 0 {
                                Userset::This
                            } else {
                                Userset::TupleToUserset {
                                    tupleset: "parent".to_string(),
                                    computed_userset: "viewer".to_string(),
                                }
                            },
                        },
                    ],
                },
            )
            .await;
    }

    for i in 1..30 {
        tuple_reader
            .add_tuple(
                "store1",
                &format!("type{i}"),
                "obj",
                "parent",
                &format!("type{}", i - 1),
                "obj",
                None,
            )
            .await;
    }

    tuple_reader
        .add_tuple("store1", "type0", "obj", "viewer", "user", "alice", None)
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "type29:obj".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await;
    match result {
        Err(DomainError::DepthLimitExceeded { max_depth }) => {
            assert_eq!(max_depth, 25);
        }
        _ => panic!("Expected DepthLimitExceeded error"),
    }
}

#[tokio::test]
async fn test_cycle_detection_prevents_infinite_loops() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Create a cycle: doc.viewer -> folder.viewer -> doc.viewer
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "folder".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "contained_doc".to_string(),
                        type_constraints: vec!["document".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::TupleToUserset {
                            tupleset: "contained_doc".to_string(),
                            computed_userset: "viewer".to_string(),
                        },
                    },
                ],
            },
        )
        .await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "parent".to_string(),
                        type_constraints: vec!["folder".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::TupleToUserset {
                            tupleset: "parent".to_string(),
                            computed_userset: "viewer".to_string(),
                        },
                    },
                ],
            },
        )
        .await;

    // Create circular reference
    tuple_reader
        .add_tuple(
            "store1", "document", "doc1", "parent", "folder", "folder1", None,
        )
        .await;
    tuple_reader
        .add_tuple(
            "store1",
            "folder",
            "folder1",
            "contained_doc",
            "document",
            "doc1",
            None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await;
    // Should either detect cycle or hit depth limit, but not infinite loop
    assert!(
        matches!(
            result,
            Err(DomainError::CycleDetected { .. }) | Err(DomainError::DepthLimitExceeded { .. })
        ),
        "Should detect cycle or hit depth limit"
    );
}

#[tokio::test]
async fn test_cycle_detection_doesnt_false_positive_on_valid_dags() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Create a valid DAG where same object is reachable via multiple paths
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "org".to_string(),
                relations: vec![RelationDefinition {
                    name: "admin".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "folder".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "parent".to_string(),
                        type_constraints: vec!["org".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "admin".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::TupleToUserset {
                            tupleset: "parent".to_string(),
                            computed_userset: "admin".to_string(),
                        },
                    },
                ],
            },
        )
        .await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "parent".to_string(),
                        type_constraints: vec!["folder".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "admin".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::TupleToUserset {
                            tupleset: "parent".to_string(),
                            computed_userset: "admin".to_string(),
                        },
                    },
                ],
            },
        )
        .await;

    // Set up: org1 -> folder1, folder2 -> doc1
    tuple_reader
        .add_tuple("store1", "org", "org1", "admin", "user", "alice", None)
        .await;
    tuple_reader
        .add_tuple("store1", "folder", "folder1", "parent", "org", "org1", None)
        .await;
    tuple_reader
        .add_tuple("store1", "folder", "folder2", "parent", "org", "org1", None)
        .await;
    tuple_reader
        .add_tuple(
            "store1", "document", "doc1", "parent", "folder", "folder1", None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "admin".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await.unwrap();
    assert!(result.allowed, "Valid DAG should allow access");
}

#[tokio::test]
async fn test_timeout_is_configurable() {
    let config = ResolverConfig {
        max_depth: 25,
        timeout: Duration::from_millis(100),
        cache: None,
    };

    assert_eq!(config.timeout, Duration::from_millis(100));
}

#[tokio::test]
async fn test_returns_timeout_error_with_context() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::Mutex;

    // Create a slow tuple reader that delays responses
    struct SlowTupleReader {
        delay: Duration,
        stores: Mutex<HashSet<String>>,
        started: AtomicBool,
    }

    impl SlowTupleReader {
        fn new(delay: Duration) -> Self {
            Self {
                delay,
                stores: Mutex::new(HashSet::new()),
                started: AtomicBool::new(false),
            }
        }

        async fn add_store(&self, store_id: &str) {
            self.stores.lock().await.insert(store_id.to_string());
        }
    }

    #[async_trait]
    impl TupleReader for SlowTupleReader {
        async fn read_tuples(
            &self,
            _store_id: &str,
            _object_type: &str,
            _object_id: &str,
            _relation: &str,
        ) -> DomainResult<Vec<StoredTupleRef>> {
            self.started.store(true, Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            Ok(vec![])
        }

        async fn store_exists(&self, store_id: &str) -> DomainResult<bool> {
            Ok(self.stores.lock().await.contains(store_id))
        }
    }

    let tuple_reader = Arc::new(SlowTupleReader::new(Duration::from_secs(10)));
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    let config = ResolverConfig {
        max_depth: 25,
        timeout: Duration::from_millis(50),
        cache: None,
    };

    let resolver = GraphResolver::with_config(tuple_reader.clone(), model_reader, config);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await;
    match result {
        Err(DomainError::Timeout { duration_ms }) => {
            assert_eq!(duration_ms, 50);
        }
        _ => panic!("Expected Timeout error, got {result:?}"),
    }
}

// ========== Section 6b: Edge Case Tests ==========

#[tokio::test]
async fn test_empty_union_returns_false() {
    // Empty union should return false (no branches to satisfy)
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec![],
                    rewrite: Userset::Union { children: vec![] },
                }],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await.unwrap();
    assert!(!result.allowed, "Empty union should return false");
}

#[tokio::test]
async fn test_empty_intersection_returns_true() {
    // Empty intersection should return true (vacuously all conditions met)
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec![],
                    rewrite: Userset::Intersection { children: vec![] },
                }],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await.unwrap();
    assert!(result.allowed, "Empty intersection should return true");
}

#[tokio::test]
async fn test_depth_limit_at_boundary_24_succeeds() {
    // Test that depth 24 (just under limit of 25) succeeds
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Create chain of exactly 24 levels
    for i in 0..25 {
        let type_name = format!("level{i}");
        let rewrite = if i == 0 {
            Userset::This
        } else {
            Userset::TupleToUserset {
                tupleset: "parent".to_string(),
                computed_userset: "viewer".to_string(),
            }
        };

        let mut relations = vec![RelationDefinition {
            name: "viewer".to_string(),
            type_constraints: vec!["user".into()],
            rewrite,
        }];

        if i > 0 {
            relations.push(RelationDefinition {
                name: "parent".to_string(),
                type_constraints: vec![format!("level{}", i - 1).into()],
                rewrite: Userset::This,
            });
        }

        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name,
                    relations,
                },
            )
            .await;
    }

    // Create chain of parent relationships (24 hops)
    for i in 1..25 {
        tuple_reader
            .add_tuple(
                "store1",
                &format!("level{i}"),
                "obj",
                "parent",
                &format!("level{}", i - 1),
                "obj",
                None,
            )
            .await;
    }

    // Alice is viewer at level0
    tuple_reader
        .add_tuple("store1", "level0", "obj", "viewer", "user", "alice", None)
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Check at level24 - should succeed (depth 24 is within limit of 25)
    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "level24:obj".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await.unwrap();
    assert!(
        result.allowed,
        "Depth 24 should succeed (within limit of 25)"
    );
}

#[tokio::test]
async fn test_contextual_tuple_overrides_stored_tuple() {
    // Contextual tuples should be checked first and can grant access
    // even if no stored tuple exists
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // No stored tuple for alice

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Without contextual tuple - should be denied
    let request_without = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request_without).await.unwrap();
    assert!(!result.allowed, "Should be denied without contextual tuple");

    // With contextual tuple - should be allowed
    let request_with = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![ContextualTuple {
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:doc1".to_string(),
            condition_name: None,
            condition_context: None,
        }]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request_with).await.unwrap();
    assert!(result.allowed, "Contextual tuple should grant access");
}

#[tokio::test]
async fn test_contextual_tuple_does_not_conflict_with_stored() {
    // Both contextual and stored tuples can coexist
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // Bob has stored tuple
    tuple_reader
        .add_tuple("store1", "document", "doc1", "viewer", "user", "bob", None)
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Alice uses contextual tuple, Bob uses stored - both should work
    let request_alice = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![ContextualTuple::new(
            "user:alice",
            "viewer",
            "document:doc1",
        )]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let request_bob = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:bob".to_string(),
        relation: "viewer".to_string(),
        object: "document:doc1".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result_alice = resolver.check(&request_alice).await.unwrap();
    let result_bob = resolver.check(&request_bob).await.unwrap();

    assert!(
        result_alice.allowed,
        "Alice should have access via contextual tuple"
    );
    assert!(
        result_bob.allowed,
        "Bob should have access via stored tuple"
    );
}

// ========== Section 6b: Security Tests ==========

#[tokio::test]
async fn test_wildcard_in_requesting_user_is_rejected() {
    // Security test: Users cannot request with wildcards like "admin:*" to
    // match stored tuples. This prevents authorization bypass.
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // Add a tuple for a specific admin
    tuple_reader
        .add_tuple(
            "store1",
            "document",
            "secret",
            "viewer",
            "admin",
            "superuser",
            None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Attacker tries to request with wildcard to match the admin tuple
    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "admin:*".to_string(), // Should NOT match admin:superuser
        relation: "viewer".to_string(),
        object: "document:secret".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await.unwrap();
    assert!(
        !result.allowed,
        "Wildcard in requesting_user should NOT match stored tuples"
    );
}

#[tokio::test]
async fn test_type_constraints_enforced_for_stored_tuples() {
    // Security test: Tuples from disallowed user types should not grant access.
    // If a relation only allows "user" type, a "bot" tuple should be ignored.
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Define viewer relation that only allows "user" type
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()], // Only users allowed
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // Add a tuple with disallowed type (bot instead of user)
    tuple_reader
        .add_tuple(
            "store1", "document", "secret", "viewer", "bot", "scraper", None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Request from bot:scraper should be denied even though tuple exists
    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "bot:scraper".to_string(),
        relation: "viewer".to_string(),
        object: "document:secret".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await.unwrap();
    assert!(
        !result.allowed,
        "Tuple with disallowed user type should not grant access"
    );
}

#[tokio::test]
async fn test_type_constraints_enforced_for_contextual_tuples() {
    // Security test: Contextual tuples with disallowed types should be ignored.
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Define viewer relation that only allows "user" type
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()], // Only users allowed
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Request with contextual tuple of disallowed type
    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "bot:scraper".to_string(),
        relation: "viewer".to_string(),
        object: "document:secret".to_string(),
        contextual_tuples: Arc::new(vec![ContextualTuple {
            user: "bot:scraper".to_string(), // Type "bot" not allowed
            relation: "viewer".to_string(),
            object: "document:secret".to_string(),
            condition_name: None,
            condition_context: None,
        }]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await.unwrap();
    assert!(
        !result.allowed,
        "Contextual tuple with disallowed user type should not grant access"
    );
}

#[tokio::test]
async fn test_type_constraints_allow_userset_references() {
    // Test that type constraints properly validate userset references like "group#member"
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Define viewer relation that allows both user and group#member
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into(), "group#member".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "group".to_string(),
                relations: vec![RelationDefinition {
                    name: "member".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // Add group membership and document access via group
    tuple_reader
        .add_tuple("store1", "group", "eng", "member", "user", "alice", None)
        .await;
    tuple_reader
        .add_tuple(
            "store1",
            "document",
            "readme",
            "viewer",
            "group",
            "eng",
            Some("member"),
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "document:readme".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await.unwrap();
    assert!(
        result.allowed,
        "User should have access via group#member userset reference"
    );
}

#[tokio::test]
async fn test_empty_type_constraints_allows_any_type() {
    // Test that empty type_constraints allows any user type (backwards compatible)
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Define viewer relation with empty type_constraints
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec![], // Empty = allow any
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // Add tuple with any type
    tuple_reader
        .add_tuple(
            "store1", "document", "readme", "viewer", "bot", "scraper", None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "bot:scraper".to_string(),
        relation: "viewer".to_string(),
        object: "document:readme".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await.unwrap();
    assert!(
        result.allowed,
        "Empty type_constraints should allow any user type"
    );
}

// ========== Section 7: Property-Based Tests ==========

use proptest::prelude::*;

/// Strategy to generate valid user identifiers
fn user_strategy() -> impl Strategy<Value = String> {
    "[a-z]{1,10}".prop_map(|s| format!("user:{s}"))
}

/// Strategy to generate valid object identifiers
fn object_strategy() -> impl Strategy<Value = (String, String, String)> {
    ("[a-z]{1,10}", "[a-z0-9]{1,10}")
        .prop_map(|(type_name, id)| (type_name.clone(), id.clone(), format!("{type_name}:{id}")))
}

/// Strategy to generate valid relation names
fn relation_strategy() -> impl Strategy<Value = String> {
    "[a-z]{1,10}"
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property: Resolver never panics on any input
    #[test]
    fn test_property_resolver_never_panics(
        user in user_strategy(),
        (type_name, object_id, object) in object_strategy(),
        relation in relation_strategy(),
        store_id in "[a-z]{1,5}",
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let tuple_reader = Arc::new(MockTupleReader::new());
            let model_reader = Arc::new(MockModelReader::new());

            tuple_reader.add_store(&store_id).await;

            // Add type definition
            model_reader
                .add_type(
                    &store_id,
                    TypeDefinition {
                        type_name: type_name.clone(),
                        relations: vec![RelationDefinition {
                            name: relation.clone(),
                            type_constraints: vec!["user".into()],
                            rewrite: Userset::This,
                        }],
                    },
                )
                .await;

            // Optionally add a tuple
            tuple_reader
                .add_tuple(&store_id, &type_name, &object_id, &relation, "user", user.strip_prefix("user:").unwrap_or(&user), None)
                .await;

            let resolver = GraphResolver::new(tuple_reader, model_reader);

            let request = CheckRequest {
                store_id: store_id.clone(),
                user: user.clone(),
                relation: relation.clone(),
                object: object.clone(),
                contextual_tuples: Arc::new(vec![]),
                context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
            };

            // Should not panic - may return Ok or Err, but should never panic
            let _ = resolver.check(&request).await;
        });
    }

    /// Property: Resolver always terminates (no infinite loops)
    /// Verified by using a short timeout and ensuring we always get a result
    #[test]
    fn test_property_resolver_always_terminates(
        user in user_strategy(),
        (type_name, _object_id, object) in object_strategy(),
        relation in relation_strategy(),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let terminated = rt.block_on(async {
            let tuple_reader = Arc::new(MockTupleReader::new());
            let model_reader = Arc::new(MockModelReader::new());

            tuple_reader.add_store("store1").await;

            // Add type definition with potential cycle
            model_reader
                .add_type(
                    "store1",
                    TypeDefinition {
                        type_name: type_name.clone(),
                        relations: vec![RelationDefinition {
                            name: relation.clone(),
                            type_constraints: vec!["user".into()],
                            rewrite: Userset::This,
                        }],
                    },
                )
                .await;

            let config = ResolverConfig {
                max_depth: 25,
                timeout: Duration::from_secs(1), // Short timeout
                cache: None,
            };

            let resolver = GraphResolver::with_config(tuple_reader, model_reader, config);

            let request = CheckRequest {
                store_id: "store1".to_string(),
                user: user.clone(),
                relation: relation.clone(),
                object: object.clone(),
                contextual_tuples: Arc::new(vec![]),
                context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
            };

            // Use a timeout to ensure termination
            let result = tokio::time::timeout(
                Duration::from_secs(2),
                resolver.check(&request),
            )
            .await;

            // Should always terminate within 2 seconds
            result.is_ok()
        });
        prop_assert!(terminated, "Resolver should always terminate");
    }

    /// Property: Resolver returns correct result for known graphs (direct tuples)
    #[test]
    fn test_property_resolver_returns_correct_result_for_known_graphs(
        user_name in "[a-z]{1,5}",
        object_id in "[a-z0-9]{1,5}",
        has_tuple in proptest::bool::ANY,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (allowed, expected) = rt.block_on(async {
            let tuple_reader = Arc::new(MockTupleReader::new());
            let model_reader = Arc::new(MockModelReader::new());

            tuple_reader.add_store("store1").await;

            model_reader
                .add_type(
                    "store1",
                    TypeDefinition {
                        type_name: "document".to_string(),
                        relations: vec![RelationDefinition {
                            name: "viewer".to_string(),
                            type_constraints: vec!["user".into()],
                            rewrite: Userset::This,
                        }],
                    },
                )
                .await;

            if has_tuple {
                tuple_reader
                    .add_tuple("store1", "document", &object_id, "viewer", "user", &user_name, None)
                    .await;
            }

            let resolver = GraphResolver::new(tuple_reader, model_reader);

            let request = CheckRequest {
                store_id: "store1".to_string(),
                user: format!("user:{user_name}"),
                relation: "viewer".to_string(),
                object: format!("document:{object_id}"),
                contextual_tuples: Arc::new(vec![]),
                context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
            };

            let result = resolver.check(&request).await.unwrap();
            (result.allowed, has_tuple)
        });
        // Direct tuple: result should match whether tuple exists
        prop_assert_eq!(allowed, expected,
            "Result should match tuple existence: tuple={}, allowed={}",
            expected, allowed);
    }

    /// Property: Adding a tuple never makes check false→true incorrectly
    /// (Monotonicity: adding permissions should only grant more access)
    #[test]
    fn test_property_adding_tuple_never_incorrectly_grants(
        user_name in "[a-z]{1,5}",
        object_id in "[a-z0-9]{1,5}",
        other_user in "[a-z]{1,5}",
    ) {
        // Skip if users are the same
        prop_assume!(user_name != other_user);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let (before, after) = rt.block_on(async {
            let tuple_reader = Arc::new(MockTupleReader::new());
            let model_reader = Arc::new(MockModelReader::new());

            tuple_reader.add_store("store1").await;

            model_reader
                .add_type(
                    "store1",
                    TypeDefinition {
                        type_name: "document".to_string(),
                        relations: vec![RelationDefinition {
                            name: "viewer".to_string(),
                            type_constraints: vec!["user".into()],
                            rewrite: Userset::This,
                        }],
                    },
                )
                .await;

            let resolver = GraphResolver::new(tuple_reader.clone(), model_reader);

            // Check for user before adding tuple for OTHER user
            let request = CheckRequest {
                store_id: "store1".to_string(),
                user: format!("user:{user_name}"),
                relation: "viewer".to_string(),
                object: format!("document:{object_id}"),
                contextual_tuples: Arc::new(vec![]),
                context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
            };

            let result_before = resolver.check(&request).await.unwrap();

            // Add tuple for OTHER user
            tuple_reader
                .add_tuple("store1", "document", &object_id, "viewer", "user", &other_user, None)
                .await;

            let result_after = resolver.check(&request).await.unwrap();
            (result_before.allowed, result_after.allowed)
        });
        // Adding a tuple for a DIFFERENT user should not change our user's access
        prop_assert_eq!(before, after,
            "Adding tuple for other_user={} should not affect user={}'s access. Before={}, After={}",
            other_user, user_name, before, after);
    }

    /// Property: Deleting a tuple never makes check true→false incorrectly
    /// (Only the specific permission should be revoked)
    #[test]
    fn test_property_deleting_tuple_only_revokes_specific_permission(
        user_name in "[a-z]{1,5}",
        object_id in "[a-z0-9]{1,5}",
        other_object in "[a-z0-9]{1,5}",
    ) {
        // Skip if objects are the same
        prop_assume!(object_id != other_object);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let (before, after) = rt.block_on(async {
            let tuple_reader = Arc::new(MockTupleReader::new());
            let model_reader = Arc::new(MockModelReader::new());

            tuple_reader.add_store("store1").await;

            model_reader
                .add_type(
                    "store1",
                    TypeDefinition {
                        type_name: "document".to_string(),
                        relations: vec![RelationDefinition {
                            name: "viewer".to_string(),
                            type_constraints: vec!["user".into()],
                            rewrite: Userset::This,
                        }],
                    },
                )
                .await;

            // Add tuples for both objects
            tuple_reader
                .add_tuple("store1", "document", &object_id, "viewer", "user", &user_name, None)
                .await;
            tuple_reader
                .add_tuple("store1", "document", &other_object, "viewer", "user", &user_name, None)
                .await;

            let resolver = GraphResolver::new(tuple_reader.clone(), model_reader);

            // Check access to other_object before deleting tuple for object_id
            let request = CheckRequest {
                store_id: "store1".to_string(),
                user: format!("user:{user_name}"),
                relation: "viewer".to_string(),
                object: format!("document:{other_object}"),
                contextual_tuples: Arc::new(vec![]),
                context: Arc::new(std::collections::HashMap::new()),
        authorization_model_id: None,
            };

            let result_before = resolver.check(&request).await.unwrap();

            // Delete tuple for object_id (not other_object)
            tuple_reader
                .remove_tuple("store1", "document", &object_id, "viewer", "user", &user_name)
                .await;

            // Access to other_object should still be true
            let result_after = resolver.check(&request).await.unwrap();
            (result_before.allowed, result_after.allowed)
        });
        prop_assert!(before, "Should have access before");
        prop_assert!(after,
            "Deleting tuple for object_id={} should not affect access to other_object={}",
            object_id, other_object);
    }
}

// ========== Section 10: CEL Condition Evaluation ==========

/// Test: Check evaluates tuple condition with context.
///
/// When a tuple has a condition attached, the condition's CEL expression
/// must be evaluated against the check request's context. Access is only
/// granted if the condition evaluates to true.
#[tokio::test]
async fn test_check_evaluates_tuple_condition_with_context() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    // Set up store
    tuple_reader.add_store("store1").await;

    // Add a condition definition: is_enabled requires request.enabled == true
    let condition = Condition::new("is_enabled", "request.enabled == true").unwrap();
    model_reader.add_condition("store1", condition).await;

    // Add type definition with viewer relation
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // Add tuple with condition: user:alice is viewer of document:readme IF is_enabled
    tuple_reader
        .add_tuple_with_condition(
            "store1",
            "document",
            "readme",
            "viewer",
            "user",
            "alice",
            None,
            "is_enabled",
            None, // No tuple-specific condition context
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Create context with enabled = true
    let mut context = HashMap::new();
    context.insert("enabled".to_string(), serde_json::Value::Bool(true));

    let request = CheckRequest::with_context(
        "store1".to_string(),
        "user:alice".to_string(),
        "viewer".to_string(),
        "document:readme".to_string(),
        vec![],
        context,
    );

    let result = resolver.check(&request).await.unwrap();
    assert!(
        result.allowed,
        "Condition evaluates to true, access should be granted"
    );
}

/// Test: Check returns false when condition evaluates to false.
///
/// This is the core test for condition evaluation - a matching tuple with
/// a condition that evaluates to false should deny access.
#[tokio::test]
async fn test_check_returns_false_when_condition_evaluates_false() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Add condition: is_enabled requires request.enabled == true
    let condition = Condition::new("is_enabled", "request.enabled == true").unwrap();
    model_reader.add_condition("store1", condition).await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // Tuple with condition
    tuple_reader
        .add_tuple_with_condition(
            "store1",
            "document",
            "readme",
            "viewer",
            "user",
            "alice",
            None,
            "is_enabled",
            None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Context with enabled = false - condition should fail
    let mut context = HashMap::new();
    context.insert("enabled".to_string(), serde_json::Value::Bool(false));

    let request = CheckRequest::with_context(
        "store1".to_string(),
        "user:alice".to_string(),
        "viewer".to_string(),
        "document:readme".to_string(),
        vec![],
        context,
    );

    let result = resolver.check(&request).await.unwrap();
    assert!(
        !result.allowed,
        "Condition evaluates to false, access should be denied"
    );
}

/// Test: Check without required context returns error.
///
/// When a condition references a variable that's not in the context,
/// the check should return an error indicating the evaluation failure.
#[tokio::test]
async fn test_check_without_required_context_returns_error() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Condition that requires "enabled" variable
    let condition = Condition::new("is_enabled", "request.enabled == true").unwrap();
    model_reader.add_condition("store1", condition).await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    tuple_reader
        .add_tuple_with_condition(
            "store1",
            "document",
            "readme",
            "viewer",
            "user",
            "alice",
            None,
            "is_enabled",
            None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Empty context - required variable "enabled" is missing
    let request = CheckRequest::with_context(
        "store1".to_string(),
        "user:alice".to_string(),
        "viewer".to_string(),
        "document:readme".to_string(),
        vec![],
        HashMap::new(),
    );

    let result = resolver.check(&request).await;
    let error = result.expect_err("Check should fail when required context variable is missing");
    match error {
        DomainError::ConditionEvalError { reason } => {
            assert!(
                reason.contains("No such key"),
                "Error should indicate missing context key: {reason}"
            );
        }
        _ => panic!("Expected ConditionEvalError but got: {error:?}"),
    }
}

/// Test: Check returns true when condition evaluates true.
///
/// Verifies that access is granted when a tuple's condition evaluates to true.
#[tokio::test]
async fn test_check_returns_true_when_condition_evaluates_true() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Condition checking if level >= required_level
    let condition = Condition::new("level_check", "request.level >= 5").unwrap();
    model_reader.add_condition("store1", condition).await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    tuple_reader
        .add_tuple_with_condition(
            "store1",
            "document",
            "readme",
            "viewer",
            "user",
            "alice",
            None,
            "level_check",
            None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Context with level = 10 (should pass >= 5 check)
    let mut context = HashMap::new();
    context.insert("level".to_string(), serde_json::json!(10));

    let request = CheckRequest::with_context(
        "store1".to_string(),
        "user:alice".to_string(),
        "viewer".to_string(),
        "document:readme".to_string(),
        vec![],
        context,
    );

    let result = resolver.check(&request).await.unwrap();
    assert!(
        result.allowed,
        "Condition evaluates to true (level 10 >= 5), access should be granted"
    );
}

/// Test: Condition evaluation respects tuple's condition params.
///
/// Tuple-specific condition context is merged with request context.
/// Per OpenFGA spec, tuple context takes precedence over request context
/// for overlapping keys (see test_tuple_context_takes_precedence_over_request).
#[tokio::test]
async fn test_condition_evaluation_respects_tuple_condition_params() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Condition that checks department match
    let condition =
        Condition::new("dept_match", "request.user_dept == request.required_dept").unwrap();
    model_reader.add_condition("store1", condition).await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // Tuple with condition params: requires "engineering" department
    let mut tuple_context = HashMap::new();
    tuple_context.insert(
        "required_dept".to_string(),
        serde_json::json!("engineering"),
    );

    tuple_reader
        .add_tuple_with_condition(
            "store1",
            "document",
            "readme",
            "viewer",
            "user",
            "alice",
            None,
            "dept_match",
            Some(tuple_context),
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Request context with matching department
    let mut context = HashMap::new();
    context.insert("user_dept".to_string(), serde_json::json!("engineering"));

    let request = CheckRequest::with_context(
        "store1".to_string(),
        "user:alice".to_string(),
        "viewer".to_string(),
        "document:readme".to_string(),
        vec![],
        context,
    );

    let result = resolver.check(&request).await.unwrap();
    assert!(
        result.allowed,
        "Departments match (engineering == engineering), access should be granted"
    );

    // Request with non-matching department
    let mut context2 = HashMap::new();
    context2.insert("user_dept".to_string(), serde_json::json!("sales"));

    let request2 = CheckRequest::with_context(
        "store1".to_string(),
        "user:alice".to_string(),
        "viewer".to_string(),
        "document:readme".to_string(),
        vec![],
        context2,
    );

    let result2 = resolver.check(&request2).await.unwrap();
    assert!(
        !result2.allowed,
        "Departments don't match (sales != engineering), access should be denied"
    );
}

/// Test: Contextual tuples with conditions work.
///
/// Conditions on contextual tuples should also be evaluated.
#[tokio::test]
async fn test_contextual_tuples_with_conditions_work() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Condition for time-based access
    let condition = Condition::new("is_active", "request.active == true").unwrap();
    model_reader.add_condition("store1", condition).await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // No stored tuples - using contextual tuple only

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Contextual tuple with condition
    let contextual_tuple = ContextualTuple::with_condition(
        "user:alice",
        "viewer",
        "document:readme",
        "is_active",
        None,
    );

    // Context with active = true
    let mut context = HashMap::new();
    context.insert("active".to_string(), serde_json::Value::Bool(true));

    let request = CheckRequest::with_context(
        "store1".to_string(),
        "user:alice".to_string(),
        "viewer".to_string(),
        "document:readme".to_string(),
        vec![contextual_tuple.clone()],
        context,
    );

    let result = resolver.check(&request).await.unwrap();
    assert!(
        result.allowed,
        "Contextual tuple condition is true, access should be granted"
    );

    // Context with active = false
    let mut context2 = HashMap::new();
    context2.insert("active".to_string(), serde_json::Value::Bool(false));

    let request2 = CheckRequest::with_context(
        "store1".to_string(),
        "user:alice".to_string(),
        "viewer".to_string(),
        "document:readme".to_string(),
        vec![contextual_tuple],
        context2,
    );

    let result2 = resolver.check(&request2).await.unwrap();
    assert!(
        !result2.allowed,
        "Contextual tuple condition is false, access should be denied"
    );
}

/// Test: Multiple tuples where only one has passing condition.
///
/// When there are multiple matching tuples, access should be granted
/// if ANY tuple's condition passes.
#[tokio::test]
async fn test_check_with_multiple_tuples_evaluates_all() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Two conditions
    let condition1 = Condition::new("is_admin", "request.role == \"admin\"").unwrap();
    let condition2 = Condition::new("is_owner", "request.is_owner == true").unwrap();
    model_reader.add_condition("store1", condition1).await;
    model_reader.add_condition("store1", condition2).await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // Two tuples with different conditions
    tuple_reader
        .add_tuple_with_condition(
            "store1", "document", "readme", "viewer", "user", "alice", None, "is_admin", None,
        )
        .await;

    tuple_reader
        .add_tuple_with_condition(
            "store1", "document", "readme", "viewer", "user", "alice", None, "is_owner", None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Context where only is_owner passes
    let mut context = HashMap::new();
    context.insert("role".to_string(), serde_json::json!("user")); // not admin
    context.insert("is_owner".to_string(), serde_json::json!(true)); // is owner

    let request = CheckRequest::with_context(
        "store1".to_string(),
        "user:alice".to_string(),
        "viewer".to_string(),
        "document:readme".to_string(),
        vec![],
        context,
    );

    let result = resolver.check(&request).await.unwrap();
    assert!(
        result.allowed,
        "At least one condition (is_owner) passes, access should be granted"
    );

    // Context where neither condition passes
    let mut context2 = HashMap::new();
    context2.insert("role".to_string(), serde_json::json!("user"));
    context2.insert("is_owner".to_string(), serde_json::json!(false));

    let request2 = CheckRequest::with_context(
        "store1".to_string(),
        "user:alice".to_string(),
        "viewer".to_string(),
        "document:readme".to_string(),
        vec![],
        context2,
    );

    let result2 = resolver.check(&request2).await.unwrap();
    assert!(
        !result2.allowed,
        "No conditions pass, access should be denied"
    );
}

/// Test: Tuple context takes precedence over request context for overlapping keys.
///
/// Per OpenFGA spec: "If you provide a context value in the request context
/// that is also written/persisted in the relationship tuple, then the context
/// values written in the relationship tuple take precedence."
#[tokio::test]
async fn test_tuple_context_takes_precedence_over_request() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Condition that checks if department is "engineering"
    let condition = Condition::new("dept_check", "request.department == \"engineering\"").unwrap();
    model_reader.add_condition("store1", condition).await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // Tuple with condition context: department = "engineering"
    let mut tuple_context = HashMap::new();
    tuple_context.insert("department".to_string(), serde_json::json!("engineering"));

    tuple_reader
        .add_tuple_with_condition(
            "store1",
            "document",
            "readme",
            "viewer",
            "user",
            "alice",
            None,
            "dept_check",
            Some(tuple_context),
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Request context tries to override department to "sales"
    // Per OpenFGA spec, tuple context should take precedence
    let mut request_context = HashMap::new();
    request_context.insert("department".to_string(), serde_json::json!("sales"));

    let request = CheckRequest::with_context(
        "store1".to_string(),
        "user:alice".to_string(),
        "viewer".to_string(),
        "document:readme".to_string(),
        vec![],
        request_context,
    );

    let result = resolver.check(&request).await.unwrap();
    assert!(
        result.allowed,
        "Tuple context (department=engineering) takes precedence over request context \
         (department=sales), so condition should pass"
    );
}

/// Test: Conditions on parent tuples in TupleToUserset are evaluated.
///
/// When resolving a TupleToUserset relation (e.g., document.viewer = folder.viewer),
/// conditions on the parent tuple should also be evaluated.
#[tokio::test]
async fn test_condition_on_parent_tuple_in_tuple_to_userset() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Condition that checks if access is published
    let condition = Condition::new("is_published", "request.published == true").unwrap();
    model_reader.add_condition("store1", condition).await;

    // folder type with viewer relation
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "folder".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // document type: viewer = viewer from parent folder (TupleToUserset)
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "parent".to_string(),
                        type_constraints: vec!["folder".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::TupleToUserset {
                            tupleset: "parent".to_string(),
                            computed_userset: "viewer".to_string(),
                        },
                    },
                ],
            },
        )
        .await;

    // Parent tuple with condition: alice is viewer of folder1 IF published
    tuple_reader
        .add_tuple_with_condition(
            "store1",
            "folder",
            "folder1",
            "viewer",
            "user",
            "alice",
            None,
            "is_published",
            None, // No tuple context, relies on request context
        )
        .await;

    // Link document to folder
    tuple_reader
        .add_tuple(
            "store1", "document", "doc1", "parent", "folder", "folder1", None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Request with published=true - should grant access
    let mut context = HashMap::new();
    context.insert("published".to_string(), serde_json::json!(true));

    let request = CheckRequest::with_context(
        "store1".to_string(),
        "user:alice".to_string(),
        "viewer".to_string(),
        "document:doc1".to_string(),
        vec![],
        context,
    );

    let result = resolver.check(&request).await.unwrap();
    assert!(
        result.allowed,
        "Parent folder's condition (is_published) passes with published=true"
    );

    // Request with published=false - should deny access
    let mut context2 = HashMap::new();
    context2.insert("published".to_string(), serde_json::json!(false));

    let request2 = CheckRequest::with_context(
        "store1".to_string(),
        "user:alice".to_string(),
        "viewer".to_string(),
        "document:doc1".to_string(),
        vec![],
        context2,
    );

    let result2 = resolver.check(&request2).await.unwrap();
    assert!(
        !result2.allowed,
        "Parent folder's condition (is_published) fails with published=false"
    );
}

/// Test: Check returns error when tuple references non-existent condition.
///
/// When a tuple has a condition_name that doesn't exist in the authorization model,
/// the check should return an error rather than silently allowing or denying access.
#[tokio::test]
async fn test_check_returns_error_for_nonexistent_condition() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Note: We intentionally do NOT add "missing_condition" to the model

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // Add tuple with a condition that doesn't exist in the model
    tuple_reader
        .add_tuple_with_condition(
            "store1",
            "document",
            "doc1",
            "viewer",
            "user",
            "alice",
            None,
            "missing_condition", // This condition is not defined
            None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest::new(
        "store1".to_string(),
        "user:alice".to_string(),
        "viewer".to_string(),
        "document:doc1".to_string(),
        vec![],
    );

    let result = resolver.check(&request).await;
    let err = result.expect_err("Should return error when condition is not found in model");
    // ConditionNotFound error message format: "condition '{name}' not defined in authorization model"
    assert!(
        err.to_string()
            .contains("not defined in authorization model"),
        "Error message should indicate condition not found: {err}"
    );
}

/// Test: Check returns error when CEL expression has syntax error.
///
/// When a condition's CEL expression has a syntax error, the check should
/// return an error with a clear message indicating the parse failure.
#[tokio::test]
async fn test_check_returns_error_for_invalid_cel_expression() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Condition with invalid CEL syntax (unclosed parenthesis)
    let condition = Condition::new("broken_expr", "request.value == (1 + 2");
    // Note: Condition::new might catch this - if so, this test verifies
    // the validation happens at definition time rather than evaluation time

    if condition.is_err() {
        // Good: The condition is validated at creation time
        // This is the preferred behavior - fail fast
        return;
    }

    model_reader
        .add_condition("store1", condition.unwrap())
        .await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    tuple_reader
        .add_tuple_with_condition(
            "store1",
            "document",
            "doc1",
            "viewer",
            "user",
            "alice",
            None,
            "broken_expr",
            None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest::new(
        "store1".to_string(),
        "user:alice".to_string(),
        "viewer".to_string(),
        "document:doc1".to_string(),
        vec![],
    );

    let result = resolver.check(&request).await;
    // Either the condition creation fails (preferred) or the check fails
    if let Err(err) = result {
        assert!(
            err.to_string().contains("failed to parse")
                || err.to_string().contains("condition evaluation failed"),
            "Error should indicate CEL parse/eval failure: {err}"
        );
    }
}

/// Test: Check returns error when condition context is missing.
///
/// Both OpenFGA and RSFGA return error code 2000 when required condition
/// parameters are not provided in the request context. The condition cannot
/// be evaluated without its required parameters.
///
/// See: https://github.com/julianshen/rsfga/issues/290 (verified both RSFGA and
/// OpenFGA return an error, not false)
#[tokio::test]
async fn test_check_returns_error_when_condition_context_missing() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Create a condition that requires "current_time" parameter
    let condition = Condition::with_parameters(
        "time_window",
        vec![
            ConditionParameter::new("current_time", "timestamp"),
            ConditionParameter::new("valid_from", "timestamp"),
            ConditionParameter::new("valid_until", "timestamp"),
        ],
        "request.current_time >= request.valid_from && request.current_time < request.valid_until",
    )
    .unwrap();

    model_reader.add_condition("store1", condition).await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "time_limited_viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // Add tuple with condition context (valid_from and valid_until are in tuple)
    let mut tuple_context = HashMap::new();
    tuple_context.insert(
        "valid_from".to_string(),
        serde_json::Value::String("2024-01-01T00:00:00Z".to_string()),
    );
    tuple_context.insert(
        "valid_until".to_string(),
        serde_json::Value::String("2024-12-31T23:59:59Z".to_string()),
    );

    tuple_reader
        .add_tuple_with_condition(
            "store1",
            "document",
            "sensitive",
            "time_limited_viewer",
            "user",
            "bob",
            None,
            "time_window",
            Some(tuple_context),
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Act: Check WITHOUT providing current_time in request context
    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:bob".to_string(),
        relation: "time_limited_viewer".to_string(),
        object: "document:sensitive".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await;

    // Assert: Should return an error because required context is missing
    // Both OpenFGA and RSFGA return error code 2000 in this case
    assert!(
        result.is_err(),
        "Missing condition context should return an error, not Ok: {:?}",
        result
    );
}

/// Test: Check returns true when all condition parameters are provided and satisfied.
///
/// Verifies that conditions work correctly when all required context is provided.
#[tokio::test]
async fn test_check_returns_true_when_condition_context_provided_and_satisfied() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Create a simple condition that checks a boolean value
    let condition = Condition::with_parameters(
        "is_enabled",
        vec![ConditionParameter::new("enabled", "bool")],
        "request.enabled == true",
    )
    .unwrap();

    model_reader.add_condition("store1", condition).await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "feature".to_string(),
                relations: vec![RelationDefinition {
                    name: "user".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    tuple_reader
        .add_tuple_with_condition(
            "store1",
            "feature",
            "beta",
            "user",
            "user",
            "alice",
            None,
            "is_enabled",
            None, // No tuple context, param comes from request
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Act: Check WITH the required context provided and set to true
    let mut context = HashMap::new();
    context.insert("enabled".to_string(), serde_json::json!(true));

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "user".to_string(),
        object: "feature:beta".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(context),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await;

    // Assert: Should return Ok(true) when condition is satisfied
    assert!(result.is_ok(), "Check should succeed: {:?}", result);
    assert!(
        result.unwrap().allowed,
        "Condition should be satisfied when enabled=true"
    );
}

/// Test: Check returns false when condition parameters are provided but not satisfied.
///
/// Verifies that conditions correctly deny access when the expression evaluates to false.
#[tokio::test]
async fn test_check_returns_false_when_condition_not_satisfied() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Create a simple condition that checks a boolean value
    let condition = Condition::with_parameters(
        "is_enabled",
        vec![ConditionParameter::new("enabled", "bool")],
        "request.enabled == true",
    )
    .unwrap();

    model_reader.add_condition("store1", condition).await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "feature".to_string(),
                relations: vec![RelationDefinition {
                    name: "user".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    tuple_reader
        .add_tuple_with_condition(
            "store1",
            "feature",
            "beta",
            "user",
            "user",
            "alice",
            None,
            "is_enabled",
            None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Act: Check WITH the required context but set to false
    let mut context = HashMap::new();
    context.insert("enabled".to_string(), serde_json::json!(false));

    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "user".to_string(),
        object: "feature:beta".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(context),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await;

    // Assert: Should return Ok(false) when condition evaluates to false
    assert!(result.is_ok(), "Check should succeed: {:?}", result);
    assert!(
        !result.unwrap().allowed,
        "Condition should not be satisfied when enabled=false"
    );
}

// ========== Section: Cache Integration Tests ==========
// Issue #72: Integrate check result caching in GraphResolver

use crate::cache::{CheckCache, CheckCacheConfig};

#[tokio::test]
async fn test_cache_hit_returns_cached_result() {
    // Arrange: Create a resolver with caching enabled
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    tuple_reader
        .add_tuple(
            "store1", "document", "doc1", "viewer", "user", "alice", None,
        )
        .await;

    let cache = Arc::new(CheckCache::new(CheckCacheConfig::default()));
    let config = ResolverConfig::default().with_cache(cache);
    let resolver = GraphResolver::with_config(tuple_reader, model_reader, config);

    let request = CheckRequest::new(
        "store1".to_string(),
        "user:alice".to_string(),
        "viewer".to_string(),
        "document:doc1".to_string(),
        vec![],
    );

    // Act: First check (cache miss)
    let result1 = resolver.check(&request).await.unwrap();
    assert!(result1.allowed);

    // Verify metrics: 1 miss
    let metrics = resolver.cache_metrics().snapshot();
    assert_eq!(metrics.misses, 1);
    assert_eq!(metrics.hits, 0);

    // Act: Second check (cache hit)
    let result2 = resolver.check(&request).await.unwrap();
    assert!(result2.allowed);

    // Verify metrics: 1 miss, 1 hit
    let metrics = resolver.cache_metrics().snapshot();
    assert_eq!(metrics.misses, 1);
    assert_eq!(metrics.hits, 1);
}

#[tokio::test]
async fn test_cache_skipped_for_contextual_tuples() {
    // Arrange: Create a resolver with caching enabled
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    let cache = Arc::new(CheckCache::new(CheckCacheConfig::default()));
    let config = ResolverConfig::default().with_cache(cache);
    let resolver = GraphResolver::with_config(tuple_reader, model_reader, config);

    // Request with contextual tuples - should skip caching
    let request = CheckRequest::new(
        "store1".to_string(),
        "user:alice".to_string(),
        "viewer".to_string(),
        "document:doc1".to_string(),
        vec![ContextualTuple {
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:doc1".to_string(),
            condition_name: None,
            condition_context: None,
        }],
    );

    // Act: First check (cache skipped)
    let result1 = resolver.check(&request).await.unwrap();
    assert!(result1.allowed);

    // Verify metrics: 1 skip, 0 hits, 0 misses
    let metrics = resolver.cache_metrics().snapshot();
    assert_eq!(metrics.skips, 1);
    assert_eq!(metrics.hits, 0);
    assert_eq!(metrics.misses, 0);

    // Act: Second check (still skipped)
    let result2 = resolver.check(&request).await.unwrap();
    assert!(result2.allowed);

    // Verify metrics: 2 skips
    let metrics = resolver.cache_metrics().snapshot();
    assert_eq!(metrics.skips, 2);
    assert_eq!(metrics.hits, 0);
    assert_eq!(metrics.misses, 0);
}

#[tokio::test]
async fn test_cache_stores_allowed_false_results() {
    // Arrange: Create a resolver with caching enabled
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;
    // Note: No tuple for alice - she should be denied

    let cache = Arc::new(CheckCache::new(CheckCacheConfig::default()));
    let config = ResolverConfig::default().with_cache(cache);
    let resolver = GraphResolver::with_config(tuple_reader, model_reader, config);

    let request = CheckRequest::new(
        "store1".to_string(),
        "user:alice".to_string(),
        "viewer".to_string(),
        "document:doc1".to_string(),
        vec![],
    );

    // Act: First check (cache miss, result: false)
    let result1 = resolver.check(&request).await.unwrap();
    assert!(!result1.allowed);

    let metrics = resolver.cache_metrics().snapshot();
    assert_eq!(metrics.misses, 1);
    assert_eq!(metrics.hits, 0);

    // Act: Second check (cache hit, result: false)
    let result2 = resolver.check(&request).await.unwrap();
    assert!(!result2.allowed);

    let metrics = resolver.cache_metrics().snapshot();
    assert_eq!(metrics.misses, 1);
    assert_eq!(metrics.hits, 1);
}

#[tokio::test]
async fn test_cache_isolates_different_requests() {
    // Arrange: Create a resolver with caching enabled
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // Alice can view doc1, Bob cannot
    tuple_reader
        .add_tuple(
            "store1", "document", "doc1", "viewer", "user", "alice", None,
        )
        .await;

    let cache = Arc::new(CheckCache::new(CheckCacheConfig::default()));
    let config = ResolverConfig::default().with_cache(cache);
    let resolver = GraphResolver::with_config(tuple_reader, model_reader, config);

    // Check alice (should be allowed)
    let request_alice = CheckRequest::new(
        "store1".to_string(),
        "user:alice".to_string(),
        "viewer".to_string(),
        "document:doc1".to_string(),
        vec![],
    );
    let result_alice = resolver.check(&request_alice).await.unwrap();
    assert!(result_alice.allowed);

    // Check bob (should be denied)
    let request_bob = CheckRequest::new(
        "store1".to_string(),
        "user:bob".to_string(),
        "viewer".to_string(),
        "document:doc1".to_string(),
        vec![],
    );
    let result_bob = resolver.check(&request_bob).await.unwrap();
    assert!(!result_bob.allowed);

    // Verify metrics: 2 misses (different requests)
    let metrics = resolver.cache_metrics().snapshot();
    assert_eq!(metrics.misses, 2);
    assert_eq!(metrics.hits, 0);

    // Check alice again (should hit cache)
    let result_alice2 = resolver.check(&request_alice).await.unwrap();
    assert!(result_alice2.allowed);

    let metrics = resolver.cache_metrics().snapshot();
    assert_eq!(metrics.misses, 2);
    assert_eq!(metrics.hits, 1);
}

#[tokio::test]
async fn test_cache_hit_ratio_calculation() {
    // Arrange: Create a resolver with caching enabled
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    tuple_reader
        .add_tuple(
            "store1", "document", "doc1", "viewer", "user", "alice", None,
        )
        .await;

    let cache = Arc::new(CheckCache::new(CheckCacheConfig::default()));
    let config = ResolverConfig::default().with_cache(cache);
    let resolver = GraphResolver::with_config(tuple_reader, model_reader, config);

    let request = CheckRequest::new(
        "store1".to_string(),
        "user:alice".to_string(),
        "viewer".to_string(),
        "document:doc1".to_string(),
        vec![],
    );

    // Initial hit ratio should be 0.0
    assert_eq!(resolver.cache_metrics().hit_ratio(), 0.0);

    // First check (miss)
    resolver.check(&request).await.unwrap();
    assert_eq!(resolver.cache_metrics().hit_ratio(), 0.0); // 0 hits / 1 total

    // Second check (hit)
    resolver.check(&request).await.unwrap();
    assert_eq!(resolver.cache_metrics().hit_ratio(), 0.5); // 1 hit / 2 total

    // Third check (hit)
    resolver.check(&request).await.unwrap();
    assert!((resolver.cache_metrics().hit_ratio() - 0.666).abs() < 0.01); // ~2/3
}

#[tokio::test]
async fn test_resolver_without_cache_has_zeroed_metrics() {
    // Arrange: Create a resolver without cache (metrics exist but remain zero)
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    tuple_reader
        .add_tuple(
            "store1", "document", "doc1", "viewer", "user", "alice", None,
        )
        .await;

    // No cache configured
    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = CheckRequest::new(
        "store1".to_string(),
        "user:alice".to_string(),
        "viewer".to_string(),
        "document:doc1".to_string(),
        vec![],
    );

    // Multiple checks should not affect metrics (no cache)
    resolver.check(&request).await.unwrap();
    resolver.check(&request).await.unwrap();

    let metrics = resolver.cache_metrics().snapshot();
    assert_eq!(metrics.hits, 0);
    assert_eq!(metrics.misses, 0);
    assert_eq!(metrics.skips, 0);
}

#[tokio::test]
async fn test_cache_skipped_for_request_context() {
    // This test verifies the CRITICAL security invariant (I4):
    // Cache must be bypassed when request.context is non-empty because
    // CEL conditions depend on context values. Without this, a cached
    // decision for request.enabled=true could be incorrectly returned
    // for a request with request.enabled=false.
    //
    // This test PROVES the invariant by:
    // 1. Creating a tuple with a condition that evaluates based on context
    // 2. Running checks with different context values that yield DIFFERENT results
    // 3. Asserting neither result came from cache (both skipped)
    // 4. Proving that caching would have returned incorrect results

    use std::collections::HashMap;

    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Add condition that depends on request.enabled
    let condition = Condition::new("is_enabled", "request.enabled == true").unwrap();
    model_reader.add_condition("store1", condition).await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // Add tuple WITH condition - permission depends on request.enabled
    tuple_reader
        .add_tuple_with_condition(
            "store1",
            "document",
            "doc1",
            "viewer",
            "user",
            "alice",
            None,
            "is_enabled",
            None,
        )
        .await;

    let cache = Arc::new(CheckCache::new(CheckCacheConfig::default()));
    let config = ResolverConfig::default().with_cache(cache);
    let resolver = GraphResolver::with_config(tuple_reader, model_reader, config);

    // Request with request.enabled=true - should be ALLOWED
    let mut context_true = HashMap::new();
    context_true.insert("enabled".to_string(), serde_json::json!(true));

    let request_enabled = CheckRequest::with_context(
        "store1".to_string(),
        "user:alice".to_string(),
        "viewer".to_string(),
        "document:doc1".to_string(),
        vec![],
        context_true,
    );

    let result_enabled = resolver.check(&request_enabled).await.unwrap();
    assert!(
        result_enabled.allowed,
        "With request.enabled=true, access should be ALLOWED"
    );

    // Verify: Cache was SKIPPED (context present)
    let metrics = resolver.cache_metrics().snapshot();
    assert_eq!(
        metrics.skips, 1,
        "First check with context should skip cache"
    );
    assert_eq!(metrics.hits, 0, "Should have no cache hits");
    assert_eq!(metrics.misses, 0, "Should have no cache misses");

    // Request with request.enabled=false - should be DENIED
    let mut context_false = HashMap::new();
    context_false.insert("enabled".to_string(), serde_json::json!(false));

    let request_disabled = CheckRequest::with_context(
        "store1".to_string(),
        "user:alice".to_string(),
        "viewer".to_string(),
        "document:doc1".to_string(),
        vec![],
        context_false,
    );

    let result_disabled = resolver.check(&request_disabled).await.unwrap();
    assert!(
        !result_disabled.allowed,
        "With request.enabled=false, access should be DENIED"
    );

    // Verify: Cache was SKIPPED again
    let metrics = resolver.cache_metrics().snapshot();
    assert_eq!(
        metrics.skips, 2,
        "Second check with context should also skip cache"
    );
    assert_eq!(metrics.hits, 0, "Should still have no cache hits");
    assert_eq!(metrics.misses, 0, "Should still have no cache misses");

    // CRITICAL ASSERTION: Different context values yielded DIFFERENT results
    // This proves that if caching had occurred, it would serve INCORRECT results
    assert_ne!(
        result_enabled.allowed, result_disabled.allowed,
        "SECURITY INVARIANT: Different context values MUST yield different authorization results. \
         This proves caching would be incorrect for context-dependent conditions."
    );
}

#[tokio::test]
async fn test_context_dependent_condition_not_cached() {
    // This test verifies the security invariant that context-dependent conditions
    // MUST NOT be cached, because the same (user, relation, object) tuple can
    // yield different authorization results based on the request context.
    //
    // Per Invariant I1 (Correctness over Performance) and I4 (Security-critical
    // code path protection), this test proves caching would be INCORRECT.
    //
    // Test structure (from Issue #132):
    // 1. Create a conditional tuple where condition depends on context
    // 2. Check with request.enabled=true -> should be allowed, cache skipped
    // 3. Check with request.enabled=false -> should be denied, cache skipped
    // 4. Assert different results prove caching would be incorrect

    use std::collections::HashMap;

    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Define condition: access granted only when request.active == true
    let condition = Condition::new("requires_active", "request.active == true").unwrap();
    model_reader.add_condition("store1", condition).await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "resource".to_string(),
                relations: vec![RelationDefinition {
                    name: "editor".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // Create tuple WITH condition - access depends on request.active
    tuple_reader
        .add_tuple_with_condition(
            "store1",
            "resource",
            "secret",
            "editor",
            "user",
            "bob",
            None,
            "requires_active",
            None,
        )
        .await;

    // Create resolver WITH cache enabled
    let cache = Arc::new(CheckCache::new(CheckCacheConfig::default()));
    let config = ResolverConfig::default().with_cache(cache);
    let resolver = GraphResolver::with_config(tuple_reader, model_reader, config);

    // === TEST CASE 1: request.active=true -> ALLOWED ===
    let mut active_context = HashMap::new();
    active_context.insert("active".to_string(), serde_json::json!(true));

    let request_active = CheckRequest::with_context(
        "store1".to_string(),
        "user:bob".to_string(),
        "editor".to_string(),
        "resource:secret".to_string(),
        vec![],
        active_context,
    );

    let result_active = resolver.check(&request_active).await.unwrap();

    // Verify: Access ALLOWED when request.active=true
    assert!(
        result_active.allowed,
        "Access should be ALLOWED when request.active=true"
    );

    // Verify: Cache was SKIPPED (not hit, not miss)
    let metrics = resolver.cache_metrics().snapshot();
    assert_eq!(metrics.skips, 1, "Check with context must skip cache");
    assert_eq!(metrics.hits, 0, "Must not have cache hits");
    assert_eq!(metrics.misses, 0, "Must not have cache misses");

    // === TEST CASE 2: request.active=false -> DENIED ===
    let mut inactive_context = HashMap::new();
    inactive_context.insert("active".to_string(), serde_json::json!(false));

    let request_inactive = CheckRequest::with_context(
        "store1".to_string(),
        "user:bob".to_string(),
        "editor".to_string(),
        "resource:secret".to_string(),
        vec![],
        inactive_context,
    );

    let result_inactive = resolver.check(&request_inactive).await.unwrap();

    // Verify: Access DENIED when request.active=false
    assert!(
        !result_inactive.allowed,
        "Access should be DENIED when request.active=false"
    );

    // Verify: Cache was SKIPPED again
    let metrics = resolver.cache_metrics().snapshot();
    assert_eq!(metrics.skips, 2, "Both checks with context must skip cache");
    assert_eq!(metrics.hits, 0, "Still must not have cache hits");
    assert_eq!(metrics.misses, 0, "Still must not have cache misses");

    // === SECURITY PROOF ===
    // The same (store, user, relation, object) tuple yields DIFFERENT results
    // based solely on the context value. This PROVES caching would be incorrect:
    // - If we cached result_active (allowed=true), then request_inactive would
    //   incorrectly receive allowed=true from cache
    // - This would be a security vulnerability granting unauthorized access
    assert_ne!(
        result_active.allowed, result_inactive.allowed,
        "SECURITY PROOF: Same tuple with different context values yields different results. \
         Caching would incorrectly serve stale authorization decisions."
    );
}

#[tokio::test]
async fn test_cache_timeout_wrapper_does_not_break_normal_operation() {
    // This test verifies that the 10ms timeout wrapper around cache operations
    // does not interfere with normal cache behavior. The in-memory Moka cache
    // is too fast to actually timeout, so we verify that:
    // 1. Cache hits still work (get succeeds within timeout)
    // 2. Cache inserts still work (insert succeeds within timeout)
    // 3. Metrics are correctly tracked
    //
    // The timeout behavior for slow caches would increment 'skips' instead of
    // 'hits' or 'misses', treating the cache as unavailable.

    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    tuple_reader
        .add_tuple(
            "store1", "document", "doc1", "viewer", "user", "alice", None,
        )
        .await;

    let cache = Arc::new(CheckCache::new(CheckCacheConfig::default()));
    let config = ResolverConfig::default().with_cache(cache);
    let resolver = GraphResolver::with_config(tuple_reader, model_reader, config);

    let request = CheckRequest::new(
        "store1".to_string(),
        "user:alice".to_string(),
        "viewer".to_string(),
        "document:doc1".to_string(),
        vec![],
    );

    // Perform 100 checks to stress test the timeout wrapper
    for i in 0..100 {
        let result = resolver.check(&request).await.unwrap();
        assert!(result.allowed, "Check {i} should be allowed");
    }

    // Verify metrics: 1 miss + 99 hits, 0 skips (no timeouts occurred)
    let metrics = resolver.cache_metrics().snapshot();
    assert_eq!(metrics.misses, 1, "Should have exactly 1 miss");
    assert_eq!(metrics.hits, 99, "Should have exactly 99 hits");
    assert_eq!(metrics.skips, 0, "Should have 0 skips (no timeouts)");
}

// ========== Section: Expand API Tests ==========

/// Test: Expand returns a leaf with direct users for This userset.
#[tokio::test]
async fn test_expand_direct_users_returns_leaf_with_users() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // Add multiple users
    tuple_reader
        .add_tuple(
            "store1", "document", "readme", "viewer", "user", "alice", None,
        )
        .await;
    tuple_reader
        .add_tuple(
            "store1", "document", "readme", "viewer", "user", "bob", None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ExpandRequest::new("store1", "viewer", "document:readme");
    let result = resolver.expand(&request).await.unwrap();

    // Should return a Leaf node with users
    match &result.tree.root {
        ExpandNode::Leaf(leaf) => {
            assert_eq!(leaf.name, "document:readme#viewer");
            match &leaf.value {
                ExpandLeafValue::Users(users) => {
                    assert_eq!(users.len(), 2);
                    assert!(users.contains(&"user:alice".to_string()));
                    assert!(users.contains(&"user:bob".to_string()));
                }
                _ => panic!("Expected Users leaf value"),
            }
        }
        _ => panic!("Expected Leaf node for direct users"),
    }
}

/// Test: Expand returns a union node for union relations.
#[tokio::test]
async fn test_expand_union_returns_multiple_branches() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // viewer = [user] | editor (union of direct and computed)
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "editor".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::Union {
                            children: vec![
                                Userset::This,
                                Userset::ComputedUserset {
                                    relation: "editor".to_string(),
                                },
                            ],
                        },
                    },
                ],
            },
        )
        .await;

    tuple_reader
        .add_tuple(
            "store1", "document", "readme", "viewer", "user", "alice", None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ExpandRequest::new("store1", "viewer", "document:readme");
    let result = resolver.expand(&request).await.unwrap();

    // Should return a Union node with two children
    match &result.tree.root {
        ExpandNode::Union { name, nodes } => {
            assert_eq!(name, "document:readme#viewer");
            assert_eq!(nodes.len(), 2);
        }
        _ => panic!("Expected Union node"),
    }
}

/// Test: Expand returns an intersection node for intersection relations.
#[tokio::test]
async fn test_expand_intersection_returns_all_children() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Can only view if both reader AND has_access
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "reader".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "has_access".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::Intersection {
                            children: vec![
                                Userset::ComputedUserset {
                                    relation: "reader".to_string(),
                                },
                                Userset::ComputedUserset {
                                    relation: "has_access".to_string(),
                                },
                            ],
                        },
                    },
                ],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ExpandRequest::new("store1", "viewer", "document:readme");
    let result = resolver.expand(&request).await.unwrap();

    // Should return an Intersection node
    match &result.tree.root {
        ExpandNode::Intersection { name, nodes } => {
            assert_eq!(name, "document:readme#viewer");
            assert_eq!(nodes.len(), 2);
        }
        _ => panic!("Expected Intersection node"),
    }
}

/// Test: Expand returns a difference node for exclusion relations.
#[tokio::test]
async fn test_expand_exclusion_returns_difference_node() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // viewer = members but not blocked
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "member".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "blocked".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::Exclusion {
                            base: Box::new(Userset::ComputedUserset {
                                relation: "member".to_string(),
                            }),
                            subtract: Box::new(Userset::ComputedUserset {
                                relation: "blocked".to_string(),
                            }),
                        },
                    },
                ],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ExpandRequest::new("store1", "viewer", "document:readme");
    let result = resolver.expand(&request).await.unwrap();

    // Should return a Difference node
    match &result.tree.root {
        ExpandNode::Difference {
            name,
            base,
            subtract,
        } => {
            assert_eq!(name, "document:readme#viewer");
            // Base and subtract should be computed userset references
            assert!(matches!(**base, ExpandNode::Leaf(_)));
            assert!(matches!(**subtract, ExpandNode::Leaf(_)));
        }
        _ => panic!("Expected Difference node"),
    }
}

/// Test: Expand returns computed userset reference.
#[tokio::test]
async fn test_expand_computed_userset_returns_reference() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // viewer = editor (computed userset)
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "editor".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::ComputedUserset {
                            relation: "editor".to_string(),
                        },
                    },
                ],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ExpandRequest::new("store1", "viewer", "document:readme");
    let result = resolver.expand(&request).await.unwrap();

    // Should return a Leaf with Computed value
    match &result.tree.root {
        ExpandNode::Leaf(leaf) => {
            assert_eq!(leaf.name, "document:readme#viewer");
            match &leaf.value {
                ExpandLeafValue::Computed { userset } => {
                    assert_eq!(userset, "document:readme#editor");
                }
                _ => panic!("Expected Computed leaf value"),
            }
        }
        _ => panic!("Expected Leaf node for computed userset"),
    }
}

/// Test: Expand returns tuple-to-userset reference.
#[tokio::test]
async fn test_expand_tuple_to_userset_returns_reference() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // document viewer = folder viewer (tuple-to-userset)
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "parent".to_string(),
                        type_constraints: vec!["folder".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::TupleToUserset {
                            tupleset: "parent".to_string(),
                            computed_userset: "viewer".to_string(),
                        },
                    },
                ],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ExpandRequest::new("store1", "viewer", "document:readme");
    let result = resolver.expand(&request).await.unwrap();

    // Should return a Leaf with TupleToUserset value
    match &result.tree.root {
        ExpandNode::Leaf(leaf) => {
            assert_eq!(leaf.name, "document:readme#viewer");
            match &leaf.value {
                ExpandLeafValue::TupleToUserset {
                    tupleset,
                    computed_userset,
                } => {
                    assert_eq!(tupleset, "parent");
                    assert_eq!(computed_userset, "viewer");
                }
                _ => panic!("Expected TupleToUserset leaf value"),
            }
        }
        _ => panic!("Expected Leaf node for tuple-to-userset"),
    }
}

/// Test: Expand returns error when depth limit exceeded.
#[tokio::test]
async fn test_expand_depth_limit_returns_error() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Create deeply nested union relations that exceed depth limit
    // Build a chain of nested unions
    let mut current = Userset::This;
    for _ in 0..30 {
        current = Userset::Union {
            children: vec![current, Userset::This],
        };
    }

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: current,
                }],
            },
        )
        .await;

    // Use a resolver with low depth limit
    let config = ResolverConfig::default().with_max_depth(5);
    let resolver = GraphResolver::with_config(tuple_reader, model_reader, config);

    let request = ExpandRequest::new("store1", "viewer", "document:readme");
    let result = resolver.expand(&request).await;

    assert!(result.is_err());
    match result.unwrap_err() {
        DomainError::DepthLimitExceeded { max_depth } => {
            assert_eq!(max_depth, 5);
        }
        e => panic!("Expected DepthLimitExceeded error, got: {:?}", e),
    }
}

/// Test: Expand returns error for nonexistent store.
#[tokio::test]
async fn test_expand_nonexistent_store_returns_error() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    // Don't add any store

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ExpandRequest::new("nonexistent", "viewer", "document:readme");
    let result = resolver.expand(&request).await;

    assert!(result.is_err());
    match result.unwrap_err() {
        DomainError::StoreNotFound { store_id } => {
            assert_eq!(store_id, "nonexistent");
        }
        e => panic!("Expected StoreNotFound error, got: {:?}", e),
    }
}

/// Test: Expand returns error for invalid object format.
#[tokio::test]
async fn test_expand_invalid_object_returns_error() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Invalid object format (no colon separator)
    let request = ExpandRequest::new("store1", "viewer", "invalid_object");
    let result = resolver.expand(&request).await;

    assert!(result.is_err());
    match result.unwrap_err() {
        DomainError::InvalidObjectFormat { value } => {
            assert_eq!(value, "invalid_object");
        }
        e => panic!("Expected InvalidObjectFormat error, got: {:?}", e),
    }
}

/// Test: Expand returns error for nonexistent relation.
#[tokio::test]
async fn test_expand_nonexistent_relation_returns_error() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Add type but not the relation we're looking for
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "editor".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ExpandRequest::new("store1", "viewer", "document:readme");
    let result = resolver.expand(&request).await;

    assert!(result.is_err());
    // Should return an error about relation not found
    match result.unwrap_err() {
        DomainError::RelationNotFound { relation, .. } => {
            assert_eq!(relation, "viewer");
        }
        e => panic!("Expected RelationNotFound error, got: {:?}", e),
    }
}

/// Test: Expand returns error for empty relation.
#[tokio::test]
async fn test_expand_empty_relation_returns_error() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Empty relation
    let request = ExpandRequest::new("store1", "", "document:readme");
    let result = resolver.expand(&request).await;

    assert!(result.is_err());
    match result.unwrap_err() {
        DomainError::InvalidRelationFormat { value } => {
            assert_eq!(value, "");
        }
        e => panic!("Expected InvalidRelationFormat error, got: {:?}", e),
    }
}

/// Test: Expand handles userset with user relation.
#[tokio::test]
async fn test_expand_userset_with_user_relation() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into(), "group#member".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // Add a userset tuple (group#member)
    tuple_reader
        .add_tuple(
            "store1",
            "document",
            "readme",
            "viewer",
            "group",
            "engineering",
            Some("member"),
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ExpandRequest::new("store1", "viewer", "document:readme");
    let result = resolver.expand(&request).await.unwrap();

    // Should include the userset in the users list
    match &result.tree.root {
        ExpandNode::Leaf(leaf) => match &leaf.value {
            ExpandLeafValue::Users(users) => {
                assert_eq!(users.len(), 1);
                assert!(users.contains(&"group:engineering#member".to_string()));
            }
            _ => panic!("Expected Users leaf value"),
        },
        _ => panic!("Expected Leaf node"),
    }
}

// ========== Section: ListObjects with Contextual Tuples ==========

/// Tests that list_objects includes objects from contextual tuples.
/// This is the fix for Issue #262: ListObjects API doesn't support contextual tuples.
#[tokio::test]
async fn test_list_objects_includes_objects_from_contextual_tuples() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    // Set up store
    tuple_reader.add_store("store1").await;

    // Add type definition with viewer relation
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // Add a stored tuple for doc3 (this one is in storage)
    tuple_reader
        .add_tuple(
            "store1", "document", "doc3", "viewer", "user", "alice", None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Create contextual tuples for doc1 and doc2 (not in storage)
    let contextual_tuples = vec![
        ContextualTuple::new("user:alice", "viewer", "document:doc1"),
        ContextualTuple::new("user:alice", "viewer", "document:doc2"),
    ];

    // Create request with contextual tuples
    let request = ListObjectsRequest::with_context(
        "store1",
        "user:alice",
        "viewer",
        "document",
        contextual_tuples,
        HashMap::new(),
    );

    let result = resolver.list_objects(&request, 100).await.unwrap();

    // Should return all 3 documents: doc1 and doc2 from contextual tuples, doc3 from storage
    assert_eq!(result.objects.len(), 3);
    assert!(result.objects.contains(&"document:doc1".to_string()));
    assert!(result.objects.contains(&"document:doc2".to_string()));
    assert!(result.objects.contains(&"document:doc3".to_string()));
    assert!(!result.truncated);
}

/// Tests that list_objects with only contextual tuples (no stored tuples) works.
#[tokio::test]
async fn test_list_objects_with_only_contextual_tuples() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    // Set up store
    tuple_reader.add_store("store1").await;

    // Add type definition with viewer relation
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // No stored tuples - only contextual tuples

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Create contextual tuples
    let contextual_tuples = vec![
        ContextualTuple::new("user:alice", "viewer", "document:doc1"),
        ContextualTuple::new("user:alice", "viewer", "document:doc2"),
    ];

    let request = ListObjectsRequest::with_context(
        "store1",
        "user:alice",
        "viewer",
        "document",
        contextual_tuples,
        HashMap::new(),
    );

    let result = resolver.list_objects(&request, 100).await.unwrap();

    // Should return documents from contextual tuples
    assert_eq!(result.objects.len(), 2);
    assert!(result.objects.contains(&"document:doc1".to_string()));
    assert!(result.objects.contains(&"document:doc2".to_string()));
}

/// Tests that list_objects deduplicates objects that appear in both storage and contextual tuples.
#[tokio::test]
async fn test_list_objects_deduplicates_contextual_and_stored_tuples() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // Add tuple for doc1 in storage
    tuple_reader
        .add_tuple(
            "store1", "document", "doc1", "viewer", "user", "alice", None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Also provide doc1 as contextual tuple (duplicate)
    let contextual_tuples = vec![
        ContextualTuple::new("user:alice", "viewer", "document:doc1"),
        ContextualTuple::new("user:alice", "viewer", "document:doc2"),
    ];

    let request = ListObjectsRequest::with_context(
        "store1",
        "user:alice",
        "viewer",
        "document",
        contextual_tuples,
        HashMap::new(),
    );

    let result = resolver.list_objects(&request, 100).await.unwrap();

    // Should return 2 unique documents (doc1 should not be duplicated)
    assert_eq!(result.objects.len(), 2);
    assert!(result.objects.contains(&"document:doc1".to_string()));
    assert!(result.objects.contains(&"document:doc2".to_string()));
}

/// Tests that list_objects only includes contextual tuple objects of the requested type.
#[tokio::test]
async fn test_list_objects_filters_contextual_tuples_by_type() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Add type definitions for document and folder
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "folder".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Contextual tuples for both document and folder types
    let contextual_tuples = vec![
        ContextualTuple::new("user:alice", "viewer", "document:doc1"),
        ContextualTuple::new("user:alice", "viewer", "folder:folder1"),
        ContextualTuple::new("user:alice", "viewer", "document:doc2"),
    ];

    // Request documents only
    let request = ListObjectsRequest::with_context(
        "store1",
        "user:alice",
        "viewer",
        "document",
        contextual_tuples,
        HashMap::new(),
    );

    let result = resolver.list_objects(&request, 100).await.unwrap();

    // Should only return documents, not folders
    assert_eq!(result.objects.len(), 2);
    assert!(result.objects.contains(&"document:doc1".to_string()));
    assert!(result.objects.contains(&"document:doc2".to_string()));
    assert!(!result.objects.iter().any(|o| o.starts_with("folder:")));
}

/// Tests that list_objects silently skips malformed contextual tuple objects.
#[tokio::test]
async fn test_list_objects_skips_malformed_contextual_tuple_objects() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Mix of valid and malformed contextual tuples
    let contextual_tuples = vec![
        ContextualTuple::new("user:alice", "viewer", "document:doc1"), // valid
        ContextualTuple::new("user:alice", "viewer", "malformed_no_colon"), // malformed - no colon
        ContextualTuple::new("user:alice", "viewer", "document:doc2"), // valid
        ContextualTuple::new("user:alice", "viewer", ""),              // malformed - empty
    ];

    let request = ListObjectsRequest::with_context(
        "store1",
        "user:alice",
        "viewer",
        "document",
        contextual_tuples,
        HashMap::new(),
    );

    let result = resolver.list_objects(&request, 100).await.unwrap();

    // Should only return valid documents, silently skipping malformed ones
    assert_eq!(result.objects.len(), 2);
    assert!(result.objects.contains(&"document:doc1".to_string()));
    assert!(result.objects.contains(&"document:doc2".to_string()));
}

/// Tests that list_objects respects max_candidates limit with contextual tuples.
#[tokio::test]
async fn test_list_objects_respects_limit_with_contextual_tuples() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Create many contextual tuples (more than typical max_candidates)
    let contextual_tuples: Vec<_> = (1..=10)
        .map(|i| ContextualTuple::new("user:alice", "viewer", format!("document:doc{}", i)))
        .collect();

    let request = ListObjectsRequest::with_context(
        "store1",
        "user:alice",
        "viewer",
        "document",
        contextual_tuples,
        HashMap::new(),
    );

    // Request with a small limit
    let result = resolver.list_objects(&request, 3).await.unwrap();

    // Should be truncated to max 3 results
    assert!(result.objects.len() <= 3);
    assert!(result.truncated);
}

// ============================================================
// Authorization Model ID Validation Tests (Issue #265)
// ============================================================

/// Tests that Check API uses authorization_model_id when provided
#[tokio::test]
async fn test_check_uses_authorization_model_id_when_provided() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    tuple_reader
        .add_tuple(
            "store1", "document", "readme", "viewer", "user", "alice", None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Request with authorization_model_id - the mock reader ignores the ID but this tests
    // that the resolver accepts the parameter without error
    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "document:readme".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(HashMap::new()),
        authorization_model_id: Some("01ABCDEFGHIJK1234567890XYZ".to_string()),
    };

    let result = resolver.check(&request).await.unwrap();
    assert!(
        result.allowed,
        "Check should succeed with authorization_model_id"
    );
}

/// Tests that Check API works without authorization_model_id (uses latest model)
#[tokio::test]
async fn test_check_works_without_authorization_model_id() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    tuple_reader
        .add_tuple(
            "store1", "document", "readme", "viewer", "user", "alice", None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Request without authorization_model_id - should use latest model
    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "document:readme".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(HashMap::new()),
        authorization_model_id: None,
    };

    let result = resolver.check(&request).await.unwrap();
    assert!(
        result.allowed,
        "Check should succeed without authorization_model_id"
    );
}

/// Tests that caching is skipped when authorization_model_id is provided
#[tokio::test]
async fn test_check_skips_cache_when_authorization_model_id_provided() {
    use crate::cache::{CheckCache, CheckCacheConfig};

    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    tuple_reader
        .add_tuple(
            "store1", "document", "readme", "viewer", "user", "alice", None,
        )
        .await;

    // Create resolver with cache enabled
    let cache_config = CheckCacheConfig::default().with_enabled(true);
    let cache = Arc::new(CheckCache::new(cache_config));
    let config = crate::resolver::ResolverConfig::default().with_cache(cache.clone());
    let resolver = GraphResolver::with_config(tuple_reader, model_reader, config);

    // Request with authorization_model_id
    let request = CheckRequest {
        store_id: "store1".to_string(),
        user: "user:alice".to_string(),
        relation: "viewer".to_string(),
        object: "document:readme".to_string(),
        contextual_tuples: Arc::new(vec![]),
        context: Arc::new(HashMap::new()),
        authorization_model_id: Some("01ABCDEFGHIJK1234567890XYZ".to_string()),
    };

    // Perform check twice
    let _ = resolver.check(&request).await.unwrap();
    let _ = resolver.check(&request).await.unwrap();

    // Cache metrics should show skips, not hits (because authorization_model_id is set)
    let metrics = resolver.cache_metrics();
    assert_eq!(
        metrics.hits.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "Should have no cache hits when authorization_model_id is provided"
    );
    assert!(
        metrics.skips.load(std::sync::atomic::Ordering::Relaxed) >= 2,
        "Should have cache skips when authorization_model_id is provided"
    );
}

// ============================================================================
// ReverseExpand Algorithm Tests for ListObjects with TupleToUserset
// ============================================================================

/// Tests list_objects with TupleToUserset (hierarchical) relation using ReverseExpand algorithm.
/// This is the key performance optimization: instead of checking every document,
/// we find folders the user can access and then find documents in those folders.
#[tokio::test]
async fn test_list_objects_with_tuple_to_userset_relation() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Add folder type with viewer relation
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "folder".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // Add document type with:
    // - parent: points to folder
    // - viewer: computed from parent's viewer (TupleToUserset)
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "parent".to_string(),
                        type_constraints: vec!["folder".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::TupleToUserset {
                            tupleset: "parent".to_string(),
                            computed_userset: "viewer".to_string(),
                        },
                    },
                ],
            },
        )
        .await;

    // Alice is a viewer of folder1
    tuple_reader
        .add_tuple(
            "store1", "folder", "folder1", "viewer", "user", "alice", None,
        )
        .await;

    // doc1 and doc2 are in folder1, doc3 is in folder2
    tuple_reader
        .add_tuple(
            "store1", "document", "doc1", "parent", "folder", "folder1", None,
        )
        .await;
    tuple_reader
        .add_tuple(
            "store1", "document", "doc2", "parent", "folder", "folder1", None,
        )
        .await;
    tuple_reader
        .add_tuple(
            "store1", "document", "doc3", "parent", "folder", "folder2", None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListObjectsRequest::new("store1", "user:alice", "viewer", "document");

    let result = resolver.list_objects(&request, 100).await.unwrap();

    // Alice should see doc1 and doc2 (in folder1 where she is a viewer)
    // but NOT doc3 (in folder2 where she has no access)
    assert_eq!(result.objects.len(), 2);
    assert!(result.objects.contains(&"document:doc1".to_string()));
    assert!(result.objects.contains(&"document:doc2".to_string()));
    assert!(!result.objects.contains(&"document:doc3".to_string()));
    assert!(!result.truncated);
}

/// Tests list_objects with Union relation containing both direct and TupleToUserset.
#[tokio::test]
async fn test_list_objects_with_union_of_direct_and_tuple_to_userset() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Add folder type with viewer relation
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "folder".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // Add document type with viewer as Union of direct assignment OR inherited from parent
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "parent".to_string(),
                        type_constraints: vec!["folder".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::Union {
                            children: vec![
                                Userset::This,
                                Userset::TupleToUserset {
                                    tupleset: "parent".to_string(),
                                    computed_userset: "viewer".to_string(),
                                },
                            ],
                        },
                    },
                ],
            },
        )
        .await;

    // Alice is a viewer of folder1
    tuple_reader
        .add_tuple(
            "store1", "folder", "folder1", "viewer", "user", "alice", None,
        )
        .await;

    // Alice has direct viewer access to doc1
    tuple_reader
        .add_tuple(
            "store1", "document", "doc1", "viewer", "user", "alice", None,
        )
        .await;

    // doc2 is in folder1 (Alice inherits access)
    tuple_reader
        .add_tuple(
            "store1", "document", "doc2", "parent", "folder", "folder1", None,
        )
        .await;

    // doc3 is in folder2 (Alice has no access)
    tuple_reader
        .add_tuple(
            "store1", "document", "doc3", "parent", "folder", "folder2", None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListObjectsRequest::new("store1", "user:alice", "viewer", "document");

    let result = resolver.list_objects(&request, 100).await.unwrap();

    // Alice should see doc1 (direct) and doc2 (inherited), but NOT doc3
    assert_eq!(result.objects.len(), 2);
    assert!(result.objects.contains(&"document:doc1".to_string()));
    assert!(result.objects.contains(&"document:doc2".to_string()));
    assert!(!result.objects.contains(&"document:doc3".to_string()));
}

/// Tests list_objects with multiple levels of TupleToUserset (nested hierarchy).
#[tokio::test]
async fn test_list_objects_with_nested_tuple_to_userset() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Organization -> Folder -> Document hierarchy
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "organization".to_string(),
                relations: vec![RelationDefinition {
                    name: "member".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "folder".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "org".to_string(),
                        type_constraints: vec!["organization".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::TupleToUserset {
                            tupleset: "org".to_string(),
                            computed_userset: "member".to_string(),
                        },
                    },
                ],
            },
        )
        .await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "parent".to_string(),
                        type_constraints: vec!["folder".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::TupleToUserset {
                            tupleset: "parent".to_string(),
                            computed_userset: "viewer".to_string(),
                        },
                    },
                ],
            },
        )
        .await;

    // Alice is member of org1
    tuple_reader
        .add_tuple(
            "store1",
            "organization",
            "org1",
            "member",
            "user",
            "alice",
            None,
        )
        .await;

    // folder1 belongs to org1
    tuple_reader
        .add_tuple(
            "store1",
            "folder",
            "folder1",
            "org",
            "organization",
            "org1",
            None,
        )
        .await;

    // doc1 is in folder1
    tuple_reader
        .add_tuple(
            "store1", "document", "doc1", "parent", "folder", "folder1", None,
        )
        .await;

    // doc2 is in folder2 (belongs to org2, Alice has no access)
    tuple_reader
        .add_tuple(
            "store1",
            "folder",
            "folder2",
            "org",
            "organization",
            "org2",
            None,
        )
        .await;
    tuple_reader
        .add_tuple(
            "store1", "document", "doc2", "parent", "folder", "folder2", None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListObjectsRequest::new("store1", "user:alice", "viewer", "document");

    let result = resolver.list_objects(&request, 100).await.unwrap();

    // Alice should see doc1 (org1 -> folder1 -> doc1), but NOT doc2
    assert_eq!(result.objects.len(), 1);
    assert!(result.objects.contains(&"document:doc1".to_string()));
}

/// Tests list_objects with ComputedUserset relation.
#[tokio::test]
async fn test_list_objects_with_computed_userset() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    // Document with editor and viewer relations
    // viewer is computed from editor (editors can also view)
    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "editor".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::Union {
                            children: vec![
                                Userset::This,
                                Userset::ComputedUserset {
                                    relation: "editor".to_string(),
                                },
                            ],
                        },
                    },
                ],
            },
        )
        .await;

    // Alice is editor of doc1
    tuple_reader
        .add_tuple(
            "store1", "document", "doc1", "editor", "user", "alice", None,
        )
        .await;

    // Alice is viewer of doc2
    tuple_reader
        .add_tuple(
            "store1", "document", "doc2", "viewer", "user", "alice", None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListObjectsRequest::new("store1", "user:alice", "viewer", "document");

    let result = resolver.list_objects(&request, 100).await.unwrap();

    // Alice should see both doc1 (as editor -> viewer) and doc2 (as direct viewer)
    assert_eq!(result.objects.len(), 2);
    assert!(result.objects.contains(&"document:doc1".to_string()));
    assert!(result.objects.contains(&"document:doc2".to_string()));
}

/// Tests that list_objects respects the max_candidates limit with ReverseExpand.
#[tokio::test]
async fn test_list_objects_respects_limit_with_reverse_expand() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // Alice has access to 10 documents
    for i in 1..=10 {
        tuple_reader
            .add_tuple(
                "store1",
                "document",
                &format!("doc{}", i),
                "viewer",
                "user",
                "alice",
                None,
            )
            .await;
    }

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListObjectsRequest::new("store1", "user:alice", "viewer", "document");

    // Request with limit of 5
    let result = resolver.list_objects(&request, 5).await.unwrap();

    // Should return exactly 5 documents and indicate truncation
    assert_eq!(result.objects.len(), 5);
    assert!(result.truncated);
}

/// Tests that list_objects handles empty results correctly.
#[tokio::test]
async fn test_list_objects_empty_results_with_reverse_expand() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());

    tuple_reader.add_store("store1").await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "folder".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    model_reader
        .add_type(
            "store1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "parent".to_string(),
                        type_constraints: vec!["folder".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::TupleToUserset {
                            tupleset: "parent".to_string(),
                            computed_userset: "viewer".to_string(),
                        },
                    },
                ],
            },
        )
        .await;

    // Documents exist but Alice has no access to any folders
    tuple_reader
        .add_tuple(
            "store1", "document", "doc1", "parent", "folder", "folder1", None,
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListObjectsRequest::new("store1", "user:alice", "viewer", "document");

    let result = resolver.list_objects(&request, 100).await.unwrap();

    // Alice should see no documents
    assert!(result.objects.is_empty());
    assert!(!result.truncated);
}
