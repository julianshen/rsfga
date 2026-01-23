//! gRPC API tests for Section 3.

use std::sync::Arc;

use tonic::Request;

use rsfga_domain::cel::global_cache;
use rsfga_storage::{DataStore, MemoryDataStore, StoredAuthorizationModel, StoredTuple};

use crate::proto::openfga::v1::open_fga_service_server::OpenFgaService;
use crate::proto::openfga::v1::*;

use super::service::OpenFgaGrpcService;

/// Helper to create a test service with in-memory storage.
fn test_service() -> OpenFgaGrpcService<MemoryDataStore> {
    let storage = Arc::new(MemoryDataStore::new());
    OpenFgaGrpcService::new(storage)
}

/// Helper to create a test service with pre-configured storage.
fn test_service_with_storage(storage: Arc<MemoryDataStore>) -> OpenFgaGrpcService<MemoryDataStore> {
    OpenFgaGrpcService::new(storage)
}

/// Helper to create a simple authorization model with document and user types.
/// This model allows direct tuple assignments (user:X viewer document:Y).
fn simple_model_json() -> String {
    r#"{
        "type_definitions": [
            {
                "type": "user"
            },
            {
                "type": "document",
                "relations": {
                    "viewer": {},
                    "editor": {},
                    "owner": {}
                }
            }
        ]
    }"#
    .to_string()
}

/// Helper function to create an authorization model JSON with conditions.
/// This model includes conditions used in condition-related tests.
fn model_with_conditions_json() -> String {
    r#"{
        "type_definitions": [
            {"type": "user"},
            {"type": "document", "relations": {"viewer": {}, "editor": {}, "owner": {}}}
        ],
        "conditions": {
            "ip_restriction": {
                "name": "ip_restriction",
                "expression": "true",
                "parameters": {}
            },
            "ip_restriction-v2": {
                "name": "ip_restriction-v2",
                "expression": "true",
                "parameters": {}
            },
            "time_based_access": {
                "name": "time_based_access",
                "expression": "true",
                "parameters": {}
            },
            "valid_condition": {
                "name": "valid_condition",
                "expression": "true",
                "parameters": {}
            },
            "valid-condition-with-dashes": {
                "name": "valid-condition-with-dashes",
                "expression": "true",
                "parameters": {}
            },
            "age_restriction": {
                "name": "age_restriction",
                "expression": "true",
                "parameters": {}
            }
        }
    }"#
    .to_string()
}

/// Helper to create and write a simple authorization model to a store.
async fn setup_simple_model(storage: &MemoryDataStore, store_id: &str) -> String {
    let model = StoredAuthorizationModel::new(
        ulid::Ulid::new().to_string(),
        store_id,
        "1.1",
        simple_model_json(),
    );
    let model_id = model.id.clone();
    storage.write_authorization_model(model).await.unwrap();
    model_id
}

/// Helper to create and write an authorization model with conditions to a store.
async fn setup_model_with_conditions(storage: &MemoryDataStore, store_id: &str) -> String {
    let model = StoredAuthorizationModel::new(
        ulid::Ulid::new().to_string(),
        store_id,
        "1.1",
        model_with_conditions_json(),
    );
    let model_id = model.id.clone();
    storage.write_authorization_model(model).await.unwrap();
    model_id
}

/// Test: gRPC server starts
///
/// Verifies the gRPC service can be instantiated and basic operations work.
#[tokio::test]
async fn test_grpc_server_starts() {
    let service = test_service();

    // Create a store to verify service is functional
    let request = Request::new(CreateStoreRequest {
        name: "Test Store".to_string(),
    });

    let response = service.create_store(request).await;
    assert!(response.is_ok());

    let store = response.unwrap().into_inner();
    assert!(!store.id.is_empty());
    assert_eq!(store.name, "Test Store");
    assert!(store.created_at.is_some());
    assert!(store.updated_at.is_some());
}

/// Test: Check RPC works correctly
///
/// Verifies the Check RPC returns correct results for existing and non-existing tuples.
#[tokio::test]
async fn test_check_rpc_works_correctly() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    // Create authorization model
    setup_simple_model(&storage, "test-store").await;

    // Write a tuple
    storage
        .write_tuple(
            "test-store",
            StoredTuple::new("document", "readme", "viewer", "user", "alice", None),
        )
        .await
        .unwrap();

    let service = test_service_with_storage(storage);

    // Check existing tuple - should be allowed
    let request = Request::new(CheckRequest {
        store_id: "test-store".to_string(),
        tuple_key: Some(TupleKey {
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:readme".to_string(),
            condition: None,
        }),
        contextual_tuples: None,
        authorization_model_id: String::new(),
        trace: false,
        context: None,
        consistency: 0,
    });

    let response = service.check(request).await;
    assert!(response.is_ok());
    assert!(response.unwrap().into_inner().allowed);

    // Check non-existing tuple - should not be allowed
    let request = Request::new(CheckRequest {
        store_id: "test-store".to_string(),
        tuple_key: Some(TupleKey {
            user: "user:bob".to_string(),
            relation: "viewer".to_string(),
            object: "document:readme".to_string(),
            condition: None,
        }),
        contextual_tuples: None,
        authorization_model_id: String::new(),
        trace: false,
        context: None,
        consistency: 0,
    });

    let response = service.check(request).await;
    assert!(response.is_ok());
    assert!(!response.unwrap().into_inner().allowed);
}

/// Test: BatchCheck RPC works correctly
///
/// Verifies the BatchCheck RPC processes multiple checks and returns correct results.
#[tokio::test]
async fn test_batch_check_rpc_works_correctly() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    // Set up authorization model (required for GraphResolver)
    setup_simple_model(&storage, "test-store").await;

    // Write tuples
    storage
        .write_tuple(
            "test-store",
            StoredTuple::new("document", "doc1", "viewer", "user", "alice", None),
        )
        .await
        .unwrap();

    let service = test_service_with_storage(storage);

    let request = Request::new(BatchCheckRequest {
        store_id: "test-store".to_string(),
        checks: vec![
            BatchCheckItem {
                tuple_key: Some(TupleKey {
                    user: "user:alice".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:doc1".to_string(),
                    condition: None,
                }),
                contextual_tuples: None,
                context: None,
                correlation_id: "check-1".to_string(),
            },
            BatchCheckItem {
                tuple_key: Some(TupleKey {
                    user: "user:bob".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:doc1".to_string(),
                    condition: None,
                }),
                contextual_tuples: None,
                context: None,
                correlation_id: "check-2".to_string(),
            },
        ],
        authorization_model_id: String::new(),
        consistency: 0,
    });

    let response = service.batch_check(request).await;
    assert!(response.is_ok());

    let result = response.unwrap().into_inner();
    assert_eq!(result.result.len(), 2);
    assert!(result.result.get("check-1").unwrap().allowed);
    assert!(!result.result.get("check-2").unwrap().allowed);
}

