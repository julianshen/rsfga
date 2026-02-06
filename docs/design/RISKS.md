# RSFGA Risk Register

## Overview

This document catalogs identified risks, their likelihood, impact, and mitigation strategies. Risks are reviewed at each milestone and before major decisions.

**Last Updated**: 2024-01-03
**Risk Review Frequency**: Monthly during development, quarterly post-launch

---

## Risk Assessment Matrix

| Likelihood | Impact | Priority |
|------------|--------|----------|
| High | High | 🔴 Critical |
| High | Medium | 🟠 High |
| Medium | High | 🟠 High |
| High | Low | 🟡 Medium |
| Medium | Medium | 🟡 Medium |
| Low | High | 🟡 Medium |
| Medium | Low | 🟢 Low |
| Low | Medium | 🟢 Low |
| Low | Low | 🟢 Low |

---

## Critical Risks (🔴)

### R-001: API Compatibility Breaks
**Category**: Technical
**Likelihood**: Medium
**Impact**: High
**Priority**: 🔴 Critical

**Description**: RSFGA may inadvertently break OpenFGA API compatibility, preventing drop-in replacement.

**Impact Analysis**:
- Users cannot migrate from OpenFGA
- Ecosystem tooling breaks
- Invalidates core project value proposition

**Mitigation**:
- ✅ Build comprehensive compatibility test suite (Phase 0, Milestone 0.1-0.7)
- ✅ Run compatibility test suite in CI against RSFGA
- ✅ Document all deviations explicitly
- ✅ Version API independently
- ⏸️ Create migration validator tool (Phase 1, Milestone 1.7)

**Monitoring**:
- CI test pass rate: 100% required
- Community reported compatibility issues: Track GitHub issues
- Review quarterly: Before each release

**Owner**: Architecture Team
**Status**: Mitigated (with monitoring)

---

### R-002: Performance Claims Unvalidated
**Category**: Technical
**Likelihood**: High
**Impact**: High
**Priority**: 🔴 Critical

**Description**: Claimed 2-5x performance improvements may not materialize under real workloads.

**Impact Analysis**:
- Loss of credibility
- No competitive advantage over OpenFGA
- Wasted optimization effort
- Users disappointed after migration

**Mitigation**:
- ✅ Benchmark against OpenFGA continuously (Milestone 1.7)
- ✅ Qualify all claims as "target" or "unvalidated"
- ✅ Profile before optimizing
- ⏸️ Publish benchmark methodology publicly
- ⏸️ Invite community benchmarking

**Validation Criteria**:
- Check: ≥1000 req/s sustained (current: 483 req/s OpenFGA)
- Batch: ≥500 checks/s (current: 23 checks/s OpenFGA)
- p95 latency: <20ms (current: 22ms OpenFGA)

**Confidence Level**: 60% (based on architectural analysis, not tested)

**Owner**: Performance Team
**Status**: Active Risk - Requires validation in Milestone 1.7

---

### R-003: Authorization Correctness Bug
**Category**: Security/Correctness
**Likelihood**: Medium
**Impact**: Critical
**Priority**: 🔴 Critical

**Description**: Graph resolution bug could grant unauthorized access or deny legitimate access.

**Impact Analysis**:
- **Security breach**: Unauthorized users gain access
- **Service denial**: Legitimate users blocked
- **Compliance violations**: Audit failures
- **Reputation damage**: Trust loss in security product

**Mitigation**:
- ✅ Comprehensive unit tests (>90% coverage)
- ✅ Integration tests against compatibility test suite (Phase 0)
- ✅ Fuzzing for edge cases
- ⏸️ Formal verification of graph algorithm (Phase 1, Milestone 1.4)
- ⏸️ Property-based testing (QuickCheck/Proptest)
- ⏸️ Shadow mode testing in production

**Quality Gates**:
- No graph resolution bug escapes to production
- 100% compatibility test suite pass (Phase 0 validates all relation types)
- Property tests cover all relation types

**Owner**: Security Team
**Status**: Active - Requires rigorous testing

---

## High Risks (🟠)

### R-004: Database Performance Bottleneck
**Category**: Technical
**Likelihood**: High
**Impact**: Medium
**Priority**: 🟠 High

**Description**: PostgreSQL may become bottleneck under high write load, limiting scalability.

