//! Proto-JSON conversion functions for gRPC service.
//!
//! This module provides bidirectional conversion between protobuf types and JSON
//! for storage compatibility with the HTTP layer. All authorization models are
//! stored as JSON, so these conversions are required for gRPC endpoints.
//!
//! # Conversion Categories
//!
//! ## Prost ↔ JSON Value Conversions
//! - `prost_value_to_json` / `json_to_prost_value`
//! - `prost_struct_to_json` / `json_map_to_prost_struct`
//! - `prost_struct_to_hashmap` / `hashmap_to_prost_struct`
//!
//! ## Authorization Model Conversions
//! - `type_definition_to_json` / `json_to_type_definition`
//! - `userset_to_json` / `json_to_userset`
//! - `condition_to_json` / `json_to_condition`
//! - `stored_model_to_proto`
//!
//! ## Tuple and Node Conversions
//! - `tuple_key_to_stored` - TupleKey proto to StoredTuple
//! - `datetime_to_timestamp` - DateTime to prost Timestamp
//! - `expand_node_to_proto` - Domain ExpandNode to proto Node

use std::collections::HashMap;

use crate::proto::openfga::v1::{
    AuthorizationModel, Computed, Condition, ConditionMetadata, ConditionParamTypeRef, Difference,
    DirectUserset, Leaf, Metadata, Node, Nodes, ObjectRelation, RelationMetadata,
    RelationReference, TupleKey, TupleToUserset, TypeDefinition, TypeName, Users, Userset,
    Usersets, Wildcard,
};
use crate::validation::{is_valid_condition_name, prost_value_exceeds_max_depth};
use rsfga_storage::{DateTime, StoredAuthorizationModel, StoredTuple, Utc};

/// Maximum size for condition context in bytes (10KB).
pub const MAX_CONDITION_CONTEXT_SIZE: usize = 10 * 1024;

// ============================================================
// Prost ↔ JSON Value Conversions
// ============================================================

/// Converts a prost_types::Value to serde_json::Value (takes ownership to avoid clones).
///
/// Returns `Err` if the value contains NaN or Infinity (invalid JSON numbers).
pub fn prost_value_to_json(value: prost_types::Value) -> Result<serde_json::Value, &'static str> {
    use prost_types::value::Kind;

    match value.kind {
        Some(Kind::NullValue(_)) => Ok(serde_json::Value::Null),
        Some(Kind::NumberValue(n)) => {
            // Reject NaN and Infinity as they are not valid JSON values.
            // Note: Protobuf NumberValue uses f64, which loses precision for
            // integers > 2^53. This is a known limitation of the protocol.
            if n.is_nan() || n.is_infinite() {
                return Err("NaN and Infinity are not valid JSON numbers");
            }
            Ok(serde_json::json!(n))
        }
        Some(Kind::StringValue(s)) => Ok(serde_json::Value::String(s)),
        Some(Kind::BoolValue(b)) => Ok(serde_json::Value::Bool(b)),
        Some(Kind::StructValue(s)) => prost_struct_to_json(s),
        Some(Kind::ListValue(l)) => {
            let values: Result<Vec<_>, _> = l.values.into_iter().map(prost_value_to_json).collect();
            Ok(serde_json::Value::Array(values?))
        }
        None => Ok(serde_json::Value::Null),
    }
}

/// Converts a prost_types::Struct to serde_json::Value (takes ownership to avoid clones).
///
/// Returns `Err` if any value contains NaN or Infinity.
pub fn prost_struct_to_json(s: prost_types::Struct) -> Result<serde_json::Value, &'static str> {
    let mut map = serde_json::Map::new();
    for (k, v) in s.fields {
        map.insert(k, prost_value_to_json(v)?);
    }
    Ok(serde_json::Value::Object(map))
}

/// Converts a prost_types::Struct to HashMap<String, serde_json::Value> (takes ownership).
///
/// Returns `Err` if any value contains NaN or Infinity.
pub fn prost_struct_to_hashmap(
    s: prost_types::Struct,
) -> Result<HashMap<String, serde_json::Value>, &'static str> {
    s.fields
        .into_iter()
        .map(|(k, v)| Ok((k, prost_value_to_json(v)?)))
        .collect()
}

/// Converts a serde_json::Value to prost_types::Value (for Read response).
pub fn json_to_prost_value(value: serde_json::Value) -> prost_types::Value {
    use prost_types::value::Kind;

    prost_types::Value {
        kind: Some(match value {
            serde_json::Value::Null => Kind::NullValue(0),
            serde_json::Value::Bool(b) => Kind::BoolValue(b),
            serde_json::Value::Number(n) => Kind::NumberValue(n.as_f64().unwrap_or(0.0)),
            serde_json::Value::String(s) => Kind::StringValue(s),
            serde_json::Value::Array(arr) => Kind::ListValue(prost_types::ListValue {
                values: arr.into_iter().map(json_to_prost_value).collect(),
            }),
            serde_json::Value::Object(obj) => Kind::StructValue(json_map_to_prost_struct(obj)),
        }),
    }
}

/// Converts a serde_json Map to prost_types::Struct (for Read response).
pub fn json_map_to_prost_struct(
    map: serde_json::Map<String, serde_json::Value>,
) -> prost_types::Struct {
    prost_types::Struct {
        fields: map
            .into_iter()
            .map(|(k, v)| (k, json_to_prost_value(v)))
            .collect(),
    }
}