/// Test: BatchCheck RPC rejects checks with missing tuple_key
///
/// Verifies that a batch check with a missing tuple_key returns an invalid_argument error.
#[tokio::test]
async fn test_batch_check_rpc_rejects_missing_tuple_key() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    let service = test_service_with_storage(storage);

    // Submit a batch with one valid check and one missing tuple_key
    let request = Request::new(BatchCheckRequest {
        store_id: "test-store".to_string(),
        checks: vec![
            BatchCheckItem {
                tuple_key: Some(TupleKey {
                    user: "user:alice".to_string(),
                    relation: "viewer".to_string(),
                    object: "document:doc1".to_string(),
                    condition: None,
                }),
                contextual_tuples: None,
                context: None,
                correlation_id: "check-1".to_string(),
            },
            BatchCheckItem {
                tuple_key: None, // Missing tuple_key!
                contextual_tuples: None,
                context: None,
                correlation_id: "check-2".to_string(),
            },
        ],
        authorization_model_id: String::new(),
        consistency: 0,
    });

    let response = service.batch_check(request).await;
    assert!(response.is_err());

    let status = response.unwrap_err();
    assert_eq!(status.code(), tonic::Code::InvalidArgument);
    assert!(status.message().contains("tuple_key required"));
    assert!(status.message().contains("index 1"));
}

/// Test: BatchCheck RPC rejects excessively long correlation_ids
///
/// Validates that the API rejects correlation_ids exceeding the maximum length
/// to prevent DoS attacks via excessive memory allocation.
#[tokio::test]
async fn test_batch_check_rpc_rejects_oversized_correlation_id() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    let service = test_service_with_storage(storage);

    // Create a correlation_id that exceeds the 256 byte limit
    let oversized_correlation_id = "x".repeat(300);

    let request = Request::new(BatchCheckRequest {
        store_id: "test-store".to_string(),
        checks: vec![BatchCheckItem {
            tuple_key: Some(TupleKey {
                user: "user:alice".to_string(),
                relation: "viewer".to_string(),
                object: "document:doc1".to_string(),
                condition: None,
            }),
            contextual_tuples: None,
            context: None,
            correlation_id: oversized_correlation_id,
        }],
        authorization_model_id: String::new(),
        consistency: 0,
    });

    let response = service.batch_check(request).await;
    assert!(response.is_err());

    let status = response.unwrap_err();
    assert_eq!(status.code(), tonic::Code::InvalidArgument);
    assert!(status.message().contains("correlation_id"));
    assert!(status.message().contains("exceeds maximum length"));
}

/// Test: BatchCheck RPC accepts correlation_id at exactly 256 bytes
///
/// Edge case: exactly at the limit should be accepted.
#[tokio::test]
async fn test_batch_check_rpc_accepts_max_length_correlation_id() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();
    setup_simple_model(&storage, "test-store").await;

    let service = test_service_with_storage(storage);

    // Create a correlation_id at exactly 256 bytes (the limit)
    let max_length_correlation_id = "x".repeat(256);

    let request = Request::new(BatchCheckRequest {
        store_id: "test-store".to_string(),
        checks: vec![BatchCheckItem {
            tuple_key: Some(TupleKey {
                user: "user:alice".to_string(),
                relation: "viewer".to_string(),
                object: "document:doc1".to_string(),
                condition: None,
            }),
            contextual_tuples: None,
            context: None,
            correlation_id: max_length_correlation_id.clone(),
        }],
        authorization_model_id: String::new(),
        consistency: 0,
    });

    let response = service.batch_check(request).await;
    // Should succeed (not rejected for length)
    assert!(response.is_ok());

    // Verify the correlation_id is preserved in response
    let result = response.unwrap().into_inner();
    assert!(result.result.contains_key(&max_length_correlation_id));
}

/// Test: BatchCheck RPC rejects correlation_id at 257 bytes
///
/// Edge case: exactly one byte over the limit should be rejected.
#[tokio::test]
async fn test_batch_check_rpc_rejects_correlation_id_at_257_bytes() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    let service = test_service_with_storage(storage);

    // Create a correlation_id at 257 bytes (one over the limit)
    let over_limit_correlation_id = "x".repeat(257);

    let request = Request::new(BatchCheckRequest {
        store_id: "test-store".to_string(),
        checks: vec![BatchCheckItem {
            tuple_key: Some(TupleKey {
                user: "user:alice".to_string(),
                relation: "viewer".to_string(),
                object: "document:doc1".to_string(),
                condition: None,
            }),
            contextual_tuples: None,
            context: None,
            correlation_id: over_limit_correlation_id,
        }],
        authorization_model_id: String::new(),
        consistency: 0,
    });

    let response = service.batch_check(request).await;
    assert!(response.is_err());

    let status = response.unwrap_err();
    assert_eq!(status.code(), tonic::Code::InvalidArgument);
    assert!(status.message().contains("exceeds maximum length"));
}

/// Test: BatchCheck RPC accepts empty correlation_id
///
/// Empty strings should be valid correlation IDs.
#[tokio::test]
async fn test_batch_check_rpc_accepts_empty_correlation_id() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();
    setup_simple_model(&storage, "test-store").await;

    let service = test_service_with_storage(storage);

    let request = Request::new(BatchCheckRequest {
        store_id: "test-store".to_string(),
        checks: vec![BatchCheckItem {
            tuple_key: Some(TupleKey {
                user: "user:alice".to_string(),
                relation: "viewer".to_string(),
                object: "document:doc1".to_string(),
                condition: None,
            }),
            contextual_tuples: None,
            context: None,
            correlation_id: String::new(), // Empty correlation ID
        }],
        authorization_model_id: String::new(),
        consistency: 0,
    });

    let response = service.batch_check(request).await;
    // Should succeed
    assert!(response.is_ok());

    // Verify the empty correlation_id is in the response
    let result = response.unwrap().into_inner();
    assert!(result.result.contains_key(""));
}

/// Test: BatchCheck RPC enforces MAX_BATCH_SIZE limit
///
/// Verifies that batches exceeding MAX_BATCH_SIZE (50) are rejected.
#[tokio::test]
async fn test_batch_check_rpc_rejects_oversized_batch() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    let service = test_service_with_storage(storage);

    // Create 51 checks (one over the limit of 50)
    let checks: Vec<BatchCheckItem> = (0..51)
        .map(|i| BatchCheckItem {
            tuple_key: Some(TupleKey {
                user: format!("user:user{i}"),
                relation: "viewer".to_string(),
                object: format!("document:doc{i}"),
                condition: None,
            }),
            contextual_tuples: None,
            context: None,
            correlation_id: format!("check-{i}"),
        })
        .collect();

    let request = Request::new(BatchCheckRequest {
        store_id: "test-store".to_string(),
        checks,
        authorization_model_id: String::new(),
        consistency: 0,
    });

    let response = service.batch_check(request).await;
    assert!(response.is_err());

    let status = response.unwrap_err();
    assert_eq!(status.code(), tonic::Code::InvalidArgument);
    assert!(status.message().contains("batch size"));
    assert!(status.message().contains("exceeds maximum"));
}

