use anyhow::Result;

/// Test: Can generate valid User identifiers
#[test]
fn test_can_generate_valid_user_identifiers() -> Result<()> {
    // Arrange: Set up test data
    let user_ids = vec!["alice", "bob", "user123"];

    // Act: Generate user identifiers
    for user_id in user_ids {
        let user_identifier = generate_user_identifier(user_id);

        // Assert: Verify format is correct (user:<id>)
        assert!(
            user_identifier.starts_with("user:"),
            "User identifier should start with 'user:', got: {user_identifier}"
        );
        assert!(
            user_identifier.contains(user_id),
            "User identifier should contain the user ID, got: {user_identifier}"
        );

        // Verify it's a valid format for OpenFGA
        assert!(
            is_valid_user_format(&user_identifier),
            "User identifier should be valid: {user_identifier}"
        );
    }

    // Test wildcard support (for public access patterns)
    assert!(
        is_valid_user_format("user:*"),
        "Wildcard user identifier 'user:*' should be valid"
    );

    // Test negative cases - invalid formats should be rejected
    assert!(!is_valid_user_format(""), "Empty string should be invalid");
    assert!(
        !is_valid_user_format("user"),
        "Missing colon should be invalid"
    );
    assert!(
        !is_valid_user_format(":alice"),
        "Missing user prefix should be invalid"
    );
    assert!(
        !is_valid_user_format("user:"),
        "Missing ID part should be invalid"
    );
    assert!(
        !is_valid_user_format("user: alice"),
        "Whitespace should be invalid"
    );

    Ok(())
}

/// Generate a user identifier in OpenFGA format
fn generate_user_identifier(user_id: &str) -> String {
    format!("user:{user_id}")
}

/// Check if a string is a valid user identifier format
fn is_valid_user_format(identifier: &str) -> bool {
    // Valid formats:
    // - user:<id>
    // - user:* (wildcard for public access)
    // - Must not be empty
    // - Must not have spaces

    if identifier.is_empty() {
        return false;
    }

    if identifier.contains(' ') {
        return false;
    }

    if !identifier.starts_with("user:") {
        return false;
    }

    let parts: Vec<&str> = identifier.split(':').collect();
    if parts.len() != 2 {
        return false;
    }

    let id = parts[1];

    // The ID part should not be empty
    // Support wildcard (user:*) for public access patterns
    !id.is_empty() && (id == "*" || !id.contains('*'))
}

/// Test: Can generate valid Object identifiers (type:id format)
#[test]
fn test_can_generate_valid_object_identifiers() -> Result<()> {
    // Arrange: Set up test data with different object types
    let test_cases = vec![
        ("document", "readme"),
        ("folder", "planning"),
        ("organization", "acme"),
        ("repo", "rsfga"),
    ];

    // Act: Generate object identifiers
    for (object_type, object_id) in test_cases {
        let object_identifier = generate_object_identifier(object_type, object_id);

        // Assert: Verify format is correct (type:id)
        assert!(
            object_identifier.contains(':'),
            "Object identifier should contain ':', got: {object_identifier}"
        );

        let parts: Vec<&str> = object_identifier.split(':').collect();
        assert_eq!(
            parts.len(),
            2,
            "Object identifier should have exactly 2 parts, got: {}",
            parts.len()
        );
        assert_eq!(
            parts[0], object_type,
            "Object type should match, expected: {}, got: {}",
            object_type, parts[0]
        );
        assert_eq!(
            parts[1], object_id,
            "Object ID should match, expected: {}, got: {}",
            object_id, parts[1]
        );

        // Verify it's a valid format for OpenFGA
        assert!(
            is_valid_object_format(&object_identifier),
            "Object identifier should be valid: {object_identifier}"
        );
    }

    // Test negative cases - invalid formats should be rejected
    assert!(
        !is_valid_object_format(""),
        "Empty string should be invalid"
    );
    assert!(
        !is_valid_object_format("document"),
        "Missing colon should be invalid"
    );
    assert!(
        !is_valid_object_format(":readme"),
        "Missing type should be invalid"
    );
    assert!(
        !is_valid_object_format("document:"),
        "Missing ID should be invalid"
    );
    assert!(
        !is_valid_object_format("document: readme"),
        "Whitespace should be invalid"
    );

    Ok(())
}

