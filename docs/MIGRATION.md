# Migration Guide: OpenFGA to RSFGA

This guide explains how to migrate from OpenFGA to RSFGA. Since RSFGA is 100% API compatible with OpenFGA, migration is straightforward.

## Overview

RSFGA is a drop-in replacement for OpenFGA. The migration process involves:

1. Deploying RSFGA alongside OpenFGA (optional, for testing)
2. Updating your application's endpoint configuration
3. Verifying functionality
4. Switching traffic to RSFGA

## Prerequisites

- RSFGA server built and ready to deploy
- Access to your existing OpenFGA configuration
- Ability to update application configuration

## Migration Steps

### Step 1: Prepare RSFGA Configuration

Create a configuration file that matches your OpenFGA setup:

```yaml
# config.yaml
server:
  host: "0.0.0.0"
  port: 8080
  request_timeout_secs: 30

storage:
  backend: postgres  # or "memory" for testing
  database_url: "postgres://user:password@localhost:5432/rsfga"
  pool_size: 10

logging:
  level: info
  json: true

metrics:
  enabled: true
  path: /metrics

tracing:
  enabled: false  # default is false; set to true to enable Jaeger tracing
  jaeger_endpoint: "localhost:6831"
  service_name: rsfga
```

### Step 2: Database Migration (PostgreSQL)

If you're using PostgreSQL, RSFGA uses a compatible schema. You have two options:

#### Option A: Fresh Database (Recommended for testing)

Start with a fresh database and recreate your authorization models and tuples:

```bash
# Create new database
createdb rsfga

# Start RSFGA
cargo run --release -- --config config.yaml
```

#### Option B: Shared Database

RSFGA can work with an existing OpenFGA database. The schema is compatible:

```bash
# Point RSFGA to existing OpenFGA database
export RSFGA_STORAGE__DATABASE_URL="postgres://user:password@localhost:5432/openfga"

cargo run --release -- --config config.yaml
```

**Note**: When sharing a database, ensure only one server (OpenFGA or RSFGA) writes at a time to avoid conflicts.

### Step 3: Deploy RSFGA

> **Note**: Docker and Kubernetes deployment is being developed in Milestone 1.9, Section 4. The examples below show the expected deployment pattern once complete.

#### Docker Deployment

```bash
# Build Docker image (requires Dockerfile - coming in M1.9 Section 4)
docker build -t rsfga:v1.9.0 .

# Run container
docker run -d \
  --name rsfga \
  -p 8080:8080 \
  -v $(pwd)/config.yaml:/app/config.yaml \
  -e RSFGA_STORAGE__DATABASE_URL="postgres://..." \
  rsfga:v1.9.0
```

#### Kubernetes Deployment

```bash
# First, create the ConfigMap from your config file
kubectl create configmap rsfga-config --from-file=config.yaml

# Create the secret for database credentials
kubectl create secret generic rsfga-secrets \
  --from-literal=database-url='postgres://user:password@host:5432/rsfga'
```

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: rsfga
spec:
  replicas: 3
  selector:
    matchLabels:
      app: rsfga
  template:
    metadata:
      labels:
        app: rsfga
    spec:
      containers:
      - name: rsfga
        image: rsfga:v1.9.0  # Use specific version, not :latest
        ports:
        - containerPort: 8080
        env:
        - name: RSFGA_STORAGE__DATABASE_URL
          valueFrom:
            secretKeyRef:
              name: rsfga-secrets
              key: database-url
        volumeMounts:
        - name: config
          mountPath: /app/config.yaml
          subPath: config.yaml
      volumes:
      - name: config
        configMap:
          name: rsfga-config
```

### Step 4: Update Application Configuration

Update your application to point to RSFGA:

#### SDK Configuration

**Go SDK:**
```go
// Before (OpenFGA)
client, _ := openfga.NewClient(openfga.ClientConfig{
    ApiUrl: "http://openfga:8080",
})

// After (RSFGA) - just change the URL
client, _ := openfga.NewClient(openfga.ClientConfig{
    ApiUrl: "http://rsfga:8080",  // Updated endpoint
})
```

**JavaScript SDK:**
```javascript
// Before (OpenFGA)
const client = new OpenFGAClient({
    apiUrl: 'http://openfga:8080',
});

