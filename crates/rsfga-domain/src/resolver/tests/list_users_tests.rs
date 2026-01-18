//! Unit tests for the list_users functionality.
//!
//! These tests cover:
//! - Direct relations
//! - Userset filters
//! - Wildcards
//! - Computed relations
//! - Contextual tuples
//! - Error handling

use std::collections::HashMap;
use std::sync::Arc;

use super::mocks::*;
use crate::error::DomainError;
use crate::model::{RelationDefinition, TypeDefinition, Userset};
use crate::resolver::{
    GraphResolver, ListUsersRequest, ListUsersResult, UserFilter, UserResult,
};

// ========== Section 1: Direct Relations ==========

#[tokio::test]
async fn test_list_users_returns_users_with_direct_relation() {
    // Setup: document:readme has viewer relation with user:alice and user:bob
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;
    
    tuple_reader.add_tuple("store-1", "document", "readme", "viewer", "user", "alice", None).await;
    tuple_reader.add_tuple("store-1", "document", "readme", "viewer", "user", "bob", None).await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader.add_type("store-1", TypeDefinition {
        type_name: "document".to_string(),
        relations: vec![RelationDefinition {
            name: "viewer".to_string(),
            type_constraints: vec!["user".into()],
            rewrite: Userset::This,
        }],
    }).await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListUsersRequest::new(
        "store-1",
        "document:readme",
        "viewer",
        vec![UserFilter::new("user")],
    );

    let result = resolver.list_users(&request).await.unwrap();

    assert_eq!(result.users.len(), 2);
    assert!(result.users.contains(&UserResult::object("user", "alice")));
    assert!(result.users.contains(&UserResult::object("user", "bob")));
    assert!(result.excluded_users.is_empty());
}

#[tokio::test]
async fn test_list_users_filters_by_user_type() {
    // Setup: document has viewers of type 'user' and 'service_account'
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;

    tuple_reader.add_tuple("store-1", "document", "readme", "viewer", "user", "alice", None).await;
    tuple_reader.add_tuple("store-1", "document", "readme", "viewer", "service_account", "bot-1", None).await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader.add_type("store-1", TypeDefinition {
        type_name: "document".to_string(),
        relations: vec![RelationDefinition {
            name: "viewer".to_string(),
            type_constraints: vec!["user".into(), "service_account".into()],
            rewrite: Userset::This,
        }],
    }).await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Only request users of type 'user'
    let request = ListUsersRequest::new(
        "store-1",
        "document:readme",
        "viewer",
        vec![UserFilter::new("user")],
    );

    let result = resolver.list_users(&request).await.unwrap();

    assert_eq!(result.users.len(), 1);
    assert!(result.users.contains(&UserResult::object("user", "alice")));
}

#[tokio::test]
async fn test_list_users_returns_empty_for_no_matches() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;
    
    let model_reader = Arc::new(MockModelReader::new());
    model_reader.add_type("store-1", TypeDefinition {
        type_name: "document".to_string(),
        relations: vec![RelationDefinition {
            name: "viewer".to_string(),
            type_constraints: vec!["user".into()],
            rewrite: Userset::This,
        }],
    }).await;
    
    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListUsersRequest::new(
        "store-1",
        "document:readme",
        "viewer",
        vec![UserFilter::new("user")],
    );

    let result = resolver.list_users(&request).await.unwrap();

    assert!(result.users.is_empty());
}

// ========== Section 2: Wildcards ==========

#[tokio::test]
async fn test_list_users_returns_wildcard() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;
    
    tuple_reader.add_tuple("store-1", "document", "public", "viewer", "user", "*", None).await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader.add_type("store-1", TypeDefinition {
        type_name: "document".to_string(),
        relations: vec![RelationDefinition {
            name: "viewer".to_string(),
            type_constraints: vec!["user".into()],
            rewrite: Userset::This,
        }],
    }).await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListUsersRequest::new(
        "store-1",
        "document:public",
        "viewer",
        vec![UserFilter::new("user")],
    );

    let result = resolver.list_users(&request).await.unwrap();

    assert_eq!(result.users.len(), 1);
    assert!(result.users.contains(&UserResult::wildcard("user")));
}

// ========== Section 3: Userset Filters ==========

