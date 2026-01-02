# RSFGA - Rust OpenFGA Implementation Architecture

## Executive Summary

RSFGA is a high-performance Rust implementation of OpenFGA, an authorization/permission engine inspired by Google Zanzibar. This implementation focuses on:

- **Correctness**: Full API compatibility with OpenFGA
- **Performance**: 2-5x improvement in check/batch operations
- **Scalability**: Support for distributed edge deployments
- **Efficiency**: Lower memory footprint and zero-GC latency

### Performance Goals

| Operation    | OpenFGA Baseline | RSFGA Target | Strategy |
|--------------|------------------|--------------|----------|
| Check        | 483 req/s        | 1000+ req/s  | Async graph traversal, lock-free caching |
| Batch-Check  | 23 checks/s      | 500+ checks/s| Parallel execution, cross-request dedup |
| Write        | 59 req/s         | 150+ req/s   | Batch processing, async invalidation |
| List-Objects | 52 req/s         | 100+ req/s   | Optimized query planning |

**Note**: All performance targets require validation through benchmarking.

## Three-Phase Implementation Strategy

### Phase 1: MVP - Compatible Core (Milestone 1)
- Full OpenFGA API compatibility (HTTP/gRPC)
- Storage backends: PostgreSQL, MySQL, in-memory
- Single-node deployment
- Focus: Correctness and feature parity

### Phase 2: Precomputed Checks (Milestone 2)
- Precomputation engine for check results
- Valkey/Redis integration for result cache
- Incremental invalidation on writes
- Focus: Check operation performance

### Phase 3: Distributed Edge (Milestone 3)
- Edge node architecture
- Product-based data partitioning
- Multi-region replication
- Focus: Global scalability and latency

---

## Core Architecture (MVP Phase)

### System Layers

```
┌─────────────────────────────────────────────────────────┐
│                    API Layer                             │
│  ┌─────────────┐  ┌──────────────┐  ┌───────────────┐  │
│  │  gRPC API   │  │  HTTP/REST   │  │  Middleware   │  │
│  │  (Tonic)    │  │  (Axum)      │  │  (Auth/Logs)  │  │
│  └─────────────┘  └──────────────┘  └───────────────┘  │
└─────────────────────────────────────────────────────────┘
                          ↓
┌─────────────────────────────────────────────────────────┐
│                   Server Layer                           │
│  ┌──────────┐ ┌──────────┐ ┌────────┐ ┌──────────────┐ │
│  │  Check   │ │  Batch   │ │ Write  │ │ List Objects │ │
│  │ Handler  │ │ Handler  │ │Handler │ │   Handler    │ │
│  └──────────┘ └──────────┘ └────────┘ └──────────────┘ │
└─────────────────────────────────────────────────────────┘
                          ↓
┌─────────────────────────────────────────────────────────┐
│                  Domain Layer                            │
│  ┌──────────────┐  ┌────────────┐  ┌────────────────┐  │
│  │   Graph      │  │   Type     │  │    Query       │  │
│  │  Resolver    │  │   System   │  │  Optimizer     │  │
│  │  (Async)     │  │  (Parser)  │  │                │  │
│  └──────────────┘  └────────────┘  └────────────────┘  │
└─────────────────────────────────────────────────────────┘
                          ↓
┌─────────────────────────────────────────────────────────┐
│               Storage Abstraction Layer                  │
│  ┌──────────────┐  ┌────────────┐  ┌────────────────┐  │
│  │   Cached     │  │  Bounded   │  │   Connection   │  │
│  │  Datastore   │  │   Reader   │  │     Pool       │  │
│  │ (Arc/DashMap)│  │            │  │                │  │
│  └──────────────┘  └────────────┘  └────────────────┘  │
└─────────────────────────────────────────────────────────┘
                          ↓
┌─────────────────────────────────────────────────────────┐
│               Storage Backend Layer                      │
│  ┌──────────┐  ┌──────────┐  ┌───────────┐            │
│  │PostgreSQL│  │  MySQL   │  │ In-Memory │            │
│  │(SQLx)    │  │ (SQLx)   │  │(DashMap)  │            │
│  └──────────┘  └──────────┘  └───────────┘            │
└─────────────────────────────────────────────────────────┘
```

