# RSFGA Implementation Roadmap

## Overview

This roadmap outlines the phased implementation of RSFGA, from MVP to distributed edge deployment. Each phase builds upon the previous one, ensuring we maintain correctness while progressively improving performance and scalability.

---

## Phase 0: OpenFGA Compatibility Test Suite

**Goal**: Build comprehensive test suite that captures OpenFGA's actual behavior across all APIs

**Why This Phase Exists**: OpenFGA doesn't provide a compatibility test suite. We need ground truth to validate RSFGA's 100% API compatibility claim. This phase creates the validation framework used at the end of every subsequent phase.

**Success Criteria**:
- ✅ Test harness can execute tests against both OpenFGA and RSFGA
- ✅ ~150 compatibility tests covering all endpoints (Check, Batch, Write, Read, Expand, ListObjects, Store, Model)
- ✅ All relation types validated (direct, union, intersection, exclusion, this, wildcards)
- ✅ Performance baselines established for comparison
- ✅ Error response catalog documented
- ✅ Edge cases and limits documented

**Duration Estimate**: 7 weeks

**Validation**: Run suite against OpenFGA (100% pass)

---

### Milestone 0.1: Test Harness Foundation (Week 1)

**Goal**: Infrastructure to execute tests against OpenFGA and capture expected behavior

#### Tasks

**0.1.1 OpenFGA Test Environment**
- Set up Docker Compose for OpenFGA
- Create test environment setup/teardown scripts
- Verify HTTP and gRPC connectivity
- Implement test store lifecycle management

**0.1.2 Test Data Generators**
- User/Object/Relation identifier generators
- Tuple generators (all relation types)
- Authorization model generators (direct, computed, nested)
- Property-based test data generation

**0.1.3 Response Capture Framework**
- HTTP request/response capture utility
- gRPC request/response capture utility
- Serialization to JSON test fixtures
- Response comparison and diff tooling

**Validation Criteria**:
- [ ] OpenFGA running in Docker, accessible via HTTP/gRPC
- [ ] Test data generators produce valid, diverse inputs
- [ ] Can capture and replay requests deterministically

**Deliverables**:
- `tests/compatibility/harness/` - test infrastructure
- `docker-compose.openfga.yml` - OpenFGA environment
- Test data generator library

---

### Milestone 0.2: Store & Model API Tests (Week 2)

**Goal**: Capture Store and Authorization Model API behavior

#### Tasks

**0.2.1 Store CRUD Operations**
- Test store creation (POST /stores)
- Test store retrieval (GET /stores/{id})
- Test store deletion (DELETE /stores/{id})
- Test store listing with pagination
- Error cases (404, invalid inputs)

**0.2.2 Authorization Model Write**
- Model creation with various relation types
- Direct relations, computed relations (union/intersection/exclusion)
- this keyword, wildcards
- Model validation errors
- Duplicate types, undefined relation references

**0.2.3 Authorization Model Read**
- Model retrieval by ID
- Latest model retrieval
- Model listing and pagination
- Response format validation

**Validation Criteria**:
- [ ] 30+ test cases for Store API
- [ ] 15+ test cases for Authorization Model API
- [ ] All captured responses match OpenFGA exactly

**Deliverables**:
- Store API test suite with expected responses
- Authorization Model API test suite
- Error response catalog (Store/Model)

---

### Milestone 0.3: Tuple API Tests (Week 3)

**Goal**: Validate Tuple write and read operations

#### Tasks

**0.3.1 Tuple Write Operations**
- Single tuple writes
- Batch tuple writes
- Tuple deletions
- Idempotency verification
- Conditional writes
- Validation errors (invalid format, undefined types/relations)

**0.3.2 Tuple Read Operations**
- Read with user filter
- Read with relation filter
- Read with object filter
- Combined filters (AND logic)
- Pagination (page_size, continuation_token)

**0.3.3 Edge Cases**
- Wildcard users (user:*)
- Userset relations
- Contextual tuples
- Long identifiers (1000+ chars)
- Special characters, Unicode

**Validation Criteria**:
- [ ] 25+ test cases for Tuple API
- [ ] Pagination behavior documented
- [ ] Edge case limits identified

**Deliverables**:
- Tuple API test suite
- Edge case documentation
- Identifier length/character limits

---

### Milestone 0.4: Check API Tests (Week 4)

**Goal**: Validate Check and Batch Check (core authorization logic)

#### Tasks

**0.4.1 Basic Check Operations**
- Direct relation checks
- Computed relation resolution (union, intersection, exclusion)
- this keyword resolution
- Contextual tuples
- Error cases (404, 400)

**0.4.2 Complex Relation Resolution**
- Multi-hop relations (parent → child inheritance)
- Deeply nested relations (5+ hops)
- Union/intersection/exclusion combinations
- Cycle detection
- Wildcards

**0.4.3 Batch Check Operations**
- Multiple checks in single request
- Result ordering verification
- Mixed allowed/denied results
- Large batches (100+ checks)
- Deduplication behavior

**0.4.4 Performance Baselines**
- Direct relation check latency
- Computed relation check latency
- Deep nested relation latency
- Batch check throughput
- Consistency timing (write → check)

**Validation Criteria**:
- [ ] 30+ test cases for Check API
- [ ] All relation types tested
- [ ] Performance baselines established

**Deliverables**:
- Check API test suite
- Batch check test suite
- Performance baseline document (for Phase 1 comparison)
- Consistency guarantees documentation

---

### Milestone 0.5: Expand & ListObjects API Tests (Week 5)

**Goal**: Validate Expand and ListObjects APIs

#### Tasks

**0.5.1 Expand API**
- Relation tree generation
- Direct vs computed relation trees
- Union/intersection branches
- max_depth parameter
- Deterministic output verification

**0.5.2 ListObjects API**
- Objects accessible by user
- Direct relation filtering
- Computed relation filtering
- Type filters
- Pagination
- Contextual tuples

**0.5.3 Edge Cases**
- Very deep trees (20+ levels)
- Circular relation definitions
- Large result sets (1000+ objects)