/// Test: BatchCheck RPC accepts exactly MAX_BATCH_SIZE items
///
/// Edge case: exactly at the limit should be accepted.
#[tokio::test]
async fn test_batch_check_rpc_accepts_max_batch_size() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();
    setup_simple_model(&storage, "test-store").await;

    let service = test_service_with_storage(storage);

    // Create exactly 50 checks (at the limit)
    let checks: Vec<BatchCheckItem> = (0..50)
        .map(|i| BatchCheckItem {
            tuple_key: Some(TupleKey {
                user: format!("user:user{i}"),
                relation: "viewer".to_string(),
                object: format!("document:doc{i}"),
                condition: None,
            }),
            contextual_tuples: None,
            context: None,
            correlation_id: format!("check-{i}"),
        })
        .collect();

    let request = Request::new(BatchCheckRequest {
        store_id: "test-store".to_string(),
        checks,
        authorization_model_id: String::new(),
        consistency: 0,
    });

    let response = service.batch_check(request).await;
    // Should succeed at exactly the limit
    assert!(response.is_ok());

    // Verify we got 50 results
    let result = response.unwrap().into_inner();
    assert_eq!(result.result.len(), 50);
}

/// Test: Write RPC works correctly
///
/// Verifies the Write RPC can create and delete tuples.
#[tokio::test]
async fn test_write_rpc_works_correctly() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();
    setup_simple_model(&storage, "test-store").await;

    let service = test_service_with_storage(Arc::clone(&storage));

    // Write a tuple
    let request = Request::new(WriteRequest {
        store_id: "test-store".to_string(),
        writes: Some(WriteRequestWrites {
            tuple_keys: vec![TupleKey {
                user: "user:alice".to_string(),
                relation: "viewer".to_string(),
                object: "document:readme".to_string(),
                condition: None,
            }],
        }),
        deletes: None,
        authorization_model_id: String::new(),
    });

    let response = service.write(request).await;
    assert!(response.is_ok());

    // Verify tuple was written by reading it
    let tuples = storage
        .read_tuples("test-store", &Default::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1);
    assert_eq!(tuples[0].user_id, "alice");

    // Delete the tuple
    let request = Request::new(WriteRequest {
        store_id: "test-store".to_string(),
        writes: None,
        deletes: Some(WriteRequestDeletes {
            tuple_keys: vec![TupleKeyWithoutCondition {
                user: "user:alice".to_string(),
                relation: "viewer".to_string(),
                object: "document:readme".to_string(),
            }],
        }),
        authorization_model_id: String::new(),
    });

    let response = service.write(request).await;
    assert!(response.is_ok());

    // Verify tuple was deleted
    let tuples = storage
        .read_tuples("test-store", &Default::default())
        .await
        .unwrap();
    assert!(tuples.is_empty());
}

/// Test: Read RPC works correctly
///
/// Verifies the Read RPC returns tuples from the store.
#[tokio::test]
async fn test_read_rpc_works_correctly() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    // Write some tuples
    storage
        .write_tuple(
            "test-store",
            StoredTuple::new("document", "doc1", "viewer", "user", "alice", None),
        )
        .await
        .unwrap();
    storage
        .write_tuple(
            "test-store",
            StoredTuple::new("document", "doc2", "editor", "user", "bob", None),
        )
        .await
        .unwrap();

    let service = test_service_with_storage(storage);

    // Read all tuples
    let request = Request::new(ReadRequest {
        store_id: "test-store".to_string(),
        tuple_key: None,
        page_size: None,
        continuation_token: String::new(),
        consistency: 0,
    });

    let response = service.read(request).await;
    assert!(response.is_ok());

    let result = response.unwrap().into_inner();
    assert_eq!(result.tuples.len(), 2);

    // Read filtered by user
    let request = Request::new(ReadRequest {
        store_id: "test-store".to_string(),
        tuple_key: Some(TupleKeyWithoutCondition {
            user: "user:alice".to_string(),
            relation: String::new(),
            object: String::new(),
        }),
        page_size: None,
        continuation_token: String::new(),
        consistency: 0,
    });

    let response = service.read(request).await;
    assert!(response.is_ok());

    let result = response.unwrap().into_inner();
    assert_eq!(result.tuples.len(), 1);
    assert_eq!(result.tuples[0].key.as_ref().unwrap().user, "user:alice");
}

/// Test: gRPC errors map correctly to status codes
///
/// Verifies that storage errors are correctly mapped to gRPC status codes.
#[tokio::test]
async fn test_grpc_errors_map_correctly_to_status_codes() {
    let service = test_service();

    // Non-existent store should return NOT_FOUND
    let request = Request::new(CheckRequest {
        store_id: "nonexistent-store".to_string(),
        tuple_key: Some(TupleKey {
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:readme".to_string(),
            condition: None,
        }),
        contextual_tuples: None,
        authorization_model_id: String::new(),
        trace: false,
        context: None,
        consistency: 0,
    });

    let response = service.check(request).await;
    assert!(response.is_err());
    assert_eq!(response.unwrap_err().code(), tonic::Code::NotFound);

    // Missing tuple_key should return INVALID_ARGUMENT
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();
    let service = test_service_with_storage(storage);

    let request = Request::new(CheckRequest {
        store_id: "test-store".to_string(),
        tuple_key: None, // Missing required field
        contextual_tuples: None,
        authorization_model_id: String::new(),
        trace: false,
        context: None,
        consistency: 0,
    });

    let response = service.check(request).await;
    assert!(response.is_err());
    assert_eq!(response.unwrap_err().code(), tonic::Code::InvalidArgument);

    // Empty batch should return INVALID_ARGUMENT
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();
    let service = test_service_with_storage(storage);

    let request = Request::new(BatchCheckRequest {
        store_id: "test-store".to_string(),
        checks: vec![], // Empty batch
        authorization_model_id: String::new(),
        consistency: 0,
    });

    let response = service.batch_check(request).await;
    assert!(response.is_err());
    assert_eq!(response.unwrap_err().code(), tonic::Code::InvalidArgument);

    // Batch size exceeded should return INVALID_ARGUMENT
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();
    let service = test_service_with_storage(storage);

    let mut checks = Vec::new();
    for i in 0..100 {
        checks.push(BatchCheckItem {
            tuple_key: Some(TupleKey {
                user: format!("user:{i}"),
                relation: "viewer".to_string(),
                object: format!("doc:{i}"),
                condition: None,
            }),
            contextual_tuples: None,
            context: None,
            correlation_id: format!("check-{i}"),
        });
    }

    let request = Request::new(BatchCheckRequest {
        store_id: "test-store".to_string(),
        checks,
        authorization_model_id: String::new(),
        consistency: 0,
    });

    let response = service.batch_check(request).await;
    assert!(response.is_err());
    assert_eq!(response.unwrap_err().code(), tonic::Code::InvalidArgument);
}

