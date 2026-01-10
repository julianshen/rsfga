//! gRPC API tests for Section 3.

use std::sync::Arc;

use tonic::Request;

use rsfga_storage::{DataStore, MemoryDataStore, StoredTuple};

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
