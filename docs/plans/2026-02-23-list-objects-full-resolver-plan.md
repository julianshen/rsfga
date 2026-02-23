# ListObjects Full Resolver Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Bring ListObjects to full parity with OpenFGA — correctness fixes, parallel graph traversal, and StreamedListObjects gRPC streaming.

**Architecture:** Tidy First structural refactoring (extract `ReverseExpandContext`/`State` from 14-parameter function), then layer behavioral changes: `authorization_model_id` support, contextual tuples threaded through traversal, userset type constraint handling, parallel union/intersection/exclusion branches, and a new `StreamedListObjects` gRPC server-streaming endpoint.

**Tech Stack:** Rust, Tokio (FuturesUnordered, mpsc), Tonic gRPC, protobuf

**Design Doc:** `docs/plans/2026-02-23-list-objects-full-resolver-design.md`

---

## Task 1: Structural — Extract `ReverseExpandContext` and `ReverseExpandState`

Pure structural change. All existing tests must pass before and after. No behavioral change.

**Files:**
- Modify: `crates/rsfga-domain/src/resolver/types.rs` (add structs after line 407)
- Modify: `crates/rsfga-domain/src/resolver/graph_resolver.rs:1370-2046` (refactor `list_objects_inner` + `reverse_expand_objects`)

**Step 1: Run existing tests to establish baseline**

```bash
cargo test --lib -p rsfga-domain -- list_objects
```

Expected: All ~15 list_objects tests pass.

**Step 2: Add `ReverseExpandContext` and `ReverseExpandState` to `types.rs`**

After the `ListObjectsResult` struct (line 407), add:

```rust
/// Immutable context for the ReverseExpand algorithm.
/// Shared across parallel branches (Clone-free via references).
pub(crate) struct ReverseExpandContext<'a> {
    pub store_id: &'a str,
    pub user: &'a str,
    pub object_type: &'a str,
    pub request_context: &'a HashMap<String, serde_json::Value>,
    pub limit: usize,
    pub max_depth: u32,
}

/// Mutable traversal state for the ReverseExpand algorithm.
/// Forked per parallel branch, merged after completion.
pub(crate) struct ReverseExpandState {
    pub seen: HashSet<String>,
    pub visited: HashSet<String>,
    pub results: Vec<String>,
    pub truncated: bool,
    pub depth: u32,
}

impl ReverseExpandState {
    pub fn new() -> Self {
        Self {
            seen: HashSet::new(),
            visited: HashSet::new(),
            results: Vec::new(),
            truncated: false,
            depth: 0,
        }
    }

    /// Fork state for a parallel branch. Shares seen+visited (cloned),
    /// but starts with empty results.
    pub fn fork(&self) -> Self {
        Self {
            seen: self.seen.clone(),
            visited: self.visited.clone(),
            results: Vec::new(),
            truncated: false,
            depth: self.depth,
        }
    }

    /// Merge results from a union branch back into this state.
    pub fn merge_union(&mut self, branch: Self) {
        for obj in branch.results {
            if !self.seen.contains(&obj) {
                self.seen.insert(obj.clone());
                self.results.push(obj);
            }
        }
        if branch.truncated {
            self.truncated = true;
        }
    }
}
```

Add required import at top of `types.rs`:
```rust
use std::collections::{HashMap, HashSet};
```

**Step 3: Refactor `reverse_expand_objects` signature**

In `graph_resolver.rs`, change the signature from 14 parameters to:

```rust
fn reverse_expand_objects<'a>(
    &'a self,
    ctx: &'a ReverseExpandContext<'a>,
    state: &'a mut ReverseExpandState,
    relation: &'a str,
    userset: &'a Userset,
) -> BoxFuture<'a, DomainResult<()>>
```

Update the function body to use `ctx.store_id`, `ctx.user`, `ctx.object_type`, `ctx.request_context`, `ctx.limit`, `ctx.max_depth` instead of the old parameters, and `state.seen`, `state.results`, `state.visited`, `state.truncated`, `state.depth` instead of the mutable refs.

**Step 4: Update `list_objects_inner` to construct context and state**

In `list_objects_inner` (line 1370), replace the call site at lines 1429-1444:

