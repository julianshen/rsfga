# Edge Sync Deployment Guide

This guide covers deploying and operating `rsfga-edge`, the distributed edge sync daemon that replicates authorization data from a central RSFGA instance to edge nodes via NATS JetStream.

## Overview

`rsfga-edge` subscribes to the `RSFGA_EVENTS` JetStream stream and applies committed tuple writes and deletes to a local storage backend. This enables low-latency authorization checks at edge locations with eventual consistency.

**Key characteristics:**

- **At-least-once delivery** with idempotent application via sync watermarks
- **Optional store filtering** to replicate only specific stores to each edge
- **Graceful shutdown** with SIGINT/SIGTERM handling
- **Prometheus metrics** for monitoring sync lag, throughput, and errors

## Prerequisites

1. **NATS server with JetStream enabled** (v2.9+)
2. **Central RSFGA API** with NATS async writes enabled (`--nats-url`)
3. **rsfga-writer** daemon running to consume writes and publish `CommittedEvent` to `RSFGA_EVENTS`

The central write path is: Client -> RSFGA API -> `RSFGA_WRITES` stream -> rsfga-writer -> local storage + `RSFGA_EVENTS` stream -> rsfga-edge.

## CLI Reference

```
rsfga-edge [OPTIONS]
```

| Flag | Env Var | Default | Description |
|------|---------|---------|-------------|
| `--nats-url` | `NATS_URL` | `nats://localhost:4222` | NATS server URL |
| `--consumer-name` | `CONSUMER_NAME` | `rsfga-edge-1` | Unique consumer name per instance |
| `--store-filter` | `STORE_FILTER` | *(none, replicate all)* | Comma-separated store IDs to replicate |
| `--batch-size` | `BATCH_SIZE` | `100` | Messages per fetch batch |
| `--fetch-timeout-ms` | `FETCH_TIMEOUT_MS` | `500` | Fetch timeout in milliseconds |
| `--storage-backend` | `STORAGE_BACKEND` | `memory` | Backend: `memory`, `rocksdb`, `postgres`, `mysql` |
| `--database-url` | `DATABASE_URL` | *(none)* | Connection URL for postgres/mysql backends |
| `--rocksdb-path` | `ROCKSDB_PATH` | `/var/lib/rsfga-edge/rocksdb` | Data directory for RocksDB backend |
| `--metrics-port` | `METRICS_PORT` | `9091` | Prometheus metrics HTTP port |
| `--log-level` | `LOG_LEVEL` | `info` | Log level (overridden by `RUST_LOG` if set) |

## Configuration Examples

### Minimal (in-memory, all stores)

```bash
rsfga-edge --nats-url nats://nats:4222
```

### Docker Run (RocksDB, filtered stores)

```bash
docker run -d \
  --name rsfga-edge \
  -e NATS_URL=nats://nats:4222 \
  -e CONSUMER_NAME=edge-dc-west-1 \
  -e STORE_FILTER=store-abc,store-def \
  -e STORAGE_BACKEND=rocksdb \
  -e ROCKSDB_PATH=/data/rocksdb \
  -e METRICS_PORT=9091 \
  -v rsfga-edge-data:/data \
  ghcr.io/julianshen/rsfga-edge:latest
```

### Docker Compose

```yaml
services:
  nats:
    image: nats:2.10-alpine
    command: ["--jetstream"]
    ports:
      - "4222:4222"

  rsfga:
    image: ghcr.io/julianshen/rsfga:latest
    environment:
      NATS_URL: nats://nats:4222
      STORAGE_BACKEND: memory
    ports:
      - "8080:8080"

  rsfga-writer:
    image: ghcr.io/julianshen/rsfga-writer:latest
    environment:
      NATS_URL: nats://nats:4222
      STORAGE_BACKEND: memory

  rsfga-edge:
    image: ghcr.io/julianshen/rsfga-edge:latest
    environment:
      NATS_URL: nats://nats:4222
      CONSUMER_NAME: edge-1
      STORAGE_BACKEND: rocksdb
      ROCKSDB_PATH: /data/rocksdb
    volumes:
      - edge-data:/data

volumes:
  edge-data:
```

