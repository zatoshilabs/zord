# Build stage
FROM rust:1.91-slim-bookworm AS builder

WORKDIR /build

# Copy manifests. Match both Cargo.toml and Cargo.lock without failing
# when the lockfile is absent (e.g. older commits).
COPY Cargo.* ./

# Copy source code
COPY src ./src

# Install build dependencies
RUN apt-get update && \
    apt-get install -y build-essential pkg-config libssl-dev && \
    rm -rf /var/lib/apt/lists/*

# Build release binary
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 curl && \
    rm -rf /var/lib/apt/lists/*

# Copy binary from builder
COPY --from=builder /build/target/release/zord /usr/local/bin/zord

# Copy web assets
COPY web ./web

# Create data directory
RUN mkdir -p /data

# Environment variables (can be overridden)
ENV RUST_LOG=info
ENV ZSTART_HEIGHT=3132356
ENV API_PORT=8080
ENV DB_PATH=/data/zord.db

# Expose API port
EXPOSE 8080

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=40s --retries=3 \
    CMD curl -f http://localhost:${API_PORT}/health || exit 1

# Run the indexer
CMD ["zord"]