```rust
let ctx = ReverseExpandContext {
    store_id: &request.store_id,
    user: &request.user,
    object_type: &request.object_type,
    request_context: &request.context,
    limit,
    max_depth: self.config.max_depth,
};
let mut state = ReverseExpandState::new();

self.reverse_expand_objects(&ctx, &mut state, &request.relation, &relation_def.userset)
    .await
    .or_else(|e| match e {
        DomainError::DepthLimitExceeded { .. } | DomainError::CycleDetected { .. } => {
            state.truncated = true;
            Ok(())
        }
        other => Err(other),
    })?;

let mut result_objects = state.results;
let mut seen = state.seen;
let was_truncated = state.truncated;
```

**Step 5: Update all recursive `reverse_expand_objects` call sites**

Inside `reverse_expand_objects`, every recursive call (in `ComputedUserset`, `TupleToUserset`, `Union`, `Intersection`, `Exclusion` branches) must be updated to pass `ctx` and `state` (or forked state) instead of the individual parameters.

Key call sites to update (approximate current lines):
- `ComputedUserset` recursive call (~line 1612)
- `TupleToUserset` parent resolution call (~line 1700)
- `TupleToUserset` child resolution calls (~line 1740)
- `Union` branch calls (~line 1810)
- `Intersection` branch calls (~line 1880)
- `Exclusion` base/subtract calls (~line 1970)

**Step 6: Run tests to verify no behavioral change**

```bash
cargo test --lib -p rsfga-domain -- list_objects
cargo clippy --all-targets --all-features -- -D warnings
```

Expected: All tests still pass. No clippy warnings.

**Step 7: Commit**

```bash
git add crates/rsfga-domain/src/resolver/types.rs crates/rsfga-domain/src/resolver/graph_resolver.rs
git commit -m "[STRUCTURAL] Extract ReverseExpandContext and ReverseExpandState from reverse_expand_objects

Refactor the 14-parameter reverse_expand_objects function into a clean
interface using two structs: ReverseExpandContext (immutable, shared) and
ReverseExpandState (mutable, per-branch). No behavioral changes.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 2: Correctness — `authorization_model_id` Support

Thread `authorization_model_id` through the entire ListObjects pipeline.

**Files:**
- Modify: `crates/rsfga-domain/src/resolver/types.rs:267-317` (add field to `ListObjectsRequest`)
- Modify: `crates/rsfga-domain/src/resolver/graph_resolver.rs:1370-1444` (use `get_relation_definition_with_model_id`)
- Modify: `crates/rsfga-api/src/http/routes.rs:3077-3084` (pass field)
- Modify: `crates/rsfga-api/src/grpc/service.rs:1054-1061` (pass field)
- Test: `crates/rsfga-domain/src/resolver/tests/resolver_tests.rs`

**Step 1: Write failing test**

In `resolver_tests.rs`, add a test that verifies `authorization_model_id` is respected (i.e., the resolver uses the specific model, not latest):

```rust
#[tokio::test]
async fn test_list_objects_respects_authorization_model_id() {
    // Setup: Two models for the same store, different relation definitions
    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());
    tuple_reader.add_store("store1").await;

    // Model v1: document has "viewer" as direct (This)
    // Model v2: document has "viewer" as computed from "editor"
    // With model_id pinned to v1, should use direct lookup only
    // (Details depend on MockModelReader capabilities for model-by-id lookup)

    let resolver = GraphResolver::new(tuple_reader.clone(), model_reader.clone());
    let mut request = ListObjectsRequest::new("store1", "user:alice", "viewer", "document");
    request.authorization_model_id = Some("model-v1".to_string());

    let result = resolver.list_objects(&request, 100).await.unwrap();
    // Assert behavior matches pinned model, not latest
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test --lib -p rsfga-domain -- test_list_objects_respects_authorization_model_id
```

Expected: FAIL — `authorization_model_id` field doesn't exist on `ListObjectsRequest`.

**Step 3: Add field to `ListObjectsRequest`**

In `types.rs` line 267, add the field:

```rust
pub struct ListObjectsRequest {
    pub store_id: String,
    pub authorization_model_id: Option<String>,  // NEW
    pub user: String,
    pub relation: String,
    pub object_type: String,
    pub contextual_tuples: Arc<Vec<ContextualTuple>>,
    pub context: Arc<HashMap<String, serde_json::Value>>,
}
```

Update constructors `new()` and `with_context()` to set `authorization_model_id: None`.

**Step 4: Add field to `ReverseExpandContext`**

```rust
pub(crate) struct ReverseExpandContext<'a> {
    pub store_id: &'a str,
    pub user: &'a str,
    pub object_type: &'a str,
    pub authorization_model_id: Option<&'a str>,  // NEW
    pub request_context: &'a HashMap<String, serde_json::Value>,
    pub limit: usize,
    pub max_depth: u32,
}
```

**Step 5: Update `list_objects_inner` to use model-ID-aware lookup**

At line ~1421, change:
```rust
// Before:
let relation_def = self.model_reader
    .get_relation_definition(&request.store_id, &request.object_type, &request.relation)
    .await?;

