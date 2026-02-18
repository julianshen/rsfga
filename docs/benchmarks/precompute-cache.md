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

> Results below are placeholder values. Run the benchmarks on your hardware to collect actual numbers.

### Criterion (Micro-Benchmark)

| Benchmark | Time (mean) | Speedup |
|-----------|-------------|---------|
| `no_precompute_direct` | ~X us | baseline |
| `precompute_cache_hit` | ~Y ns | ~NNx |
| `precompute_cache_miss` | ~X us | ~1x (+ miss overhead) |
| `union_no_precompute` | ~Z us | baseline |
| `union_precompute_hit` | ~Y ns | ~NNx |
| `batch_25_no_precompute` | ~W us | baseline |
| `batch_25_all_precompute_hits` | ~V us | ~NNx |

### k6 (End-to-End)

| Metric | Without Precompute | With Precompute |
|--------|--------------------|--------------------|
| p50 latency | TBD ms | TBD ms |
| p95 latency | TBD ms | TBD ms |
| p99 latency | TBD ms | TBD ms |
| Throughput | TBD req/s | TBD req/s |
| Cache hit rate | N/A | TBD % |

## How to Reproduce

### Criterion

```bash
cargo bench -p rsfga-api --features precompute --bench precompute_bench
```

Results are written to `target/criterion/` with HTML reports.

### k6 (requires Docker)

```bash
# 1. Start base services (PostgreSQL + RSFGA without precompute)
docker-compose up -d rsfga postgres

# 2. Run baseline scenario (no precompute)
cd load-tests
./scripts/run-suite.sh check-precompute --url http://localhost:8080

# 3. Start precompute services (Valkey + precompute worker)
docker-compose --profile precompute up -d

# 4. Run with precompute (worker fills cache during warm-up phase)
./scripts/run-suite.sh check-precompute --url http://localhost:8080
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

```
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
