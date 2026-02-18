# Precompute Cache Benchmarks

Benchmarks comparing RSFGA check latency with and without the Valkey-backed precompute cache introduced in v2.1.0.

## Methodology

### Criterion Micro-Benchmarks

Isolate the speedup factor by comparing domain-level graph resolution against a simulated Valkey cache hit (in-memory HashMap + JSON deserialization). No real Valkey instance is required.

**Benchmark groups:**

| Group | What it measures |
|-------|------------------|
| `precompute_comparison/no_precompute_direct` | Full graph resolution for a direct relation (baseline) |
| `precompute_comparison/precompute_cache_hit` | HashMap lookup + JSON deser (simulated Valkey hit) |
| `precompute_comparison/precompute_cache_miss` | Cache miss + full graph resolution |
| `precompute_union/union_no_precompute` | Union relation resolution (2-branch) |
| `precompute_union/union_precompute_hit` | Same union, served from cache |
| `precompute_batch/batch_25_no_precompute` | 25 parallel graph resolutions |
| `precompute_batch/batch_25_all_precompute_hits` | 25 cache lookups |

### k6 End-to-End Load Tests

Full-stack comparison using Docker services. A two-stage scenario:

1. **Warm-up** -- Sends checks to populate the hot-path registry, then waits for the precompute worker to fill the Valkey cache.
2. **Measurement** -- Constant-arrival-rate (200 req/s) for 3 minutes.

**Key metrics:**

| Metric | Description |
|--------|-------------|
| `check_latency` p50/p95/p99 | Overall check latency |
| `precompute_latency` p50/p95/p99 | Latency during measurement phase |
| `precompute_hit_rate` | Fraction of responses < 1ms (estimated cache hits) |
| `check_throughput` | Sustained requests per second |

## Results

> **Confidence: unvalidated (~60%)** — targets become validated only after running the benchmarks described below. See [ADR Validation Status](../design/ARCHITECTURE_DECISIONS.md#validation-status-summary) for the project convention on performance claim confidence levels.

### Criterion (Micro-Benchmark)

Measured on Apple Silicon (M-series). Results will vary by hardware.

| Benchmark | Time (mean) | Throughput | Speedup |
|-----------|-------------|------------|---------|
| `no_precompute_direct` | 390 ns | 2.56 Melem/s | baseline |
| `precompute_cache_hit` | 116 ns | 8.65 Melem/s | **3.4x** |
| `precompute_cache_miss` | 374 ns | 2.67 Melem/s | ~1x (miss overhead negligible) |
| `union_no_precompute` | 368 ns | 2.72 Melem/s | baseline |
| `union_precompute_hit` | 119 ns | 8.43 Melem/s | **3.1x** |
| `batch_25_no_precompute` | 11.0 us | 2.27 Melem/s | baseline |
| `batch_25_all_precompute_hits` | 6.4 us | 3.90 Melem/s | **1.7x** |

**Key takeaway**: The simulated cache hit path (HashMap + JSON deser) is **3-3.4x faster** than full graph resolution for individual checks. In production, the actual Valkey lookup adds network RTT (~0.1-0.5ms on localhost), but the total remains well under the 1ms target for co-located deployments.

### k6 (End-to-End)

Measured on Apple Silicon (M-series), PostgreSQL 14 (localhost), 50 users, 20 objects, 200 req/s constant-arrival-rate for 3 minutes.

| Metric | Without Precompute | With Precompute | Notes |
|--------|--------------------|--------------------|----|
| p50 latency | 3.10 ms | 3.22 ms | |
| p95 latency | 5.64 ms | 4.53 ms | |
| p99 latency | 13.7 ms | 10.66 ms | |
| max latency | 174 ms | 63 ms | Tail variance reduced |
| Throughput | 118 req/s | 118 req/s | Capped by constant-arrival-rate executor |
| Cache hit rate | N/A | ~0% | See note below |

> **Note on cache hit rate**: The current k6 scenario writes tuples during setup (generating NATS committed events) *before* the warmup phase creates hot-path entries via check requests. The precompute worker only scans the hot-path on committed events, so it finds no entries to precompute. A future improvement would add a post-warmup write trigger to force the worker to re-scan the populated hot-path. The latency improvements at p95/p99 are likely attributable to application-level caching (Moka) warming up during the warmup phase, not the Valkey precompute cache.

## How to Reproduce

### Criterion

```bash
cargo bench -p rsfga-api --features precompute --bench precompute_bench
```

Results are written to `target/criterion/` with HTML reports.

### k6 (requires Docker)

```bash
# All commands run from the project root

# 1. Start base services (PostgreSQL + RSFGA without precompute)
docker-compose up -d rsfga postgres

# 2. Run baseline scenario (no precompute)
(cd load-tests && ./scripts/run-suite.sh check-precompute --url http://localhost:8080)

# 3. Start precompute services (Valkey + precompute worker)
docker-compose --profile precompute up -d

# 4. Run with precompute (worker fills cache during warm-up phase)
(cd load-tests && ./scripts/run-suite.sh check-precompute --url http://localhost:8080)
```

Compare the two k6 summary reports to see latency and throughput differences.

### Environment Variables for k6

| Variable | Default | Description |
|----------|---------|-------------|
| `RSFGA_URL` | `http://localhost:8080` | RSFGA server URL |
| `USER_COUNT` | 500 | Number of users in test data |
| `OBJECT_COUNT` | 50 | Number of objects in test data |
| `WARMUP_WAIT` | 10 | Seconds to wait after warm-up for cache population |

## Architecture

```text
Client (k6 / Criterion)
    |
    v
RSFGA HTTP API
    |
    +---> Valkey (precompute cache)  <-- cache hit: ~sub-ms
    |         |
    |         +-- miss --> hot-path registry (ZADD)
    |
    +---> Graph Resolver             <-- cache miss: full resolution
              |
              v
          Storage (Postgres / Memory)
```

The precompute worker runs as a separate process that reads the hot-path registry from Valkey, executes the check via the graph resolver, and stores the result back in Valkey with a configurable TTL.