/// Test: WriteAuthorizationModel invalidates CEL cache
///
/// Verifies that writing a new authorization model clears the CEL expression
/// cache to prevent stale expressions from being used with new models.
/// This is critical for security: old condition expressions must not be
/// evaluated against updated authorization models.
#[tokio::test]
async fn test_write_authorization_model_invalidates_cel_cache() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    let service = test_service_with_storage(storage);

    // Populate the CEL cache with some expressions
    let cache = global_cache();
    cache.get_or_parse("request.ip == '192.168.1.1'").unwrap();
    cache
        .get_or_parse("request.time > timestamp('2024-01-01T00:00:00Z')")
        .unwrap();
    cache.run_pending_tasks(); // Ensure entries are visible
    assert!(
        cache.entry_count() >= 2,
        "Cache should have at least 2 entries, got {}",
        cache.entry_count()
    );

    // Call write_authorization_model with a minimal valid model
    let request = Request::new(WriteAuthorizationModelRequest {
        store_id: "test-store".to_string(),
        type_definitions: vec![TypeDefinition {
            r#type: "user".to_string(),
            relations: Default::default(),
            metadata: None,
        }],
        schema_version: "1.1".to_string(),
        conditions: Default::default(),
    });

    let response = service.write_authorization_model(request).await;
    assert!(
        response.is_ok(),
        "write_authorization_model should succeed: {:?}",
        response.err()
    );

    // Verify the CEL cache was invalidated
    cache.run_pending_tasks(); // Ensure invalidation is visible
    assert_eq!(
        cache.entry_count(),
        0,
        "CEL cache should be empty after write_authorization_model"
    );
}

/// Test: WriteAuthorizationModel returns error for non-existent store
///
/// Verifies that write_authorization_model validates the store exists
/// before attempting to update the model.
#[tokio::test]
async fn test_write_authorization_model_validates_store_exists() {
    let service = test_service();

    let request = Request::new(WriteAuthorizationModelRequest {
        store_id: "nonexistent-store".to_string(),
        type_definitions: vec![],
        schema_version: String::new(),
        conditions: Default::default(),
    });

    let response = service.write_authorization_model(request).await;
    assert!(response.is_err());
    assert_eq!(response.unwrap_err().code(), tonic::Code::NotFound);
}

/// Test: Write RPC correctly parses and stores conditions
///
/// Verifies that when a TupleKey includes a condition, the condition name
/// and context are correctly extracted and stored in the StoredTuple.
#[tokio::test]
async fn test_write_rpc_parses_conditions() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();
    setup_model_with_conditions(&storage, "test-store").await;

    let service = test_service_with_storage(Arc::clone(&storage));

    // Build a condition with context
    let mut context_fields = std::collections::BTreeMap::new();
    context_fields.insert(
        "allowed_ip".to_string(),
        prost_types::Value {
            kind: Some(prost_types::value::Kind::StringValue(
                "192.168.1.100".to_string(),
            )),
        },
    );
    context_fields.insert(
        "max_age".to_string(),
        prost_types::Value {
            kind: Some(prost_types::value::Kind::NumberValue(30.0)),
        },
    );

    let condition = RelationshipCondition {
        name: "ip_restriction".to_string(),
        context: Some(prost_types::Struct {
            fields: context_fields,
        }),
    };

    // Write a tuple with a condition
    let request = Request::new(WriteRequest {
        store_id: "test-store".to_string(),
        writes: Some(WriteRequestWrites {
            tuple_keys: vec![TupleKey {
                user: "user:alice".to_string(),
                relation: "viewer".to_string(),
                object: "document:secret".to_string(),
                condition: Some(condition),
            }],
        }),
        deletes: None,
        authorization_model_id: String::new(),
    });

    let response = service.write(request).await;
    assert!(response.is_ok());

    // Verify the tuple was written with the condition
    let tuples = storage
        .read_tuples("test-store", &Default::default())
        .await
        .unwrap();

    assert_eq!(tuples.len(), 1);
    let tuple = &tuples[0];

    // Verify condition name
    assert_eq!(tuple.condition_name, Some("ip_restriction".to_string()));

    // Verify condition context
    let context = tuple
        .condition_context
        .as_ref()
        .expect("condition context should be present");
    assert_eq!(
        context.get("allowed_ip"),
        Some(&serde_json::json!("192.168.1.100"))
    );
    assert_eq!(context.get("max_age"), Some(&serde_json::json!(30.0)));
}

/// Test: Write RPC correctly parses condition without context
///
/// Verifies that a condition with only a name (no context) is correctly parsed.
#[tokio::test]
async fn test_write_rpc_parses_condition_without_context() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();
    setup_model_with_conditions(&storage, "test-store").await;

    let service = test_service_with_storage(Arc::clone(&storage));

    // Condition with name but no context
    let condition = RelationshipCondition {
        name: "time_based_access".to_string(),
        context: None,
    };

    let request = Request::new(WriteRequest {
        store_id: "test-store".to_string(),
        writes: Some(WriteRequestWrites {
            tuple_keys: vec![TupleKey {
                user: "user:bob".to_string(),
                relation: "editor".to_string(),
                object: "document:report".to_string(),
                condition: Some(condition),
            }],
        }),
        deletes: None,
        authorization_model_id: String::new(),
    });

    let response = service.write(request).await;
    assert!(response.is_ok());

    // Verify the tuple was written
    let tuples = storage
        .read_tuples("test-store", &Default::default())
        .await
        .unwrap();

    assert_eq!(tuples.len(), 1);
    let tuple = &tuples[0];

    // Verify condition name is present but context is None
    assert_eq!(tuple.condition_name, Some("time_based_access".to_string()));
    assert!(tuple.condition_context.is_none());
}

