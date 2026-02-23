# ListObjects Full Resolver Design

**Date**: 2026-02-23
**Status**: Approved
**Milestone**: 1.16

## Problem

The current ListObjects implementation has six gaps compared to OpenFGA behavior:

1. **`authorization_model_id` silently dropped** — Both HTTP and gRPC handlers accept this field but never pass it to the domain layer. ListObjects always uses the latest model.
2. **Contextual tuples not threaded through graph traversal** — Only injected as a post-processing step for direct tuple matches; computed relations through contextual tuples are missed.
3. **TupleToUserset strips userset type constraints** — `group#member` type constraints have `#member` stripped, losing the relation context.
4. **Sequential union/intersection/exclusion branches** — Check uses `FuturesUnordered` for parallel evaluation; ListObjects iterates sequentially.
5. **`StreamedListObjects` gRPC endpoint missing** — Proto doesn't define it; no streaming implementation exists.
6. **No result caching** — Deferred to a future milestone.

## Approach: Tidy First + Targeted Fixes

Structural refactoring first, then behavioral changes on top.

## Section 1: Structural — `ReverseExpandContext` + `ReverseExpandState`

Extract the 13 parameters of `reverse_expand_objects` into two structs:

### `ReverseExpandContext` (immutable, shared across parallel branches)

```rust
pub(crate) struct ReverseExpandContext<'a> {
    pub store_id: &'a str,
    pub user: &'a str,
    pub object_type: &'a str,
    pub authorization_model_id: Option<&'a str>,
    pub contextual_tuples: &'a [ContextualTuple],
    pub request_context: &'a HashMap<String, serde_json::Value>,
    pub limit: usize,
    pub max_depth: u32,
}
```

### `ReverseExpandState` (mutable, forked per parallel branch)

```rust
pub(crate) struct ReverseExpandState {
    pub seen: HashSet<String>,
    pub visited: HashSet<String>,
    pub results: Vec<String>,
    pub truncated: bool,
    pub depth: u32,
}

impl ReverseExpandState {
    fn fork(&self) -> Self { /* clone seen+visited, fresh results, same depth */ }
    fn merge_union(&mut self, branch: Self) { /* union seen, append results, or truncated */ }
}
```

### Simplified signature

```rust
fn reverse_expand_objects<'a>(
    &'a self,
    ctx: &'a ReverseExpandContext<'a>,
    state: &'a mut ReverseExpandState,
    relation: &'a str,
    userset: &'a Userset,
) -> BoxFuture<'a, DomainResult<()>>
```

## Section 2: Correctness — `authorization_model_id`

### Changes

- **`ListObjectsRequest`** (`types.rs`): Add `pub authorization_model_id: Option<String>` field.
- **HTTP handler** (`routes.rs`): Pass `body.authorization_model_id` into `ListObjectsRequest`.
- **gRPC handler** (`service.rs`): Convert empty string to `None`, pass through.
- **`list_objects_inner`**: If `authorization_model_id` is `Some`, use it when calling `get_relation_definition`. Otherwise use latest (current behavior).
- **`ReverseExpandContext`**: Carries it so all recursive `get_relation_definition` calls use the same model version.

Mirrors how Check handles model pinning.

## Section 3: Correctness — Contextual Tuples Through Graph Traversal

### Problem

Contextual tuples are only scanned post-traversal for direct matches:
```
user:alice viewer document:x → found (post-processing)
user:alice editor document:x → NOT found via computed "can_access = viewer | editor"
```

### Fix

Move contextual tuple scanning into `reverse_expand_objects` at the `Userset::This` branch:

```rust
Userset::This => {
    // 1. Storage lookup (existing)
    let direct_objects = self.tuple_reader.get_objects_for_user(...).await?;
    // ... process direct_objects ...

    // 2. Contextual tuple scan (NEW)
    for ct in ctx.contextual_tuples {
        if ct.relation == relation {
            let matches_user = ct.user == ctx.user
                || ct.user.ends_with(":*")  // wildcard
                // Also handle userset references in ct.user
            ;
            if matches_user {
                if let Some((obj_type, _)) = ct.object.split_once(':') {
                    if obj_type == ctx.object_type && !state.seen.contains(&ct.object) {
                        // Evaluate condition if present
                        state.seen.insert(ct.object.clone());
                        state.results.push(ct.object.clone());
                    }
                }
            }
        }
    }
}
```