// After (RSFGA) - just change the URL
const client = new OpenFGAClient({
    apiUrl: 'http://rsfga:8080',  // Updated endpoint
});
```

**Python SDK:**
```python
# Before (OpenFGA)
client = OpenFGAClient(
    api_url="http://openfga:8080"
)

# After (RSFGA) - just change the URL
client = OpenFGAClient(
    api_url="http://rsfga:8080"  # Updated endpoint
)
```

#### Direct API Calls

If you're making direct HTTP calls, update the base URL:

```bash
# Before
curl http://openfga:8080/stores

# After
curl http://rsfga:8080/stores
```

### Step 5: Verify Functionality

Run your test suite against RSFGA to verify compatibility:

```bash
# Example: Run a simple check
curl -X POST http://rsfga:8080/stores/${STORE_ID}/check \
  -H "Content-Type: application/json" \
  -d '{
    "tuple_key": {
      "user": "user:alice",
      "relation": "viewer",
      "object": "document:readme"
    }
  }'
```

Verify that:
- All API endpoints return expected responses
- Authorization decisions match OpenFGA behavior
- Performance meets expectations

### Step 6: Gradual Traffic Migration (Optional)

For production environments, consider gradual migration:

1. **Shadow Mode**: Run RSFGA in parallel, comparing responses
2. **Canary Release**: Route a small percentage of traffic to RSFGA
3. **Full Migration**: Switch all traffic after verification

## API Compatibility

RSFGA implements 100% of the OpenFGA API:

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/stores` | POST | Create store |
| `/stores` | GET | List stores |
| `/stores/{id}` | GET | Get store |
| `/stores/{id}` | DELETE | Delete store |
| `/stores/{id}/authorization-models` | POST | Create model |
| `/stores/{id}/authorization-models` | GET | List models |
| `/stores/{id}/authorization-models/{model_id}` | GET | Get model |
| `/stores/{id}/write` | POST | Write tuples |
| `/stores/{id}/read` | POST | Read tuples |
| `/stores/{id}/check` | POST | Check permission |
| `/stores/{id}/expand` | POST | Expand relation |
| `/stores/{id}/list-objects` | POST | List objects |

All request/response formats are identical to OpenFGA.

## Differences from OpenFGA

While RSFGA is API compatible, there are some implementation differences:

| Aspect | OpenFGA | RSFGA |
|--------|---------|-------|
| Language | Go | Rust |
| Graph Traversal | Sequential | Parallel (async) |
| Caching | sync.Map | DashMap (lock-free) |
| Performance | Baseline | 2x+ throughput |

These differences are internal and don't affect API behavior.

## Rollback Plan

If you need to rollback to OpenFGA:

1. Update application configuration to point back to OpenFGA
2. Stop RSFGA instances
3. Verify OpenFGA is handling traffic correctly

Since both use the same API, rollback is as simple as updating the endpoint.

## Troubleshooting

### Connection Refused

```text
Error: Connection refused
```

**Solution**: Verify RSFGA is running and accessible:
```bash
curl http://rsfga:8080/health
```

### Database Connection Failed

```text
Error: Failed to connect to database
```

**Solution**: Check database URL and credentials:

```bash
# Verify connection string
RSFGA_STORAGE__DATABASE_URL="postgres://user:password@host:5432/db"
```

### Model Not Found

```text
Error: Authorization model not found
```

**Solution**: If using a fresh database, recreate your authorization models:
```bash
curl -X POST "http://rsfga:8080/stores/${STORE_ID}/authorization-models" \
  -H "Content-Type: application/json" \
  -d @model.json
```

### Performance Issues

If you experience performance issues:

1. Check connection pool settings:
   ```yaml
   storage:
     pool_size: 20  # Increase if needed
   ```

2. Enable metrics to identify bottlenecks:
   ```yaml
   metrics:
     enabled: true
   ```

3. Review logs for slow queries:
   ```yaml
   logging:
     level: debug
   ```

## Getting Help

- **Issues**: [GitHub Issues](https://github.com/julianshen/rsfga/issues)
- **Documentation**: [docs/design/](docs/design/)
- **API Reference**: [API Specification](docs/design/API_SPECIFICATION.md)

## Next Steps

After successful migration:

1. Remove OpenFGA deployment (if no longer needed)
2. Update monitoring dashboards for RSFGA metrics
3. Configure alerting based on RSFGA metrics
4. Document the migration for your team
