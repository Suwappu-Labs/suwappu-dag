#!/usr/bin/env bash
# Cross-compile gsx-node + gsx-loadgen + gsx-metrics to x86_64-unknown-linux-musl
# so the resulting binary runs unmodified on the Amazon Linux EC2 instances
# spun up by terraform/perf.
#
# Uses `cross` (https://github.com/cross-rs/cross) because pqcrypto needs a
# linker; the host (macOS) toolchain can't reach it without a docker image.
#
# Usage: scripts/perf/build.sh [--target TRIPLE]
set -euo pipefail

TARGET="${TARGET:-x86_64-unknown-linux-musl}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="$ROOT/target/perf"

if ! command -v cross >/dev/null 2>&1; then
  cat >&2 <<EOF
error: 'cross' not installed.

Install it:
    cargo install cross --git https://github.com/cross-rs/cross --rev main

Then re-run this script. 'cross' uses a docker image to invoke the linker for
the musl target, which is required because pqcrypto-mldsa and pqcrypto-mlkem
include C source that must be compiled against musl libc.
EOF
  exit 1
fi

echo "[build] target = $TARGET"
cd "$ROOT"
cross build \
  --release \
  --target "$TARGET" \
  --bin gsx-node \
  --bin gsx-loadgen \
  --bin gsx-metrics

mkdir -p "$OUT"
for bin in gsx-node gsx-loadgen gsx-metrics; do
  src="target/$TARGET/release/$bin"
  cp "$src" "$OUT/$bin"
  strip "$OUT/$bin" 2>/dev/null || true
done

echo "[build] artifacts in $OUT:"
ls -lh "$OUT"
