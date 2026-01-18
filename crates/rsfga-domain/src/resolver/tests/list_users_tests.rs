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
use crate::resolver::{GraphResolver, ListUsersRequest, ResolverConfig, UserFilter, UserResult};

// ========== Section 1: Direct Relations ==========

#[tokio::test]
async fn test_list_users_returns_users_with_direct_relation() {
    // Setup: document:readme has viewer relation with user:alice and user:bob
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;

    tuple_reader
        .add_tuple(
            "store-1", "document", "readme", "viewer", "user", "alice", None,
        )
        .await;
    tuple_reader
        .add_tuple(
            "store-1", "document", "readme", "viewer", "user", "bob", None,
        )
        .await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListUsersRequest::new(
        "store-1",
        "document:readme",
        "viewer",
        vec![UserFilter::new("user")],
    );

    let result = resolver.list_users(&request, 1000).await.unwrap();

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

    tuple_reader
        .add_tuple(
            "store-1", "document", "readme", "viewer", "user", "alice", None,
        )
        .await;
    tuple_reader
        .add_tuple(
            "store-1",
            "document",
            "readme",
            "viewer",
            "service_account",
            "bot-1",
            None,
        )
        .await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into(), "service_account".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Only request users of type 'user'
    let request = ListUsersRequest::new(
        "store-1",
        "document:readme",
        "viewer",
        vec![UserFilter::new("user")],
    );

    let result = resolver.list_users(&request, 1000).await.unwrap();

    assert_eq!(result.users.len(), 1);
    assert!(result.users.contains(&UserResult::object("user", "alice")));
}

#[tokio::test]
async fn test_list_users_returns_empty_for_no_matches() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListUsersRequest::new(
        "store-1",
        "document:readme",
        "viewer",
        vec![UserFilter::new("user")],
    );

    let result = resolver.list_users(&request, 1000).await.unwrap();

    assert!(result.users.is_empty());
}

// ========== Section 2: Wildcards ==========

#[tokio::test]
async fn test_list_users_returns_wildcard() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;

    tuple_reader
        .add_tuple("store-1", "document", "public", "viewer", "user", "*", None)
        .await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListUsersRequest::new(
        "store-1",
        "document:public",
        "viewer",
        vec![UserFilter::new("user")],
    );

    let result = resolver.list_users(&request, 1000).await.unwrap();

    assert_eq!(result.users.len(), 1);
    assert!(result.users.contains(&UserResult::wildcard("user")));
}

// ========== Section 3: Userset Filters ==========

#[tokio::test]
async fn test_list_users_returns_userset_references() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;

    tuple_reader
        .add_tuple(
            "store-1",
            "document",
            "readme",
            "viewer",
            "group",
            "engineering",
            Some("member"),
        )
        .await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["group".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListUsersRequest::new(
        "store-1",
        "document:readme",
        "viewer",
        vec![UserFilter::with_relation("group", "member")],
    );

    let result = resolver.list_users(&request, 1000).await.unwrap();

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

    tuple_reader
        .add_tuple(
            "store-1", "document", "readme", "viewer", "user", "alice", None,
        )
        .await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

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

    let result = resolver.list_users(&request, 1000).await.unwrap();

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

    let result = resolver.list_users(&request, 1000).await;

    assert!(matches!(result, Err(DomainError::StoreNotFound { .. })));
}

#[tokio::test]
async fn test_list_users_returns_error_for_empty_relation() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;

    let model_reader = Arc::new(MockModelReader::new());
    // Basic model setup mostly ignored for validation error early return
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListUsersRequest::new(
        "store-1",
        "document:readme",
        "", // Empty relation
        vec![UserFilter::new("user")],
    );

    let result = resolver.list_users(&request, 1000).await;

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

    let result = resolver.list_users(&request, 1000).await;

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

    let result = resolver.list_users(&request, 1000).await;

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
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

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
    let result = resolver.list_users(&request, 1000).await.unwrap();
    assert!(result.users.is_empty());
}

#[tokio::test]
async fn test_contextual_tuple_with_empty_userset_relation_is_skipped() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;
    let model_reader = Arc::new(MockModelReader::new());
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

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
    let result = resolver.list_users(&request, 1000).await.unwrap();
    assert!(result.users.is_empty());
}

// ========== Section 7: Multiple User Filters ==========

#[tokio::test]
async fn test_list_users_with_multiple_user_filters() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;

    tuple_reader
        .add_tuple(
            "store-1", "document", "readme", "viewer", "user", "alice", None,
        )
        .await;
    tuple_reader
        .add_tuple(
            "store-1",
            "document",
            "readme",
            "viewer",
            "group",
            "eng",
            Some("member"),
        )
        .await;
    tuple_reader
        .add_tuple(
            "store-1",
            "document",
            "readme",
            "viewer",
            "service_account",
            "bot",
            None,
        )
        .await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into(), "group".into(), "service_account".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

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

    let result = resolver.list_users(&request, 1000).await.unwrap();

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

    tuple_reader
        .add_tuple(
            "store-1", "document", "readme", "viewer", "user", "alice", None,
        )
        .await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

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

    let result = resolver.list_users(&request, 1000).await.unwrap();

    // Should only have alice once
    assert_eq!(result.users.len(), 1);
    assert!(result.users.contains(&UserResult::object("user", "alice")));
}

