#!/usr/bin/env bash
# Minimal local TPS benchmark: launches a 4-node loopback devnet (no
# docker), drives it with suwappu-loadgen, joins the resulting
# per-validator event logs with suwappu-metrics, and prints a real
# P50/P95/P99 + steady-state TPS report.
#
# This is the epic item #4 "3-5 day minimal version" — see
# tasks/pq-competitive-gaps-epic.md item 4. It is deliberately NOT the
# 7-region AWS perf-testnet (scripts/perf/); it answers "does the DAG
# commit real signed intents end-to-end and how fast, on one box" — an
# honest early-testnet number, not a mainnet-representative one.
#
# IMPORTANT rate-limit caveat: suwappu-mempool's per-connection leaky
# bucket (crates/suwappu-mempool/src/mempool.rs MempoolConfig::default)
# is hardcoded to 50 tokens/sec refill, 100 burst — NOT exposed via
# node.toml. suwappu-loadgen opens exactly one persistent connection per
# `--target`, so the sustainable ceiling from a single loadgen process is
# `num_targets * 50` TPS; requesting more with `--rate` causes mid-batch
# `mempool ... rate limited` rejections that (as of this writing) leave
# the connection unusable ("Broken pipe" on the next batch) rather than
# recovering. This script's default RATE stays safely under that ceiling
# for a clean run. Reaching a higher network-saturation number needs
# multiple concurrent client connections per target (more loadgen
# processes / a multi-connection loadgen mode) — not implemented here.
#
# Usage:
#   ./scripts/devnet-local-bench.sh                 # RATE=180 DURATION=30
#   RATE=150 DURATION=60 ./scripts/devnet-local-bench.sh
#
# Requirements: a `cargo build --release` of suwappu-keygen, suwappu-node,
# suwappu-loadgen, suwappu-metrics (this script does NOT build them, to
# keep re-runs fast — build once with the command printed below if any
# binary is missing).

set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

RATE=${RATE:-180}
DURATION=${DURATION:-30}
BATCH_SIZE=${BATCH_SIZE:-20}
OUT_DIR=${OUT_DIR:-target/devnet}
BUILD_CMD="cargo build --release -p suwappu-crypto --bin suwappu-keygen -p suwappu-node --bins"

for bin in suwappu-keygen suwappu-node suwappu-loadgen suwappu-metrics; do
  if [ ! -x "target/release/${bin}" ]; then
    echo "error: target/release/${bin} not found. Build first:" >&2
    echo "  ${BUILD_CMD}" >&2
    exit 1
  fi
done

export PATH="${repo_root}/target/release:${PATH}"

echo "==> generating devnet genesis (4 nodes, real ML-DSA-65/BLS keys) under ${OUT_DIR}/..."
rm -rf "${OUT_DIR}"
python3 scripts/gen-devnet-genesis.py --num-nodes 4 --out-dir "${OUT_DIR}"

echo
echo "==> writing loopback node.toml (v0..v3 on 127.0.0.1:9100..9131)..."
mkdir -p "${OUT_DIR}/logs"
for i in 0 1 2 3; do
  base=$((9100 + i * 10))
  listen=$((base))
  client=$((base + 1))
  rpc=$((base + 2))
  {
    echo "self_id = \"v${i}\""
    echo "authority_id = ${i}"
    echo "listen = \"127.0.0.1:${listen}\""
    echo "client_listen = \"127.0.0.1:${client}\""
    echo "rpc_listen = \"127.0.0.1:${rpc}\""
    echo "round_ms = 250"
    echo "checkpoint_cadence_rounds = 1"
    echo "mldsa_secret_key_path = \"${repo_root}/${OUT_DIR}/v${i}/mldsa.sk\""
    echo "bls_secret_key_path = \"${repo_root}/${OUT_DIR}/v${i}/bls.sk\""
    echo "genesis_manifest_path = \"${repo_root}/${OUT_DIR}/genesis.toml\""
    echo "event_log_path = \"${repo_root}/${OUT_DIR}/v${i}/events.ndjson\""
    echo
    echo "max_client_connections = 256"
    echo "client_idle_timeout_ms = 30000"
    echo "client_per_ip_limit = 8"
    echo
    for j in 0 1 2 3; do
      if [ "$j" != "$i" ]; then
        jbase=$((9100 + j * 10))
        echo "[[peers]]"
        echo "id = \"v${j}\""
        echo "addr = \"127.0.0.1:${jbase}\""
      fi
    done
  } > "${OUT_DIR}/v${i}/node.toml"
