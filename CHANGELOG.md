# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [2.0.0] - 2026-02-10

### Added

#### NATS Async Writes (Phase 2)
- **Async Write Path** - High-throughput write pipeline using NATS JetStream with durable delivery
- **rsfga-writer daemon** - Storage consumer that batches writes from NATS for improved throughput
- **rsfga-edge daemon** - Edge sync consumer for distributed authorization with eventual consistency
- **RYOW Consistency** - Read-Your-Own-Writes via write tickets for strong consistency after async writes
- **X-Consistency Header** - HTTP header for per-request consistency control (`strong` / `eventual`)
- **Dead Letter Queue** - Automatic routing of failed messages with categorized error tracking
- **Write Tracker** - Per-store sequence tracking for RYOW and strong consistency guarantees
- **Edge Cache Invalidation** - Automatic check cache invalidation on edge event receipt
- **Bootstrap Sync** - Initial data sync for new edge nodes joining the cluster

#### Infrastructure
- **Multi-binary Docker images** - Parameterized Dockerfile for rsfga, rsfga-writer, and rsfga-edge
- **Docker Compose profiles** - NATS and edge profiles for local development
- **Edge deployment guide** - Operational documentation for edge sync deployment

#### New Crates
- **rsfga-nats** - NATS JetStream client, stream management, event types, write tracker
- **rsfga-writer** - Storage consumer daemon with message batching and DLQ routing
- **rsfga-edge** - Edge sync daemon with store filtering and watermark-based idempotency

### Changed
- API handlers (check, expand, list_objects, list_users) accept `X-Consistency` header
- Release workflow builds and pushes 3 Docker images in parallel via matrix strategy

## [1.0.1] - 2025-01-31

### Fixed
- **RocksDB backend validation** - Add "rocksdb" to valid storage backends list in config validation
- **RocksDB data_path validation** - Add validation that data_path is required when using RocksDB backend

## [1.0.0] - 2025-01-31

### 🎉 Initial Production Release

RSFGA 1.0.0 is a high-performance, 100% API-compatible Rust implementation of OpenFGA. This release marks the completion of Phase 1 (MVP) with ~918 tests passing and full production readiness.

### Core Features

#### APIs
- **Check API** - Single and batch permission checks with contextual tuples support
- **Write API** - Create and delete relationship tuples with transaction support
- **Read API** - Query tuples with filtering and pagination
- **Expand API** - Visualize the authorization graph for debugging
- **ListObjects API** - Find all objects a user can access (with ReverseExpand optimization)
- **ListUsers API** - Find all users with access to an object
- **ReadChanges API** - Changelog for tuple mutations with continuation tokens

#### Storage Backends
- **In-Memory** - Fast development and testing
- **PostgreSQL** - Production-grade with connection pooling
- **MySQL/MariaDB/TiDB** - Full compatibility with MySQL-compatible databases
- **CockroachDB** - Distributed PostgreSQL-compatible database
- **RocksDB** - Embedded storage for edge/IoT deployments (NEW)

#### Protocol Support
- **REST API** - Full OpenFGA HTTP API compatibility
- **gRPC API** - High-performance binary protocol with reflection
- **Health Checks** - HTTP and gRPC health endpoints

#### Advanced Features
- **CEL Conditions** - Attribute-based access control with Common Expression Language
- **Batch Check** - Deduplicated parallel permission checks
- **Check Cache** - Lock-free LRU cache with TTL support
- **Contextual Tuples** - Runtime tuple injection for dynamic permissions

#### Observability
- **Prometheus Metrics** - Request latency, cache hit rates, throughput
- **Structured Logging** - JSON-formatted logs with trace IDs
- **OpenTelemetry Tracing** - Distributed tracing with Jaeger support

### Performance Highlights

| Metric | Target | Achieved |
|--------|--------|----------|
| Check throughput | 1000 req/s | ✅ Validated |
| Batch check | 500 checks/s | ✅ Validated |
| ListObjects p95 | <100ms | 5.9ms |
| Cache hit rate | >80% | ✅ Validated |

### RocksDB Storage Backend

New embedded storage backend for edge deployment scenarios:

- **5.9x faster than target** for write operations (1.7ms vs 10ms)
- **7.4x faster than target** for read operations (135µs vs 1ms)
- **Persistent storage** without external database dependency
- **Key injection protection** with validated input sanitization
- **Atomic batch writes** for transaction safety

### Compatibility

- 100% OpenFGA API compatibility verified with CLI test suite
- Drop-in replacement - swap OpenFGA for RSFGA with zero code changes
- Tested against OpenFGA v1.8.x behavior

### Docker Support

Multi-architecture images published to GitHub Container Registry:

```bash
docker pull ghcr.io/julianshen/rsfga:v1.0.0
docker pull ghcr.io/julianshen/rsfga:latest
```

Supported architectures: `linux/amd64`, `linux/arm64`

### Documentation
- Comprehensive architecture documentation (ADR-001 through ADR-019)
- API specification with examples
- Deployment guides for Docker and Kubernetes
- Risk register and mitigation strategies

### Contributors
This release represents significant development effort with contributions from:
- Core authorization engine implementation
- Multiple storage backend implementations
- Comprehensive test suite (918+ tests)
- Documentation and deployment automation

## [Unreleased]

No unreleased changes yet.