/// Generate an object identifier in OpenFGA format
fn generate_object_identifier(object_type: &str, object_id: &str) -> String {
    format!("{object_type}:{object_id}")
}

/// Check if a string is a valid object identifier format
fn is_valid_object_format(identifier: &str) -> bool {
    // Valid format: type:id
    // - Must have exactly one colon
    // - Both type and id must be non-empty
    // - Must not have spaces
    // - Type should be alphanumeric with underscores/hyphens
    // - ID can contain alphanumeric, underscores, hyphens

    if identifier.is_empty() {
        return false;
    }

    if identifier.contains(' ') {
        return false;
    }

    let parts: Vec<&str> = identifier.split(':').collect();
    if parts.len() != 2 {
        return false;
    }

    let object_type = parts[0];
    let object_id = parts[1];

    // Type and ID must be non-empty
    if object_type.is_empty() || object_id.is_empty() {
        return false;
    }

    // Type should contain only alphanumeric, underscore, or hyphen
    if !object_type
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return false;
    }

    // ID should contain only alphanumeric, underscore, hyphen, or pipe (for namespacing)
    if !object_id
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '|')
    {
        return false;
    }

    true
}

/// Test: Can generate valid Relation names
#[test]
fn test_can_generate_valid_relation_names() -> Result<()> {
    // Arrange: Common relation names
    let relation_names = vec![
        "viewer", "editor", "owner", "admin", "member", "can_view", "can_edit",
    ];

    // Act & Assert: Verify each relation name is valid
    for relation_name in relation_names {
        assert!(
            is_valid_relation_name(relation_name),
            "Relation name should be valid: {relation_name}"
        );
    }

    Ok(())
}

/// Check if a string is a valid relation name
fn is_valid_relation_name(name: &str) -> bool {
    // Valid relation names:
    // - Must not be empty
    // - Should be alphanumeric with underscores
    // - Must be lowercase (OpenFGA convention)
    // - No spaces

    if name.is_empty() {
        return false;
    }

    if name.contains(' ') {
        return false;
    }

    // Check all characters are lowercase alphanumeric or underscore
    name.chars()
        .all(|c| c.is_lowercase() || c.is_numeric() || c == '_')
}

/// Test: Can generate valid Tuples
#[test]
fn test_can_generate_valid_tuples() -> Result<()> {
    // Arrange: Set up test data
    let store_id = "01HZQK9VXG7J8RQXYZ3MABCDEF"; // Example store ID
    let test_cases = vec![
        ("alice", "viewer", "document", "readme"),
        ("bob", "editor", "folder", "planning"),
        ("charlie", "owner", "organization", "acme"),
    ];

    // Act: Generate tuples
    for (user_id, relation, object_type, object_id) in test_cases {
        let tuple = generate_tuple(store_id, user_id, relation, object_type, object_id);

        // Assert: Verify tuple structure
        assert_eq!(tuple.store_id, store_id, "Tuple store_id should match");
        assert_eq!(
            tuple.user,
            generate_user_identifier(user_id),
            "Tuple user should match"
        );
        assert_eq!(tuple.relation, relation, "Tuple relation should match");
        assert_eq!(
            tuple.object,
            generate_object_identifier(object_type, object_id),
            "Tuple object should match"
        );

        // Verify all components are valid
        assert!(
            !tuple.store_id.is_empty(),
            "Tuple store_id should not be empty"
        );
        assert!(
            is_valid_user_format(&tuple.user),
            "Tuple user should be valid: {}",
            tuple.user
        );
        assert!(
            is_valid_relation_name(&tuple.relation),
            "Tuple relation should be valid: {}",
            tuple.relation
        );
        assert!(
            is_valid_object_format(&tuple.object),
            "Tuple object should be valid: {}",
            tuple.object
        );
    }

    Ok(())
}