/// Test: Write RPC ignores empty condition name
///
/// Verifies that a condition with an empty name is treated as no condition.
#[tokio::test]
async fn test_write_rpc_ignores_empty_condition_name() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();
    setup_simple_model(&storage, "test-store").await;

    let service = test_service_with_storage(Arc::clone(&storage));

    // Condition with empty name
    let condition = RelationshipCondition {
        name: String::new(),
        context: Some(prost_types::Struct {
            fields: std::collections::BTreeMap::new(),
        }),
    };

    let request = Request::new(WriteRequest {
        store_id: "test-store".to_string(),
        writes: Some(WriteRequestWrites {
            tuple_keys: vec![TupleKey {
                user: "user:carol".to_string(),
                relation: "owner".to_string(),
                object: "document:public".to_string(),
                condition: Some(condition),
            }],
        }),
        deletes: None,
        authorization_model_id: String::new(),
    });

    let response = service.write(request).await;
    assert!(response.is_ok());

    // Verify the tuple was written without condition
    let tuples = storage
        .read_tuples("test-store", &Default::default())
        .await
        .unwrap();

    assert_eq!(tuples.len(), 1);
    let tuple = &tuples[0];

    // Empty condition name should be treated as no condition
    assert!(tuple.condition_name.is_none());
    assert!(tuple.condition_context.is_none());
}

/// Test: Write RPC rejects invalid condition name with special characters
///
/// Verifies that condition names with special characters are rejected (security I4).
#[tokio::test]
async fn test_write_rpc_rejects_invalid_condition_name() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();
    setup_simple_model(&storage, "test-store").await;

    let service = test_service_with_storage(Arc::clone(&storage));

    // Condition with invalid name (contains special characters)
    let condition = RelationshipCondition {
        name: "invalid;DROP TABLE--".to_string(),
        context: None,
    };

    let request = Request::new(WriteRequest {
        store_id: "test-store".to_string(),
        writes: Some(WriteRequestWrites {
            tuple_keys: vec![TupleKey {
                user: "user:alice".to_string(),
                relation: "viewer".to_string(),
                object: "document:secret".to_string(),
                condition: Some(condition),
            }],
        }),
        deletes: None,
        authorization_model_id: String::new(),
    });

    let response = service.write(request).await;
    assert!(response.is_err());
    let status = response.unwrap_err();
    assert_eq!(status.code(), tonic::Code::InvalidArgument);
    assert!(status.message().contains("invalid condition name"));
}

/// Test: Write RPC accepts valid condition names with underscore and hyphen
///
/// Verifies that valid condition names are accepted.
#[tokio::test]
async fn test_write_rpc_accepts_valid_condition_name_formats() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();
    setup_model_with_conditions(&storage, "test-store").await;

    let service = test_service_with_storage(Arc::clone(&storage));

    // Valid condition names with underscore and hyphen
    let condition = RelationshipCondition {
        name: "ip_restriction-v2".to_string(),
        context: None,
    };

    let request = Request::new(WriteRequest {
        store_id: "test-store".to_string(),
        writes: Some(WriteRequestWrites {
            tuple_keys: vec![TupleKey {
                user: "user:alice".to_string(),
                relation: "viewer".to_string(),
                object: "document:report".to_string(),
                condition: Some(condition),
            }],
        }),
        deletes: None,
        authorization_model_id: String::new(),
    });

    let response = service.write(request).await;
    assert!(response.is_ok());

    // Verify the tuple was written with the condition
    let tuples = storage
        .read_tuples("test-store", &Default::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1);
    assert_eq!(
        tuples[0].condition_name,
        Some("ip_restriction-v2".to_string())
    );
}

/// Test: Write RPC rejects NaN/Infinity in condition context
///
/// Verifies that invalid numeric values (NaN, Infinity) are rejected
/// since they cannot be represented in JSON (constraint C11).
#[tokio::test]
async fn test_write_rpc_rejects_nan_infinity_in_condition_context() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();
    setup_simple_model(&storage, "test-store").await;

    let service = test_service_with_storage(Arc::clone(&storage));

    // Test NaN value in context
    let mut context_fields = std::collections::BTreeMap::new();
    context_fields.insert(
        "invalid_value".to_string(),
        prost_types::Value {
            kind: Some(prost_types::value::Kind::NumberValue(f64::NAN)),
        },
    );

    let condition = RelationshipCondition {
        name: "test_condition".to_string(),
        context: Some(prost_types::Struct {
            fields: context_fields,
        }),
    };

    let request = Request::new(WriteRequest {
        store_id: "test-store".to_string(),
        writes: Some(WriteRequestWrites {
            tuple_keys: vec![TupleKey {
                user: "user:alice".to_string(),
                relation: "viewer".to_string(),
                object: "document:test".to_string(),
                condition: Some(condition),
            }],
        }),
        deletes: None,
        authorization_model_id: String::new(),
    });

    let response = service.write(request).await;
    assert!(response.is_err());
    let status = response.unwrap_err();
    assert_eq!(status.code(), tonic::Code::InvalidArgument);
    assert!(status.message().contains("NaN or Infinity"));

    // Test Infinity value in context
    let mut context_fields = std::collections::BTreeMap::new();
    context_fields.insert(
        "invalid_value".to_string(),
        prost_types::Value {
            kind: Some(prost_types::value::Kind::NumberValue(f64::INFINITY)),
        },
    );

    let condition = RelationshipCondition {
        name: "test_condition".to_string(),
        context: Some(prost_types::Struct {
            fields: context_fields,
        }),
    };

    let request = Request::new(WriteRequest {
        store_id: "test-store".to_string(),
        writes: Some(WriteRequestWrites {
            tuple_keys: vec![TupleKey {
                user: "user:alice".to_string(),
                relation: "viewer".to_string(),
                object: "document:test".to_string(),
                condition: Some(condition),
            }],
        }),
        deletes: None,
        authorization_model_id: String::new(),
    });

    let response = service.write(request).await;
    assert!(response.is_err());
    let status = response.unwrap_err();
    assert_eq!(status.code(), tonic::Code::InvalidArgument);
    assert!(status.message().contains("NaN or Infinity"));
}

/// Test: Read RPC returns conditions stored with tuples
///
/// Verifies read-after-write contract: conditions written are returned by Read (I2).
#[tokio::test]
async fn test_read_rpc_returns_conditions() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();
    setup_model_with_conditions(&storage, "test-store").await;

    let service = test_service_with_storage(Arc::clone(&storage));

    // Write a tuple with a condition
    let mut context_fields = std::collections::BTreeMap::new();
    context_fields.insert(
        "max_age".to_string(),
        prost_types::Value {
            kind: Some(prost_types::value::Kind::NumberValue(25.0)),
        },
    );

    let condition = RelationshipCondition {
        name: "age_restriction".to_string(),
        context: Some(prost_types::Struct {
            fields: context_fields,
        }),
    };

    let write_request = Request::new(WriteRequest {
        store_id: "test-store".to_string(),
        writes: Some(WriteRequestWrites {
            tuple_keys: vec![TupleKey {
                user: "user:bob".to_string(),
                relation: "viewer".to_string(),
                object: "document:restricted".to_string(),
                condition: Some(condition),
            }],
        }),
        deletes: None,
        authorization_model_id: String::new(),
    });

    service.write(write_request).await.unwrap();

    // Read the tuple back
    let read_request = Request::new(ReadRequest {
        store_id: "test-store".to_string(),
        tuple_key: None,
        page_size: None,
        continuation_token: String::new(),
        consistency: 0,
    });

    let response = service.read(read_request).await.unwrap();
    let tuples = response.into_inner().tuples;

    assert_eq!(tuples.len(), 1);
    let tuple = &tuples[0];
    let key = tuple.key.as_ref().expect("tuple should have key");

    // Verify condition is returned
    let condition = key.condition.as_ref().expect("condition should be present");
    assert_eq!(condition.name, "age_restriction");

    let context = condition
        .context
        .as_ref()
        .expect("context should be present");
    let max_age = context
        .fields
        .get("max_age")
        .expect("max_age should be present");
    assert!(matches!(
        max_age.kind,
        Some(prost_types::value::Kind::NumberValue(25.0))
    ));
}

