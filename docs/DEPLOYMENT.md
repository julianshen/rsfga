# Deployment Guide

This guide covers deploying RSFGA in various environments.

## Table of Contents

- [Prerequisites](#prerequisites)
- [Docker](#docker)
- [Kubernetes](#kubernetes)
- [Helm](#helm)
- [Configuration](#configuration)
- [Database Setup](#database-setup)
- [Health Checks](#health-checks)
- [Monitoring](#monitoring)

## Prerequisites

- Docker 20.10+ (for container deployment)
- Kubernetes 1.24+ (for K8s deployment)
- Helm 3.0+ (for Helm deployment)
- PostgreSQL 14+ (for persistent storage)

## Docker

### Building the Image

```bash
# Build the Docker image
docker build -t rsfga:v0.1.0 .

# Verify the image
docker images rsfga
```

### Running with In-Memory Storage

For development and testing:

```bash
docker run -d \
  --name rsfga \
  -p 8080:8080 \
  -e RSFGA_STORAGE__BACKEND=memory \
  -e RSFGA_LOGGING__LEVEL=debug \
  rsfga:v0.1.0
```

### Running with PostgreSQL

For production:

```bash
# Start PostgreSQL (if not already running)
docker run -d \
  --name postgres \
  -e POSTGRES_USER=rsfga \
  -e POSTGRES_PASSWORD=changeme \
  -e POSTGRES_DB=rsfga \
  -p 5432:5432 \
  postgres:14

# Start RSFGA
docker run -d \
  --name rsfga \
  -p 8080:8080 \
  --link postgres:postgres \
  -e RSFGA_STORAGE__BACKEND=postgres \
  -e RSFGA_STORAGE__DATABASE_URL="postgres://rsfga:changeme@postgres:5432/rsfga" \
  -e RSFGA_LOGGING__JSON=true \
  rsfga:v0.1.0
```

### Using a Config File

```bash
# Create config file
cat > config.yaml << EOF
server:
  host: "0.0.0.0"
  port: 8080

storage:
  backend: postgres
  pool_size: 10

logging:
  level: info
  json: true

metrics:
  enabled: true
EOF

# Run with config file
docker run -d \
  --name rsfga \
  -p 8080:8080 \
  -v $(pwd)/config.yaml:/app/config.yaml \
  -e RSFGA_STORAGE__DATABASE_URL="postgres://..." \
  rsfga:v0.1.0 --config /app/config.yaml
```

### Docker Compose

Create a `docker-compose.yaml`:

```yaml
version: '3.8'

services:
  rsfga:
    build: .
    ports:
      - "8080:8080"
    environment:
      - RSFGA_STORAGE__BACKEND=postgres
      - RSFGA_STORAGE__DATABASE_URL=postgres://rsfga:changeme@postgres:5432/rsfga
      - RSFGA_LOGGING__JSON=true
    depends_on:
      postgres:
        condition: service_healthy

  postgres:
    image: postgres:14
    environment:
      - POSTGRES_USER=rsfga
      - POSTGRES_PASSWORD=changeme
      - POSTGRES_DB=rsfga
    volumes:
      - postgres_data:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U rsfga"]
      interval: 5s
      timeout: 5s
      retries: 5

volumes:
  postgres_data:
```

Run with:

```bash
docker-compose up -d
```

## Kubernetes

### Quick Start with Kustomize

```bash
# Apply all manifests
kubectl apply -k deploy/kubernetes/

# Check status
kubectl get pods -n rsfga
kubectl get svc -n rsfga
```

### Manual Deployment

```bash
# Create namespace
kubectl apply -f deploy/kubernetes/namespace.yaml

# Create ConfigMap
kubectl apply -f deploy/kubernetes/configmap.yaml

# Create secret (edit with your actual credentials first!)
kubectl create secret generic rsfga-secrets \
  --namespace rsfga \
  --from-literal=database-url='postgres://user:password@host:5432/rsfga'

# Deploy
kubectl apply -f deploy/kubernetes/deployment.yaml
kubectl apply -f deploy/kubernetes/service.yaml

# Verify
kubectl get pods -n rsfga -w
```

### Accessing the Service

```bash
# Port forward for local testing
kubectl port-forward -n rsfga svc/rsfga 8080:8080

# Test
curl http://localhost:8080/health
```

## Helm

### Installation

```bash
# Add Helm repository (if published)
# helm repo add rsfga https://your-org.github.io/rsfga
# helm repo update

# Or install from local chart
helm install rsfga deploy/helm/rsfga \
  --namespace rsfga \
  --create-namespace \
  --set database.url="postgres://user:password@host:5432/rsfga"
```

### Using an Existing Secret

```bash
# First, create the secret
kubectl create secret generic my-db-secret \
  --namespace rsfga \
  --from-literal=db-url='postgres://...'

# Install with existing secret
helm install rsfga deploy/helm/rsfga \
  --namespace rsfga \
  --create-namespace \
  --set database.existingSecret=my-db-secret \
  --set database.existingSecretKey=db-url
```

### Custom Values

Create a `values-prod.yaml`:

```yaml
replicaCount: 5

resources:
  requests:
    cpu: 500m
    memory: 256Mi
  limits:
    cpu: 2000m
    memory: 1Gi

autoscaling:
  enabled: true
  minReplicas: 3
  maxReplicas: 20
  targetCPUUtilizationPercentage: 70

config:
  storage:
    poolSize: 20
  logging:
    level: info
    json: true
  tracing:
    enabled: true
    jaegerEndpoint: "jaeger-collector:6831"

ingress:
  enabled: true
  className: nginx
  hosts:
    - host: rsfga.example.com
      paths:
        - path: /
          pathType: Prefix
  tls:
    - secretName: rsfga-tls
      hosts:
        - rsfga.example.com
```

Install with custom values:

```bash
helm install rsfga deploy/helm/rsfga \
  --namespace rsfga \
  --create-namespace \
  -f values-prod.yaml
```

### Upgrading

```bash
helm upgrade rsfga deploy/helm/rsfga \
  --namespace rsfga \
  -f values-prod.yaml
```

### Uninstalling

```bash
helm uninstall rsfga --namespace rsfga
```

## Configuration

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `RSFGA_SERVER__HOST` | Host to bind | `0.0.0.0` |
| `RSFGA_SERVER__PORT` | Port to listen | `8080` |
| `RSFGA_STORAGE__BACKEND` | Storage backend (`memory` or `postgres`) | `memory` |
| `RSFGA_STORAGE__DATABASE_URL` | PostgreSQL connection URL | - |
| `RSFGA_STORAGE__POOL_SIZE` | Connection pool size | `10` |
| `RSFGA_LOGGING__LEVEL` | Log level | `info` |
| `RSFGA_LOGGING__JSON` | JSON log format | `false` |
| `RSFGA_METRICS__ENABLED` | Enable metrics endpoint | `true` |
| `RSFGA_TRACING__ENABLED` | Enable distributed tracing | `false` |
| `RSFGA_TRACING__JAEGER_ENDPOINT` | Jaeger collector endpoint | `localhost:6831` |

### Config File

```yaml
server:
  host: "0.0.0.0"
  port: 8080
  request_timeout_secs: 30
  max_connections: 10000

storage:
  backend: postgres
  database_url: "postgres://user:pass@localhost:5432/rsfga"
  pool_size: 10
  connection_timeout_secs: 5

logging:
  level: info
  json: true

metrics:
  enabled: true
  path: /metrics

tracing:
  enabled: false
  jaeger_endpoint: "localhost:6831"
  service_name: rsfga
```

## Database Setup

### PostgreSQL Requirements

- PostgreSQL 14 or later
- Database created with UTF-8 encoding
- User with CREATE TABLE permissions

### Automatic Migrations

RSFGA automatically runs database migrations on startup. The following tables are created:

- `stores` - Authorization stores
- `tuples` - Relationship tuples

### Manual Migration

If you need to run migrations manually:

```sql
-- Create stores table
CREATE TABLE IF NOT EXISTS stores (
    id VARCHAR(255) PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);

-- Create tuples table
CREATE TABLE IF NOT EXISTS tuples (
    id BIGSERIAL PRIMARY KEY,
    store_id VARCHAR(255) NOT NULL,
    object_type VARCHAR(255) NOT NULL,
    object_id VARCHAR(255) NOT NULL,
    relation VARCHAR(255) NOT NULL,
    user_type VARCHAR(255) NOT NULL,
    user_id VARCHAR(255) NOT NULL,
    user_relation VARCHAR(255),
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    FOREIGN KEY (store_id) REFERENCES stores(id) ON DELETE CASCADE
);

-- Create unique index
CREATE UNIQUE INDEX IF NOT EXISTS idx_tuples_unique
ON tuples (store_id, object_type, object_id, relation, user_type, user_id, COALESCE(user_relation, ''));
```

## Health Checks

### Liveness Probe

```bash
curl http://localhost:8080/health
# Returns: {"status":"ok"}
```

### Readiness Probe

```bash
curl http://localhost:8080/ready
# Returns: {"status":"ready","checks":{"storage":"ok"}}
```

## Monitoring

### Prometheus Metrics

Metrics are exposed at `/metrics` when enabled:

```bash
curl http://localhost:8080/metrics
```

Key metrics:

- `http_requests_total` - Total HTTP requests
- `http_request_duration_seconds` - Request latency histogram
- `check_requests_total` - Permission check requests
- `batch_check_requests_total` - Batch check requests

### Jaeger Tracing

Enable tracing in configuration:

```yaml
tracing:
  enabled: true
  jaeger_endpoint: "jaeger-collector:6831"
  service_name: rsfga
```

Or via environment:

```bash
export RSFGA_TRACING__ENABLED=true
export RSFGA_TRACING__JAEGER_ENDPOINT="jaeger-collector:6831"
```

## Production Recommendations

1. **Use PostgreSQL** for persistent storage
2. **Enable JSON logging** for log aggregation
3. **Run multiple replicas** (3+) for high availability
4. **Set resource limits** to prevent resource exhaustion
5. **Enable metrics** for monitoring
6. **Use secrets** for database credentials (never commit them)
7. **Configure pod anti-affinity** for fault tolerance
8. **Set up health checks** for automatic recovery

## Troubleshooting

### Connection Refused

```bash
# Check if the service is running
kubectl get pods -n rsfga
kubectl logs -n rsfga deployment/rsfga

# Verify service endpoint
kubectl get endpoints -n rsfga
```

### Database Connection Failed

```bash
# Test database connectivity
kubectl run -it --rm debug \
  --image=postgres:14 \
  --restart=Never \
  -- psql postgres://user:pass@host:5432/rsfga -c "SELECT 1"
```

### High Latency

1. Check connection pool size (increase if needed)
2. Check database query performance
3. Enable tracing to identify bottlenecks
4. Review resource utilization