### Key Design Decisions

#### 1. Asynchronous Graph Traversal

**Problem**: OpenFGA's synchronous graph resolution limits parallelism.

**Solution**: Tokio-based async graph walker with concurrent subtree exploration.

```rust
// Conceptual design (not final implementation)
async fn resolve_check(
    user: &str,
    relation: &str,
    object: &str,
    ctx: &CheckContext
) -> Result<bool> {
    // Parallel exploration of multiple resolution paths
    let futures: Vec<_> = resolution_paths
        .into_iter()
        .map(|path| async move {
            explore_path(path, ctx).await
        })
        .collect();

    // Race to first success or collect all failures
    tokio::select! {
        result = race_to_success(futures) => result,
        _ = ctx.timeout => Err(Timeout)
    }
}
```

**Benefits**:
- Exploits I/O parallelism during database queries
- Early termination on first successful path
- Better CPU utilization on multi-core systems

#### 2. Lock-Free Caching

**Problem**: OpenFGA uses mutex-based caching, creating contention.

**Solution**: `DashMap` for concurrent reads/writes without global locks.

```rust
struct CheckCache {
    // Sharded concurrent hashmap
    cache: Arc<DashMap<CacheKey, CheckResult>>,
    // LRU eviction via tokio background task
    evictor: tokio::task::JoinHandle<()>,
}
```

**Benefits**:
- 10-100x faster cache access under contention
- No mutex blocking during high-concurrency bursts
- Predictable latency (no GC pauses)

#### 3. Batch Check Parallelism with Deduplication

**Problem**: OpenFGA processes batch checks with synchronous blocking and no cross-request deduplication.

**Solution**: Multi-stage parallel pipeline:

```
Request → Deduplicate → Partition → Parallel Execute → Merge
```

**Implementation Strategy**:

```rust
async fn batch_check(
    checks: Vec<CheckRequest>,
    ctx: &Context
) -> Vec<CheckResult> {
    // Stage 1: Intra-batch dedup
    let (unique_checks, dedup_map) = deduplicate_checks(&checks);

    // Stage 2: Cross-request dedup via async-singleflight
    let cached_results = check_cache(&unique_checks).await;

    // Stage 3: Partition uncached by resolution complexity
    let (simple, complex) = partition_by_complexity(uncached);

    // Stage 4: Execute in parallel pools
    let simple_results = tokio::spawn(execute_simple_pool(simple));
    let complex_results = tokio::spawn(execute_complex_pool(complex));

    // Stage 5: Merge and expand deduplicated results
    merge_results(
        cached_results,
        simple_results.await?,
        complex_results.await?,
        dedup_map
    )
}
```

**Benefits**:
- 20-50x improvement for batch operations
- Streaming results (gRPC) for partial responses
- Cross-request deduplication eliminates redundant work

#### 4. Streaming Write Processing

**Problem**: OpenFGA processes writes sequentially with blocking cache invalidation.

**Solution**: Async write pipeline with background invalidation.

```rust
async fn write_tuples(
    tuples: Vec<Tuple>,
    deletes: Vec<TupleKey>
) -> Result<()> {
    // Stage 1: Validate and batch
    let batch = validate_and_batch(tuples, deletes)?;

    // Stage 2: Write to storage (batched SQL)
    let write_future = storage.batch_write(batch);

    // Stage 3: Async cache invalidation (non-blocking)
    let invalidation = tokio::spawn(async move {
        invalidate_cache_entries(batch).await
    });

    // Wait only for write, not invalidation
    write_future.await?;

    // Invalidation happens in background
    Ok(())
}
```

**Benefits**:
- 2-3x write throughput
- Lower p99 latency (no blocking on cache invalidation)
- Eventual consistency acceptable for authorization

### Storage Backend Interface