// After:
let relation_def = self.model_reader
    .get_relation_definition_with_model_id(
        &request.store_id,
        &request.object_type,
        &request.relation,
        request.authorization_model_id.as_deref(),
    )
    .await?;
```

Also set `authorization_model_id` in the `ReverseExpandContext` construction.

**Step 6: Update `reverse_expand_objects` to use model-ID-aware lookups**

Every `get_relation_definition` call inside `reverse_expand_objects` (at `ComputedUserset` ~line 1601, `TupleToUserset` ~lines 1639/1657) must be changed to:

```rust
self.model_reader
    .get_relation_definition_with_model_id(
        ctx.store_id,
        type_name,
        relation_name,
        ctx.authorization_model_id,
    )
    .await?
```

**Step 7: Thread field through HTTP handler**

In `routes.rs` at lines 3077-3084, pass `body.authorization_model_id`:

```rust
let mut list_request = ListObjectsRequest::with_context(
    store_id, body.user, body.relation, body.r#type,
    contextual_tuples, body.context.unwrap_or_default(),
);
list_request.authorization_model_id = body.authorization_model_id;
```

Or add an `authorization_model_id` parameter to the `with_context` constructor.

**Step 8: Thread field through gRPC handler**

In `service.rs` at lines 1054-1061:

```rust
let authorization_model_id = if req.authorization_model_id.is_empty() {
    None
} else {
    Some(req.authorization_model_id.clone())
};

let mut list_request = DomainListObjectsRequest::with_context(
    req.store_id, req.user, req.relation, req.r#type,
    contextual_tuples, context,
);
list_request.authorization_model_id = authorization_model_id;
```

**Step 9: Run tests**

```bash
cargo test --lib -p rsfga-domain -- list_objects
cargo test --lib -p rsfga-api
cargo clippy --all-targets --all-features -- -D warnings
```

Expected: All pass, including the new test.

**Step 10: Commit**

```bash
git add crates/rsfga-domain/ crates/rsfga-api/
git commit -m "[BEHAVIORAL] Add authorization_model_id support to ListObjects

Thread authorization_model_id through HTTP/gRPC handlers into the domain
resolver. All recursive get_relation_definition calls in reverse_expand_objects
now use get_relation_definition_with_model_id, pinning the entire graph
traversal to a specific model snapshot when requested.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 3: Correctness — Contextual Tuples Through Graph Traversal

Move contextual tuple handling from post-processing into the `Userset::This` branch of `reverse_expand_objects`.

**Files:**
- Modify: `crates/rsfga-domain/src/resolver/graph_resolver.rs:1447-1473` (remove post-processing)
- Modify: `crates/rsfga-domain/src/resolver/graph_resolver.rs:1542-1584` (`Userset::This` branch)
- Modify: `crates/rsfga-domain/src/resolver/types.rs` (add `contextual_tuples` to `ReverseExpandContext`)
- Test: `crates/rsfga-domain/src/resolver/tests/resolver_tests.rs`

**Step 1: Write failing test for computed-relation contextual tuples**