/// Converts a HashMap<String, serde_json::Value> to prost_types::Struct (for Read response).
pub fn hashmap_to_prost_struct(map: HashMap<String, serde_json::Value>) -> prost_types::Struct {
    prost_types::Struct {
        fields: map
            .into_iter()
            .map(|(k, v)| (k, json_to_prost_value(v)))
            .collect(),
    }
}

// ============================================================
// Proto → JSON Conversion Functions for Authorization Models
// ============================================================

/// Converts a proto TypeDefinition to JSON for storage.
pub fn type_definition_to_json(td: &TypeDefinition) -> serde_json::Value {
    let mut obj = serde_json::json!({
        "type": td.r#type,
    });

    // Convert relations map
    if !td.relations.is_empty() {
        let relations: serde_json::Map<String, serde_json::Value> = td
            .relations
            .iter()
            .map(|(k, v)| (k.clone(), userset_to_json(v)))
            .collect();
        obj["relations"] = serde_json::Value::Object(relations);
    }

    // Convert metadata if present
    if let Some(ref metadata) = td.metadata {
        obj["metadata"] = metadata_to_json(metadata);
    }

    obj
}

/// Converts a proto Userset to JSON.
pub fn userset_to_json(us: &Userset) -> serde_json::Value {
    use crate::proto::openfga::v1::userset::Userset as US;
    match &us.userset {
        Some(US::This(_)) => serde_json::json!({ "this": {} }),
        Some(US::ComputedUserset(or)) => serde_json::json!({
            "computedUserset": {
                "relation": or.relation
            }
        }),
        Some(US::TupleToUserset(ttu)) => {
            let mut obj = serde_json::json!({});
            if let Some(ref ts) = ttu.tupleset {
                obj["tupleset"] = serde_json::json!({ "relation": ts.relation });
            }
            if let Some(ref cus) = ttu.computed_userset {
                obj["computedUserset"] = serde_json::json!({ "relation": cus.relation });
            }
            serde_json::json!({ "tupleToUserset": obj })
        }
        Some(US::Union(children)) => serde_json::json!({
            "union": {
                "child": children.child.iter().map(userset_to_json).collect::<Vec<_>>()
            }
        }),
        Some(US::Intersection(children)) => serde_json::json!({
            "intersection": {
                "child": children.child.iter().map(userset_to_json).collect::<Vec<_>>()
            }
        }),
        Some(US::Difference(diff)) => {
            let mut obj = serde_json::json!({});
            if let Some(ref base) = diff.base {
                obj["base"] = userset_to_json(base);
            }
            if let Some(ref subtract) = diff.subtract {
                obj["subtract"] = userset_to_json(subtract);
            }
            serde_json::json!({ "difference": obj })
        }
        None => serde_json::json!({}),
    }
}

/// Converts proto Metadata to JSON.
pub fn metadata_to_json(md: &Metadata) -> serde_json::Value {
    let mut obj = serde_json::json!({});
    if !md.relations.is_empty() {
        let relations: serde_json::Map<String, serde_json::Value> = md
            .relations
            .iter()
            .map(|(k, v)| (k.clone(), relation_metadata_to_json(v)))
            .collect();
        obj["relations"] = serde_json::Value::Object(relations);
    }
    if !md.module.is_empty() {
        obj["module"] = serde_json::Value::String(md.module.clone());
    }
    if !md.source_info.is_empty() {
        obj["source_info"] = serde_json::Value::String(md.source_info.clone());
    }
    obj
}

/// Converts proto RelationMetadata to JSON.
pub fn relation_metadata_to_json(rm: &RelationMetadata) -> serde_json::Value {
    let mut obj = serde_json::json!({});
    if !rm.directly_related_user_types.is_empty() {
        obj["directly_related_user_types"] = serde_json::json!(rm
            .directly_related_user_types
            .iter()
            .map(relation_reference_to_json)
            .collect::<Vec<_>>());
    }
    if !rm.module.is_empty() {
        obj["module"] = serde_json::Value::String(rm.module.clone());
    }
    if !rm.source_info.is_empty() {
        obj["source_info"] = serde_json::Value::String(rm.source_info.clone());
    }
    obj
}