// ========== Section 9: Computed Relations ==========
// Note: These tests document expected behavior for computed relations.
// Currently ListUsers only supports direct relations (Userset::This).
// Computed relation traversal is tracked for future implementation.

#[tokio::test]
#[ignore = "ListUsers computed relation resolution requires recursive definition traversal - not yet implemented"]
async fn test_list_users_resolves_union_relation() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;

    // viewer = owner or editor
    tuple_reader
        .add_tuple(
            "store-1", "document", "readme", "owner", "user", "alice", None,
        )
        .await;
    tuple_reader
        .add_tuple(
            "store-1", "document", "readme", "editor", "user", "bob", None,
        )
        .await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "owner".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "editor".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::Union {
                            children: vec![
                                Userset::ComputedUserset {
                                    relation: "owner".to_string(),
                                },
                                Userset::ComputedUserset {
                                    relation: "editor".to_string(),
                                },
                            ],
                        },
                    },
                ],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListUsersRequest::new(
        "store-1",
        "document:readme",
        "viewer",
        vec![UserFilter::new("user")],
    );

    let result = resolver.list_users(&request, 1000).await.unwrap();

    // Both alice (owner) and bob (editor) should be viewers
    assert_eq!(result.users.len(), 2);
    assert!(result.users.contains(&UserResult::object("user", "alice")));
    assert!(result.users.contains(&UserResult::object("user", "bob")));
}

#[tokio::test]
#[ignore = "ListUsers computed relation resolution requires recursive definition traversal - not yet implemented"]
async fn test_list_users_resolves_intersection_relation() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;

    // alice is both member and approved
    tuple_reader
        .add_tuple(
            "store-1", "resource", "doc1", "member", "user", "alice", None,
        )
        .await;
    tuple_reader
        .add_tuple(
            "store-1", "resource", "doc1", "approved", "user", "alice", None,
        )
        .await;
    // bob is only member
    tuple_reader
        .add_tuple("store-1", "resource", "doc1", "member", "user", "bob", None)
        .await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "resource".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "member".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "approved".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "accessor".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::Intersection {
                            children: vec![
                                Userset::ComputedUserset {
                                    relation: "member".to_string(),
                                },
                                Userset::ComputedUserset {
                                    relation: "approved".to_string(),
                                },
                            ],
                        },
                    },
                ],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListUsersRequest::new(
        "store-1",
        "resource:doc1",
        "accessor",
        vec![UserFilter::new("user")],
    );

    let result = resolver.list_users(&request, 1000).await.unwrap();

    // Only alice should be returned (has both member AND approved)
    assert_eq!(result.users.len(), 1);
    assert!(result.users.contains(&UserResult::object("user", "alice")));
}

#[tokio::test]
#[ignore = "ListUsers computed relation resolution requires recursive definition traversal - not yet implemented"]
async fn test_list_users_resolves_exclusion_relation() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;

    // alice and bob are members, but bob is banned
    tuple_reader
        .add_tuple(
            "store-1", "resource", "doc1", "member", "user", "alice", None,
        )
        .await;
    tuple_reader
        .add_tuple("store-1", "resource", "doc1", "member", "user", "bob", None)
        .await;
    tuple_reader
        .add_tuple("store-1", "resource", "doc1", "banned", "user", "bob", None)
        .await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "resource".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "member".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "banned".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "active_member".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::Exclusion {
                            base: Box::new(Userset::ComputedUserset {
                                relation: "member".to_string(),
                            }),
                            subtract: Box::new(Userset::ComputedUserset {
                                relation: "banned".to_string(),
                            }),
                        },
                    },
                ],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListUsersRequest::new(
        "store-1",
        "resource:doc1",
        "active_member",
        vec![UserFilter::new("user")],
    );

    let result = resolver.list_users(&request, 1000).await.unwrap();

    // Only alice should be returned (member but not banned)
    assert_eq!(result.users.len(), 1);
    assert!(result.users.contains(&UserResult::object("user", "alice")));
}

