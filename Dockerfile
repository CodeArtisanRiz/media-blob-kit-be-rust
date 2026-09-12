# -------------------------------------------------------------
# 1. Chef Planner: Extracts dependency recipe from Cargo files
# -------------------------------------------------------------
FROM lukemathwalker/cargo-chef:latest-rust-1-bookworm AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# -------------------------------------------------------------
# 2. Builder: Pre-compiles and CACHES all crates with lld linker
# -------------------------------------------------------------
FROM chef AS builder
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    clang \
    lld \
    && rm -rf /var/lib/apt/lists/*

COPY --from=planner /app/recipe.json recipe.json

# Build & cache dependencies using persistent BuildKit mounts to survive Docker image pruning
ENV RUSTFLAGS="-C link-arg=-fuse-ld=lld"
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo chef cook --release --recipe-path recipe.json

# Build actual application binary
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo build --release --bin media-blob-kit && \
    # Move the binary out of the cached target folder so we can copy it in the next stage
    cp /app/target/release/media-blob-kit /app/media-blob-kit

# -------------------------------------------------------------
# 3. Minimal Production Runtime (~95MB Debian Slim)
# -------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    dumb-init \
    curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy compiled binary from builder
COPY --from=builder /app/media-blob-kit .

# Environment defaults
ENV RUST_LOG=info
ENV APP_HOST=0.0.0.0
ENV APP_PORT=3000

EXPOSE 3000

ENTRYPOINT ["/usr/bin/dumb-init", "--"]
CMD ["./media-blob-kit"]