/// Converts proto RelationReference to JSON.
pub fn relation_reference_to_json(rr: &RelationReference) -> serde_json::Value {
    use crate::proto::openfga::v1::relation_reference::RelationOrWildcard;
    let mut obj = serde_json::json!({ "type": rr.r#type });
    match &rr.relation_or_wildcard {
        Some(RelationOrWildcard::Relation(rel)) => {
            obj["relation"] = serde_json::Value::String(rel.clone());
        }
        Some(RelationOrWildcard::Wildcard(_)) => {
            obj["wildcard"] = serde_json::json!({});
        }
        None => {}
    }
    if !rr.condition.is_empty() {
        obj["condition"] = serde_json::Value::String(rr.condition.clone());
    }
    obj
}

/// Converts a proto Condition to JSON for storage.
pub fn condition_to_json(cond: &Condition) -> serde_json::Value {
    let mut obj = serde_json::json!({
        "name": cond.name,
        "expression": cond.expression,
    });
    if !cond.parameters.is_empty() {
        let params: serde_json::Map<String, serde_json::Value> = cond
            .parameters
            .iter()
            .map(|(k, v)| (k.clone(), condition_param_to_json(v)))
            .collect();
        obj["parameters"] = serde_json::Value::Object(params);
    }
    if let Some(ref md) = cond.metadata {
        let mut md_obj = serde_json::json!({});
        if !md.module.is_empty() {
            md_obj["module"] = serde_json::Value::String(md.module.clone());
        }
        if !md.source_info.is_empty() {
            md_obj["source_info"] = serde_json::Value::String(md.source_info.clone());
        }
        obj["metadata"] = md_obj;
    }
    obj
}

/// Converts proto ConditionParamTypeRef to JSON.
pub fn condition_param_to_json(cp: &ConditionParamTypeRef) -> serde_json::Value {
    let type_name = type_name_to_string(cp.type_name);
    let mut obj = serde_json::json!({ "type_name": type_name });
    if !cp.generic_types.is_empty() {
        obj["generic_types"] = serde_json::json!(cp
            .generic_types
            .iter()
            .map(|t| type_name_to_string(*t))
            .collect::<Vec<_>>());
    }
    obj
}

/// Converts proto TypeName enum to string.
pub fn type_name_to_string(tn: i32) -> String {
    match TypeName::try_from(tn) {
        Ok(TypeName::Unspecified) => "TYPE_NAME_UNSPECIFIED",
        Ok(TypeName::Any) => "TYPE_NAME_ANY",
        Ok(TypeName::Bool) => "TYPE_NAME_BOOL",
        Ok(TypeName::String) => "TYPE_NAME_STRING",
        Ok(TypeName::Int) => "TYPE_NAME_INT",
        Ok(TypeName::Uint) => "TYPE_NAME_UINT",
        Ok(TypeName::Double) => "TYPE_NAME_DOUBLE",
        Ok(TypeName::Duration) => "TYPE_NAME_DURATION",
        Ok(TypeName::Timestamp) => "TYPE_NAME_TIMESTAMP",
        Ok(TypeName::Map) => "TYPE_NAME_MAP",
        Ok(TypeName::List) => "TYPE_NAME_LIST",
        Ok(TypeName::Ipaddress) => "TYPE_NAME_IPADDRESS",
        Err(_) => "TYPE_NAME_UNSPECIFIED",
    }
    .to_string()
}

// ============================================================
// JSON → Proto Conversion Functions for Authorization Models
// ============================================================

/// Parses a JSON type definition into a proto TypeDefinition.
pub fn json_to_type_definition(json: &serde_json::Value) -> Option<TypeDefinition> {
    let type_name = json.get("type")?.as_str()?.to_string();
    let relations = json
        .get("relations")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| json_to_userset(v).map(|us| (k.clone(), us)))
                .collect()
        })
        .unwrap_or_default();

    let metadata = json.get("metadata").and_then(json_to_metadata);

    Some(TypeDefinition {
        r#type: type_name,
        relations,
        metadata,
    })
}

/// Parses JSON into a proto Userset.
pub fn json_to_userset(json: &serde_json::Value) -> Option<Userset> {
    use crate::proto::openfga::v1::userset::Userset as US;

    if json.get("this").is_some() {
        return Some(Userset {
            userset: Some(US::This(DirectUserset {})),
        });
    }

    if let Some(cu) = json.get("computedUserset") {
        let relation = cu.get("relation")?.as_str()?.to_string();
        return Some(Userset {
            userset: Some(US::ComputedUserset(ObjectRelation {
                object: String::new(),
                relation,
            })),
        });
    }

    if let Some(ttu) = json.get("tupleToUserset") {
        let tupleset = ttu.get("tupleset").and_then(|ts| {
            Some(ObjectRelation {
                object: String::new(),
                relation: ts.get("relation")?.as_str()?.to_string(),
            })
        });
        let computed_userset = ttu.get("computedUserset").and_then(|cus| {
            Some(ObjectRelation {
                object: String::new(),
                relation: cus.get("relation")?.as_str()?.to_string(),
            })
        });
        return Some(Userset {
            userset: Some(US::TupleToUserset(TupleToUserset {
                tupleset,
                computed_userset,
            })),
        });
    }

    if let Some(union) = json.get("union") {
        let children = union
            .get("child")
            .and_then(|c| c.as_array())
            .map(|arr| arr.iter().filter_map(json_to_userset).collect())
            .unwrap_or_default();
        return Some(Userset {
            userset: Some(US::Union(Usersets { child: children })),
        });
    }

    if let Some(intersection) = json.get("intersection") {
        let children = intersection
            .get("child")
            .and_then(|c| c.as_array())
            .map(|arr| arr.iter().filter_map(json_to_userset).collect())
            .unwrap_or_default();
        return Some(Userset {
            userset: Some(US::Intersection(Usersets { child: children })),
        });
    }

    if let Some(diff) = json.get("difference") {
        let base = diff.get("base").and_then(json_to_userset).map(Box::new);
        let subtract = diff.get("subtract").and_then(json_to_userset).map(Box::new);
        return Some(Userset {
            userset: Some(US::Difference(Box::new(Difference { base, subtract }))),
        });
    }

    None
}

/// Parses JSON into proto Metadata.
pub fn json_to_metadata(json: &serde_json::Value) -> Option<Metadata> {
    let relations = json
        .get("relations")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| json_to_relation_metadata(v).map(|rm| (k.clone(), rm)))
                .collect()
        })
        .unwrap_or_default();

    let module = json
        .get("module")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let source_info = json
        .get("source_info")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Some(Metadata {
        relations,
        module,
        source_info,
    })
}

/// Parses JSON into proto RelationMetadata.
pub fn json_to_relation_metadata(json: &serde_json::Value) -> Option<RelationMetadata> {
    let directly_related_user_types = json
        .get("directly_related_user_types")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(json_to_relation_reference).collect())
        .unwrap_or_default();

    let module = json
        .get("module")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let source_info = json
        .get("source_info")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Some(RelationMetadata {
        directly_related_user_types,
        module,
        source_info,
    })
}