#[tokio::test]
#[ignore = "ListUsers computed relation resolution requires recursive definition traversal - not yet implemented"]
async fn test_list_users_resolves_tuple_to_userset() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;

    // Document has a parent folder
    tuple_reader
        .add_tuple(
            "store-1", "document", "readme", "parent", "folder", "root", None,
        )
        .await;
    // Folder has viewers
    tuple_reader
        .add_tuple("store-1", "folder", "root", "viewer", "user", "alice", None)
        .await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "folder".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "parent".to_string(),
                        type_constraints: vec!["folder".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::TupleToUserset {
                            tupleset: "parent".to_string(),
                            computed_userset: "viewer".to_string(),
                        },
                    },
                ],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListUsersRequest::new(
        "store-1",
        "document:readme",
        "viewer",
        vec![UserFilter::new("user")],
    );

    let result = resolver.list_users(&request, 1000).await.unwrap();

    // Alice should be returned (viewer of parent folder)
    assert_eq!(result.users.len(), 1);
    assert!(result.users.contains(&UserResult::object("user", "alice")));
}

// ========== Section 10: Property-Based Tests ==========

use proptest::prelude::*;

/// Strategy to generate valid object identifiers
fn list_users_object_strategy() -> impl Strategy<Value = (String, String, String)> {
    ("[a-z]{1,8}", "[a-z0-9]{1,8}")
        .prop_map(|(type_name, id)| (type_name.clone(), id.clone(), format!("{type_name}:{id}")))
}

/// Strategy to generate valid relation names
fn list_users_relation_strategy() -> impl Strategy<Value = String> {
    "[a-z]{1,8}"
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    /// Property: ListUsers never panics on any input
    #[test]
    fn test_property_list_users_never_panics(
        (type_name, _object_id, object) in list_users_object_strategy(),
        relation in list_users_relation_strategy(),
        store_id in "[a-z]{1,5}",
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let tuple_reader = Arc::new(MockTupleReader::new());
            let model_reader = Arc::new(MockModelReader::new());

            tuple_reader.add_store(&store_id).await;

            // Add type definition
            model_reader
                .add_type(
                    &store_id,
                    TypeDefinition {
                        type_name: type_name.clone(),
                        relations: vec![RelationDefinition {
                            name: relation.clone(),
                            type_constraints: vec!["user".into()],
                            rewrite: Userset::This,
                        }],
                    },
                )
                .await;

            let resolver = GraphResolver::new(tuple_reader, model_reader);

            let request = ListUsersRequest::new(
                &store_id,
                &object,
                &relation,
                vec![UserFilter::new("user")],
            );

            // Should not panic - either succeeds or returns an error
            let _result = resolver.list_users(&request, 1000).await;
        });
    }

    /// Property: ListUsers always terminates within timeout
    #[test]
    fn test_property_list_users_always_terminates(
        (type_name, object_id, _object) in list_users_object_strategy(),
        relation in list_users_relation_strategy(),
        num_users in 1..20usize,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let terminated = rt.block_on(async {
            let tuple_reader = Arc::new(MockTupleReader::new());
            let model_reader = Arc::new(MockModelReader::new());

            tuple_reader.add_store("store1").await;

            // Add multiple users with varying computed relations
            for i in 0..num_users {
                tuple_reader
                    .add_tuple(
                        "store1",
                        &type_name,
                        &object_id,
                        &relation,
                        "user",
                        &format!("user{i}"),
                        None,
                    )
                    .await;
            }

            model_reader
                .add_type(
                    "store1",
                    TypeDefinition {
                        type_name: type_name.clone(),
                        relations: vec![RelationDefinition {
                            name: relation.clone(),
                            type_constraints: vec!["user".into()],
                            rewrite: Userset::This,
                        }],
                    },
                )
                .await;

            let resolver = GraphResolver::new(tuple_reader, model_reader);

            let request = ListUsersRequest::new(
                "store1",
                format!("{type_name}:{object_id}"),
                &relation,
                vec![UserFilter::new("user")],
            );

            // Use a timeout to ensure termination
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                resolver.list_users(&request, 1000),
            )
            .await;

            result.is_ok()
        });
        prop_assert!(terminated, "ListUsers should always terminate within 5 seconds");
    }

    /// Property: ListUsers returns only users matching the requested filter types
    #[test]
    fn test_property_list_users_returns_only_matching_filter_types(
        user_name in "[a-z]{1,5}",
        object_id in "[a-z0-9]{1,5}",
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let all_match = rt.block_on(async {
            let tuple_reader = Arc::new(MockTupleReader::new());
            let model_reader = Arc::new(MockModelReader::new());

            tuple_reader.add_store("store1").await;

            // Add tuples with different user types
            tuple_reader
                .add_tuple("store1", "document", &object_id, "viewer", "user", &user_name, None)
                .await;
            tuple_reader
                .add_tuple("store1", "document", &object_id, "viewer", "service", "bot1", None)
                .await;

            model_reader
                .add_type(
                    "store1",
                    TypeDefinition {
                        type_name: "document".to_string(),
                        relations: vec![RelationDefinition {
                            name: "viewer".to_string(),
                            type_constraints: vec!["user".into(), "service".into()],
                            rewrite: Userset::This,
                        }],
                    },
                )
                .await;

            let resolver = GraphResolver::new(tuple_reader, model_reader);

            // Request only users of type "user"
            let request = ListUsersRequest::new(
                "store1",
                format!("document:{object_id}"),
                "viewer",
                vec![UserFilter::new("user")],
            );

            let result = resolver.list_users(&request, 1000).await.unwrap();

            // All returned users should be of type "user"
            result.users.iter().all(|u| {
                match u {
                    UserResult::Object { user_type, .. } => user_type == "user",
                    UserResult::Userset { userset_type, .. } => userset_type == "user",
                    UserResult::Wildcard { wildcard_type } => wildcard_type == "user",
                }
            })
        });
        prop_assert!(all_match, "All returned users should match the requested filter type");
    }

    /// Property: For any user returned by ListUsers, Check should return true
    #[test]
    fn test_property_list_users_check_inverse_consistency(
        user_names in prop::collection::vec("[a-z]{1,5}", 1..5),
        object_id in "[a-z0-9]{1,5}",
    ) {
        use crate::resolver::CheckRequest;

        let rt = tokio::runtime::Runtime::new().unwrap();
        let all_consistent = rt.block_on(async {
            let tuple_reader = Arc::new(MockTupleReader::new());
            let model_reader = Arc::new(MockModelReader::new());

            tuple_reader.add_store("store1").await;

            // Add tuples for each user
            for user_name in &user_names {
                tuple_reader
                    .add_tuple("store1", "document", &object_id, "viewer", "user", user_name, None)
                    .await;
            }

            model_reader
                .add_type(
                    "store1",
                    TypeDefinition {
                        type_name: "document".to_string(),
                        relations: vec![RelationDefinition {
                            name: "viewer".to_string(),
                            type_constraints: vec!["user".into()],
                            rewrite: Userset::This,
                        }],
                    },
                )
                .await;

            let resolver = GraphResolver::new(tuple_reader, model_reader);

            // Get all users via ListUsers
            let list_request = ListUsersRequest::new(
                "store1",
                format!("document:{object_id}"),
                "viewer",
                vec![UserFilter::new("user")],
            );

            let list_result = resolver.list_users(&list_request, 1000).await.unwrap();

            // Verify Check returns true for each returned user
            let mut all_match = true;
            for user_result in &list_result.users {
                let user_string = match user_result {
                    UserResult::Object { user_type, user_id } => format!("{user_type}:{user_id}"),
                    UserResult::Userset { userset_type, userset_id, relation } => {
                        format!("{userset_type}:{userset_id}#{relation}")
                    }
                    UserResult::Wildcard { wildcard_type } => format!("{wildcard_type}:*"),
                };

                // Skip wildcards for Check verification (Check doesn't accept wildcards)
                if user_string.ends_with(":*") {
                    continue;
                }

                let check_request = CheckRequest {
                    store_id: "store1".to_string(),
                    user: user_string.clone(),
                    relation: "viewer".to_string(),
                    object: format!("document:{object_id}"),
                    contextual_tuples: Arc::new(vec![]),
                    context: Arc::new(HashMap::new()),
                };

                let check_result = resolver.check(&check_request).await;
                if let Ok(result) = check_result {
                    if !result.allowed {
                        all_match = false;
                        break;
                    }
                } else {
                    all_match = false;
                    break;
                }
            }
            all_match
        });
        prop_assert!(all_consistent, "Check should return true for all users returned by ListUsers");
    }
}

