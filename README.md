# RSFGA - Rust OpenFGA Implementation

A high-performance Rust implementation of [OpenFGA](https://openfga.dev), an authorization/permission engine inspired by Google Zanzibar.

## Overview

RSFGA aims to provide:
- ✅ **100% API Compatibility** with OpenFGA
- ⚡ **2-5x Performance Improvement** through Rust's zero-cost abstractions
- 🚀 **Advanced Optimizations**: Precomputation engine, distributed edge architecture
- 🔒 **Production-Ready**: Comprehensive observability, security, testing

### Performance Goals

| Operation    | OpenFGA Baseline | RSFGA Target | Strategy |
|--------------|------------------|--------------|----------|
| Check        | 483 req/s        | 1000+ req/s  | Async graph traversal, lock-free caching |
| Batch-Check  | 23 checks/s      | 500+ checks/s| Parallel execution, cross-request dedup |
| Write        | 59 req/s         | 150+ req/s   | Batch processing, async invalidation |

> **Note**: All performance targets require validation through benchmarking.

## Project Status

🔬 **Current Phase**: Research & Design Complete

📋 **Next Steps**: Begin MVP implementation (Milestone 1.1)

## Documentation

### Design Documents

- **[ARCHITECTURE.md](./docs/design/ARCHITECTURE.md)** - Comprehensive architecture design
  - System layers and components
  - Technology stack and dependencies
  - Performance optimization strategies
  - Phase 2 & 3 advanced features (precomputation, edge)

- **[ROADMAP.md](./docs/design/ROADMAP.md)** - Detailed implementation roadmap
  - Phase 1 (MVP): 8-12 weeks, 7 milestones
  - Phase 2 (Precomputation): 6-8 weeks
  - Phase 3 (Distributed Edge): 10-12 weeks
  - Task breakdown, deliverables, testing strategy

- **[ARCHITECTURE_DECISIONS.md](./docs/design/ARCHITECTURE_DECISIONS.md)** - Key architectural decisions (ADRs)
  - ADR-001: Tokio async runtime
  - ADR-002: Lock-free caching with DashMap
  - ADR-003: Parallel graph traversal
  - ADR-005: Three-stage batch deduplication
  - ... and more

## Key Features

### Phase 1: MVP (Weeks 1-12)

Core OpenFGA-compatible implementation:

- ✅ Full HTTP & gRPC APIs
- ✅ Authorization model parser (OpenFGA DSL)
- ✅ Graph resolver with all relation types
- ✅ Storage backends: PostgreSQL, MySQL, in-memory
- ✅ Async graph traversal with Tokio
- ✅ Lock-free caching with DashMap
- ✅ Batch check with deduplication
- ✅ Comprehensive observability (metrics, tracing, logs)

### Phase 2: Precomputed Checks (Weeks 13-20)

Performance optimization through precomputation:

- ⚡ On-write precomputation engine
- ⚡ Valkey/Redis result storage
- ⚡ Sub-millisecond check operations
- ⚡ Incremental invalidation
- ⚡ Target: <1ms p99 latency for cached checks

### Phase 3: Distributed Edge (Weeks 21-32)

Global scalability with edge deployment:

- 🌍 Product-based data partitioning
- 🌍 Multi-region edge nodes
- 🌍 Selective replication via Kafka CDC
- 🌍 <10ms global latency
- 🌍 100K+ req/s cluster throughput

## Architecture Highlights

### System Layers

```
┌─────────────────────────────────────┐
│   API Layer (HTTP/gRPC)             │
├─────────────────────────────────────┤
│   Server Layer (Handlers)           │
├─────────────────────────────────────┤
│   Domain Layer (Resolver, Parser)   │
├─────────────────────────────────────┤
│   Storage Abstraction               │
├─────────────────────────────────────┤
│   Storage Backends (PG, MySQL, Mem) │
└─────────────────────────────────────┘
```

### Core Innovations

**1. Async Graph Traversal**
```rust
// Parallel resolution of union relations
futures::future::select_ok(
    union_branches.map(|branch| resolve(branch))
).await // Race to first success
```

**2. Three-Stage Deduplication**
```
Batch Request → Intra-batch Dedup → Singleflight → Cache Lookup → Parallel Execute
```

**3. Lock-Free Caching**
```rust
// DashMap instead of Arc<Mutex<HashMap>>
cache: Arc<DashMap<CacheKey, bool>>
// 10-100x faster under contention
```

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

## Research Summary

Based on comprehensive research of OpenFGA's architecture and performance:

### Key Findings

**Bottlenecks Identified**:
1. **Batch-check**: Only 23 checks/s (vs 483 req/s for single checks)
   - Synchronous blocking until all checks complete
   - No cross-request deduplication
   - Sequential processing

2. **Cache invalidation**: 1-second async timeout creates consistency windows

3. **Single-node graph resolution**: No parallelism across cluster

4. **GC pauses**: Go runtime introduces unpredictable latency

**Optimization Opportunities**:
1. Async graph traversal → 2-3x faster graph resolution
2. Lock-free caching → 10-100x faster cache access
3. Parallel batch execution → 20-50x batch throughput
4. Precomputation → <1ms check latency
5. Edge architecture → <10ms global latency

See [Architecture Review](https://github.com/julianshen/openfga/blob/main/ARCHITECTURE_REVIEW.md) and [Batch Check Analysis](https://github.com/julianshen/openfga/blob/main/BATCH_CHECK_ANALYSIS.md) for details.

## Quick Start (Future)

Once MVP is complete:

### Docker

```bash
docker run -d \
  -p 8080:8080 \
  -p 8081:8081 \
  -e DATASTORE_ENGINE=postgres \
  -e DATASTORE_URI=postgresql://user:pass@db:5432/rsfga \
  rsfga/rsfga:latest
```

### Kubernetes

```bash
helm install rsfga rsfga/rsfga \
  --set datastore.engine=postgres \
  --set datastore.uri=postgresql://...
```

### Rust Library

```rust
use rsfga::{Server, Config, PostgresDataStore};

#[tokio::main]
async fn main() -> Result<()> {
    let store = PostgresDataStore::new("postgresql://...").await?;
    let config = Config::default();
    let server = Server::new(store, config);

    server.serve("0.0.0.0:8080").await?;
    Ok(())
}
```

## Development Setup

### Prerequisites

- Rust 1.75+ (install via [rustup](https://rustup.rs))
- PostgreSQL 14+ (for integration tests)
- Docker (for testcontainers)

### Getting Started

```bash
# Clone repository
git clone https://github.com/yourusername/rsfga.git
cd rsfga

# Install tools
cargo install cargo-watch cargo-criterion sqlx-cli

# Set up database
createdb rsfga_test
sqlx migrate run

# Run tests
cargo test

# Run benchmarks
cargo bench

# Start development server
cargo run --bin rsfga-server
```

## Benchmarking

Compare against OpenFGA baseline:

```bash
# Start RSFGA server
cargo run --release --bin rsfga-server

# Run k6 load tests (same as OpenFGA benchmarks)
k6 run tests/k6/check_load_test.js
k6 run tests/k6/batch_check_load_test.js
k6 run tests/k6/write_load_test.js

# Generate comparison report
./scripts/compare_benchmarks.sh
```

## Migration from OpenFGA

### Export Data

```bash
# Export tuples from OpenFGA
openfga export --store-id=<store-id> > data.json
```

### Import to RSFGA

```bash
# Import into RSFGA
rsfga import --store-id=<store-id> < data.json
```

### Gradual Rollout

1. **Shadow Mode**: Mirror OpenFGA data, compare results
2. **Canary**: Route 1% traffic to RSFGA
3. **Full Migration**: Route 100% traffic
4. **Decommission**: Remove OpenFGA after stability period

See Migration Guide (to be written) for details.

## Contributing

Contributions are welcome! See [CONTRIBUTING.md](./CONTRIBUTING.md) for guidelines.

### Development Workflow

1. Create feature branch from `main`
2. Implement changes with tests
3. Run `cargo test`, `cargo fmt`, `cargo clippy`
4. Submit PR with description of changes
5. Ensure CI passes (tests, benchmarks, compatibility)

### Areas for Contribution

- **Core**: Graph resolver, storage backends, API handlers
- **Performance**: Optimization, profiling, benchmarking
- **Testing**: Compatibility tests, integration tests, load tests
- **Documentation**: Guides, examples, API docs
- **Tools**: CLI, migration tools, deployment automation

## License

Apache 2.0 (same as OpenFGA)

## References

### OpenFGA Resources

- [OpenFGA Official Website](https://www.openfga.dev/)
- [OpenFGA GitHub Repository](https://github.com/openfga/openfga)
- [OpenFGA Documentation](https://openfga.dev/docs)
- [OpenFGA Playground](https://play.openfga.dev/)

### Research Documents

- [Architecture Review](https://github.com/julianshen/openfga/blob/main/ARCHITECTURE_REVIEW.md)
- [Batch Check Analysis](https://github.com/julianshen/openfga/blob/main/BATCH_CHECK_ANALYSIS.md)
- [OpenFGA Research Docs](https://github.com/julianshen/openfga/tree/main/docs)

### Papers & Articles

- [Google Zanzibar Paper](https://research.google/pubs/pub48190/) - Inspiration for OpenFGA
- [Relationship-Based Access Control](https://en.wikipedia.org/wiki/Relationship-based_access_control)

## Roadmap Timeline

```
┌────────────┬────────────┬────────────┬────────────┐
│  Weeks 1-4 │ Weeks 5-8  │ Weeks 9-12 │ Weeks 13+  │
├────────────┼────────────┼────────────┼────────────┤
│ Foundation │  Storage   │  API &     │Precompute  │
│Type System │  Graph     │  Testing   │Edge Deploy │
│            │  Resolver  │  Benchmark │            │
└────────────┴────────────┴────────────┴────────────┘
              MVP Complete ↑           Phase 2/3 →
```

See [ROADMAP.md](./docs/design/ROADMAP.md) for detailed breakdown.

## Contact

- **Issues**: [GitHub Issues](https://github.com/yourusername/rsfga/issues)
- **Discussions**: [GitHub Discussions](https://github.com/yourusername/rsfga/discussions)
- **Email**: your.email@example.com (for security issues)

## Acknowledgments

- **OpenFGA Team**: For creating the original implementation and excellent documentation
- **Google Zanzibar**: For pioneering the authorization model
- **Rust Community**: For the amazing ecosystem and tools

---

**Status**: 🔬 Research & Design Complete | 🚧 Implementation Starting Soon

Built with ❤️ in Rust
