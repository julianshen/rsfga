# Leopard Pre-computation Architecture Diagrams

## 1. System Architecture (Mermaid)

### 1.1 Overall System Architecture

```mermaid
flowchart TB
    subgraph K8s["Kubernetes Cluster"]
        subgraph API["API Pods (Stateless, HPA)"]
            API1["rsfga-api-1"]
            API2["rsfga-api-2"]
            API3["rsfga-api-n"]
        end

        subgraph Workers["Pre-computation Workers (HPA)"]
            W1["worker-1"]
            W2["worker-2"]
            W3["worker-n"]
        end

        subgraph Valkey["Valkey Cluster"]
            VP["Primary"]
            VR1["Replica"]
            VR2["Replica"]
            VP --- VR1
            VP --- VR2
        end

        subgraph NATS["NATS JetStream"]
            NT["rsfga.tuples.*"]
            NM["rsfga.models.*"]
            NC["rsfga.cache.invalidate.*"]
        end

        subgraph PG["PostgreSQL"]
            PGP["Primary"]
            PGR["Replica"]
            PGP --- PGR
        end
    end

    Client([Client]) --> API
    API -->|Read/Write| PG
    API -->|Cache Lookup| Valkey
    API -->|Publish Events| NATS
    API -->|Subscribe Invalidation| NC

    Workers -->|Subscribe| NT
    Workers -->|Subscribe| NM
    Workers -->|Read Tuples| PG
    Workers -->|Write Cache| Valkey
    Workers -->|Publish Invalidation| NC

    style API fill:#e1f5fe
    style Workers fill:#fff3e0
    style Valkey fill:#f3e5f5
    style NATS fill:#e8f5e9
    style PG fill:#fce4ec
```

### 1.2 Cache Tier Architecture

```mermaid
flowchart TB
    Request([Check Request]) --> L1

    subgraph L1["L1: In-Memory Hot Cache"]
        direction LR
        L1D["Per-Pod Local Cache<br/>Moka TinyLFU<br/><1μs latency<br/>~100K entries"]
    end

    L1 -->|Miss| L2

    subgraph L2["L2: Valkey Distributed Cache"]
        direction LR
        L2D["Shared Across Pods<br/>LRU Eviction<br/>100-500μs latency<br/>2GB+ capacity"]
    end

    L2 -->|Miss| L3

    subgraph L3["L3: Graph Resolver"]
        direction LR
        L3D["PostgreSQL<br/>Full Resolution<br/>1-10ms latency<br/>Source of Truth"]
    end

    L3 -->|Populate| L1
    L3 -->|Populate| L2

    style L1 fill:#c8e6c9,stroke:#2e7d32
    style L2 fill:#bbdefb,stroke:#1565c0
    style L3 fill:#ffecb3,stroke:#ff8f00
```

### 1.3 Write Path Flow

```mermaid
sequenceDiagram
    participant C as Client
    participant API as API Pod
    participant PG as PostgreSQL
    participant NATS as NATS JetStream
    participant W as Worker
    participant V as Valkey
    participant L1 as Hot Cache (L1)

    C->>API: WriteTuples()
    API->>PG: INSERT tuples
    PG-->>API: OK
    API->>NATS: Publish TupleChangeEvent
    NATS-->>API: ACK
    API-->>C: OK

    Note over NATS,W: Async Pre-computation

    NATS->>W: TupleChangeEvent
    W->>W: Analyze Impact<br/>(Reverse Expansion)
    W->>PG: Query affected checks
    PG-->>W: Affected (user, relation, object)

    loop For each affected check
        W->>PG: Resolve check
        PG-->>W: Result
        W->>V: SET precomputed result
    end

    W->>NATS: Publish CacheInvalidationEvent
    NATS->>API: CacheInvalidationEvent
    API->>L1: Invalidate affected keys
    W-->>NATS: ACK
```

### 1.4 Read Path Flow

```mermaid
sequenceDiagram
    participant C as Client
    participant API as API Pod
    participant L1 as Hot Cache (L1)
    participant V as Valkey (L2)
    participant PG as PostgreSQL (L3)

    C->>API: Check(user, relation, object)

    API->>L1: GET cache_key

    alt L1 Hit (Hot Data)
        L1-->>API: CachedCheckResult
        API-->>C: Result (<1μs)
    else L1 Miss
        API->>V: GET cache_key

        alt L2 Hit (Pre-computed)
            V-->>API: PrecomputedCheck
            API->>L1: Promote to hot cache
            API-->>C: Result (~100-500μs)
        else L2 Miss
            API->>PG: Graph Resolution
            PG-->>API: Result
            API->>L1: Insert (sync)
            API-->>V: SET (async, fire-and-forget)
            API-->>C: Result (~1-10ms)
        end
    end
```