```rust
#[async_trait]
pub trait DataStore: Send + Sync {
    // Read operations
    async fn read(&self, store: &str, key: &TupleKey)
        -> Result<Option<Tuple>>;

    async fn read_page(&self, store: &str, opts: &ReadPageOptions)
        -> Result<(Vec<Tuple>, Option<ContinuationToken>)>;

    async fn read_user_tuples(&self, store: &str, filter: &TupleFilter)
        -> Result<Vec<Tuple>>;

    async fn read_userset_tuples(&self, store: &str, filter: &TupleFilter)
        -> Result<Vec<Tuple>>;

    // Write operations
    async fn write(&self, store: &str, deletes: Vec<TupleKey>, writes: Vec<Tuple>)
        -> Result<()>;

    // Store management
    async fn create_store(&self, store: &str) -> Result<()>;
    async fn delete_store(&self, store: &str) -> Result<()>;
    async fn list_stores(&self, opts: &ListOptions)
        -> Result<Vec<String>>;

    // Model operations
    async fn write_authorization_model(&self, store: &str, model: &Model)
        -> Result<String>; // Returns model ID

    async fn read_authorization_model(&self, store: &str, id: &str)
        -> Result<Model>;

    async fn read_latest_authorization_model(&self, store: &str)
        -> Result<Model>;
}
```

### Type System and Model Parser

**Approach**: Use `nom` parser combinator for OpenFGA DSL.

```rust
pub struct AuthorizationModel {
    pub schema_version: String,
    pub types: HashMap<String, TypeDefinition>,
}

pub struct TypeDefinition {
    pub relations: HashMap<String, Relation>,
}

pub enum Relation {
    Direct(Vec<TypeRef>),              // define viewer: [user]
    Computed(ComputedUserset),         // define viewer: editor
    Union(Vec<Relation>),              // define viewer: editor or owner
    Intersection(Vec<Relation>),       // define can_view: viewer and active
    Exclusion(Box<Relation>, Box<Relation>), // define viewer: all but banned
    TupleToUserset(TTU),               // define viewer: editor from parent
}
```

### Graph Resolution Algorithm

**Core Logic**: Depth-first search with cycle detection and breadth/depth limits.

```rust
struct GraphResolver {
    datastore: Arc<dyn DataStore>,
    cache: Arc<CheckCache>,
    type_system: Arc<TypeSystem>,
}

impl GraphResolver {
    async fn resolve(
        &self,
        user: &str,
        relation: &str,
        object: &str,
        visited: &mut HashSet<(String, String, String)>,
        depth: u32,
    ) -> Result<bool> {
        // Depth limit check
        if depth > MAX_DEPTH {
            return Err(Error::DepthLimitExceeded);
        }

        // Cycle detection
        let key = (user.to_string(), relation.to_string(), object.to_string());
        if !visited.insert(key.clone()) {
            return Ok(false); // Cycle detected
        }

        // Check cache
        if let Some(result) = self.cache.get(&key) {
            return Ok(result);
        }

        // Resolve based on relation type
        let relation_def = self.type_system.get_relation(object, relation)?;

        let result = match relation_def {
            Relation::Direct(types) => {
                // Check if direct tuple exists
                self.check_direct_tuple(user, relation, object).await?
            }

            Relation::Computed(userset) => {
                // Resolve computed userset
                self.resolve(user, &userset.relation, object, visited, depth + 1).await?
            }

            Relation::Union(relations) => {
                // Parallel check all union branches
                let futures: Vec<_> = relations.iter()
                    .map(|r| self.resolve_relation(user, r, object, visited, depth + 1))
                    .collect();

                // Race to first success
                futures::future::select_ok(futures).await.is_ok()
            }

            Relation::TupleToUserset(ttu) => {
                // Two-step resolution
                self.resolve_ttu(user, ttu, object, visited, depth + 1).await?
            }

            // ... other relation types
        };

        // Cache result
        self.cache.insert(key, result);

        Ok(result)
    }
}
```

---

## Technology Stack

### Core Dependencies