// ============================================================================
// Section: Consistency and Model Versioning Tests (gRPC)
// ============================================================================
//
// These tests verify consistency preference handling and model version selection
// via gRPC endpoints, complementing the HTTP integration tests.

/// Test: gRPC Check accepts MINIMIZE_LATENCY consistency preference.
#[tokio::test]
async fn test_grpc_check_accepts_minimize_latency_consistency() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    setup_simple_model(&storage, "test-store").await;

    storage
        .write_tuple(
            "test-store",
            StoredTuple::new("document", "doc1", "viewer", "user", "alice", None),
        )
        .await
        .unwrap();

    let service = test_service_with_storage(storage);

    // Check with MINIMIZE_LATENCY consistency (value = 1)
    let request = Request::new(CheckRequest {
        store_id: "test-store".to_string(),
        tuple_key: Some(TupleKey {
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:doc1".to_string(),
            condition: None,
        }),
        contextual_tuples: None,
        authorization_model_id: String::new(),
        trace: false,
        context: None,
        consistency: 1, // MINIMIZE_LATENCY
    });

    let response = service.check(request).await;
    assert!(
        response.is_ok(),
        "Check with MINIMIZE_LATENCY should succeed"
    );
    assert!(response.unwrap().into_inner().allowed);
}

/// Test: gRPC Check accepts HIGHER_CONSISTENCY preference.
#[tokio::test]
async fn test_grpc_check_accepts_higher_consistency() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    setup_simple_model(&storage, "test-store").await;

    storage
        .write_tuple(
            "test-store",
            StoredTuple::new("document", "doc1", "viewer", "user", "alice", None),
        )
        .await
        .unwrap();

    let service = test_service_with_storage(storage);

    // Check with HIGHER_CONSISTENCY (value = 2)
    let request = Request::new(CheckRequest {
        store_id: "test-store".to_string(),
        tuple_key: Some(TupleKey {
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:doc1".to_string(),
            condition: None,
        }),
        contextual_tuples: None,
        authorization_model_id: String::new(),
        trace: false,
        context: None,
        consistency: 2, // HIGHER_CONSISTENCY
    });

    let response = service.check(request).await;
    assert!(
        response.is_ok(),
        "Check with HIGHER_CONSISTENCY should succeed"
    );
    assert!(response.unwrap().into_inner().allowed);
}

/// Test: gRPC Check with specific authorization_model_id.
#[tokio::test]
async fn test_grpc_check_with_specific_model_id() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    let model_id = setup_simple_model(&storage, "test-store").await;

    storage
        .write_tuple(
            "test-store",
            StoredTuple::new("document", "doc1", "viewer", "user", "alice", None),
        )
        .await
        .unwrap();

    let service = test_service_with_storage(storage);

    // Check with specific model ID
    let request = Request::new(CheckRequest {
        store_id: "test-store".to_string(),
        tuple_key: Some(TupleKey {
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:doc1".to_string(),
            condition: None,
        }),
        contextual_tuples: None,
        authorization_model_id: model_id,
        trace: false,
        context: None,
        consistency: 0,
    });

    let response = service.check(request).await;
    assert!(
        response.is_ok(),
        "Check with specific model_id should succeed"
    );
    assert!(response.unwrap().into_inner().allowed);
}

/// Test: gRPC Check without model_id uses latest model.
#[tokio::test]
async fn test_grpc_check_uses_latest_model_by_default() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    // Create first model (viewer only)
    let _model_id_1 = setup_simple_model(&storage, "test-store").await;

    // Create second model with additional relations
    let model2 = StoredAuthorizationModel::new(
        ulid::Ulid::new().to_string(),
        "test-store",
        "1.1",
        r#"{
            "type_definitions": [
                {"type": "user"},
                {"type": "document", "relations": {
                    "viewer": {},
                    "editor": {},
                    "admin": {}
                }}
            ]
        }"#,
    );
    storage.write_authorization_model(model2).await.unwrap();

    // Write tuple for admin relation (only in latest model)
    storage
        .write_tuple(
            "test-store",
            StoredTuple::new("document", "doc1", "admin", "user", "alice", None),
        )
        .await
        .unwrap();

    let service = test_service_with_storage(storage);

    // Check without model_id - should use latest model
    let request = Request::new(CheckRequest {
        store_id: "test-store".to_string(),
        tuple_key: Some(TupleKey {
            user: "user:alice".to_string(),
            relation: "admin".to_string(),
            object: "document:doc1".to_string(),
            condition: None,
        }),
        contextual_tuples: None,
        authorization_model_id: String::new(), // Empty = use latest
        trace: false,
        context: None,
        consistency: 0,
    });

    let response = service.check(request).await;
    assert!(
        response.is_ok(),
        "Check without model_id should succeed using latest model"
    );
    assert!(response.unwrap().into_inner().allowed);
}

/// Test: gRPC BatchCheck with consistency and model_id.
#[tokio::test]
async fn test_grpc_batch_check_with_consistency_and_model_id() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    let model_id = setup_simple_model(&storage, "test-store").await;

    storage
        .write_tuple(
            "test-store",
            StoredTuple::new("document", "doc1", "viewer", "user", "alice", None),
        )
        .await
        .unwrap();

    let service = test_service_with_storage(storage);

    let request = Request::new(BatchCheckRequest {
        store_id: "test-store".to_string(),
        checks: vec![BatchCheckItem {
            tuple_key: Some(TupleKey {
                user: "user:alice".to_string(),
                relation: "viewer".to_string(),
                object: "document:doc1".to_string(),
                condition: None,
            }),
            contextual_tuples: None,
            context: None,
            correlation_id: "check-1".to_string(),
        }],
        authorization_model_id: model_id,
        consistency: 2, // HIGHER_CONSISTENCY
    });

    let response = service.batch_check(request).await;
    assert!(
        response.is_ok(),
        "BatchCheck with consistency and model_id should succeed"
    );

    let result = response.unwrap().into_inner();
    assert!(result.result.get("check-1").unwrap().allowed);
}