```rust
#[tokio::test]
async fn test_list_objects_contextual_tuples_via_computed_relation() {
    // Model: document type with "can_access" = union(viewer, editor)
    // Stored tuple: user:alice viewer document:stored-doc
    // Contextual tuple: user:alice editor document:ctx-doc
    // Expected: list_objects for "can_access" returns both documents
    // (currently only returns "stored-doc" because contextual tuple
    //  post-processing only matches direct relation, not computed)

    let tuple_reader = Arc::new(MockTupleReader::new());
    let model_reader = Arc::new(MockModelReader::new());
    tuple_reader.add_store("store1").await;

    model_reader.add_type("store1", TypeDefinition {
        type_name: "document".to_string(),
        relations: vec![
            RelationDefinition {
                name: "viewer".to_string(),
                userset: Userset::This,
                type_constraints: vec![TypeConstraint::direct("user")],
            },
            RelationDefinition {
                name: "editor".to_string(),
                userset: Userset::This,
                type_constraints: vec![TypeConstraint::direct("user")],
            },
            RelationDefinition {
                name: "can_access".to_string(),
                userset: Userset::Union {
                    children: vec![
                        Userset::ComputedUserset { relation: "viewer".to_string() },
                        Userset::ComputedUserset { relation: "editor".to_string() },
                    ],
                },
                type_constraints: vec![],
            },
        ],
    }).await;

    tuple_reader.add_tuple("store1", "document", "stored-doc", "viewer", "user", "alice", None).await;

    let contextual_tuples = vec![ContextualTuple {
        user: "user:alice".to_string(),
        relation: "editor".to_string(),
        object: "document:ctx-doc".to_string(),
        condition_name: None,
        condition_context: None,
    }];

    let request = ListObjectsRequest::with_context(
        "store1", "user:alice", "can_access", "document",
        contextual_tuples, Default::default(),
    );

    let result = resolver.list_objects(&request, 100).await.unwrap();

    assert!(result.objects.contains(&"document:stored-doc".to_string()));
    assert!(result.objects.contains(&"document:ctx-doc".to_string()),
        "Contextual tuple via computed relation (editor → can_access) should be found");
    assert_eq!(result.objects.len(), 2);
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test --lib -p rsfga-domain -- test_list_objects_contextual_tuples_via_computed_relation
```

Expected: FAIL — `ctx-doc` not in results (contextual tuples only checked post-processing for direct "can_access" match, not via computed "editor").

**Step 3: Add `contextual_tuples` to `ReverseExpandContext`**

```rust
pub(crate) struct ReverseExpandContext<'a> {
    // ... existing fields ...
    pub contextual_tuples: &'a [ContextualTuple],  // NEW
}
```

Update construction in `list_objects_inner`.

**Step 4: Add contextual tuple scanning to `Userset::This` branch**

After the existing `get_objects_for_user` call and its processing (line ~1584), add:

```rust
// Also check contextual tuples for direct matches on this relation
for ct in ctx.contextual_tuples.iter() {
    if state.results.len() >= ctx.limit {
        state.truncated = true;
        break;
    }
    if ct.relation != relation {
        continue;
    }
    // Check user match (direct or wildcard)
    let user_matches = ct.user == ctx.user
        || (ct.user.contains(':') && ct.user.ends_with(":*") && {
            let ct_type = ct.user.split(':').next().unwrap_or("");
            let user_type = ctx.user.split(':').next().unwrap_or("");
            ct_type == user_type
        });
    if !user_matches {
        continue;
    }
    if let Some((obj_type, _obj_id)) = ct.object.split_once(':') {
        if obj_type == ctx.object_type && !state.seen.contains(&ct.object) {
            // Evaluate condition if present
            if ct.condition_name.is_some() {
                let condition_ok = self
                    .evaluate_condition(
                        ctx.store_id,
                        ct.condition_name.as_deref(),
                        ct.condition_context.as_ref(),
                        ctx.request_context,
                    )
                    .await?;
                if !condition_ok {
                    continue;
                }
            }
            state.seen.insert(ct.object.clone());
            state.results.push(ct.object.clone());
        }
    }
}
```

**Step 5: Remove post-processing block**

Remove lines 1447-1473 in `list_objects_inner` (the `for ct in request.contextual_tuples.iter()` block). The contextual tuples are now handled inside the graph traversal.

**Step 6: Run all list_objects tests**

```bash
cargo test --lib -p rsfga-domain -- list_objects
```