| Category | Library | Rationale |
|----------|---------|-----------|
| **Async Runtime** | `tokio` | Industry standard, excellent ecosystem |
| **HTTP Server** | `axum` | Fast, ergonomic, built on tower/hyper |
| **gRPC** | `tonic` | Pure Rust gRPC with excellent performance |
| **Database** | `sqlx` | Async SQL with compile-time query checking |
| **Concurrency** | `dashmap` | Lock-free concurrent hashmap |
| **Serialization** | `serde` + `prost` | JSON/Protobuf support |
| **Parsing** | `nom` | Parser combinator for DSL |
| **Caching** | `moka` | High-performance async cache with TTL |
| **Metrics** | `metrics` + `prometheus` | Observability |
| **Tracing** | `tracing` + `opentelemetry` | Distributed tracing |

### Storage Backends

- **PostgreSQL**: `sqlx` with connection pooling via `deadpool`
- **MySQL**: `sqlx` with connection pooling
- **In-Memory**: `DashMap` with optional persistence (snapshot to disk)
- **Future**: SQLite, Valkey/Redis

---

## Performance Optimization Strategies

### 1. Query Optimization

**Problem**: Graph traversal generates many small database queries.

**Solutions**:
- **Query batching**: Combine multiple tuple lookups into single query
- **Prefetching**: Eagerly load related tuples based on model structure
- **Query plan caching**: Cache SQL query plans for common patterns

### 2. Cache Hierarchy

**L1 Cache**: In-memory DashMap (sub-microsecond access)
- Check results with 10-second TTL
- Authorization models (infinite TTL, invalidate on update)
- Type definitions

**L2 Cache** (Future - Phase 2): Valkey/Redis
- Precomputed check results
- Shared across cluster nodes
- 60-second TTL

### 3. Connection Pooling

**Configuration**:
```rust
pub struct PoolConfig {
    pub min_connections: u32,      // 10
    pub max_connections: u32,      // 100
    pub acquire_timeout: Duration, // 5s
    pub idle_timeout: Duration,    // 10m
    pub max_lifetime: Duration,    // 30m
}
```

### 4. Memory Management

**Strategies**:
- Use `Arc` for shared immutable data (models, type system)
- Pool frequently allocated objects (CheckRequest, Tuple)
- Stream large result sets instead of loading into memory
- Bounded queues to prevent memory exhaustion

---

## Phase 2: Precomputation Engine

### Architecture

```
┌───────────────────────────────────────────────────┐
│              Write Operation                       │
└───────────────┬───────────────────────────────────┘
                ↓
┌───────────────────────────────────────────────────┐
│         Change Classification                      │
│  ┌──────────┐ ┌──────────┐ ┌──────────────────┐  │
│  │  Direct  │ │ Computed │ │ Tuple-to-Userset │  │
│  └──────────┘ └──────────┘ └──────────────────┘  │
└───────────────┬───────────────────────────────────┘
                ↓
┌───────────────────────────────────────────────────┐
│       Impact Analysis (Reverse Expansion)          │
│   - Find affected objects                          │
│   - Find affected users                            │
│   - Generate (object, relation, user) tuples       │
└───────────────┬───────────────────────────────────┘
                ↓
┌───────────────────────────────────────────────────┐
│         Parallel Recomputation                     │
│   - Batch check affected tuples                    │
│   - Generate check results                         │
└───────────────┬───────────────────────────────────┘
                ↓
┌───────────────────────────────────────────────────┐
│       Result Distribution (Valkey)                 │
│   - Store in Redis with TTL                        │
│   - Publish to edge nodes (Phase 3)                │
└───────────────────────────────────────────────────┘
```

### Change Classification

```rust
enum ChangeType {
    DirectAssignment {
        // user:alice → document:doc1#viewer
        subject: String,
        relation: String,
        object: String,
    },

    ComputedUsersetSource {
        // Changes to editor affect viewer (if viewer := editor)
        source_relation: String,
        computed_relations: Vec<String>,
    },

    GroupMembership {
        // user:alice → group:eng#member
        user: String,
        group: String,
    },

    TupleToUserset {
        // document:doc1#parent → folder:folder1
        child: String,
        parent: String,
        ttu_relation: String,
    },
}
```

### Impact Analysis

**Reverse Expansion Algorithm**:

```rust
async fn find_affected_checks(
    change: &TupleChange,
    model: &AuthorizationModel
) -> Result<Vec<(String, String, String)>> {
    match classify_change(change)? {
        ChangeType::DirectAssignment { subject, relation, object } => {
            // Simple case: only this exact check is affected
            vec![(object, relation, subject)]
        }

        ChangeType::GroupMembership { user, group } => {
            // Find all objects that grant access via this group
            // Example: If group is "team:eng#member", find all:
            //   - document:*#viewer@team:eng#member

            let objects_with_group = datastore
                .find_tuples_by_subject(&group)
                .await?;

            // For each object, generate check for the user
            objects_with_group
                .into_iter()
                .map(|t| (t.object, t.relation, user.clone()))
                .collect()
        }

        ChangeType::TupleToUserset { child, parent, ttu_relation } => {
            // Find all users with access to parent
            let parent_users = expand_relation(&parent, ttu_relation).await?;

            // Find all relations on child that inherit from parent
            let child_relations = model.get_ttu_relations(&child, &parent)?;

            // Cartesian product
            parent_users.iter()
                .flat_map(|user| {
                    child_relations.iter()
                        .map(move |rel| (child.clone(), rel.clone(), user.clone()))
                })
                .collect()
        }

        // ... other change types
    }
}
```

### Precomputation Storage (Valkey/Redis)

**Key Format**:
```
check:{store_id}:{object_type}:{object_id}#{relation}@{user}
```

**Value Format**:
```json
{
  "allowed": true,
  "computed_at": "2024-01-15T10:30:00Z",
  "ttl": 60
}
```

**Benefits**:
- Check operations become O(1) Redis lookups
- Shared cache across cluster nodes
- TTL-based expiration handles stale data

---

## Phase 3: Distributed Edge Architecture

### Node Types

**Edge Nodes**:
- Embedded storage (RocksDB/sled)
- Product-based data partitioning
- Check/Read operations only
- Sync from regional via Kafka

**Regional Nodes**:
- Full PostgreSQL replica
- Valkey cache cluster
- Check/Read/Write operations
- Sync from central via CDC

**Central Nodes**:
- Primary PostgreSQL cluster
- All operations
- Schema registry
- Kafka event stream

### Product-Based Partitioning

```rust
pub struct ProductConfig {
    pub product_id: String,
    pub tier: DataTier,
    pub primary_regions: Vec<Region>,
    pub secondary_regions: Vec<Region>,
    pub replication_rules: ReplicationRules,
}

pub enum DataTier {
    Critical,   // Replicate everywhere (models, metadata)
    Hot,        // Replicate to primary/secondary regions
    Warm,       // Cache-only, fetch on demand
    Cold,       // Never replicate to edge (audit logs, compliance)
}

pub struct ReplicationRules {
    pub object_types: Vec<String>,     // Which types to replicate
    pub min_access_frequency: u32,      // Minimum accesses to replicate
    pub access_recency: Duration,       // Recent access window
}
```

### Edge Sync Agent

```rust
struct EdgeSyncAgent {
    kafka_consumer: StreamConsumer,
    local_store: Arc<dyn DataStore>,
    product_registry: Arc<ProductRegistry>,
}

impl EdgeSyncAgent {
    async fn run(&self) {
        loop {
            // Consume change events from Kafka
            let message = self.kafka_consumer.recv().await?;

            // Deserialize change event
            let change: TupleChange = deserialize(&message.payload())?;

            // Check if this change is relevant to our edge
            if !self.should_sync(&change).await? {
                continue;
            }

            // Apply change to local store
            self.local_store.write(
                &change.store_id,
                change.deletes,
                change.writes
            ).await?;

            // Update watermark
            self.update_watermark(message.offset()).await?;
        }
    }

    async fn should_sync(&self, change: &TupleChange) -> Result<bool> {
        // Get product configuration
        let product = self.product_registry
            .get_product(&change.store_id)
            .await?;

        // Check data tier
        match product.tier {
            DataTier::Critical => Ok(true),
            DataTier::Hot => {
                // Check if object type is in replication rules
                product.replication_rules
                    .object_types
                    .contains(&change.object_type)
            }
            DataTier::Warm | DataTier::Cold => Ok(false),
        }
    }
}
```

