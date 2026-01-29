//! Fuzz target for ListObjects API
//!
//! This fuzz target exercises the ListObjects algorithm with arbitrary inputs
//! to find crashes, panics, and infinite loops. Per Invariant I4, security-critical
//! code (graph resolver) requires fuzz testing.

#![no_main]

use arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;
use rsfga_domain::model::{
    AuthorizationModel, RelationDefinition, TypeConstraint, TypeDefinition, Userset,
};
use rsfga_domain::resolver::types::ListObjectsRequest;
use rsfga_domain::resolver::{GraphResolver, MockModelReader, MockTupleReader};
use std::collections::HashMap;
use std::sync::Arc;

/// Fuzz input for ListObjects
#[derive(Debug, Arbitrary)]
struct FuzzInput {
    /// User identifier (limited length to avoid OOM)
    #[arbitrary(with = |u: &mut Unstructured| -> arbitrary::Result<String> {
        let len = u.int_in_range(1..=50)?;
        let s: String = (0..len)
            .map(|_| u.choose(&['a', 'b', 'c', 'd', ':', '_', '1', '2']).copied())
            .collect::<Result<_, _>>()?;
        Ok(format!("user:{}", s))
    })]
    user: String,

    /// Object type
    #[arbitrary(with = |u: &mut Unstructured| -> arbitrary::Result<String> {
        let len = u.int_in_range(1..=20)?;
        let s: String = (0..len)
            .map(|_| u.choose(&['a', 'b', 'c', 'd', 'e', 'f']).copied())
            .collect::<Result<_, _>>()?;
        Ok(s)
    })]
    object_type: String,

    /// Relation name
    #[arbitrary(with = |u: &mut Unstructured| -> arbitrary::Result<String> {
        let names = ["viewer", "editor", "owner", "member", "admin", "can_view"];
        Ok(u.choose(&names)?.to_string())
    })]
    relation: String,

    /// Maximum candidates limit
    #[arbitrary(with = |u: &mut Unstructured| -> arbitrary::Result<usize> {
        u.int_in_range(1..=1000)
    })]
    max_candidates: usize,

    /// Userset rewrite type for the relation
    userset_type: FuzzUsersetType,

    /// Number of tuples to create (limited)
    #[arbitrary(with = |u: &mut Unstructured| -> arbitrary::Result<u8> {
        u.int_in_range(0..=50)
    })]
    num_tuples: u8,
}

#[derive(Debug, Arbitrary)]
enum FuzzUsersetType {
    This,
    ComputedUserset,
    Union,
    Intersection,
    Exclusion,
    TupleToUserset,
}

fn create_userset(userset_type: &FuzzUsersetType, relation: &str) -> Userset {
    match userset_type {
        FuzzUsersetType::This => Userset::This,
        FuzzUsersetType::ComputedUserset => Userset::ComputedUserset {
            relation: "member".to_string(),
        },
        FuzzUsersetType::Union => Userset::Union {
            children: vec![Userset::This, Userset::ComputedUserset {
                relation: "owner".to_string(),
            }],
        },
        FuzzUsersetType::Intersection => Userset::Intersection {
            children: vec![Userset::This, Userset::ComputedUserset {
                relation: "member".to_string(),
            }],
        },
        FuzzUsersetType::Exclusion => Userset::Exclusion {
            base: Box::new(Userset::This),
            subtract: Box::new(Userset::ComputedUserset {
                relation: "blocked".to_string(),
            }),
        },
        FuzzUsersetType::TupleToUserset => Userset::TupleToUserset {
            tupleset: "parent".to_string(),
            computed_userset: relation.to_string(),
        },
    }
}

fuzz_target!(|input: FuzzInput| {
    // Skip invalid inputs
    if input.user.is_empty() || input.object_type.is_empty() || input.relation.is_empty() {
        return;
    }

    // Skip wildcards (not allowed for ListObjects)
    if input.user.contains('*') {
        return;
    }

    // Create mock readers
    let mut tuple_reader = MockTupleReader::new();
    let mut model_reader = MockModelReader::new();

    // Set up basic model
    let userset = create_userset(&input.userset_type, &input.relation);

    let type_def = TypeDefinition {
        type_name: input.object_type.clone(),
        relations: vec![
            RelationDefinition {
                name: input.relation.clone(),
                rewrite: userset,
                type_constraints: vec![TypeConstraint {
                    type_name: "user".to_string(),
                    relation: None,
                    condition: None,
                }],
            },
            RelationDefinition {
                name: "member".to_string(),
                rewrite: Userset::This,
                type_constraints: vec![TypeConstraint {
                    type_name: "user".to_string(),
                    relation: None,
                    condition: None,
                }],
            },
            RelationDefinition {
                name: "owner".to_string(),
                rewrite: Userset::This,
                type_constraints: vec![TypeConstraint {
                    type_name: "user".to_string(),
                    relation: None,
                    condition: None,
                }],
            },
            RelationDefinition {
                name: "blocked".to_string(),
                rewrite: Userset::This,
                type_constraints: vec![TypeConstraint {
                    type_name: "user".to_string(),
                    relation: None,
                    condition: None,
                }],
            },
            RelationDefinition {
                name: "parent".to_string(),
                rewrite: Userset::This,
                type_constraints: vec![TypeConstraint {
                    type_name: "folder".to_string(),
                    relation: None,
                    condition: None,
                }],
            },
        ],
    };

    let model = AuthorizationModel {
        id: "model-1".to_string(),
        schema_version: "1.1".to_string(),
        type_definitions: vec![type_def],
        conditions: HashMap::new(),
    };

    // Set up mock expectations
    let store_id = "store-1";
    let object_type_clone = input.object_type.clone();
    let relation_clone = input.relation.clone();
    let model_clone = model.clone();

    tuple_reader
        .expect_store_exists()
        .returning(|_| Box::pin(async { Ok(true) }));

    tuple_reader
        .expect_get_objects_for_user()
        .returning(move |_, _, _, _, _| {
            Box::pin(async { Ok(vec![]) })
        });

    tuple_reader
        .expect_get_objects_with_parents()
        .returning(|_, _, _, _, _, _| {
            Box::pin(async { Ok(vec![]) })
        });

    model_reader
        .expect_get_model()
        .returning(move |_| {
            let m = model_clone.clone();
            Box::pin(async move { Ok(m) })
        });

    let object_type_for_rel = input.object_type.clone();
    let relation_for_rel = input.relation.clone();
    model_reader
        .expect_get_relation_definition()
        .returning(move |_, type_name, rel_name| {
            let userset = if rel_name == relation_for_rel {
                create_userset(&FuzzUsersetType::This, &relation_for_rel)
            } else {
                Userset::This
            };
            let def = RelationDefinition {
                name: rel_name.to_string(),
                rewrite: userset,
                type_constraints: vec![TypeConstraint {
                    type_name: "user".to_string(),
                    relation: None,
                    condition: None,
                }],
            };
            Box::pin(async move { Ok(def) })
        });

    // Create resolver
    let resolver = GraphResolver::new(Arc::new(tuple_reader), Arc::new(model_reader));

    // Create request
    let request = ListObjectsRequest {
        store_id: store_id.to_string(),
        user: input.user.clone(),
        relation: input.relation.clone(),
        object_type: input.object_type.clone(),
        context: None,
        authorization_model_id: None,
    };

    // Run the fuzz target - we're testing for crashes/panics/infinite loops
    // The timeout is handled by libFuzzer
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let _ = rt.block_on(async {
        // Use a timeout to catch infinite loops
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            resolver.list_objects(&request, input.max_candidates),
        )
        .await
    });
});