**Impact Analysis**:
- Write throughput ceiling at ~150 req/s (estimated)
- Cannot scale horizontally easily
- May require expensive vertical scaling

**Mitigation**:
- ✅ Connection pooling (100 connections)
- ✅ Optimized indexes on hot paths
- ⏸️ Read replicas for read scaling (Phase 2)
- ⏸️ Sharding strategy for multi-tenant (Phase 3)
- ⏸️ Consider write-optimized storage (append-only log)

**Monitoring**:
- Connection pool saturation: Alert at 80%
- Query latency p95: Alert at >50ms
- Lock contention: Monitor pg_stat_activity

**Fallback**:
- Implement write buffering/batching
- Add write queue with back-pressure
- Horizontal sharding by store_id

**Owner**: Infrastructure Team
**Status**: Monitoring required in production

---

### R-005: Cache Consistency Issues
**Category**: Technical/Correctness
**Likelihood**: Medium
**Impact**: High
**Priority**: 🟠 High

**Description**: Async cache invalidation may serve stale results during 1-10ms window.

**Impact Analysis**:
- Recent permission changes not reflected immediately
- Security issue if user revoked but cache still allows
- Audit compliance issues

**Mitigation**:
- ✅ Document consistency guarantees clearly
- ✅ Provide strong consistency mode (bypass cache)
- ⏸️ Measure actual staleness window in production
- ⏸️ Implement cache invalidation metrics
- ⏸️ Add invalidation success rate tracking

**Acceptance Criteria**:
- Staleness window: <100ms p99 (target)
- Strong consistency mode available for critical operations
- Clear documentation for users on consistency model

**Owner**: Domain Team
**Status**: Acceptable risk with mitigation

---

### R-006: NATS Edge Sync Lag
**Category**: Technical (Phase 3)
**Likelihood**: Medium
**Impact**: Medium
**Priority**: 🟠 High

**Description**: Network partitions could cause edge nodes to lag behind central, serving outdated data.

**Impact Analysis**:
- Edge nodes serve stale authorization decisions
- Eventual consistency window extends indefinitely during partition
- Requires fallback to regional/central

**Mitigation**:
- ✅ Design fallback to regional on high lag
- ⏸️ Monitor sync lag per edge node
- ⏸️ Alert on lag >5 seconds
- ⏸️ Automatic fallback on partition detection
- ⏸️ Implement conflict-free replicated data types (CRDTs) if needed

**Monitoring**:
- Sync lag: Alert at >5s
- Fallback rate: Track per edge
- Partition detection: Auto-alert

**Owner**: Edge Team
**Status**: Phase 3 - Design consideration

---

### R-007: Dependency Vulnerabilities
**Category**: Security
**Likelihood**: High
**Impact**: Medium
**Priority**: 🟠 High

**Description**: Third-party Rust crates may have security vulnerabilities.

**Impact Analysis**:
- Supply chain attacks
- Known CVEs in dependencies
- Compliance violations

**Mitigation**:
- ✅ Run `cargo audit` in CI
- ✅ Dependabot alerts enabled
- ⏸️ Pin dependency versions
- ⏸️ Review all dependency updates
- ⏸️ Minimize dependencies
- ⏸️ Prefer tier-1 maintained crates

**Monitoring**:
- Weekly cargo audit runs
- Immediate action on high/critical CVEs
- Quarterly dependency review

**Owner**: Security Team
**Status**: Continuous monitoring required

---

## Medium Risks (🟡)

### R-008: Tokio Runtime Overhead
**Category**: Technical
**Likelihood**: Low
**Impact**: High
**Priority**: 🟡 Medium

**Description**: Async overhead may negate performance benefits for fast operations.

**Impact Analysis**:
- Slower than OpenFGA on simple checks
- Claims of 2x performance invalid
- Wasted refactoring effort

**Mitigation**:
- ⏸️ Benchmark sync vs async for simple operations
- ⏸️ Hybrid approach: sync for fast path, async for slow
- ⏸️ Profile async overhead

**Validation**: Milestone 1.7 benchmarking

**Owner**: Performance Team
**Status**: Low priority - validate in benchmarking phase

---