**Validation Criteria**:
- [ ] 20+ test cases for Expand/ListObjects
- [ ] Tree format documented
- [ ] Edge case behavior understood

**Deliverables**:
- Expand API test suite
- ListObjects API test suite
- Relation tree format specification

---

### Milestone 0.6: Error Handling & Edge Cases (Week 6)

**Goal**: Systematically document all error conditions and limits

#### Tasks

**0.6.1 Error Response Format**
- 400 Bad Request structure
- 404 Not Found structure
- 500 Internal Server Error structure
- Error code enumeration
- Field-level validation errors

**0.6.2 Consistency & Concurrency**
- Concurrent writes to same tuple
- Read-after-write consistency timing
- Check during model update
- Stale model handling

**0.6.3 Limits & Boundaries**
- Maximum tuple size
- Maximum tuples per write
- Maximum checks per batch
- Maximum model size
- Maximum relation depth
- Request timeout behavior

**Validation Criteria**:
- [ ] All error formats documented
- [ ] Consistency model understood
- [ ] All limits documented

**Deliverables**:
- Complete error response catalog
- Consistency guarantee specification
- Limits and boundaries document

---

### Milestone 0.7: gRPC API Compatibility (Week 7)

**Goal**: Validate gRPC API matches REST API behavior

#### Tasks

**0.7.1 gRPC Service Validation**
- Store service methods
- Authorization Model service methods
- Tuple service methods
- Check service methods

**0.7.2 REST/gRPC Parity**
- Store.Create ↔ POST /stores
- Store.Get ↔ GET /stores/{id}
- Store.Delete ↔ DELETE /stores/{id}
- Check ↔ POST /stores/{id}/check
- All other endpoints verified

**0.7.3 gRPC-Specific Features**
- Streaming (if supported)
- Metadata/headers
- Error code mapping (gRPC ↔ HTTP)
- Error details format

**Validation Criteria**:
- [ ] All gRPC services tested
- [ ] REST/gRPC parity verified
- [ ] Protobuf message examples captured

**Deliverables**:
- gRPC test suite (mirroring REST tests)
- Protobuf message catalog
- REST/gRPC compatibility matrix

---

## Phase 0 Summary

**Total Test Count**: ~150 compatibility tests

**Key Deliverables**:
1. OpenFGA test harness (Docker-based)
2. Complete REST API test suite with captured responses
3. Complete gRPC API test suite
4. Test data generators
5. Performance baselines
6. Error catalog
7. Limits and edge case documentation

**Usage in Subsequent Phases**:
After each phase (1, 2, 3), run this test suite against RSFGA to ensure:
- 100% compatibility maintained
- Performance meets or exceeds baselines
- No behavioral regressions

**Risk Mitigation**:
- **R-001 (API Compatibility Breaks)**: This test suite is the validation mechanism
- **R-002 (Performance Claims Unvalidated)**: Baselines enable comparison
- **R-003 (Authorization Correctness Bug)**: Validates graph resolution correctness

---

## Phase 1: MVP - OpenFGA Compatible Core

**Goal**: Drop-in replacement for OpenFGA with improved performance

**Success Criteria**:
- ✅ 100% API compatibility (HTTP & gRPC)
- ✅ Pass OpenFGA official test suite
- ✅ Support PostgreSQL and in-memory storage
- ✅ 2x performance improvement on check operations
- ✅ Production-ready observability

**Duration Estimate**: 8-12 weeks

---

### Milestone 1.1: Project Foundation (Week 1-2)

#### Tasks

**1.1.1 Project Setup**
```bash
# Initialize Rust workspace
cargo new rsfga --lib
cd rsfga

# Set up workspace structure
mkdir -p {src/{api,server,domain,storage},tests,benches,proto}

# Initialize git
git init
git remote add upstream https://github.com/openfga/openfga.git
```

**Dependencies** (`Cargo.toml`):
```toml
[workspace]
members = ["rsfga-api", "rsfga-server", "rsfga-storage", "rsfga-domain"]

[dependencies]
# Async runtime
tokio = { version = "1.35", features = ["full"] }
tokio-util = "0.7"

# HTTP server
axum = "0.7"
tower = "0.4"
tower-http = { version = "0.5", features = ["trace", "cors"] }

# gRPC
tonic = "0.11"
prost = "0.12"

# Database
sqlx = { version = "0.7", features = ["runtime-tokio", "postgres", "mysql"] }
deadpool-postgres = "0.13"

# Concurrency
dashmap = "5.5"
parking_lot = "0.12"
arc-swap = "1.6"

# Serialization
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"

# Parsing
nom = "7.1"

# Caching
moka = { version = "0.12", features = ["future"] }

# Observability
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
metrics = "0.22"
metrics-exporter-prometheus = "0.13"
opentelemetry = "0.21"
opentelemetry-jaeger = "0.20"

# Error handling
thiserror = "1.0"
anyhow = "1.0"

# Utilities
uuid = { version = "1.6", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
```

**1.1.2 Project Structure**
```
rsfga/
├── Cargo.toml              # Workspace manifest
├── proto/                  # Protocol buffer definitions
│   └── openfga/
│       └── v1/
│           ├── service.proto
│           └── model.proto
├── rsfga-api/              # HTTP & gRPC API layer
│   ├── src/
│   │   ├── grpc.rs        # gRPC service implementation
│   │   ├── http.rs        # HTTP REST API
│   │   └── middleware.rs  # Auth, logging, metrics
│   └── Cargo.toml
├── rsfga-server/           # Business logic layer
│   ├── src/
│   │   ├── handlers/      # Request handlers
│   │   │   ├── check.rs
│   │   │   ├── batch_check.rs
│   │   │   ├── write.rs
│   │   │   ├── read.rs
│   │   │   └── list_objects.rs
│   │   └── lib.rs
│   └── Cargo.toml
├── rsfga-domain/           # Core domain logic
│   ├── src/
│   │   ├── model/         # Type system & model parser
│   │   ├── resolver/      # Graph resolution engine
│   │   ├── cache/         # Caching layer
│   │   └── lib.rs
│   └── Cargo.toml
├── rsfga-storage/          # Storage abstraction
│   ├── src/
│   │   ├── traits.rs      # DataStore trait
│   │   ├── postgres.rs    # PostgreSQL implementation
│   │   ├── memory.rs      # In-memory implementation
│   │   └── lib.rs
│   └── Cargo.toml
├── tests/                  # Integration tests
│   ├── compatibility/     # OpenFGA compatibility tests
│   └── performance/       # Performance regression tests
├── benches/                # Benchmarks
│   ├── check_bench.rs
│   └── batch_check_bench.rs
└── examples/               # Usage examples
    ├── simple_server.rs
    └── embedded.rs
```