// ========== Section 11: ListUsers/ListObjects Inverse Consistency ==========

#[tokio::test]
#[ignore = "ListUsers/ListObjects consistency requires computed relations support"]
async fn test_list_users_list_objects_inverse_consistency() {
    #[allow(unused_imports)]
    use crate::resolver::{CheckRequest, ListObjectsRequest};

    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;

    // Setup: alice can view doc1, doc2; bob can view doc2, doc3
    tuple_reader
        .add_tuple(
            "store-1", "document", "doc1", "viewer", "user", "alice", None,
        )
        .await;
    tuple_reader
        .add_tuple(
            "store-1", "document", "doc2", "viewer", "user", "alice", None,
        )
        .await;
    tuple_reader
        .add_tuple("store-1", "document", "doc2", "viewer", "user", "bob", None)
        .await;
    tuple_reader
        .add_tuple("store-1", "document", "doc3", "viewer", "user", "bob", None)
        .await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Get objects alice can view via ListObjects
    let list_objects_request =
        ListObjectsRequest::new("store-1", "user:alice", "viewer", "document");
    let objects_result = resolver
        .list_objects(&list_objects_request, 100)
        .await
        .unwrap();

    // For each object, verify ListUsers returns alice
    for object in &objects_result.objects {
        let list_users_request =
            ListUsersRequest::new("store-1", object, "viewer", vec![UserFilter::new("user")]);

        let users_result = resolver
            .list_users(&list_users_request, 1000)
            .await
            .unwrap();
        assert!(
            users_result
                .users
                .contains(&UserResult::object("user", "alice")),
            "ListUsers for {} should contain alice",
            object
        );
    }

    // Inverse: For doc2, verify both alice and bob are returned by ListUsers
    let list_users_doc2 = ListUsersRequest::new(
        "store-1",
        "document:doc2",
        "viewer",
        vec![UserFilter::new("user")],
    );
    let users_doc2 = resolver.list_users(&list_users_doc2, 1000).await.unwrap();

    // For each user, verify ListObjects returns doc2
    for user_result in &users_doc2.users {
        let user_string = match user_result {
            UserResult::Object { user_type, user_id } => format!("{user_type}:{user_id}"),
            _ => continue,
        };

        let list_objects_for_user =
            ListObjectsRequest::new("store-1", &user_string, "viewer", "document");
        let objects_for_user = resolver
            .list_objects(&list_objects_for_user, 100)
            .await
            .unwrap();

        assert!(
            objects_for_user
                .objects
                .contains(&"document:doc2".to_string()),
            "ListObjects for {} should contain document:doc2",
            user_string
        );
    }
}

