# RSFGA Security Review

**Date**: 2026-03-23
**Scope**: Full codebase security review (API, Storage, Domain/Resolver, Server/Config/Infrastructure)
**Reviewer**: Senior Security Review (Automated)

---

## Executive Summary

RSFGA demonstrates **strong security engineering** in its core authorization logic (graph resolver, cycle detection, type constraints, CEL sandboxing). However, the deployment and API layers have **critical gaps** that must be addressed before production use with real authorization data.

| Severity | Count | Key Areas |
|----------|-------|-----------|
| Critical | 4 | No authentication, no TLS, permissive CORS defaults, default credentials in templates |
| High | 6 | No rate limiting, gRPC reflection default-on, unwrap/expect in prod, unencrypted NATS/Valkey, error info leakage, depth tracking asymmetry in ReverseExpand |
| Medium | 10 | Unauthenticated metrics, K8s RBAC gaps, JSON depth inconsistency, Docker permissions, env var credential exposure, request memory tracking, decompression bombs, unchecked depth increment, concurrent visited set mutations, visited set cloning overhead |
| Low | 5 | Missing security headers, config file trust, health endpoint exposure, gRPC health check default, timing side-channel in CEL evaluation |

---

## Critical Findings

### C1: No Authentication on Any Endpoint

**Severity**: CRITICAL
**Files**: `crates/rsfga-api/src/http/routes.rs` (lines 80-126), `crates/rsfga-api/src/grpc/service.rs`

All HTTP and gRPC endpoints are publicly accessible with zero authentication:

- No bearer token validation
- No API key checks
- No JWT verification
- No mTLS

Any network client can create/delete stores, write/delete tuples, modify authorization models, and read all authorization data.

**Impact**: Complete API takeover. An attacker on the network can grant themselves any permission or deny access to legitimate users.

**Recommendation**:
- Implement authentication middleware (API key at minimum, OAuth2/OIDC preferred)
- Add per-endpoint authorization checks
- Support mTLS for service-to-service communication
- This is the single most important fix before any production deployment

---

### C2: No TLS/SSL Support (HTTP or gRPC)

**Severity**: CRITICAL
**Files**: `crates/rsfga-api/src/main.rs` (lines 365-383), `crates/rsfga-api/src/grpc/server.rs` (lines 137-230)

Both HTTP and gRPC servers bind to plain TCP sockets with no TLS option:

```rust
// HTTP - plain TCP
let listener = tokio::net::TcpListener::bind(addr).await?;
axum::serve(listener, router)

// gRPC - plain TCP via Tonic
Server::builder()
    // No TLS configuration
```

**Impact**: All authorization requests/responses transmitted in plaintext. Man-in-the-middle attacks can intercept and modify authorization decisions. Non-compliant with OWASP, NIST, SOC2.

**Recommendation**:
- Add `rustls`/`tokio-rustls` for HTTP TLS
- Add Tonic TLS configuration for gRPC
- Make TLS configurable but strongly recommended in production
- Support certificate rotation

---

### C3: Permissive CORS Configuration

**Severity**: HIGH (currently mitigated by not being applied to router)
**File**: `crates/rsfga-api/src/middleware/mod.rs` (lines 22-31)

```rust
pub fn cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(Any)     // Any origin
        .allow_methods(Any)    // Any HTTP method
        .allow_headers(Any)    // Any header
        .expose_headers(Any)   // All response headers exposed
}
```

The function exists but is **not currently applied** to the router in `http/routes.rs`. However, the permissive defaults are a security footgun waiting to be activated.

**Recommendation**:
- Replace `Any` with configurable origin whitelist
- Restrict methods to `GET, POST, PUT, DELETE, OPTIONS`
- Restrict headers to `Content-Type, Authorization, X-Request-ID`
- Add configuration via environment variables

---

### C4: Default Credentials in Deployment Templates

**Severity**: HIGH
**Files**:
- `docker-compose.yaml` (lines 81-83): `POSTGRES_PASSWORD=rsfga`
- `deploy/kubernetes/secret.yaml` (lines 25-26): `password: changeme`
- `deploy/helm/rsfga/values.yaml` (line 127): `postgres://rsfga:changeme@...`