Remove the post-processing pass in `list_objects_inner` (lines 1447-1473).

`ComputedUserset` branches recursively call `reverse_expand_objects` with a different relation, which will now naturally find contextual tuples matching that computed relation.

## Section 4: Correctness — TupleToUserset Userset Type Constraints

### Problem

Type constraints like `group#member` have `#member` stripped (line 1647):
```rust
type_constraint.type_name.split('#').next() // "group#member" → "group"
```

### Fix

When a type constraint contains `#`, it's a userset reference. The algorithm must:

1. Split `group#member` into base type `group` and relation `member`.
2. Find objects of type `group` where the user has `member` relation (via `reverse_expand_objects` recursion or `get_objects_for_user`).
3. Use those resolved objects as parent IDs for the tupleset lookup.

For non-userset type constraints (no `#`), the current behavior is unchanged.

## Section 5: Performance — Parallel Union/Intersection/Exclusion

### Union

Spawn all children concurrently with `FuturesUnordered`. Each branch gets a forked `ReverseExpandState`. After all complete, merge results (union of `seen`, append `results`). Errors in individual branches set `truncated = true` but don't fail the whole operation.

### Intersection

Evaluate first child, build candidate set. Evaluate remaining children concurrently. Intersect: only objects in ALL children survive. If any child fails or is truncated, mark result as truncated.

### Exclusion

Evaluate base and subtract concurrently via `tokio::join!`. Return base - subtract. If subtract is truncated, mark result as truncated (conservative).

## Section 6: StreamedListObjects gRPC Endpoint

### Proto additions (`openfga_service.proto`)

```protobuf
rpc StreamedListObjects(StreamedListObjectsRequest) returns (stream StreamedListObjectsResponse);

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

### Domain layer

Add a streaming variant in the resolver:

```rust
pub async fn list_objects_streamed(
    &self,
    request: ListObjectsRequest,
    tx: mpsc::Sender<String>,
) -> DomainResult<()>
```

The `ReverseExpandState` can be adapted to send results via the channel as they're discovered, rather than collecting into a Vec.

### gRPC handler

Uses `tokio::sync::mpsc` channel + `ReceiverStream` for the tonic streaming response. The resolver runs in a spawned task.

### Existing `list_objects` refactoring

The existing `list_objects` method internally calls `list_objects_streamed`, collecting all results into a Vec. This avoids duplicating the traversal logic.

## Files to Modify

| File | Change |
|------|--------|
| `crates/rsfga-domain/src/resolver/types.rs` | Add `authorization_model_id` to `ListObjectsRequest`, add `ReverseExpandContext`/`State` |
| `crates/rsfga-domain/src/resolver/graph_resolver.rs` | Refactor `reverse_expand_objects`, fix all 6 Userset branches, add `list_objects_streamed` |
| `crates/rsfga-api/src/http/routes.rs` | Thread `authorization_model_id` |
| `crates/rsfga-api/src/grpc/service.rs` | Thread `authorization_model_id`, add `StreamedListObjects` handler |
| `crates/rsfga-api/proto/openfga/v1/openfga_service.proto` | Add `StreamedListObjects` RPC + messages |
| `crates/rsfga-api/proto/openfga/v1/openfga.proto` | Add message types if not in service proto |

## Testing Strategy

- **Unit tests**: Each Userset branch with contextual tuples, model_id pinning, userset type constraints
- **Integration tests**: Full resolver with parallel branches, streaming, edge cases
- **Compatibility tests**: Existing Section 15 tests must still pass; Section 34 (StreamedListObjects) tests should now pass against RSFGA
- **Property tests**: Random authorization models with contextual tuples

## Invariants

- **I1 (Correctness)**: Contextual tuples with conditions must be evaluated. Wildcard contextual tuples must be handled.
- **I2 (Compatibility)**: `authorization_model_id` support matches OpenFGA behavior. StreamedListObjects proto is wire-compatible.
- **I4 (Security)**: Depth limits, cycle detection, and timeout still enforced in all new paths.

## Out of Scope

- ListObjects result caching (future milestone)
- ListUsers equivalent improvements (separate milestone, same patterns apply)
- HTTP streaming endpoint (OpenFGA only supports gRPC streaming for ListObjects)