Expected: All pass, including the new computed-relation test AND all existing contextual tuple tests (which now exercise the in-traversal path instead of post-processing).

**Step 7: Commit**

```bash
git add crates/rsfga-domain/
git commit -m "[BEHAVIORAL] Thread contextual tuples through ListObjects graph traversal

Move contextual tuple handling from post-processing into the Userset::This
branch of reverse_expand_objects. This enables contextual tuples to be
found via computed relations (e.g., can_access = viewer | editor).

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 4: Correctness — TupleToUserset Userset Type Constraints

Handle `group#member` type constraints properly in TupleToUserset resolution.

**Files:**
- Modify: `crates/rsfga-domain/src/resolver/graph_resolver.rs` (TupleToUserset branch ~line 1626)
- Test: `crates/rsfga-domain/src/resolver/tests/resolver_tests.rs`

**Step 1: Write failing test**

```rust
#[tokio::test]
async fn test_list_objects_ttu_with_userset_type_constraint() {
    // Model:
    //   group type: "member" relation (This)
    //   document type: "parent" relation (This, type_constraint: group#member)
    //                  "viewer" relation (TupleToUserset: tupleset=parent, computed=member)
    //
    // Tuples:
    //   user:alice member group:eng
    //   group:eng#member parent document:doc1
    //
    // Expected: list_objects("user:alice", "viewer", "document") returns ["document:doc1"]
    // because alice is a member of group:eng, and group:eng#member is a parent of doc1,
    // so alice gets viewer on doc1 via the TupleToUserset.

    // ... setup MockTupleReader + MockModelReader ...

    let result = resolver.list_objects(&request, 100).await.unwrap();
    assert!(result.objects.contains(&"document:doc1".to_string()),
        "Should find doc1 via userset type constraint group#member");
}
```

**Step 2: Run test to verify it fails**

Expected: FAIL — the current code strips `#member` from `group#member`, treating it as just `group`.

**Step 3: Fix TupleToUserset branch**

In the `TupleToUserset` branch (~line 1647), change the type constraint parsing:

```rust
// Current (broken):
let parent_type = type_constraint.type_name.split('#').next().unwrap_or(&type_constraint.type_name);

// Fixed: handle userset references
let (parent_type, parent_relation) = if let Some((t, r)) = type_constraint.type_name.split_once('#') {
    (t, Some(r))
} else {
    (type_constraint.type_name.as_str(), None)
};
```

When `parent_relation` is `Some(rel)`, the algorithm must find parents via:
1. Find objects of `parent_type` where the user has `parent_relation` (via `reverse_expand_objects` recursion or `get_objects_for_user`)
2. Use those objects as parent IDs for the tupleset lookup (existing `get_objects_with_parents` call)

When `parent_relation` is `None`, the existing logic applies unchanged.

**Step 4: Run tests**

```bash
cargo test --lib -p rsfga-domain -- list_objects
```

**Step 5: Commit**