#[tokio::test]
#[ignore = "ListUsers computed relations not yet implemented - documents expected behavior"]
async fn test_list_users_list_objects_consistency_with_computed_relations() {
    use crate::resolver::ListObjectsRequest;

    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;

    // owner and editor are viewers
    tuple_reader
        .add_tuple(
            "store-1", "document", "doc1", "owner", "user", "alice", None,
        )
        .await;
    tuple_reader
        .add_tuple("store-1", "document", "doc1", "editor", "user", "bob", None)
        .await;
    tuple_reader
        .add_tuple("store-1", "document", "doc2", "editor", "user", "bob", None)
        .await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "owner".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "editor".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::Union {
                            children: vec![
                                Userset::ComputedUserset {
                                    relation: "owner".to_string(),
                                },
                                Userset::ComputedUserset {
                                    relation: "editor".to_string(),
                                },
                            ],
                        },
                    },
                ],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // ListUsers for doc1 viewer should return alice (owner) and bob (editor)
    let list_users_request = ListUsersRequest::new(
        "store-1",
        "document:doc1",
        "viewer",
        vec![UserFilter::new("user")],
    );
    let users_result = resolver
        .list_users(&list_users_request, 1000)
        .await
        .unwrap();

    assert_eq!(users_result.users.len(), 2);
    assert!(users_result
        .users
        .contains(&UserResult::object("user", "alice")));
    assert!(users_result
        .users
        .contains(&UserResult::object("user", "bob")));

    // ListObjects for bob viewer should return doc1 and doc2
    let list_objects_request = ListObjectsRequest::new("store-1", "user:bob", "viewer", "document");
    let objects_result = resolver
        .list_objects(&list_objects_request, 100)
        .await
        .unwrap();

    assert_eq!(objects_result.objects.len(), 2);
    assert!(objects_result
        .objects
        .contains(&"document:doc1".to_string()));
    assert!(objects_result
        .objects
        .contains(&"document:doc2".to_string()));
}

// ========== Section 12: Complex Graph Structures ==========
// Note: These tests require TupleToUserset computed relation support.

#[tokio::test]
#[ignore = "ListUsers computed relations not yet implemented - documents expected behavior"]
async fn test_list_users_with_deep_hierarchy() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;

    // Create a 5-level hierarchy: org -> team -> project -> folder -> document
    tuple_reader
        .add_tuple("store-1", "org", "acme", "member", "user", "alice", None)
        .await;
    tuple_reader
        .add_tuple("store-1", "team", "eng", "parent", "org", "acme", None)
        .await;
    tuple_reader
        .add_tuple("store-1", "project", "proj1", "parent", "team", "eng", None)
        .await;
    tuple_reader
        .add_tuple(
            "store-1", "folder", "root", "parent", "project", "proj1", None,
        )
        .await;
    tuple_reader
        .add_tuple(
            "store-1", "document", "readme", "parent", "folder", "root", None,
        )
        .await;

    let model_reader = Arc::new(MockModelReader::new());

    // Org type
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "org".to_string(),
                relations: vec![RelationDefinition {
                    name: "member".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    // Team type - member from parent org
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "team".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "parent".to_string(),
                        type_constraints: vec!["org".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "member".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::TupleToUserset {
                            tupleset: "parent".to_string(),
                            computed_userset: "member".to_string(),
                        },
                    },
                ],
            },
        )
        .await;

    // Project type - member from parent team
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "project".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "parent".to_string(),
                        type_constraints: vec!["team".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "member".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::TupleToUserset {
                            tupleset: "parent".to_string(),
                            computed_userset: "member".to_string(),
                        },
                    },
                ],
            },
        )
        .await;

    // Folder type - viewer from parent project
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "folder".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "parent".to_string(),
                        type_constraints: vec!["project".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::TupleToUserset {
                            tupleset: "parent".to_string(),
                            computed_userset: "member".to_string(),
                        },
                    },
                ],
            },
        )
        .await;

    // Document type - viewer from parent folder
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "parent".to_string(),
                        type_constraints: vec!["folder".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::TupleToUserset {
                            tupleset: "parent".to_string(),
                            computed_userset: "viewer".to_string(),
                        },
                    },
                ],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // ListUsers for document should resolve through the entire hierarchy
    let request = ListUsersRequest::new(
        "store-1",
        "document:readme",
        "viewer",
        vec![UserFilter::new("user")],
    );

    let result = resolver.list_users(&request, 1000).await.unwrap();

    // Alice should be found through: org:acme -> team:eng -> project:proj1 -> folder:root -> document:readme
    assert_eq!(result.users.len(), 1);
    assert!(result.users.contains(&UserResult::object("user", "alice")));
}