**1.1.3 CI/CD Setup**
- GitHub Actions for CI (test, lint, format)
- Docker build pipeline
- Benchmark comparison vs OpenFGA baseline

**Deliverables**:
- ✅ Rust workspace with proper structure
- ✅ CI/CD pipeline running
- ✅ Proto definitions copied from OpenFGA

---

### Milestone 1.2: Type System & Model Parser (Week 3-4)

#### Objectives
Implement OpenFGA's authorization model parser and type system validator.

#### Tasks

**1.2.1 Protocol Buffer Definitions**

Copy and adapt from OpenFGA:
```proto
syntax = "proto3";

message AuthorizationModel {
  string schema_version = 1;
  map<string, TypeDefinition> type_definitions = 2;
}

message TypeDefinition {
  map<string, Userset> relations = 1;
  map<string, Metadata> metadata = 2;
}

message Userset {
  oneof userset {
    DirectUserset this = 1;
    ComputedUserset computed_userset = 2;
    TupleToUserset tuple_to_userset = 3;
    SetOperation set_operation = 4;
  }
}
```

**1.2.2 DSL Parser** (`rsfga-domain/src/model/parser.rs`)

Implement nom-based parser for OpenFGA DSL:
```rust
// Example DSL:
// model
//   schema 1.1
//
// type document
//   relations
//     define viewer: [user] or editor
//     define editor: [user]
//     define parent: [folder]

use nom::{
    IResult,
    bytes::complete::tag,
    character::complete::{alpha1, alphanumeric1, multispace0},
    multi::separated_list1,
    sequence::{delimited, preceded, tuple},
};

pub fn parse_model(input: &str) -> IResult<&str, AuthorizationModel> {
    // Implementation
}

pub fn parse_type_definition(input: &str) -> IResult<&str, TypeDefinition> {
    // Implementation
}

pub fn parse_relation(input: &str) -> IResult<&str, Relation> {
    // Implementation
}
```

**1.2.3 Type System** (`rsfga-domain/src/model/type_system.rs`)

```rust
use std::collections::HashMap;
use std::sync::Arc;

pub struct TypeSystem {
    model: Arc<AuthorizationModel>,
    type_cache: DashMap<String, Arc<TypeDefinition>>,
}

impl TypeSystem {
    pub fn new(model: AuthorizationModel) -> Self {
        Self {
            model: Arc::new(model),
            type_cache: DashMap::new(),
        }
    }

    pub fn get_type(&self, type_name: &str) -> Result<Arc<TypeDefinition>> {
        // Implementation with caching
    }

    pub fn get_relation(&self, type_name: &str, relation: &str)
        -> Result<Arc<Relation>>
    {
        // Implementation
    }

    pub fn validate_tuple(&self, tuple: &Tuple) -> Result<()> {
        // Validate tuple against type system
    }

    pub fn validate_model(&self) -> Result<()> {
        // Semantic validation:
        // - No cycles in computed usersets
        // - All referenced types exist
        // - TTU references are valid
    }
}
```

**1.2.4 Unit Tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_model() {
        let dsl = r#"
model
  schema 1.1

type user

type document
  relations
    define viewer: [user]
        "#;

        let model = parse_model(dsl).unwrap();
        assert_eq!(model.schema_version, "1.1");
        assert_eq!(model.types.len(), 2);
    }

    #[test]
    fn test_parse_complex_relations() {
        // Union: viewer or editor
        // Intersection: viewer and active
        // Exclusion: all but banned
        // TTU: editor from parent
    }

    #[test]
    fn test_type_system_validation() {
        // Test cycle detection
        // Test missing type references
        // Test invalid TTU
    }
}
```

**Deliverables**:
- ✅ OpenFGA DSL parser (100% feature coverage)
- ✅ Type system with validation
- ✅ 95%+ unit test coverage
- ✅ Compatibility with OpenFGA test models

---

### Milestone 1.3: Storage Layer (Week 5-6)

#### Objectives
Implement storage abstraction and PostgreSQL backend.

#### Tasks

**1.3.1 Storage Trait** (`rsfga-storage/src/traits.rs`)

```rust
use async_trait::async_trait;

#[async_trait]
pub trait DataStore: Send + Sync + 'static {
    // Tuple operations
    async fn read_tuples(&self, store: &str, filter: &TupleFilter)
        -> Result<Vec<Tuple>>;

    async fn write_tuples(
        &self,
        store: &str,
        deletes: Vec<TupleKey>,
        writes: Vec<Tuple>
    ) -> Result<()>;

    // Model operations
    async fn write_model(&self, store: &str, model: &AuthorizationModel)
        -> Result<String>;

    async fn read_model(&self, store: &str, model_id: &str)
        -> Result<AuthorizationModel>;

    async fn read_latest_model(&self, store: &str)
        -> Result<AuthorizationModel>;

    // Store operations
    async fn create_store(&self, id: &str, name: &str) -> Result<()>;
    async fn delete_store(&self, id: &str) -> Result<()>;
    async fn list_stores(&self) -> Result<Vec<Store>>;
}

