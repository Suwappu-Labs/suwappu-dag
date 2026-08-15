# Multi-stage Dockerfile for the suwappu-node binary set.
#
# Stage 1 (chef-plan) caches dependency resolution.
# Stage 2 (builder) compiles release binaries with cargo.
# Stage 3 (runtime) is a slim Debian image with just the binaries.
#
# Built by `scripts/devnet-local.sh` and consumed by
# `docker-compose.yml` for the 4-node local devnet.
#
# The cargo build fetches the PRIVATE suwappu-db git dependency over
# SSH, so BuildKit must forward an ssh-agent holding a key with read
# access to Suwappu-Labs/suwappu-db:
#   - Local (org members, ssh-agent running):
#       DOCKER_BUILDKIT=1 docker compose build --ssh default
#     or: DOCKER_BUILDKIT=1 docker build --ssh default .
#   - CI (.github/workflows/docker.yml): webfactory/ssh-agent loads
#     the SUWAPPU_DB_DEPLOY_KEY secret, then build-push-action passes
#     `ssh: default`.

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
# The private suwappu-db git dep is fetched over SSH: rewrite the
# https URL cargo sees to git@github.com, use the system git client
# (so it talks to the forwarded ssh-agent), and trust github.com's
# host key before any fetch.
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true
RUN git config --global url."git@github.com:".insteadOf "https://github.com/"
RUN --mount=type=ssh,required=false mkdir -p ~/.ssh && ssh-keyscan github.com >> ~/.ssh/known_hosts
RUN cargo install --locked cargo-chef
COPY --from=chef-plan /work/recipe.json recipe.json
# Build deps once, cache across application rebuilds.
RUN --mount=type=ssh cargo chef cook --release --recipe-path recipe.json --bin suwappu-node
COPY . .
# Build the binaries the devnet needs.
RUN --mount=type=ssh cargo build --release \
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
