# RSFGA - High-Performance Rust Implementation of OpenFGA

[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](https://www.rust-lang.org/)

**RSFGA** is a high-performance, 100% API-compatible Rust implementation of [OpenFGA](https://openfga.dev/), an authorization/permission engine inspired by Google Zanzibar.

## Status

**Current Phase**: Phase 0 - Building OpenFGA Compatibility Test Suite ⏳

This project is in active development. We are currently building a comprehensive compatibility test suite to validate OpenFGA's behavior before implementing RSFGA. See [ROADMAP.md](docs/design/ROADMAP.md) for details.

## Goals

- ✅ **100% API Compatibility**: Drop-in replacement for OpenFGA
- ✅ **High Performance**: 2-5x performance improvement over OpenFGA (target, unvalidated)
- ✅ **Production Ready**: Comprehensive observability, testing, and reliability
- ✅ **Distributed Ready**: Designed for edge deployment (Phase 3)

## Architecture

RSFGA is built on a 5-layer architecture optimized for performance and correctness:

```
API Layer (HTTP REST, gRPC)
    ↓
Server Layer (Request Handlers)
    ↓
Domain Layer (Graph Resolver, Type System, Cache)
    ↓
Storage Abstraction (DataStore trait)
    ↓
Storage Backends (PostgreSQL, MySQL, In-Memory)
```

**Key Technologies**:
- **Async Runtime**: Tokio (work-stealing, I/O parallelism)
- **HTTP**: Axum (fast, ergonomic)
- **gRPC**: Tonic (pure Rust, performant)
- **Database**: SQLx (async, compile-time query checking)
- **Concurrency**: DashMap (lock-free cache)
- **Observability**: tracing + metrics + OpenTelemetry

For detailed architecture, see [docs/design/ARCHITECTURE.md](docs/design/ARCHITECTURE.md).

## Project Structure

```
rsfga/
├── crates/
│   ├── rsfga-api/          # HTTP & gRPC API layer
│   ├── rsfga-server/       # Request handlers & business logic
│   ├── rsfga-domain/       # Graph resolver, type system, cache
│   └── rsfga-storage/      # Storage abstraction & backends
├── tests/
│   └── compatibility/      # OpenFGA compatibility test suite (Phase 0)
├── docs/
│   └── design/             # Architecture & design documents
├── CLAUDE.md               # AI assistant guide (TDD methodology)
└── plan.md                 # Detailed implementation plan (500+ tests)
```

## Quick Start

**Prerequisites**:
- Rust 1.75+ ([Install Rust](https://rustup.rs/))
- Docker (for OpenFGA compatibility tests)
- grpcurl (optional, for gRPC-specific tests in Sections 21, 23, 34)
  - macOS: `brew install grpcurl`
  - Linux/Windows: [Download from GitHub](https://github.com/fullstorydev/grpcurl/releases)

**Clone and Build**:
```bash
git clone https://github.com/your-org/rsfga.git
cd rsfga
cargo build
```

**Run Tests** (Phase 0+):
```bash
# Unit tests
cargo test

# Compatibility tests (Phase 0)
cd tests/compatibility
docker-compose up -d
cargo test --test compatibility
```

## Roadmap

### Phase 0: OpenFGA Compatibility Test Suite (Current - 7 weeks)
Build comprehensive test suite (~150 tests) that validates OpenFGA behavior across all APIs.

**Why Phase 0?** OpenFGA doesn't provide a compatibility test suite. We must build our own validation framework before implementing RSFGA to ensure 100% API compatibility.

**Milestones**:
- 0.1: Test Harness Foundation (Docker, generators, capture framework)
- 0.2: Store & Model API Tests
- 0.3: Tuple API Tests
- 0.4: Check API Tests (core authorization)
- 0.5: Expand & ListObjects API Tests
- 0.6: Error Handling & Edge Cases
- 0.7: gRPC API Compatibility

### Phase 1: MVP - OpenFGA Compatible Core (12 weeks)
100% API-compatible drop-in replacement with 2x performance improvement.

**Milestones**:
- 1.1: Project Foundation
- 1.2: Type System & Parser
- 1.3: Storage Layer
- 1.4: Graph Resolver
- 1.5: Batch Check Handler
- 1.6: API Layer
- 1.7: Testing & Benchmarking

### Phase 2: Precomputation Engine (Future - 6 weeks)
Precompute check results on writes for <1ms p99 latency.

### Phase 3: Distributed Edge (Future - 10 weeks)
NATS-based edge deployment for <10ms global latency.

See [ROADMAP.md](docs/design/ROADMAP.md) for detailed milestones and tasks.

## Documentation

### Design Documents
- [ARCHITECTURE.md](docs/design/ARCHITECTURE.md) - System architecture & design
- [ROADMAP.md](docs/design/ROADMAP.md) - Implementation roadmap with Phase 0-3
- [ARCHITECTURE_DECISIONS.md](docs/design/ARCHITECTURE_DECISIONS.md) - 16 ADRs documenting key decisions
- [RISKS.md](docs/design/RISKS.md) - Risk register with mitigation strategies
- [API_SPECIFICATION.md](docs/design/API_SPECIFICATION.md) - Complete API reference
- [DATA_MODELS.md](docs/design/DATA_MODELS.md) - Data structures & schemas

### Development Guides
- [CLAUDE.md](CLAUDE.md) - AI assistant guide (TDD methodology, invariants, workflow)
- [plan.md](plan.md) - Detailed implementation plan with 500+ testable increments
- [CONTRIBUTING.md](CONTRIBUTING.md) - Contribution guidelines & workflow

## Performance Targets

| Operation | OpenFGA | RSFGA Target | Strategy |
|-----------|---------|--------------|----------|
| Check | 483 req/s | 1000+ req/s | Async graph, lock-free cache |
| Batch Check | 23 checks/s | 500+ checks/s | Parallel + dedup |
| Write | 59 req/s | 150+ req/s | Async invalidation |

**Note**: All targets are unvalidated (60% confidence) until benchmarked in Phase 1. Performance baselines will be established in Phase 0.

## Critical Architectural Invariants

**I1: Correctness Over Performance** - Never trade authorization correctness for performance

**I2: 100% OpenFGA API Compatibility** - All endpoints, formats, and behaviors must be identical

**I3: Performance Claims Require Validation** - All targets unvalidated until benchmarked

**I4: Security-Critical Code Path Protection** - Graph resolver requires rigorous testing and security review

See [CLAUDE.md](CLAUDE.md) for detailed invariants and quality gates.

## Technology Stack

| Category | Library | Purpose |
|----------|---------|---------|
| **Async Runtime** | tokio | I/O parallelism, work-stealing scheduler |
| **HTTP Server** | axum | Fast, ergonomic web framework |
| **gRPC** | tonic | Pure Rust gRPC implementation |
| **Database** | sqlx | Async SQL with compile-time checking |
| **Concurrency** | dashmap | Lock-free concurrent hashmap |
| **Caching** | moka | High-performance cache with TTL |
| **Parsing** | nom | Parser combinator for DSL |
| **Observability** | tracing + metrics | Logging, metrics, distributed tracing |

## Contributing

We welcome contributions! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for:
- Development workflow (TDD, branch naming, PR process)
- Code quality standards
- Testing requirements
- Commit message conventions

**Quick Start for Contributors**:
1. Read [CLAUDE.md](CLAUDE.md) for development philosophy
2. Review [plan.md](plan.md) for current tasks
3. Follow TDD methodology (Red → Green → Refactor)
4. Create PRs per section (5-15 tests, <500 lines)

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.

## Acknowledgments

- [OpenFGA](https://openfga.dev/) for the original implementation and inspiration
- [Google Zanzibar](https://research.google/pubs/pub48190/) for the foundational concepts
- The Rust community for excellent async ecosystem

## Contact

- Issues: [GitHub Issues](https://github.com/your-org/rsfga/issues)
- Discussions: [GitHub Discussions](https://github.com/your-org/rsfga/discussions)

## References

### OpenFGA Resources
- [OpenFGA Official Website](https://www.openfga.dev/)
- [OpenFGA GitHub Repository](https://github.com/openfga/openfga)
- [OpenFGA Documentation](https://openfga.dev/docs)
- [OpenFGA Playground](https://play.openfga.dev/)

### Research Papers
- [Google Zanzibar Paper](https://research.google/pubs/pub48190/) - Inspiration for OpenFGA
- [Relationship-Based Access Control](https://en.wikipedia.org/wiki/Relationship-based_access_control)

---

**Status**: Architecture & Design ✅ Complete | Phase 0 ⏳ In Progress

**Next Milestone**: 0.1 - Test Harness Foundation

Built with ❤️ in Rust
