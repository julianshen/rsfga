# RSFGA Edge Architecture with NATS

## Executive Summary

This document describes the distributed edge architecture for RSFGA using **NATS** as the messaging backbone. NATS provides a lightweight, high-performance pub/sub system with JetStream for persistence and NATS Leaf Nodes for edge connectivity.

## Why NATS Over Kafka

| Aspect | NATS | Kafka |
|--------|------|-------|
| **Footprint** | 10-20MB binary | ~500MB+ JVM |
| **Latency** | Sub-millisecond | 5-10ms minimum |
| **Edge Support** | Native leaf nodes | Requires MirrorMaker/bridges |
| **Ops Complexity** | Simple, single binary | Complex (Zookeeper/Raft, tuning) |
| **At-Most-Once** | Native support | Requires configuration |
| **Multi-tenancy** | Built-in accounts | Manual topic management |
| **Exactly-Once** | JetStream | Complex configuration |

**Key Advantages for Edge Deployment**:
- **Leaf Nodes**: Purpose-built for edge scenarios with auto-reconnect
- **Lightweight**: Can run on resource-constrained edge nodes
- **Zero Configuration**: Auto-discovery and dynamic routing
- **Built-in Security**: TLS, accounts, and user authentication

---

## Architecture Overview

### Three-Tier Topology

```
┌─────────────────────────────────────────────────────────────┐
│                     Edge Tier (Global)                       │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐      │
│  │  Edge Node   │  │  Edge Node   │  │  Edge Node   │      │
│  │  (Tokyo)     │  │  (London)    │  │  (NYC)       │      │
│  │              │  │              │  │              │      │
│  │ NATS Leaf    │  │ NATS Leaf    │  │ NATS Leaf    │      │
│  │ Local Store  │  │ Local Store  │  │ Local Store  │      │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘      │
│         └──────────────────┼──────────────────┘              │
└────────────────────────────┼─────────────────────────────────┘
                             │ NATS Leaf Connection
┌────────────────────────────┼─────────────────────────────────┐
│                  Regional Tier (3 Regions)                    │
│  ┌──────────────────────────┼─────────────────────────────┐  │
│  │         NATS Cluster     ▼                             │  │
│  │  ┌─────────┐  ┌─────────┐  ┌─────────┐               │  │
│  │  │ NATS 1  │──│ NATS 2  │──│ NATS 3  │ (JetStream)   │  │
│  │  └─────────┘  └─────────┘  └─────────┘               │  │
│  │                                                        │  │
│  │  ┌──────────────┐  ┌──────────────┐                  │  │
│  │  │  Regional    │  │  PostgreSQL  │                  │  │
│  │  │  API Nodes   │  │  Read Replica│                  │  │
│  │  └──────────────┘  └──────────────┘                  │  │
│  └────────────────────────┬───────────────────────────────┘  │
└───────────────────────────┼──────────────────────────────────┘
                            │ NATS Cluster Mesh
┌───────────────────────────┼──────────────────────────────────┐
│                    Central Tier (Global)                      │
│  ┌────────────────────────┼─────────────────────────────┐    │
│  │      NATS Super Cluster▼                             │    │
│  │  ┌─────────┐  ┌─────────┐  ┌─────────┐             │    │
│  │  │ NATS 1  │══│ NATS 2  │══│ NATS 3  │ (JetStream) │    │
│  │  └─────────┘  └─────────┘  └─────────┘             │    │
│  │                                                      │    │
│  │  ┌──────────────┐  ┌──────────────┐                │    │
│  │  │  Central API │  │  PostgreSQL  │                │    │
│  │  │  Nodes       │  │  Primary     │                │    │
│  │  └──────────────┘  └──────────────┘                │    │
│  └──────────────────────────────────────────────────────┘    │
└───────────────────────────────────────────────────────────────┘

Legend:
  ─── NATS Leaf Connection (edge → regional/central)
  ═══ NATS Cluster Mesh (full mesh between nodes)
  ─── NATS Super Cluster (gateway between regions)
```

---