pub struct TupleFilter {
    pub object_type: Option<String>,
    pub object_id: Option<String>,
    pub relation: Option<String>,
    pub user: Option<String>,
}
```

**1.3.2 PostgreSQL Implementation** (`rsfga-storage/src/postgres.rs`)

**Schema**:
```sql
-- stores table
CREATE TABLE stores (
    id UUID PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- authorization_models table
CREATE TABLE authorization_models (
    id UUID PRIMARY KEY,
    store_id UUID NOT NULL REFERENCES stores(id) ON DELETE CASCADE,
    model JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_models_store_created ON authorization_models(store_id, created_at DESC);

-- tuples table
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
    PRIMARY KEY (store_id, object_type, object_id, relation, user_type, user_id, COALESCE(user_relation, ''))
);

-- Indexes for common query patterns
CREATE INDEX idx_tuples_object ON tuples(store_id, object_type, object_id);
CREATE INDEX idx_tuples_user ON tuples(store_id, user_type, user_id);
CREATE INDEX idx_tuples_userset ON tuples(store_id, user_type, user_id, user_relation)
    WHERE user_relation IS NOT NULL;

-- Changelog for replication
CREATE TABLE changelog (
    id BIGSERIAL PRIMARY KEY,
    store_id UUID NOT NULL,
    operation VARCHAR(10) NOT NULL, -- 'INSERT', 'DELETE'
    tuple JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_changelog_store_created ON changelog(store_id, created_at);
```

**Implementation**:
```rust
pub struct PostgresDataStore {
    pool: Arc<PgPool>,
}

impl PostgresDataStore {
    pub async fn new(database_url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(100)
            .min_connections(10)
            .acquire_timeout(Duration::from_secs(5))
            .connect(database_url)
            .await?;

        Ok(Self { pool: Arc::new(pool) })
    }
}

#[async_trait]
impl DataStore for PostgresDataStore {
    async fn read_tuples(&self, store: &str, filter: &TupleFilter)
        -> Result<Vec<Tuple>>
    {
        let mut query = QueryBuilder::new(
            "SELECT object_type, object_id, relation, user_type, user_id, user_relation
             FROM tuples WHERE store_id = "
        );
        query.push_bind(Uuid::parse_str(store)?);

        if let Some(obj_type) = &filter.object_type {
            query.push(" AND object_type = ").push_bind(obj_type);
        }

        if let Some(obj_id) = &filter.object_id {
            query.push(" AND object_id = ").push_bind(obj_id);
        }

        // ... other filters

        let tuples = query
            .build_query_as::<Tuple>()
            .fetch_all(&*self.pool)
            .await?;

        Ok(tuples)
    }

    async fn write_tuples(
        &self,
        store: &str,
        deletes: Vec<TupleKey>,
        writes: Vec<Tuple>
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        // Delete tuples
        for key in deletes {
            sqlx::query!(
                "DELETE FROM tuples
                 WHERE store_id = $1
                   AND object_type = $2
                   AND object_id = $3
                   AND relation = $4
                   AND user_type = $5
                   AND user_id = $6",
                Uuid::parse_str(store)?,
                key.object_type,
                key.object_id,
                key.relation,
                key.user_type,
                key.user_id
            )
            .execute(&mut *tx)
            .await?;

            // Insert into changelog
            sqlx::query!(
                "INSERT INTO changelog (store_id, operation, tuple)
                 VALUES ($1, 'DELETE', $2)",
                Uuid::parse_str(store)?,
                serde_json::to_value(&key)?
            )
            .execute(&mut *tx)
            .await?;
        }

        // Insert tuples
        for tuple in writes {
            sqlx::query!(
                "INSERT INTO tuples
                 (store_id, object_type, object_id, relation, user_type, user_id, user_relation)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)
                 ON CONFLICT DO NOTHING",
                Uuid::parse_str(store)?,
                tuple.object_type,
                tuple.object_id,
                tuple.relation,
                tuple.user_type,
                tuple.user_id,
                tuple.user_relation
            )
            .execute(&mut *tx)
            .await?;

            // Insert into changelog
            sqlx::query!(
                "INSERT INTO changelog (store_id, operation, tuple)
                 VALUES ($1, 'INSERT', $2)",
                Uuid::parse_str(store)?,
                serde_json::to_value(&tuple)?
            )
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    // ... other methods
}
```

**1.3.3 In-Memory Implementation** (`rsfga-storage/src/memory.rs`)

```rust
pub struct MemoryDataStore {
    stores: DashMap<String, Store>,
    tuples: DashMap<String, Vec<Tuple>>,  // Key: store_id
    models: DashMap<String, Vec<AuthorizationModel>>, // Key: store_id
}

// Implementation using DashMap for thread-safe in-memory storage
```

**1.3.4 Integration Tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use testcontainers::*;

    #[tokio::test]
    async fn test_postgres_write_read() {
        // Start PostgreSQL container
        let container = clients::Cli::default()
            .run(images::postgres::Postgres::default());

        let store = PostgresDataStore::new(&get_connection_string(&container))
            .await
            .unwrap();

        // Test write and read
        store.create_store("test-store", "Test Store").await.unwrap();

        let tuples = vec![
            Tuple {
                object_type: "document".to_string(),
                object_id: "doc1".to_string(),
                relation: "viewer".to_string(),
                user_type: "user".to_string(),
                user_id: "alice".to_string(),
                user_relation: None,
            }
        ];

        store.write_tuples("test-store", vec![], tuples.clone()).await.unwrap();

        let filter = TupleFilter {
            object_type: Some("document".to_string()),
            ..Default::default()
        };

        let result = store.read_tuples("test-store", &filter).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].object_id, "doc1");
    }
}
```

**Deliverables**:
- ✅ Storage trait abstraction
- ✅ PostgreSQL implementation with connection pooling
- ✅ In-memory implementation
- ✅ Migration scripts
- ✅ Integration tests with testcontainers

---

### Milestone 1.4: Graph Resolver (Week 7-8)

#### Objectives
Implement async graph traversal engine for check operations.

#### Tasks

**1.4.1 Graph Resolver Core** (`rsfga-domain/src/resolver/mod.rs`)

```rust
use tokio::sync::Semaphore;
use std::sync::Arc;

pub struct GraphResolver {
    datastore: Arc<dyn DataStore>,
    type_system: Arc<TypeSystem>,
    cache: Arc<CheckCache>,
    config: ResolverConfig,
}

pub struct ResolverConfig {
    pub max_depth: u32,              // Default: 25
    pub max_breadth: u32,            // Default: 10
    pub timeout: Duration,           // Default: 5s
    pub max_concurrent_checks: usize, // Default: 100
}

pub struct CheckContext {
    visited: DashMap<(String, String, String), ()>,
    depth: AtomicU32,
    semaphore: Arc<Semaphore>,
    start_time: Instant,
    timeout: Duration,
}

impl GraphResolver {
    #[instrument(skip(self), fields(store=%store))]
    pub async fn check(
        &self,
        store: &str,
        user: &str,
        relation: &str,
        object: &str,
    ) -> Result<bool> {
        let ctx = CheckContext::new(self.config.clone());

        // Check cache first
        let cache_key = CacheKey::new(store, object, relation, user);
        if let Some(result) = self.cache.get(&cache_key).await {
            return Ok(result);
        }

        // Resolve with context
        let result = self.resolve(store, user, relation, object, &ctx).await?;

        // Cache result
        self.cache.insert(cache_key, result, Duration::from_secs(10)).await;

        Ok(result)
    }

    #[instrument(skip(self, ctx))]
    async fn resolve(
        &self,
        store: &str,
        user: &str,
        relation: &str,
        object: &str,
        ctx: &CheckContext,
    ) -> Result<bool> {
        // Timeout check
        if ctx.start_time.elapsed() > ctx.timeout {
            return Err(Error::Timeout);
        }

        // Depth check
        let depth = ctx.depth.fetch_add(1, Ordering::Relaxed);
        if depth > self.config.max_depth {
            return Err(Error::DepthLimitExceeded);
        }

        // Cycle detection
        let key = (object.to_string(), relation.to_string(), user.to_string());
        if ctx.visited.contains_key(&key) {
            return Ok(false);
        }
        ctx.visited.insert(key.clone(), ());

        // Get relation definition
        let (object_type, object_id) = parse_object(object)?;
        let relation_def = self.type_system.get_relation(object_type, relation)?;

        // Resolve based on relation type
        let result = match relation_def.as_ref() {
            Relation::Direct(allowed_types) => {
                self.resolve_direct(store, user, relation, object, allowed_types, ctx).await?
            }

            Relation::Computed(userset) => {
                self.resolve(store, user, &userset.relation, object, ctx).await?
            }

            Relation::Union(relations) => {
                self.resolve_union(store, user, object, relations, ctx).await?
            }

            Relation::Intersection(relations) => {
                self.resolve_intersection(store, user, object, relations, ctx).await?
            }

            Relation::TupleToUserset(ttu) => {
                self.resolve_ttu(store, user, object, ttu, ctx).await?
            }

            Relation::Exclusion(base, exclude) => {
                let has_base = self.resolve_relation(store, user, object, base, ctx).await?;
                let has_exclude = self.resolve_relation(store, user, object, exclude, ctx).await?;
                has_base && !has_exclude
            }
        };

        ctx.visited.remove(&key);
        ctx.depth.fetch_sub(1, Ordering::Relaxed);

        Ok(result)
    }

    async fn resolve_direct(
        &self,
        store: &str,
        user: &str,
        relation: &str,
        object: &str,
        allowed_types: &[TypeRef],
        ctx: &CheckContext,
    ) -> Result<bool> {
        // Check if direct tuple exists
        let filter = TupleFilter {
            object_type: Some(parse_object(object)?.0.to_string()),
            object_id: Some(parse_object(object)?.1.to_string()),
            relation: Some(relation.to_string()),
            user: Some(user.to_string()),
        };

        let tuples = self.datastore.read_tuples(store, &filter).await?;

        if !tuples.is_empty() {
            return Ok(true);
        }

        // Check userset references (e.g., group:eng#member)
        if let Some((user_obj, user_rel)) = parse_userset(user) {
            // Expand the userset
            let members = self.expand_userset(store, user_obj, user_rel, ctx).await?;
            // Check if any member has direct access
            // ... implementation
        }

        Ok(false)
    }

    async fn resolve_union(
        &self,
        store: &str,
        user: &str,
        object: &str,
        relations: &[Relation],
        ctx: &CheckContext,
    ) -> Result<bool> {
        // Parallel resolution - race to first success
        let futures: Vec<_> = relations.iter()
            .map(|rel| {
                let ctx = ctx.clone();
                async move {
                    self.resolve_relation(store, user, object, rel, &ctx).await
                }
            })
            .collect();

        // Use select_ok to return on first success
        match futures::future::select_ok(futures).await {
            Ok((result, _)) => Ok(result),
            Err(_) => Ok(false), // All failed
        }
    }

    async fn resolve_intersection(
        &self,
        store: &str,
        user: &str,
        object: &str,
        relations: &[Relation],
        ctx: &CheckContext,
    ) -> Result<bool> {
        // Parallel resolution - all must succeed
        let futures: Vec<_> = relations.iter()
            .map(|rel| {
                let ctx = ctx.clone();
                async move {
                    self.resolve_relation(store, user, object, rel, &ctx).await
                }
            })
            .collect();

        let results = futures::future::join_all(futures).await;

        // Check if all succeeded
        Ok(results.into_iter().all(|r| r.unwrap_or(false)))
    }

    async fn resolve_ttu(
        &self,
        store: &str,
        user: &str,
        object: &str,
        ttu: &TupleToUserset,
        ctx: &CheckContext,
    ) -> Result<bool> {
        // Step 1: Find tuples where object has tupleset relation
        // Example: document:doc1#parent -> folder:folder1

        let filter = TupleFilter {
            object_type: Some(parse_object(object)?.0.to_string()),
            object_id: Some(parse_object(object)?.1.to_string()),
            relation: Some(ttu.tupleset.to_string()),
            user: None,
        };

        let tuples = self.datastore.read_tuples(store, &filter).await?;

        // Step 2: For each parent, check if user has computed_userset relation
        let futures: Vec<_> = tuples.iter()
            .map(|t| {
                let parent = format!("{}:{}", t.user_type, t.user_id);
                let ctx = ctx.clone();
                async move {
                    self.resolve(store, user, &ttu.computed_userset, &parent, &ctx).await
                }
            })
            .collect();

        // Race to first success
        match futures::future::select_ok(futures).await {
            Ok((result, _)) => Ok(result),
            Err(_) => Ok(false),
        }
    }
}
```

**1.4.2 Check Cache** (`rsfga-domain/src/cache/mod.rs`)

```rust
use moka::future::Cache;

pub struct CheckCache {
    cache: Cache<CacheKey, bool>,
}

impl CheckCache {
    pub fn new(max_capacity: u64, ttl: Duration) -> Self {
        let cache = Cache::builder()
            .max_capacity(max_capacity)
            .time_to_live(ttl)
            .build();

        Self { cache }
    }

    pub async fn get(&self, key: &CacheKey) -> Option<bool> {
        self.cache.get(key).await
    }

    pub async fn insert(&self, key: CacheKey, value: bool, ttl: Duration) {
        self.cache.insert(key, value).await;
    }

    pub async fn invalidate(&self, key: &CacheKey) {
        self.cache.invalidate(key).await;
    }

    pub async fn invalidate_pattern(&self, pattern: &str) {
        // Invalidate all keys matching pattern
        // This is expensive, used only on writes
    }
}

#[derive(Hash, Eq, PartialEq, Clone)]
pub struct CacheKey {
    store: String,
    object: String,
    relation: String,
    user: String,
}
```

**1.4.3 Unit Tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_direct_check() {
        // user:alice has document:doc1#viewer
    }

    #[tokio::test]
    async fn test_computed_userset() {
        // viewer := editor
    }

    #[tokio::test]
    async fn test_union() {
        // viewer: editor or owner
    }

    #[tokio::test]
    async fn test_ttu() {
        // document#viewer: editor from parent
    }

    #[tokio::test]
    async fn test_cycle_detection() {
        // Ensure cycles don't cause infinite loops
    }

    #[tokio::test]
    async fn test_depth_limit() {
        // Ensure depth limit is enforced
    }
}
```

**Deliverables**:
- ✅ Async graph resolver with all relation types
- ✅ Check cache with TTL
- ✅ Cycle detection and depth limiting
- ✅ 90%+ unit test coverage

---

### Milestone 1.5: Batch Check Handler (Week 9)

#### Objectives
Implement high-performance batch check with deduplication and parallel execution.

#### Tasks

**1.5.1 Batch Check Implementation** (`rsfga-server/src/handlers/batch_check.rs`)

```rust
use tokio::sync::Semaphore;
use dashmap::DashMap;

pub struct BatchCheckHandler {
    resolver: Arc<GraphResolver>,
    config: BatchConfig,
    // Singleflight for cross-request deduplication
    inflight: Arc<DashMap<CacheKey, Arc<Semaphore>>>,
}

pub struct BatchConfig {
    pub max_checks_per_batch: usize,  // Default: 100
    pub max_parallel_checks: usize,    // Default: 50
}

impl BatchCheckHandler {
    #[instrument(skip(self))]
    pub async fn batch_check(
        &self,
        store: &str,
        checks: Vec<CheckRequest>,
    ) -> Result<Vec<CheckResponse>> {
        // Validate batch size
        if checks.len() > self.config.max_checks_per_batch {
            return Err(Error::BatchTooLarge);
        }

        // Stage 1: Intra-batch deduplication
        let (unique_checks, dedup_map) = self.deduplicate_checks(&checks);

        // Stage 2: Cross-request deduplication (singleflight)
        let results = self.execute_with_singleflight(store, unique_checks).await?;

        // Stage 3: Expand results back to original requests
        let expanded = self.expand_results(&results, &dedup_map);

        Ok(expanded)
    }

    fn deduplicate_checks(
        &self,
        checks: &[CheckRequest],
    ) -> (Vec<CheckRequest>, HashMap<usize, usize>) {
        let mut unique = Vec::new();
        let mut seen = HashMap::new();
        let mut dedup_map = HashMap::new();

        for (idx, check) in checks.iter().enumerate() {
            let key = (
                check.object.clone(),
                check.relation.clone(),
                check.user.clone(),
            );

            if let Some(&unique_idx) = seen.get(&key) {
                // Duplicate found
                dedup_map.insert(idx, unique_idx);
            } else {
                // New unique check
                let unique_idx = unique.len();
                unique.push(check.clone());
                seen.insert(key, unique_idx);
                dedup_map.insert(idx, unique_idx);
            }
        }

        (unique, dedup_map)
    }

    async fn execute_with_singleflight(
        &self,
        store: &str,
        checks: Vec<CheckRequest>,
    ) -> Result<Vec<bool>> {
        let futures: Vec<_> = checks.into_iter()
            .map(|check| {
                let cache_key = CacheKey::new(
                    store,
                    &check.object,
                    &check.relation,
                    &check.user,
                );

                async move {
                    // Check if request is already in-flight
                    let permit = self.inflight
                        .entry(cache_key.clone())
                        .or_insert_with(|| Arc::new(Semaphore::new(1)))
                        .clone();

                    let _guard = permit.acquire().await;

                    // Check cache again (may have been populated while waiting)
                    if let Some(result) = self.resolver.cache.get(&cache_key).await {
                        self.inflight.remove(&cache_key);
                        return Ok(result);
                    }

                    // Execute check
                    let result = self.resolver.check(
                        store,
                        &check.user,
                        &check.relation,
                        &check.object,
                    ).await?;

                    self.inflight.remove(&cache_key);
                    Ok(result)
                }
            })
            .collect();

        // Execute in parallel with semaphore limiting
        let semaphore = Arc::new(Semaphore::new(self.config.max_parallel_checks));

        let results = futures::stream::iter(futures)
            .map(|fut| {
                let sem = semaphore.clone();
                async move {
                    let _permit = sem.acquire().await;
                    fut.await
                }
            })
            .buffer_unordered(self.config.max_parallel_checks)
            .collect::<Vec<_>>()
            .await;

        results.into_iter().collect()
    }

    fn expand_results(
        &self,
        results: &[bool],
        dedup_map: &HashMap<usize, usize>,
    ) -> Vec<CheckResponse> {
        (0..dedup_map.len())
            .map(|idx| {
                let unique_idx = dedup_map[&idx];
                CheckResponse {
                    allowed: results[unique_idx],
                }
            })
            .collect()
    }
}
```

**Deliverables**:
- ✅ Batch check handler with deduplication
- ✅ Singleflight for cross-request deduplication
- ✅ Parallel execution with semaphore limiting
- ✅ Benchmark showing >20x improvement over baseline

---

### Milestone 1.6: API Layer (Week 10)

#### Objectives
Implement HTTP and gRPC APIs with full OpenFGA compatibility.

#### Tasks

**1.6.1 gRPC Service** (`rsfga-api/src/grpc.rs`)

```rust
use tonic::{Request, Response, Status};
use openfga::v1::{
    open_fga_service_server::{OpenFgaService, OpenFgaServiceServer},
    CheckRequest, CheckResponse,
    BatchCheckRequest, BatchCheckResponse,
    WriteRequest, WriteResponse,
    ReadRequest, ReadResponse,
};

pub struct OpenFgaServiceImpl {
    check_handler: Arc<CheckHandler>,
    batch_check_handler: Arc<BatchCheckHandler>,
    write_handler: Arc<WriteHandler>,
    read_handler: Arc<ReadHandler>,
}

#[tonic::async_trait]
impl OpenFgaService for OpenFgaServiceImpl {
    async fn check(
        &self,
        request: Request<CheckRequest>,
    ) -> Result<Response<CheckResponse>, Status> {
        let req = request.into_inner();

        let result = self.check_handler.check(
            &req.store_id,
            &req.tuple_key.user,
            &req.tuple_key.relation,
            &req.tuple_key.object,
        )
        .await
        .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(CheckResponse {
            allowed: result,
            resolution: "".to_string(),
        }))
    }

    async fn batch_check(
        &self,
        request: Request<BatchCheckRequest>,
    ) -> Result<Response<BatchCheckResponse>, Status> {
        // Implementation
    }

    async fn write(
        &self,
        request: Request<WriteRequest>,
    ) -> Result<Response<WriteResponse>, Status> {
        // Implementation
    }

    async fn read(
        &self,
        request: Request<ReadRequest>,
    ) -> Result<Response<ReadResponse>, Status> {
        // Implementation
    }

    // ... other methods
}
```

**1.6.2 HTTP REST API** (`rsfga-api/src/http.rs`)

```rust
use axum::{
    Router,
    routing::{get, post},
    extract::{Path, State},
    Json,
};

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/stores/:store_id/check", post(check_handler))
        .route("/stores/:store_id/batch-check", post(batch_check_handler))
        .route("/stores/:store_id/write", post(write_handler))
        .route("/stores/:store_id/read", post(read_handler))
        .route("/stores/:store_id/expand", post(expand_handler))
        .route("/stores/:store_id/list-objects", post(list_objects_handler))
        .route("/stores", get(list_stores_handler).post(create_store_handler))
        .route("/stores/:store_id", get(get_store_handler).delete(delete_store_handler))
        .route("/stores/:store_id/authorization-models", post(write_model_handler))
        .route("/stores/:store_id/authorization-models/:model_id", get(read_model_handler))
        .with_state(state)
        .layer(/* middleware */)
}