### R-009: DashMap Scalability Limits
**Category**: Technical
**Likelihood**: Medium
**Impact**: Medium
**Priority**: 🟡 Medium

**Description**: DashMap may hit contention issues at extreme concurrency (>1000 threads).

**Impact Analysis**:
- Cache becomes bottleneck instead of acceleration
- Need to revert to alternative caching strategy

**Mitigation**:
- ⏸️ Benchmark at high concurrency (Milestone 1.7)
- ⏸️ Consider sharded DashMap
- ⏸️ Fallback: Moka or custom lock-free structure

**Owner**: Performance Team
**Status**: Validate in testing

---

### R-010: Graph Cycle Detection False Positives
**Category**: Correctness
**Likelihood**: Low
**Impact**: High
**Priority**: 🟡 Medium

**Description**: Cycle detection may incorrectly identify valid paths as cycles.

**Impact Analysis**:
- Legitimate permissions denied
- User complaints
- Breaks authorization model assumptions

**Mitigation**:
- ✅ Comprehensive cycle detection tests
- ⏸️ Validate against known-good models
- ⏸️ Add cycle detection metrics

**Owner**: Domain Team
**Status**: Test coverage required

---

### R-011: Memory Exhaustion on Large Models
**Category**: Availability
**Likelihood**: Medium
**Impact**: Medium
**Priority**: 🟡 Medium

**Description**: Very large authorization models (>10k types) may exhaust memory.

**Impact Analysis**:
- Service crashes
- Cannot support large enterprises
- Memory limits deployment options

**Mitigation**:
- ⏸️ Lazy-load model components
- ⏸️ Implement model size limits
- ⏸️ Memory profiling on large models
- ⏸️ Document maximum supported model size

**Owner**: Domain Team
**Status**: Document limitations

---

### R-012: Precomputation Storage Explosion (Phase 2)
**Category**: Technical (Phase 2)
**Likelihood**: High
**Impact**: Medium
**Priority**: 🟡 Medium

**Description**: Precomputing all checks could generate terabytes of cached results.

**Impact Analysis**:
- Valkey/Redis storage costs exceed benefits
- Write amplification on tuple changes
- Slower writes negate faster reads

**Mitigation**:
- ⏸️ Selective precomputation (hot paths only)
- ⏸️ TTL-based eviction (60s default)
- ⏸️ Cost-benefit analysis per relation type
- ⏸️ Lazy precomputation (compute on first access)

**Owner**: Precomputation Team
**Status**: Phase 2 - Design carefully

---

## Low Risks (🟢)

### R-013: Protobuf Breaking Changes
**Category**: Technical
**Likelihood**: Low
**Impact**: Medium
**Priority**: 🟢 Low

**Description**: OpenFGA protobuf changes could break compatibility.

**Mitigation**:
- Monitor OpenFGA releases
- Pin protobuf versions
- Test against new OpenFGA versions

**Owner**: API Team
**Status**: Monitoring

---

### R-014: NATS vs Kafka Regret (Phase 3)
**Category**: Technical (Phase 3)
**Likelihood**: Low
**Impact**: Medium
**Priority**: 🟢 Low

**Description**: NATS may prove insufficient for edge sync at scale.

**Impact Analysis**:
- Need to rewrite edge sync
- Migration complexity
- Wasted effort

**Mitigation**:
- ✅ Document NATS vs Kafka decision rationale (ADR-014)
- ⏸️ Prototype NATS at scale before Phase 3
- ⏸️ Keep Kafka as fallback option

**Trigger to Revisit**:
- NATS throughput <100k msgs/s
- NATS leaf node instability
- NATS operational complexity exceeds Kafka

**Owner**: Edge Team
**Status**: Acceptable - Monitor in Phase 3

---

### R-015: Rust Compiler Bugs
**Category**: Technical
**Likelihood**: Very Low
**Impact**: High
**Priority**: 🟢 Low

**Description**: Rare Rust compiler bugs could cause silent correctness issues.

**Mitigation**:
- Use stable Rust channel
- Keep compiler updated
- Test extensively

**Owner**: Infrastructure Team
**Status**: Low probability

---

## Organizational Risks

### R-016: Insufficient Testing Resources
**Category**: Organizational
**Likelihood**: Medium
**Impact**: High
**Priority**: 🟠 High