done

echo
echo "==> starting 4 validators..."
pids=()
for i in 0 1 2 3; do
  ./target/release/suwappu-node --config "${OUT_DIR}/v${i}/node.toml" > "${OUT_DIR}/logs/v${i}.log" 2>&1 &
  pids+=($!)
done
cleanup() {
  echo "==> stopping validators..."
  kill "${pids[@]}" 2>/dev/null || true
}
trap cleanup EXIT

echo "==> waiting for v0 JSON-RPC..."
for _ in $(seq 1 30); do
  if curl -sfX POST http://127.0.0.1:9102 \
       -H 'content-type: application/json' \
       -d '{"jsonrpc":"2.0","id":1,"method":"suwappu_getEpoch","params":null}' \
       > /dev/null 2>&1; then
    break
  fi
  sleep 1
done

echo
echo "==> running suwappu-loadgen: rate=${RATE} duration=${DURATION}s batch=${BATCH_SIZE} against 4 targets..."
./target/release/suwappu-loadgen \
  --targets 127.0.0.1:9101,127.0.0.1:9111,127.0.0.1:9121,127.0.0.1:9131 \
  --rate "${RATE}" --duration "${DURATION}" --batch-size "${BATCH_SIZE}" --seed "$(date +%s 2>/dev/null || echo 1)" \
  --mldsa-secret-key "${OUT_DIR}/v0/mldsa.sk" \
  --mldsa-public-key "${OUT_DIR}/v0/mldsa.pk" \
  --network-id suwappu-devnet-local \
  > "${OUT_DIR}/loadgen.out" 2> "${OUT_DIR}/loadgen.err"
tail -5 "${OUT_DIR}/loadgen.err"

echo
echo "==> joining event logs with suwappu-metrics (mode=e2e)..."
./target/release/suwappu-metrics \
  --logs v0="${OUT_DIR}/v0/events.ndjson" \
  --logs v1="${OUT_DIR}/v1/events.ndjson" \
  --logs v2="${OUT_DIR}/v2/events.ndjson" \
  --logs v3="${OUT_DIR}/v3/events.ndjson" \
  --loadgen-csv "${OUT_DIR}/loadgen.out" \
  --mode e2e > "${OUT_DIR}/metrics_e2e.csv"

echo
echo "==> report (deduped by tx_hash, network-first-finalized latency):"
python3 - "${OUT_DIR}/metrics_e2e.csv" <<'PYEOF'
import csv, statistics, sys
from collections import defaultdict

with open(sys.argv[1]) as f:
    rows = list(csv.DictReader(f))

by_tx = defaultdict(list)
for r in rows:
    by_tx[r["tx_hash"]].append(r)

best = {tx: min(rs, key=lambda r: int(r["first_committed_ms"])) for tx, rs in by_tx.items()}
lat = sorted(int(r["e2e_latency_ms"]) for r in best.values())
n = len(lat)
if n == 0:
    print("no committed intents joined — check loadgen.err / node logs")
    sys.exit(1)

def pct(p):
    return lat[min(n - 1, int(n * p / 100))]

committed = sorted(int(r["first_committed_ms"]) for r in best.values())
span_s = (committed[-1] - committed[0]) / 1000 if n > 1 else 0

print(f"  unique intents committed: {n}")
print(f"  p50 latency:  {pct(50)} ms")
print(f"  p95 latency:  {pct(95)} ms")
print(f"  p99 latency:  {pct(99)} ms")
print(f"  min/max:      {lat[0]} / {lat[-1]} ms")
print(f"  mean:         {statistics.mean(lat):.1f} ms")
if span_s > 0:
    print(f"  steady-state TPS: {n / span_s:.1f} (over {span_s:.1f}s commit span)")
print()
print("  NOTE: this is an early-testnet, single-host, single-loadgen-process")
print("  number bound by suwappu-mempool's fixed 50 tok/s-per-connection rate")
print("  limit (4 targets => ~200 TPS ceiling from one loadgen process), NOT")
print("  a network-saturation or mainnet-representative figure.")
PYEOF