#[tokio::test]
#[ignore = "ListUsers computed relations not yet implemented - documents expected behavior"]
async fn test_list_users_handles_multiple_paths_to_same_user() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;

    // alice has access through multiple paths
    tuple_reader
        .add_tuple(
            "store-1", "document", "doc1", "owner", "user", "alice", None,
        )
        .await;
    tuple_reader
        .add_tuple(
            "store-1", "document", "doc1", "editor", "user", "alice", None,
        )
        .await;
    tuple_reader
        .add_tuple(
            "store-1", "document", "doc1", "viewer", "user", "alice", None,
        )
        .await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "owner".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "editor".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::Union {
                            children: vec![
                                Userset::This,
                                Userset::ComputedUserset {
                                    relation: "owner".to_string(),
                                },
                                Userset::ComputedUserset {
                                    relation: "editor".to_string(),
                                },
                            ],
                        },
                    },
                ],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListUsersRequest::new(
        "store-1",
        "document:doc1",
        "viewer",
        vec![UserFilter::new("user")],
    );

    let result = resolver.list_users(&request, 1000).await.unwrap();

    // alice should appear only once despite multiple paths
    assert_eq!(result.users.len(), 1);
    assert!(result.users.contains(&UserResult::object("user", "alice")));
}

// ========== Section: Truncation Tests ==========

#[tokio::test]
async fn test_list_users_truncates_when_exceeding_max_results() {
    // Setup: Create many users so we can test truncation
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;

    // Add 10 users
    for i in 0..10 {
        tuple_reader
            .add_tuple(
                "store-1",
                "document",
                "readme",
                "viewer",
                "user",
                &format!("user{}", i),
                None,
            )
            .await;
    }

    let model_reader = Arc::new(MockModelReader::new());
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListUsersRequest::new(
        "store-1",
        "document:readme",
        "viewer",
        vec![UserFilter::new("user")],
    );

    // Request with max_results = 5 (less than the 10 users we have)
    let result = resolver.list_users(&request, 5).await.unwrap();

    // Should truncate to 5 results
    assert_eq!(result.users.len(), 5);
    // Should indicate truncation
    assert!(
        result.truncated,
        "truncated flag should be true when results exceed max_results"
    );
}

#[tokio::test]
async fn test_list_users_not_truncated_when_under_max_results() {
    // Setup: Create a few users
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;

    tuple_reader
        .add_tuple(
            "store-1", "document", "readme", "viewer", "user", "alice", None,
        )
        .await;
    tuple_reader
        .add_tuple(
            "store-1", "document", "readme", "viewer", "user", "bob", None,
        )
        .await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListUsersRequest::new(
        "store-1",
        "document:readme",
        "viewer",
        vec![UserFilter::new("user")],
    );

    // Request with max_results = 10 (more than the 2 users we have)
    let result = resolver.list_users(&request, 10).await.unwrap();

    // Should return all 2 users
    assert_eq!(result.users.len(), 2);
    // Should NOT indicate truncation
    assert!(
        !result.truncated,
        "truncated flag should be false when results are under max_results"
    );
}

#[tokio::test]
async fn test_list_users_exactly_at_max_results_not_truncated() {
    // Setup: Create exactly max_results users
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;

    // Add exactly 5 users
    for i in 0..5 {
        tuple_reader
            .add_tuple(
                "store-1",
                "document",
                "readme",
                "viewer",
                "user",
                &format!("user{}", i),
                None,
            )
            .await;
    }

    let model_reader = Arc::new(MockModelReader::new());
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListUsersRequest::new(
        "store-1",
        "document:readme",
        "viewer",
        vec![UserFilter::new("user")],
    );

    // Request with max_results = 5 (exactly the number of users we have)
    let result = resolver.list_users(&request, 5).await.unwrap();

    // Should return all 5 users
    assert_eq!(result.users.len(), 5);
    // Should NOT indicate truncation (exact match is not truncated)
    assert!(
        !result.truncated,
        "truncated flag should be false when results exactly match max_results"
    );
}