Hardcoded weak credentials in deployment templates that may be copied to production.

**Recommendation**:
- Remove inline passwords from templates
- Use `.env.example` with placeholder values for docker-compose
- Remove `stringData` from K8s secret template; require `kubectl create secret`
- Add prominent `# CHANGE THIS` warnings and CI checks for default values

---

## High Severity Findings

### H1: No Rate Limiting

**Severity**: HIGH
**Files**: No rate limiting middleware found in any router configuration

No rate limiting on any endpoint. An attacker can:
- Exhaust server resources with check/batch-check floods
- Brute-force authorization relationships
- Cause denial of service

**Recommendation**:
- Add `tower-governor` or similar rate limiting middleware
- Implement per-IP and per-API-key limits
- Return HTTP 429 when exceeded
- Make limits configurable per environment

---

### H2: gRPC Reflection Enabled by Default

**Severity**: HIGH
**Files**: `crates/rsfga-server/src/config.rs` (lines 156-158), `crates/rsfga-api/src/grpc/server.rs` (lines 45-46, 115-128)

gRPC reflection allows any client to enumerate all services, methods, and message types without proto files.

**Recommendation**: Default to `false`; require explicit opt-in for development environments.

---

### H3: `unwrap()`/`expect()` in Production Code Paths

**Severity**: HIGH
**Files**:
- `crates/rsfga-api/src/main.rs:390`: `signal::ctrl_c().await.expect("failed to install Ctrl+C handler")`
- `crates/rsfga-api/src/main.rs:396`: `signal::unix::signal(...).expect("failed to install signal handler")`
- `crates/rsfga-storage/src/traits.rs:165`: `serde_json::to_string(self).expect("cursor serialization should not fail")`

These panic on failure, causing process crash (DoS).

**Recommendation**: Replace with proper error handling. Signal handler setup should log and exit gracefully. Serialization should return `Result`.

---

### H4: Unencrypted NATS and Valkey Connections

**Severity**: HIGH
**Files**:
- `crates/rsfga-api/src/main.rs` (lines 437-442): NATS config with no TLS enforcement
- `crates/rsfga-api/src/main.rs` (lines 218-222): Valkey config with no TLS enforcement

Authorization data and cached decisions transmitted in plaintext over internal network.

**Recommendation**:
- Enforce TLS for NATS (`nats+s://`) and Valkey (`rediss://`)
- Add startup warning if plaintext URLs are used
- Document TLS configuration in deployment guides

---

### H5: Error Information Leakage

**Severity**: HIGH (partially mitigated)
**File**: `crates/rsfga-api/src/http/routes.rs` (lines 530-605), `crates/rsfga-api/src/errors.rs`

Production mode (default) properly hides sensitive details. However, some error paths still expose user-supplied values:

```rust
DomainError::InvalidUserFormat { value } => {
    ApiError::validation_error(format!("invalid user format: {}", value))
}
```

Storage errors may include connection strings or database schema details.

**Recommendation**:
- Never include raw user-supplied values in error responses
- Wrap all storage errors with generic messages for clients
- Log detailed errors server-side only

---

## Medium Severity Findings

### M1: Unauthenticated Metrics Endpoint

**File**: `crates/rsfga-api/src/http/routes.rs` (line 198)

`/metrics` endpoint is publicly accessible, exposing operational data (request rates, latencies, queue depths, error rates).

**Recommendation**: Add optional authentication for metrics; restrict to internal network.

---

### M2: Kubernetes RBAC Not Configured

**Files**: `deploy/kubernetes/deployment.yaml`, `deploy/kubernetes/serviceaccount.yaml`

ServiceAccount created but no RBAC rules or NetworkPolicy defined.

**Recommendation**: Create minimal RBAC ClusterRole; add NetworkPolicy to restrict ingress/egress.

---

### M3: JSON Depth Validation Inconsistency