## NATS Configuration

### Central NATS Cluster

```conf
# central-nats.conf
server_name: central-1
listen: 0.0.0.0:4222
http: 0.0.0.0:8222

# JetStream for persistence
jetstream {
    store_dir: /data/nats/jetstream
    max_memory_store: 10GB
    max_file_store: 100GB
}

# Cluster configuration (full mesh with other central nodes)
cluster {
    name: central
    listen: 0.0.0.0:6222
    routes: [
        nats://central-2:6222
        nats://central-3:6222
    ]
}

# Gateway to regional clusters
gateway {
    name: central
    listen: 0.0.0.0:7222
    gateways: [
        {name: us-west, urls: ["nats://us-west-1:7222"]},
        {name: eu-west, urls: ["nats://eu-west-1:7222"]},
        {name: ap-east, urls: ["nats://ap-east-1:7222"]}
    ]
}

# Accounts for multi-tenancy
accounts {
    SYSTEM: {
        users: [{user: admin, password: $ADMIN_PASS}]
    }
    RSFGA: {
        jetstream: enabled
        users: [
            {user: writer, password: $WRITER_PASS, permissions: {
                publish: ["rsfga.changes.>"]
                subscribe: ["_INBOX.>"]
            }}
            {user: reader, password: $READER_PASS, permissions: {
                subscribe: ["rsfga.changes.>", "_INBOX.>"]
            }}
        ]
    }
}

# TLS for security
tls {
    cert_file: "/etc/nats/tls/server.crt"
    key_file: "/etc/nats/tls/server.key"
    ca_file: "/etc/nats/tls/ca.crt"
    verify: true
}
```

### Regional NATS Cluster

```conf
# regional-nats.conf
server_name: us-west-1
listen: 0.0.0.0:4222
http: 0.0.0.0:8222

jetstream {
    store_dir: /data/nats/jetstream
    max_memory_store: 5GB
    max_file_store: 50GB
}

cluster {
    name: us-west
    listen: 0.0.0.0:6222
    routes: [
        nats://us-west-2:6222
        nats://us-west-3:6222
    ]
}

# Gateway to central cluster
gateway {
    name: us-west
    listen: 0.0.0.0:7222
    gateways: [
        {name: central, urls: ["nats://central-1:7222"]}
    ]
}

# Leaf node configuration for edge nodes
leafnodes {
    listen: 0.0.0.0:7422
    authorization {
        user: leaf_user
        password: $LEAF_PASS
        timeout: 2
    }
}
```

### Edge NATS Leaf Node

```conf
# edge-nats.conf
server_name: edge-tokyo-1
listen: 127.0.0.1:4222  # Local only
http: 127.0.0.1:8222

# Connect to regional cluster as leaf node
leafnodes {
    remotes: [
        {
            url: "nats://us-west-1:7422"
            credentials: "/etc/nats/creds/leaf.creds"

            # Automatic reconnect
            reconnect_time_wait: 5s
            max_reconnect_attempts: -1  # Infinite

            # Only subscribe to relevant subjects
            deny_import: [">"]
            deny_export: [">"]
        }
    ]
}

# Local JetStream for edge caching (optional)
jetstream {
    store_dir: /data/nats/jetstream
    max_memory_store: 1GB
    max_file_store: 10GB
}
```

---

## Data Flow with NATS

### Subject Hierarchy

```
rsfga.changes.{store_id}.{operation}.{object_type}

Examples:
- rsfga.changes.store-123.write.document
- rsfga.changes.store-123.delete.document
- rsfga.changes.store-456.write.user
- rsfga.changes.store-456.model.updated

Control subjects:
- rsfga.control.sync.{edge_id}
- rsfga.control.invalidate.{cache_key}
```

### Write Flow (Central → Edge)