## Storage Backend Selection

| Backend | Use Case | Persistence | Performance |
|---------|----------|-------------|-------------|
| `memory` | Development, testing | None (lost on restart) | Fastest |
| `rocksdb` | **Recommended for edge** | Disk-based, durable | Fast reads, good for auth |
| `postgres` | When Postgres is available at the edge | Full ACID | Moderate overhead |
| `mysql` | When MySQL is available at the edge | Full ACID | Moderate overhead |

**Recommendation:** Use `rocksdb` for production edge deployments. It provides durable local storage without requiring a separate database server, making it ideal for edge nodes where simplicity matters.

For `rocksdb`, the binary must be compiled with the `rocksdb` feature:

```bash
cargo build --release -p rsfga-edge --features rocksdb
```

## Monitoring

### Prometheus Metrics

The edge daemon exposes metrics on the configured `--metrics-port` (default: 9091).

**Counters:**

| Metric | Description |
|--------|-------------|
| `rsfga_edge_events_consumed_total` | Total events fetched from NATS |
| `rsfga_edge_events_applied_total` | Events successfully written to local storage |
| `rsfga_edge_events_skipped_total` | Events skipped (already applied, idempotency) |
| `rsfga_edge_events_failed_total` | Events that failed to apply |
| `rsfga_edge_tuples_written_total` | Total tuples written |
| `rsfga_edge_tuples_deleted_total` | Total tuples deleted |
| `rsfga_edge_cache_invalidations_total` | Cache entries invalidated on event receipt |
| `rsfga_edge_storage_failures_total` | Storage backend write failures |

**Gauges:**

| Metric | Labels | Description |
|--------|--------|-------------|
| `rsfga_edge_sync_position` | `store_id` | Last applied sequence per store |
| `rsfga_edge_sync_lag_events` | `store_id` | Events behind central node |

**Histograms:**

| Metric | Description |
|--------|-------------|
| `rsfga_edge_storage_write_duration_seconds` | Storage write latency |

### Key Alerts

- **`rsfga_edge_events_failed_total` increasing**: Investigate storage backend health
- **`rsfga_edge_sync_lag_events` growing**: Edge is falling behind; check NATS connectivity and storage performance
- **`rsfga_edge_storage_failures_total` increasing**: Storage backend issues (disk full, connection errors)

## Troubleshooting

### Edge not receiving events

1. Verify NATS connectivity: `nats sub "rsfga.events.>"`
2. Check that `rsfga-writer` is running and publishing to `RSFGA_EVENTS`
3. Verify the consumer name is unique (shared names cause split delivery)
4. Check `--store-filter` isn't excluding the expected stores

### Events consumed but not applied

1. Check `rsfga_edge_events_skipped_total` - events may already be applied (normal after restart)
2. Check `rsfga_edge_events_failed_total` - storage write failures
3. Check `rsfga_edge_storage_failures_total` - underlying storage errors
4. Review logs at `RUST_LOG=debug` for detailed event processing

### High sync lag

1. Increase `--batch-size` (default: 100) for higher throughput
2. Decrease `--fetch-timeout-ms` for more frequent fetches
3. Check storage backend latency via `rsfga_edge_storage_write_duration_seconds`
4. Consider using `rocksdb` instead of `postgres`/`mysql` for lower overhead

### Duplicate data after restart

This is expected and safe. The edge consumer uses at-least-once delivery with idempotent writes (upsert semantics). After restart, it may re-process some events before the watermark catches up. The sync watermark per store ensures no data is lost or incorrectly applied.

### Consumer name conflicts

Each edge instance **must** have a unique `--consumer-name`. If two instances share the same name, NATS splits message delivery between them, causing each to receive only a subset of events. Use distinct names like `edge-dc-west-1`, `edge-dc-east-1`.
