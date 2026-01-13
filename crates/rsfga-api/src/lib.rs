//! rsfga-api: HTTP and gRPC API layer
//!
//! This crate provides the API layer including:
//! - HTTP REST endpoints via Axum
//! - gRPC services via Tonic
//! - Middleware (auth, metrics, tracing)
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │                 rsfga-api                    │
//! ├─────────────────────────────────────────────┤
//! │  proto/      - Generated protobuf types     │
//! │  http/       - HTTP REST endpoints          │
//! │  grpc/       - gRPC service implementations │
//! │  middleware/ - Auth, metrics, tracing       │
//! └─────────────────────────────────────────────┘
//! ```

pub mod adapters;
pub mod grpc;
pub mod http;
pub mod middleware;
pub mod observability;
pub mod utils;

#[cfg(test)]
mod compatibility;

/// OpenFGA-compatible protobuf types and service definitions.
///
/// Generated from proto files at compile time via tonic-build.
pub mod proto {
    /// OpenFGA v1 API types and service.
    pub mod openfga {
        pub mod v1 {
            tonic::include_proto!("openfga.v1");
        }
    }
}

// Re-export commonly used types
pub use proto::openfga::v1::*;

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    // Section 1: Protobuf Definitions

    /// Test: Protobuf files compile
    ///
    /// This test verifies that the protobuf files compile successfully
    /// and generate valid Rust types. The fact that this module compiles
    /// at all proves the proto files are syntactically correct.
    #[test]
    fn test_protobuf_files_compile() {
        // If this test runs, proto files compiled successfully.
        // Create instances of key types to verify they exist.
        let _tuple_key = TupleKey::default();
        let _check_request = CheckRequest::default();
        let _check_response = CheckResponse::default();
        let _batch_check_request = BatchCheckRequest::default();
        let _batch_check_response = BatchCheckResponse::default();
        let _write_request = WriteRequest::default();
        let _read_request = ReadRequest::default();
        let _authorization_model = AuthorizationModel::default();
        let _store = Store::default();
    }

    /// Test: Generated Rust code is identical to OpenFGA types
    ///
    /// Verifies the generated types have the expected fields matching
    /// the OpenFGA protobuf definitions. This ensures wire compatibility.
    #[test]
    fn test_generated_rust_code_matches_openfga_types() {
        // TupleKey fields
        let tuple_key = TupleKey {
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:readme".to_string(),
            condition: None,
        };
        assert_eq!(tuple_key.user, "user:alice");
        assert_eq!(tuple_key.relation, "viewer");
        assert_eq!(tuple_key.object, "document:readme");

        // CheckRequest fields
        let check_req = CheckRequest {
            store_id: "store-123".to_string(),
            tuple_key: Some(tuple_key.clone()),
            contextual_tuples: None,
            authorization_model_id: "model-456".to_string(),
            trace: false,
            context: None,
            consistency: ConsistencyPreference::Unspecified as i32,
        };
        assert_eq!(check_req.store_id, "store-123");
        assert_eq!(check_req.authorization_model_id, "model-456");

        // CheckResponse fields
        let check_resp = CheckResponse {
            allowed: true,
            resolution: "direct".to_string(),
        };
        assert!(check_resp.allowed);
        assert_eq!(check_resp.resolution, "direct");

        // BatchCheckRequest fields
        let batch_req = BatchCheckRequest {
            store_id: "store-123".to_string(),
            checks: vec![BatchCheckItem {
                tuple_key: Some(tuple_key.clone()),
                contextual_tuples: None,
                context: None,
                correlation_id: "check-1".to_string(),
            }],
            authorization_model_id: "model-456".to_string(),
            consistency: ConsistencyPreference::HigherConsistency as i32,
        };
        assert_eq!(batch_req.checks.len(), 1);
        assert_eq!(batch_req.checks[0].correlation_id, "check-1");

        // BatchCheckResponse with map result
        let mut batch_resp = BatchCheckResponse::default();
        batch_resp.result.insert(
            "check-1".to_string(),
            BatchCheckSingleResult {
                allowed: true,
                error: None,
            },
        );
        assert!(batch_resp.result.contains_key("check-1"));
        assert!(batch_resp.result.get("check-1").unwrap().allowed);

        // Store fields
        let store = Store {
            id: "store-123".to_string(),
            name: "Test Store".to_string(),
            created_at: None,
            updated_at: None,
            deleted_at: None,
        };
        assert_eq!(store.id, "store-123");
        assert_eq!(store.name, "Test Store");

        // AuthorizationModel fields
        let model = AuthorizationModel {
            id: "model-456".to_string(),
            schema_version: "1.1".to_string(),
            type_definitions: vec![],
            conditions: Default::default(),
        };
        assert_eq!(model.schema_version, "1.1");
    }

    /// Test: Can serialize/deserialize Check request
    ///
    /// Verifies CheckRequest can be encoded to bytes and decoded back,
    /// ensuring wire format compatibility with OpenFGA.
    #[test]
    fn test_serialize_deserialize_check_request() {
        let tuple_key = TupleKey {
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:readme".to_string(),
            condition: None,
        };

        let original = CheckRequest {
            store_id: "store-123".to_string(),
            tuple_key: Some(tuple_key),
            contextual_tuples: Some(ContextualTupleKeys {
                tuple_keys: vec![TupleKey {
                    user: "user:bob".to_string(),
                    relation: "editor".to_string(),
                    object: "document:readme".to_string(),
                    condition: None,
                }],
            }),
            authorization_model_id: "model-456".to_string(),
            trace: true,
            context: None,
            consistency: ConsistencyPreference::HigherConsistency as i32,
        };

        // Serialize to bytes
        let mut buf = Vec::new();
        original
            .encode(&mut buf)
            .expect("Failed to encode CheckRequest");
        assert!(!buf.is_empty(), "Encoded bytes should not be empty");

        // Deserialize back
        let decoded = CheckRequest::decode(&buf[..]).expect("Failed to decode CheckRequest");

        // Verify round-trip
        assert_eq!(decoded.store_id, original.store_id);
        assert_eq!(
            decoded.authorization_model_id,
            original.authorization_model_id
        );
        assert_eq!(decoded.trace, original.trace);
        assert_eq!(decoded.consistency, original.consistency);

        let decoded_tuple = decoded
            .tuple_key
            .as_ref()
            .expect("tuple_key should be present");
        let original_tuple = original.tuple_key.as_ref().unwrap();
        assert_eq!(decoded_tuple.user, original_tuple.user);
        assert_eq!(decoded_tuple.relation, original_tuple.relation);
        assert_eq!(decoded_tuple.object, original_tuple.object);

        let decoded_ctx = decoded
            .contextual_tuples
            .as_ref()
            .expect("contextual_tuples should be present");
        assert_eq!(decoded_ctx.tuple_keys.len(), 1);
        assert_eq!(decoded_ctx.tuple_keys[0].user, "user:bob");
    }

    /// Test: Can serialize/deserialize Check response
    ///
    /// Verifies CheckResponse can be encoded and decoded correctly.
    #[test]
    fn test_serialize_deserialize_check_response() {
        let original = CheckResponse {
            allowed: true,
            resolution: "union.computed".to_string(),
        };

        // Serialize
        let mut buf = Vec::new();
        original
            .encode(&mut buf)
            .expect("Failed to encode CheckResponse");
        assert!(!buf.is_empty());

        // Deserialize
        let decoded = CheckResponse::decode(&buf[..]).expect("Failed to decode CheckResponse");

        assert_eq!(decoded.allowed, original.allowed);
        assert_eq!(decoded.resolution, original.resolution);

        // Test false response too
        let false_response = CheckResponse {
            allowed: false,
            resolution: String::new(),
        };
        let mut buf2 = Vec::new();
        false_response.encode(&mut buf2).unwrap();
        let decoded2 = CheckResponse::decode(&buf2[..]).unwrap();
        assert!(!decoded2.allowed);
    }

    /// Test: Can serialize/deserialize all request types
    ///
    /// Verifies all major request/response types can be serialized
    /// and deserialized correctly.
    #[test]
    fn test_serialize_deserialize_all_request_types() {
        // BatchCheckRequest
        {
            let original = BatchCheckRequest {
                store_id: "store-123".to_string(),
                checks: vec![
                    BatchCheckItem {
                        tuple_key: Some(TupleKey {
                            user: "user:alice".to_string(),
                            relation: "viewer".to_string(),
                            object: "doc:1".to_string(),
                            condition: None,
                        }),
                        contextual_tuples: None,
                        context: None,
                        correlation_id: "check-1".to_string(),
                    },
                    BatchCheckItem {
                        tuple_key: Some(TupleKey {
                            user: "user:bob".to_string(),
                            relation: "editor".to_string(),
                            object: "doc:2".to_string(),
                            condition: None,
                        }),
                        contextual_tuples: None,
                        context: None,
                        correlation_id: "check-2".to_string(),
                    },
                ],
                authorization_model_id: "model-456".to_string(),
                consistency: ConsistencyPreference::MinimizeLatency as i32,
            };

            let mut buf = Vec::new();
            original.encode(&mut buf).unwrap();
            let decoded = BatchCheckRequest::decode(&buf[..]).unwrap();

            assert_eq!(decoded.store_id, original.store_id);
            assert_eq!(decoded.checks.len(), 2);
            assert_eq!(decoded.checks[0].correlation_id, "check-1");
            assert_eq!(decoded.checks[1].correlation_id, "check-2");
        }

        // BatchCheckResponse
        {
            let mut original = BatchCheckResponse::default();
            original.result.insert(
                "check-1".to_string(),
                BatchCheckSingleResult {
                    allowed: true,
                    error: None,
                },
            );
            original.result.insert(
                "check-2".to_string(),
                BatchCheckSingleResult {
                    allowed: false,
                    error: Some(CheckError {
                        code: ErrorCode::ValidationError as i32,
                        message: "Invalid input".to_string(),
                    }),
                },
            );

            let mut buf = Vec::new();
            original.encode(&mut buf).unwrap();
            let decoded = BatchCheckResponse::decode(&buf[..]).unwrap();

            assert_eq!(decoded.result.len(), 2);
            assert!(decoded.result.get("check-1").unwrap().allowed);
            assert!(!decoded.result.get("check-2").unwrap().allowed);
            assert!(decoded.result.get("check-2").unwrap().error.is_some());
        }

        // WriteRequest
        {
            let original = WriteRequest {
                store_id: "store-123".to_string(),
                writes: Some(WriteRequestWrites {
                    tuple_keys: vec![TupleKey {
                        user: "user:alice".to_string(),
                        relation: "viewer".to_string(),
                        object: "doc:1".to_string(),
                        condition: None,
                    }],
                }),
                deletes: Some(WriteRequestDeletes {
                    tuple_keys: vec![TupleKeyWithoutCondition {
                        user: "user:bob".to_string(),
                        relation: "editor".to_string(),
                        object: "doc:2".to_string(),
                    }],
                }),
                authorization_model_id: "model-456".to_string(),
            };

            let mut buf = Vec::new();
            original.encode(&mut buf).unwrap();
            let decoded = WriteRequest::decode(&buf[..]).unwrap();

            assert_eq!(decoded.store_id, original.store_id);
            assert!(decoded.writes.is_some());
            assert!(decoded.deletes.is_some());
            assert_eq!(decoded.writes.unwrap().tuple_keys.len(), 1);
            assert_eq!(decoded.deletes.unwrap().tuple_keys.len(), 1);
        }

        // ReadRequest
        {
            let original = ReadRequest {
                store_id: "store-123".to_string(),
                tuple_key: Some(TupleKeyWithoutCondition {
                    user: "user:alice".to_string(),
                    relation: String::new(),
                    object: String::new(),
                }),
                page_size: Some(100),
                continuation_token: "token-abc".to_string(),
                consistency: ConsistencyPreference::HigherConsistency as i32,
            };

            let mut buf = Vec::new();
            original.encode(&mut buf).unwrap();
            let decoded = ReadRequest::decode(&buf[..]).unwrap();

            assert_eq!(decoded.store_id, original.store_id);
            assert_eq!(decoded.page_size, Some(100));
            assert_eq!(decoded.continuation_token, "token-abc");
        }

        // ExpandRequest
        {
            let original = ExpandRequest {
                store_id: "store-123".to_string(),
                tuple_key: Some(TupleKey {
                    user: String::new(),
                    relation: "viewer".to_string(),
                    object: "doc:1".to_string(),
                    condition: None,
                }),
                authorization_model_id: "model-456".to_string(),
                consistency: ConsistencyPreference::Unspecified as i32,
            };

            let mut buf = Vec::new();
            original.encode(&mut buf).unwrap();
            let decoded = ExpandRequest::decode(&buf[..]).unwrap();

            assert_eq!(decoded.store_id, original.store_id);
            assert!(decoded.tuple_key.is_some());
        }

        // CreateStoreRequest
        {
            let original = CreateStoreRequest {
                name: "My Test Store".to_string(),
            };

            let mut buf = Vec::new();
            original.encode(&mut buf).unwrap();
            let decoded = CreateStoreRequest::decode(&buf[..]).unwrap();

            assert_eq!(decoded.name, "My Test Store");
        }

        // WriteAuthorizationModelRequest
        {
            let original = WriteAuthorizationModelRequest {
                store_id: "store-123".to_string(),
                type_definitions: vec![TypeDefinition {
                    r#type: "document".to_string(),
                    relations: Default::default(),
                    metadata: None,
                }],
                schema_version: "1.1".to_string(),
                conditions: Default::default(),
            };

            let mut buf = Vec::new();
            original.encode(&mut buf).unwrap();
            let decoded = WriteAuthorizationModelRequest::decode(&buf[..]).unwrap();

            assert_eq!(decoded.store_id, "store-123");
            assert_eq!(decoded.type_definitions.len(), 1);
            assert_eq!(decoded.type_definitions[0].r#type, "document");
            assert_eq!(decoded.schema_version, "1.1");
        }
    }
}