**File**: `crates/rsfga-api/src/validation.rs` (line 14)

`MAX_JSON_DEPTH = 10` enforced only for condition context, not all JSON payloads.

**Recommendation**: Apply depth validation consistently to all JSON request bodies.

---

### M4: Docker Container Permissions

**File**: `Dockerfile` (lines 159-174)

Good: Non-root user (UID 1000). Issue: `/app` directory writable by app user; no capability dropping in Dockerfile (only in K8s manifests).

**Recommendation**: Make `/app` root-owned read-only; add `--cap-drop=ALL` if Docker is used standalone.

---

### M5: Environment Variable Credential Exposure

**Files**: Multiple (docker-compose, Kubernetes, Helm)

`RSFGA_STORAGE__DATABASE_URL` containing credentials is visible via `docker inspect`, `kubectl describe pod`, and process listings.

**Recommendation**: Use Kubernetes Secrets with volume mounts or external secrets management (Vault, External Secrets Operator).

---

### M6: Request Memory Tracking

**File**: `crates/rsfga-api/src/http/routes.rs` (line 75)

1MB body limit exists (good), but no per-request memory tracking. Large batch checks could consume significant memory before limits apply.

**Recommendation**: Add per-request memory budgets; track allocation during batch operations.

---

### M7: Potential Decompression Bomb

If `tower-http` automatic decompression is enabled, attackers can send highly compressed payloads that expand beyond the 1MB body limit.

**Recommendation**: Verify decompression limits; set `max_decompressed_size` if compression layer is active.

---

## Low Severity Findings

### L1: Missing Security Response Headers

No security headers added by HTTP layer: `X-Content-Type-Options`, `X-Frame-Options`, `Strict-Transport-Security`, `Content-Security-Policy`.

**Recommendation**: Add security header middleware via `tower-http::SetResponseHeaderLayer`.

---

### L2: Implicit Trust of Configuration Files

**File**: `crates/rsfga-api/src/main.rs` (lines 51-55)

No validation that config file is from a trusted source or has appropriate permissions.

**Recommendation**: Validate file permissions (not world-readable); prefer environment variables for secrets.

---

### L3: Health Endpoint Information Disclosure

`/health` and `/ready` endpoints publicly accessible. Attackers can determine service status.

**Recommendation**: Acceptable for K8s probes; document that these should not be exposed to untrusted networks.

---

### L4: NATS Default No Authentication

**File**: `crates/rsfga-nats/src/config.rs` (lines 18-58)

NATS defaults to no authentication. Any NATS client can publish write events, potentially corrupting authorization data.

**Recommendation**: Add startup warning if NATS authentication is not configured; document production authentication requirements.

---

## Domain/Resolver Specific Findings

### D1: Depth Increment/Decrement Asymmetry in ReverseExpand

**Severity**: HIGH
**Category**: State Consistency
**File**: `crates/rsfga-domain/src/resolver/graph_resolver.rs` (lines 1637-1641)

The ComputedUserset code path mutates the shared `state.depth` directly:

```rust
state.depth += 1;
let result = self.reverse_expand_objects(ctx, state, computed_rel, &rel_def.rewrite).await;
state.depth -= 1;
```

This contrasts with Union/Intersection branches (lines 1836-1881) which correctly use `state.fork()` to create isolated copies. The asymmetry means:

- ComputedUserset mutations affect the shared state object
- If ComputedUserset executes during a Union/Intersection branch that later merges, depth tracking becomes inconsistent
- Could cause incorrect depth limit enforcement or silent result truncation

**Recommendation**: Use the fork pattern consistently. Replace increment/decrement with `let mut computed_state = state.fork(); computed_state.depth = state.depth + 1;` and merge results afterward.

---

### D2: Unchecked Integer Addition in Depth Increment

**Severity**: MEDIUM
**Category**: Integer Overflow (Defensive)
**Files**:
- `crates/rsfga-domain/src/resolver/context.rs:26` - `self.depth + 1`
- `crates/rsfga-domain/src/resolver/graph_resolver.rs:1637,1842,1895,1958` - various `state.depth + 1`

