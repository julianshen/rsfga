# RSFGA Data Models

## Overview

This document specifies all data models used in RSFGA, including:
- Core domain models (Stores, Authorization Models, Tuples)
- Runtime models (Check contexts, cache keys)
- Storage models (Database schemas)
- Wire formats (Protobuf, JSON)

---

## Table of Contents

1. [Core Domain Models](#core-domain-models)
2. [Authorization Model DSL](#authorization-model-dsl)
3. [Relationship Tuples](#relationship-tuples)
4. [Runtime Models](#runtime-models)
5. [Storage Models](#storage-models)
6. [Protobuf Definitions](#protobuf-definitions)

---

## Core Domain Models

### Store

Represents an authorization store (tenant).

**Rust**:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Store {
    /// Unique identifier (ULID)
    pub id: String,

    /// Human-readable name
    pub name: String,

    /// Creation timestamp
    pub created_at: DateTime<Utc>,

    /// Last update timestamp
    pub updated_at: DateTime<Utc>,

    /// Soft delete timestamp (optional)
    pub deleted_at: Option<DateTime<Utc>>,
}
```

**PostgreSQL**:
```sql
CREATE TABLE stores (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name VARCHAR(255) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX idx_stores_deleted ON stores(deleted_at) WHERE deleted_at IS NULL;
```

---

### Authorization Model

Defines the authorization schema for a store.

**Rust**:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizationModel {
    /// Model identifier (ULID)
    pub id: String,

    /// Store this model belongs to
    pub store_id: String,

    /// Schema version (e.g., "1.1")
    pub schema_version: String,

    /// Type definitions
    pub type_definitions: HashMap<String, TypeDefinition>,

    /// Creation timestamp
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeDefinition {
    /// Type name (e.g., "document", "user")
    pub type_name: String,

    /// Relations defined on this type
    pub relations: HashMap<String, Relation>,

    /// Metadata (for API compatibility)
    pub metadata: Option<TypeMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeMetadata {
    /// Relation metadata
    pub relations: HashMap<String, RelationMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationMetadata {
    /// Directly related user types
    pub directly_related_user_types: Vec<RelationReference>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationReference {
    /// Type name
    pub type_name: String,

    /// Optional relation (for usersets like "team#member")
    pub relation: Option<String>,

    /// Optional wildcard indicator
    pub wildcard: Option<bool>,
}
```

---

### Relation Types

All supported relation definitions.

**Rust**:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Relation {
    /// Direct assignment: user:alice → document:doc1#viewer
    Direct {
        /// Allowed user types
        allowed_types: Vec<TypeRef>,
    },

    /// Computed userset: viewer := editor
    Computed {
        /// Source relation
        userset: String,
    },

    /// Union: viewer := editor or owner
    Union {
        /// Child relations
        children: Vec<Relation>,
    },

    /// Intersection: can_view := viewer and active
    Intersection {
        /// Child relations
        children: Vec<Relation>,
    },

    /// Exclusion: viewer := all but banned
    Exclusion {
        /// Base relation
        base: Box<Relation>,

        /// Subtract relation
        subtract: Box<Relation>,
    },

    /// Tuple-to-userset: viewer := editor from parent
    TupleToUserset {
        /// Tupleset relation (e.g., "parent")
        tupleset: String,

        /// Computed userset on parent (e.g., "editor")
        computed_userset: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeRef {
    /// Type name
    pub type_name: String,

    /// Optional relation for usersets
    pub relation: Option<String>,
}
```

**Examples**:

1. **Direct**:
```rust
Relation::Direct {
    allowed_types: vec![
        TypeRef { type_name: "user".to_string(), relation: None },
        TypeRef { type_name: "team".to_string(), relation: Some("member".to_string()) },
    ],
}
```
DSL: `define viewer: [user, team#member]`

2. **Computed**:
```rust
Relation::Computed {
    userset: "editor".to_string(),
}
```
DSL: `define viewer: editor`

3. **Union**:
```rust
Relation::Union {
    children: vec![
        Relation::Computed { userset: "editor".to_string() },
        Relation::Computed { userset: "owner".to_string() },
    ],
}
```
DSL: `define viewer: editor or owner`

4. **Tuple-to-Userset**:
```rust
Relation::TupleToUserset {
    tupleset: "parent".to_string(),
    computed_userset: "viewer".to_string(),
}
```
DSL: `define viewer: viewer from parent`

---

## Authorization Model DSL

### Grammar

```ebnf
model          = "model" schema_version type_defs
schema_version = "schema" VERSION
type_defs      = type_def+
type_def       = "type" IDENTIFIER ( "relations" relation_defs )?
relation_defs  = relation_def+
relation_def   = "define" IDENTIFIER ":" relation_expr

relation_expr  = direct_expr
               | computed_expr
               | union_expr
               | intersection_expr
               | exclusion_expr
               | ttu_expr

direct_expr    = "[" type_ref ( "," type_ref )* "]"
computed_expr  = IDENTIFIER
union_expr     = relation_expr ( "or" relation_expr )+
intersection   = relation_expr ( "and" relation_expr )+
exclusion_expr = relation_expr "but not" relation_expr
ttu_expr       = IDENTIFIER "from" IDENTIFIER

type_ref       = IDENTIFIER ( "#" IDENTIFIER )?
```

### Example Model

```
model
  schema 1.1

type user

type team
  relations
    define member: [user]

type document
  relations
    define parent: [folder]
    define owner: [user]
    define editor: [user, team#member]
    define viewer: [user, team#member] or editor or owner
    define can_delete: owner
    define can_share: owner or editor
    define can_view: viewer or viewer from parent
```

---

## Relationship Tuples

### Tuple

Represents a relationship between a user and an object.

**Rust**:
```rust
#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
pub struct Tuple {
    /// Store identifier
    pub store_id: String,

    /// Object type and ID (e.g., "document:readme")
    pub object: ObjectRef,

    /// Relation (e.g., "viewer")
    pub relation: String,

    /// User/subject (e.g., "user:alice" or "team:eng#member")
    pub user: UserRef,

    /// Optional condition
    pub condition: Option<Condition>,

    /// Timestamp when tuple was created
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
pub struct ObjectRef {
    pub type_name: String,
    pub id: String,
}

impl ObjectRef {
    pub fn parse(s: &str) -> Result<Self> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 2 {
            return Err(Error::InvalidObjectRef(s.to_string()));
        }
        Ok(Self {
            type_name: parts[0].to_string(),
            id: parts[1].to_string(),
        })
    }

    pub fn to_string(&self) -> String {
        format!("{}:{}", self.type_name, self.id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
pub struct UserRef {
    pub type_name: String,
    pub id: String,
    /// Optional relation for usersets (e.g., "team:eng#member")
    pub relation: Option<String>,
}

impl UserRef {
    pub fn parse(s: &str) -> Result<Self> {
        // Parse "user:alice" or "team:eng#member"
        let parts: Vec<&str> = s.split('#').collect();

        let obj_parts: Vec<&str> = parts[0].split(':').collect();
        if obj_parts.len() != 2 {
            return Err(Error::InvalidUserRef(s.to_string()));
        }

        Ok(Self {
            type_name: obj_parts[0].to_string(),
            id: obj_parts[1].to_string(),
            relation: parts.get(1).map(|s| s.to_string()),
        })
    }

    pub fn to_string(&self) -> String {
        match &self.relation {
            Some(rel) => format!("{}:{}#{}", self.type_name, self.id, rel),
            None => format!("{}:{}", self.type_name, self.id),
        }
    }

    pub fn is_userset(&self) -> bool {
        self.relation.is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
pub struct Condition {
    /// Condition name (from model)
    pub name: String,

    /// Context data for CEL evaluation
    pub context: serde_json::Value,
}
```

**Examples**:

```rust
// Simple direct assignment
Tuple {
    store_id: "store-123".to_string(),
    object: ObjectRef::parse("document:readme")?,
    relation: "viewer".to_string(),
    user: UserRef::parse("user:alice")?,
    condition: None,
    timestamp: Utc::now(),
}

// Userset assignment
Tuple {
    store_id: "store-123".to_string(),
    object: ObjectRef::parse("document:readme")?,
    relation: "viewer".to_string(),
    user: UserRef::parse("team:engineering#member")?,  // All team members
    condition: None,
    timestamp: Utc::now(),
}

// With condition (CEL)
Tuple {
    store_id: "store-123".to_string(),
    object: ObjectRef::parse("document:readme")?,
    relation: "viewer".to_string(),
    user: UserRef::parse("user:alice")?,
    condition: Some(Condition {
        name: "ip_allowlist".to_string(),
        context: json!({"allowed_ips": ["192.168.1.0/24"]}),
    }),
    timestamp: Utc::now(),
}
```

---

## Runtime Models

### Check Context

Context for a single check operation.

**Rust**:
```rust
#[derive(Debug, Clone)]
pub struct CheckContext {
    /// Store identifier
    pub store_id: String,

    /// Authorization model version
    pub model_id: String,

    /// Authorization model (cached)
    pub model: Arc<AuthorizationModel>,

    /// Contextual tuples (temporary, not in DB)
    pub contextual_tuples: Vec<Tuple>,

    /// CEL context for condition evaluation
    pub cel_context: Option<serde_json::Value>,

    /// Visited nodes (cycle detection)
    pub visited: Arc<DashMap<(String, String, String), ()>>,

    /// Current depth
    pub depth: Arc<AtomicU32>,

    /// Maximum depth allowed
    pub max_depth: u32,

    /// Start time (for timeout)
    pub start_time: Instant,

    /// Timeout duration
    pub timeout: Duration,

    /// Whether to include resolution trace
    pub trace: bool,

    /// Resolution trace (if enabled)
    pub resolution_trace: Arc<Mutex<Vec<String>>>,
}

impl CheckContext {
    pub fn new(
        store_id: String,
        model: Arc<AuthorizationModel>,
        config: &ResolverConfig,
    ) -> Self {
        Self {
            store_id,
            model_id: model.id.clone(),
            model,
            contextual_tuples: Vec::new(),
            cel_context: None,
            visited: Arc::new(DashMap::new()),
            depth: Arc::new(AtomicU32::new(0)),
            max_depth: config.max_depth,
            start_time: Instant::now(),
            timeout: config.timeout,
            trace: false,
            resolution_trace: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn with_contextual_tuples(mut self, tuples: Vec<Tuple>) -> Self {
        self.contextual_tuples = tuples;
        self
    }

    pub fn with_trace(mut self, trace: bool) -> Self {
        self.trace = trace;
        self
    }

    pub fn is_timeout(&self) -> bool {
        self.start_time.elapsed() > self.timeout
    }

    pub fn add_trace(&self, message: String) {
        if self.trace {
            self.resolution_trace.lock().unwrap().push(message);
        }
    }
}
```

---

### Cache Key

Key for caching check results.

**Rust**:
```rust
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct CacheKey {
    pub store_id: String,
    pub model_id: String,
    pub object: String,
    pub relation: String,
    pub user: String,
    /// Hash of contextual tuples (if any)
    pub contextual_tuples_hash: Option<u64>,
}

impl CacheKey {
    pub fn new(
        store_id: &str,
        model_id: &str,
        object: &str,
        relation: &str,
        user: &str,
        contextual_tuples: &[Tuple],
    ) -> Self {
        let contextual_tuples_hash = if contextual_tuples.is_empty() {
            None
        } else {
            Some(Self::hash_tuples(contextual_tuples))
        };

        Self {
            store_id: store_id.to_string(),
            model_id: model_id.to_string(),
            object: object.to_string(),
            relation: relation.to_string(),
            user: user.to_string(),
            contextual_tuples_hash,
        }
    }

    fn hash_tuples(tuples: &[Tuple]) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        tuples.hash(&mut hasher);
        hasher.finish()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        // Serialize for Redis/Valkey storage
        bincode::serialize(self).unwrap()
    }
}
```

---

## Storage Models

### PostgreSQL Schema

**Stores**:
```sql
CREATE TABLE stores (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name VARCHAR(255) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deleted_at TIMESTAMPTZ,

    CONSTRAINT stores_name_check CHECK (length(name) > 0)
);

CREATE INDEX idx_stores_deleted ON stores(deleted_at) WHERE deleted_at IS NULL;
CREATE INDEX idx_stores_created ON stores(created_at DESC);
```

**Authorization Models**:
```sql
CREATE TABLE authorization_models (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    store_id UUID NOT NULL REFERENCES stores(id) ON DELETE CASCADE,
    schema_version VARCHAR(10) NOT NULL,
    type_definitions JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT authorization_models_schema_check
        CHECK (schema_version IN ('1.0', '1.1'))
);

CREATE INDEX idx_models_store_created
    ON authorization_models(store_id, created_at DESC);

CREATE INDEX idx_models_latest
    ON authorization_models(store_id, created_at DESC)
    WHERE created_at = (
        SELECT MAX(created_at)
        FROM authorization_models am2
        WHERE am2.store_id = authorization_models.store_id
    );

-- GIN index for searching type definitions
CREATE INDEX idx_models_type_defs
    ON authorization_models USING GIN (type_definitions);
```

**Tuples**:
```sql
CREATE TABLE tuples (
    store_id UUID NOT NULL REFERENCES stores(id) ON DELETE CASCADE,
    object_type VARCHAR(255) NOT NULL,
    object_id VARCHAR(255) NOT NULL,
    relation VARCHAR(255) NOT NULL,
    user_type VARCHAR(255) NOT NULL,
    user_id VARCHAR(255) NOT NULL,
    user_relation VARCHAR(255),
    condition_name VARCHAR(255),
    condition_context JSONB,
    inserted_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- Composite primary key (prevents duplicates)
    PRIMARY KEY (
        store_id,
        object_type,
        object_id,
        relation,
        user_type,
        user_id,
        COALESCE(user_relation, '')
    ),

    CONSTRAINT tuples_object_id_check CHECK (length(object_id) > 0),
    CONSTRAINT tuples_user_id_check CHECK (length(user_id) > 0)
);

-- Index for finding tuples by object (forward expansion)
CREATE INDEX idx_tuples_object
    ON tuples(store_id, object_type, object_id);

-- Index for finding tuples by user (reverse expansion)
CREATE INDEX idx_tuples_user
    ON tuples(store_id, user_type, user_id);

-- Index for usersets (team#member lookups)
CREATE INDEX idx_tuples_userset
    ON tuples(store_id, user_type, user_id, user_relation)
    WHERE user_relation IS NOT NULL;

-- Index for relation-specific lookups
CREATE INDEX idx_tuples_relation
    ON tuples(store_id, object_type, relation);

-- Index for time-based queries (CDC, cleanup)
CREATE INDEX idx_tuples_inserted
    ON tuples(store_id, inserted_at DESC);
```

**Changelog (for replication)**:
```sql
CREATE TABLE changelog (
    id BIGSERIAL PRIMARY KEY,
    store_id UUID NOT NULL,
    operation VARCHAR(10) NOT NULL, -- 'WRITE' or 'DELETE'
    object_type VARCHAR(255) NOT NULL,
    object_id VARCHAR(255) NOT NULL,
    relation VARCHAR(255) NOT NULL,
    user_type VARCHAR(255) NOT NULL,
    user_id VARCHAR(255) NOT NULL,
    user_relation VARCHAR(255),
    condition_name VARCHAR(255),
    condition_context JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT changelog_operation_check
        CHECK (operation IN ('WRITE', 'DELETE'))
);

CREATE INDEX idx_changelog_store_created
    ON changelog(store_id, created_at);

CREATE INDEX idx_changelog_created
    ON changelog(created_at DESC);

-- Partition by month for efficient cleanup
CREATE TABLE changelog_y2024m01 PARTITION OF changelog
    FOR VALUES FROM ('2024-01-01') TO ('2024-02-01');

-- Auto-cleanup old changelog entries
CREATE OR REPLACE FUNCTION cleanup_old_changelog()
RETURNS void AS $$
BEGIN
    DELETE FROM changelog
    WHERE created_at < NOW() - INTERVAL '7 days';
END;
$$ LANGUAGE plpgsql;

-- Schedule cleanup (requires pg_cron extension)
-- SELECT cron.schedule('cleanup-changelog', '0 2 * * *', 'SELECT cleanup_old_changelog()');
```

---

## Protobuf Definitions

### Store

```protobuf
syntax = "proto3";

package openfga.v1;

import "google/protobuf/timestamp.proto";

message Store {
  string id = 1;
  string name = 2;
  google.protobuf.Timestamp created_at = 3;
  google.protobuf.Timestamp updated_at = 4;
  google.protobuf.Timestamp deleted_at = 5;
}

message CreateStoreRequest {
  string name = 1;
}

message CreateStoreResponse {
  Store store = 1;
}

message GetStoreRequest {
  string store_id = 1;
}

message GetStoreResponse {
  Store store = 1;
}

message ListStoresRequest {
  int32 page_size = 1;
  string continuation_token = 2;
}

message ListStoresResponse {
  repeated Store stores = 1;
  string continuation_token = 2;
}

message DeleteStoreRequest {
  string store_id = 1;
}

message DeleteStoreResponse {}
```

### Authorization Model

```protobuf
message AuthorizationModel {
  string id = 1;
  string schema_version = 2;
  repeated TypeDefinition type_definitions = 3;
}

message TypeDefinition {
  string type = 1;
  map<string, Userset> relations = 2;
  TypeDefinitionMetadata metadata = 3;
}

message TypeDefinitionMetadata {
  map<string, RelationMetadata> relations = 1;
}

message RelationMetadata {
  repeated RelationReference directly_related_user_types = 1;
}

message RelationReference {
  string type = 1;
  string relation = 2;
  Wildcard wildcard = 3;
}

message Wildcard {}

message Userset {
  oneof userset {
    DirectUserset this = 1;
    ComputedUserset computed_userset = 2;
    TupleToUserset tuple_to_userset = 3;
    SetOperation union = 4;
    SetOperation intersection = 5;
    Difference difference = 6;
  }
}

message DirectUserset {}

message ComputedUserset {
  string relation = 1;
}

message TupleToUserset {
  string tupleset = 1;
  ComputedUserset computed_userset = 2;
}

message SetOperation {
  repeated Userset child = 1;
}

message Difference {
  Userset base = 1;
  Userset subtract = 2;
}
```

### Tuple

```protobuf
message TupleKey {
  string user = 1;
  string relation = 2;
  string object = 3;
  Condition condition = 4;
}

message Condition {
  string name = 1;
  google.protobuf.Struct context = 2;
}

message Tuple {
  TupleKey key = 1;
  google.protobuf.Timestamp timestamp = 2;
}

message ContextualTupleKeys {
  repeated TupleKey tuple_keys = 1;
}
```

### Check

```protobuf
message CheckRequest {
  string store_id = 1;
  TupleKey tuple_key = 2;
  ContextualTupleKeys contextual_tuples = 3;
  string authorization_model_id = 4;
  bool trace = 5;
  google.protobuf.Struct context = 6;
}

message CheckResponse {
  bool allowed = 1;
  string resolution = 2;
}

message BatchCheckRequest {
  string store_id = 1;
  repeated CheckRequestItem checks = 2;
  string authorization_model_id = 3;
}

message CheckRequestItem {
  TupleKey tuple_key = 1;
  ContextualTupleKeys contextual_tuples = 2;
  google.protobuf.Struct context = 3;
}

message BatchCheckResponse {
  repeated CheckResponseItem results = 1;
}

message CheckResponseItem {
  CheckRequestItem request = 1;
  bool allowed = 2;
  string resolution = 3;
  Error error = 4;
}

message Error {
  string code = 1;
  string message = 2;
  google.protobuf.Struct details = 3;
}
```

---

## Serialization Formats

### JSON Examples

**Store**:
```json
{
  "id": "01HQWV7Y8Z9X6W5V4U3T2S1R0Q",
  "name": "My Application",
  "created_at": "2024-01-15T10:30:00Z",
  "updated_at": "2024-01-15T10:30:00Z"
}
```

**Tuple**:
```json
{
  "user": "user:alice",
  "relation": "viewer",
  "object": "document:readme",
  "condition": {
    "name": "ip_allowlist",
    "context": {
      "allowed_ips": ["192.168.1.0/24"]
    }
  }
}
```

**Authorization Model** (simplified):
```json
{
  "schema_version": "1.1",
  "type_definitions": [
    {
      "type": "document",
      "relations": {
        "viewer": {
          "this": {}
        },
        "editor": {
          "this": {}
        }
      },
      "metadata": {
        "relations": {
          "viewer": {
            "directly_related_user_types": [
              {"type": "user"}
            ]
          }
        }
      }
    }
  ]
}
```

---

## Data Validation

### Tuple Validation

```rust
impl Tuple {
    pub fn validate(&self, model: &AuthorizationModel) -> Result<()> {
        // 1. Check if object type exists
        let type_def = model.type_definitions
            .get(&self.object.type_name)
            .ok_or_else(|| Error::TypeNotFound(self.object.type_name.clone()))?;

        // 2. Check if relation exists
        let relation_def = type_def.relations
            .get(&self.relation)
            .ok_or_else(|| Error::RelationNotFound(self.relation.clone()))?;

        // 3. Check if user type is allowed
        let allowed_types = relation_def.get_allowed_types()?;

        let user_type_ref = if let Some(rel) = &self.user.relation {
            format!("{}#{}", self.user.type_name, rel)
        } else {
            self.user.type_name.clone()
        };

        let is_allowed = allowed_types.iter().any(|t| {
            if let Some(rel) = &t.relation {
                format!("{}#{}", t.type_name, rel) == user_type_ref
            } else {
                t.type_name == self.user.type_name
            }
        });

        if !is_allowed {
            return Err(Error::UserTypeNotAllowed {
                user_type: user_type_ref,
                relation: self.relation.clone(),
                object_type: self.object.type_name.clone(),
            });
        }

        // 4. Validate condition (if present)
        if let Some(condition) = &self.condition {
            // Check if condition exists in model
            // Validate CEL expression (future)
        }

        Ok(())
    }
}
```

---

## Summary

This document specifies all data models used in RSFGA:

- ✅ **Core Models**: Store, AuthorizationModel, Tuple
- ✅ **Relation Types**: Direct, Computed, Union, Intersection, Exclusion, TTU
- ✅ **Runtime Models**: CheckContext, CacheKey
- ✅ **Storage Schema**: PostgreSQL tables with indexes
- ✅ **Wire Formats**: Protobuf and JSON

**Next**: See [C4_DIAGRAMS.md](./C4_DIAGRAMS.md) for architecture visualizations.