// ========== Section: Depth Limit Tests ==========

/// Test that ListUsers returns DepthLimitExceeded error when graph exceeds max_depth.
/// This ensures the resolver properly enforces depth limits during userset traversal.
#[tokio::test]
async fn test_list_users_returns_error_on_depth_limit_exceeded() {
    use std::time::Duration;

    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;

    let model_reader = Arc::new(MockModelReader::new());
    // Create a model with deeply nested Union that will exceed depth limit
    // viewer -> Union -> Union -> Union -> Computed[base]
    // Depth:    1        2        3        4
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "base".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::Union {
                            children: vec![Userset::Union {
                                children: vec![Userset::Union {
                                    children: vec![Userset::ComputedUserset {
                                        relation: "base".to_string(),
                                    }],
                                }],
                            }],
                        },
                    },
                ],
            },
        )
        .await;

    // Add a user at the base level
    tuple_reader
        .add_tuple(
            "store-1", "document", "readme", "base", "user", "alice", None,
        )
        .await;

    // Use a very low max_depth (2) so the nested unions exceed it
    let config = ResolverConfig::default()
        .with_max_depth(2)
        .with_timeout(Duration::from_secs(30));
    let resolver = GraphResolver::with_config(tuple_reader, model_reader, config);

    let request = ListUsersRequest::new(
        "store-1",
        "document:readme",
        "viewer",
        vec![UserFilter::new("user")],
    );

    let result = resolver.list_users(&request, 1000).await;

    // Should return DepthLimitExceeded error
    assert!(result.is_err());
    match result.unwrap_err() {
        DomainError::DepthLimitExceeded { max_depth } => {
            assert_eq!(max_depth, 2);
        }
        err => panic!("Expected DepthLimitExceeded, got: {:?}", err),
    }
}

/// Test that ListUsers returns DepthLimitExceeded when traversing deeply nested intersection.
#[tokio::test]
async fn test_list_users_returns_error_on_intersection_depth_limit_exceeded() {
    use std::time::Duration;

    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;

    let model_reader = Arc::new(MockModelReader::new());
    // Create a model with deeply nested Intersection
    // viewer -> Intersection -> Intersection -> Intersection -> Computed[base]
    // Depth:    1               2               3               4
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "base".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::Intersection {
                            children: vec![Userset::Intersection {
                                children: vec![Userset::Intersection {
                                    children: vec![Userset::ComputedUserset {
                                        relation: "base".to_string(),
                                    }],
                                }],
                            }],
                        },
                    },
                ],
            },
        )
        .await;

    tuple_reader
        .add_tuple(
            "store-1", "document", "readme", "base", "user", "alice", None,
        )
        .await;

    // Use low max_depth (2) so nested intersections exceed it
    let config = ResolverConfig::default()
        .with_max_depth(2)
        .with_timeout(Duration::from_secs(30));
    let resolver = GraphResolver::with_config(tuple_reader, model_reader, config);

    let request = ListUsersRequest::new(
        "store-1",
        "document:readme",
        "viewer",
        vec![UserFilter::new("user")],
    );

    let result = resolver.list_users(&request, 1000).await;

    // Should return DepthLimitExceeded error
    assert!(result.is_err());
    match result.unwrap_err() {
        DomainError::DepthLimitExceeded { max_depth } => {
            assert_eq!(max_depth, 2);
        }
        err => panic!("Expected DepthLimitExceeded, got: {:?}", err),
    }
}

/// Test that ListUsers succeeds when Union nesting is within depth limits.
#[tokio::test]
async fn test_list_users_succeeds_within_depth_limit() {
    use std::time::Duration;

    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;

    let model_reader = Arc::new(MockModelReader::new());
    // Create a model with simple Union (depth 1)
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![
                    RelationDefinition {
                        name: "owner".to_string(),
                        type_constraints: vec!["user".into()],
                        rewrite: Userset::This,
                    },
                    RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::Union {
                            children: vec![Userset::ComputedUserset {
                                relation: "owner".to_string(),
                            }],
                        },
                    },
                ],
            },
        )
        .await;

    tuple_reader
        .add_tuple(
            "store-1", "document", "readme", "owner", "user", "alice", None,
        )
        .await;

    // Use max_depth (25) which is plenty for a single Union level
    let config = ResolverConfig::default()
        .with_max_depth(25)
        .with_timeout(Duration::from_secs(30));
    let resolver = GraphResolver::with_config(tuple_reader, model_reader, config);

    let request = ListUsersRequest::new(
        "store-1",
        "document:readme",
        "viewer",
        vec![UserFilter::new("user")],
    );

    let result = resolver.list_users(&request, 1000).await;

    // Should succeed and return alice
    assert!(
        result.is_ok(),
        "ListUsers should succeed within depth limit, got {:?}",
        result
    );
    let result = result.unwrap();
    assert_eq!(result.users.len(), 1);
    assert!(result.users.contains(&UserResult::object("user", "alice")));
}