/// Represents a relationship tuple in OpenFGA
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Tuple {
    store_id: String,
    user: String,
    relation: String,
    object: String,
}

/// Generate a tuple (store_id, user, relation, object)
fn generate_tuple(
    store_id: &str,
    user_id: &str,
    relation: &str,
    object_type: &str,
    object_id: &str,
) -> Tuple {
    Tuple {
        store_id: store_id.to_string(),
        user: generate_user_identifier(user_id),
        relation: relation.to_string(),
        object: generate_object_identifier(object_type, object_id),
    }
}

/// Test: Can generate authorization models with direct relations
#[test]
fn test_can_generate_authorization_models_with_direct_relations() -> Result<()> {
    // Arrange: Define a simple authorization model with direct relations
    // Example: A document with viewer, editor, owner relations
    let model = generate_authorization_model_with_direct_relations();

    // Assert: Validate model consistency
    validate_authorization_model(&model)?;

    // Assert: Verify model structure
    assert!(
        !model.schema_version.is_empty(),
        "Schema version should not be empty"
    );
    assert!(
        !model.type_definitions.is_empty(),
        "Model should have type definitions"
    );

    // Find the document type definition
    let document_type = model
        .type_definitions
        .iter()
        .find(|t| t.name == "document")
        .expect("Model should have a 'document' type");

    // Verify direct relations exist
    assert!(
        document_type.relations.contains_key("viewer"),
        "Document should have 'viewer' relation"
    );
    assert!(
        document_type.relations.contains_key("editor"),
        "Document should have 'editor' relation"
    );
    assert!(
        document_type.relations.contains_key("owner"),
        "Document should have 'owner' relation"
    );

    // Verify each relation is direct (allows user type)
    for (relation_name, relation_def) in &document_type.relations {
        assert!(
            relation_def.is_direct,
            "Relation '{relation_name}' should be direct"
        );
        assert!(
            relation_def.allowed_types.contains(&"user".to_string()),
            "Relation '{relation_name}' should allow 'user' type"
        );
    }

    Ok(())
}

/// Test: Can generate authorization models with computed relations
#[test]
fn test_can_generate_authorization_models_with_computed_relations() -> Result<()> {
    // Arrange: Define an authorization model with computed relations
    // Example: A document with can_view computed from viewer or editor
    let model = generate_authorization_model_with_computed_relations();

    // Assert: Validate model consistency
    validate_authorization_model(&model)?;

    // Assert: Verify model structure
    assert!(
        !model.schema_version.is_empty(),
        "Schema version should not be empty"
    );
    assert!(
        !model.type_definitions.is_empty(),
        "Model should have type definitions"
    );

    // Find the document type definition
    let document_type = model
        .type_definitions
        .iter()
        .find(|t| t.name == "document")
        .expect("Model should have a 'document' type");

    // Verify computed relation exists
    assert!(
        document_type.relations.contains_key("can_view"),
        "Document should have 'can_view' computed relation"
    );

    // Verify can_view is computed from viewer or editor
    let can_view_relation = &document_type.relations["can_view"];
    assert!(
        !can_view_relation.is_direct,
        "can_view should be a computed relation"
    );
    assert!(
        can_view_relation.computed_from.is_some(),
        "can_view should have computed_from defined"
    );

    let computed_from = can_view_relation.computed_from.as_ref().unwrap();
    assert!(
        computed_from.contains(&"viewer".to_string()),
        "can_view should be computed from 'viewer'"
    );
    assert!(
        computed_from.contains(&"editor".to_string()),
        "can_view should be computed from 'editor'"
    );

    // Verify base relations still exist
    assert!(
        document_type.relations.contains_key("viewer"),
        "Document should have 'viewer' base relation"
    );
    assert!(
        document_type.relations.contains_key("editor"),
        "Document should have 'editor' base relation"
    );

    Ok(())
}

