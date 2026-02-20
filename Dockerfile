# =============================================================================
# RSFGA Dockerfile - Multi-stage build for minimal production image
# =============================================================================
#
# Builds one of three binaries depending on the BINARY build arg:
#   rsfga        - Main API server (default)
#   rsfga-writer - Storage consumer daemon for NATS async writes
#   rsfga-edge   - Edge sync daemon for distributed authorization
#
# Build:
#   docker build -t rsfga:latest .
#   docker build --build-arg BINARY=rsfga-writer -t rsfga-writer:latest .
#   docker build --build-arg BINARY=rsfga-edge -t rsfga-edge:latest .
#
# Run:
#   docker run -p 8080:8080 rsfga:latest
#   docker run -e NATS_URL=nats://nats:4222 rsfga-writer:latest
#   docker run -e NATS_URL=nats://nats:4222 rsfga-edge:latest
#
# =============================================================================

# Binary to build (rsfga, rsfga-writer, rsfga-edge, or rsfga-precompute)
ARG BINARY=rsfga
# Optional cargo features (workspace-qualified, e.g. "rsfga-api/nats,rsfga-api/precompute")
ARG FEATURES=""

# -----------------------------------------------------------------------------
# Stage 1: Build environment
# -----------------------------------------------------------------------------
FROM rust:1.88-bookworm AS builder

ARG BINARY
ARG FEATURES

