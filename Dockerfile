# Multi-stage Dockerfile for the suwappu-node binary set.
#
# Stage 1 (builder) compiles release binaries with cargo.
# Stage 2 (runtime) is a slim Debian image with just the binaries.
#
# Built by `scripts/devnet-local.sh` and consumed by
# `docker-compose.yml` for the 4-node local devnet, and published to
# GHCR by `.github/workflows/docker.yml`.
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
#
# Note on cargo-chef: an earlier version of this file used cargo-chef to
# cache dependency layers. It was removed because `cargo chef cook`
# reconstructs a *skeleton* of the workspace from recipe.json and does
# not recreate the Cargo.toml of crates that are EXCLUDED from the
# workspace but pulled in as path deps (here: `zkvm/reserve-coverage-*`,
# excluded via `exclude = ["zkvm"]` in the root Cargo.toml). cook then
# panicked with "failed to read /work/zkvm/reserve-coverage-host/
# Cargo.toml: No such file or directory". A plain `COPY . . && cargo
# build` has the real files on disk and sidesteps the skeleton problem
# entirely, at the cost of recompiling deps on every image build (the
# image is built rarely — main pushes + release tags — so this is fine).

# -------- builder --------
# Base image tracks a recent stable Rust. The workspace itself is
# edition-2021 / MSRV 1.78, but parts of the dependency graph have moved
# to edition2024, which needs Cargo >= 1.85; pinning 1.78 broke the
# build with "feature `edition2024` is required". CI builds the workspace
# on stable, so this matches CI's real toolchain.
FROM rust:1.98-bookworm AS builder
WORKDIR /work
# The private suwappu-db git dep is fetched over SSH: rewrite the
# https URL cargo sees to git@github.com, use the system git client
# (so it talks to the forwarded ssh-agent), and trust github.com's
# host key before any fetch.
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true
RUN git config --global url."git@github.com:".insteadOf "https://github.com/"
RUN --mount=type=ssh,required=false mkdir -p ~/.ssh && ssh-keyscan github.com >> ~/.ssh/known_hosts
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
