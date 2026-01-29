# RSFGA Load Testing Suite

Comprehensive k6 load test suite for validating RSFGA performance against documented targets.

## Performance Targets

| Operation | OpenFGA Baseline | RSFGA Target | Confidence |
|-----------|------------------|--------------|------------|
| Check | 483 req/s | 1000+ req/s | 60% (unvalidated) |
| Batch-Check | 23 checks/s | 500+ checks/s | 60% (unvalidated) |
| Write | 59 req/s | 150+ req/s | 60% (unvalidated) |
| List-Objects | 52 req/s | 100+ req/s | 60% (unvalidated) |
| Check p95 latency | 22ms | <20ms | 60% (unvalidated) |

## Quick Start

### Prerequisites

1. **Install k6**:
   ```bash
   # macOS
   brew install k6

   # Linux (Debian/Ubuntu)
   sudo gpg --no-default-keyring --keyring /usr/share/keyrings/k6-archive-keyring.gpg --keyserver hkp://keyserver.ubuntu.com:80 --recv-keys C5AD17C747E3415A3642D57D77C6C491D6AC1D69
   echo "deb [signed-by=/usr/share/keyrings/k6-archive-keyring.gpg] https://dl.k6.io/deb stable main" | sudo tee /etc/apt/sources.list.d/k6.list
   sudo apt-get update && sudo apt-get install k6
   ```

2. **Start RSFGA server**:
   ```bash
   cargo run --release --bin rsfga-server
   ```

### Run Tests

```bash
# Run a single scenario
./scripts/run-suite.sh check-direct

# Run all check scenarios
./scripts/run-suite.sh check

# Run the full suite
./scripts/run-suite.sh all

# Run with custom URL
./scripts/run-suite.sh mixed --url http://localhost:8080

# Dry run (show commands without executing)
./scripts/run-suite.sh all --dry-run
```

## Directory Structure

```
load-tests/
├── k6/
│   ├── scenarios/              # Test scenarios
│   │   ├── check-direct.js     # Direct relation checks
│   │   ├── check-computed.js   # Union/intersection checks
│   │   ├── check-deep-hierarchy.js # 10-15 level traversals
│   │   ├── batch-check.js      # Batch operations
│   │   ├── list-objects.js     # ListObjects API
│   │   ├── list-users.js       # ListUsers API
│   │   ├── expand.js           # Expand API
│   │   ├── write-tuples.js     # Write throughput
│   │   └── mixed-workload.js   # Production-like traffic
│   ├── data/
│   │   ├── models/             # Authorization models
│   │   │   ├── simple.json
│   │   │   ├── deep-hierarchy.json
│   │   │   ├── complex-unions.json
│   │   │   ├── exclusion.json
│   │   │   └── cel-conditions.json
│   │   └── tuples/             # Pre-generated tuple data
│   ├── lib/
│   │   ├── client.js           # RSFGA HTTP client
│   │   ├── setup.js            # Test setup helpers
│   │   └── metrics.js          # Custom k6 metrics
│   └── config/
│       ├── load-test.json      # Standard load profile
│       ├── stress-test.json    # Find breaking points
│       ├── soak-test.json      # 1-hour sustained load
│       └── spike-test.json     # Traffic burst simulation
├── grafana/
│   └── dashboards/
│       └── rsfga-load-test.json # Grafana dashboard
├── scripts/
│   ├── generate-tuples.js      # Generate test data
│   └── run-suite.sh            # Run test suite
├── reports/                    # Generated reports
└── README.md
```

## Test Scenarios

### 1. Check API - Direct Relations (`check-direct.js`)

Tests direct relation checks without graph traversal.

**Expected Performance**:
- p95 latency: <5ms
- Cache hit rate: >90%
- Error rate: <0.1%

```bash
./scripts/run-suite.sh check-direct
```

### 2. Check API - Computed Relations (`check-computed.js`)

Tests union, intersection, and computed relations with 5+ branches.

**Expected Performance**:
- p95 latency: <50ms
- Error rate: <1%

```bash
./scripts/run-suite.sh check-computed
```

### 3. Check API - Deep Hierarchy (`check-deep-hierarchy.js`)

Tests graph traversal through 10-15 level hierarchies.

**Expected Performance**:
- p95 latency: <100ms
- No depth limit exceeded errors
- Error rate: <1%

```bash
./scripts/run-suite.sh check-deep
```

### 4. Batch Check (`batch-check.js`)

Tests batch check operations with deduplication.

**Expected Performance**:
- >500 checks/s
- Visible deduplication savings
- Error rate: <1%

```bash
./scripts/run-suite.sh batch-check
```

### 5. ListObjects (`list-objects.js`)

Tests ListObjects with users having access to 100-1000 objects.

**Expected Performance**:
- p95 latency: <200ms
- Average >50 objects returned
- Error rate: <1%

```bash
./scripts/run-suite.sh list-objects
```

### 6. ListUsers (`list-users.js`)

Tests ListUsers with wildcard and userset expansion.

**Expected Performance**:
- p95 latency: <300ms
- Error rate: <1%

```bash
./scripts/run-suite.sh list-users
```

### 7. Write Throughput (`write-tuples.js`)

Tests write and delete operations.