// ========== Section: Exclusion Behavior Documentation ==========

/// This test documents the current behavior of excluded_users in ListUsers.
///
/// Note: excluded_users is currently always empty because full exclusion support
/// (tracking which users are excluded by 'but not' relations) is not yet implemented.
/// This matches OpenFGA's current behavior where excluded_users is rarely populated.
///
/// When full exclusion support is added, this test should be updated to verify
/// that excluded users are properly returned for 'but not' relations.
#[tokio::test]
async fn test_list_users_excluded_users_is_empty_for_direct_relations() {
    // Setup: document:readme has viewer relation with user:alice
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;

    tuple_reader
        .add_tuple(
            "store-1", "document", "readme", "viewer", "user", "alice", None,
        )
        .await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListUsersRequest::new(
        "store-1",
        "document:readme",
        "viewer",
        vec![UserFilter::new("user")],
    );

    let result = resolver.list_users(&request, 1000).await.unwrap();

    // Current behavior: excluded_users is always empty
    assert!(
        result.excluded_users.is_empty(),
        "excluded_users should be empty (exclusion tracking not yet implemented)"
    );
    // Users are returned normally
    assert_eq!(result.users.len(), 1);
    assert!(result.users.contains(&UserResult::object("user", "alice")));
}

// ========== Section: User Filter Validation Tests ==========

#[tokio::test]
async fn test_list_users_rejects_empty_filter_type() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Empty type_name in filter
    let request = ListUsersRequest::new(
        "store-1",
        "document:readme",
        "viewer",
        vec![UserFilter::new("")], // Empty type
    );

    let result = resolver.list_users(&request, 1000).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("user_filter type cannot be empty"),
        "Expected error about empty type, got: {}",
        err
    );
}

#[tokio::test]
async fn test_list_users_rejects_filter_type_with_invalid_characters() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Type with colon (invalid)
    let request = ListUsersRequest::new(
        "store-1",
        "document:readme",
        "viewer",
        vec![UserFilter::new("user:invalid")], // Contains colon
    );

    let result = resolver.list_users(&request, 1000).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("invalid characters"),
        "Expected error about invalid characters, got: {}",
        err
    );
}

#[tokio::test]
async fn test_list_users_rejects_filter_with_empty_relation() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["group".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Filter with empty relation
    let request = ListUsersRequest::new(
        "store-1",
        "document:readme",
        "viewer",
        vec![UserFilter::with_relation("group", "")], // Empty relation
    );

    let result = resolver.list_users(&request, 1000).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.to_string()
            .contains("user_filter relation cannot be empty"),
        "Expected error about empty relation, got: {}",
        err
    );
}

#[tokio::test]
async fn test_list_users_accepts_valid_filter_formats() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;

    tuple_reader
        .add_tuple(
            "store-1", "document", "readme", "viewer", "user", "alice", None,
        )
        .await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    // Valid type names with underscores and dashes
    let request = ListUsersRequest::new(
        "store-1",
        "document:readme",
        "viewer",
        vec![
            UserFilter::new("user"),
            UserFilter::new("service_account"),
            UserFilter::new("user-group"),
        ],
    );

    let result = resolver.list_users(&request, 1000).await;
    assert!(result.is_ok(), "Valid filter formats should be accepted");
}

// ========== Section 10: max_results Validation ==========

#[tokio::test]
async fn test_list_users_rejects_zero_max_results() {
    let tuple_reader = Arc::new(MockTupleReader::new());
    tuple_reader.add_store("store-1").await;

    let model_reader = Arc::new(MockModelReader::new());
    model_reader
        .add_type(
            "store-1",
            TypeDefinition {
                type_name: "document".to_string(),
                relations: vec![RelationDefinition {
                    name: "viewer".to_string(),
                    type_constraints: vec!["user".into()],
                    rewrite: Userset::This,
                }],
            },
        )
        .await;

    let resolver = GraphResolver::new(tuple_reader, model_reader);

    let request = ListUsersRequest::new(
        "store-1",
        "document:readme",
        "viewer",
        vec![UserFilter::new("user")],
    );

    // max_results = 0 should be rejected
    let result = resolver.list_users(&request, 0).await;
    assert!(result.is_err(), "max_results = 0 should be rejected");

    let err = result.unwrap_err();
    assert!(
        matches!(err, DomainError::ResolverError { .. }),
        "Should return ResolverError for invalid max_results"
    );
}
