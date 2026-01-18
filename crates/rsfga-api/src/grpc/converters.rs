use crate::proto::openfga::v1 as proto;
use rsfga_domain::model as domain;
use std::collections::HashMap;

// ===================================
// Proto -> Domain
// ===================================

/// Convert Proto AuthorizationModel to Domain AuthorizationModel.
pub fn proto_to_domain_model(proto_model: proto::AuthorizationModel) -> domain::AuthorizationModel {
    let type_definitions = proto_model
        .type_definitions
        .into_iter()
        .map(proto_to_domain_type_definition)
        .collect();

    let conditions = proto_model
        .conditions
        .into_iter()
        .map(|(_, c)| proto_to_domain_condition(c))
        .collect();

    let mut model = domain::AuthorizationModel::with_types_and_conditions(
        proto_model.schema_version,
        type_definitions,
        conditions,
    );
    model.id = if proto_model.id.is_empty() {
        None
    } else {
        Some(proto_model.id)
    };
    model
}

fn proto_to_domain_type_definition(proto_def: proto::TypeDefinition) -> domain::TypeDefinition {
    let mut relations = Vec::new();
    let metadata_relations = proto_def.metadata.map(|m| m.relations).unwrap_or_default();

    for (name, userset) in proto_def.relations {
        let metadata = metadata_relations.get(&name);

        let type_constraints = if let Some(meta) = metadata {
            meta.directly_related_user_types
                .iter()
                .map(proto_to_domain_type_constraint)
                .collect()
        } else {
            Vec::new()
        };

        relations.push(domain::RelationDefinition {
            name,
            type_constraints,
            rewrite: proto_to_domain_userset(userset),
        });
    }

    // Sort relations by name for deterministic order (optional but good)
    relations.sort_by(|a, b| a.name.cmp(&b.name));

    domain::TypeDefinition {
        type_name: proto_def.r#type,
        relations,
    }
}