/// Test: gRPC Read with consistency parameter.
#[tokio::test]
async fn test_grpc_read_with_consistency() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    storage
        .write_tuple(
            "test-store",
            StoredTuple::new("document", "doc1", "viewer", "user", "alice", None),
        )
        .await
        .unwrap();

    let service = test_service_with_storage(storage);

    // Read with HIGHER_CONSISTENCY
    let request = Request::new(ReadRequest {
        store_id: "test-store".to_string(),
        tuple_key: None,
        page_size: None,
        continuation_token: String::new(),
        consistency: 2, // HIGHER_CONSISTENCY
    });

    let response = service.read(request).await;
    assert!(
        response.is_ok(),
        "Read with HIGHER_CONSISTENCY should succeed"
    );

    let result = response.unwrap().into_inner();
    assert_eq!(result.tuples.len(), 1);
}

/// Test: gRPC Write with authorization_model_id.
#[tokio::test]
async fn test_grpc_write_with_model_id() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    let model_id = setup_simple_model(&storage, "test-store").await;

    let service = test_service_with_storage(Arc::clone(&storage));

    // Write with specific model_id
    let request = Request::new(WriteRequest {
        store_id: "test-store".to_string(),
        writes: Some(WriteRequestWrites {
            tuple_keys: vec![TupleKey {
                user: "user:alice".to_string(),
                relation: "viewer".to_string(),
                object: "document:doc1".to_string(),
                condition: None,
            }],
        }),
        deletes: None,
        authorization_model_id: model_id,
    });

    let response = service.write(request).await;
    assert!(response.is_ok(), "Write with model_id should succeed");

    // Verify tuple was written
    let tuples = storage
        .read_tuples("test-store", &Default::default())
        .await
        .unwrap();
    assert_eq!(tuples.len(), 1);
}

/// Test: gRPC GetAuthorizationModel returns correct model.
///
/// NOTE: read_authorization_model is currently a stub that returns NOT_FOUND.
/// This test documents expected behavior when implemented.
#[tokio::test]
#[ignore = "read_authorization_model is a stub - returns NOT_FOUND for all requests"]
async fn test_grpc_get_authorization_model() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    let model_id = setup_simple_model(&storage, "test-store").await;

    let service = test_service_with_storage(storage);

    let request = Request::new(ReadAuthorizationModelRequest {
        store_id: "test-store".to_string(),
        id: model_id.clone(),
    });

    let response = service.read_authorization_model(request).await;
    assert!(response.is_ok(), "GetAuthorizationModel should succeed");

    let result = response.unwrap().into_inner();
    let model = result.authorization_model.expect("Should have model");
    assert_eq!(model.id, model_id);
}

/// Test: gRPC GetAuthorizationModel with non-existent ID returns NOT_FOUND.
#[tokio::test]
async fn test_grpc_get_nonexistent_model_returns_not_found() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    let service = test_service_with_storage(storage);

    let request = Request::new(ReadAuthorizationModelRequest {
        store_id: "test-store".to_string(),
        id: "01H000000000000000000FAKE".to_string(), // Non-existent ID
    });

    let response = service.read_authorization_model(request).await;
    assert!(response.is_err());
    assert_eq!(response.unwrap_err().code(), tonic::Code::NotFound);
}

/// Test: gRPC ListAuthorizationModels returns all models.
///
/// NOTE: read_authorization_models is currently a stub that returns empty list.
/// This test documents expected behavior when implemented.
#[tokio::test]
#[ignore = "read_authorization_models is a stub - returns empty list"]
async fn test_grpc_list_authorization_models() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    // Create multiple models
    let model_id_1 = setup_simple_model(&storage, "test-store").await;

    let model2 = StoredAuthorizationModel::new(
        ulid::Ulid::new().to_string(),
        "test-store",
        "1.1",
        r#"{"type_definitions": [{"type": "user"}, {"type": "folder", "relations": {"viewer": {}}}]}"#,
    );
    let model_id_2 = model2.id.clone();
    storage.write_authorization_model(model2).await.unwrap();

    let service = test_service_with_storage(storage);

    let request = Request::new(ReadAuthorizationModelsRequest {
        store_id: "test-store".to_string(),
        page_size: None,
        continuation_token: String::new(),
    });

    let response = service.read_authorization_models(request).await;
    assert!(response.is_ok(), "ListAuthorizationModels should succeed");

    let result = response.unwrap().into_inner();
    assert_eq!(result.authorization_models.len(), 2);

    let model_ids: std::collections::HashSet<String> = result
        .authorization_models
        .iter()
        .map(|m| m.id.clone())
        .collect();
    assert!(model_ids.contains(&model_id_1));
    assert!(model_ids.contains(&model_id_2));
}

// ============================================================================
// ReadChanges API Tests
// ============================================================================

/// Test: ReadChanges returns tuple writes for a store.
#[tokio::test]
async fn test_grpc_read_changes_returns_tuples() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    // Write some tuples
    storage
        .write_tuple(
            "test-store",
            StoredTuple::new("document", "doc1", "viewer", "user", "alice", None),
        )
        .await
        .unwrap();
    storage
        .write_tuple(
            "test-store",
            StoredTuple::new("document", "doc2", "editor", "user", "bob", None),
        )
        .await
        .unwrap();

    let service = test_service_with_storage(storage);

    let request = Request::new(ReadChangesRequest {
        store_id: "test-store".to_string(),
        r#type: String::new(),
        page_size: None,
        continuation_token: String::new(),
    });

    let response = service.read_changes(request).await;
    assert!(response.is_ok(), "ReadChanges should succeed");

    let result = response.unwrap().into_inner();
    assert_eq!(result.changes.len(), 2, "Should return 2 changes");

    // Verify change format
    for change in &result.changes {
        assert!(
            change.tuple_key.is_some(),
            "Each change should have tuple_key"
        );
        assert!(
            change.timestamp.is_some(),
            "Each change should have timestamp"
        );
        // All changes are WRITE operations (0)
        assert_eq!(change.operation, 0, "Operation should be WRITE (0)");
    }
}

/// Test: ReadChanges with type filter only returns matching types.
#[tokio::test]
async fn test_grpc_read_changes_with_type_filter() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    // Write tuples of different types
    storage
        .write_tuple(
            "test-store",
            StoredTuple::new("document", "doc1", "viewer", "user", "alice", None),
        )
        .await
        .unwrap();
    storage
        .write_tuple(
            "test-store",
            StoredTuple::new("folder", "folder1", "viewer", "user", "bob", None),
        )
        .await
        .unwrap();

    let service = test_service_with_storage(storage);

    // Request only document type changes
    let request = Request::new(ReadChangesRequest {
        store_id: "test-store".to_string(),
        r#type: "document".to_string(),
        page_size: None,
        continuation_token: String::new(),
    });

    let response = service.read_changes(request).await;
    assert!(response.is_ok());

    let result = response.unwrap().into_inner();
    assert_eq!(
        result.changes.len(),
        1,
        "Should return only document changes"
    );

    let change = &result.changes[0];
    let tuple_key = change.tuple_key.as_ref().unwrap();
    assert!(
        tuple_key.object.starts_with("document:"),
        "Filtered change should be document type"
    );
}