### 1.5 Pre-computation Worker Flow

```mermaid
flowchart TB
    subgraph Input["Event Sources"]
        TE["TupleChangeEvent"]
        ME["ModelChangeEvent"]
    end

    subgraph Worker["Pre-computation Worker"]
        subgraph Analysis["Impact Analysis"]
            DC["Direct Check"]
            RE["Reverse Expansion"]
            GI["Group Impact<br/>(Leopard)"]
            TTU["TTU Expansion"]
        end

        subgraph Compute["Parallel Computation"]
            PC["Parallel Check<br/>Resolution"]
        end

        subgraph Output["Cache Update"]
            VW["Write to Valkey"]
            INV["Broadcast<br/>Invalidation"]
        end
    end

    TE --> DC
    TE --> RE
    TE --> GI
    TE --> TTU
    ME --> INV

    DC --> PC
    RE --> PC
    GI --> PC
    TTU --> PC

    PC --> VW
    VW --> INV

    style Analysis fill:#e3f2fd
    style Compute fill:#fff8e1
    style Output fill:#f1f8e9
```

### 1.6 Hot Cache Invalidation Flow

```mermaid
flowchart LR
    subgraph Workers["Pre-computation Workers"]
        W1["Worker 1"]
        W2["Worker 2"]
    end

    subgraph NATS["NATS JetStream"]
        INV["rsfga.cache.invalidate.*"]
    end

    subgraph APIs["API Pods"]
        subgraph Pod1["Pod 1"]
            HC1["Hot Cache"]
        end
        subgraph Pod2["Pod 2"]
            HC2["Hot Cache"]
        end
        subgraph Pod3["Pod 3"]
            HC3["Hot Cache"]
        end
    end

    W1 -->|Publish| INV
    W2 -->|Publish| INV
    INV -->|Subscribe| Pod1
    INV -->|Subscribe| Pod2
    INV -->|Subscribe| Pod3

    style INV fill:#c8e6c9
```

---

## 2. Component Diagrams (Mermaid)

### 2.1 API Pod Components

```mermaid
classDiagram
    class CachedCheckResolver {
        -valkey: ConnectionManager
        -resolver: GraphResolver
        -nats: NatsClient
        -hot_cache: Cache~String, CachedCheckResult~
        -metrics: CheckMetrics
        +check(store_id, user, relation, object) Result~bool~
        +write_tuples(store_id, writes, deletes) Result
        +start_invalidation_listener() Result
    }

    class HotCacheConfig {
        +max_entries: u64
        +ttl: Duration
        +tti: Duration
    }

    class CachedCheckResult {
        +allowed: bool
        +model_id: String
        +cached_at: Instant
    }

    class PrecomputedCheck {
        +allowed: bool
        +computed_at: DateTime
        +model_id: String
    }

    CachedCheckResolver --> HotCacheConfig
    CachedCheckResolver --> CachedCheckResult
    CachedCheckResolver --> PrecomputedCheck
```

### 2.2 Worker Components

```mermaid
classDiagram
    class PrecomputeWorker {
        -jetstream: JetStreamContext
        -tuple_consumer: PullConsumer
        -model_consumer: PullConsumer
        -valkey: ConnectionManager
        -db_pool: PgPool
        -resolver: GraphResolver
        -config: WorkerConfig
        +run() Result
        -process_tuple_events(messages) Result
        -process_model_events(messages) Result
        -analyze_impact(store_id, events) Vec~CheckToCompute~
        -compute_and_cache(check) Result
    }

    class WorkerConfig {
        +batch_size: usize
        +max_concurrency: usize
        +assigned_stores: Vec~String~
        +enable_leopard_optimization: bool
        +max_expansion_depth: u32
        +cache_ttl: Duration
    }

    class CheckToCompute {
        +store_id: String
        +user: String
        +relation: String
        +object: String
    }

    PrecomputeWorker --> WorkerConfig
    PrecomputeWorker --> CheckToCompute
```

### 2.3 Event Schema

