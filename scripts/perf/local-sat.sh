#!/usr/bin/env bash
# Local saturation benchmark for the performance loop.
#
# Launches a 4-node loopback devnet, drives it with LOADGENS concurrent
# suwappu-loadgen processes (each opens one connection per validator, so
# the mempool's per-connection 50 tok/s bucket stops being the ceiling),
# then reports committed TPS, end-to-end latency percentiles, round time
# and node CPU cost per committed intent, and appends one row to
# docs/perf/local-history.csv.
#
# Usage:
#   LABEL=baseline ./scripts/perf/local-sat.sh
#   LOADGENS=6 RATE_PER=190 DURATION=30 LABEL=x ./scripts/perf/local-sat.sh
#
# Requires release binaries:
#   cargo build --release -p suwappu-crypto --bin suwappu-keygen -p suwappu-node --bins
#
# This is a single-host number on a shared box: compare rows against each
# other (same LOADGENS/RATE_PER/DURATION/ROUND_MS), not against a WAN
# testnet.

set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$repo_root"

LOADGENS=${LOADGENS:-4}
RATE_PER=${RATE_PER:-190}
DURATION=${DURATION:-30}
BATCH_SIZE=${BATCH_SIZE:-20}
ROUND_MS=${ROUND_MS:-250}
LABEL=${LABEL:-run}
OUT_DIR=${OUT_DIR:-target/perf-local}
HISTORY=${HISTORY:-docs/perf/local-history.csv}
BASE_PORT=${BASE_PORT:-9300}

for bin in suwappu-keygen suwappu-node suwappu-loadgen; do
  if [ ! -x "target/release/${bin}" ]; then
    echo "error: target/release/${bin} missing; build with:" >&2
    echo "  cargo build --release -p suwappu-crypto --bin suwappu-keygen -p suwappu-node --bins" >&2
    exit 1
  fi
done
export PATH="${repo_root}/target/release:${PATH}"

rm -rf "${OUT_DIR}"
python3 scripts/gen-devnet-genesis.py --num-nodes 4 --out-dir "${OUT_DIR}" > /dev/null 2>&1
mkdir -p "${OUT_DIR}/logs" "${OUT_DIR}/loadgen"

for i in 0 1 2 3; do
  base=$((BASE_PORT + i * 10))
  {
    echo "self_id = \"v${i}\""
    echo "authority_id = ${i}"
    echo "listen = \"127.0.0.1:${base}\""
    echo "client_listen = \"127.0.0.1:$((base + 1))\""
    echo "round_ms = ${ROUND_MS}"
    echo "checkpoint_cadence_rounds = 1"
    echo "mldsa_secret_key_path = \"${repo_root}/${OUT_DIR}/v${i}/mldsa.sk\""
    echo "bls_secret_key_path = \"${repo_root}/${OUT_DIR}/v${i}/bls.sk\""
    echo "genesis_manifest_path = \"${repo_root}/${OUT_DIR}/genesis.toml\""
    echo "event_log_path = \"${repo_root}/${OUT_DIR}/v${i}/events.ndjson\""
    echo "max_client_connections = 256"
    echo "client_idle_timeout_ms = 30000"
    echo "client_per_ip_limit = 32"
    for j in 0 1 2 3; do
      if [ "$j" != "$i" ]; then
        echo "[[peers]]"
        echo "id = \"v${j}\""
        echo "addr = \"127.0.0.1:$((BASE_PORT + j * 10))\""
      fi
    done
  } > "${OUT_DIR}/v${i}/node.toml"
done

pids=()
for i in 0 1 2 3; do
  suwappu-node --config "${OUT_DIR}/v${i}/node.toml" > "${OUT_DIR}/logs/v${i}.log" 2>&1 &
  pids+=($!)
done
cleanup() { kill "${pids[@]}" 2>/dev/null || true; wait 2>/dev/null || true; }
trap cleanup EXIT

# Warm-up: let the mesh connect and commit a few rounds.
sleep 6

cpu_ticks() {  # utime+stime of each node, space separated
  for p in "${pids[@]}"; do awk '{print $14+$15}' "/proc/${p}/stat"; done | paste -sd' '
}
cpu_start=$(cpu_ticks)
t_start=$(date +%s%3N)

targets=""
for i in 0 1 2 3; do targets+="127.0.0.1:$((BASE_PORT + i * 10 + 1)),"; done
targets=${targets%,}
lg_pids=()
for k in $(seq 1 "${LOADGENS}"); do
  suwappu-loadgen --targets "${targets}" --rate "${RATE_PER}" --duration "${DURATION}" \
    --batch-size "${BATCH_SIZE}" --seed "$((k * 7919 + $(date +%s)))" \
    --mldsa-secret-key "${OUT_DIR}/v0/mldsa.sk" --mldsa-public-key "${OUT_DIR}/v0/mldsa.pk" \
    --network-id suwappu-devnet-local \
    > "${OUT_DIR}/loadgen/${k}.csv" 2> "${OUT_DIR}/loadgen/${k}.err" &
  lg_pids+=($!)
done
wait "${lg_pids[@]}" || true
t_end=$(date +%s%3N)
cpu_end=$(cpu_ticks)
# Drain: let in-flight intents commit.
sleep 4
cleanup
trap - EXIT

python3 scripts/perf/local_report.py \
  --out-dir "${OUT_DIR}" --history "${HISTORY}" --label "${LABEL}" \
  --loadgens "${LOADGENS}" --rate-per "${RATE_PER}" --duration "${DURATION}" \
  --round-ms "${ROUND_MS}" --t-start "${t_start}" --t-end "${t_end}" \
  --cpu-start "${cpu_start}" --cpu-end "${cpu_end}" \
  --sha "$(git rev-parse --short HEAD)$(git diff --quiet || echo '+dirty')"
