# Spike: Leopard Indexing for RSFGA Pre-computation

**Date**: 2026-01-12
**Author**: Claude (Research Spike)
**Status**: Research Complete
**Related**: Phase 2 (Precomputed Checks), ADR-TBD

---

## Executive Summary

Google's **Leopard** indexing system is a specialized pre-computation engine within Zanzibar that flattens group hierarchies into an in-memory transitive closure for sub-millisecond authorization lookups. After evaluating Leopard's architecture against RSFGA's Phase 2 requirements, **Leopard is partially suitable** with significant adaptations required.

### Recommendation

**Adopt Leopard's core principles with a modified architecture**:
1. Use Leopard's two-layer approach (offline index + online incremental)
2. Simplify data structures (skip lists → bloom filters + sorted vectors)
3. Scope narrowly to group membership resolution (not general relations)
4. Consider hybrid approach with existing cache layer

---

## 1. Leopard Architecture Overview

### 1.1 What is Leopard?

Leopard is a specialized indexing system that pre-computes the transitive closure of group memberships. It solves a specific problem: deeply nested or widely branching group hierarchies cause "pointer chasing" during check evaluation, requiring multiple serial database lookups.

### 1.2 Core Data Structures

Leopard uses two specialized set types:

```
GROUP2GROUP(ancestor) → {descendant groups}
MEMBER2GROUP(user) → {groups where user is a direct member}
```

To check if user U belongs to group G:
```
(MEMBER2GROUP(U) ∩ GROUP2GROUP(G)) ≠ ∅
```

This transforms graph traversal into a set intersection operation.

### 1.3 Three-Layer Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Leopard Serving System                     │
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

## 2. RSFGA Phase 2 Requirements

### 2.1 Current Phase 2 Design

From `docs/design/ARCHITECTURE.md`:

```
Write Operation
    ↓
Change Classification
    ↓
Impact Analysis (Reverse Expansion)
    ↓
Parallel Recomputation
    ↓
Result Distribution (Valkey)
```

### 2.2 Key Requirements

| Requirement | Current Phase 2 | Leopard |
|-------------|-----------------|---------|
| Check latency target | <1ms | <150μs ✓ |
| Scope | All relations | Group membership only |
| Storage | Valkey/Redis | In-memory |
| Update propagation | Async invalidation | Incremental index |
| Consistency | Eventual (acceptable) | Snapshot + incremental |

### 2.3 RSFGA-Specific Constraints

1. **C12**: Cache memory budget <500MB per node
2. **C7**: Memory usage <2x tuple storage size
3. **A9**: Users can tolerate 1-10ms cache staleness window
4. **C4**: Single-region deployment must work

---

## 3. Suitability Analysis

### 3.1 Where Leopard Fits Well

#### ✅ Group Membership Resolution

Leopard excels at the exact problem RSFGA faces with deeply nested groups:

```
# Example: Is user:alice in group:company#member?
# Without Leopard: 5+ database queries for each hop
user:alice → team:engineering#member → dept:product#member → org:tech#member → company#member

# With Leopard: 2 set lookups + 1 intersection
MEMBER2GROUP(user:alice) = {team:engineering, group:devs}
GROUP2GROUP(group:company) = {org:tech, dept:product, team:engineering, ...}
Intersection: team:engineering → TRUE
```

#### ✅ High Read/Low Write Ratio

RSFGA assumes infrequent model changes (A6: <1 per hour). Leopard's offline rebuild + incremental layer suits this pattern.

#### ✅ Deterministic Set Operations

Leopard's set-theoretic approach eliminates graph traversal nondeterminism and enables efficient caching.

### 3.2 Where Leopard Doesn't Fit

#### ❌ Write Amplification at Scale

**Critical Concern**: A single Zanzibar tuple addition can generate tens of thousands of Leopard tuple events.

```
# Adding user:bob to group:company
# Leopard must update:
# - MEMBER2GROUP(user:bob) = {group:company}
# - For every object granting access via company membership:
#   - Every computed relation referencing company
#   - Every TTU relation through company

# At Google scale: Acceptable (dedicated infrastructure)
# At RSFGA scale: May exceed memory constraints (C12)
```

#### ❌ General Relation Resolution

Leopard only handles group-to-group and member-to-group relationships. It cannot pre-compute:

- **TupleToUserset (TTU)**: `viewer: editor from parent`
- **Computed usersets**: `viewer: editor` (unless editor is a group)
- **Exclusion**: `viewer: all but banned`
- **Intersection**: `can_view: viewer and active`

#### ❌ Memory Requirements

Leopard keeps the entire transitive closure in memory. For RSFGA:

```
# Estimate for 1M tuples with 10% group membership
# Groups: 100K group relationships
# Transitive closure expansion: ~10x (conservative)
# Storage per tuple: ~100 bytes
# Total: 100K × 10 × 100 = 100MB (groups only)

# With full relation coverage: Exceeds C12 (500MB limit)
```

#### ❌ Offline Index Rebuild

Leopard's offline index builder requires periodic full rebuilds. This:
- Creates staleness windows during rebuild
- Requires dedicated build infrastructure
- Doesn't fit single-node deployment (C4)

---

## 4. Alternative Approaches

### 4.1 Hybrid Leopard-Lite

**Approach**: Implement Leopard principles for group membership only, use existing cache for other relations.

```rust
pub struct LeopardLite {
    // Group transitive closure (Leopard-style)
    group_to_groups: DashMap<GroupId, Arc<RoaringBitmap>>,
    member_to_groups: DashMap<UserId, Arc<RoaringBitmap>>,

    // Incremental update layer
    pending_updates: DashMap<GroupId, Vec<GroupUpdate>>,

    // Existing cache for non-group relations
    check_cache: Arc<CheckCache>,
}

impl LeopardLite {
    /// Check group membership: O(1) + intersection
    pub fn is_member(&self, user: &UserId, group: &GroupId) -> bool {
        if let (Some(user_groups), Some(group_groups)) = (
            self.member_to_groups.get(user),
            self.group_to_groups.get(group),
        ) {
            // Roaring bitmap intersection is very fast
            !user_groups.is_disjoint(group_groups)
        } else {
            false
        }
    }

    /// Handle tuple write with incremental update
    pub async fn on_tuple_write(&self, tuple: &Tuple) {
        if self.is_group_membership(tuple) {
            // Incremental transitive closure update
            self.update_group_index(tuple).await;
        } else {
            // Invalidate check cache
            self.check_cache.invalidate_pattern(&tuple.to_pattern()).await;
        }
    }
}
```

**Memory Estimate**:
- RoaringBitmap: ~1-2 bytes per group ID (compressed)
- 100K groups × 10 average memberships = 1-2MB
- Well within C12 constraint

### 4.2 Precomputed Check Results (Current Phase 2 Design)

**Approach**: Keep current Phase 2 design with Valkey but optimize hot paths.

```rust
pub struct PrecomputedChecker {
    // L1: In-memory hot cache
    hot_cache: Arc<moka::Cache<CacheKey, bool>>,

    // L2: Valkey for shared cache
    valkey: Arc<ValkeyClient>,

    // L3: Fallback to graph resolver
    resolver: Arc<GraphResolver>,
}

impl PrecomputedChecker {
    pub async fn check(&self, req: &CheckRequest) -> Result<bool> {
        // L1: Hot cache (sub-microsecond)
        if let Some(result) = self.hot_cache.get(&req.cache_key()) {
            return Ok(result);
        }

        // L2: Valkey lookup (~100μs)
        if let Some(result) = self.valkey.get(&req.cache_key()).await? {
            self.hot_cache.insert(req.cache_key(), result);
            return Ok(result);
        }

        // L3: Graph resolution (~1-10ms)
        let result = self.resolver.check(req).await?;

        // Async cache population
        tokio::spawn(self.populate_cache(req.clone(), result));

        Ok(result)
    }
}
```

### 4.3 Materialized Views (Database-Level)

**Approach**: Use PostgreSQL materialized views for group expansion.

```sql
-- Materialized view of transitive group membership
CREATE MATERIALIZED VIEW group_transitive_closure AS
WITH RECURSIVE group_hierarchy AS (
    -- Base case: direct group memberships
    SELECT store_id, user_type, user_id, object_id as group_id, 1 as depth
    FROM tuples
    WHERE object_type = 'group' AND relation = 'member'

    UNION ALL

    -- Recursive case: group-to-group memberships
    SELECT gh.store_id, gh.user_type, gh.user_id, t.object_id, gh.depth + 1
    FROM group_hierarchy gh
    JOIN tuples t ON gh.group_id = t.user_id
        AND t.object_type = 'group'
        AND t.relation = 'member'
        AND t.user_type = 'group'
    WHERE gh.depth < 25
)
SELECT DISTINCT * FROM group_hierarchy;

-- Index for fast lookups
CREATE INDEX idx_gtc_user ON group_transitive_closure(store_id, user_type, user_id);
CREATE INDEX idx_gtc_group ON group_transitive_closure(store_id, group_id);

-- Refresh strategy (incremental in PostgreSQL 15+)
REFRESH MATERIALIZED VIEW CONCURRENTLY group_transitive_closure;
```