async fn check_handler(
    State(state): State<AppState>,
    Path(store_id): Path<String>,
    Json(payload): Json<CheckRequest>,
) -> Result<Json<CheckResponse>, AppError> {
    let result = state.check_handler.check(
        &store_id,
        &payload.tuple_key.user,
        &payload.tuple_key.relation,
        &payload.tuple_key.object,
    ).await?;

    Ok(Json(CheckResponse { allowed: result }))
}
```

**1.6.3 Middleware** (`rsfga-api/src/middleware.rs`)

```rust
use tower_http::{
    trace::TraceLayer,
    cors::CorsLayer,
    compression::CompressionLayer,
};
use axum::middleware::from_fn;

pub fn create_middleware_stack() -> Router {
    Router::new()
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .layer(CompressionLayer::new())
        .layer(from_fn(auth_middleware))
        .layer(from_fn(metrics_middleware))
}

async fn auth_middleware(
    req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    // Extract and validate bearer token
    let auth_header = req.headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok());

    if let Some(token) = auth_header {
        // Validate token
        // ...
    }

    Ok(next.run(req).await)
}

async fn metrics_middleware(
    req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let start = Instant::now();
    let method = req.method().clone();
    let uri = req.uri().clone();

    let response = next.run(req).await;

    let duration = start.elapsed();
    let status = response.status();

    metrics::histogram!(
        "http_request_duration_seconds",
        duration.as_secs_f64(),
        "method" => method.to_string(),
        "path" => uri.path().to_string(),
        "status" => status.as_u16().to_string(),
    );

    Ok(response)
}
```

**Deliverables**:
- ✅ gRPC service with all OpenFGA endpoints
- ✅ HTTP REST API with all endpoints
- ✅ Middleware (auth, logging, metrics, tracing)
- ✅ OpenAPI/Swagger documentation

---

### Milestone 1.7: Testing & Benchmarking (Week 11-12)

#### Objectives
Comprehensive testing and performance validation.

#### Tasks

**1.7.1 Compatibility Tests** (`tests/compatibility/`)

```rust
// Import OpenFGA test suite and run against RSFGA
#[tokio::test]
async fn test_openfga_compatibility() {
    // Run official OpenFGA test cases
}
```

**1.7.2 Load Tests** (`benches/`)

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_check(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let resolver = setup_resolver();

    c.bench_function("check_simple", |b| {
        b.to_async(&rt).iter(|| async {
            resolver.check(
                "store-1",
                "user:alice",
                "viewer",
                "document:doc1",
            ).await
        });
    });
}

fn bench_batch_check(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handler = setup_batch_handler();

    c.bench_function("batch_check_100", |b| {
        b.to_async(&rt).iter(|| async {
            let checks = create_test_checks(100);
            handler.batch_check("store-1", checks).await
        });
    });
}

criterion_group!(benches, bench_check, bench_batch_check);
criterion_main!(benches);
```

