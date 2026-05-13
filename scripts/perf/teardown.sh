#!/usr/bin/env bash
# Destroy the perf testnet. Stops the EC2 meter.
#
# Usage: scripts/perf/teardown.sh [--yes]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TF="$ROOT/terraform/perf"

CONFIRM=true
for arg in "$@"; do
  case "$arg" in
    --yes) CONFIRM=false ;;
  esac
done

OPERATOR_CIDR="${OPERATOR_CIDR:-0.0.0.0/0}"
SSH_PUB="${SSH_PUB:-$HOME/.ssh/id_ed25519.pub}"
SSH_PUB_CONTENT=""
if [ -f "$SSH_PUB" ]; then
  SSH_PUB_CONTENT="$(cat "$SSH_PUB")"
fi

if [ "$CONFIRM" = true ]; then
  echo "About to destroy the perf testnet (7 regions). Continue? [y/N]"
  read -r answer
  [ "$answer" = "y" ] || [ "$answer" = "Y" ] || { echo "aborted"; exit 1; }
fi

cd "$TF"
terraform destroy -auto-approve \
  -var "operator_ip_cidr=$OPERATOR_CIDR" \
  -var "ssh_public_key=$SSH_PUB_CONTENT"

echo "[teardown] done. Verify with: aws ec2 describe-instances --region us-east-1 --filters Name=tag:Project,Values=gsx-dag --profile gsn"