```
┌─────────────┐
│   Client    │
└──────┬──────┘
       │ 1. Write Request (HTTP/gRPC)
       ▼
┌─────────────────┐
│  Central API    │
│                 │
│ 2. Write to DB  │
│ 3. Publish to   │
│    NATS         │
└────────┬────────┘
         │ NATS Publish
         │ Subject: rsfga.changes.store-123.write.document
         │ Payload: TupleChange (protobuf)
         ▼
┌──────────────────────────────────────────────────┐
│          NATS Central Cluster                    │
│                                                  │
│  ┌─────────────────────────────────────────┐    │
│  │       JetStream Stream                  │    │
│  │  Name: RSFGA_CHANGES                    │    │
│  │  Subjects: rsfga.changes.>              │    │
│  │  Retention: WorkQueue (at-least-once)   │    │
│  │  Replicas: 3                            │    │
│  └─────────────────────────────────────────┘    │
└────────┬─────────────────────────────────────────┘
         │ Gateway Mesh
         │
    ┌────┴─────┬────────────┐
    ▼          ▼            ▼
┌─────────┐ ┌─────────┐ ┌─────────┐
│Regional │ │Regional │ │Regional │
│ NATS    │ │ NATS    │ │ NATS    │
│US-West  │ │EU-West  │ │AP-East  │
└────┬────┘ └────┬────┘ └────┬────┘
     │           │           │
     │ Leaf      │ Leaf      │ Leaf
     │ Nodes     │ Nodes     │ Nodes
     ▼           ▼           ▼
┌─────────┐ ┌─────────┐ ┌─────────┐
│Edge Node│ │Edge Node│ │Edge Node│
│Tokyo    │ │London   │ │NYC      │
│         │ │         │ │         │
│4. Filter│ │4. Filter│ │4. Filter│
│5. Apply │ │5. Apply │ │5. Apply │
└─────────┘ └─────────┘ └─────────┘
```

### Read Flow (Edge)

```
┌─────────────┐
│   Client    │
└──────┬──────┘
       │ 1. Check Request
       ▼
┌─────────────────┐
│  Edge Node API  │
│                 │
│ 2. Check local  │
│    product      │
│    config       │
└────────┬────────┘
         │
    ┌────┴─────┐
    │          │
    ▼          ▼
┌─────────┐ ┌──────────────┐
│ Full    │ │ Cache/Forward│
│ Mode    │ │ Mode         │
│         │ │              │
│3. Query │ │3. Query cache│
│   Local │ │   or forward │
│   Store │ │   to regional│
│         │ │              │
│4. Resolve│ │4. Return or  │
│   Graph  │ │   forward    │
└─────────┘ └──────────────┘
```

---

## NATS Streams and Consumers

### JetStream Stream Configuration

```rust
use async_nats::jetstream;

async fn create_change_stream(js: &jetstream::Context) -> Result<()> {
    let stream = js.create_stream(jetstream::stream::Config {
        name: "RSFGA_CHANGES".to_string(),
        subjects: vec!["rsfga.changes.>".to_string()],

        // Retention: WorkQueue (auto-delete after ack)
        retention: jetstream::stream::RetentionPolicy::WorkQueue,

        // Replicas for HA
        num_replicas: 3,

        // Storage
        storage: jetstream::stream::StorageType::File,
        max_bytes: 100 * 1024 * 1024 * 1024, // 100GB

        // Deduplication window
        duplicate_window: Duration::from_secs(120),

        // Discard old messages when full
        discard: jetstream::stream::DiscardPolicy::Old,

        ..Default::default()
    }).await?;

    Ok(())
}
```

### Consumer Configuration (Edge Sync)

```rust
async fn create_edge_consumer(
    js: &jetstream::Context,
    edge_id: &str,
    store_filters: Vec<String>,
) -> Result<Consumer> {
    let filter_subject = if store_filters.is_empty() {
        "rsfga.changes.>".to_string()
    } else {
        format!("rsfga.changes.{{{}}}.*", store_filters.join(","))
    };

    let consumer = js.create_consumer_on_stream(
        jetstream::consumer::Config {
            name: Some(format!("edge-{}", edge_id)),
            durable_name: Some(format!("edge-{}", edge_id)),

            // Filter by store IDs
            filter_subject: filter_subject,

            // Deliver new messages only (not replay)
            deliver_policy: jetstream::consumer::DeliverPolicy::New,

            // Ack explicitly
            ack_policy: jetstream::consumer::AckPolicy::Explicit,
            ack_wait: Duration::from_secs(30),

            // Max delivery attempts
            max_deliver: 3,

            // Flow control
            max_ack_pending: 1000,
            max_waiting: 512,

            ..Default::default()
        },
        "RSFGA_CHANGES",
    ).await?;

    Ok(consumer)
}
```