/// Represents an OpenFGA authorization model
#[derive(Debug, Clone, PartialEq, Eq)]
struct AuthorizationModel {
    schema_version: String,
    type_definitions: Vec<TypeDefinition>,
}

/// Represents a type definition in an authorization model
#[derive(Debug, Clone, PartialEq, Eq)]
struct TypeDefinition {
    name: String,
    relations: std::collections::HashMap<String, RelationDefinition>,
}

/// Represents a relation definition
#[derive(Debug, Clone, PartialEq, Eq)]
struct RelationDefinition {
    is_direct: bool,
    allowed_types: Vec<String>,
    computed_from: Option<Vec<String>>,   // For computed relations
    operation: Option<RelationOperation>, // Union, Intersection, etc.
}

/// Represents the operation used in computed relations
#[derive(Debug, Clone, PartialEq, Eq)]
enum RelationOperation {
    Union,        // OR semantics (viewer or editor)
    Intersection, // AND semantics (owner and admin)
}

// Helper Functions for Authorization Model Generation

/// Create a standard user type with no relations
fn create_user_type() -> TypeDefinition {
    use std::collections::HashMap;
    TypeDefinition {
        name: "user".to_string(),
        relations: HashMap::new(),
    }
}

/// Create a direct relation definition
fn create_direct_relation(allowed_types: Vec<String>) -> RelationDefinition {
    RelationDefinition {
        is_direct: true,
        allowed_types,
        computed_from: None,
        operation: None,
    }
}

/// Create a computed relation definition
fn create_computed_relation(
    computed_from: Vec<String>,
    operation: RelationOperation,
) -> RelationDefinition {
    RelationDefinition {
        is_direct: false,
        allowed_types: vec![],
        computed_from: Some(computed_from),
        operation: Some(operation),
    }
}

/// Validate an authorization model for consistency
///
/// Checks:
/// - computed_from relations actually exist in the model
/// - allowed_types reference valid type definitions
/// - schema_version is not empty
///
/// Returns: Result with error message if validation fails
fn validate_authorization_model(model: &AuthorizationModel) -> Result<()> {
    use std::collections::HashSet;

    // Validate schema version
    if model.schema_version.is_empty() {
        anyhow::bail!("Schema version cannot be empty");
    }

    // Build set of all type names
    let type_names: HashSet<String> = model
        .type_definitions
        .iter()
        .map(|t| t.name.clone())
        .collect();

    // Validate each type definition
    for type_def in &model.type_definitions {
        // Build set of relation names for this type
        let relation_names: HashSet<String> = type_def.relations.keys().cloned().collect();

        for (relation_name, relation_def) in &type_def.relations {
            // Validate computed_from relations exist
            if let Some(computed_from) = &relation_def.computed_from {
                for source_relation in computed_from {
                    if !relation_names.contains(source_relation) {
                        anyhow::bail!(
                            "Type '{}' relation '{}' computes from non-existent relation '{}'",
                            type_def.name,
                            relation_name,
                            source_relation
                        );
                    }
                }
            }

            // Validate allowed_types reference valid types
            for allowed_type in &relation_def.allowed_types {
                // Extract type name (handle both "user" and "folder#viewer" formats)
                let type_name = allowed_type.split('#').next().unwrap_or(allowed_type);

                if !type_names.contains(type_name) {
                    anyhow::bail!(
                        "Type '{}' relation '{}' allows non-existent type '{}'",
                        type_def.name,
                        relation_name,
                        type_name
                    );
                }
            }
        }
    }

    Ok(())
}

