#!/usr/bin/env bash
# Join the collected NDJSON logs through gsx-metrics into a single CSV, then
# render the plot.
#
# Usage: scripts/perf/analyze.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
LOG_DIR="$ROOT/target/perf/run/logs"
OUT_CSV="$ROOT/target/perf/run/main_lane.csv"

if [ ! -d "$LOG_DIR" ]; then
  echo "error: no logs collected yet. Run scripts/perf/collect.sh first." >&2
  exit 1
fi

LOG_ARGS=()
for f in "$LOG_DIR"/*.ndjson; do
  region="$(basename "$f" .ndjson)"
  LOG_ARGS+=(--logs "$region=$f")
done

echo "[analyze] joining ${#LOG_ARGS[@]} log files"
"$ROOT/target/perf/gsx-metrics" "${LOG_ARGS[@]}" --lane main > "$OUT_CSV"
echo "[analyze] wrote $OUT_CSV"
wc -l "$OUT_CSV"

echo "[analyze] plotting"
python3 "$ROOT/scripts/perf/plot.py" --csv "$OUT_CSV"