---

## Edge Sync Agent Implementation

### NATS-Based Sync Agent

```rust
use async_nats::{Client, jetstream};
use futures::StreamExt;

pub struct NatsEdgeSyncAgent {
    nats_client: Client,
    js_context: jetstream::Context,
    local_store: Arc<dyn DataStore>,
    product_registry: Arc<ProductRegistry>,
    edge_id: String,
}

impl NatsEdgeSyncAgent {
    pub async fn new(
        nats_url: &str,
        edge_id: String,
        local_store: Arc<dyn DataStore>,
        product_registry: Arc<ProductRegistry>,
    ) -> Result<Self> {
        // Connect to regional NATS as leaf node
        let nats_client = async_nats::connect(nats_url).await?;
        let js_context = jetstream::new(nats_client.clone());

        Ok(Self {
            nats_client,
            js_context,
            local_store,
            product_registry,
            edge_id,
        })
    }

    pub async fn start(&self) -> Result<()> {
        // Get list of stores to sync
        let store_filters = self.product_registry
            .get_edge_stores(&self.edge_id)
            .await?;

        // Create or get consumer
        let consumer = create_edge_consumer(
            &self.js_context,
            &self.edge_id,
            store_filters,
        ).await?;

        // Start consuming messages
        let mut messages = consumer.messages().await?;

        while let Some(msg) = messages.next().await {
            let msg = msg?;

            // Process change
            match self.process_change(&msg).await {
                Ok(_) => {
                    // Ack message
                    msg.ack().await?;
                }
                Err(e) => {
                    error!("Failed to process change: {}", e);
                    // NAck with delay for retry
                    msg.ack_with(jetstream::AckKind::Nak(Some(
                        Duration::from_secs(5)
                    ))).await?;
                }
            }
        }

        Ok(())
    }

    async fn process_change(&self, msg: &jetstream::Message) -> Result<()> {
        // Deserialize change event
        let change: TupleChange = prost::Message::decode(msg.payload.as_ref())?;

        // Check if we should sync this change
        if !self.should_sync(&change).await? {
            return Ok(());
        }

        // Apply to local store
        self.local_store.write(
            &change.store_id,
            change.deletes,
            change.writes,
        ).await?;

        // Update sync metadata
        self.update_watermark(&change).await?;

        Ok(())
    }

    async fn should_sync(&self, change: &TupleChange) -> Result<bool> {
        let product = self.product_registry
            .get_product(&change.store_id)
            .await?;

        match product.tier {
            DataTier::Critical => Ok(true),
            DataTier::Hot => {
                // Check replication rules
                product.replication_rules
                    .object_types
                    .contains(&change.object_type)
            }
            DataTier::Warm | DataTier::Cold => Ok(false),
        }
    }
}
```

### Write Publisher (Central)

```rust
pub struct NatsChangePublisher {
    nats_client: Client,
    js_context: jetstream::Context,
}

impl NatsChangePublisher {
    pub async fn publish_change(&self, change: &TupleChange) -> Result<()> {
        let subject = format!(
            "rsfga.changes.{}.{}.{}",
            change.store_id,
            change.operation,
            change.object_type
        );

        // Serialize to protobuf
        let mut buf = Vec::new();
        change.encode(&mut buf)?;

        // Publish to JetStream
        let ack = self.js_context.publish(subject, buf.into()).await?;

        // Wait for ack from stream
        ack.await?;

        Ok(())
    }

    pub async fn publish_batch(&self, changes: Vec<TupleChange>) -> Result<()> {
        // Batch publish for better throughput
        let mut publish_futures = Vec::new();

        for change in changes {
            let subject = format!(
                "rsfga.changes.{}.{}.{}",
                change.store_id,
                change.operation,
                change.object_type
            );

            let mut buf = Vec::new();
            change.encode(&mut buf)?;

            let fut = self.js_context.publish(subject, buf.into());
            publish_futures.push(fut);
        }

        // Wait for all publishes
        let acks = futures::future::try_join_all(publish_futures).await?;

        // Wait for all stream acks
        for ack in acks {
            ack.await?;
        }

        Ok(())
    }
}
```

