#!/usr/bin/env bash
# Pull /var/log/gsx/events.ndjson from every validator over SSH and stash it
# locally under target/perf/run/logs/<region>.ndjson.
#
# Usage: scripts/perf/collect.sh [--ssh-user ubuntu]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TF="$ROOT/terraform/perf"
SSH_USER="${SSH_USER:-ubuntu}"
LOG_DIR="$ROOT/target/perf/run/logs"
mkdir -p "$LOG_DIR"

VAL_JSON="$(cd "$TF" && terraform output -json validators)"
regions=$(echo "$VAL_JSON" | jq -r 'keys[]')

for region in $regions; do
  ip=$(echo "$VAL_JSON" | jq -r --arg r "$region" '.[$r].public_ip')
  echo "[collect] $region @ $ip"
  scp -o StrictHostKeyChecking=accept-new \
      "$SSH_USER@$ip:/var/log/gsx/events.ndjson" \
      "$LOG_DIR/$region.ndjson"
done

echo "[collect] done. logs in $LOG_DIR"
ls -lh "$LOG_DIR"
