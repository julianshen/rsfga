//! Proto-JSON conversion utilities for gRPC services.
//!
//! This module provides bidirectional conversion functions between:
//! - `prost_types::Value` / `prost_types::Struct` and `serde_json::Value`
//! - Domain types and proto types
//!
//! # Design Decisions
//!
//! - **Ownership-based API**: Functions take ownership to avoid clones in the happy path.
//!   Errors return the original values for error messages.
//! - **NaN/Infinity rejection**: JSON does not support these values, so conversions fail.
//! - **Validation integration**: Uses shared validation from `crate::validation`.

use std::collections::HashMap;

use rsfga_storage::{DateTime, StoredTuple, Utc};

use crate::proto::openfga::v1::{
    Computed, Leaf, Node, Nodes, ObjectRelation, TupleKey, TupleToUserset, Users,
};
use crate::validation::{is_valid_condition_name, prost_value_exceeds_max_depth};

/// Maximum size for condition context in bytes (10KB).
pub const MAX_CONDITION_CONTEXT_SIZE: usize = 10 * 1024;

// =============================================================================
// Prost → JSON Conversions
// =============================================================================

/// Converts a `prost_types::Value` to `serde_json::Value` (takes ownership to avoid clones).
///
/// # Errors
///
/// Returns `Err` if the value contains NaN or Infinity (invalid JSON numbers).
///
/// # Note
///
/// Protobuf NumberValue uses f64, which loses precision for integers > 2^53.
/// This is a known limitation of the protocol.
pub fn prost_value_to_json(value: prost_types::Value) -> Result<serde_json::Value, &'static str> {
    use prost_types::value::Kind;

    match value.kind {
        Some(Kind::NullValue(_)) => Ok(serde_json::Value::Null),
        Some(Kind::NumberValue(n)) => {
            // Reject NaN and Infinity as they are not valid JSON values.
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

/// Converts a `prost_types::Struct` to `serde_json::Value` (takes ownership to avoid clones).
///
/// # Errors
///
/// Returns `Err` if any value contains NaN or Infinity.
pub fn prost_struct_to_json(s: prost_types::Struct) -> Result<serde_json::Value, &'static str> {
    let mut map = serde_json::Map::new();
    for (k, v) in s.fields {
        map.insert(k, prost_value_to_json(v)?);
    }
    Ok(serde_json::Value::Object(map))
}

/// Converts a `prost_types::Struct` to `HashMap<String, serde_json::Value>` (takes ownership).
///
/// # Errors
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

// =============================================================================
// JSON → Prost Conversions
// =============================================================================

/// Converts a `serde_json::Value` to `prost_types::Value`.
///
/// This function is infallible since all JSON values can be represented in proto.
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

/// Converts a serde_json Map to `prost_types::Struct`.
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

/// Converts a `HashMap<String, serde_json::Value>` to `prost_types::Struct`.
pub fn hashmap_to_prost_struct(map: HashMap<String, serde_json::Value>) -> prost_types::Struct {
    prost_types::Struct {
        fields: map
            .into_iter()
            .map(|(k, v)| (k, json_to_prost_value(v)))
            .collect(),
    }
}

// =============================================================================
// Tuple Key Conversion
// =============================================================================

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

// =============================================================================
// Timestamp Conversion
// =============================================================================

/// Converts a chrono `DateTime<Utc>` to a `prost_types::Timestamp`.
pub fn datetime_to_timestamp(dt: DateTime<Utc>) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: dt.timestamp(),
        nanos: dt.timestamp_subsec_nanos() as i32,
    }
}

// =============================================================================
// Expand Node Conversion
// =============================================================================