---

## Product-Based Partitioning with NATS

### Subject Filtering

Edge nodes subscribe to specific subjects based on product configuration:

```rust
pub struct ProductConfig {
    pub product_id: String,
    pub tier: DataTier,
    pub nats_subjects: Vec<String>,  // Specific subjects to subscribe
}

// Example configurations:
ProductConfig {
    product_id: "acme-corp",
    tier: DataTier::Hot,
    nats_subjects: vec![
        "rsfga.changes.store-acme-1.>",
        "rsfga.changes.store-acme-2.>",
    ],
}

ProductConfig {
    product_id: "global-app",
    tier: DataTier::Critical,
    nats_subjects: vec![
        "rsfga.changes.store-global.>",
    ],
}
```

### Dynamic Subject Subscription

```rust
impl NatsEdgeSyncAgent {
    pub async fn update_subscriptions(
        &self,
        new_products: Vec<ProductConfig>,
    ) -> Result<()> {
        // Collect all subjects to subscribe
        let subjects: Vec<String> = new_products
            .iter()
            .flat_map(|p| p.nats_subjects.clone())
            .collect();

        // Create new consumer with updated filter
        let filter_subject = subjects.join(",");

        let consumer = self.js_context.create_consumer_on_stream(
            jetstream::consumer::Config {
                name: Some(format!("edge-{}", self.edge_id)),
                durable_name: Some(format!("edge-{}", self.edge_id)),
                filter_subject: filter_subject,
                // ... other config
                ..Default::default()
            },
            "RSFGA_CHANGES",
        ).await?;

        // Restart consumption with new consumer
        // (implementation details)

        Ok(())
    }
}
```

---

## NATS Monitoring and Observability

### Metrics to Track

```rust
// NATS-specific metrics
metrics::gauge!("nats_connected", if connected { 1.0 } else { 0.0 });
metrics::counter!("nats_messages_received_total");
metrics::counter!("nats_messages_published_total");
metrics::histogram!("nats_message_latency_seconds");
metrics::gauge!("nats_pending_messages");
metrics::gauge!("nats_consumer_lag");

// JetStream metrics
metrics::gauge!("jetstream_stream_messages");
metrics::gauge!("jetstream_stream_bytes");
metrics::gauge!("jetstream_consumer_pending");
metrics::counter!("jetstream_consumer_ack_total");
metrics::counter!("jetstream_consumer_nak_total");
```

### Health Checks

```rust
impl NatsEdgeSyncAgent {
    pub async fn health_check(&self) -> Result<HealthStatus> {
        // Check NATS connection
        if !self.nats_client.connection_state().is_connected() {
            return Ok(HealthStatus::Unhealthy {
                reason: "NATS disconnected".to_string(),
            });
        }

        // Check consumer lag
        let consumer_info = self.consumer.info().await?;
        let lag = consumer_info.num_pending;

        if lag > 10000 {
            return Ok(HealthStatus::Degraded {
                reason: format!("High consumer lag: {}", lag),
            });
        }

        Ok(HealthStatus::Healthy)
    }
}
```

---

## Failure Scenarios and Recovery

### Edge Node Disconnection

**Scenario**: Edge node loses connection to regional NATS

**NATS Behavior**:
- Leaf node automatically attempts reconnection
- Backoff strategy: 5s, 10s, 20s, max 60s
- Infinite retry attempts