```bash
git add crates/rsfga-domain/
git commit -m "[BEHAVIORAL] Handle userset type constraints in TupleToUserset ListObjects

When a TupleToUserset type constraint is a userset reference (e.g.,
group#member), resolve the relation through the member relation instead
of treating the entire constraint as a bare type.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 5: Performance — Parallel Union/Intersection/Exclusion

Replace sequential branch evaluation with concurrent execution.

**Files:**
- Modify: `crates/rsfga-domain/src/resolver/graph_resolver.rs` (Union ~line 1788, Intersection ~line 1830, Exclusion ~line 1944)
- Test: `crates/rsfga-domain/src/resolver/tests/resolver_tests.rs`

**Step 1: Add `futures` import if not present**

Ensure `use futures::stream::{FuturesUnordered, StreamExt};` is imported in `graph_resolver.rs`.

**Step 2: Refactor `Union` branch to use `FuturesUnordered`**

```rust
Userset::Union { children } => {
    let mut futures = FuturesUnordered::new();
    for child in children {
        let mut branch_state = state.fork();
        futures.push(async move {
            let result = self
                .reverse_expand_objects(ctx, &mut branch_state, relation, child)
                .await;
            (result, branch_state)
        });
    }
    while let Some((result, branch_state)) = futures.next().await {
        match result {
            Ok(()) => state.merge_union(branch_state),
            Err(DomainError::DepthLimitExceeded { .. })
            | Err(DomainError::CycleDetected { .. }) => {
                state.truncated = true;
                state.merge_union(branch_state); // keep partial results
            }
            Err(e) => return Err(e),
        }
        if state.results.len() >= ctx.limit {
            state.truncated = true;
            break;
        }
    }
}
```

**Step 3: Refactor `Intersection` branch**

Evaluate first child to get candidate set, then evaluate remaining children concurrently. Intersect results after all complete:

```rust
Userset::Intersection { children } => {
    if children.is_empty() {
        return Ok(());
    }
    // Evaluate first child to get candidate set
    let mut first_state = state.fork();
    match self.reverse_expand_objects(ctx, &mut first_state, relation, &children[0]).await {
        Ok(()) => {}
        Err(DomainError::DepthLimitExceeded { .. } | DomainError::CycleDetected { .. }) => {
            state.truncated = true;
            return Ok(());
        }
        Err(e) => return Err(e),
    }

    let mut candidates: HashSet<String> = first_state.results.into_iter().collect();
    if first_state.truncated {
        state.truncated = true;
    }

    // Evaluate remaining children concurrently
    let mut futures = FuturesUnordered::new();
    for child in &children[1..] {
        let mut branch_state = state.fork();
        futures.push(async move {
            let result = self.reverse_expand_objects(ctx, &mut branch_state, relation, child).await;
            (result, branch_state)
        });
    }
    while let Some((result, branch_state)) = futures.next().await {
        match result {
            Ok(()) => {
                let branch_objects: HashSet<String> = branch_state.results.into_iter().collect();
                candidates.retain(|obj| branch_objects.contains(obj));
                if branch_state.truncated {
                    state.truncated = true;
                }
            }
            Err(DomainError::DepthLimitExceeded { .. } | DomainError::CycleDetected { .. }) => {
                state.truncated = true;
                candidates.clear(); // conservative: can't guarantee intersection
            }
            Err(e) => return Err(e),
        }
    }

    for obj in candidates {
        if !state.seen.contains(&obj) {
            state.seen.insert(obj.clone());
            state.results.push(obj);
        }
    }
}
```

**Step 4: Refactor `Exclusion` branch**

Use `tokio::join!` for concurrent evaluation:

```rust
Userset::Exclusion { base, subtract } => {
    let mut base_state = state.fork();
    let mut sub_state = state.fork();

    let (base_result, sub_result) = tokio::join!(
        self.reverse_expand_objects(ctx, &mut base_state, relation, base),
        self.reverse_expand_objects(ctx, &mut sub_state, relation, subtract),
    );

    // Handle base errors
    match base_result {
        Ok(()) => {}
        Err(DomainError::DepthLimitExceeded { .. } | DomainError::CycleDetected { .. }) => {
            state.truncated = true;
        }
        Err(e) => return Err(e),
    }

    // Handle subtract errors (conservative: if subtract fails, include all base)
    let subtract_objects: HashSet<String> = match sub_result {
        Ok(()) => sub_state.results.into_iter().collect(),
        Err(DomainError::DepthLimitExceeded { .. } | DomainError::CycleDetected { .. }) => {
            state.truncated = true;
            HashSet::new() // conservative: might over-include
        }
        Err(e) => return Err(e),
    };

    for obj in base_state.results {
        if !subtract_objects.contains(&obj) && !state.seen.contains(&obj) {
            state.seen.insert(obj.clone());
            state.results.push(obj);
        }
    }
    if base_state.truncated || sub_state.truncated {
        state.truncated = true;
    }
}
```

**Step 5: Run all tests**

```bash
cargo test --lib -p rsfga-domain -- list_objects
cargo clippy --all-targets --all-features -- -D warnings
```

Expected: All pass. The existing `test_list_objects_with_union_of_direct_and_tuple_to_userset`, `test_list_objects_with_exclusion_relation`, and `test_list_objects_with_intersection_relation` tests validate correctness.

**Step 6: Commit**

```bash
git add crates/rsfga-domain/
git commit -m "[BEHAVIORAL] Parallelize union/intersection/exclusion in ListObjects