**1.7.3 K6 Load Tests** (matching OpenFGA benchmark setup)

```javascript
// check_load_test.js
import http from 'k6/http';
import { check } from 'k6';

export let options = {
  stages: [
    { duration: '1m', target: 10 },
    { duration: '2m', target: 100 },
    { duration: '1m', target: 0 },
  ],
};

export default function() {
  let payload = JSON.stringify({
    tuple_key: {
      user: 'user:alice',
      relation: 'viewer',
      object: 'document:doc1',
    },
  });

  let res = http.post('http://localhost:8080/stores/test-store/check', payload, {
    headers: { 'Content-Type': 'application/json' },
  });

  check(res, {
    'status is 200': (r) => r.status === 200,
    'latency < 50ms': (r) => r.timings.duration < 50,
  });
}
```

**1.7.4 Performance Comparison**

Create benchmark report comparing RSFGA vs OpenFGA:

| Metric | OpenFGA | RSFGA | Improvement |
|--------|---------|-------|-------------|
| Check RPS | 483 | ? | ?x |
| Batch RPS | 23 | ? | ?x |
| Write RPS | 59 | ? | ?x |
| p95 latency | 22ms | ? | ? |

**Deliverables**:
- ✅ 100% passing compatibility tests
- ✅ Load test suite (k6)
- ✅ Benchmark comparison report
- ✅ Performance regression detection in CI