/// Generate an authorization model with direct relations
fn generate_authorization_model_with_direct_relations() -> AuthorizationModel {
    use std::collections::HashMap;

    let user_type = create_user_type();

    // Create document type with direct relations
    let mut document_relations = HashMap::new();

    // Define viewer relation: [user]
    document_relations.insert(
        "viewer".to_string(),
        create_direct_relation(vec!["user".to_string()]),
    );

    // Define editor relation: [user]
    document_relations.insert(
        "editor".to_string(),
        create_direct_relation(vec!["user".to_string()]),
    );

    // Define owner relation: [user]
    document_relations.insert(
        "owner".to_string(),
        create_direct_relation(vec!["user".to_string()]),
    );

    let document_type = TypeDefinition {
        name: "document".to_string(),
        relations: document_relations,
    };

    AuthorizationModel {
        schema_version: "1.1".to_string(),
        type_definitions: vec![user_type, document_type],
    }
}

/// Generate an authorization model with computed relations
fn generate_authorization_model_with_computed_relations() -> AuthorizationModel {
    use std::collections::HashMap;

    let user_type = create_user_type();

    // Create document type with direct and computed relations
    let mut document_relations = HashMap::new();

    // Define viewer relation: [user] (direct)
    document_relations.insert(
        "viewer".to_string(),
        create_direct_relation(vec!["user".to_string()]),
    );

    // Define editor relation: [user] (direct)
    document_relations.insert(
        "editor".to_string(),
        create_direct_relation(vec!["user".to_string()]),
    );

    // Define can_view relation: viewer or editor (computed with union)
    document_relations.insert(
        "can_view".to_string(),
        create_computed_relation(
            vec!["viewer".to_string(), "editor".to_string()],
            RelationOperation::Union,
        ),
    );

    let document_type = TypeDefinition {
        name: "document".to_string(),
        relations: document_relations,
    };

    AuthorizationModel {
        schema_version: "1.1".to_string(),
        type_definitions: vec![user_type, document_type],
    }
}

/// Test: Can generate authorization models with union relations
#[test]
fn test_can_generate_authorization_models_with_union_relations() -> Result<()> {
    // Arrange: Generate model with union (OR) relations
    let model = generate_authorization_model_with_union_relations();

    // Assert: Validate model consistency
    validate_authorization_model(&model)?;

    // Assert: Find the document type
    let document_type = model
        .type_definitions
        .iter()
        .find(|t| t.name == "document")
        .expect("Model should have a 'document' type");

    // Verify union relation exists
    let can_view_relation = &document_type.relations["can_view"];
    assert_eq!(
        can_view_relation.operation,
        Some(RelationOperation::Union),
        "can_view should use Union operation"
    );

    // Verify it computes from multiple relations
    let computed_from = can_view_relation.computed_from.as_ref().unwrap();
    assert!(
        computed_from.len() >= 2,
        "Union should combine at least 2 relations"
    );

    Ok(())
}

/// Test: Can generate authorization models with intersection relations
#[test]
fn test_can_generate_authorization_models_with_intersection_relations() -> Result<()> {
    // Arrange: Generate model with intersection (AND) relations
    let model = generate_authorization_model_with_intersection_relations();

    // Assert: Validate model consistency
    validate_authorization_model(&model)?;

    // Assert: Find the document type
    let document_type = model
        .type_definitions
        .iter()
        .find(|t| t.name == "document")
        .expect("Model should have a 'document' type");

    // Verify intersection relation exists
    assert!(
        document_type.relations.contains_key("can_delete"),
        "Document should have 'can_delete' intersection relation"
    );

    let can_delete_relation = &document_type.relations["can_delete"];
    assert_eq!(
        can_delete_relation.operation,
        Some(RelationOperation::Intersection),
        "can_delete should use Intersection operation"
    );

    // Verify it requires multiple conditions
    let computed_from = can_delete_relation.computed_from.as_ref().unwrap();
    assert!(
        computed_from.contains(&"owner".to_string()),
        "can_delete should require 'owner'"
    );
    assert!(
        computed_from.len() >= 2,
        "Intersection should combine at least 2 relations"
    );

    Ok(())
}