Use FuturesUnordered for concurrent branch evaluation in union and
intersection, and tokio::join! for exclusion. Each branch gets a forked
ReverseExpandState, merged after completion.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 6: StreamedListObjects — Proto + Domain + gRPC Handler

Add the `StreamedListObjects` gRPC server-streaming endpoint.

**Files:**
- Modify: `crates/rsfga-api/proto/openfga/v1/openfga_service.proto` (add RPC + messages)
- Modify: `crates/rsfga-api/proto/openfga/v1/openfga.proto` (add message types if needed)
- Modify: `crates/rsfga-api/build.rs` (if proto compilation needs changes)
- Modify: `crates/rsfga-domain/src/resolver/graph_resolver.rs` (add `list_objects_streamed`)
- Modify: `crates/rsfga-api/src/grpc/service.rs` (add handler)
- Test: `crates/rsfga-api/src/grpc/tests.rs`

**Step 1: Add proto definitions**

In `openfga_service.proto`, add the RPC in the `service OpenFGAService` block (after `ListObjects`):

```protobuf
// Stream objects a user has access to (server-streaming)
rpc StreamedListObjects(StreamedListObjectsRequest) returns (stream StreamedListObjectsResponse);
```

Add message types (at end of proto or in `openfga.proto`):

```protobuf
message StreamedListObjectsRequest {
  string store_id = 1;
  string authorization_model_id = 2;
  string type = 3;
  string relation = 4;
  string user = 5;
  ContextualTupleKeys contextual_tuples = 6;
  google.protobuf.Struct context = 7;
  ConsistencyPreference consistency = 8;
}

message StreamedListObjectsResponse {
  string object = 1;
}
```

**Step 2: Rebuild protos**

```bash
cargo build -p rsfga-api
```

This triggers `build.rs` which compiles the proto. Expect compilation errors in `service.rs` because the trait now requires `streamed_list_objects`.

**Step 3: Add `list_objects_streamed` to domain resolver**

In `graph_resolver.rs`, add a method that sends results via an `mpsc::Sender`:

```rust
pub async fn list_objects_streamed(
    &self,
    request: &super::types::ListObjectsRequest,
    tx: tokio::sync::mpsc::Sender<String>,
    max_candidates: usize,
) -> DomainResult<()> {
    // Reuse list_objects_inner, then stream results
    let result = self.list_objects_inner(request, max_candidates).await?;
    for object in result.objects {
        if tx.send(object).await.is_err() {
            break; // client disconnected
        }
    }
    Ok(())
}
```

Note: A more efficient implementation would stream results as they're discovered (modifying `ReverseExpandState` to use a channel). For this milestone, collecting-then-streaming is simpler and correct. The optimization can be a follow-up.

**Step 4: Add gRPC handler**

In `service.rs`, implement the `streamed_list_objects` method on `OpenFgaGrpcService`:

```rust
type StreamedListObjectsStream = ReceiverStream<Result<StreamedListObjectsResponse, Status>>;

async fn streamed_list_objects(
    &self,
    request: Request<StreamedListObjectsRequest>,
) -> Result<Response<Self::StreamedListObjectsStream>, Status> {
    let req = request.into_inner();

    // Validation (same as list_objects)
    // ... validate store_id, user, relation, type ...

    // Parse contextual tuples and context (same as list_objects)
    // ... same conversion logic ...

    let mut list_request = DomainListObjectsRequest::with_context(
        req.store_id, req.user, req.relation, req.r#type,
        contextual_tuples, context,
    );
    list_request.authorization_model_id = if req.authorization_model_id.is_empty() {
        None
    } else {
        Some(req.authorization_model_id)
    };

    let (tx, rx) = tokio::sync::mpsc::channel(100);
    let resolver = self.resolver.clone();

    tokio::spawn(async move {
        match resolver.list_objects_streamed(&list_request, tx.clone(), MAX_LIST_OBJECTS_CANDIDATES).await {
            Ok(()) => {}
            Err(e) => {
                let _ = tx.send(Err(Status::internal(e.to_string()))).await;
            }
        }
    });

    // Transform String channel to StreamedListObjectsResponse channel
    let (resp_tx, resp_rx) = tokio::sync::mpsc::channel(100);
    tokio::spawn(async move {
        while let Some(object) = rx.recv().await {
            if resp_tx.send(Ok(StreamedListObjectsResponse { object })).await.is_err() {
                break;
            }
        }
    });

    Ok(Response::new(ReceiverStream::new(resp_rx)))
}
```

