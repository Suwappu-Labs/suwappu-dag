# Multi-stage Dockerfile for the gsx-node binary set.
#
# Stage 1 (chef-plan) caches dependency resolution.
# Stage 2 (builder) compiles release binaries with cargo.
# Stage 3 (runtime) is a slim Debian image with just the binaries.
#
# Built by `scripts/devnet-local.sh` and consumed by
# `docker-compose.yml` for the 4-node local devnet.

# -------- chef-plan --------
FROM rust:1.78-bookworm AS chef-plan
WORKDIR /work
# cargo-chef caches dependency builds when only application code changes.
RUN cargo install --locked cargo-chef
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# -------- builder --------
FROM rust:1.78-bookworm AS builder
WORKDIR /work
RUN cargo install --locked cargo-chef
COPY --from=chef-plan /work/recipe.json recipe.json
# Build deps once, cache across application rebuilds.
RUN cargo chef cook --release --recipe-path recipe.json --bin gsx-node
COPY . .
# Build the binaries the devnet needs.
RUN cargo build --release \
        -p gsx-node --bin gsx-node \
        -p gsx-node --bin gsx-loadgen \
        -p gsx-indexer --bin gsx-indexer

# -------- runtime --------
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /work/target/release/gsx-node     /usr/local/bin/gsx-node
COPY --from=builder /work/target/release/gsx-loadgen  /usr/local/bin/gsx-loadgen
COPY --from=builder /work/target/release/gsx-indexer  /usr/local/bin/gsx-indexer
# Default entrypoint runs the validator. docker-compose overrides
# the command for loadgen + indexer services.
ENTRYPOINT ["/usr/local/bin/gsx-node"]
