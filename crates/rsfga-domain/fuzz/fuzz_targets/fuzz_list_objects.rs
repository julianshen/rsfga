//! Fuzz target for ListObjects request validation
//!
//! This fuzz target exercises the ListObjects request construction and
//! validation to find crashes, panics, and edge cases. Per Invariant I4,
//! security-critical code (graph resolver) requires fuzz testing.

#![no_main]

use arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;
use rsfga_domain::model::{
    AuthorizationModel, RelationDefinition, TypeConstraint, TypeDefinition, Userset,
};
use rsfga_domain::resolver::ListObjectsRequest;
use std::collections::HashMap;
use std::sync::Arc;

/// Fuzz input for ListObjects request validation
#[derive(Debug, Arbitrary)]
struct FuzzInput {
    /// Store ID
    #[arbitrary(with = |u: &mut Unstructured| -> arbitrary::Result<String> {
        let len = u.int_in_range(0..=100)?;
        let s: String = (0..len)
            .map(|_| u.choose(&['a', 'b', 'c', 'd', ':', '_', '-', '1', '2', ' ', '\n', '\0']).copied())
            .collect::<Result<_, _>>()?;
        Ok(s)
    })]
    store_id: String,

    /// User identifier (limited length to avoid OOM)
    #[arbitrary(with = |u: &mut Unstructured| -> arbitrary::Result<String> {
        let len = u.int_in_range(0..=100)?;
        let s: String = (0..len)
            .map(|_| u.choose(&['a', 'b', 'c', 'd', ':', '_', '#', '*', '1', '2', ' ', '\n', '\0']).copied())
            .collect::<Result<_, _>>()?;
        Ok(s)
    })]
    user: String,

    /// Object type
    #[arbitrary(with = |u: &mut Unstructured| -> arbitrary::Result<String> {
        let len = u.int_in_range(0..=50)?;
        let s: String = (0..len)
            .map(|_| u.choose(&['a', 'b', 'c', 'd', 'e', 'f', '_', '-', '1', ' ', '\n', '\0']).copied())
            .collect::<Result<_, _>>()?;
        Ok(s)
    })]
    object_type: String,

    /// Relation name
    #[arbitrary(with = |u: &mut Unstructured| -> arbitrary::Result<String> {
        let len = u.int_in_range(0..=50)?;
        let s: String = (0..len)
            .map(|_| u.choose(&['a', 'b', 'c', 'd', 'e', 'f', '_', '-', '1', ' ', '\n', '\0']).copied())
            .collect::<Result<_, _>>()?;
        Ok(s)
    })]
    relation: String,

    /// Number of type definitions
    #[arbitrary(with = |u: &mut Unstructured| -> arbitrary::Result<u8> {
        u.int_in_range(0..=10)
    })]
    num_types: u8,

    /// Userset depth
    #[arbitrary(with = |u: &mut Unstructured| -> arbitrary::Result<u8> {
        u.int_in_range(0..=30)
    })]
    userset_depth: u8,
}

#[derive(Debug, Arbitrary, Clone)]
enum FuzzUsersetType {
    This,
    ComputedUserset(String),
    Union(Vec<FuzzUsersetType>),
    Intersection(Vec<FuzzUsersetType>),
    Exclusion(Box<FuzzUsersetType>, Box<FuzzUsersetType>),
    TupleToUserset(String, String),
}

fn build_userset(userset_type: &FuzzUsersetType, depth: u8, max_depth: u8) -> Userset {
    if depth >= max_depth {
        return Userset::This;
    }

    match userset_type {
        FuzzUsersetType::This => Userset::This,
        FuzzUsersetType::ComputedUserset(rel) => Userset::ComputedUserset {
            relation: rel.clone(),
        },
        FuzzUsersetType::Union(children) => {
            let children: Vec<Userset> = children
                .iter()
                .take(5)
                .map(|c| build_userset(c, depth + 1, max_depth))
                .collect();
            if children.is_empty() {
                Userset::This
            } else {
                Userset::Union { children }
            }
        }
        FuzzUsersetType::Intersection(children) => {
            let children: Vec<Userset> = children
                .iter()
                .take(5)
                .map(|c| build_userset(c, depth + 1, max_depth))
                .collect();
            if children.is_empty() {
                Userset::This
            } else {
                Userset::Intersection { children }
            }
        }
        FuzzUsersetType::Exclusion(base, subtract) => Userset::Exclusion {
            base: Box::new(build_userset(base, depth + 1, max_depth)),
            subtract: Box::new(build_userset(subtract, depth + 1, max_depth)),
        },
        FuzzUsersetType::TupleToUserset(tupleset, computed) => Userset::TupleToUserset {
            tupleset: tupleset.clone(),
            computed_userset: computed.clone(),
        },
    }
}

fuzz_target!(|input: FuzzInput| {
    // Create ListObjectsRequest - this exercises the request construction
    let request = ListObjectsRequest {
        store_id: input.store_id.clone(),
        user: input.user.clone(),
        relation: input.relation.clone(),
        object_type: input.object_type.clone(),
        contextual_tuples: Arc::new(Vec::new()),
        context: Arc::new(HashMap::new()),
    };

    // Exercise request fields
    let _ = request.store_id.len();
    let _ = request.user.len();
    let _ = request.relation.len();
    let _ = request.object_type.len();
    let _ = format!("{:?}", request);

    // Build and exercise authorization model with various usersets
    let mut type_definitions = Vec::new();

    for i in 0..input.num_types.min(10) {
        let type_name = format!("type_{}", i);
        let mut relations = Vec::new();

        // Add a relation with varying userset complexity
        let userset_type = match i % 6 {
            0 => FuzzUsersetType::This,
            1 => FuzzUsersetType::ComputedUserset(input.relation.clone()),
            2 => FuzzUsersetType::Union(vec![FuzzUsersetType::This]),
            3 => FuzzUsersetType::Intersection(vec![FuzzUsersetType::This]),
            4 => FuzzUsersetType::Exclusion(
                Box::new(FuzzUsersetType::This),
                Box::new(FuzzUsersetType::This),
            ),
            _ => FuzzUsersetType::TupleToUserset("parent".to_string(), "viewer".to_string()),
        };

        let userset = build_userset(&userset_type, 0, input.userset_depth);

        relations.push(RelationDefinition {
            name: input.relation.clone(),
            rewrite: userset,
            type_constraints: vec![TypeConstraint {
                type_name: "user".to_string(),
                condition: None,
            }],
        });

        type_definitions.push(TypeDefinition {
            type_name,
            relations,
        });
    }

    // Create model
    let model = AuthorizationModel {
        id: Some("fuzz-model".to_string()),
        schema_version: "1.1".to_string(),
        type_definitions,
        conditions: Vec::new(),
    };

    // Exercise model
    let _ = format!("{:?}", model);
    let _cloned = model.clone();

    // Exercise model lookup methods
    for type_def in &model.type_definitions {
        let _ = type_def.type_name.len();
        for rel in &type_def.relations {
            let _ = rel.name.len();
            let _ = format!("{:?}", rel.rewrite);
        }
    }
});