**Pros**: Database handles consistency, no application memory pressure
**Cons**: Refresh latency, additional database load

---

## 5. Recommended Architecture

### 5.1 Tiered Pre-computation Strategy

```
┌─────────────────────────────────────────────────────────────┐
│                   Check Request                              │
└───────────────────────────┬─────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────┐
│  Tier 1: In-Memory Hot Cache (Moka)                         │
│  - Recent check results                                      │
│  - Sub-microsecond latency                                   │
│  - 100MB budget                                              │
└───────────────────────────┬─────────────────────────────────┘
                            ↓ (miss)
┌─────────────────────────────────────────────────────────────┐
│  Tier 2: Leopard-Lite (Group Membership Only)               │
│  - Transitive closure for groups                             │
│  - <50μs latency                                             │
│  - 50MB budget                                               │
│  - Roaring bitmaps for efficient storage                     │
└───────────────────────────┬─────────────────────────────────┘
                            ↓ (not group check or miss)
┌─────────────────────────────────────────────────────────────┐
│  Tier 3: Valkey Precomputed Cache                           │
│  - Full check results                                        │
│  - ~100μs latency                                            │
│  - Shared across nodes                                       │
│  - TTL-based expiration                                      │
└───────────────────────────┬─────────────────────────────────┘
                            ↓ (miss)
┌─────────────────────────────────────────────────────────────┐
│  Tier 4: Graph Resolver (Fallback)                          │
│  - Full resolution                                           │
│  - 1-10ms latency                                            │
│  - Populates upper tiers                                     │
└─────────────────────────────────────────────────────────────┘
```

### 5.2 Leopard-Lite Implementation

```rust
use roaring::RoaringBitmap;
use dashmap::DashMap;

/// Leopard-Lite: Optimized group membership pre-computation
pub struct LeopardLite {
    /// Maps group ID → all ancestor group IDs (transitive)
    group_ancestors: DashMap<u64, Arc<RoaringBitmap>>,

    /// Maps group ID → all descendant group IDs (transitive)
    group_descendants: DashMap<u64, Arc<RoaringBitmap>>,

    /// Maps user ID → directly member groups
    user_groups: DashMap<u64, Arc<RoaringBitmap>>,

    /// ID allocator for stable integer IDs
    id_allocator: IdAllocator,

    /// Pending incremental updates
    update_queue: tokio::sync::mpsc::Sender<GroupUpdate>,
}

impl LeopardLite {
    /// Check if user is member of group (direct or transitive)
    pub fn check_membership(&self, user_id: u64, group_id: u64) -> bool {
        // Get user's direct group memberships
        let user_bitmap = match self.user_groups.get(&user_id) {
            Some(b) => b.clone(),
            None => return false,
        };

        // Check if user is directly in group
        if user_bitmap.contains(group_id as u32) {
            return true;
        }

        // Get group's transitive descendants (groups that are members)
        let group_descendants = match self.group_descendants.get(&group_id) {
            Some(b) => b.clone(),
            None => return false,
        };

        // Check intersection: is user in any descendant group?
        !user_bitmap.is_disjoint(&group_descendants)
    }

    /// Handle incremental group update
    pub async fn apply_update(&self, update: GroupUpdate) {
        match update {
            GroupUpdate::AddMember { group_id, user_id } => {
                self.user_groups
                    .entry(user_id)
                    .or_insert_with(|| Arc::new(RoaringBitmap::new()))
                    .clone()
                    .insert(group_id as u32);
            }
            GroupUpdate::RemoveMember { group_id, user_id } => {
                if let Some(mut bitmap) = self.user_groups.get_mut(&user_id) {
                    Arc::make_mut(&mut bitmap).remove(group_id as u32);
                }
            }
            GroupUpdate::AddSubgroup { parent_id, child_id } => {
                // Update transitive closure
                self.propagate_subgroup_add(parent_id, child_id).await;
            }
            GroupUpdate::RemoveSubgroup { parent_id, child_id } => {
                // Requires full recomputation of affected paths
                self.schedule_partial_rebuild(parent_id).await;
            }
        }
    }

    /// Propagate subgroup addition through transitive closure
    async fn propagate_subgroup_add(&self, parent_id: u64, child_id: u64) {
        // Get child's descendants (including itself)
        let mut to_add = RoaringBitmap::new();
        to_add.insert(child_id as u32);
        if let Some(child_descendants) = self.group_descendants.get(&child_id) {
            to_add |= child_descendants.as_ref();
        }

        // Add to parent's descendants
        self.group_descendants
            .entry(parent_id)
            .or_insert_with(|| Arc::new(RoaringBitmap::new()))
            .clone()
            .extend(to_add.iter().map(|x| x));

        // Propagate to parent's ancestors
        if let Some(ancestors) = self.group_ancestors.get(&parent_id) {
            for ancestor in ancestors.iter() {
                self.group_descendants
                    .entry(ancestor as u64)
                    .or_insert_with(|| Arc::new(RoaringBitmap::new()))
                    .clone()
                    .extend(to_add.iter().map(|x| x));
            }
        }
    }
}

#[derive(Clone)]
pub enum GroupUpdate {
    AddMember { group_id: u64, user_id: u64 },
    RemoveMember { group_id: u64, user_id: u64 },
    AddSubgroup { parent_id: u64, child_id: u64 },
    RemoveSubgroup { parent_id: u64, child_id: u64 },
}
```