#[tokio::test]
async fn test_list_users_returns_userset_references() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;
    
    tuple_reader.add_tuple("store-1", "document", "readme", "viewer", "group", "engineering", Some("member")).await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader.add_type("store-1", TypeDefinition {
        type_name: "document".to_string(),
        relations: vec![RelationDefinition {
            name: "viewer".to_string(),
            type_constraints: vec!["group".into()],
            rewrite: Userset::This,
        }],
    }).await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListUsersRequest::new(
        "store-1",
        "document:readme",
        "viewer",
        vec![UserFilter::with_relation("group", "member")],
    );

    let result = resolver.list_users(&request).await.unwrap();

    assert_eq!(result.users.len(), 1);
    assert!(result
        .users
        .contains(&UserResult::userset("group", "engineering", "member")));
}

// ========== Section 4: Contextual Tuples ==========

#[tokio::test]
async fn test_list_users_includes_contextual_tuples() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;
    
    tuple_reader.add_tuple("store-1", "document", "readme", "viewer", "user", "alice", None).await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader.add_type("store-1", TypeDefinition {
        type_name: "document".to_string(),
        relations: vec![RelationDefinition {
            name: "viewer".to_string(),
            type_constraints: vec!["user".into()],
            rewrite: Userset::This,
        }],
    }).await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Add contextual tuple for bob
    let contextual_tuples = vec![crate::resolver::ContextualTuple::new(
        "user:bob",
        "viewer",
        "document:readme",
    )];

    let request = ListUsersRequest::with_context(
        "store-1",
        "document:readme",
        "viewer",
        vec![UserFilter::new("user")],
        contextual_tuples,
        HashMap::new(),
    );

    let result = resolver.list_users(&request).await.unwrap();

    assert_eq!(result.users.len(), 2);
    assert!(result.users.contains(&UserResult::object("user", "alice")));
    assert!(result.users.contains(&UserResult::object("user", "bob")));
}

// ========== Section 5: Error Handling ==========

#[tokio::test]
async fn test_list_users_returns_error_for_nonexistent_store() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());
    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListUsersRequest::new(
        "nonexistent",
        "document:readme",
        "viewer",
        vec![UserFilter::new("user")],
    );

    let result = resolver.list_users(&request).await;

    assert!(matches!(result, Err(DomainError::StoreNotFound { .. })));
}

#[tokio::test]
async fn test_list_users_returns_error_for_empty_relation() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;
    
    let model_reader = Arc::new(MockModelReader::new());
    // Basic model setup mostly ignored for validation error early return
    model_reader.add_type("store-1", TypeDefinition {
        type_name: "document".to_string(),
        relations: vec![],
    }).await;
    
    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListUsersRequest::new(
        "store-1",
        "document:readme",
        "", // Empty relation
        vec![UserFilter::new("user")],
    );

    let result = resolver.list_users(&request).await;

    assert!(matches!(
        result,
        Err(DomainError::InvalidRelationFormat { .. })
    ));
}

#[tokio::test]
async fn test_list_users_returns_error_for_empty_user_filters() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;
    
    let model_reader = Arc::new(MockModelReader::new());
    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListUsersRequest::new(
        "store-1",
        "document:readme",
        "viewer",
        vec![], // Empty user filters
    );

    let result = resolver.list_users(&request).await;

    // This returns a generic ResolverError or InvalidInput
    assert!(matches!(result, Err(DomainError::ResolverError { .. })));
}

#[tokio::test]
async fn test_list_users_returns_error_for_invalid_object() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;
    let model_reader = Arc::new(MockModelReader::new());
    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListUsersRequest::new(
        "store-1",
        "invalid_object_format", // Missing colon
        "viewer",
        vec![UserFilter::new("user")],
    );

    let result = resolver.list_users(&request).await;

    assert!(matches!(
        result,
        Err(DomainError::InvalidObjectFormat { .. })
    ));
}

// ========== Section 6: parse_user_string validation ==========

