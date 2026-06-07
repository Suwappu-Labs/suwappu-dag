# Multi-stage Dockerfile for the suwappu-node binary set.
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
RUN cargo chef cook --release --recipe-path recipe.json --bin suwappu-node
COPY . .
# Build the binaries the devnet needs.
RUN cargo build --release \
        -p suwappu-node --bin suwappu-node \
        -p suwappu-node --bin suwappu-loadgen \
        -p suwappu-indexer --bin suwappu-indexer

# -------- runtime --------
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /work/target/release/suwappu-node     /usr/local/bin/suwappu-node
COPY --from=builder /work/target/release/suwappu-loadgen  /usr/local/bin/suwappu-loadgen
COPY --from=builder /work/target/release/suwappu-indexer  /usr/local/bin/suwappu-indexer
# Default entrypoint runs the validator. docker-compose overrides
# the command for loadgen + indexer services.
ENTRYPOINT ["/usr/local/bin/suwappu-node"]
