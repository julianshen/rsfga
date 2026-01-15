# CockroachDB Deployment Guide

This guide covers deploying RSFGA with CockroachDB as the storage backend.

## Table of Contents

- [Overview](#overview)
- [Compatibility](#compatibility)
- [Quick Start](#quick-start)
- [Connection Configuration](#connection-configuration)
- [CockroachDB-Specific Behaviors](#cockroachdb-specific-behaviors)
- [Multi-Region Deployment](#multi-region-deployment)
- [Performance Considerations](#performance-considerations)
- [Cluster Sizing](#cluster-sizing)
- [Best Practices](#best-practices)
- [Troubleshooting](#troubleshooting)

## Overview

CockroachDB is a distributed SQL database that provides:

- **PostgreSQL Compatibility**: Uses the PostgreSQL wire protocol
- **Automatic Sharding**: Data is automatically distributed across nodes
- **Strong Consistency**: Serializable isolation by default
- **Horizontal Scalability**: Add nodes to increase capacity
- **Fault Tolerance**: Survives node, zone, and region failures

RSFGA uses the `PostgresDataStore` to connect to CockroachDB, as CockroachDB implements the PostgreSQL wire protocol.

## Compatibility

| CockroachDB Version | RSFGA Compatibility | Notes |
|---------------------|---------------------|-------|
| 23.1+ (latest)      | Fully Compatible    | Recommended |
| 22.2.x              | Fully Compatible    | LTS release |
| 22.1.x              | Compatible          | - |
| 21.x                | Compatible          | Limited testing |

### Tested Operations

All RSFGA storage operations have been tested on CockroachDB:

- Store CRUD operations
- Tuple CRUD operations (single and batch)
- Authorization model operations
- Pagination with continuation tokens
- Concurrent read/write operations
- Large batch operations (1000+ tuples)

## Quick Start

### Single-Node (Development/Testing)

```bash
# Create Docker network
docker network create rsfga-network

# Start CockroachDB single-node
docker run --name rsfga-crdb \
    --network rsfga-network \
    -p 26257:26257 \
    -p 8090:8080 \
    -d cockroachdb/cockroach:latest \
    start-single-node --insecure

# Create database
docker exec -it rsfga-crdb cockroach sql --insecure \
    -e "CREATE DATABASE IF NOT EXISTS rsfga"

# Start RSFGA
docker run -d \
    --name rsfga \
    --network rsfga-network \
    -p 8080:8080 \
    -e RSFGA_STORAGE__BACKEND=postgres \
    -e RSFGA_STORAGE__DATABASE_URL="postgresql://root@rsfga-crdb:26257/rsfga" \
    ghcr.io/julianshen/rsfga:latest
```

### Multi-Node Cluster (Production)

See [Multi-Region Deployment](#multi-region-deployment) for production cluster setup.

## Connection Configuration

### Connection String Format

CockroachDB uses PostgreSQL-compatible connection strings:

```text
postgresql://[user[:password]@][host][:port]/[database][?options]
```

**Examples:**

```bash
# Insecure (development only)
postgresql://root@localhost:26257/rsfga

# With password
postgresql://rsfga_user:password@localhost:26257/rsfga

# With SSL (production)
postgresql://rsfga_user:password@crdb.example.com:26257/rsfga?sslmode=verify-full&sslrootcert=/certs/ca.crt

# Cluster with multiple nodes
postgresql://rsfga_user:password@node1:26257,node2:26257,node3:26257/rsfga?sslmode=verify-full
```

### Environment Variables

```bash
# Required
export RSFGA_STORAGE__BACKEND=postgres
export RSFGA_STORAGE__DATABASE_URL="postgresql://..."

# Optional (with RSFGA defaults shown)
export RSFGA_STORAGE__POOL_SIZE=10
export RSFGA_STORAGE__CONNECT_TIMEOUT_SECS=30    # Default: 30s
export RSFGA_STORAGE__QUERY_TIMEOUT_SECS=30      # Default: 30s
export RSFGA_STORAGE__READ_TIMEOUT_SECS=30       # Optional, falls back to QUERY_TIMEOUT
export RSFGA_STORAGE__WRITE_TIMEOUT_SECS=30      # Optional, falls back to QUERY_TIMEOUT
export RSFGA_STORAGE__HEALTH_CHECK_TIMEOUT_SECS=5  # Default: 5s
```

> **CockroachDB Note:** The default timeouts (30s) are generally appropriate for CockroachDB. For multi-region deployments with higher latency, consider increasing `query_timeout_secs` to 60s. For single-node development, the defaults work well.

### Config File

```yaml
storage:
  backend: postgres
  database_url: "postgresql://rsfga_user:password@crdb:26257/rsfga"
  pool_size: 10
  connect_timeout_secs: 30       # Time to establish connection
  query_timeout_secs: 30         # Default timeout for all queries
  read_timeout_secs: 30          # Optional: override for read operations
  write_timeout_secs: 60         # Optional: override for write operations (higher for multi-region)
  health_check_timeout_secs: 5   # Health check timeout
```

## CockroachDB-Specific Behaviors

### Transaction Isolation

CockroachDB uses **Serializable isolation** by default (the strongest isolation level), which differs from PostgreSQL's default of Read Committed.

**Implications for RSFGA:**

- **Stronger consistency**: All authorization checks see a consistent view of tuples
- **Potential for transaction retries**: Under high contention, CockroachDB may require transaction retries
- **No phantom reads**: Queries within a transaction see a consistent snapshot

**Note on Transaction Retries:** SQLx provides basic connection retry logic, but does not automatically handle CockroachDB's serializable transaction retries (error code 40001). Under high contention, applications may need additional retry logic. For most RSFGA workloads with proper indexing, transaction retries are rare.

### Distributed SQL Execution

CockroachDB distributes data across nodes using range partitioning. This affects query performance:

**Range Partitioning:**
- Data is split into ranges (default ~512MB each)
- Ranges are distributed across nodes
- Queries may need to access multiple nodes

**Impact on Tuple Queries:**
- Queries filtering by `store_id` are efficient (data locality)
- Full table scans are expensive (touch all nodes)
- Pagination is important for large result sets

### AUTO_INCREMENT / SERIAL Behavior

CockroachDB implements `SERIAL` differently than PostgreSQL:

- Uses unique_rowid() which generates globally unique IDs
- IDs are not strictly sequential (distributed generation)
- No ID collisions across nodes

This is transparent to RSFGA and requires no special handling.

### JSON Support

CockroachDB supports `JSONB` type compatible with PostgreSQL:

- Conditional tuples with JSON context work correctly
- JSON indexing is supported for query optimization
- Full JSON path expressions are available

## Multi-Region Deployment

### Topology Patterns

CockroachDB supports several multi-region patterns:

#### 1. Regional Tables (Recommended for RSFGA)

Data is pinned to the region where it's accessed most:

```sql
-- Create regional by row tables
ALTER DATABASE rsfga SET PRIMARY REGION = "us-east1";
ALTER DATABASE rsfga ADD REGION "us-west1";
ALTER DATABASE rsfga ADD REGION "eu-west1";

-- Stores table: regional by row
ALTER TABLE stores SET LOCALITY REGIONAL BY ROW;

-- Tuples table: regional by row
ALTER TABLE tuples SET LOCALITY REGIONAL BY ROW;
```

#### 2. Global Tables (For shared configuration)

Data is replicated to all regions (higher write latency):

```sql
-- For data that rarely changes but needs low-latency reads everywhere
ALTER TABLE authorization_models SET LOCALITY GLOBAL;
```

### Multi-Region Example

> **Note:** Multi-region CockroachDB deployments are complex and require careful planning. The example below is simplified for illustration. For production deployments, consult the [CockroachDB multi-region documentation](https://www.cockroachlabs.com/docs/stable/multiregion-overview.html) and consider using the official [CockroachDB Kubernetes Operator](https://github.com/cockroachdb/cockroach-operator).

```yaml
# Simplified Kubernetes StatefulSet example (single region shown)
# For true multi-region, deploy separate StatefulSets per region with
# appropriate --locality flags and node affinity rules
apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: cockroachdb
spec:
  serviceName: cockroachdb
  replicas: 3
  selector:
    matchLabels:
      app: cockroachdb
  template:
    metadata:
      labels:
        app: cockroachdb
    spec:
      containers:
      - name: cockroachdb
        image: cockroachdb/cockroach:latest
        ports:
        - containerPort: 26257
          name: grpc
        - containerPort: 8080
          name: http
        args:
        - start
        - --locality=region=us-east1,zone=us-east1-a
        - --join=cockroachdb-0.cockroachdb,cockroachdb-1.cockroachdb,cockroachdb-2.cockroachdb
        - --advertise-addr=$(POD_NAME).cockroachdb
        env:
        - name: POD_NAME
          valueFrom:
            fieldRef:
              fieldPath: metadata.name
        volumeMounts:
        - name: datadir
          mountPath: /cockroach/cockroach-data
  volumeClaimTemplates:
  - metadata:
      name: datadir
    spec:
      accessModes: ["ReadWriteOnce"]
      resources:
        requests:
          storage: 100Gi
---
# Headless service for StatefulSet DNS
apiVersion: v1
kind: Service
metadata:
  name: cockroachdb
spec:
  clusterIP: None
  selector:
    app: cockroachdb
  ports:
  - port: 26257
    name: grpc
  - port: 8080
    name: http
```

## Performance Considerations

> **Disclaimer:** The performance metrics below are approximate estimates based on typical deployments with standard hardware configurations. Actual performance varies significantly based on hardware specifications, network topology, data volume, workload patterns, and cluster configuration. These figures should be used as rough guidelines only. Always benchmark your specific workload before production deployment.

### Read Performance

| Operation | Single-Node | 3-Node Cluster | 9-Node (Multi-Region) |
|-----------|-------------|----------------|----------------------|
| Read tuple by key | ~1ms | ~2-3ms | ~5-10ms (local) |
| List tuples (100) | ~5ms | ~10ms | ~20ms |
| Pagination | ~5ms | ~10ms | ~20ms |

### Write Performance

| Operation | Single-Node | 3-Node Cluster | 9-Node (Multi-Region) |
|-----------|-------------|----------------|----------------------|
| Write single tuple | ~5ms | ~10ms | ~50-100ms |
| Batch write (100) | ~20ms | ~40ms | ~100-200ms |
| Delete tuple | ~5ms | ~10ms | ~50-100ms |

**Notes:**
- Multi-region writes are slower due to consensus across regions
- Read performance can be optimized with follower reads (eventual consistency)
- Batch operations are more efficient than individual writes

### Optimizing for CockroachDB

1. **Use batch operations**: Combine multiple writes into single transactions
2. **Filter by store_id**: Ensures query locality
3. **Appropriate page sizes**: Use pagination to limit result sets
4. **Connection pooling**: Reuse connections to reduce latency

### Index Recommendations

RSFGA's default indexes are optimized for CockroachDB:

```sql
-- Primary indexes (created automatically)
CREATE UNIQUE INDEX idx_tuples_unique ON tuples (
    store_id, object_type, object_id, relation,
    user_type, user_id, COALESCE(user_relation, '')
);

-- Performance indexes
CREATE INDEX idx_tuples_store ON tuples(store_id);
CREATE INDEX idx_tuples_object ON tuples(store_id, object_type, object_id);
CREATE INDEX idx_tuples_user ON tuples(store_id, user_type, user_id);
CREATE INDEX idx_tuples_relation ON tuples(store_id, object_type, object_id, relation);
CREATE INDEX idx_tuples_store_relation ON tuples(store_id, relation);
CREATE INDEX idx_tuples_condition ON tuples(store_id, condition_name) WHERE condition_name IS NOT NULL;
```

> **Index Tuning Caveat:** These indexes cover common RSFGA query patterns. For specific workloads with unusual access patterns, use `EXPLAIN ANALYZE` to identify missing indexes. Be cautious about adding indexes as they increase write latency and storage. Monitor index usage with `SHOW STATISTICS` and remove unused indexes.

## Cluster Sizing

> **Note:** The sizing recommendations below are starting points based on general workload characteristics. Your actual requirements may vary significantly. Always benchmark with representative data and traffic patterns before finalizing production sizing.

### Recommendations by Workload

| Workload | Tuples | Check QPS | Nodes | vCPUs/Node | Memory/Node |
|----------|--------|-----------|-------|------------|-------------|
| Small    | <100K  | <100      | 3     | 2          | 8GB         |
| Medium   | <1M    | <1000     | 3     | 4          | 16GB        |
| Large    | <10M   | <5000     | 5     | 8          | 32GB        |
| X-Large  | <100M  | <20000    | 9+    | 16         | 64GB        |

### Storage Requirements

- **Tuples**: ~500 bytes per tuple (with indexes)
- **Stores**: ~200 bytes per store
- **Authorization Models**: ~10KB per model (varies with complexity)

**Example calculation:**
- 1 million tuples ≈ 500MB data + 500MB indexes = ~1GB
- Replication factor 3 = ~3GB total storage

### Connection Pool Sizing

```text
Recommended pool size = (nodes × cores) / rsfga_instances
```

**Example:**
- 3-node cluster, 4 cores each = 12 total cores
- 2 RSFGA instances
- Pool size per instance = 12 / 2 = 6 (use 10 for headroom)

> **Caveat:** This formula is a starting point. Actual optimal pool size depends on query complexity, connection overhead, and workload patterns. Monitor connection wait times and adjust accordingly. Too many connections can overwhelm the cluster; too few can cause request queuing.

## Best Practices

### 1. Use SSL in Production

```bash
# Generate certificates
cockroach cert create-ca --certs-dir=certs --ca-key=certs/ca.key
cockroach cert create-node localhost crdb.example.com --certs-dir=certs --ca-key=certs/ca.key
cockroach cert create-client rsfga_user --certs-dir=certs --ca-key=certs/ca.key

# Connection string with SSL
postgresql://rsfga_user@crdb:26257/rsfga?sslmode=verify-full&sslrootcert=/certs/ca.crt&sslcert=/certs/client.rsfga_user.crt&sslkey=/certs/client.rsfga_user.key
```

> **Security Note:** Never store database credentials in plain text configuration files or environment variables in production. Use secrets management solutions such as:
> - Kubernetes Secrets (with encryption at rest)
> - HashiCorp Vault
> - AWS Secrets Manager / GCP Secret Manager / Azure Key Vault
> - Environment variable injection from CI/CD pipelines

### 2. Configure Appropriate Timeouts

```yaml
storage:
  connect_timeout_secs: 30       # Allow for cluster discovery
  query_timeout_secs: 30         # Default for all queries
  write_timeout_secs: 60         # Higher for multi-region writes
  health_check_timeout_secs: 5   # Quick health checks
```

> **Tip:** For multi-region deployments, increase `write_timeout_secs` to accommodate cross-region consensus latency.

### 3. Monitor Cluster Health

Key metrics to monitor:

- `sql.conn.latency`: Connection establishment time
- `sql.exec.latency`: Query execution time
- `ranges.unavailable`: Unavailable data ranges (should be 0)
- `liveness.heartbeatlatency`: Node health

### 4. Plan for Maintenance

- CockroachDB supports rolling upgrades with zero downtime
- Node decommission is graceful (data rebalances automatically)
- Schema changes are online (no table locks)

### 5. Use Dedicated User

```sql
-- Create dedicated user for RSFGA
CREATE USER rsfga_user WITH PASSWORD 'secure_password';

-- Grant necessary permissions
GRANT ALL ON DATABASE rsfga TO rsfga_user;
GRANT ALL ON ALL TABLES IN SCHEMA public TO rsfga_user;
```

## Troubleshooting

### Connection Timeout

**Symptom:** `connection timed out` errors

**Solutions:**
1. Verify CockroachDB is running: `cockroach node status`
2. Check firewall allows port 26257
3. Increase `connection_timeout_secs`
4. Verify SSL certificates if using secure mode

### Transaction Retry Errors

**Symptom:** `TransactionRetryError` or `restart transaction` messages

**Solutions:**
1. This is normal under high contention
2. RSFGA handles retries automatically via SQLx
3. If frequent, consider:
   - Reducing batch sizes
   - Adding application-level retry logic
   - Scaling the cluster

### Slow Queries

**Symptom:** High latency on tuple operations

**Solutions:**
1. Verify indexes exist: `SHOW INDEXES FROM tuples`
2. Check query plans: `EXPLAIN ANALYZE SELECT ...`
3. Ensure queries filter by `store_id`
4. Review cluster CPU/memory utilization

### Range Unavailable

**Symptom:** `range unavailable` errors

**Solutions:**
1. Check node status: `cockroach node status`
2. Verify replication factor: `SHOW ZONE CONFIGURATION FOR DATABASE rsfga`
3. Wait for automatic recovery (usually <1 minute)
4. If persistent, check node logs

## Running Integration Tests

```bash
# Start CockroachDB
docker run --name rsfga-crdb \
    -p 26257:26257 \
    -p 8090:8080 \
    -d cockroachdb/cockroach:latest \
    start-single-node --insecure

# Create database
docker exec -it rsfga-crdb cockroach sql --insecure \
    -e "CREATE DATABASE IF NOT EXISTS rsfga"

# Set environment variable
export COCKROACHDB_URL=postgresql://root@localhost:26257/rsfga

# Run tests
cargo test -p rsfga-storage --test cockroachdb_integration -- --ignored --test-threads=1

# Cleanup
docker rm -f rsfga-crdb
```

## Related Documentation

- [Deployment Guide](DEPLOYMENT.md) - General deployment instructions
- [Migration Guide](MIGRATION.md) - Migrating from OpenFGA
- [CockroachDB Official Docs](https://www.cockroachlabs.com/docs/)
