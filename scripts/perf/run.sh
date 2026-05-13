#!/usr/bin/env bash
# Run the load generator against one validator (default: us-east-1) for the
# configured duration. Assumes `provision.sh` has finished and all 7 nodes
# are healthy.
#
# Usage: scripts/perf/run.sh [--target-region us-east-1] [--rate 100] [--duration 60]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TF="$ROOT/terraform/perf"

TARGET_REGION="${TARGET_REGION:-us-east-1}"
RATE="${RATE:-100}"
DURATION="${DURATION:-60}"
CLIENT_PORT="${CLIENT_PORT:-9091}"

VAL_JSON="$(cd "$TF" && terraform output -json validators)"
TARGET_IP=$(echo "$VAL_JSON" | jq -r --arg r "$TARGET_REGION" '.[$r].public_ip')

if [ -z "$TARGET_IP" ] || [ "$TARGET_IP" = "null" ]; then
  echo "error: no public_ip found for region $TARGET_REGION" >&2
  exit 1
fi

mkdir -p "$ROOT/target/perf/run"
OUT="$ROOT/target/perf/run/loadgen-$(date -u +%Y%m%dT%H%M%S).csv"

echo "[run] target=$TARGET_REGION ($TARGET_IP:$CLIENT_PORT) rate=$RATE dur=${DURATION}s"
echo "[run] writing $OUT"

"$ROOT/target/perf/gsx-loadgen" \
  --target "$TARGET_IP:$CLIENT_PORT" \
  --rate "$RATE" \
  --duration "$DURATION" \
  > "$OUT"

echo "[run] done"