The depth counter (`u32`) uses unchecked addition. While the depth limit check (`>= 25`) runs before the increment, making overflow impractical in current code, the pattern is fragile. The codebase already uses `saturating_add` at lines 1431 and 1530 for limit counters.

**Recommendation**: Replace `self.depth + 1` with `self.depth.saturating_add(1)` for consistency and defense-in-depth.

---

### D3: Concurrent Visited Set Mutations in ReverseExpand

**Severity**: MEDIUM
**Category**: Race Condition
**File**: `crates/rsfga-domain/src/resolver/graph_resolver.rs` (lines 1714-1724, 1733)

Union/Intersection branches share the `visited` set via `Arc` clone:

```rust
// fork() shares visited via Arc clone
visited: self.visited.clone(),

// Concurrent branches then mutate:
state.visited.insert(parent_cycle_key.clone());  // line 1719
state.visited.remove(&parent_cycle_key);          // line 1724
```

`HashSet` is not thread-safe. While Tokio's cooperative scheduling within a single thread prevents data races in practice, this relies on implementation details rather than type-system guarantees. If the runtime changes or tasks are dispatched to multiple threads, cycle detection could produce incorrect results.

**Recommendation**: Use `Arc<DashMap<String, ()>>` for the visited set, or ensure fork creates a deep copy rather than sharing.

---

### D4: Visited Set Cloning Overhead (DoS Vector)

**Severity**: MEDIUM
**Category**: Resource Exhaustion
**File**: `crates/rsfga-domain/src/resolver/context.rs` (lines 31-38)

Every call to `with_visited()` clones the entire `HashSet`:

```rust
let mut new_visited = (*self.visited).clone();  // Full clone
new_visited.insert(key.to_string());
```

For adversarial models with high branching factor and max depth (25), this creates O(branches^depth) clones of growing sets. While depth and timeout limits provide upper bounds, a carefully crafted model could trigger significant memory allocation within those bounds.

**Recommendation**: Consider a RAII-based `CycleGuard` pattern that inserts on construction and removes on drop, avoiding full clones.

---

### D5: No Timing Attack Mitigation for Condition Evaluation

**Severity**: LOW
**Category**: Side-Channel
**File**: `crates/rsfga-domain/src/resolver/graph_resolver.rs` (lines 1195-1309)

CEL condition evaluation timing varies based on condition complexity and context values. An attacker measuring response times could infer:
- Whether a condition exists on a tuple
- The complexity of the condition
- Whether context values satisfy or fail the condition

**Recommendation**: For most authorization use cases this is acceptable. If timing-sensitive, add constant-time response padding or batch evaluation.

---

## Positive Security Findings (Strengths)

### Graph Resolver Security: EXCELLENT

1. **Cycle Detection**: Proper visited-node tracking with `Arc<HashSet>` and copy-on-write semantics (`crates/rsfga-domain/src/resolver/context.rs:8-40`)

2. **Depth Limiting**: Enforced at three points (expand_userset, resolve_check, reverse_expand_objects) with configurable max_depth=25 matching OpenFGA (`crates/rsfga-domain/src/resolver/config.rs:34`)

3. **Timeout Protection**: 30s default on all operations; 10ms cache timeout (`crates/rsfga-domain/src/resolver/graph_resolver.rs:35`)

4. **Wildcard Attack Prevention**: Wildcards rejected in requesting_user; only allowed in stored tuples (`graph_resolver.rs:1075-1076`)

5. **Context Injection Prevention**: Tuple condition context (admin-set) takes precedence over request context (caller-supplied), preventing callers from weakening restrictions (`graph_resolver.rs:1250-1295`)

6. **Type Constraint Validation**: Applied to both contextual and stored tuples (`graph_resolver.rs:671, 743`)

7. **Union/Intersection/Exclusion**: Proper short-circuiting with correct error semantics (`graph_resolver.rs:862-1066`)

### CEL Expression Safety: EXCELLENT

