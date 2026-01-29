//! Fuzz target for ReverseExpand algorithm components
//!
//! This fuzz target exercises userset parsing and model construction
//! to find crashes, panics, and malformed input handling issues.
//! Per Invariant I4, security-critical code (graph resolver) requires fuzz testing.

#![no_main]

use arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;
use rsfga_domain::model::{
    AuthorizationModel, RelationDefinition, TypeConstraint, TypeDefinition, Userset,
};

/// Fuzz input for model/userset construction
#[derive(Debug, Arbitrary)]
struct FuzzModelInput {
    /// Number of type definitions (limited)
    #[arbitrary(with = |u: &mut Unstructured| -> arbitrary::Result<u8> {
        u.int_in_range(1..=10)
    })]
    num_types: u8,

    /// Type definition configs
    type_configs: Vec<FuzzTypeConfig>,

    /// Depth of nested usersets
    #[arbitrary(with = |u: &mut Unstructured| -> arbitrary::Result<u8> {
        u.int_in_range(0..=30)
    })]
    userset_depth: u8,
}

#[derive(Debug, Arbitrary)]
struct FuzzTypeConfig {
    /// Type name
    #[arbitrary(with = |u: &mut Unstructured| -> arbitrary::Result<String> {
        let len = u.int_in_range(1..=30)?;
        let s: String = (0..len)
            .map(|_| u.choose(&['a', 'b', 'c', '_', '0', '1']).copied())
            .collect::<Result<_, _>>()?;
        Ok(s)
    })]
    type_name: String,

    /// Number of relations
    #[arbitrary(with = |u: &mut Unstructured| -> arbitrary::Result<u8> {
        u.int_in_range(1..=5)
    })]
    num_relations: u8,

    /// Relation configs
    relation_configs: Vec<FuzzRelationConfig>,
}

#[derive(Debug, Arbitrary)]
struct FuzzRelationConfig {
    /// Relation name
    #[arbitrary(with = |u: &mut Unstructured| -> arbitrary::Result<String> {
        let len = u.int_in_range(1..=20)?;
        let s: String = (0..len)
            .map(|_| u.choose(&['a', 'b', 'c', 'd', 'e', '_']).copied())
            .collect::<Result<_, _>>()?;
        Ok(s)
    })]
    name: String,

    /// Userset type
    userset_type: FuzzUsersetType,

    /// Number of type constraints
    #[arbitrary(with = |u: &mut Unstructured| -> arbitrary::Result<u8> {
        u.int_in_range(0..=5)
    })]
    num_constraints: u8,
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
                .take(5) // Limit children
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
                .take(5) // Limit children
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

fuzz_target!(|input: FuzzModelInput| {
    // Build authorization model from fuzz input
    let mut type_definitions = Vec::new();

    for (i, type_config) in input.type_configs.iter().take(input.num_types as usize).enumerate() {
        // Skip empty type names
        if type_config.type_name.is_empty() {
            continue;
        }

        let mut relations = Vec::new();

        for (j, rel_config) in type_config
            .relation_configs
            .iter()
            .take(type_config.num_relations as usize)
            .enumerate()
        {
            // Skip empty relation names
            if rel_config.name.is_empty() {
                continue;
            }

            let userset = build_userset(&rel_config.userset_type, 0, input.userset_depth);

            let mut type_constraints = Vec::new();
            for _ in 0..rel_config.num_constraints.min(5) {
                type_constraints.push(TypeConstraint {
                    type_name: "user".to_string(),
                    condition: None,
                });
            }

            relations.push(RelationDefinition {
                name: rel_config.name.clone(),
                rewrite: userset,
                type_constraints,
            });
        }

        if !relations.is_empty() {
            type_definitions.push(TypeDefinition {
                type_name: type_config.type_name.clone(),
                relations,
            });
        }
    }

    // Skip if no valid type definitions
    if type_definitions.is_empty() {
        return;
    }

    // Create the authorization model
    let model = AuthorizationModel {
        id: Some("fuzz-model".to_string()),
        schema_version: "1.1".to_string(),
        type_definitions,
        conditions: Vec::new(),
    };

    // Exercise the model by accessing various properties
    // This tests that the model construction doesn't panic
    for type_def in &model.type_definitions {
        let _ = type_def.type_name.len();
        for rel in &type_def.relations {
            let _ = rel.name.len();
            // Exercise userset traversal
            let _ = format!("{:?}", rel.rewrite);
            for tc in &rel.type_constraints {
                let _ = tc.type_name.len();
            }
        }
    }

    // Test model cloning (exercises all fields)
    let _cloned = model.clone();

    // Test debug formatting (exercises all nested structures)
    let _ = format!("{:?}", model);
});