### Request Routing at Edge

```rust
async fn check_at_edge(
    request: CheckRequest,
    edge: &EdgeNode
) -> Result<CheckResponse> {
    // Step 1: Extract product ID
    let product_id = extract_product_id(&request)?;

    // Step 2: Get product configuration
    let product = edge.product_registry.get(product_id).await?;

    // Step 3: Route based on product mode
    match product.mode {
        ProductMode::Full => {
            // Try local resolution
            edge.local_resolver.check(request).await
        }

        ProductMode::HotOnly => {
            // Check if data is available locally
            if edge.has_local_data(&request).await? {
                edge.local_resolver.check(request).await
            } else {
                // Forward to regional
                edge.regional_client.check(request).await
            }
        }

        ProductMode::CacheOnly => {
            // Check L1/L2 cache only
            if let Some(result) = edge.cache.get(&request).await? {
                Ok(result)
            } else {
                // Forward to regional
                edge.regional_client.check(request).await
            }
        }

        ProductMode::Forward => {
            // Always forward to regional/central
            edge.regional_client.check(request).await
        }
    }
}
```

---

## Security Considerations

### 1. Authentication & Authorization

- **API Authentication**: Bearer tokens, API keys, mTLS
- **Store Isolation**: Each store is isolated, no cross-store access
- **RBAC**: Admin API requires elevated permissions

### 2. Input Validation

- **Model Validation**: Validate DSL syntax and semantic correctness
- **Tuple Validation**: Ensure tuples match model types
- **Query Validation**: Prevent injection attacks in filters

### 3. Rate Limiting

- **Per-Store Limits**: Prevent single tenant from overwhelming system
- **Per-IP Limits**: DDoS protection
- **Adaptive Throttling**: Slow down abusive clients

### 4. Audit Logging

- **Write Operations**: Log all tuple writes/deletes
- **Model Changes**: Log authorization model updates
- **Admin Operations**: Log store creation/deletion

---

## Observability

### Metrics

**RED Metrics** (Rate, Errors, Duration):
```
# Request rate
rsfga_requests_total{operation="check", store="acme"}
rsfga_requests_duration_seconds{operation="check", store="acme"}
rsfga_request_errors_total{operation="check", store="acme", error_type="timeout"}

# Business metrics
rsfga_graph_depth{operation="check", store="acme"}
rsfga_cache_hit_ratio{cache_type="l1", store="acme"}
rsfga_tuple_count{store="acme", object_type="document"}
```

### Tracing

**OpenTelemetry Integration**:
- Trace each request through all layers
- Include span attributes: store_id, object_type, relation, user
- Distributed tracing across edge → regional → central

### Logging

**Structured Logging** with `tracing`:
```rust
#[instrument(skip(self), fields(store=%store, relation=%relation))]
async fn check(&self, store: &str, user: &str, relation: &str, object: &str)
    -> Result<bool>
{
    debug!("Starting check operation");

    let result = self.resolve(user, relation, object).await?;

    info!(allowed=%result, "Check completed");

    Ok(result)
}
```

---

## Testing Strategy

### Unit Tests

- **Type System**: Parser correctness, model validation
- **Graph Resolver**: Resolution logic for all relation types
- **Storage**: CRUD operations, pagination, filtering

### Integration Tests

- **API Compatibility**: Test against OpenFGA test suite
- **Multi-Store**: Isolation and concurrent access
- **Storage Backends**: Test all supported databases

### Performance Tests

- **Load Testing**: k6 scripts matching OpenFGA benchmarks
- **Stress Testing**: Sustained high load, memory leaks
- **Chaos Testing**: Failure scenarios (DB down, network partitions)

### Compatibility Tests

- **API Compatibility**: Test all OpenFGA API endpoints
- **Model Compatibility**: Test all DSL features
- **Migration Testing**: Import OpenFGA data and verify correctness

---

## Deployment Considerations

### Single-Node Deployment (MVP)

