# Spike: Leopard Indexing for RSFGA Pre-computation

**Date**: 2026-01-12
**Author**: Claude (Research Spike)
**Status**: Research Complete
**Related**: Phase 2 (Precomputed Checks), ADR-TBD

---

## Executive Summary

Google's **Leopard** indexing system is a specialized pre-computation engine within Zanzibar that flattens group hierarchies into a transitive closure for sub-millisecond authorization lookups. After evaluating Leopard's architecture against RSFGA's Phase 2 requirements in a **cloud-native deployment context**, **Leopard's architecture is well-suited** with adaptations for our technology stack.

### Recommendation

**Adopt Leopard's full architecture adapted for cloud-native deployment**:
1. Use NATS JetStream for change event streaming (replaces Zanzibar's Watch API)
2. Store pre-computed results in Valkey (replaces in-memory skip lists)
3. Deploy pre-computation workers as separate Kubernetes pods (horizontal scaling)
4. Extend beyond group membership to cover all relation types
5. Implement incremental updates with periodic full rebuilds for consistency

---

## 1. Leopard Architecture Overview

### 1.1 What is Leopard?

Leopard is a specialized indexing system that pre-computes the transitive closure of group memberships. It solves a specific problem: deeply nested or widely branching group hierarchies cause "pointer chasing" during check evaluation, requiring multiple serial database lookups.

### 1.2 Core Data Structures

Leopard uses two specialized set types:

```text
GROUP2GROUP(ancestor) → {descendant groups}
MEMBER2GROUP(user) → {groups where user is a direct member}
```

To check if user U belongs to group G:
```text
(MEMBER2GROUP(U) ∩ GROUP2GROUP(G)) ≠ ∅
```

This transforms graph traversal into a set intersection operation.

### 1.3 Three-Layer Architecture (Original Zanzibar)

```text
┌─────────────────────────────────────────────────────────────┐
│                    Leopard Serving System                    │
│  ┌─────────────────┐  ┌─────────────────────────────────┐   │
│  │   Skip Lists    │  │      Set Intersection Engine     │   │
│  │  (Ordered IDs)  │  │  O(min(|A|,|B|)) seeks          │   │
│  └─────────────────┘  └─────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
                              ↑
┌─────────────────────────────────────────────────────────────┐
│                 Online Incremental Layer                     │
│  ┌─────────────────┐  ┌─────────────────────────────────┐   │
│  │  Watch API      │  │  Delta Index                     │   │
│  │  Consumer       │  │  (Updates since offline build)   │   │
│  └─────────────────┘  └─────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
                              ↑
┌─────────────────────────────────────────────────────────────┐
│               Offline Index Builder                          │
│  ┌─────────────────┐  ┌─────────────────────────────────┐   │
│  │  Snapshot       │  │  Transitive Closure              │   │
│  │  Reader         │  │  Computation                     │   │
│  └─────────────────┘  └─────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

### 1.4 Performance Characteristics (Google Scale)

| Metric | Value |
|--------|-------|
| Query latency (p50) | <150 μs |
| Query latency (p99) | <1 ms |
| Query throughput | 1.56M QPS (median) |
| Index updates | 500-1.5K updates/sec |
| Write amplification | 1 Zanzibar tuple → tens of thousands of Leopard tuples |

---

## 2. RSFGA Phase 2 Requirements (Updated)

### 2.1 Deployment Environment

**Cloud-Native Architecture**:
- **Production**: Kubernetes (EKS, GKE, AKS, or self-hosted)
- **Local Development**: Docker Compose
- **No single-node constraint**: Horizontal scaling is expected

### 2.2 Technology Stack

| Component | Technology | Purpose |
|-----------|------------|---------|
| Event Streaming | **NATS JetStream** | Tuple/model change events |
| Pre-computed Cache | **Valkey** | Distributed cache for check results |
| Primary Storage | PostgreSQL/MySQL | Authoritative tuple storage |
| Container Orchestration | Kubernetes | Production deployment |
| Local Development | Docker Compose | Developer experience |

### 2.3 Key Requirements

| Requirement | Target | Notes |
|-------------|--------|-------|
| Check latency (cache hit) | <1ms | Valkey lookup |
| Check latency (cache miss) | <10ms | Full graph resolution |
| Pre-computation lag | <100ms | Event to cache update |
| Horizontal scaling | Yes | Workers scale independently |
| Consistency model | Eventual | Acceptable per A9 |

### 2.4 Updated Constraints

| Constraint | Value | Rationale |
|------------|-------|-----------|
| ~~C4~~ | ~~Removed~~ | Cloud-native, not single-node |
| C12 | <500MB per API pod | Workers can use more |
| A9 | 1-10ms staleness OK | Eventual consistency acceptable |

---

## 3. Proposed Architecture

### 3.1 System Overview

```text
┌─────────────────────────────────────────────────────────────────────────────┐
│                              Kubernetes Cluster                              │
│                                                                              │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │                         API Pods (Stateless)                          │   │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐                   │   │
│  │  │  rsfga-api  │  │  rsfga-api  │  │  rsfga-api  │  ... (HPA)       │   │
│  │  │  :8080      │  │  :8080      │  │  :8080      │                   │   │
│  │  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘                   │   │
│  │         │                │                │                           │   │
│  └─────────┼────────────────┼────────────────┼───────────────────────────┘   │
│            │                │                │                               │
│            ▼                ▼                ▼                               │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │                         Valkey Cluster                                │   │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐                   │   │
│  │  │   Primary   │──│   Replica   │──│   Replica   │                   │   │
│  │  │   :6379     │  │   :6379     │  │   :6379     │                   │   │
│  │  └─────────────┘  └─────────────┘  └─────────────┘                   │   │
│  └──────────────────────────────────────────────────────────────────────┘   │
│                                    ▲                                         │
│                                    │ Write pre-computed results              │
│                                    │                                         │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │                    Pre-computation Workers                            │   │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐                   │   │
│  │  │  worker-0   │  │  worker-1   │  │  worker-2   │  ... (HPA)       │   │
│  │  │  (store A)  │  │  (store B)  │  │  (store C)  │                   │   │
│  │  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘                   │   │
│  │         │                │                │                           │   │
│  └─────────┼────────────────┼────────────────┼───────────────────────────┘   │
│            │ Subscribe      │                │                               │
│            ▼                ▼                ▼                               │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │                       NATS JetStream                                  │   │
│  │  ┌─────────────────────────────────────────────────────────────────┐ │   │
│  │  │  Streams:                                                        │ │   │
│  │  │  • rsfga.tuples.{store_id}     - Tuple change events            │ │   │
│  │  │  • rsfga.models.{store_id}     - Model change events            │ │   │
│  │  │  • rsfga.precompute.commands   - Rebuild commands               │ │   │
│  │  └─────────────────────────────────────────────────────────────────┘ │   │
│  └──────────────────────────────────────────────────────────────────────┘   │
│            ▲                                                                 │
│            │ Publish events                                                  │
│            │                                                                 │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │                         API Pods (Write Path)                         │   │
│  │  Write Tuple → PostgreSQL → Publish Event → NATS                     │   │
│  └──────────────────────────────────────────────────────────────────────┘   │
│                                                                              │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │                         PostgreSQL                                    │   │
│  │  ┌─────────────┐  ┌─────────────┐                                    │   │
│  │  │   Primary   │──│   Replica   │                                    │   │
│  │  │   :5432     │  │   :5432     │                                    │   │
│  │  └─────────────┘  └─────────────┘                                    │   │
│  └──────────────────────────────────────────────────────────────────────┘   │
│                                                                              │
└──────────────────────────────────────────────────────────────────────────────┘
```

### 3.2 Event Flow

```text
┌─────────────────────────────────────────────────────────────────────────────┐
│                            Write Path                                        │
│                                                                              │
│   Client                API Pod              PostgreSQL         NATS         │
│     │                     │                      │               │           │
│     │──WriteTuples()────▶│                      │               │           │
│     │                     │──INSERT tuples─────▶│               │           │
│     │                     │◀─────────OK─────────│               │           │
│     │                     │                      │               │           │
│     │                     │──Publish TupleChangeEvent──────────▶│           │
│     │                     │◀──────────ACK───────────────────────│           │
│     │◀──────OK───────────│                      │               │           │
│     │                     │                      │               │           │
└─────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────┐
│                         Pre-computation Path                                 │
│                                                                              │
│   NATS              Worker                PostgreSQL         Valkey          │
│     │                 │                      │                 │             │
│     │──TupleChangeEvent─▶│                   │                 │             │
│     │                 │                      │                 │             │
│     │                 │──Analyze Impact─────▶│                 │             │
│     │                 │  (reverse expansion) │                 │             │
│     │                 │◀─Affected checks─────│                 │             │
│     │                 │                      │                 │             │
│     │                 │──Recompute checks───▶│                 │             │
│     │                 │◀─Check results───────│                 │             │
│     │                 │                      │                 │             │
│     │                 │──Store pre-computed─────────────────▶│             │
│     │                 │◀─────────OK──────────────────────────│             │
│     │                 │                      │                 │             │
│     │◀────ACK────────│                      │                 │             │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────┐
│                            Read Path                                         │
│                                                                              │
│   Client              API Pod               Valkey           PostgreSQL      │
│     │                   │                     │                  │           │
│     │──Check()────────▶│                     │                  │           │
│     │                   │──GET cache key────▶│                  │           │
│     │                   │◀──HIT (result)─────│                  │           │
│     │◀──────Result─────│                     │                  │           │
│     │                   │                     │                  │           │
│     │       OR (cache miss):                 │                  │           │
│     │                   │──GET cache key────▶│                  │           │
│     │                   │◀──MISS─────────────│                  │           │
│     │                   │──Graph resolve────────────────────▶│           │
│     │                   │◀──Result──────────────────────────│           │
│     │                   │──SET cache (async)─▶│                  │           │
│     │◀──────Result─────│                     │                  │           │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 3.3 NATS Event Schema

```rust
/// Events published when tuples change
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TupleChangeEvent {
    /// Unique event ID for deduplication
    pub event_id: Uuid,

    /// Store where the change occurred
    pub store_id: String,

    /// Timestamp of the change (for ordering)
    pub timestamp: DateTime<Utc>,

    /// The actual changes
    pub changes: Vec<TupleChange>,

    /// Authorization model version at time of change
    pub model_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TupleChange {
    Write {
        user: String,
        relation: String,
        object: String,
        condition: Option<String>,
    },
    Delete {
        user: String,
        relation: String,
        object: String,
    },
}

/// Events published when authorization models change
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelChangeEvent {
    pub event_id: Uuid,
    pub store_id: String,
    pub timestamp: DateTime<Utc>,
    pub old_model_id: Option<String>,
    pub new_model_id: String,
    /// Hint for workers: which relations changed
    pub affected_relations: Vec<String>,
}

/// Commands for manual pre-computation control
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PrecomputeCommand {
    /// Rebuild all pre-computed data for a store
    FullRebuild { store_id: String },

    /// Rebuild specific object types
    PartialRebuild {
        store_id: String,
        object_types: Vec<String>,
    },

    /// Invalidate cache entries matching pattern
    Invalidate {
        store_id: String,
        pattern: String,
    },
}
```

### 3.4 NATS Stream Configuration

```yaml
# nats-streams.yaml
streams:
  # Tuple change events - partitioned by store
  - name: RSFGA_TUPLES
    subjects:
      - "rsfga.tuples.>"
    retention: limits
    max_age: 24h              # Keep events for 24 hours
    max_bytes: 10GB           # Per stream limit
    storage: file             # Persistent storage
    replicas: 3               # High availability
    discard: old              # Discard oldest when full

  # Model change events
  - name: RSFGA_MODELS
    subjects:
      - "rsfga.models.>"
    retention: limits
    max_age: 168h             # Keep for 7 days (models change rarely)
    max_bytes: 1GB
    storage: file
    replicas: 3
    discard: old

  # Pre-computation commands
  - name: RSFGA_PRECOMPUTE
    subjects:
      - "rsfga.precompute.>"
    retention: workqueue      # Remove after ACK
    max_bytes: 100MB
    storage: file
    replicas: 3

consumers:
  # Worker consumer group for tuple events
  - stream: RSFGA_TUPLES
    name: precompute-workers
    durable: precompute-workers
    deliver_policy: all
    ack_policy: explicit
    max_deliver: 5            # Retry up to 5 times
    ack_wait: 30s             # 30 second processing timeout
    filter_subject: "rsfga.tuples.>"
```

---

## 4. Pre-computation Worker Design

### 4.1 Worker Architecture

```rust
use async_nats::jetstream::{self, consumer::PullConsumer};
use valkey::aio::ConnectionManager;

/// Pre-computation worker that processes tuple/model changes
pub struct PrecomputeWorker {
    /// NATS JetStream connection
    jetstream: jetstream::Context,

    /// Consumer for tuple change events
    tuple_consumer: PullConsumer,

    /// Consumer for model change events
    model_consumer: PullConsumer,

    /// Valkey connection pool
    valkey: ConnectionManager,

    /// PostgreSQL connection pool for graph resolution
    db_pool: PgPool,

    /// Graph resolver for computing check results
    resolver: Arc<GraphResolver>,

    /// Worker configuration
    config: WorkerConfig,

    /// Metrics
    metrics: WorkerMetrics,
}

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// Batch size for processing events
    pub batch_size: usize,

    /// Maximum concurrent pre-computations
    pub max_concurrency: usize,

    /// Stores this worker is responsible for (empty = all)
    pub assigned_stores: Vec<String>,

    /// Whether to process group membership specially (Leopard-style)
    pub enable_leopard_optimization: bool,

    /// Maximum depth for reverse expansion
    pub max_expansion_depth: u32,

    /// TTL for pre-computed results in Valkey
    pub cache_ttl: Duration,

    /// Types to skip during reverse expansion (unbounded types like 'user', 'file')
    /// These would cause catastrophic expansion; rely on cache-on-read instead
    pub unbounded_types: HashSet<String>,

    /// Maximum objects to query per type during reverse expansion
    pub max_objects_per_type: usize,  // e.g., 10,000

    /// Maximum total affected checks per tuple change
    pub max_affected_checks: usize,   // e.g., 100,000
}

impl PrecomputeWorker {
    pub async fn run(&self) -> Result<()> {
        loop {
            tokio::select! {
                // Process tuple change events
                msgs = self.tuple_consumer.fetch().max_messages(self.config.batch_size).await => {
                    self.process_tuple_events(msgs?).await?;
                }

                // Process model change events (higher priority)
                msgs = self.model_consumer.fetch().max_messages(10).await => {
                    self.process_model_events(msgs?).await?;
                }
            }
        }
    }

    async fn process_tuple_events(&self, messages: Vec<Message>) -> Result<()> {
        let events: Vec<TupleChangeEvent> = messages
            .iter()
            .map(|m| serde_json::from_slice(&m.payload))
            .collect::<Result<_, _>>()?;

        // Group by store for efficient batch processing
        let by_store = events.into_iter().group_by(|e| e.store_id.clone());

        for (store_id, store_events) in &by_store {
            // Skip if not assigned to this worker
            if !self.is_assigned_store(&store_id) {
                continue;
            }

            let store_events: Vec<_> = store_events.collect();

            // Batch analyze impact
            let affected_checks = self.analyze_impact(&store_id, &store_events).await?;

            // Pre-compute in parallel with bounded concurrency
            let results = stream::iter(affected_checks)
                .map(|check| self.compute_and_cache(check))
                .buffer_unordered(self.config.max_concurrency)
                .collect::<Vec<_>>()
                .await;

            // Log any errors but don't fail the batch
            for result in results {
                if let Err(e) = result {
                    tracing::warn!("Pre-computation failed: {}", e);
                    self.metrics.precompute_errors.inc();
                }
            }
        }

        // ACK all messages
        for msg in messages {
            msg.ack().await?;
        }

        Ok(())
    }

    /// Analyze which checks are affected by tuple changes
    async fn analyze_impact(
        &self,
        store_id: &str,
        events: &[TupleChangeEvent],
    ) -> Result<Vec<CheckToCompute>> {
        let mut affected = Vec::new();

        for event in events {
            for change in &event.changes {
                match change {
                    TupleChange::Write { user, relation, object, .. } |
                    TupleChange::Delete { user, relation, object } => {
                        // Direct check is always affected
                        affected.push(CheckToCompute {
                            store_id: store_id.to_string(),
                            user: user.clone(),
                            relation: relation.clone(),
                            object: object.clone(),
                        });

                        // Reverse expand to find other affected checks
                        let expanded = self.reverse_expand(
                            store_id,
                            user,
                            relation,
                            object,
                        ).await?;

                        affected.extend(expanded);
                    }
                }
            }
        }

        // Deduplicate
        affected.sort();
        affected.dedup();

        Ok(affected)
    }

    /// Reverse expand to find checks affected by a tuple change
    ///
    /// IMPORTANT: This can be expensive for relations referenced by types with
    /// many objects. We mitigate this by:
    /// 1. Limiting expansion to bounded types (configured via `unbounded_types`)
    /// 2. Capping the number of affected checks per expansion
    /// 3. For unbounded relations, we skip pre-computation and rely on cache-on-read
    async fn reverse_expand(
        &self,
        store_id: &str,
        user: &str,
        relation: &str,
        object: &str,
    ) -> Result<Vec<CheckToCompute>> {
        let mut affected = Vec::new();

        // Get authorization model
        let model = self.resolver.get_model(store_id).await?;

        // Find relations that reference this relation (computed usersets)
        for (type_name, type_def) in &model.types {
            // Skip unbounded types (e.g., 'user', 'file') - these would cause
            // catastrophic expansion. Instead, rely on cache-on-read for these.
            if self.config.unbounded_types.contains(type_name) {
                tracing::debug!(
                    type_name = %type_name,
                    "Skipping pre-computation for unbounded type"
                );
                continue;
            }

            for (rel_name, rel_def) in &type_def.relations {
                if self.relation_references(rel_def, relation) {
                    // Query with limit to prevent runaway expansion
                    let objects = self.db_pool
                        .query_objects_by_type_limited(
                            store_id,
                            type_name,
                            self.config.max_objects_per_type, // e.g., 10,000
                        )
                        .await?;

                    if objects.len() >= self.config.max_objects_per_type {
                        tracing::warn!(
                            type_name = %type_name,
                            limit = self.config.max_objects_per_type,
                            "Type has too many objects, pre-computation truncated"
                        );
                    }

                    for obj in objects {
                        affected.push(CheckToCompute {
                            store_id: store_id.to_string(),
                            user: user.to_string(),
                            relation: rel_name.clone(),
                            object: obj,
                        });
                    }
                }
            }
        }

        // Find TTU relations that inherit through this object
        let ttu_affected = self.expand_ttu_impact(store_id, object, &model).await?;
        affected.extend(ttu_affected);

        // If this is a group membership change, use Leopard optimization
        if self.config.enable_leopard_optimization && self.is_group_membership(object, relation) {
            let group_affected = self.expand_group_impact(store_id, user, object).await?;
            affected.extend(group_affected);
        }

        // Final safety cap
        if affected.len() > self.config.max_affected_checks {
            tracing::warn!(
                count = affected.len(),
                limit = self.config.max_affected_checks,
                "Too many affected checks, truncating"
            );
            affected.truncate(self.config.max_affected_checks);
        }

        Ok(affected)
    }

    /// Compute a check and store in Valkey with retry logic
    ///
    /// Uses exponential backoff for transient failures:
    /// - Resolver errors: No retry (likely permanent - bad model, invalid check)
    /// - Valkey errors: Retry with backoff (transient network/connection issues)
    /// - NATS errors: Retry with backoff (transient, but non-critical)
    async fn compute_and_cache(&self, check: CheckToCompute) -> Result<()> {
        let start = Instant::now();

        // Compute the check result (no retry - resolver errors are usually permanent)
        let result = match self.resolver.check(
            &check.store_id,
            &check.user,
            &check.relation,
            &check.object,
        ).await {
            Ok(r) => r,
            Err(e) => {
                self.metrics.precompute_errors.with_label_values(&["resolver"]).inc();
                return Err(e);
            }
        };

        // Store in Valkey (L2) with retry
        let cache_key = format!(
            "check:{}:{}#{}@{}",
            check.store_id,
            check.object,
            check.relation,
            check.user,
        );

        let model_id = self.resolver.get_model_id(&check.store_id).await?;
        let cache_value = PrecomputedCheck {
            allowed: result,
            computed_at: Utc::now(),
            model_id,
        };

        let serialized = serde_json::to_string(&cache_value)?;

        // Retry Valkey write with exponential backoff
        let mut attempt = 0;
        let max_retries = 3;
        let mut backoff = Duration::from_millis(100);

        loop {
            match self.valkey
                .set_ex(
                    &cache_key,
                    &serialized,
                    self.config.cache_ttl.as_secs() as usize,
                )
                .await
            {
                Ok(_) => break,
                Err(e) => {
                    attempt += 1;
                    if attempt >= max_retries {
                        self.metrics.precompute_errors.with_label_values(&["valkey"]).inc();
                        tracing::error!(
                            cache_key = %cache_key,
                            attempts = attempt,
                            error = %e,
                            "Failed to write to Valkey after retries"
                        );
                        return Err(e.into());
                    }
                    tracing::warn!(
                        cache_key = %cache_key,
                        attempt = attempt,
                        backoff_ms = backoff.as_millis(),
                        error = %e,
                        "Valkey write failed, retrying"
                    );
                    tokio::time::sleep(backoff).await;
                    backoff *= 2; // Exponential backoff: 100ms, 200ms, 400ms
                }
            }
        }

        // Invalidate hot cache (L1) across all API pods
        // Non-critical: if this fails, L1 will eventually expire via TTL
        // Retry once, then log and continue
        let invalidation_result = self.nats
            .publish(
                format!("rsfga.cache.invalidate.{}", check.store_id),
                serde_json::to_vec(&CacheInvalidationEvent::Check {
                    cache_key: cache_key.clone(),
                })?.into(),
            )
            .await;

        if let Err(e) = invalidation_result {
            // Retry once
            tokio::time::sleep(Duration::from_millis(50)).await;
            if let Err(e2) = self.nats
                .publish(
                    format!("rsfga.cache.invalidate.{}", check.store_id),
                    serde_json::to_vec(&CacheInvalidationEvent::Check {
                        cache_key: cache_key.clone(),
                    })?.into(),
                )
                .await
            {
                // Log but don't fail - L1 will expire via TTL
                self.metrics.precompute_errors.with_label_values(&["nats_invalidation"]).inc();
                tracing::warn!(
                    cache_key = %cache_key,
                    error = %e2,
                    original_error = %e,
                    "Failed to broadcast L1 invalidation (non-fatal)"
                );
            }
        }

        // Update metrics
        self.metrics.precompute_duration.observe(start.elapsed().as_secs_f64());
        self.metrics.precompute_total.inc();

        Ok(())
    }

    /// Handle model change - invalidate all cached checks for the store
    ///
    /// NOTE: We intentionally avoid SCAN/DEL operations on Valkey as they can
    /// block the server for extended periods on large keystores. Instead, we:
    /// 1. Update the model_version key - readers check this and reject stale entries
    /// 2. Broadcast L1 hot cache invalidation via NATS
    /// 3. Let stale L2 entries naturally expire via TTL (1 hour)
    ///
    /// This "lazy invalidation" approach trades slightly higher memory usage
    /// for significantly better availability during model changes.
    async fn process_model_events(&self, messages: Vec<Message>) -> Result<()> {
        for msg in &messages {
            let event: ModelChangeEvent = serde_json::from_slice(&msg.payload)?;

            // Update model version in Valkey - readers will reject entries
            // with mismatched model_id (see CachedCheckResolver::check)
            let model_key = format!("model_version:{}", event.store_id);
            self.valkey
                .set::<_, _, ()>(&model_key, &event.new_model_id)
                .await?;

            // Broadcast hot cache invalidation to all API pods
            // L1 caches are small enough that full invalidation is fast
            self.nats
                .publish(
                    format!("rsfga.cache.invalidate.{}", event.store_id),
                    serde_json::to_vec(&CacheInvalidationEvent::Store {
                        store_id: event.store_id.clone(),
                    })?.into(),
                )
                .await?;

            tracing::info!(
                store_id = %event.store_id,
                old_model = ?event.old_model_id,
                new_model = %event.new_model_id,
                "Updated model version, stale cache entries will be rejected on read"
            );
        }

        // ACK all messages
        for msg in messages {
            msg.ack().await?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CheckToCompute {
    store_id: String,
    user: String,
    relation: String,
    object: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct PrecomputedCheck {
    allowed: bool,
    computed_at: DateTime<Utc>,
    model_id: String,
}
```

### 4.2 Leopard Optimization for Groups

```rust
impl PrecomputeWorker {
    /// Expand impact of group membership changes (Leopard-style)
    async fn expand_group_impact(
        &self,
        store_id: &str,
        user: &str,
        group: &str,
    ) -> Result<Vec<CheckToCompute>> {
        let mut affected = Vec::new();

        // Get all objects that grant access via this group
        // e.g., document:*#viewer@group:engineering#member
        let objects_with_group = self.db_pool
            .query("
                SELECT DISTINCT object_type, object_id, relation
                FROM tuples
                WHERE store_id = $1
                  AND user_type = 'group'
                  AND user_id = $2
            ", &[&store_id, &group])
            .await?;

        for row in objects_with_group {
            let object = format!("{}:{}", row.object_type, row.object_id);
            affected.push(CheckToCompute {
                store_id: store_id.to_string(),
                user: user.to_string(),
                relation: row.relation,
                object,
            });
        }

        // Get transitive group memberships (group contains other groups)
        let parent_groups = self.get_parent_groups(store_id, group).await?;

        for parent in parent_groups {
            // Recursively expand impact for parent groups
            let parent_affected = self.expand_group_impact(store_id, user, &parent).await?;
            affected.extend(parent_affected);
        }

        Ok(affected)
    }

    /// Pre-compute transitive group closure (Leopard core algorithm)
    async fn compute_group_closure(
        &self,
        store_id: &str,
        group: &str,
    ) -> Result<HashSet<String>> {
        let mut closure = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(group.to_string());

        while let Some(current) = queue.pop_front() {
            if closure.contains(&current) {
                continue; // Already processed (cycle detection)
            }
            closure.insert(current.clone());

            // Get direct child groups
            let children = self.db_pool
                .query("
                    SELECT user_id
                    FROM tuples
                    WHERE store_id = $1
                      AND object_type = 'group'
                      AND object_id = $2
                      AND relation = 'member'
                      AND user_type = 'group'
                ", &[&store_id, &current])
                .await?;

            for child in children {
                if !closure.contains(&child.user_id) {
                    queue.push_back(child.user_id);
                }
            }
        }

        Ok(closure)
    }

    /// Store group transitive closure in Valkey for fast lookups
    async fn cache_group_closure(
        &self,
        store_id: &str,
        group: &str,
        closure: &HashSet<String>,
    ) -> Result<()> {
        let key = format!("group_closure:{}:{}", store_id, group);

        // Use Valkey SET for efficient membership checks
        let mut pipe = valkey::pipe();
        pipe.del(&key);
        for member in closure {
            pipe.sadd(&key, member);
        }
        pipe.expire(&key, self.config.cache_ttl.as_secs() as usize);

        pipe.query_async(&mut self.valkey.clone()).await?;

        Ok(())
    }
}
```

### 4.3 In-Memory Hot Cache (L1)

The in-memory hot cache is critical for achieving sub-millisecond latency on frequently accessed check results. This cache lives in each API pod and uses Moka's TinyLFU algorithm to automatically identify and retain hot data.

```rust
use moka::future::Cache;
use std::time::Duration;

/// Hot cache configuration optimized for frequently accessed checks
pub struct HotCacheConfig {
    /// Maximum number of entries (not bytes - entries are small)
    pub max_entries: u64,

    /// Time-to-live for entries
    pub ttl: Duration,

    /// Time-to-idle - evict if not accessed within this window
    pub tti: Duration,
}

impl Default for HotCacheConfig {
    fn default() -> Self {
        Self {
            max_entries: 100_000,      // ~10MB assuming 100 bytes/entry
            ttl: Duration::from_secs(60),   // 1 minute max lifetime
            tti: Duration::from_secs(10),   // Evict if idle for 10s
        }
    }
}

/// Build the hot cache with TinyLFU eviction
pub fn build_hot_cache(config: &HotCacheConfig) -> Cache<String, CachedCheckResult> {
    Cache::builder()
        .max_capacity(config.max_entries)
        .time_to_live(config.ttl)
        .time_to_idle(config.tti)
        // TinyLFU: Frequency + recency based eviction
        // Automatically identifies and retains hot data
        .build()
}

/// Cached check result with metadata
#[derive(Clone, Debug)]
pub struct CachedCheckResult {
    pub allowed: bool,
    pub model_id: String,
    pub cached_at: Instant,
}
```

**Why TinyLFU for Hot Data:**
- Combines frequency (how often accessed) with recency (when last accessed)
- Automatically evicts cold data while retaining hot data
- No manual "hot list" management required
- O(1) lookup with minimal memory overhead

**Hot Data Characteristics:**
- Repeatedly checked permissions (e.g., `user:system#admin` on `org:default`)
- High-traffic user/object combinations
- Service accounts with broad permissions
- Recently computed results likely to be checked again

### 4.4 API Server Integration

```rust
/// Check resolver that uses pre-computed cache
pub struct CachedCheckResolver {
    /// Valkey connection for pre-computed results
    valkey: ConnectionManager,

    /// Fallback graph resolver
    resolver: Arc<GraphResolver>,

    /// NATS client for publishing invalidations
    nats: async_nats::Client,

    /// Local hot cache (L1) - per-pod, frequency-based retention
    hot_cache: Cache<String, CachedCheckResult>,

    /// Hot cache config
    hot_cache_config: HotCacheConfig,

    /// Metrics
    metrics: CheckMetrics,
}

impl CachedCheckResolver {
    pub async fn check(
        &self,
        store_id: &str,
        user: &str,
        relation: &str,
        object: &str,
    ) -> Result<bool> {
        let cache_key = format!("check:{}:{}#{}@{}", store_id, object, relation, user);
        let start = Instant::now();

        // ┌─────────────────────────────────────────────────────────────┐
        // │ L1: In-Memory Hot Cache (sub-microsecond)                   │
        // │ - Per-pod local cache using Moka TinyLFU                    │
        // │ - Automatically retains frequently accessed (hot) data      │
        // │ - ~100K entries, 60s TTL, 10s TTI                           │
        // └─────────────────────────────────────────────────────────────┘
        if let Some(cached) = self.hot_cache.get(&cache_key).await {
            // Verify model hasn't changed (defense-in-depth: hot cache also
            // subscribes to NATS invalidation via start_invalidation_listener(),
            // but model version check provides additional consistency guarantee)
            let current_model = self.resolver.get_model_id(store_id).await?;
            if cached.model_id == current_model {
                self.metrics.cache_hits.with_label_values(&["l1_hot"]).inc();
                self.metrics.check_latency.with_label_values(&["l1_hot"])
                    .observe(start.elapsed().as_secs_f64());
                return Ok(cached.allowed);
            }
            // Model changed - invalidate this entry and fall through
            self.hot_cache.invalidate(&cache_key).await;
        }

        // ┌─────────────────────────────────────────────────────────────┐
        // │ L2: Valkey Distributed Cache (~100-500μs)                   │
        // │ - Shared across all API pods                                │
        // │ - Pre-computed by workers, or populated on cache miss       │
        // │ - 1 hour TTL                                                │
        // └─────────────────────────────────────────────────────────────┘
        if let Some(cached) = self.valkey.get::<_, Option<String>>(&cache_key).await? {
            let precomputed: PrecomputedCheck = serde_json::from_str(&cached)?;

            // Verify model version is current
            let current_model = self.resolver.get_model_id(store_id).await?;
            if precomputed.model_id == current_model {
                // Promote to hot cache (TinyLFU will decide if it stays)
                self.hot_cache.insert(
                    cache_key.clone(),
                    CachedCheckResult {
                        allowed: precomputed.allowed,
                        model_id: precomputed.model_id,
                        cached_at: Instant::now(),
                    },
                ).await;

                self.metrics.cache_hits.with_label_values(&["l2_valkey"]).inc();
                self.metrics.check_latency.with_label_values(&["l2_valkey"])
                    .observe(start.elapsed().as_secs_f64());
                return Ok(precomputed.allowed);
            }
            // Model changed, cache is stale - fall through
        }

        // ┌─────────────────────────────────────────────────────────────┐
        // │ L3: Graph Resolution (1-10ms)                               │
        // │ - Full database-backed resolution                           │
        // │ - Async populate both L1 and L2 caches                      │
        // └─────────────────────────────────────────────────────────────┘
        self.metrics.cache_misses.inc();
        let result = self.resolver.check(store_id, user, relation, object).await?;
        let model_id = self.resolver.get_model_id(store_id).await?;

        self.metrics.check_latency.with_label_values(&["l3_resolver"])
            .observe(start.elapsed().as_secs_f64());

        // Populate hot cache immediately (this check was just requested, likely hot)
        self.hot_cache.insert(
            cache_key.clone(),
            CachedCheckResult {
                allowed: result,
                model_id: model_id.clone(),
                cached_at: Instant::now(),
            },
        ).await;

        // Async populate Valkey (don't block response)
        let valkey = self.valkey.clone();
        let cache_key_clone = cache_key.clone();
        tokio::spawn(async move {
            let value = PrecomputedCheck {
                allowed: result,
                computed_at: Utc::now(),
                model_id,
            };
            let _ = valkey.set_ex::<_, _, ()>(
                &cache_key_clone,
                serde_json::to_string(&value).unwrap(),
                3600, // 1 hour TTL
            ).await;
        });

        Ok(result)
    }

    /// Write tuples and publish change event
    pub async fn write_tuples(
        &self,
        store_id: &str,
        writes: Vec<TupleWrite>,
        deletes: Vec<TupleKey>,
    ) -> Result<()> {
        // Write to database
        self.resolver.write_tuples(store_id, &writes, &deletes).await?;

        // Publish change event to NATS
        let event = TupleChangeEvent {
            event_id: Uuid::new_v4(),
            store_id: store_id.to_string(),
            timestamp: Utc::now(),
            changes: writes.iter()
                .map(|w| TupleChange::Write {
                    user: w.user.clone(),
                    relation: w.relation.clone(),
                    object: w.object.clone(),
                    condition: w.condition.clone(),
                })
                .chain(deletes.iter().map(|d| TupleChange::Delete {
                    user: d.user.clone(),
                    relation: d.relation.clone(),
                    object: d.object.clone(),
                }))
                .collect(),
            model_id: self.resolver.get_model_id(store_id).await?,
        };

        self.nats
            .publish(
                format!("rsfga.tuples.{}", store_id),
                serde_json::to_vec(&event)?.into(),
            )
            .await?;

        Ok(())
    }

    /// Subscribe to NATS for hot cache invalidation
    /// Each API pod subscribes to invalidation events
    pub async fn start_invalidation_listener(&self) -> Result<()> {
        let subscriber = self.nats
            .subscribe("rsfga.cache.invalidate.>")
            .await?;

        let hot_cache = self.hot_cache.clone();

        tokio::spawn(async move {
            while let Some(msg) = subscriber.next().await {
                if let Ok(event) = serde_json::from_slice::<CacheInvalidationEvent>(&msg.payload) {
                    match event {
                        CacheInvalidationEvent::Check { cache_key } => {
                            hot_cache.invalidate(&cache_key).await;
                        }
                        CacheInvalidationEvent::Pattern { store_id, pattern } => {
                            // Invalidate all entries matching pattern
                            // Moka doesn't support pattern invalidation, so we track keys
                            hot_cache.invalidate_entries_if(move |key, _| {
                                key.starts_with(&format!("check:{}:", store_id))
                                    && key.contains(&pattern)
                            });
                        }
                        CacheInvalidationEvent::Store { store_id } => {
                            // Invalidate all entries for a store (model change)
                            hot_cache.invalidate_entries_if(move |key, _| {
                                key.starts_with(&format!("check:{}:", store_id))
                            });
                        }
                    }
                }
            }
        });

        Ok(())
    }
}

/// Events for invalidating hot cache across pods
#[derive(Debug, Serialize, Deserialize)]
pub enum CacheInvalidationEvent {
    /// Invalidate a specific check
    Check { cache_key: String },

    /// Invalidate checks matching a pattern (e.g., all for an object)
    Pattern { store_id: String, pattern: String },

    /// Invalidate all checks for a store (e.g., model change)
    Store { store_id: String },
}
```

### 4.5 Cache Tier Summary

```text
┌─────────────────────────────────────────────────────────────────────────────┐
│                           Cache Tier Architecture                            │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  ┌─────────────────────────────────────────────────────────────────────┐    │
│  │ L1: In-Memory Hot Cache (per API pod)                               │    │
│  │ ┌─────────────────────────────────────────────────────────────────┐ │    │
│  │ │ • Latency: <1μs                                                 │ │    │
│  │ │ • Capacity: ~100K entries (~10MB)                               │ │    │
│  │ │ • Algorithm: TinyLFU (frequency + recency)                      │ │    │
│  │ │ • TTL: 60s, TTI: 10s                                            │ │    │
│  │ │ • Scope: Hot/frequently accessed checks only                    │ │    │
│  │ │ • Invalidation: NATS subscription per pod                       │ │    │
│  │ └─────────────────────────────────────────────────────────────────┘ │    │
│  └─────────────────────────────────────────────────────────────────────┘    │
│                                    │                                         │
│                                    ▼ miss                                    │
│  ┌─────────────────────────────────────────────────────────────────────┐    │
│  │ L2: Valkey Distributed Cache (shared across pods)                   │    │
│  │ ┌─────────────────────────────────────────────────────────────────┐ │    │
│  │ │ • Latency: 100-500μs                                            │ │    │
│  │ │ • Capacity: 2GB+ (cluster)                                      │ │    │
│  │ │ • Algorithm: LRU                                                │ │    │
│  │ │ • TTL: 1 hour                                                   │ │    │
│  │ │ • Scope: All pre-computed check results                         │ │    │
│  │ │ • Population: Workers (proactive) + API (reactive)              │ │    │
│  │ └─────────────────────────────────────────────────────────────────┘ │    │
│  └─────────────────────────────────────────────────────────────────────┘    │
│                                    │                                         │
│                                    ▼ miss                                    │
│  ┌─────────────────────────────────────────────────────────────────────┐    │
│  │ L3: Graph Resolver (PostgreSQL)                                     │    │
│  │ ┌─────────────────────────────────────────────────────────────────┐ │    │
│  │ │ • Latency: 1-10ms                                               │ │    │
│  │ │ • Capacity: Unlimited (database)                                │ │    │
│  │ │ • Always consistent (source of truth)                           │ │    │
│  │ │ • Populates L1 and L2 on resolution                             │ │    │
│  │ └─────────────────────────────────────────────────────────────────┘ │    │
│  └─────────────────────────────────────────────────────────────────────┘    │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

**Hot Data Flow:**
1. First request → L3 (resolver) → populate L1 + L2
2. Subsequent requests → L1 hit (sub-microsecond) if hot
3. TinyLFU automatically evicts cold data, retains hot data
4. On tuple change → workers update L2 → broadcast invalidate to L1

### 4.6 Write Amplification Analysis

Write amplification is a critical concern for pre-computation systems. A single tuple write can trigger re-computation of many cached check results. This section quantifies the amplification factors and mitigation strategies.

#### Amplification Scenarios

**Scenario 1: Direct Assignment (Low Amplification)**
```text
Tuple: user:alice → viewer → document:readme

Affected checks: 1
  - (user:alice, viewer, document:readme)

Amplification factor: 1x
```

**Scenario 2: Group Membership (Moderate Amplification)**
```text
Tuple: user:alice → member → group:engineering

If group:engineering grants access to 100 documents:
Affected checks: 100
  - (user:alice, viewer, document:1)
  - (user:alice, viewer, document:2)
  - ...

Amplification factor: 100x
```

**Scenario 3: Nested Group Hierarchy (High Amplification)**
```text
Tuple: user:alice → member → group:engineering

Group hierarchy:
  group:engineering ∈ group:product ∈ group:company

If each group grants access to objects:
  - group:engineering → 50 documents
  - group:product → 200 folders
  - group:company → 500 resources

Affected checks: 750
Amplification factor: 750x
```

**Scenario 4: TTU with Popular Parent (Very High Amplification)**
```text
Tuple: user:alice → owner → folder:root

If folder:root contains 10,000 documents with:
  type document { viewer: [user] or owner from parent }

Affected checks: 10,000
Amplification factor: 10,000x
```

#### Quantified Bounds

| Scenario | Typical Amplification | Max Bounded |
|----------|----------------------|-------------|
| Direct assignment | 1x | 1x |
| Flat group membership | 10-100x | max_objects_per_type |
| Nested groups (depth 3) | 100-1,000x | depth × max_objects_per_type |
| TTU inheritance | 1,000-100,000x | max_affected_checks |
| Unbounded type (skipped) | ∞ (not computed) | 0 (rely on cache-on-read) |

#### Mitigation Summary

1. **Unbounded type exclusion**: Types like `user`, `file` with millions of instances are skipped entirely; rely on cache-on-read instead of pre-computation.

2. **Per-type limits**: `max_objects_per_type` (default: 10,000) caps how many objects we query during reverse expansion.

3. **Total check limit**: `max_affected_checks` (default: 100,000) provides a hard cap on pre-computation work per tuple change.

4. **Backpressure**: NATS JetStream with `max_pending` prevents workers from being overwhelmed; HPA scales workers based on queue depth.

5. **Selective pre-computation**: Hot paths benefit from pre-computation; cold paths fall back to cache-on-read with acceptable latency.

#### Cost-Benefit Analysis

```text
Pre-computation cost:  O(amplification × check_resolution_time)
Pre-computation benefit: O(cache_hit_rate × query_volume × latency_savings)

Break-even when:
  cache_hit_rate × query_volume × (l3_latency - l2_latency) >
  amplification × check_resolution_time × write_frequency
```

For most workloads (read-heavy, moderate write rate), pre-computation is beneficial. For write-heavy workloads with high amplification, consider:
- Disabling pre-computation for specific types
- Increasing TTL to reduce re-computation frequency
- Using cache-on-read only for high-amplification relations

### 4.7 Consistency Model

This section clarifies the consistency guarantees provided by the pre-computation architecture.

#### Consistency Level: Eventual Consistency

The system provides **eventual consistency** (per A9: 1-10ms staleness acceptable). This means:
- Reads may return stale results for a brief window after writes
- All caches eventually converge to the correct state
- No read-your-writes guarantee at the cache level

#### Read-Your-Writes Guarantee

**The system does NOT provide read-your-writes at the cache layer.** After writing a tuple, immediate reads may return stale cached results until:
1. The pre-computation worker processes the event (~10-100ms)
2. L1 hot cache receives invalidation via NATS (~1-10ms)
3. L2 Valkey entry is updated or expires

**Workaround for strong consistency needs:**

```rust
impl CachedCheckResolver {
    /// Check with read-your-writes guarantee (bypasses cache)
    pub async fn check_consistent(
        &self,
        store_id: &str,
        user: &str,
        relation: &str,
        object: &str,
    ) -> Result<bool> {
        // Skip L1 and L2, go directly to L3 (source of truth)
        self.resolver.check(store_id, user, relation, object).await
    }

    /// Write tuples with optional consistency token
    pub async fn write_tuples_with_token(
        &self,
        store_id: &str,
        writes: Vec<TupleWrite>,
        deletes: Vec<TupleKey>,
    ) -> Result<ConsistencyToken> {
        self.write_tuples(store_id, writes, deletes).await?;

        // Return token that can be used to ensure subsequent reads
        // see at least this write
        Ok(ConsistencyToken {
            event_id: Uuid::new_v4(),
            timestamp: Utc::now(),
        })
    }

    /// Check with consistency token (waits for pre-computation)
    pub async fn check_after(
        &self,
        store_id: &str,
        user: &str,
        relation: &str,
        object: &str,
        token: &ConsistencyToken,
    ) -> Result<bool> {
        // Wait until pre-computation has caught up to this token
        self.wait_for_watermark(store_id, &token.event_id).await?;

        // Now safe to read from cache
        self.check(store_id, user, relation, object).await
    }
}
```

#### Model Change Atomicity

Model changes are **atomic at the store level**:

1. New model is written to PostgreSQL
2. `ModelChangeEvent` is published to NATS
3. Worker updates `model_version:{store_id}` in Valkey
4. Worker broadcasts `CacheInvalidationEvent::Store` via NATS
5. All API pods invalidate L1 entries for the store

**Atomicity guarantee**: All cached entries for a store are invalidated together. There is no window where some entries use the old model while others use the new model (within a single store).

**Cross-store consistency**: Not guaranteed. Each store's model version is tracked independently.

#### Staleness Windows

| Cache Tier | Staleness Window | Mechanism |
|------------|------------------|-----------|
| L1 Hot Cache | 1-10ms | NATS invalidation broadcast |
| L2 Valkey | 10-100ms | Worker pre-computation + version check |
| L3 PostgreSQL | 0ms | Source of truth |

#### Failure Mode Consistency

| Failure | Consistency Impact | Recovery |
|---------|-------------------|----------|
| Worker crash | Stale cache until TTL or restart | NATS redelivers unacked events |
| NATS partition | L1 may serve stale data | Falls back to L2/L3 on version mismatch |
| Valkey failure | All reads go to L3 (consistent) | Graceful degradation |
| PostgreSQL failure | System unavailable | No consistency violation |

#### Tuning for Stronger Consistency

For applications requiring stronger guarantees:

```yaml
# Shorter TTLs reduce staleness window
hot_cache:
  ttl: 10s    # Instead of 60s
  tti: 2s     # Instead of 10s

valkey:
  cache_ttl: 300s  # Instead of 3600s

# Or use check_consistent() for critical paths
```

---

## 5. Valkey Cache Schema

### 5.1 Key Patterns

```text
# Pre-computed check results
check:{store_id}:{object}#{relation}@{user}
  → JSON: { "allowed": bool, "computed_at": timestamp, "model_id": string }
  → TTL: 1 hour (configurable)

# Group transitive closure (Leopard optimization)
group_closure:{store_id}:{group_id}
  → SET: { member_group_1, member_group_2, ... }
  → TTL: 1 hour

# User's direct group memberships
user_groups:{store_id}:{user_id}
  → SET: { group_1, group_2, ... }
  → TTL: 1 hour

# Model version tracking (for cache invalidation)
model_version:{store_id}
  → STRING: model_id
  → TTL: none (updated on model change)

# Pre-computation watermark (for consistency)
precompute_watermark:{store_id}
  → STRING: event_id
  → TTL: none
```

### 5.2 Valkey Configuration

```yaml
# valkey.conf
maxmemory 2gb
maxmemory-policy allkeys-lru    # Evict any key using LRU
appendonly yes                   # Persistence
appendfsync everysec             # Async persistence
cluster-enabled yes              # Cluster mode
cluster-node-timeout 5000
```

---

## 6. Deployment

### 6.1 Docker Compose (Local Development)

```yaml
# docker-compose.yml
version: '3.8'

services:
  # RSFGA API Server
  rsfga-api:
    build:
      context: .
      dockerfile: Dockerfile
    ports:
      - "8080:8080"   # HTTP
      - "8081:8081"   # gRPC
    environment:
      - DATABASE_URL=postgres://rsfga:rsfga@postgres:5432/rsfga
      - VALKEY_URL=redis://valkey:6379
      - NATS_URL=nats://nats:4222
      - RUST_LOG=info
    depends_on:
      - postgres
      - valkey
      - nats
    deploy:
      replicas: 2

  # Pre-computation Worker
  rsfga-worker:
    build:
      context: .
      dockerfile: Dockerfile.worker
    environment:
      - DATABASE_URL=postgres://rsfga:rsfga@postgres:5432/rsfga
      - VALKEY_URL=redis://valkey:6379
      - NATS_URL=nats://nats:4222
      - WORKER_BATCH_SIZE=100
      - WORKER_CONCURRENCY=10
      - RUST_LOG=info
    depends_on:
      - postgres
      - valkey
      - nats
    deploy:
      replicas: 2

  # PostgreSQL
  postgres:
    image: postgres:16
    environment:
      - POSTGRES_DB=rsfga
      - POSTGRES_USER=rsfga
      - POSTGRES_PASSWORD=rsfga
    ports:
      - "5432:5432"
    volumes:
      - postgres_data:/var/lib/postgresql/data

  # Valkey (Redis-compatible)
  valkey:
    image: valkey/valkey:7.2
    ports:
      - "6379:6379"
    volumes:
      - valkey_data:/data
    command: valkey-server --appendonly yes

  # NATS with JetStream
  nats:
    image: nats:2.10
    ports:
      - "4222:4222"   # Client
      - "8222:8222"   # HTTP monitoring
    command: -js -sd /data
    volumes:
      - nats_data:/data

volumes:
  postgres_data:
  valkey_data:
  nats_data:
```

### 6.2 Kubernetes (Production)

```yaml
# k8s/rsfga-api.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: rsfga-api
  labels:
    app: rsfga
    component: api
spec:
  replicas: 3
  selector:
    matchLabels:
      app: rsfga
      component: api
  template:
    metadata:
      labels:
        app: rsfga
        component: api
    spec:
      containers:
      - name: rsfga-api
        image: rsfga/rsfga-api:latest
        ports:
        - containerPort: 8080
          name: http
        - containerPort: 8081
          name: grpc
        env:
        - name: DATABASE_URL
          valueFrom:
            secretKeyRef:
              name: rsfga-secrets
              key: database-url
        - name: VALKEY_URL
          value: "redis://valkey-master:6379"
        - name: NATS_URL
          value: "nats://nats:4222"
        resources:
          requests:
            memory: "256Mi"
            cpu: "250m"
          limits:
            memory: "512Mi"
            cpu: "1000m"
        readinessProbe:
          httpGet:
            path: /health
            port: 8080
          initialDelaySeconds: 5
          periodSeconds: 10
        livenessProbe:
          httpGet:
            path: /health
            port: 8080
          initialDelaySeconds: 15
          periodSeconds: 20
---
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: rsfga-api-hpa
spec:
  scaleTargetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: rsfga-api
  minReplicas: 3
  maxReplicas: 20
  metrics:
  - type: Resource
    resource:
      name: cpu
      target:
        type: Utilization
        averageUtilization: 70
  - type: Resource
    resource:
      name: memory
      target:
        type: Utilization
        averageUtilization: 80
```

```yaml
# k8s/rsfga-worker.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: rsfga-worker
  labels:
    app: rsfga
    component: worker
spec:
  replicas: 2
  selector:
    matchLabels:
      app: rsfga
      component: worker
  template:
    metadata:
      labels:
        app: rsfga
        component: worker
    spec:
      containers:
      - name: rsfga-worker
        image: rsfga/rsfga-worker:latest
        env:
        - name: DATABASE_URL
          valueFrom:
            secretKeyRef:
              name: rsfga-secrets
              key: database-url
        - name: VALKEY_URL
          value: "redis://valkey-master:6379"
        - name: NATS_URL
          value: "nats://nats:4222"
        - name: WORKER_BATCH_SIZE
          value: "100"
        - name: WORKER_CONCURRENCY
          value: "20"
        - name: ENABLE_LEOPARD_OPTIMIZATION
          value: "true"
        resources:
          requests:
            memory: "512Mi"
            cpu: "500m"
          limits:
            memory: "2Gi"
            cpu: "2000m"
---
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: rsfga-worker-hpa
spec:
  scaleTargetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: rsfga-worker
  minReplicas: 2
  maxReplicas: 10
  metrics:
  - type: External
    external:
      metric:
        name: nats_consumer_pending_messages
        selector:
          matchLabels:
            consumer: precompute-workers
      target:
        type: AverageValue
        averageValue: "1000"
```

---

## 7. Implementation Roadmap

### Phase 2a: Event Infrastructure (2 weeks)

| Week | Tasks |
|------|-------|
| 1 | NATS JetStream setup, event schema definition, stream configuration |
| 2 | Event publishing in API server, integration tests |

### Phase 2b: Pre-computation Worker (4 weeks)

| Week | Tasks |
|------|-------|
| 1 | Worker skeleton, NATS consumer, basic event processing |
| 2 | Impact analysis (reverse expansion), affected check identification |
| 3 | Parallel pre-computation, Valkey storage, batch processing |
| 4 | Leopard optimization for groups, transitive closure |

### Phase 2c: API Integration (2 weeks)

| Week | Tasks |
|------|-------|
| 1 | CachedCheckResolver, tiered cache lookup |
| 2 | Cache invalidation on model change, consistency checks |

### Phase 2d: Deployment & Operations (2 weeks)

| Week | Tasks |
|------|-------|
| 1 | Docker Compose, Kubernetes manifests, Helm chart |
| 2 | Monitoring, alerting, runbooks, load testing |

---

## 8. Validation Criteria

### Performance

- [ ] Check latency (cache hit): <1ms p99
- [ ] Check latency (cache miss): <10ms p99
- [ ] Pre-computation lag: <100ms p99 (event to cache)
- [ ] Write throughput: >500 tuples/sec sustained
- [ ] Worker throughput: >10K pre-computations/sec per worker

### Scalability

- [ ] Linear scaling with worker replicas
- [ ] API pods stateless, scale to 20+ replicas
- [ ] NATS handles >100K events/sec
- [ ] Valkey cluster handles >1M ops/sec

### Reliability

- [ ] Zero data loss on worker crash (NATS redelivery)
- [ ] Graceful degradation on Valkey failure (fallback to DB)
- [ ] Model change invalidates stale cache entries

### Correctness

- [ ] Pre-computed results match graph resolver results (100%)
- [ ] Cache invalidation covers all affected checks
- [ ] Model version tracking prevents stale reads

---

## 9. Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Write amplification overwhelms workers | Medium | High | Backpressure, rate limiting, partitioning |
| NATS message loss | Low | High | JetStream persistence, replication, monitoring |
| Valkey memory exhaustion | Medium | Medium | LRU eviction, memory limits, TTLs |
| Pre-computation lag during burst | Medium | Low | HPA scaling, queue depth alerts |
| Stale cache returns wrong result | Low | High | Model version check, TTL, consistency modes |
| Complex model causes infinite expansion | Low | Medium | Depth limits, timeout, circuit breaker |

---

## 10. Decision

**Proceed with Leopard-inspired cloud-native architecture** as described:

1. **Event-driven**: NATS JetStream for tuple/model change events
2. **Distributed cache**: Valkey for pre-computed check results
3. **Horizontal scaling**: Separate worker pods, HPA-enabled
4. **Leopard optimization**: Transitive closure for group membership
5. **Graceful degradation**: Fallback to graph resolution on cache miss

**Key differences from original Leopard**:
- Valkey replaces in-memory skip lists (distributed, persistent)
- NATS replaces Watch API (cloud-native, JetStream durability)
- Kubernetes replaces dedicated infrastructure (standard deployment)
- Extended beyond groups to all relation types

**ADR to create**: ADR-017: Cloud-Native Pre-computation Architecture

---

## Sources

- [Zanzibar Paper (USENIX ATC'19)](https://www.usenix.org/system/files/atc19-pang.pdf)
- [AuthZed: Understanding Google Zanzibar](https://authzed.com/blog/what-is-google-zanzibar)
- [AuthZed: Zanzibar Overview](https://authzed.com/zanzibar)
- [SpiceDB Leopard Issue #129](https://github.com/authzed/spicedb/issues/129)
- [OpenFGA Performance Optimizations](https://deepwiki.com/openfga/openfga/2.3-performance-optimizations)
- [NATS JetStream Documentation](https://docs.nats.io/nats-concepts/jetstream)
- [Valkey Documentation](https://valkey.io/docs/)