# Install build dependencies (including clang for RocksDB)
RUN apt-get update && apt-get install -y --no-install-recommends \
    protobuf-compiler \
    libprotobuf-dev \
    clang \
    libclang-dev \
    && rm -rf /var/lib/apt/lists/*

# Create app directory
WORKDIR /app

# Copy workspace configuration first (for dependency caching)
COPY Cargo.toml Cargo.lock ./
COPY crates/rsfga-api/Cargo.toml crates/rsfga-api/
COPY crates/rsfga-server/Cargo.toml crates/rsfga-server/
COPY crates/rsfga-domain/Cargo.toml crates/rsfga-domain/
COPY crates/rsfga-storage/Cargo.toml crates/rsfga-storage/
COPY crates/rsfga-nats/Cargo.toml crates/rsfga-nats/
COPY crates/rsfga-writer/Cargo.toml crates/rsfga-writer/
COPY crates/rsfga-edge/Cargo.toml crates/rsfga-edge/
COPY crates/rsfga-precompute/Cargo.toml crates/rsfga-precompute/
COPY crates/rsfga-valkey/Cargo.toml crates/rsfga-valkey/
COPY crates/compatibility-tests/Cargo.toml crates/compatibility-tests/

# Create dummy source files for dependency caching
RUN mkdir -p crates/rsfga-api/src \
    && mkdir -p crates/rsfga-server/src \
    && mkdir -p crates/rsfga-domain/src \
    && mkdir -p crates/rsfga-storage/src \
    && mkdir -p crates/rsfga-nats/src \
    && mkdir -p crates/rsfga-writer/src \
    && mkdir -p crates/rsfga-edge/src \
    && mkdir -p crates/rsfga-precompute/src \
    && mkdir -p crates/rsfga-valkey/src \
    && mkdir -p crates/compatibility-tests/src \
    && echo "fn main() {}" > crates/rsfga-api/src/main.rs \
    && echo "pub fn dummy() {}" > crates/rsfga-api/src/lib.rs \
    && echo "pub fn dummy() {}" > crates/rsfga-server/src/lib.rs \
    && echo "pub fn dummy() {}" > crates/rsfga-domain/src/lib.rs \
    && echo "pub fn dummy() {}" > crates/rsfga-storage/src/lib.rs \
    && echo "pub fn dummy() {}" > crates/rsfga-nats/src/lib.rs \
    && echo "fn main() {}" > crates/rsfga-writer/src/main.rs \
    && echo "fn main() {}" > crates/rsfga-edge/src/main.rs \
    && echo "fn main() {}" > crates/rsfga-precompute/src/main.rs \
    && echo "pub fn dummy() {}" > crates/rsfga-precompute/src/lib.rs \
    && mkdir -p crates/rsfga-precompute/benches \
    && echo "fn main() {}" > crates/rsfga-precompute/benches/precompute_components_bench.rs \
    && echo "pub fn dummy() {}" > crates/rsfga-valkey/src/lib.rs \
    && mkdir -p crates/rsfga-valkey/benches \
    && echo "fn main() {}" > crates/rsfga-valkey/benches/key_construction_bench.rs \
    && echo "pub fn dummy() {}" > crates/compatibility-tests/src/lib.rs

# Copy proto files (needed for build)
COPY crates/rsfga-api/proto crates/rsfga-api/proto
COPY crates/rsfga-api/build.rs crates/rsfga-api/build.rs

# Validate FEATURES against command injection (allow alphanumeric, comma, slash, hyphen, underscore)
RUN if [ -n "${FEATURES}" ]; then \
      echo "${FEATURES}" | grep -qE '^[a-zA-Z0-9_,/-]+$' || { echo "Invalid FEATURES: ${FEATURES}"; exit 1; }; \
    fi

# Build dependencies only (this layer will be cached)
RUN if [ -n "${FEATURES}" ]; then \
      cargo build --release --bin ${BINARY} --features "${FEATURES}" 2>/dev/null || true; \
    else \
      cargo build --release --bin ${BINARY} 2>/dev/null || true; \
    fi

# Copy actual source code
COPY crates/rsfga-api/src crates/rsfga-api/src
COPY crates/rsfga-api/benches crates/rsfga-api/benches
COPY crates/rsfga-server/src crates/rsfga-server/src
COPY crates/rsfga-server/benches crates/rsfga-server/benches
COPY crates/rsfga-domain/src crates/rsfga-domain/src
COPY crates/rsfga-domain/benches crates/rsfga-domain/benches
COPY crates/rsfga-storage/src crates/rsfga-storage/src
COPY crates/rsfga-storage/benches crates/rsfga-storage/benches
COPY crates/rsfga-nats/src crates/rsfga-nats/src
COPY crates/rsfga-writer/src crates/rsfga-writer/src
COPY crates/rsfga-edge/src crates/rsfga-edge/src
COPY crates/rsfga-precompute/src crates/rsfga-precompute/src
COPY crates/rsfga-precompute/benches crates/rsfga-precompute/benches
COPY crates/rsfga-valkey/src crates/rsfga-valkey/src
COPY crates/rsfga-valkey/benches crates/rsfga-valkey/benches
COPY crates/compatibility-tests/src crates/compatibility-tests/src

# Touch files to invalidate the cache for actual source
RUN touch crates/rsfga-api/src/main.rs \
    && touch crates/rsfga-api/src/lib.rs \
    && touch crates/rsfga-server/src/lib.rs \
    && touch crates/rsfga-domain/src/lib.rs \
    && touch crates/rsfga-storage/src/lib.rs \
    && touch crates/rsfga-nats/src/lib.rs \
    && touch crates/rsfga-writer/src/main.rs \
    && touch crates/rsfga-edge/src/main.rs \
    && touch crates/rsfga-precompute/src/main.rs \
    && touch crates/rsfga-precompute/src/lib.rs \
    && touch crates/rsfga-valkey/src/lib.rs

# Build the release binary
RUN if [ -n "${FEATURES}" ]; then \
      cargo build --release --bin ${BINARY} --features "${FEATURES}"; \
    else \
      cargo build --release --bin ${BINARY}; \
    fi

# Verify the binary was built
RUN ls -la target/release/${BINARY}

# -----------------------------------------------------------------------------
# Stage 2: Runtime environment
# -----------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

ARG BINARY

# Install runtime dependencies (including curl for health check)
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user for security
RUN groupadd --gid 1000 rsfga \
    && useradd --uid 1000 --gid rsfga --shell /bin/bash --create-home rsfga

# Create app and data directories
WORKDIR /app
RUN mkdir -p /data

# Copy binary from builder (use consistent name for entrypoint)
COPY --from=builder /app/target/release/${BINARY} /app/entrypoint

# Set ownership
RUN chown -R rsfga:rsfga /app /data

# Switch to non-root user
USER rsfga

# Expose common ports (API: 8080/50051, metrics: 9090/9091)
EXPOSE 8080
EXPOSE 9090
EXPOSE 9091
EXPOSE 50051

# Volume for persistent storage (RocksDB)
VOLUME /data

# Store binary name for reference
ENV RSFGA_BINARY=${BINARY}

# Default environment variables for rsfga API server
ENV RSFGA_SERVER__HOST=0.0.0.0
ENV RSFGA_SERVER__PORT=8080
ENV RSFGA_GRPC__PORT=50051
ENV RSFGA_STORAGE__BACKEND=memory
ENV RSFGA_STORAGE__DATA_PATH=/data
ENV RSFGA_LOGGING__LEVEL=info
ENV RSFGA_LOGGING__JSON=true

# Run the binary
ENTRYPOINT ["/app/entrypoint"]
CMD []