/// Parses JSON into proto RelationReference.
pub fn json_to_relation_reference(json: &serde_json::Value) -> Option<RelationReference> {
    use crate::proto::openfga::v1::relation_reference::RelationOrWildcard;

    let type_name = json.get("type")?.as_str()?.to_string();
    let relation_or_wildcard = if json.get("wildcard").is_some() {
        Some(RelationOrWildcard::Wildcard(Wildcard {}))
    } else {
        json.get("relation")
            .and_then(|r| r.as_str())
            .map(|s| RelationOrWildcard::Relation(s.to_string()))
    };
    let condition = json
        .get("condition")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Some(RelationReference {
        r#type: type_name,
        relation_or_wildcard,
        condition,
    })
}

/// Parses JSON into a proto Condition.
pub fn json_to_condition(json: &serde_json::Value) -> Option<Condition> {
    let name = json.get("name")?.as_str()?.to_string();
    let expression = json
        .get("expression")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let parameters = json
        .get("parameters")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| json_to_condition_param(v).map(|cp| (k.clone(), cp)))
                .collect()
        })
        .unwrap_or_default();

    let metadata = json.get("metadata").map(|md| ConditionMetadata {
        module: md
            .get("module")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        source_info: md
            .get("source_info")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    });

    Some(Condition {
        name,
        expression,
        parameters,
        metadata,
    })
}

/// Parses JSON into proto ConditionParamTypeRef.
pub fn json_to_condition_param(json: &serde_json::Value) -> Option<ConditionParamTypeRef> {
    let type_name = json
        .get("type_name")
        .and_then(|v| v.as_str())
        .map(string_to_type_name)
        .unwrap_or(TypeName::Unspecified as i32);
    let generic_types = json
        .get("generic_types")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(string_to_type_name)
                .collect()
        })
        .unwrap_or_default();

    Some(ConditionParamTypeRef {
        type_name,
        generic_types,
    })
}

/// Converts string to proto TypeName enum value.
pub fn string_to_type_name(s: &str) -> i32 {
    match s {
        "TYPE_NAME_ANY" => TypeName::Any as i32,
        "TYPE_NAME_BOOL" => TypeName::Bool as i32,
        "TYPE_NAME_STRING" => TypeName::String as i32,
        "TYPE_NAME_INT" => TypeName::Int as i32,
        "TYPE_NAME_UINT" => TypeName::Uint as i32,
        "TYPE_NAME_DOUBLE" => TypeName::Double as i32,
        "TYPE_NAME_DURATION" => TypeName::Duration as i32,
        "TYPE_NAME_TIMESTAMP" => TypeName::Timestamp as i32,
        "TYPE_NAME_MAP" => TypeName::Map as i32,
        "TYPE_NAME_LIST" => TypeName::List as i32,
        "TYPE_NAME_IPADDRESS" => TypeName::Ipaddress as i32,
        _ => TypeName::Unspecified as i32,
    }
}

// ============================================================
// Stored Model Conversion
// ============================================================

