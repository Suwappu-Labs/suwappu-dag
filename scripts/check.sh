#!/usr/bin/env bash
# Local /check — runs the same gates as CI.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

echo "==> cargo fmt --check"
cargo fmt --all -- --check

echo "==> cargo clippy"
cargo clippy --workspace --all-targets -- -D warnings

echo "==> cargo test --workspace"
cargo test --workspace --all-targets

echo "==> crypto lane separation"
./scripts/check-crypto-boundary.sh

echo "==> cargo deny check"
if command -v cargo-deny >/dev/null 2>&1; then
    cargo deny check
else
    echo "cargo-deny not installed (cargo install --locked cargo-deny) — CI runs it regardless" >&2
    exit 1
fi

echo "==> all gates green"
