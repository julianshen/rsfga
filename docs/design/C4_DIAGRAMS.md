# RSFGA Architecture Diagrams (C4 Model)

## Overview

This document provides architecture diagrams following the [C4 model](https://c4model.com/):
1. **System Context**: How RSFGA fits in the broader ecosystem
2. **Container**: High-level components and technology choices
3. **Component**: Detailed component interactions
4. **Code**: Key classes and relationships

Diagrams are provided in both **Mermaid** (for GitHub rendering) and **PlantUML** formats.

---

## Level 1: System Context Diagram

Shows RSFGA in the context of its users and external systems.

### Mermaid

```mermaid
C4Context
    title System Context Diagram for RSFGA

    Person(user, "Application User", "End user of applications that use RSFGA for authorization")
    Person(admin, "System Administrator", "Manages RSFGA deployment and configuration")
    Person(developer, "Developer", "Integrates RSFGA into applications")

    System(rsfga, "RSFGA", "High-performance authorization system based on Zanzibar model")

    System_Ext(app, "Client Application", "Applications that need fine-grained authorization")
    System_Ext(db, "PostgreSQL", "Primary data store for authorization data")
    System_Ext(cache, "Valkey/Redis", "Cache for precomputed check results (Phase 2)")
    System_Ext(nats, "NATS", "Message bus for edge synchronization (Phase 3)")
    System_Ext(metrics, "Prometheus", "Metrics collection and alerting")
    System_Ext(tracing, "Jaeger", "Distributed tracing")

    Rel(user, app, "Uses", "HTTPS")
    Rel(app, rsfga, "Checks permissions", "HTTP/gRPC")
    Rel(developer, rsfga, "Integrates with", "SDK")
    Rel(admin, rsfga, "Manages", "Admin API")

    Rel(rsfga, db, "Reads/Writes tuples", "SQL")
    Rel(rsfga, cache, "Caches results", "Redis Protocol")
    Rel(rsfga, nats, "Publishes changes", "NATS Protocol")
    Rel(rsfga, metrics, "Sends metrics", "Prometheus")
    Rel(rsfga, tracing, "Sends traces", "OTLP")

    UpdateLayoutConfig($c4ShapeInRow="3", $c4BoundaryInRow="2")
```

### PlantUML

```plantuml
@startuml C4_Context
!include https://raw.githubusercontent.com/plantuml-stdlib/C4-PlantUML/master/C4_Context.puml

LAYOUT_WITH_LEGEND()

title System Context Diagram for RSFGA

Person(user, "Application User", "End user of applications")
Person(admin, "Administrator", "Manages RSFGA")
Person(developer, "Developer", "Integrates RSFGA")

System(rsfga, "RSFGA", "High-performance authorization system")

System_Ext(app, "Client Apps", "Applications using RSFGA")
System_Ext(db, "PostgreSQL", "Authorization data store")
System_Ext(cache, "Valkey", "Precomputed results cache")
System_Ext(nats, "NATS", "Edge sync message bus")
System_Ext(metrics, "Prometheus", "Metrics & alerting")
System_Ext(tracing, "Jaeger", "Distributed tracing")

Rel(user, app, "Uses")
Rel(app, rsfga, "Check permissions", "HTTP/gRPC")
Rel(developer, rsfga, "Integrates", "SDK")
Rel(admin, rsfga, "Manages", "Admin API")

Rel(rsfga, db, "Store/Query", "SQL")
Rel(rsfga, cache, "Cache", "Redis")
Rel(rsfga, nats, "Publish", "NATS")
Rel(rsfga, metrics, "Metrics")
Rel(rsfga, tracing, "Traces")

@enduml
```

---

## Level 2: Container Diagram

Shows the major containers (processes/deployments) within RSFGA.

### Mermaid

```mermaid
C4Container
    title Container Diagram for RSFGA

    Person(app, "Client Application", "Applications requiring authorization")

    System_Boundary(rsfga, "RSFGA System") {
        Container(api, "API Server", "Rust, Axum/Tonic", "Provides HTTP and gRPC APIs for authorization checks")
        Container(worker, "Precomputation Worker", "Rust, Tokio", "Precomputes check results on writes (Phase 2)")
        Container(edge, "Edge Node", "Rust", "Local authorization at edge with selective sync (Phase 3)")

        ContainerDb(cache_l1, "L1 Cache", "DashMap", "In-memory check result cache")
    }

    System_Ext(db, "PostgreSQL", "Stores tuples and models")
    System_Ext(cache_l2, "Valkey Cluster", "Shared precomputed results cache")
    System_Ext(nats, "NATS Cluster", "Change event streaming")
    System_Ext(metrics, "Prometheus", "Metrics collection")
    System_Ext(tracing, "Jaeger", "Distributed tracing")

    Rel(app, api, "Check, Write, Read", "HTTP/gRPC")
    Rel(api, cache_l1, "Query", "In-memory")
    Rel(api, db, "Query/Write tuples", "SQLx")
    Rel(api, cache_l2, "Query precomputed", "Redis")
    Rel(api, nats, "Publish changes", "NATS")
    Rel(api, metrics, "Export metrics")
    Rel(api, tracing, "Export traces")

    Rel(worker, db, "Read tuples", "SQL")
    Rel(worker, cache_l2, "Write precomputed", "Redis")
    Rel(worker, nats, "Subscribe changes", "NATS")

    Rel(edge, api, "Fallback requests", "gRPC")
    Rel(edge, nats, "Subscribe changes", "NATS Leaf")
    ContainerDb(edge_db, "Local Store", "RocksDB/sled", "Edge-local tuple storage")
    Rel(edge, edge_db, "Query", "Embedded")

    UpdateLayoutConfig($c4ShapeInRow="2", $c4BoundaryInRow="1")
```

### PlantUML

```plantuml
@startuml C4_Container
!include https://raw.githubusercontent.com/plantuml-stdlib/C4-PlantUML/master/C4_Container.puml

LAYOUT_WITH_LEGEND()

title Container Diagram for RSFGA

Person(app, "Client App")

System_Boundary(rsfga, "RSFGA") {
    Container(api, "API Server", "Rust, Axum/Tonic", "HTTP/gRPC APIs")
    Container(worker, "Precompute Worker", "Rust, Tokio", "Background computation")
    Container(edge, "Edge Node", "Rust", "Edge deployment")

    ContainerDb(cache_l1, "L1 Cache", "DashMap", "In-memory cache")
    ContainerDb(edge_db, "Edge DB", "RocksDB", "Local storage")
}

ContainerDb(db, "PostgreSQL", "Authorization data")
ContainerDb(cache_l2, "Valkey", "Precomputed cache")
Container(nats, "NATS", "Message bus")
System_Ext(metrics, "Prometheus")
System_Ext(tracing, "Jaeger")

Rel(app, api, "Check/Write", "HTTP/gRPC")
Rel(api, cache_l1, "Read")
Rel(api, db, "SQL")
Rel(api, cache_l2, "Redis")
Rel(api, nats, "Publish")
Rel(api, metrics, "Metrics")
Rel(api, tracing, "Traces")

Rel(worker, db, "Read")
Rel(worker, cache_l2, "Write")
Rel(worker, nats, "Subscribe")

Rel(edge, api, "Fallback")
Rel(edge, nats, "Leaf")
Rel(edge, edge_db, "Query")

@enduml
```

---

## Level 3: Component Diagram - API Server

Shows internal components of the API Server.

### Mermaid

```mermaid
C4Component
    title Component Diagram for API Server

    Container_Boundary(api, "API Server") {
        Component(http, "HTTP Handler", "Axum", "REST API endpoints")
        Component(grpc, "gRPC Handler", "Tonic", "gRPC service implementation")
        Component(auth, "Auth Middleware", "Rust", "Bearer token authentication")
        Component(rate_limit, "Rate Limiter", "Rust", "Request rate limiting")

        Component(check_handler, "Check Handler", "Rust", "Single check operations")
        Component(batch_handler, "Batch Check Handler", "Rust", "Batch check with dedup")
        Component(write_handler, "Write Handler", "Rust", "Tuple write operations")
        Component(read_handler, "Read Handler", "Rust", "Tuple read operations")
        Component(model_handler, "Model Handler", "Rust", "Authorization model CRUD")

        Component(resolver, "Graph Resolver", "Rust", "Async graph traversal")
        Component(type_system, "Type System", "Rust", "Model parser and validator")
        Component(cache, "Check Cache", "DashMap/Moka", "L1 cache layer")

        Component(datastore, "Datastore Abstraction", "Trait", "Storage interface")
        Component(postgres, "PostgreSQL Store", "SQLx", "Postgres implementation")
        Component(valkey, "Valkey Client", "Redis", "Cache client")
    }

    Rel(http, auth, "Authenticates")
    Rel(grpc, auth, "Authenticates")
    Rel(auth, rate_limit, "Rate limits")

    Rel(http, check_handler, "Delegates")
    Rel(grpc, check_handler, "Delegates")
    Rel(http, batch_handler, "Delegates")
    Rel(http, write_handler, "Delegates")
    Rel(http, model_handler, "Delegates")

    Rel(check_handler, resolver, "Resolves")
    Rel(batch_handler, resolver, "Resolves parallel")
    Rel(write_handler, datastore, "Writes")
    Rel(read_handler, datastore, "Reads")
    Rel(model_handler, datastore, "CRUD models")

    Rel(resolver, cache, "Queries/Updates")
    Rel(resolver, type_system, "Gets relations")
    Rel(resolver, datastore, "Reads tuples")

    Rel(datastore, postgres, "Implements")
    Rel(cache, valkey, "L2 lookup")

    UpdateLayoutConfig($c4ShapeInRow="3")
```

### Diagram: Request Flow

```mermaid
sequenceDiagram
    participant Client
    participant HTTP as HTTP Handler
    participant Auth as Auth Middleware
    participant Rate as Rate Limiter
    participant Check as Check Handler
    participant Cache as Check Cache
    participant Resolver as Graph Resolver
    participant DB as PostgreSQL

    Client->>HTTP: POST /check
    HTTP->>Auth: Validate token
    Auth->>Rate: Check rate limit
    Rate->>Check: Handle request

    Check->>Cache: Lookup cache key
    alt Cache Hit
        Cache-->>Check: Return cached result
        Check-->>HTTP: Response
    else Cache Miss
        Check->>Resolver: Resolve check
        Resolver->>DB: Query tuples
        DB-->>Resolver: Tuples
        Resolver->>Resolver: Traverse graph
        Resolver-->>Check: Result
        Check->>Cache: Store result
        Check-->>HTTP: Response
    end

    HTTP-->>Client: 200 OK {allowed: true}
```

---

## Level 3: Component Diagram - Graph Resolver

Shows the graph resolution algorithm components.

### Mermaid

```mermaid
graph TB
    subgraph "Graph Resolver"
        Entry[resolve_check]
        Cache[Check Cache]
        Cycle[Cycle Detection]
        Depth[Depth Check]
        Timeout[Timeout Check]

        Direct[resolve_direct]
        Computed[resolve_computed]
        Union[resolve_union]
        Intersection[resolve_intersection]
        TTU[resolve_ttu]
        Exclusion[resolve_exclusion]

        Entry -->|1. Check cache| Cache
        Cache -->|Miss| Cycle
        Cycle -->|Not visited| Depth
        Depth -->|< max| Timeout
        Timeout -->|< deadline| Dispatch

        Dispatch{Relation Type?}
        Dispatch -->|Direct| Direct
        Dispatch -->|Computed| Computed
        Dispatch -->|Union| Union
        Dispatch -->|Intersection| Intersection
        Dispatch -->|TTU| TTU
        Dispatch -->|Exclusion| Exclusion

        Direct -->|Query DB| DB[(PostgreSQL)]
        Computed -->|Recurse| Entry
        Union -->|Parallel race| Entry
        Intersection -->|Parallel join| Entry
        TTU -->|Two-step| Entry
        Exclusion -->|Base - Subtract| Entry

        DB -->|Tuples| Result[Result]
        Entry -->|Recurse result| Result
        Result -->|Cache| Cache
        Result -->|Return| Exit[Check Response]
    end

    style Entry fill:#e1f5e1
    style Result fill:#ffe1e1
    style Dispatch fill:#e1e5ff
```

---

## Level 4: Code Diagram - Core Structures

Key classes and their relationships.

### PlantUML Class Diagram

```plantuml
@startuml CoreClasses

class Store {
    +id: String
    +name: String
    +created_at: DateTime
    +updated_at: DateTime
}

class AuthorizationModel {
    +id: String
    +store_id: String
    +schema_version: String
    +type_definitions: HashMap<String, TypeDefinition>
    +created_at: DateTime
}

class TypeDefinition {
    +type_name: String
    +relations: HashMap<String, Relation>
    +metadata: Option<TypeMetadata>
}

enum Relation {
    Direct
    Computed
    Union
    Intersection
    Exclusion
    TupleToUserset
}

class Tuple {
    +store_id: String
    +object: ObjectRef
    +relation: String
    +user: UserRef
    +condition: Option<Condition>
    +timestamp: DateTime
}

class ObjectRef {
    +type_name: String
    +id: String
    +to_string(): String
    +parse(s: &str): Result<Self>
}

class UserRef {
    +type_name: String
    +id: String
    +relation: Option<String>
    +to_string(): String
    +parse(s: &str): Result<Self>
    +is_userset(): bool
}

class CheckContext {
    +store_id: String
    +model: Arc<AuthorizationModel>
    +contextual_tuples: Vec<Tuple>
    +visited: Arc<DashMap>
    +depth: Arc<AtomicU32>
    +max_depth: u32
    +timeout: Duration
}

interface DataStore {
    +read_tuples(filter): Result<Vec<Tuple>>
    +write_tuples(deletes, writes): Result<()>
    +read_model(id): Result<AuthorizationModel>
    +write_model(model): Result<String>
}

class PostgresDataStore {
    -pool: Arc<PgPool>
}

class GraphResolver {
    -datastore: Arc<dyn DataStore>
    -type_system: Arc<TypeSystem>
    -cache: Arc<CheckCache>
    +check(user, relation, object): Result<bool>
    -resolve(user, relation, object, ctx): Result<bool>
    -resolve_direct(...): Result<bool>
    -resolve_union(...): Result<bool>
    -resolve_ttu(...): Result<bool>
}

class CheckCache {
    -cache: Cache<CacheKey, bool>
    +get(key): Option<bool>
    +insert(key, value, ttl): void
}

Store "1" -- "0..*" AuthorizationModel : has
AuthorizationModel "1" -- "0..*" TypeDefinition : contains
TypeDefinition "1" -- "0..*" Relation : defines
Store "1" -- "0..*" Tuple : contains
Tuple "1" -- "1" ObjectRef : references
Tuple "1" -- "1" UserRef : references

GraphResolver "1" -- "1" DataStore : uses
GraphResolver "1" -- "1" CheckCache : uses
DataStore <|.. PostgresDataStore : implements

GraphResolver "1" -- "1" CheckContext : creates

@enduml
```

---

## Deployment Diagram - Multi-Region Edge

### Mermaid

```mermaid
graph TB
    subgraph "Region: US-West"
        subgraph "Edge Nodes"
            Edge1[Edge Node SF]
            Edge2[Edge Node LA]
        end

        subgraph "Regional Cluster"
            NATS_US[NATS Cluster]
            API_US[API Servers]
            PG_US[(PostgreSQL Replica)]
            Valkey_US[(Valkey Cache)]
        end

        Edge1 -.->|NATS Leaf| NATS_US
        Edge2 -.->|NATS Leaf| NATS_US
        API_US --> PG_US
        API_US --> Valkey_US
        API_US --> NATS_US
    end

    subgraph "Region: EU-West"
        subgraph "Edge Nodes EU"
            Edge3[Edge Node London]
            Edge4[Edge Node Paris]
        end

        subgraph "Regional Cluster EU"
            NATS_EU[NATS Cluster]
            API_EU[API Servers]
            PG_EU[(PostgreSQL Replica)]
            Valkey_EU[(Valkey Cache)]
        end

        Edge3 -.->|NATS Leaf| NATS_EU
        Edge4 -.->|NATS Leaf| NATS_EU
        API_EU --> PG_EU
        API_EU --> Valkey_EU
        API_EU --> NATS_EU
    end

    subgraph "Central (Multi-Region)"
        NATS_Central[NATS Super Cluster]
        PG_Primary[(PostgreSQL Primary)]
        Prom[Prometheus]
        Jaeger[Jaeger]
    end

    NATS_US ===|Gateway| NATS_Central
    NATS_EU ===|Gateway| NATS_Central
    PG_US -.->|Replication| PG_Primary
    PG_EU -.->|Replication| PG_Primary

    API_US -.->|Metrics| Prom
    API_EU -.->|Metrics| Prom
    API_US -.->|Traces| Jaeger
    API_EU -.->|Traces| Jaeger

    style Edge1 fill:#c8e6c9
    style Edge2 fill:#c8e6c9
    style Edge3 fill:#c8e6c9
    style Edge4 fill:#c8e6c9
    style NATS_Central fill:#fff9c4
    style PG_Primary fill:#ffccbc
```

---

## Data Flow Diagram - Write Operation

```mermaid
sequenceDiagram
    participant Client
    participant API as API Server
    participant DB as PostgreSQL
    participant NATS
    participant Worker as Precompute Worker
    participant Cache as Valkey
    participant Edge as Edge Node

    Client->>API: POST /write {tuples}
    API->>DB: BEGIN TRANSACTION
    API->>DB: DELETE tuples
    API->>DB: INSERT tuples
    API->>DB: INSERT changelog
    API->>DB: COMMIT

    par Background Tasks
        API->>NATS: Publish change event
        API->>Cache: Invalidate related keys (async)
    end

    API-->>Client: 200 OK

    NATS->>Worker: Consume change event
    Worker->>Worker: Classify change type
    Worker->>Worker: Find affected checks
    Worker->>DB: Batch check affected
    Worker->>Cache: Write precomputed results

    NATS->>Edge: Leaf node receives event
    Edge->>Edge: Filter by product config
    alt Should Sync
        Edge->>Edge: Apply to local store
    else Skip
        Edge->>Edge: Ignore
    end
```

---

## Data Flow Diagram - Check Operation (with Precomputation)

```mermaid
sequenceDiagram
    participant Client
    participant API as API Server
    participant L1 as L1 Cache (DashMap)
    participant L2 as L2 Cache (Valkey)
    participant Resolver as Graph Resolver
    participant DB as PostgreSQL

    Client->>API: POST /check
    API->>L1: Query cache
    alt L1 Hit
        L1-->>API: Cached result
        API-->>Client: 200 OK {allowed: true}
    else L1 Miss
        API->>L2: Query precomputed
        alt L2 Hit
            L2-->>API: Precomputed result
            API->>L1: Store in L1
            API-->>Client: 200 OK {allowed: true}
        else L2 Miss
            API->>Resolver: Resolve check
            Resolver->>DB: Query tuples
            DB-->>Resolver: Tuples
            Resolver->>Resolver: Traverse graph
            Resolver-->>API: Result
            API->>L1: Store in L1
            API->>L2: Store in L2 (async)
            API-->>Client: 200 OK {allowed: true}
        end
    end
```

---

## Summary

These diagrams provide complete architectural views of RSFGA:

- **C4 Level 1 (Context)**: System in ecosystem
- **C4 Level 2 (Container)**: Major deployable units
- **C4 Level 3 (Component)**: Internal components
- **C4 Level 4 (Code)**: Key classes and relationships
- **Deployment**: Multi-region edge architecture
- **Data Flow**: Write and check operations

All diagrams are provided in Mermaid (GitHub-compatible) and PlantUML formats.

**Tools**:
- Mermaid: View directly in GitHub/IDEs with Mermaid support
- PlantUML: Use [PlantUML Online](http://www.plantuml.com/plantuml/uml/) or local tools

**Next**: See [ROADMAP.md](./ROADMAP.md) for implementation plan.