```yaml
# Docker Compose example
version: '3.8'
services:
  rsfga:
    image: rsfga:latest
    ports:
      - "8080:8080"  # HTTP
      - "8081:8081"  # gRPC
    environment:
      - DATASTORE_ENGINE=postgres
      - DATASTORE_URI=postgresql://user:pass@db:5432/rsfga
      - LOG_LEVEL=info
    depends_on:
      - db

  db:
    image: postgres:16
    environment:
      - POSTGRES_DB=rsfga
      - POSTGRES_USER=rsfga
      - POSTGRES_PASSWORD=secret
```

### Multi-Region Deployment (Phase 3)

**Requirements**:
- Kubernetes for orchestration
- Kafka for change data capture
- Valkey/Redis for distributed caching
- PostgreSQL with multi-region replication

---

## Migration Path from OpenFGA

### Data Migration

1. **Export from OpenFGA**: Use OpenFGA CLI to export tuples and models
2. **Transform**: Convert to RSFGA format (should be 1:1)
3. **Import**: Use RSFGA bulk import API
4. **Validate**: Run comparison tests to ensure correctness

### Gradual Rollout

**Stage 1**: Read-only shadow mode
- RSFGA mirrors OpenFGA data
- Compare check results (log discrepancies)
- Fix bugs, tune performance

**Stage 2**: Canary deployment
- Route 1% of check traffic to RSFGA
- Monitor error rates, latency
- Gradually increase to 100%

**Stage 3**: Full migration
- Route all traffic to RSFGA
- OpenFGA remains as fallback
- Decommission OpenFGA after stability period

---

## Open Questions & Future Research

### Performance Validation
- [ ] Benchmark async graph traversal vs sync (expected 2-5x, needs validation)
- [ ] Measure DashMap vs standard Mutex in high-contention scenarios
- [ ] Profile memory usage under sustained load

### Scalability
- [ ] Determine optimal shard count for distributed edge
- [ ] Test Kafka throughput for change data capture
- [ ] Evaluate RocksDB vs sled for embedded storage at edge

### Features
- [ ] CEL condition evaluation (deferred to post-MVP)
- [ ] Relationship queries (expand API)
- [ ] Schema versioning and migration tools

### Operational
- [ ] Auto-tuning of cache sizes and TTLs
- [ ] Automated failover in multi-region setup
- [ ] Backup and restore procedures

---

## Success Metrics

### Correctness
- ✅ 100% API compatibility with OpenFGA
- ✅ Pass OpenFGA's official test suite
- ✅ Zero correctness regressions in canary testing

### Performance
- ⚡ Check: >1000 req/s (2x improvement)
- ⚡ Batch-Check: >500 checks/s (20x improvement)
- ⚡ Write: >150 req/s (2.5x improvement)
- ⚡ p95 latency: <20ms for all operations

### Scalability
- 🌍 Edge deployment supports <10ms latency globally
- 🌍 Precomputation handles 1M+ tuples with <100ms invalidation
- 🌍 Distributed architecture supports 100K+ req/s cluster-wide

### Operational Excellence
- 📊 Full observability (metrics, traces, logs)
- 🔒 Production-ready security (auth, audit, rate limiting)
- 🐳 Easy deployment (Docker, Kubernetes, single binary)

---

## Conclusion

RSFGA provides a high-performance, Rust-based implementation of OpenFGA with:

1. **MVP Phase**: Full API compatibility with 2-5x performance improvement
2. **Phase 2**: Precomputation engine for sub-millisecond check operations
3. **Phase 3**: Distributed edge architecture for global scalability

By leveraging Rust's zero-cost abstractions, async runtime, and lock-free data structures, RSFGA aims to become the highest-performance authorization engine available, while maintaining full compatibility with the OpenFGA ecosystem.

**Next Steps**:
1. Set up Rust project structure and dependencies
2. Implement type system parser and validator
3. Build storage abstraction layer with PostgreSQL backend
4. Implement graph resolver with async traversal
5. Develop HTTP/gRPC API layer
6. Create comprehensive test suite
7. Benchmark against OpenFGA and iterate

All performance claims require validation through rigorous benchmarking and real-world testing.
