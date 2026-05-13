#!/usr/bin/env bash
# End-to-end provision: build binaries, generate keys + genesis, terraform
# apply, render per-region configs, upload artifacts to S3, kick the
# instances' gsx-bootstrap.service so they pull the configs they were
# missing during cloud-init.
#
# Idempotent: re-running with no source changes does almost nothing.
#
# Cost guard: this is the script that actually starts the EC2 meter. It
# prompts for confirmation before the terraform apply unless --yes is
# passed.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TF="$ROOT/terraform/perf"

CONFIRM=true
for arg in "$@"; do
  case "$arg" in
    --yes) CONFIRM=false ;;
  esac
done

if [ -z "${OPERATOR_CIDR:-}" ]; then
  OPERATOR_CIDR="$(curl -fsS ifconfig.me)/32"
  echo "[provision] detected operator IP: $OPERATOR_CIDR"
fi
SSH_PUB="${SSH_PUB:-$HOME/.ssh/id_ed25519.pub}"
if [ ! -f "$SSH_PUB" ]; then
  echo "error: SSH public key not found at $SSH_PUB. Set SSH_PUB=/path/to/key.pub." >&2
  exit 1
fi
SSH_PUB_CONTENT="$(cat "$SSH_PUB")"

echo "[provision] 1/4 — build binaries"
"$ROOT/scripts/perf/build.sh"

echo "[provision] 2/4 — generate keys + genesis"
"$ROOT/scripts/perf/gen-genesis.py" --out-dir "$ROOT/target/perf/keys"

echo "[provision] 3/4 — terraform apply"
if [ "$CONFIRM" = true ]; then
  cat <<EOF
ABOUT TO SPEND MONEY. This terraform apply will:
  - Create a VPC + t3.small + EIP in 7 AWS regions (~\$3.50/day)
  - Create a versioned S3 bucket (gsx-dag-perf-artifacts)
  - Open consensus + client ports to 0.0.0.0/0 (auth happens at message layer)
Operator IP: $OPERATOR_CIDR

Continue? [y/N]
EOF
  read -r answer
  [ "$answer" = "y" ] || [ "$answer" = "Y" ] || { echo "aborted"; exit 1; }
fi

cd "$TF"
terraform init -upgrade
terraform apply -auto-approve \
  -var "operator_ip_cidr=$OPERATOR_CIDR" \
  -var "ssh_public_key=$SSH_PUB_CONTENT"

echo "[provision] 4/4 — render configs and upload to S3"
"$ROOT/scripts/perf/render-configs.sh"

BUCKET="$(terraform output -raw artifact_bucket)"
cd "$ROOT"

aws s3 cp target/perf/gsx-node "s3://$BUCKET/bin/gsx-node" --profile gsn
aws s3 cp target/perf/keys/genesis.toml "s3://$BUCKET/genesis/genesis.toml" --profile gsn

for region in us-east-1 us-west-2 eu-west-1 ap-northeast-1 ap-southeast-2 sa-east-1 af-south-1; do
  aws s3 cp "target/perf/configs/$region/node.toml" "s3://$BUCKET/configs/$region/node.toml" --profile gsn
  aws s3 cp "target/perf/keys/$region/mldsa.sk" "s3://$BUCKET/keys/$region/mldsa.sk" --profile gsn
  aws s3 cp "target/perf/keys/$region/bls.sk" "s3://$BUCKET/keys/$region/bls.sk" --profile gsn
done

echo "[provision] done. SSH to a validator with:"
echo "  ssh ubuntu@\$(cd terraform/perf && terraform output -json validators | jq -r '.\"us-east-1\".public_ip')"