---

## Phase 2: Precomputed Check (Milestone 2)

**Goal**: Sub-millisecond check operations using precomputed results

**Duration Estimate**: 6-8 weeks

### Key Components

1. **Change Classification Engine**
   - Classify tuple changes by impact type
   - Identify affected check queries

2. **Impact Analysis**
   - Reverse expansion to find affected objects/users
   - Forward expansion for group membership changes

3. **Precomputation Worker**
   - Async worker pool for background computation
   - Batch processing of affected checks

4. **Valkey Integration**
   - Result storage with TTL
   - Efficient key-value lookup
   - Cluster-wide cache sharing

5. **Hybrid Resolution**
   - Try precomputed cache first
   - Fallback to graph resolution on miss
   - Progressive cache warming

---

## Phase 3: Distributed Edge (Milestone 3)

**Goal**: Global deployment with <10ms latency

**Duration Estimate**: 10-12 weeks

### Key Components

1. **Product Registry**
   - Product configuration management
   - Replication rules
   - Data tier classification

2. **Edge Sync Agent**
   - Kafka consumer for change events
   - Selective replication based on product config
   - Watermark tracking

3. **Edge Storage**
   - Embedded RocksDB/sled
   - Product-partitioned data
   - Snapshot/restore for bootstrap

4. **Request Router**
   - Product-aware routing
   - Fallback to regional on cache miss
   - Latency-based selection