(Implementation details will depend on exact generated trait signatures from tonic.)

**Step 5: Write test**

```rust
#[tokio::test]
async fn test_grpc_streamed_list_objects_returns_objects() {
    // Setup store, model, tuples
    // Call streamed_list_objects, collect stream into Vec
    // Assert correct objects returned
}
```

**Step 6: Run tests**

```bash
cargo test --workspace --exclude compatibility-tests
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
```

**Step 7: Commit**

```bash
git add crates/rsfga-api/ crates/rsfga-domain/
git commit -m "[BEHAVIORAL] Add StreamedListObjects gRPC server-streaming endpoint

Add the StreamedListObjects RPC to the proto definition and implement
the gRPC handler. Results are streamed to the client as individual
StreamedListObjectsResponse messages containing one object each.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 7: Integration Tests and Compatibility Verification

Ensure all compatibility tests pass and add integration tests for the new features.

**Files:**
- Test: `crates/rsfga-domain/src/resolver/tests/resolver_tests.rs` (additional edge cases)
- Test: `crates/rsfga-api/src/grpc/tests.rs` (gRPC integration)
- Test: `crates/compatibility-tests/` (verify Section 15 + Section 34)

**Step 1: Add edge case tests**

- `test_list_objects_contextual_tuples_with_wildcard_through_computed_relation`
- `test_list_objects_contextual_tuples_with_condition_through_computed_relation`
- `test_list_objects_parallel_union_produces_correct_results`
- `test_list_objects_parallel_intersection_produces_correct_results`
- `test_list_objects_parallel_exclusion_produces_correct_results`

**Step 2: Run full test suite**

```bash
cargo test --workspace --exclude compatibility-tests
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
```

**Step 3: Run compatibility tests** (if OpenFGA Docker available)

```bash
# Start OpenFGA
docker compose -f docker-compose.openfga.yml up -d

# Run compatibility tests
cargo test -p compatibility-tests -- test_section_15
cargo test -p compatibility-tests -- test_section_34
```

**Step 4: Commit**

```bash
git add .
git commit -m "[TEST] Add comprehensive tests for ListObjects Full Resolver

Add edge case tests for contextual tuples through computed relations,
parallel branch correctness, userset type constraints, authorization_model_id
pinning, and StreamedListObjects integration.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 8: Update CLAUDE.md and ROADMAP.md Status

**Files:**
- Modify: `CLAUDE.md` (update status section)
- Modify: `docs/design/ROADMAP.md` (add Milestone 1.16 section)

**Step 1: Add Milestone 1.16 to ROADMAP.md**

Add a section for Milestone 1.16: ListObjects Full Resolver with all tasks marked complete.

**Step 2: Update CLAUDE.md**

Update the "Current Status" section to include Milestone 1.16 completion.

**Step 3: Commit**

```bash
git add CLAUDE.md docs/design/ROADMAP.md
git commit -m "[DOCS] Update status for Milestone 1.16 ListObjects Full Resolver

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## PR Strategy

Create a single PR covering all tasks, or split into smaller PRs:

**Option A (recommended):** Two PRs:
1. **PR 1**: Tasks 1-5 (structural refactoring + all correctness/performance fixes)
2. **PR 2**: Tasks 6-8 (StreamedListObjects + tests + docs)

**Option B:** One PR for everything.

---

## Summary of Tasks

| Task | Type | Description | Estimated Steps |
|------|------|-------------|-----------------|
| 1 | Structural | Extract ReverseExpandContext/State | 7 |
| 2 | Behavioral | authorization_model_id support | 10 |
| 3 | Behavioral | Contextual tuples through traversal | 7 |
| 4 | Behavioral | TupleToUserset userset type constraints | 5 |
| 5 | Behavioral | Parallel union/intersection/exclusion | 6 |
| 6 | Behavioral | StreamedListObjects proto + handler | 7 |
| 7 | Test | Integration + compatibility tests | 4 |
| 8 | Docs | Status updates | 3 |