### 5.3 Integration with Graph Resolver

```rust
impl GraphResolver {
    pub async fn check_with_leopard(
        &self,
        store: &str,
        user: &str,
        relation: &str,
        object: &str,
    ) -> Result<bool> {
        // Parse user and object
        let (user_type, user_id) = parse_entity(user)?;
        let (object_type, object_id) = parse_entity(object)?;

        // Check if this is a group membership check
        if self.is_group_membership_check(object_type, relation) {
            if let Some(leopard) = &self.leopard {
                let user_int_id = leopard.id_allocator.get_id(user);
                let group_int_id = leopard.id_allocator.get_id(object);

                if leopard.check_membership(user_int_id, group_int_id) {
                    return Ok(true);
                }
                // Fall through to full resolution if Leopard says no
                // (might be due to stale index)
            }
        }

        // Standard graph resolution
        self.resolve(store, user, relation, object, &CheckContext::new()).await
    }
}
```

---

## 6. Implementation Roadmap

### Phase 2a: Leopard-Lite for Groups (4 weeks)

| Week | Tasks |
|------|-------|
| 1 | ID allocator, RoaringBitmap integration, basic data structures |
| 2 | Incremental update propagation, subgroup handling |
| 3 | Integration with GraphResolver, benchmarks |
| 4 | Testing, edge cases, memory profiling |

### Phase 2b: Valkey Integration (2 weeks)

| Week | Tasks |
|------|-------|
| 1 | Valkey client, cache population strategy |
| 2 | TTL management, cluster support |

### Phase 2c: Hybrid Resolution (2 weeks)

| Week | Tasks |
|------|-------|
| 1 | Tiered lookup implementation |
| 2 | Metrics, tuning, production readiness |

---

## 7. Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Memory exceeds budget | Medium | High | Monitor with metrics, LRU eviction for user_groups |
| Incremental updates lag | Medium | Medium | Queue depth monitoring, circuit breaker |
| Transitive closure explosion | Low | High | Depth limits, size limits per group |
| Stale index returns false negative | Medium | Medium | Always fall back to full resolution |

---

## 8. Validation Criteria

### Performance

- [ ] Group membership check: <100μs p99
- [ ] Non-group check: <1ms p99 (with cache hit)
- [ ] Index update latency: <10ms p99

### Memory

- [ ] Leopard-Lite: <50MB for 100K groups
- [ ] Total cache budget: <500MB (C12)

### Correctness

- [ ] All compatibility tests pass
- [ ] No false negatives (may have false positive due to staleness, triggers fallback)
- [ ] Consistent with full graph resolution

---

## 9. Decision

**Proceed with Leopard-Lite implementation** as described in Section 5, with the following scope:

1. **In scope**: Group-to-group and member-to-group transitive closure
2. **Out of scope**: General relation pre-computation (use existing cache + Valkey)
3. **Memory budget**: 50MB for Leopard-Lite, 100MB for hot cache, remainder for Valkey client

**ADR to create**: ADR-017: Leopard-Lite Pre-computation for Group Membership

---

## Sources

- [Zanzibar Paper (USENIX ATC'19)](https://www.usenix.org/system/files/atc19-pang.pdf)
- [AuthZed: Understanding Google Zanzibar](https://authzed.com/blog/what-is-google-zanzibar)
- [AuthZed: Zanzibar Overview](https://authzed.com/zanzibar)
- [SpiceDB Leopard Issue #129](https://github.com/authzed/spicedb/issues/129)
- [OpenFGA Performance Optimizations](https://deepwiki.com/openfga/openfga/2.3-performance-optimizations)
- [Oso: What is Google Zanzibar](https://www.osohq.com/learn/google-zanzibar)