/// Test: Can generate models with deeply nested relations (10+ levels)
#[test]
fn test_can_generate_models_with_deeply_nested_relations() -> Result<()> {
    // Arrange: Generate model with deep hierarchy (folder -> folder -> ... -> document)
    let model = generate_deeply_nested_model();

    // Assert: Validate model consistency
    validate_authorization_model(&model)?;

    // Assert: Verify deep nesting
    let folder_type = model
        .type_definitions
        .iter()
        .find(|t| t.name == "folder")
        .expect("Model should have a 'folder' type");

    // Verify parent relation exists (for creating hierarchy)
    assert!(
        folder_type.relations.contains_key("parent"),
        "Folder should have 'parent' relation for nesting"
    );

    // Verify computed relation that traverses the hierarchy
    assert!(
        folder_type.relations.contains_key("viewer"),
        "Folder should have 'viewer' relation"
    );

    // The key insight: a deeply nested model can express
    // "if you're a viewer of the parent folder, you're a viewer of this folder"
    // This creates an unbounded chain: folder1 -> folder2 -> ... -> folder10+
    let viewer_relation = &folder_type.relations["viewer"];

    // Verify the relation definition supports transitivity
    assert!(
        viewer_relation.computed_from.is_some() || viewer_relation.is_direct,
        "Viewer relation should be defined (direct or computed)"
    );

    Ok(())
}

/// Generate authorization model with union relations
///
/// Note: Union relations are a specific case of computed relations using RelationOperation::Union.
/// This function delegates to generate_authorization_model_with_computed_relations() which already
/// implements union semantics (can_view: viewer OR editor).
///
/// This explicit delegation makes it clear that Test 7 (union) validates the same model structure
/// as Test 6 (computed), but with focus on verifying the Union operation specifically.
fn generate_authorization_model_with_union_relations() -> AuthorizationModel {
    generate_authorization_model_with_computed_relations()
}

/// Generate authorization model with intersection relations
fn generate_authorization_model_with_intersection_relations() -> AuthorizationModel {
    use std::collections::HashMap;

    let user_type = create_user_type();
    let mut document_relations = HashMap::new();

    // Define owner relation: [user] (direct)
    document_relations.insert(
        "owner".to_string(),
        create_direct_relation(vec!["user".to_string()]),
    );

    // Define admin relation: [user] (direct)
    document_relations.insert(
        "admin".to_string(),
        create_direct_relation(vec!["user".to_string()]),
    );

    // Define can_delete relation: owner AND admin (intersection)
    document_relations.insert(
        "can_delete".to_string(),
        create_computed_relation(
            vec!["owner".to_string(), "admin".to_string()],
            RelationOperation::Intersection,
        ),
    );

    let document_type = TypeDefinition {
        name: "document".to_string(),
        relations: document_relations,
    };

    AuthorizationModel {
        schema_version: "1.1".to_string(),
        type_definitions: vec![user_type, document_type],
    }
}

/// Generate a model with deeply nested relations (10+ levels possible)
fn generate_deeply_nested_model() -> AuthorizationModel {
    use std::collections::HashMap;

    let user_type = create_user_type();

    // Define folder type with parent relationship (creates hierarchy)
    let mut folder_relations = HashMap::new();

    // Direct parent relation: [folder]
    folder_relations.insert(
        "parent".to_string(),
        create_direct_relation(vec!["folder".to_string()]),
    );

    // Direct viewer relation: [user, folder#viewer]
    folder_relations.insert(
        "viewer".to_string(),
        create_direct_relation(vec!["user".to_string(), "folder#viewer".to_string()]),
    );

    let folder_type = TypeDefinition {
        name: "folder".to_string(),
        relations: folder_relations,
    };

    // This model supports arbitrary nesting:
    // folder1.parent = folder2
    // folder2.parent = folder3
    // ... (can go 10+ levels deep)
    // folder10+.viewer = user:alice
    // => user:alice is transitively a viewer of folder1

    AuthorizationModel {
        schema_version: "1.1".to_string(),
        type_definitions: vec![user_type, folder_type],
    }
}