/// Converts a domain `ExpandNode` to a proto `Node`.
pub fn expand_node_to_proto(node: rsfga_domain::resolver::ExpandNode) -> Node {
    use rsfga_domain::resolver::{ExpandLeafValue, ExpandNode as DomainExpandNode};

    match node {
        DomainExpandNode::Leaf(leaf) => {
            // Extract object from leaf.name (format: "type:id#relation") before moving name
            // The tupleset relation is on the same object being expanded
            let object_for_tupleset = leaf.name.split('#').next().unwrap_or("").to_string();
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

// =============================================================================
// Unit Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use prost_types::value::Kind;
    use serde_json::json;

    // -------------------------------------------------------------------------
    // prost_value_to_json tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_prost_value_to_json_null() {
        let value = prost_types::Value {
            kind: Some(Kind::NullValue(0)),
        };
        let result = prost_value_to_json(value).unwrap();
        assert_eq!(result, serde_json::Value::Null);
    }

    #[test]
    fn test_prost_value_to_json_none_kind() {
        let value = prost_types::Value { kind: None };
        let result = prost_value_to_json(value).unwrap();
        assert_eq!(result, serde_json::Value::Null);
    }

    #[test]
    fn test_prost_value_to_json_bool_true() {
        let value = prost_types::Value {
            kind: Some(Kind::BoolValue(true)),
        };
        let result = prost_value_to_json(value).unwrap();
        assert_eq!(result, json!(true));
    }

    #[test]
    fn test_prost_value_to_json_bool_false() {
        let value = prost_types::Value {
            kind: Some(Kind::BoolValue(false)),
        };
        let result = prost_value_to_json(value).unwrap();
        assert_eq!(result, json!(false));
    }

    #[test]
    fn test_prost_value_to_json_number_integer() {
        let value = prost_types::Value {
            kind: Some(Kind::NumberValue(42.0)),
        };
        let result = prost_value_to_json(value).unwrap();
        assert_eq!(result, json!(42.0));
    }

    #[test]
    fn test_prost_value_to_json_number_float() {
        let value = prost_types::Value {
            kind: Some(Kind::NumberValue(1.23456)),
        };
        let result = prost_value_to_json(value).unwrap();
        assert_eq!(result, json!(1.23456));
    }

    #[test]
    fn test_prost_value_to_json_number_negative() {
        let value = prost_types::Value {
            kind: Some(Kind::NumberValue(-123.456)),
        };
        let result = prost_value_to_json(value).unwrap();
        assert_eq!(result, json!(-123.456));
    }

    #[test]
    fn test_prost_value_to_json_number_zero() {
        let value = prost_types::Value {
            kind: Some(Kind::NumberValue(0.0)),
        };
        let result = prost_value_to_json(value).unwrap();
        assert_eq!(result, json!(0.0));
    }

    #[test]
    fn test_prost_value_to_json_nan_rejected() {
        let value = prost_types::Value {
            kind: Some(Kind::NumberValue(f64::NAN)),
        };
        let result = prost_value_to_json(value);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "NaN and Infinity are not valid JSON numbers"
        );
    }

    #[test]
    fn test_prost_value_to_json_positive_infinity_rejected() {
        let value = prost_types::Value {
            kind: Some(Kind::NumberValue(f64::INFINITY)),
        };
        let result = prost_value_to_json(value);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "NaN and Infinity are not valid JSON numbers"
        );
    }

    #[test]
    fn test_prost_value_to_json_negative_infinity_rejected() {
        let value = prost_types::Value {
            kind: Some(Kind::NumberValue(f64::NEG_INFINITY)),
        };
        let result = prost_value_to_json(value);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "NaN and Infinity are not valid JSON numbers"
        );
    }

    #[test]
    fn test_prost_value_to_json_string() {
        let value = prost_types::Value {
            kind: Some(Kind::StringValue("hello world".to_string())),
        };
        let result = prost_value_to_json(value).unwrap();
        assert_eq!(result, json!("hello world"));
    }

    #[test]
    fn test_prost_value_to_json_empty_string() {
        let value = prost_types::Value {
            kind: Some(Kind::StringValue(String::new())),
        };
        let result = prost_value_to_json(value).unwrap();
        assert_eq!(result, json!(""));
    }

    #[test]
    fn test_prost_value_to_json_string_with_unicode() {
        let value = prost_types::Value {
            kind: Some(Kind::StringValue("hello 世界 🌍".to_string())),
        };
        let result = prost_value_to_json(value).unwrap();
        assert_eq!(result, json!("hello 世界 🌍"));
    }

    #[test]
    fn test_prost_value_to_json_empty_list() {
        let value = prost_types::Value {
            kind: Some(Kind::ListValue(prost_types::ListValue { values: vec![] })),
        };
        let result = prost_value_to_json(value).unwrap();
        assert_eq!(result, json!([]));
    }

    #[test]
    fn test_prost_value_to_json_list_with_values() {
        let value = prost_types::Value {
            kind: Some(Kind::ListValue(prost_types::ListValue {
                values: vec![
                    prost_types::Value {
                        kind: Some(Kind::NumberValue(1.0)),
                    },
                    prost_types::Value {
                        kind: Some(Kind::StringValue("two".to_string())),
                    },
                    prost_types::Value {
                        kind: Some(Kind::BoolValue(true)),
                    },
                ],
            })),
        };
        let result = prost_value_to_json(value).unwrap();
        assert_eq!(result, json!([1.0, "two", true]));
    }

    #[test]
    fn test_prost_value_to_json_list_with_nan_rejected() {
        let value = prost_types::Value {
            kind: Some(Kind::ListValue(prost_types::ListValue {
                values: vec![
                    prost_types::Value {
                        kind: Some(Kind::NumberValue(1.0)),
                    },
                    prost_types::Value {
                        kind: Some(Kind::NumberValue(f64::NAN)),
                    },
                ],
            })),
        };
        let result = prost_value_to_json(value);
        assert!(result.is_err());
    }

    #[test]
    fn test_prost_value_to_json_nested_list() {
        let inner_list = prost_types::Value {
            kind: Some(Kind::ListValue(prost_types::ListValue {
                values: vec![prost_types::Value {
                    kind: Some(Kind::NumberValue(42.0)),
                }],
            })),
        };
        let value = prost_types::Value {
            kind: Some(Kind::ListValue(prost_types::ListValue {
                values: vec![inner_list],
            })),
        };
        let result = prost_value_to_json(value).unwrap();
        assert_eq!(result, json!([[42.0]]));
    }

    // -------------------------------------------------------------------------
    // prost_struct_to_json tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_prost_struct_to_json_empty() {
        let s = prost_types::Struct {
            fields: std::collections::BTreeMap::new(),
        };
        let result = prost_struct_to_json(s).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn test_prost_struct_to_json_simple() {
        let mut fields = std::collections::BTreeMap::new();
        fields.insert(
            "name".to_string(),
            prost_types::Value {
                kind: Some(Kind::StringValue("Alice".to_string())),
            },
        );
        fields.insert(
            "age".to_string(),
            prost_types::Value {
                kind: Some(Kind::NumberValue(30.0)),
            },
        );
        let s = prost_types::Struct { fields };
        let result = prost_struct_to_json(s).unwrap();
        assert_eq!(result["name"], json!("Alice"));
        assert_eq!(result["age"], json!(30.0));
    }

    #[test]
    fn test_prost_struct_to_json_with_nan_rejected() {
        let mut fields = std::collections::BTreeMap::new();
        fields.insert(
            "bad".to_string(),
            prost_types::Value {
                kind: Some(Kind::NumberValue(f64::NAN)),
            },
        );
        let s = prost_types::Struct { fields };
        let result = prost_struct_to_json(s);
        assert!(result.is_err());
    }

    #[test]
    fn test_prost_struct_to_json_nested() {
        let mut inner_fields = std::collections::BTreeMap::new();
        inner_fields.insert(
            "nested_key".to_string(),
            prost_types::Value {
                kind: Some(Kind::StringValue("nested_value".to_string())),
            },
        );
        let inner_struct = prost_types::Struct {
            fields: inner_fields,
        };

        let mut fields = std::collections::BTreeMap::new();
        fields.insert(
            "inner".to_string(),
            prost_types::Value {
                kind: Some(Kind::StructValue(inner_struct)),
            },
        );
        let s = prost_types::Struct { fields };
        let result = prost_struct_to_json(s).unwrap();
        assert_eq!(result["inner"]["nested_key"], json!("nested_value"));
    }

    // -------------------------------------------------------------------------
    // prost_struct_to_hashmap tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_prost_struct_to_hashmap_empty() {
        let s = prost_types::Struct {
            fields: std::collections::BTreeMap::new(),
        };
        let result = prost_struct_to_hashmap(s).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_prost_struct_to_hashmap_simple() {
        let mut fields = std::collections::BTreeMap::new();
        fields.insert(
            "key1".to_string(),
            prost_types::Value {
                kind: Some(Kind::StringValue("value1".to_string())),
            },
        );
        fields.insert(
            "key2".to_string(),
            prost_types::Value {
                kind: Some(Kind::NumberValue(123.0)),
            },
        );
        let s = prost_types::Struct { fields };
        let result = prost_struct_to_hashmap(s).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result.get("key1"), Some(&json!("value1")));
        assert_eq!(result.get("key2"), Some(&json!(123.0)));
    }

    #[test]
    fn test_prost_struct_to_hashmap_with_nan_rejected() {
        let mut fields = std::collections::BTreeMap::new();
        fields.insert(
            "bad".to_string(),
            prost_types::Value {
                kind: Some(Kind::NumberValue(f64::INFINITY)),
            },
        );
        let s = prost_types::Struct { fields };
        let result = prost_struct_to_hashmap(s);
        assert!(result.is_err());
    }

    // -------------------------------------------------------------------------
    // json_to_prost_value tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_json_to_prost_value_null() {
        let json = serde_json::Value::Null;
        let result = json_to_prost_value(json);
        assert!(matches!(result.kind, Some(Kind::NullValue(0))));
    }

    #[test]
    fn test_json_to_prost_value_bool_true() {
        let json = json!(true);
        let result = json_to_prost_value(json);
        assert!(matches!(result.kind, Some(Kind::BoolValue(true))));
    }

    #[test]
    fn test_json_to_prost_value_bool_false() {
        let json = json!(false);
        let result = json_to_prost_value(json);
        assert!(matches!(result.kind, Some(Kind::BoolValue(false))));
    }

    #[test]
    fn test_json_to_prost_value_number() {
        let json = json!(42.5);
        let result = json_to_prost_value(json);
        if let Some(Kind::NumberValue(n)) = result.kind {
            assert!((n - 42.5).abs() < f64::EPSILON);
        } else {
            panic!("Expected NumberValue");
        }
    }

    #[test]
    fn test_json_to_prost_value_integer() {
        let json = json!(100);
        let result = json_to_prost_value(json);
        if let Some(Kind::NumberValue(n)) = result.kind {
            assert!((n - 100.0).abs() < f64::EPSILON);
        } else {
            panic!("Expected NumberValue");
        }
    }

    #[test]
    fn test_json_to_prost_value_string() {
        let json = json!("hello");
        let result = json_to_prost_value(json);
        assert!(matches!(result.kind, Some(Kind::StringValue(s)) if s == "hello"));
    }

    #[test]
    fn test_json_to_prost_value_array() {
        let json = json!([1, 2, 3]);
        let result = json_to_prost_value(json);
        if let Some(Kind::ListValue(list)) = result.kind {
            assert_eq!(list.values.len(), 3);
        } else {
            panic!("Expected ListValue");
        }
    }

    #[test]
    fn test_json_to_prost_value_object() {
        let json = json!({"key": "value"});
        let result = json_to_prost_value(json);
        if let Some(Kind::StructValue(s)) = result.kind {
            assert!(s.fields.contains_key("key"));
        } else {
            panic!("Expected StructValue");
        }
    }

    // -------------------------------------------------------------------------
    // json_map_to_prost_struct tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_json_map_to_prost_struct_empty() {
        let map = serde_json::Map::new();
        let result = json_map_to_prost_struct(map);
        assert!(result.fields.is_empty());
    }

    #[test]
    fn test_json_map_to_prost_struct_simple() {
        let mut map = serde_json::Map::new();
        map.insert("name".to_string(), json!("Bob"));
        map.insert("active".to_string(), json!(true));
        let result = json_map_to_prost_struct(map);
        assert_eq!(result.fields.len(), 2);
        assert!(result.fields.contains_key("name"));
        assert!(result.fields.contains_key("active"));
    }

    // -------------------------------------------------------------------------
    // hashmap_to_prost_struct tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_hashmap_to_prost_struct_empty() {
        let map: HashMap<String, serde_json::Value> = HashMap::new();
        let result = hashmap_to_prost_struct(map);
        assert!(result.fields.is_empty());
    }

    #[test]
    fn test_hashmap_to_prost_struct_simple() {
        let mut map: HashMap<String, serde_json::Value> = HashMap::new();
        map.insert("count".to_string(), json!(42));
        map.insert("enabled".to_string(), json!(false));
        let result = hashmap_to_prost_struct(map);
        assert_eq!(result.fields.len(), 2);
        assert!(result.fields.contains_key("count"));
        assert!(result.fields.contains_key("enabled"));
    }

    // -------------------------------------------------------------------------
    // Roundtrip tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_roundtrip_simple_struct() {
        // JSON -> Prost -> JSON
        let original = json!({"name": "test", "value": 123.0, "active": true});
        let map = original.as_object().unwrap().clone();
        let prost_struct = json_map_to_prost_struct(map);
        let back_to_json = prost_struct_to_json(prost_struct).unwrap();
        assert_eq!(original, back_to_json);
    }

    #[test]
    fn test_roundtrip_nested_struct() {
        let original = json!({
            "outer": {
                "inner": {
                    "value": 42.0
                }
            }
        });
        let map = original.as_object().unwrap().clone();
        let prost_struct = json_map_to_prost_struct(map);
        let back_to_json = prost_struct_to_json(prost_struct).unwrap();
        assert_eq!(original, back_to_json);
    }

    #[test]
    fn test_roundtrip_with_arrays() {
        let original = json!({
            "items": [1.0, 2.0, 3.0],
            "names": ["a", "b", "c"]
        });
        let map = original.as_object().unwrap().clone();
        let prost_struct = json_map_to_prost_struct(map);
        let back_to_json = prost_struct_to_json(prost_struct).unwrap();
        assert_eq!(original, back_to_json);
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
        assert_eq!(ctx.get("ip_address"), Some(&json!("192.168.1.1")));
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