fn proto_to_domain_type_constraint(proto_ref: &proto::RelationReference) -> domain::TypeConstraint {
    let type_name = match &proto_ref.relation_or_wildcard {
        Some(proto::relation_reference::RelationOrWildcard::Relation(rel)) => {
            format!("{}#{}", proto_ref.r#type, rel)
        }
        Some(proto::relation_reference::RelationOrWildcard::Wildcard(_)) => {
            format!("{}:*", proto_ref.r#type)
        }
        None => proto_ref.r#type.clone(),
    };

    let condition = if proto_ref.condition.is_empty() {
        None
    } else {
        Some(proto_ref.condition.clone())
    };

    domain::TypeConstraint {
        type_name,
        condition,
    }
}

fn proto_to_domain_userset(proto_userset: proto::Userset) -> domain::Userset {
    match proto_userset.userset {
        Some(proto::userset::Userset::This(_)) => domain::Userset::This,
        Some(proto::userset::Userset::ComputedUserset(obj_rel)) => {
            domain::Userset::ComputedUserset {
                relation: obj_rel.relation,
            }
        }
        Some(proto::userset::Userset::TupleToUserset(ttu)) => {
            let tupleset = ttu.tupleset.unwrap_or_default().relation;
            let computed_userset = ttu.computed_userset.unwrap_or_default().relation;
            domain::Userset::TupleToUserset {
                tupleset,
                computed_userset,
            }
        }
        Some(proto::userset::Userset::Union(usersets)) => domain::Userset::Union {
            children: usersets
                .child
                .into_iter()
                .map(proto_to_domain_userset)
                .collect(),
        },
        Some(proto::userset::Userset::Intersection(usersets)) => domain::Userset::Intersection {
            children: usersets
                .child
                .into_iter()
                .map(proto_to_domain_userset)
                .collect(),
        },
        Some(proto::userset::Userset::Difference(diff)) => domain::Userset::Exclusion {
            base: Box::new(proto_to_domain_userset(*diff.base.unwrap_or_default())),
            subtract: Box::new(proto_to_domain_userset(*diff.subtract.unwrap_or_default())),
        },
        None => domain::Userset::This, // Fallback? Or panic? This implies invalid model.
    }
}

fn proto_to_domain_condition(proto_cond: proto::Condition) -> domain::Condition {
    let parameters = proto_cond
        .parameters
        .into_iter()
        .map(|(name, param_ref)| {
            // param_ref.type_name is i32 enum value
            let type_name_enum = proto::TypeName::try_from(param_ref.type_name)
                .unwrap_or(proto::TypeName::Unspecified);
            domain::ConditionParameter::new(name, type_name_enum.as_str_name().to_string())
        })
        .collect();

    domain::Condition {
        name: proto_cond.name,
        expression: proto_cond.expression,
        parameters,
    }
}

// ===================================
// Domain -> Proto
// ===================================

/// Convert Domain AuthorizationModel to Proto AuthorizationModel.
pub fn domain_to_proto_model(
    domain_model: domain::AuthorizationModel,
) -> proto::AuthorizationModel {
    let type_definitions = domain_model
        .type_definitions
        .into_iter()
        .map(domain_to_proto_type_definition)
        .collect();

    let conditions = domain_model
        .conditions
        .into_iter()
        .map(|c| (c.name.clone(), domain_to_proto_condition(c)))
        .collect();

    proto::AuthorizationModel {
        id: domain_model.id.unwrap_or_default(),
        schema_version: domain_model.schema_version,
        type_definitions,
        conditions,
    }
}

fn domain_to_proto_type_definition(domain_def: domain::TypeDefinition) -> proto::TypeDefinition {
    let mut relations_map = HashMap::new();
    let mut metadata_relations_map = HashMap::new();

    for relation in domain_def.relations {
        relations_map.insert(
            relation.name.clone(),
            domain_to_proto_userset(relation.rewrite),
        );

        let directly_related_user_types = relation
            .type_constraints
            .into_iter()
            .map(domain_to_proto_relation_reference)
            .collect();

        metadata_relations_map.insert(
            relation.name,
            proto::RelationMetadata {
                directly_related_user_types,
                module: String::new(),      // Todo
                source_info: String::new(), // Todo
            },
        );
    }

    proto::TypeDefinition {
        r#type: domain_def.type_name,
        relations: relations_map,
        metadata: Some(proto::Metadata {
            relations: metadata_relations_map,
            module: String::new(),
            source_info: String::new(),
        }),
    }
}

fn domain_to_proto_relation_reference(
    constraint: domain::TypeConstraint,
) -> proto::RelationReference {
    let (type_name, relation_or_wildcard) = if constraint.type_name.ends_with(":*") {
        let base_type = constraint.type_name.trim_end_matches(":*").to_string();
        (
            base_type,
            Some(proto::relation_reference::RelationOrWildcard::Wildcard(
                proto::Wildcard {},
            )),
        )
    } else if let Some((base_type, relation)) = constraint.type_name.split_once('#') {
        (
            base_type.to_string(),
            Some(proto::relation_reference::RelationOrWildcard::Relation(
                relation.to_string(),
            )),
        )
    } else {
        (constraint.type_name, None)
    };

    proto::RelationReference {
        r#type: type_name,
        relation_or_wildcard,
        condition: constraint.condition.unwrap_or_default(),
    }
}

fn domain_to_proto_userset(domain_userset: domain::Userset) -> proto::Userset {
    let userset_variant = match domain_userset {
        domain::Userset::This => proto::userset::Userset::This(proto::DirectUserset {}),
        domain::Userset::ComputedUserset { relation } => {
            proto::userset::Userset::ComputedUserset(proto::ObjectRelation {
                relation,
                object: String::new(), // Unused in this context?
            })
        }
        domain::Userset::TupleToUserset {
            tupleset,
            computed_userset,
        } => proto::userset::Userset::TupleToUserset(proto::TupleToUserset {
            tupleset: Some(proto::ObjectRelation {
                relation: tupleset,
                object: String::new(),
            }),
            computed_userset: Some(proto::ObjectRelation {
                relation: computed_userset,
                object: String::new(),
            }),
        }),
        domain::Userset::Union { children } => proto::userset::Userset::Union(proto::Usersets {
            child: children.into_iter().map(domain_to_proto_userset).collect(),
        }),
        domain::Userset::Intersection { children } => {
            proto::userset::Userset::Intersection(proto::Usersets {
                child: children.into_iter().map(domain_to_proto_userset).collect(),
            })
        }
        domain::Userset::Exclusion { base, subtract } => {
            proto::userset::Userset::Difference(Box::new(proto::Difference {
                base: Some(Box::new(domain_to_proto_userset(*base))),
                subtract: Some(Box::new(domain_to_proto_userset(*subtract))),
            }))
        }
    };

    proto::Userset {
        userset: Some(userset_variant),
    }
}

fn domain_to_proto_condition(domain_cond: domain::Condition) -> proto::Condition {
    let parameters = domain_cond
        .parameters
        .into_iter()
        .map(|param| {
            let type_name_enum = proto::TypeName::from_str_name(&param.type_name)
                .unwrap_or(proto::TypeName::Unspecified);
            (
                param.name,
                proto::ConditionParamTypeRef {
                    type_name: type_name_enum as i32,
                    generic_types: Vec::new(),
                },
            )
        })
        .collect();

    proto::Condition {
        name: domain_cond.name,
        expression: domain_cond.expression,
        parameters,
        metadata: None, // Todo
    }
}