/// Test: ReadChanges rejects negative page_size.
#[tokio::test]
async fn test_grpc_read_changes_rejects_negative_page_size() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    let service = test_service_with_storage(storage);

    let request = Request::new(ReadChangesRequest {
        store_id: "test-store".to_string(),
        r#type: String::new(),
        page_size: Some(-1),
        continuation_token: String::new(),
    });

    let response = service.read_changes(request).await;
    assert!(response.is_err(), "Should reject negative page_size");
    assert_eq!(
        response.unwrap_err().code(),
        tonic::Code::InvalidArgument,
        "Should return InvalidArgument for negative"
    );
}

/// Test: ReadChanges treats zero page_size as server default (OpenFGA compatibility).
#[tokio::test]
async fn test_grpc_read_changes_zero_page_size_uses_default() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    // Write some tuples
    for i in 0..5 {
        storage
            .write_tuple(
                "test-store",
                StoredTuple::new(
                    "document",
                    format!("doc{i}"),
                    "viewer",
                    "user",
                    "alice",
                    None,
                ),
            )
            .await
            .unwrap();
    }

    let service = test_service_with_storage(storage);

    // Zero page_size should be treated as "use server default", not rejected
    let request = Request::new(ReadChangesRequest {
        store_id: "test-store".to_string(),
        r#type: String::new(),
        page_size: Some(0),
        continuation_token: String::new(),
    });

    let response = service.read_changes(request).await;
    assert!(
        response.is_ok(),
        "Zero page_size should succeed (use default)"
    );

    let result = response.unwrap().into_inner();
    assert_eq!(
        result.changes.len(),
        5,
        "Should return all tuples with default page size"
    );
}

/// Test: ReadChanges respects page_size limit.
#[tokio::test]
async fn test_grpc_read_changes_pagination() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    // Write multiple tuples
    for i in 0..10 {
        storage
            .write_tuple(
                "test-store",
                StoredTuple::new(
                    "document",
                    format!("doc{i}"),
                    "viewer",
                    "user",
                    "alice",
                    None,
                ),
            )
            .await
            .unwrap();
    }

    let service = test_service_with_storage(storage);

    // Request with small page size
    let request = Request::new(ReadChangesRequest {
        store_id: "test-store".to_string(),
        r#type: String::new(),
        page_size: Some(3),
        continuation_token: String::new(),
    });

    let response = service.read_changes(request).await;
    assert!(response.is_ok());

    let result = response.unwrap().into_inner();
    assert!(
        result.changes.len() <= 3,
        "Should return at most page_size changes"
    );
}

/// Test: ReadChanges caps page_size at DEFAULT_PAGE_SIZE (50) for DoS protection.
#[tokio::test]
async fn test_grpc_read_changes_caps_page_size_at_default() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    // Write more tuples than DEFAULT_PAGE_SIZE (50)
    for i in 0..60 {
        storage
            .write_tuple(
                "test-store",
                StoredTuple::new(
                    "document",
                    format!("doc{i}"),
                    "viewer",
                    "user",
                    "alice",
                    None,
                ),
            )
            .await
            .unwrap();
    }

    let service = test_service_with_storage(storage);

    // Request with page_size > DEFAULT_PAGE_SIZE (50)
    let request = Request::new(ReadChangesRequest {
        store_id: "test-store".to_string(),
        r#type: String::new(),
        page_size: Some(1000), // Much larger than DEFAULT_PAGE_SIZE
        continuation_token: String::new(),
    });

    let response = service.read_changes(request).await;
    assert!(response.is_ok());

    let result = response.unwrap().into_inner();
    // Should be capped at DEFAULT_PAGE_SIZE (50)
    assert!(
        result.changes.len() <= 50,
        "Should cap page_size at DEFAULT_PAGE_SIZE (50), got {}",
        result.changes.len()
    );
}

/// Test: ReadChanges returns empty for store with no tuples.
#[tokio::test]
async fn test_grpc_read_changes_empty_store() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    let service = test_service_with_storage(storage);

    let request = Request::new(ReadChangesRequest {
        store_id: "test-store".to_string(),
        r#type: String::new(),
        page_size: None,
        continuation_token: String::new(),
    });

    let response = service.read_changes(request).await;
    assert!(response.is_ok());

    let result = response.unwrap().into_inner();
    assert_eq!(
        result.changes.len(),
        0,
        "Empty store should have no changes"
    );
}

/// Test: ReadChanges returns error for non-existent store.
#[tokio::test]
async fn test_grpc_read_changes_nonexistent_store() {
    let service = test_service();

    let request = Request::new(ReadChangesRequest {
        store_id: "nonexistent-store".to_string(),
        r#type: String::new(),
        page_size: None,
        continuation_token: String::new(),
    });

    let response = service.read_changes(request).await;
    assert!(response.is_err(), "Should fail for non-existent store");
    assert_eq!(
        response.unwrap_err().code(),
        tonic::Code::NotFound,
        "Should return NotFound"
    );
}

/// Test: ReadChanges includes condition information.
#[tokio::test]
async fn test_grpc_read_changes_includes_conditions() {
    let storage = Arc::new(MemoryDataStore::new());
    storage
        .create_store("test-store", "Test Store")
        .await
        .unwrap();

    // Write a tuple with condition
    let mut context = std::collections::HashMap::new();
    context.insert("ip_address".to_string(), serde_json::json!("192.168.1.1"));

    storage
        .write_tuple(
            "test-store",
            StoredTuple::with_condition(
                "document",
                "doc1",
                "viewer",
                "user",
                "alice",
                None,
                "ip_allowlist",
                Some(context),
            ),
        )
        .await
        .unwrap();

    let service = test_service_with_storage(storage);

    let request = Request::new(ReadChangesRequest {
        store_id: "test-store".to_string(),
        r#type: String::new(),
        page_size: None,
        continuation_token: String::new(),
    });

    let response = service.read_changes(request).await;
    assert!(response.is_ok());

    let result = response.unwrap().into_inner();
    assert_eq!(result.changes.len(), 1);

    let change = &result.changes[0];
    let tuple_key = change.tuple_key.as_ref().unwrap();
    assert!(
        tuple_key.condition.is_some(),
        "Change should include condition"
    );

    let condition = tuple_key.condition.as_ref().unwrap();
    assert_eq!(condition.name, "ip_allowlist");
    assert!(condition.context.is_some(), "Condition should have context");
}
