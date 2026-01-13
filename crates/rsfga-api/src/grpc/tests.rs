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
    assert!(status.message().contains("tuple_key is required"));
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
                user: format!("user:user{}", i),
                relation: "viewer".to_string(),
                object: format!("document:doc{}", i),
                condition: None,
            }),
            contextual_tuples: None,
            context: None,
            correlation_id: format!("check-{}", i),
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
                user: format!("user:user{}", i),
                relation: "viewer".to_string(),
                object: format!("document:doc{}", i),
                condition: None,
            }),
            contextual_tuples: None,
            context: None,
            correlation_id: format!("check-{}", i),
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
                user: format!("user:{}", i),
                relation: "viewer".to_string(),
                object: format!("doc:{}", i),
                condition: None,
            }),
            contextual_tuples: None,
            context: None,
            correlation_id: format!("check-{}", i),
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

    // Call write_authorization_model
    let request = Request::new(WriteAuthorizationModelRequest {
        store_id: "test-store".to_string(),
        type_definitions: vec![],
        schema_version: String::new(),
        conditions: Default::default(),
    });

    let response = service.write_authorization_model(request).await;
    assert!(response.is_ok(), "write_authorization_model should succeed");

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