**Edge Node Behavior**:
```rust
// NATS client handles reconnection automatically
// Application code should handle:

async fn handle_disconnect(&self) {
    warn!("NATS connection lost, entering degraded mode");

    // Set health status to degraded
    self.health.set_degraded("NATS disconnected");

    // Continue serving from local cache
    // Forward writes to regional API (HTTP fallback)
}

async fn handle_reconnect(&self) {
    info!("NATS connection restored");

    // Consumer will automatically resume from last ack
    // No manual replay needed

    self.health.set_healthy();
}
```

### Message Replay After Downtime

**Scenario**: Edge node offline for 1 hour, comes back online

**Recovery**:
```rust
// JetStream consumer automatically delivers missed messages
// Starting from last acknowledged sequence

async fn sync_after_downtime(&self) -> Result<()> {
    let consumer_info = self.consumer.info().await?;
    let pending = consumer_info.num_pending;

    info!("Syncing {} pending messages", pending);

    // Increase parallelism for catch-up
    let mut messages = self.consumer.messages().await?;

    // Process with higher concurrency during catch-up
    let semaphore = Arc::new(Semaphore::new(10)); // vs normal 3

    while let Some(msg) = messages.next().await {
        let sem = semaphore.clone();
        tokio::spawn(async move {
            let _permit = sem.acquire().await;
            self.process_change(&msg).await
        });
    }

    Ok(())
}
```

---

## Performance Characteristics

### NATS Throughput

**Expected Performance**:
- **Publish Rate**: 500K+ msgs/sec per NATS server
- **Subscribe Rate**: 1M+ msgs/sec per NATS server
- **Latency**: <1ms p99 within same cluster
- **Leaf Node Latency**: +5-10ms (edge to regional)

### JetStream Overhead

**Storage**:
- ~100 bytes overhead per message (metadata)
- Dedupe window: 2 minutes (configurable)
- Replication factor: 3 (configurable)

**Performance Impact**:
- Publish: +1-2ms for JetStream ack
- Subscribe: Negligible (stream replay is cached)

---

## Migration Path (Kafka → NATS)

For organizations currently using Kafka:

### Dual-Write Phase

```rust
pub struct DualPublisher {
    nats: NatsChangePublisher,
    kafka: KafkaProducer,
}

impl DualPublisher {
    async fn publish(&self, change: &TupleChange) -> Result<()> {
        // Publish to both systems
        let (nats_result, kafka_result) = tokio::join!(
            self.nats.publish_change(change),
            self.kafka.send(change),
        );

        // Require both to succeed during migration
        nats_result?;
        kafka_result?;

        Ok(())
    }
}
```

### Gradual Consumer Migration

1. **Week 1**: Deploy NATS alongside Kafka, dual-write
2. **Week 2**: Migrate 10% of edge nodes to NATS consumers
3. **Week 3**: Migrate 50% of edge nodes
4. **Week 4**: Migrate 100% of edge nodes
5. **Week 5**: Stop dual-write, NATS only
6. **Week 6**: Decommission Kafka

---

## Summary

**NATS Advantages for RSFGA Edge Architecture**:

1. **Lightweight**: 10-20MB footprint vs 500MB+ for Kafka
2. **Native Edge Support**: Leaf nodes built for edge scenarios
3. **Low Latency**: Sub-millisecond within cluster
4. **Simple Operations**: Single binary, zero configuration
5. **Built-in HA**: JetStream replication and clustering
6. **Security**: Native TLS, accounts, and authorization

**Trade-offs**:
- Less mature ecosystem than Kafka
- Fewer third-party integrations
- JetStream is younger than Kafka (but stable)

**Recommendation**: NATS is an excellent choice for RSFGA's edge architecture, providing the perfect balance of performance, simplicity, and edge-native features.

---

## Next Steps

1. **Prototype NATS integration** (Milestone 3.1)
2. **Benchmark NATS vs Kafka** for RSFGA workload
3. **Deploy regional NATS clusters** (3 regions)
4. **Test edge leaf node connectivity**
5. **Implement sync agent** with JetStream consumers
6. **Monitor and tune** for production workload