**Description**: Comprehensive testing requires significant effort that may be underestimated.

**Impact Analysis**:
- Correctness issues slip to production
- Technical debt accumulates
- Milestone delays

**Mitigation**:
- ✅ Allocate dedicated testing milestone (1.7)
- ⏸️ Hire QA engineer if needed
- ⏸️ Invest in test infrastructure early

**Owner**: Project Management
**Status**: Monitor resource allocation

---

### R-017: Knowledge Silos
**Category**: Organizational
**Likelihood**: Medium
**Impact**: Medium
**Priority**: 🟡 Medium

**Description**: Architecture knowledge concentrated in few individuals.

**Impact Analysis**:
- Bus factor issues
- Onboarding difficulties
- Architectural drift

**Mitigation**:
- ✅ Comprehensive documentation (this!)
- ⏸️ Pair programming on complex components
- ⏸️ Architectural review sessions
- ⏸️ Knowledge sharing presentations

**Owner**: Technical Lead
**Status**: Documentation in progress

---

### R-018: Eventual Consistency Window for Revocations
**Category**: Security
**Likelihood**: Medium
**Impact**: High
**Priority**: 🟠 High

**Description**: When using async write path, permission revocations have a delay before taking effect, creating a window where revoked permissions may still be granted.

**Impact Analysis**:
- Revoked users may retain access for 20-100ms (typical)
- Security-sensitive applications may not tolerate any delay
- Compliance requirements may mandate immediate revocation

**Mitigation**:
- ✅ Document async path behavior clearly
- ✅ Recommend sync `/write` endpoint for revocations (DELETE operations)
- ✅ Provide `async_allow_deletes: false` config option
- ⏸️ Consider "revocation fast path" bypassing queue

**Owner**: Security Team
**Status**: Mitigated with documentation and configuration options

---

### R-019: Sequence Counter Non-Persistence
**Category**: Operational
**Likelihood**: High
**Impact**: Medium
**Priority**: 🟠 High

**Description**: The `rsfga-writer` daemon uses an in-memory global sequence counter that resets on restart, potentially breaking event ordering guarantees for downstream consumers.

**Impact Analysis**:
- Sequence numbers reset to 0 on daemon restart
- Downstream consumers may see duplicate sequence numbers across restarts
- Event ordering cannot be guaranteed across restarts
- Edge sync consumers may process events out of order
- Cache invalidation may miss events during restart window

**Mitigation**:
- ✅ Document limitation in ADR-020 Known Limitations
- ✅ Document in consumer.rs code comments
- ⏸️ Use NATS stream sequence numbers as alternative (already unique/persistent)
- ⏸️ Implement per-store persistent sequence tracking in storage
- ⏸️ Add sequence recovery on startup from last committed event

**Workarounds**:
- Use NATS message sequence instead of custom sequence for strict ordering
- Design downstream consumers to be idempotent and handle sequence gaps
- Implement "catch-up" logic on consumer startup

**Owner**: Infrastructure Team
**Status**: Active Risk - Documented, workarounds available

---

## Risk Review Triggers

### Automatic Review Required When:
1. **Milestone completion** - Review all risks in scope
2. **Architecture change** - Review related risks
3. **Security incident** - Review security risks
4. **Performance issue** - Review performance risks
5. **New dependency** - Review supply chain risks
6. **Quarterly** - Review all risks

### Risk Escalation:
- 🔴 Critical: Escalate to leadership immediately
- 🟠 High: Review in weekly planning
- 🟡 Medium: Review monthly
- 🟢 Low: Review quarterly

---

## Retired Risks

None yet.

---

## Risk Ownership

| Team | Risks Owned |
|------|-------------|
| Architecture Team | R-001 |
| Performance Team | R-002, R-004, R-008, R-009 |
| Security Team | R-003, R-007, R-018 |
| Domain Team | R-005, R-010, R-011 |
| Edge Team | R-006, R-014 |
| API Team | R-013 |
| Infrastructure Team | R-015, R-016, R-019 |
| Precomputation Team | R-012 |
| Project Management | R-016 |
| Technical Lead | R-017 |

---

**Next Review Date**: End of Milestone 1.1 (Week 2)
**Document Owner**: Chief Architect
**Approval Required**: Yes, for adding Critical or High risks
