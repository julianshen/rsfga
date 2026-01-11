# =============================================================================
# RSFGA Dockerfile - Multi-stage build for minimal production image
# =============================================================================
#
# Build: docker build -t rsfga:v0.1.0 .
# Run:   docker run -p 8080:8080 -v ./config.yaml:/app/config.yaml rsfga:v0.1.0
#
# Environment variables:
#   RSFGA_SERVER__HOST       - Host to bind (default: 0.0.0.0)
#   RSFGA_SERVER__PORT       - Port to listen (default: 8080)
#   RSFGA_STORAGE__BACKEND   - Storage backend: "memory" or "postgres"
#   RSFGA_STORAGE__DATABASE_URL - PostgreSQL connection URL (required if postgres)
#   RSFGA_LOGGING__LEVEL     - Log level: trace, debug, info, warn, error
#   RSFGA_LOGGING__JSON      - Use JSON logging format (true/false)
#
# =============================================================================

# -----------------------------------------------------------------------------
# Stage 1: Build environment
# -----------------------------------------------------------------------------
FROM rust:1.92-bookworm AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    protobuf-compiler \
    libprotobuf-dev \
    && rm -rf /var/lib/apt/lists/*

# Create app directory
WORKDIR /app

# Copy workspace configuration first (for dependency caching)
COPY Cargo.toml Cargo.lock ./
COPY crates/rsfga-api/Cargo.toml crates/rsfga-api/
COPY crates/rsfga-server/Cargo.toml crates/rsfga-server/
COPY crates/rsfga-domain/Cargo.toml crates/rsfga-domain/
COPY crates/rsfga-storage/Cargo.toml crates/rsfga-storage/
COPY crates/compatibility-tests/Cargo.toml crates/compatibility-tests/

# Create dummy source files for dependency caching
RUN mkdir -p crates/rsfga-api/src \
    && mkdir -p crates/rsfga-server/src \
    && mkdir -p crates/rsfga-domain/src \
    && mkdir -p crates/rsfga-storage/src \
    && mkdir -p crates/compatibility-tests/src \
    && echo "fn main() {}" > crates/rsfga-api/src/main.rs \
    && echo "pub fn dummy() {}" > crates/rsfga-api/src/lib.rs \
    && echo "pub fn dummy() {}" > crates/rsfga-server/src/lib.rs \
    && echo "pub fn dummy() {}" > crates/rsfga-domain/src/lib.rs \
    && echo "pub fn dummy() {}" > crates/rsfga-storage/src/lib.rs \
    && echo "pub fn dummy() {}" > crates/compatibility-tests/src/lib.rs

# Copy proto files (needed for build)
COPY crates/rsfga-api/proto crates/rsfga-api/proto
COPY crates/rsfga-api/build.rs crates/rsfga-api/build.rs

# Build dependencies only (this layer will be cached)
RUN cargo build --release --bin rsfga 2>/dev/null || true

# Copy actual source code
COPY crates/rsfga-api/src crates/rsfga-api/src
COPY crates/rsfga-server/src crates/rsfga-server/src
COPY crates/rsfga-domain/src crates/rsfga-domain/src
COPY crates/rsfga-domain/benches crates/rsfga-domain/benches
COPY crates/rsfga-storage/src crates/rsfga-storage/src
COPY crates/compatibility-tests/src crates/compatibility-tests/src

# Touch files to invalidate the cache for actual source
RUN touch crates/rsfga-api/src/main.rs \
    && touch crates/rsfga-api/src/lib.rs \
    && touch crates/rsfga-server/src/lib.rs \
    && touch crates/rsfga-domain/src/lib.rs \
    && touch crates/rsfga-storage/src/lib.rs

# Build the release binary
RUN cargo build --release --bin rsfga

# Verify the binary was built
RUN ls -la target/release/rsfga

# -----------------------------------------------------------------------------
# Stage 2: Runtime environment
# -----------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# Install runtime dependencies (including curl for health check)
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user for security
RUN groupadd --gid 1000 rsfga \
    && useradd --uid 1000 --gid rsfga --shell /bin/bash --create-home rsfga

# Create app directory
WORKDIR /app

# Copy binary from builder
COPY --from=builder /app/target/release/rsfga /app/rsfga

# Set ownership
RUN chown -R rsfga:rsfga /app

# Switch to non-root user
USER rsfga

# Expose HTTP port
EXPOSE 8080

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/health || exit 1

# Default environment variables
ENV RSFGA_SERVER__HOST=0.0.0.0
ENV RSFGA_SERVER__PORT=8080
ENV RSFGA_STORAGE__BACKEND=memory
ENV RSFGA_LOGGING__LEVEL=info
ENV RSFGA_LOGGING__JSON=true

# Run the binary
ENTRYPOINT ["/app/rsfga"]
CMD []