8. **Panic-Safe Parsing**: ANTLR parser panics caught with `catch_unwind` (`cel/expression.rs:62-90`)
9. **Expression Length Limit**: 4096 bytes max (`validation/mod.rs:21`)
10. **Parse Caching**: LRU with 10K entries and 1-hour TTL; atomic get-or-insert prevents race conditions (`cel/cache.rs:140-171`)

### Storage Layer: STRONG

11. **SQL Injection Prevention**: All queries use SQLx parameterized bindings; no string concatenation (`postgres.rs`, `mysql.rs`)
12. **Query Timeout Protection**: All operations wrapped with `tokio::time::timeout` (`postgres.rs:412-464`)
13. **Transaction Safety**: `FOR UPDATE` locking for conflict detection; post-insert verification for race conditions (`postgres.rs:1510-1562`)
14. **Credential Redaction**: Custom `Debug` impls redact database URLs (`postgres.rs:157-171`, `mysql.rs:145-159`)
15. **Input Validation**: Comprehensive per-field length limits matching OpenFGA spec (`traits.rs:293-367`)
16. **Batch Limits**: 1000 max parent_ids, 1000 max write batch size (`postgres.rs:1050`, `mysql.rs:12`)
17. **Condition Context Size Limit**: 64KB max JSON context (`postgres.rs:31-48`)

### Resource Exhaustion Protection: STRONG

18. **Cache Bounded**: 100K entries max (`cache/mod.rs:101`)
19. **CEL Cache Bounded**: 10K entries max (`cel/cache.rs:63`)
20. **Safe Arithmetic**: `saturating_add`/`saturating_sub` used throughout (`graph_resolver.rs:1431, 1530`)
21. **No Unsafe Code**: Zero `unsafe` blocks in production code
22. **Thread Safety**: Proper use of `Arc`, `moka::sync::Cache`, `AtomicU64` with correct ordering

### Kubernetes Deployment: GOOD

23. **Security Context**: Non-root (UID 1000), read-only root filesystem, no privilege escalation, all capabilities dropped (`deploy/kubernetes/deployment.yaml:25-29, 74-79`)

---

## Remediation Priority

### Immediate (Before Production)

1. **Add authentication** (C1) - Highest priority; without this, the system is open to anyone
2. **Add TLS** (C2) - Required for any network deployment
3. **Remove default credentials** (C4) - Prevents accidental insecure deployments
4. **Add rate limiting** (H1) - Prevents DoS

### Short-Term (Next Release)

5. **Fix depth tracking asymmetry in ReverseExpand** (D1) - Use consistent fork pattern
6. **Fix CORS defaults** (C3) - Replace `Any` with configurable whitelist
7. **Disable gRPC reflection by default** (H2)
8. **Replace unwrap/expect in production paths** (H3)
9. **Enforce TLS for NATS/Valkey** (H4)
10. **Sanitize all error messages** (H5)

### Medium-Term (Hardening)

11. **Use saturating_add for depth increments** (D2)
12. **Fix concurrent visited set mutations** (D3) - Use DashMap or deep copy
13. **Optimize visited set cloning** (D4) - Consider CycleGuard pattern
14. **Add security headers** (L1)
15. **Authenticate metrics endpoint** (M1)
16. **Add K8s NetworkPolicy and RBAC** (M2)
17. **Consistent JSON depth validation** (M3)
18. **Per-request memory tracking** (M6)

---

## Conclusion

The **core authorization engine** (graph resolver, type system, CEL evaluator, storage layer) demonstrates excellent security engineering with proper cycle detection, depth limiting, timeout protection, type constraint validation, and SQL injection prevention.

The **critical gaps** are all in the deployment and API layers: no authentication, no TLS, no rate limiting. These are typical for early-stage projects that have focused on correctness first (consistent with Invariant I1), but must be addressed before production deployment with real authorization data.

The codebase follows Rust best practices: no unsafe code, proper error propagation, bounded resource usage, and safe concurrent data structures. The security-critical code paths (resolver, CEL, storage) have the strongest protections.