#[tokio::test]
async fn test_contextual_tuple_with_invalid_user_format_is_skipped() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;
    let model_reader = Arc::new(MockModelReader::new());
    model_reader.add_type("store-1", TypeDefinition {
        type_name: "document".to_string(),
        relations: vec![RelationDefinition {
            name: "viewer".to_string(),
            type_constraints: vec!["user".into()],
            rewrite: Userset::This,
        }],
    }).await;
    
    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Contextual tuple with invalid user format (empty type)
    let contextual_tuples = vec![crate::resolver::ContextualTuple::new(
        ":invalid", // Invalid: empty type
        "viewer",
        "document:readme",
    )];

    let request = ListUsersRequest::with_context(
        "store-1",
        "document:readme",
        "viewer",
        vec![UserFilter::new("user")],
        contextual_tuples,
        HashMap::new(),
    );

    // Should succeed but skip the invalid tuple
    let result = resolver.list_users(&request).await.unwrap();
    assert!(result.users.is_empty());
}

#[tokio::test]
async fn test_contextual_tuple_with_empty_userset_relation_is_skipped() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;
    let model_reader = Arc::new(MockModelReader::new());
    model_reader.add_type("store-1", TypeDefinition {
        type_name: "document".to_string(),
        relations: vec![RelationDefinition {
            name: "viewer".to_string(),
            type_constraints: vec!["user".into()],
            rewrite: Userset::This,
        }],
    }).await;
    
    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Contextual tuple with invalid user format (empty relation in userset)
    let contextual_tuples = vec![crate::resolver::ContextualTuple::new(
        "group:eng#", // Invalid: empty relation after #
        "viewer",
        "document:readme",
    )];

    let request = ListUsersRequest::with_context(
        "store-1",
        "document:readme",
        "viewer",
        vec![UserFilter::new("user")],
        contextual_tuples,
        HashMap::new(),
    );

    // Should succeed but skip the invalid tuple
    let result = resolver.list_users(&request).await.unwrap();
    assert!(result.users.is_empty());
}

// ========== Section 7: Multiple User Filters ==========

#[tokio::test]
async fn test_list_users_with_multiple_user_filters() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;
    
    tuple_reader.add_tuple("store-1", "document", "readme", "viewer", "user", "alice", None).await;
    tuple_reader.add_tuple("store-1", "document", "readme", "viewer", "group", "eng", Some("member")).await;
    tuple_reader.add_tuple("store-1", "document", "readme", "viewer", "service_account", "bot", None).await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader.add_type("store-1", TypeDefinition {
        type_name: "document".to_string(),
        relations: vec![RelationDefinition {
            name: "viewer".to_string(),
            type_constraints: vec!["user".into(), "group".into(), "service_account".into()],
            rewrite: Userset::This,
        }],
    }).await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Request both 'user' and 'group#member' types
    let request = ListUsersRequest::new(
        "store-1",
        "document:readme",
        "viewer",
        vec![
            UserFilter::new("user"),
            UserFilter::with_relation("group", "member"),
        ],
    );

    let result = resolver.list_users(&request).await.unwrap();

    // Should return alice and group:eng#member, but NOT service_account:bot
    assert_eq!(result.users.len(), 2);
    assert!(result.users.contains(&UserResult::object("user", "alice")));
    assert!(result
        .users
        .contains(&UserResult::userset("group", "eng", "member")));
}

// ========== Section 8: Deduplication ==========

#[tokio::test]
async fn test_list_users_deduplicates_results() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;
    
    tuple_reader.add_tuple("store-1", "document", "readme", "viewer", "user", "alice", None).await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader.add_type("store-1", TypeDefinition {
        type_name: "document".to_string(),
        relations: vec![RelationDefinition {
            name: "viewer".to_string(),
            type_constraints: vec!["user".into()],
            rewrite: Userset::This,
        }],
    }).await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Add contextual tuple that duplicates alice
    let contextual_tuples = vec![crate::resolver::ContextualTuple::new(
        "user:alice",
        "viewer",
        "document:readme",
    )];

    let request = ListUsersRequest::with_context(
        "store-1",
        "document:readme",
        "viewer",
        vec![UserFilter::new("user")],
        contextual_tuples,
        HashMap::new(),
    );

    let result = resolver.list_users(&request).await.unwrap();

    // Should only have alice once
    assert_eq!(result.users.len(), 1);
    assert!(result.users.contains(&UserResult::object("user", "alice")));
}