```mermaid
classDiagram
    class TupleChangeEvent {
        +event_id: Uuid
        +store_id: String
        +timestamp: DateTime
        +changes: Vec~TupleChange~
        +model_id: String
    }

    class TupleChange {
        <<enumeration>>
        Write
        Delete
    }

    class ModelChangeEvent {
        +event_id: Uuid
        +store_id: String
        +timestamp: DateTime
        +old_model_id: Option~String~
        +new_model_id: String
        +affected_relations: Vec~String~
    }

    class CacheInvalidationEvent {
        <<enumeration>>
        Check
        Pattern
        Store
    }

    TupleChangeEvent --> TupleChange
```

---

## 3. Deployment Architecture (Mermaid)

### 3.1 Kubernetes Deployment

```mermaid
flowchart TB
    subgraph Ingress["Ingress Controller"]
        IG["nginx-ingress"]
    end

    subgraph Services["Kubernetes Services"]
        APIS["rsfga-api-svc<br/>ClusterIP"]
        VS["valkey-svc<br/>ClusterIP"]
        NS["nats-svc<br/>ClusterIP"]
        PGS["postgres-svc<br/>ClusterIP"]
    end

    subgraph Deployments["Deployments"]
        subgraph APIDep["rsfga-api Deployment"]
            APIHPA["HPA: 3-20 replicas<br/>CPU: 70%, Memory: 80%"]
        end

        subgraph WorkerDep["rsfga-worker Deployment"]
            WHPA["HPA: 2-10 replicas<br/>NATS pending: 1000"]
        end
    end

    subgraph StatefulSets["StatefulSets"]
        ValkeyS["Valkey<br/>3 replicas"]
        NATSS["NATS<br/>3 replicas"]
        PGS2["PostgreSQL<br/>Primary + Replica"]
    end

    IG --> APIS
    APIS --> APIDep
    APIDep --> VS
    APIDep --> NS
    APIDep --> PGS

    WorkerDep --> VS
    WorkerDep --> NS
    WorkerDep --> PGS

    VS --> ValkeyS
    NS --> NATSS
    PGS --> PGS2

    style Ingress fill:#e1f5fe
    style Services fill:#fff3e0
    style Deployments fill:#e8f5e9
    style StatefulSets fill:#fce4ec
```

---

## 4. Data Flow Diagrams (Mermaid)

### 4.1 Leopard Group Membership Expansion

```mermaid
flowchart TB
    subgraph Input["Tuple Change"]
        TC["user:alice added to<br/>group:engineering#member"]
    end

    subgraph Expansion["Impact Expansion"]
        D1["Direct: group:engineering"]

        subgraph Parents["Parent Groups"]
            P1["group:product"]
            P2["group:company"]
        end

        subgraph Objects["Objects with Group Access"]
            O1["document:spec#viewer<br/>@group:engineering"]
            O2["folder:projects#editor<br/>@group:product"]
            O3["org:acme#member<br/>@group:company"]
        end
    end

    subgraph Output["Checks to Recompute"]
        C1["user:alice, viewer, document:spec"]
        C2["user:alice, editor, folder:projects"]
        C3["user:alice, member, org:acme"]
    end

    TC --> D1
    D1 --> P1
    P1 --> P2

    D1 --> O1
    P1 --> O2
    P2 --> O3

    O1 --> C1
    O2 --> C2
    O3 --> C3

    style Input fill:#ffecb3
    style Expansion fill:#e3f2fd
    style Output fill:#c8e6c9
```

### 4.2 Model Change Invalidation

```mermaid
flowchart TB
    subgraph Trigger["Model Change"]
        MC["New model deployed<br/>store: acme"]
    end

    subgraph Valkey["Valkey Invalidation"]
        SCAN["SCAN check:acme:*"]
        DEL["DEL matching keys"]
    end

    subgraph NATS["NATS Broadcast"]
        PUB["Publish CacheInvalidationEvent::Store"]
    end

    subgraph Pods["All API Pods"]
        P1["Pod 1: invalidate_entries_if()"]
        P2["Pod 2: invalidate_entries_if()"]
        P3["Pod 3: invalidate_entries_if()"]
    end

    MC --> SCAN
    SCAN --> DEL
    DEL --> PUB
    PUB --> P1
    PUB --> P2
    PUB --> P3

    style Trigger fill:#ffcdd2
    style Valkey fill:#f3e5f5
    style NATS fill:#c8e6c9
    style Pods fill:#e3f2fd
```