**Expected Performance**:
- >150 req/s sustained
- p95 latency: <50ms
- Error rate: <1%

```bash
./scripts/run-suite.sh write
```

### 8. Mixed Workload (`mixed-workload.js`)

Simulates realistic production traffic:
- 60% checks
- 25% batch checks
- 10% list operations
- 5% writes

**Expected Performance**:
- All individual thresholds pass
- 500 req/s total for 10 minutes

```bash
./scripts/run-suite.sh mixed
```

## Load Profiles

### Standard Load Test

Validates performance under expected production load.
- Duration: 16 minutes
- Peak: 200 req/s

### Stress Test

Finds system breaking points.
- Duration: 10 minutes
- Peak: 2000 req/s

### Soak Test

Detects memory leaks and degradation.
- Duration: 65 minutes
- Sustained: 200 req/s

### Spike Test

Tests sudden traffic bursts.
- Duration: 5 minutes
- Normal: 50 req/s, Spike: 1000 req/s

## Grafana Integration

### Setup with InfluxDB

1. **Start InfluxDB**:
   ```bash
   docker run -d -p 8086:8086 \
     -e INFLUXDB_DB=k6 \
     -e INFLUXDB_ADMIN_USER=admin \
     -e INFLUXDB_ADMIN_PASSWORD=admin \
     influxdb:1.8
   ```

2. **Run tests with InfluxDB output**:
   ```bash
   ./scripts/run-suite.sh mixed --influxdb http://localhost:8086/k6
   ```

3. **Import dashboard**:
   - Open Grafana
   - Import `grafana/dashboards/rsfga-load-test.json`
   - Select your InfluxDB data source

### Dashboard Features

- **Overview Panel**: p95 latency, error rate, request rate, VUs, cache hit rate
- **Latency Panel**: Response time percentiles, latency by endpoint
- **Throughput Panel**: Request rate, operations by type
- **Authorization Metrics**: Check allowed rate, list operation results
- **Errors Panel**: Error rates, timeout rates, depth limit errors

## Generating Test Data

Generate tuple datasets for different scales:

```bash
# Generate small dataset (10K tuples)
node scripts/generate-tuples.js small

# Generate medium dataset (100K tuples)
node scripts/generate-tuples.js medium

# Generate large dataset (1M tuples)
node scripts/generate-tuples.js large

# Generate all datasets
node scripts/generate-tuples.js all
```

## Custom Metrics

The test suite tracks these custom metrics:

| Metric | Description |
|--------|-------------|
| `check_latency` | Check operation latency |
| `cache_hit_rate` | Estimated cache hit rate |
| `graph_depth` | Graph traversal depth |
| `check_allowed_rate` | Rate of allowed checks |
| `checks_per_batch` | Checks per batch request |
| `dedup_rate` | Deduplication rate |
| `write_latency` | Write operation latency |
| `write_throughput` | Tuples written per second |
| `objects_returned` | Objects returned per ListObjects |
| `users_returned` | Users returned per ListUsers |
| `error_rate` | Overall error rate |
| `timeout_rate` | Timeout error rate |
| `depth_limit_rate` | Depth limit exceeded rate |

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `RSFGA_URL` | `http://localhost:8080` | RSFGA server URL |
| `USER_COUNT` | scenario-specific | Number of test users |
| `OBJECT_COUNT` | scenario-specific | Number of test objects |
| `GROUP_COUNT` | scenario-specific | Number of test groups |
| `BATCH_SIZE_MIN` | 25 | Minimum batch size |
| `BATCH_SIZE_MAX` | 50 | Maximum batch size |
| `DUPLICATE_RATE` | 0.3 | Duplicate rate in batches |

## Interpreting Results

### Successful Test

```
✓ http_req_duration..............: avg=12.5ms  p(95)=18.2ms  p(99)=35.1ms
✓ http_req_failed................: 0.00%  ✓ 0  ✗ 25420
✓ check_latency..................: avg=12.3ms  p(95)=17.8ms
✓ cache_hit_rate.................: 92.5%
```

### Failed Thresholds

```
✗ http_req_duration..............: avg=45.2ms  p(95)=125.3ms  p(99)=250.1ms
  ✓ p(95)<20...................: 125.3ms (FAILED)
```

### Common Issues

1. **High latency**: Check database connection pool, cache configuration
2. **High error rate**: Check server logs, memory usage
3. **Low cache hit rate**: Review cache TTL settings
4. **Depth limit errors**: Model may have too deep hierarchies

## Comparing with OpenFGA

To establish baseline:

1. Run the same scenarios against OpenFGA
2. Compare metrics side-by-side
3. Document performance delta

```bash
# Run against OpenFGA
RSFGA_URL=http://localhost:8080 ./scripts/run-suite.sh mixed

# Run against RSFGA
RSFGA_URL=http://localhost:8081 ./scripts/run-suite.sh mixed
```

## Troubleshooting

### k6 not found

Install k6 following the instructions above.

### Server connection refused

Ensure RSFGA server is running and accessible at the configured URL.

### Out of memory

Reduce `USER_COUNT` and `OBJECT_COUNT` environment variables.

### Tests timing out

Increase timeout in k6 options or check server performance.