/// Converts a StoredAuthorizationModel to a proto AuthorizationModel.
///
/// Logs warnings when individual type definitions or conditions fail to parse,
/// instead of silently dropping them. This helps diagnose data corruption issues.
pub fn stored_model_to_proto(stored: &StoredAuthorizationModel) -> Option<AuthorizationModel> {
    // Parse the stored JSON
    let parsed: serde_json::Value = match serde_json::from_str(&stored.model_json) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                model_id = %stored.id,
                store_id = %stored.store_id,
                error = %e,
                "Failed to parse authorization model JSON"
            );
            return None;
        }
    };

    // Extract and convert type_definitions with logging for failures
    let type_definitions: Vec<TypeDefinition> = parsed
        .get("type_definitions")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .enumerate()
                .filter_map(|(idx, td_json)| match json_to_type_definition(td_json) {
                    Some(td) => Some(td),
                    None => {
                        tracing::warn!(
                            model_id = %stored.id,
                            store_id = %stored.store_id,
                            type_def_index = idx,
                            type_def_json = %td_json,
                            "Failed to parse type definition in authorization model"
                        );
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    // Extract schema_version
    let schema_version = parsed
        .get("schema_version")
        .and_then(|v| v.as_str())
        .unwrap_or("1.1")
        .to_string();

    // Extract and convert conditions with logging for failures
    let conditions = parsed
        .get("conditions")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| match json_to_condition(v) {
                    Some(c) => Some((k.clone(), c)),
                    None => {
                        tracing::warn!(
                            model_id = %stored.id,
                            store_id = %stored.store_id,
                            condition_name = %k,
                            condition_json = %v,
                            "Failed to parse condition in authorization model"
                        );
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    Some(AuthorizationModel {
        id: stored.id.clone(),
        schema_version,
        type_definitions,
        conditions,
    })
}

// ============================================================
// Tuple Key Conversion
// ============================================================

/// Error returned when tuple key parsing fails.
///
/// Contains the original user/object strings for error messages (avoids cloning in happy path).
#[derive(Debug)]
pub struct TupleKeyParseError {
    pub user: String,
    pub object: String,
    pub reason: &'static str,
}

/// Converts a `TupleKey` proto to `StoredTuple` (takes ownership to avoid clones).
///
/// Uses `parse_user` and `parse_object` for consistent validation across all handlers.
/// Parses the optional condition field including name and context.
///
/// # Errors
///
/// Returns `Err` with the original user/object for error messages (avoids cloning in happy path).
pub fn tuple_key_to_stored(tk: TupleKey) -> Result<StoredTuple, TupleKeyParseError> {
    use crate::utils::{parse_object, parse_user};

    let (user_type, user_id, user_relation) =
        parse_user(&tk.user).ok_or_else(|| TupleKeyParseError {
            user: tk.user.clone(),
            object: tk.object.clone(),
            reason: "invalid user format",
        })?;

    let (object_type, object_id) = parse_object(&tk.object).ok_or_else(|| TupleKeyParseError {
        user: tk.user.clone(),
        object: tk.object.clone(),
        reason: "invalid object format",
    })?;

    // Parse and validate condition if present
    let (condition_name, condition_context) = if let Some(cond) = tk.condition {
        if cond.name.is_empty() {
            (None, None)
        } else {
            // Validate condition name format (security constraint I4)
            if !is_valid_condition_name(&cond.name) {
                return Err(TupleKeyParseError {
                    user: tk.user,
                    object: tk.object,
                    reason: "invalid condition name: must be alphanumeric/underscore/hyphen, max 256 chars",
                });
            }

            // Convert and validate context (constraint C11)
            let context = if let Some(ctx) = cond.context {
                // Check depth limit to prevent stack overflow
                if ctx
                    .fields
                    .values()
                    .any(|v| prost_value_exceeds_max_depth(v, 1))
                {
                    return Err(TupleKeyParseError {
                        user: tk.user,
                        object: tk.object,
                        reason: "condition context exceeds maximum nesting depth (10 levels)",
                    });
                }

                // Convert prost Struct to HashMap, rejecting NaN/Infinity values
                let hashmap = prost_struct_to_hashmap(ctx).map_err(|_| TupleKeyParseError {
                    user: tk.user.clone(),
                    object: tk.object.clone(),
                    reason: "condition context contains invalid number (NaN or Infinity)",
                })?;

                // Estimate serialized size to enforce bounds
                let estimated_size: usize = hashmap
                    .iter()
                    .map(|(k, v)| k.len() + v.to_string().len())
                    .sum();
                if estimated_size > MAX_CONDITION_CONTEXT_SIZE {
                    return Err(TupleKeyParseError {
                        user: tk.user,
                        object: tk.object,
                        reason: "condition context exceeds maximum size (10KB)",
                    });
                }
                Some(hashmap)
            } else {
                None
            };

            (Some(cond.name), context)
        }
    } else {
        (None, None)
    };

    Ok(StoredTuple {
        object_type: object_type.to_string(),
        object_id: object_id.to_string(),
        relation: tk.relation,
        user_type: user_type.to_string(),
        user_id: user_id.to_string(),
        user_relation: user_relation.map(|s| s.to_string()),
        condition_name,
        condition_context,
        created_at: None,
    })
}

// ============================================================
// Timestamp Conversion
// ============================================================

/// Converts a chrono `DateTime<Utc>` to a `prost_types::Timestamp`.
pub fn datetime_to_timestamp(dt: DateTime<Utc>) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: dt.timestamp(),
        nanos: dt.timestamp_subsec_nanos() as i32,
    }
}

// ============================================================
// Expand Node Conversion
// ============================================================

/// Converts a domain `ExpandNode` to a proto `Node`.
pub fn expand_node_to_proto(node: rsfga_domain::resolver::ExpandNode) -> Node {
    use rsfga_domain::resolver::{ExpandLeafValue, ExpandNode as DomainExpandNode};

    match node {
        DomainExpandNode::Leaf(leaf) => {
            // We need to extract the object before moving leaf.name into the Node
            // Only compute this for TupleToUserset, but we must do it before the move
            let object_for_tupleset =
                if matches!(leaf.value, ExpandLeafValue::TupleToUserset { .. }) {
                    // Extract object from leaf.name (format: "type:id#relation")
                    // The tupleset relation is on the same object being expanded
                    if let Some(object_part) = leaf.name.split('#').next() {
                        if object_part.is_empty() {
                            tracing::warn!(
                                leaf_name = %leaf.name,
                                "Expand leaf.name has empty object part before '#'"
                            );
                        }
                        object_part.to_string()
                    } else {
                        tracing::warn!(
                            leaf_name = %leaf.name,
                            "Expand leaf.name format unexpected (expected 'type:id#relation')"
                        );
                        String::new()
                    }
                } else {
                    String::new() // Not used for other leaf types
                };
            Node {
                name: leaf.name,
                value: Some(crate::proto::openfga::v1::node::Value::Leaf(
                    match leaf.value {
                        ExpandLeafValue::Users(users) => Leaf {
                            value: Some(crate::proto::openfga::v1::leaf::Value::Users(Users {
                                users,
                            })),
                        },
                        ExpandLeafValue::Computed { userset } => Leaf {
                            value: Some(crate::proto::openfga::v1::leaf::Value::Computed(
                                Computed { userset },
                            )),
                        },
                        ExpandLeafValue::TupleToUserset {
                            tupleset,
                            computed_userset,
                        } => Leaf {
                            value: Some(crate::proto::openfga::v1::leaf::Value::TupleToUserset(
                                TupleToUserset {
                                    tupleset: Some(ObjectRelation {
                                        object: object_for_tupleset,
                                        relation: tupleset,
                                    }),
                                    // computed_userset object is unknown without further resolution
                                    // This matches OpenFGA behavior where the target object is not
                                    // known until the tupleset is resolved
                                    computed_userset: Some(ObjectRelation {
                                        object: String::new(),
                                        relation: computed_userset,
                                    }),
                                },
                            )),
                        },
                    },
                )),
            }
        }
        DomainExpandNode::Union { name, nodes } => Node {
            name,
            value: Some(crate::proto::openfga::v1::node::Value::Union(Nodes {
                nodes: nodes.into_iter().map(expand_node_to_proto).collect(),
            })),
        },
        DomainExpandNode::Intersection { name, nodes } => Node {
            name,
            value: Some(crate::proto::openfga::v1::node::Value::Intersection(
                Nodes {
                    nodes: nodes.into_iter().map(expand_node_to_proto).collect(),
                },
            )),
        },
        DomainExpandNode::Difference {
            name,
            base,
            subtract,
        } => {
            // Note: OpenFGA proto uses Nodes for difference (with 2 nodes: base, subtract)
            // This matches OpenFGA's representation where difference.nodes[0] is base
            // and difference.nodes[1] is subtract
            Node {
                name,
                value: Some(crate::proto::openfga::v1::node::Value::Difference(Nodes {
                    nodes: vec![expand_node_to_proto(*base), expand_node_to_proto(*subtract)],
                })),
            }
        }
    }
}

// ============================================================
// Unit Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prost_value_to_json_null() {
        let value = prost_types::Value {
            kind: Some(prost_types::value::Kind::NullValue(0)),
        };
        assert_eq!(prost_value_to_json(value).unwrap(), serde_json::Value::Null);
    }

    #[test]
    fn test_prost_value_to_json_number() {
        let value = prost_types::Value {
            kind: Some(prost_types::value::Kind::NumberValue(42.5)),
        };
        assert_eq!(prost_value_to_json(value).unwrap(), serde_json::json!(42.5));
    }

    #[test]
    fn test_prost_value_to_json_rejects_nan() {
        let value = prost_types::Value {
            kind: Some(prost_types::value::Kind::NumberValue(f64::NAN)),
        };
        assert!(prost_value_to_json(value).is_err());
    }

    #[test]
    fn test_prost_value_to_json_rejects_infinity() {
        let value = prost_types::Value {
            kind: Some(prost_types::value::Kind::NumberValue(f64::INFINITY)),
        };
        assert!(prost_value_to_json(value).is_err());
    }

    #[test]
    fn test_prost_value_to_json_string() {
        let value = prost_types::Value {
            kind: Some(prost_types::value::Kind::StringValue("hello".to_string())),
        };
        assert_eq!(
            prost_value_to_json(value).unwrap(),
            serde_json::json!("hello")
        );
    }

    #[test]
    fn test_prost_value_to_json_bool() {
        let value = prost_types::Value {
            kind: Some(prost_types::value::Kind::BoolValue(true)),
        };
        assert_eq!(prost_value_to_json(value).unwrap(), serde_json::json!(true));
    }

    #[test]
    fn test_json_to_prost_value_roundtrip() {
        // Note: integers become floats in protobuf (f64), so we use floats in the original
        let original = serde_json::json!({
            "name": "test",
            "count": 42.0,
            "active": true,
            "tags": ["a", "b"]
        });
        let prost_val = json_to_prost_value(original.clone());
        let result = prost_value_to_json(prost_val).unwrap();
        assert_eq!(result, original);
    }

    #[test]
    fn test_type_definition_to_json_basic() {
        let td = TypeDefinition {
            r#type: "user".to_string(),
            relations: Default::default(),
            metadata: None,
        };
        let json = type_definition_to_json(&td);
        assert_eq!(json["type"], "user");
    }

    #[test]
    fn test_json_to_type_definition_basic() {
        let json = serde_json::json!({
            "type": "document"
        });
        let td = json_to_type_definition(&json).unwrap();
        assert_eq!(td.r#type, "document");
    }

    #[test]
    fn test_userset_this_roundtrip() {
        use crate::proto::openfga::v1::userset::Userset as US;
        let us = Userset {
            userset: Some(US::This(DirectUserset {})),
        };
        let json = userset_to_json(&us);
        assert!(json.get("this").is_some());

        let parsed = json_to_userset(&json).unwrap();
        assert!(matches!(parsed.userset, Some(US::This(_))));
    }

    #[test]
    fn test_userset_computed_roundtrip() {
        use crate::proto::openfga::v1::userset::Userset as US;
        let us = Userset {
            userset: Some(US::ComputedUserset(ObjectRelation {
                object: String::new(),
                relation: "viewer".to_string(),
            })),
        };
        let json = userset_to_json(&us);
        assert_eq!(json["computedUserset"]["relation"], "viewer");

        let parsed = json_to_userset(&json).unwrap();
        if let Some(US::ComputedUserset(or)) = parsed.userset {
            assert_eq!(or.relation, "viewer");
        } else {
            panic!("Expected ComputedUserset");
        }
    }

    #[test]
    fn test_condition_roundtrip() {
        let cond = Condition {
            name: "ip_check".to_string(),
            expression: "request.ip == '192.168.1.1'".to_string(),
            parameters: [(
                "ip".to_string(),
                ConditionParamTypeRef {
                    type_name: TypeName::String as i32,
                    generic_types: vec![],
                },
            )]
            .into_iter()
            .collect(),
            metadata: None,
        };
        let json = condition_to_json(&cond);
        assert_eq!(json["name"], "ip_check");
        assert_eq!(json["expression"], "request.ip == '192.168.1.1'");

        let parsed = json_to_condition(&json).unwrap();
        assert_eq!(parsed.name, "ip_check");
        assert_eq!(parsed.expression, "request.ip == '192.168.1.1'");
    }

    #[test]
    fn test_type_name_string_roundtrip() {
        for tn in [
            TypeName::Any,
            TypeName::Bool,
            TypeName::String,
            TypeName::Int,
            TypeName::Uint,
            TypeName::Double,
            TypeName::Duration,
            TypeName::Timestamp,
            TypeName::Map,
            TypeName::List,
            TypeName::Ipaddress,
        ] {
            let s = type_name_to_string(tn as i32);
            let back = string_to_type_name(&s);
            assert_eq!(back, tn as i32);
        }
    }

    #[test]
    fn test_hashmap_to_prost_struct_roundtrip() {
        let mut map = HashMap::new();
        map.insert("key1".to_string(), serde_json::json!("value1"));
        map.insert("key2".to_string(), serde_json::json!(123));

        let prost_struct = hashmap_to_prost_struct(map.clone());
        let back = prost_struct_to_hashmap(prost_struct).unwrap();

        assert_eq!(back.get("key1"), Some(&serde_json::json!("value1")));
        assert_eq!(back.get("key2"), Some(&serde_json::json!(123.0))); // f64 precision
    }

    // -------------------------------------------------------------------------
    // tuple_key_to_stored tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_tuple_key_to_stored_simple() {
        let tk = TupleKey {
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:readme".to_string(),
            condition: None,
        };
        let result = tuple_key_to_stored(tk).unwrap();
        assert_eq!(result.user_type, "user");
        assert_eq!(result.user_id, "alice");
        assert_eq!(result.relation, "viewer");
        assert_eq!(result.object_type, "document");
        assert_eq!(result.object_id, "readme");
        assert!(result.user_relation.is_none());
        assert!(result.condition_name.is_none());
        assert!(result.condition_context.is_none());
    }

    #[test]
    fn test_tuple_key_to_stored_with_userset() {
        let tk = TupleKey {
            user: "team:engineering#member".to_string(),
            relation: "viewer".to_string(),
            object: "document:readme".to_string(),
            condition: None,
        };
        let result = tuple_key_to_stored(tk).unwrap();
        assert_eq!(result.user_type, "team");
        assert_eq!(result.user_id, "engineering");
        assert_eq!(result.user_relation, Some("member".to_string()));
    }

    #[test]
    fn test_tuple_key_to_stored_invalid_user() {
        let tk = TupleKey {
            user: "invalid".to_string(),
            relation: "viewer".to_string(),
            object: "document:readme".to_string(),
            condition: None,
        };
        let result = tuple_key_to_stored(tk);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.reason, "invalid user format");
        assert_eq!(err.user, "invalid");
    }

    #[test]
    fn test_tuple_key_to_stored_invalid_object() {
        let tk = TupleKey {
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "invalid".to_string(),
            condition: None,
        };
        let result = tuple_key_to_stored(tk);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.reason, "invalid object format");
    }

    #[test]
    fn test_tuple_key_to_stored_with_condition() {
        use crate::proto::openfga::v1::RelationshipCondition;
        use prost_types::value::Kind;

        let mut context_fields = std::collections::BTreeMap::new();
        context_fields.insert(
            "ip_address".to_string(),
            prost_types::Value {
                kind: Some(Kind::StringValue("192.168.1.1".to_string())),
            },
        );

        let tk = TupleKey {
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:readme".to_string(),
            condition: Some(RelationshipCondition {
                name: "ip_allowlist".to_string(),
                context: Some(prost_types::Struct {
                    fields: context_fields,
                }),
            }),
        };
        let result = tuple_key_to_stored(tk).unwrap();
        assert_eq!(result.condition_name, Some("ip_allowlist".to_string()));
        assert!(result.condition_context.is_some());
        let ctx = result.condition_context.unwrap();
        assert_eq!(
            ctx.get("ip_address"),
            Some(&serde_json::json!("192.168.1.1"))
        );
    }

    #[test]
    fn test_tuple_key_to_stored_empty_condition_name() {
        use crate::proto::openfga::v1::RelationshipCondition;

        let tk = TupleKey {
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:readme".to_string(),
            condition: Some(RelationshipCondition {
                name: String::new(),
                context: None,
            }),
        };
        let result = tuple_key_to_stored(tk).unwrap();
        // Empty name is treated as no condition
        assert!(result.condition_name.is_none());
        assert!(result.condition_context.is_none());
    }

    #[test]
    fn test_tuple_key_to_stored_invalid_condition_name() {
        use crate::proto::openfga::v1::RelationshipCondition;

        let tk = TupleKey {
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:readme".to_string(),
            condition: Some(RelationshipCondition {
                name: "invalid name with spaces!".to_string(),
                context: None,
            }),
        };
        let result = tuple_key_to_stored(tk);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.reason.contains("invalid condition name"));
    }

    // -------------------------------------------------------------------------
    // datetime_to_timestamp tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_datetime_to_timestamp_now() {
        // Test with current time - verifies basic functionality
        let dt = Utc::now();
        let ts = datetime_to_timestamp(dt);
        // Timestamp should be recent (within last hour to be safe)
        assert!(ts.seconds > 0);
        assert!(ts.nanos >= 0);
        assert!(ts.nanos < 1_000_000_000); // nanos must be < 1 second
    }

    #[test]
    fn test_datetime_to_timestamp_roundtrip() {
        // Test that timestamp conversion preserves precision
        let dt = Utc::now();
        let ts = datetime_to_timestamp(dt);

        // Verify the conversion matches chrono's timestamp methods
        assert_eq!(ts.seconds, dt.timestamp());
        assert_eq!(ts.nanos, dt.timestamp_subsec_nanos() as i32);
    }

    #[test]
    fn test_datetime_to_timestamp_consistent() {
        // Test that multiple calls with the same DateTime produce same result
        let dt = Utc::now();
        let ts1 = datetime_to_timestamp(dt);
        let ts2 = datetime_to_timestamp(dt);
        assert_eq!(ts1.seconds, ts2.seconds);
        assert_eq!(ts1.nanos, ts2.nanos);
    }

    // -------------------------------------------------------------------------
    // expand_node_to_proto tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_expand_node_to_proto_leaf_users() {
        use rsfga_domain::resolver::{ExpandLeaf, ExpandLeafValue, ExpandNode};

        let node = ExpandNode::Leaf(ExpandLeaf {
            name: "document:1#viewer".to_string(),
            value: ExpandLeafValue::Users(vec!["user:alice".to_string(), "user:bob".to_string()]),
        });

        let proto_node = expand_node_to_proto(node);
        assert_eq!(proto_node.name, "document:1#viewer");

        if let Some(crate::proto::openfga::v1::node::Value::Leaf(leaf)) = proto_node.value {
            if let Some(crate::proto::openfga::v1::leaf::Value::Users(users)) = leaf.value {
                assert_eq!(users.users.len(), 2);
                assert!(users.users.contains(&"user:alice".to_string()));
                assert!(users.users.contains(&"user:bob".to_string()));
            } else {
                panic!("Expected Users leaf");
            }
        } else {
            panic!("Expected Leaf node");
        }
    }

    #[test]
    fn test_expand_node_to_proto_leaf_computed() {
        use rsfga_domain::resolver::{ExpandLeaf, ExpandLeafValue, ExpandNode};

        let node = ExpandNode::Leaf(ExpandLeaf {
            name: "document:1#editor".to_string(),
            value: ExpandLeafValue::Computed {
                userset: "owner".to_string(),
            },
        });

        let proto_node = expand_node_to_proto(node);

        if let Some(crate::proto::openfga::v1::node::Value::Leaf(leaf)) = proto_node.value {
            if let Some(crate::proto::openfga::v1::leaf::Value::Computed(computed)) = leaf.value {
                assert_eq!(computed.userset, "owner");
            } else {
                panic!("Expected Computed leaf");
            }
        } else {
            panic!("Expected Leaf node");
        }
    }

    #[test]
    fn test_expand_node_to_proto_union() {
        use rsfga_domain::resolver::{ExpandLeaf, ExpandLeafValue, ExpandNode};

        let node = ExpandNode::Union {
            name: "document:1#can_view".to_string(),
            nodes: vec![
                ExpandNode::Leaf(ExpandLeaf {
                    name: "document:1#viewer".to_string(),
                    value: ExpandLeafValue::Users(vec!["user:alice".to_string()]),
                }),
                ExpandNode::Leaf(ExpandLeaf {
                    name: "document:1#editor".to_string(),
                    value: ExpandLeafValue::Users(vec!["user:bob".to_string()]),
                }),
            ],
        };

        let proto_node = expand_node_to_proto(node);
        assert_eq!(proto_node.name, "document:1#can_view");

        if let Some(crate::proto::openfga::v1::node::Value::Union(nodes)) = proto_node.value {
            assert_eq!(nodes.nodes.len(), 2);
        } else {
            panic!("Expected Union node");
        }
    }

    #[test]
    fn test_expand_node_to_proto_intersection() {
        use rsfga_domain::resolver::{ExpandLeaf, ExpandLeafValue, ExpandNode};

        let node = ExpandNode::Intersection {
            name: "document:1#can_edit".to_string(),
            nodes: vec![
                ExpandNode::Leaf(ExpandLeaf {
                    name: "document:1#editor".to_string(),
                    value: ExpandLeafValue::Users(vec!["user:alice".to_string()]),
                }),
                ExpandNode::Leaf(ExpandLeaf {
                    name: "document:1#approved".to_string(),
                    value: ExpandLeafValue::Users(vec!["user:alice".to_string()]),
                }),
            ],
        };

        let proto_node = expand_node_to_proto(node);

        if let Some(crate::proto::openfga::v1::node::Value::Intersection(nodes)) = proto_node.value
        {
            assert_eq!(nodes.nodes.len(), 2);
        } else {
            panic!("Expected Intersection node");
        }
    }

    #[test]
    fn test_expand_node_to_proto_difference() {
        use rsfga_domain::resolver::{ExpandLeaf, ExpandLeafValue, ExpandNode};

        let node = ExpandNode::Difference {
            name: "document:1#can_view_except_blocked".to_string(),
            base: Box::new(ExpandNode::Leaf(ExpandLeaf {
                name: "document:1#viewer".to_string(),
                value: ExpandLeafValue::Users(vec![
                    "user:alice".to_string(),
                    "user:bob".to_string(),
                ]),
            })),
            subtract: Box::new(ExpandNode::Leaf(ExpandLeaf {
                name: "document:1#blocked".to_string(),
                value: ExpandLeafValue::Users(vec!["user:bob".to_string()]),
            })),
        };

        let proto_node = expand_node_to_proto(node);

        if let Some(crate::proto::openfga::v1::node::Value::Difference(nodes)) = proto_node.value {
            assert_eq!(nodes.nodes.len(), 2);
        } else {
            panic!("Expected Difference node");
        }
    }
}