5. **Multi-Region Replication**
   - PostgreSQL logical replication
   - Kafka-based CDC
   - Conflict resolution

---

## Release Strategy

### Alpha Release (End of Phase 1)
- **Audience**: Early adopters, internal testing
- **Features**: Core API compatibility, single-node deployment
- **Distribution**: Docker image, source code
- **Documentation**: API reference, quick start guide

### Beta Release (End of Phase 2)
- **Audience**: Production pilots
- **Features**: Precomputation, improved performance
- **Distribution**: Docker image, Kubernetes Helm chart
- **Documentation**: Performance tuning guide, migration guide

### v1.0 Release (End of Phase 3)
- **Audience**: General availability
- **Features**: Full distributed edge architecture
- **Distribution**: Multi-arch Docker images, Helm chart, binaries
- **Documentation**: Complete documentation, runbooks, best practices

---

## Risk Mitigation

### Technical Risks

| Risk | Mitigation |
|------|------------|
| Performance targets not met | Continuous benchmarking, profiling early |
| API compatibility issues | Automated compatibility test suite |
| Distributed consistency bugs | Extensive chaos testing, formal verification |
| Storage backend bugs | Comprehensive integration tests per backend |

### Operational Risks

| Risk | Mitigation |
|------|------------|
| Migration complexity | Detailed migration guide, tooling support |
| Learning curve | Extensive documentation, examples |
| Production incidents | Gradual rollout, shadow mode testing |

---

## Success Criteria

- ✅ **MVP Phase**: 100% OpenFGA API compatibility, 2x check performance
- ✅ **Phase 2**: 20x batch-check performance, <1ms precomputed checks
- ✅ **Phase 3**: <10ms global latency, 100K+ req/s cluster throughput
- ✅ **Adoption**: 10+ production deployments, 1000+ GitHub stars
- ✅ **Community**: Active contributors, plugin ecosystem

---

## Next Steps

After reviewing this roadmap:

1. **Set up development environment**
   ```bash
   # Install Rust toolchain
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

   # Install additional tools
   cargo install cargo-watch cargo-criterion sqlx-cli

   # Clone OpenFGA for reference
   git clone https://github.com/openfga/openfga.git
   ```

2. **Start with Milestone 1.1**: Project foundation and structure

3. **Weekly progress reviews**: Track against roadmap, adjust as needed

4. **Continuous benchmarking**: Compare against OpenFGA baseline throughout development

This roadmap provides a clear path from research to production-ready implementation, with concrete milestones and deliverables at each stage.
