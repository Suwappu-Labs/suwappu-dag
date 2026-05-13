#!/usr/bin/env bash
# End-to-end provision of the 7-region perf testnet.
#
# Order matters:
#   1. Generate placeholder keys + genesis manifest locally.
#   2. terraform apply via scripts/deploy-aws.sh — creates S3 bucket,
#      CodeBuild project, and 7 region VPCs/EC2s/EIPs.
#   3. Ensure the gsx-db SSH deploy key is in SSM (CodeBuild needs it for
#      the private cargo dep).
#   4. CodeBuild compiles gsx-node binaries on AWS.
#   5. Render per-region node.toml using the now-known EIPs.
#   6. Upload binaries / configs / keys to S3.
#   7. SSH-restart each validator's gsx-bootstrap.service so cloud-init
#      pulls the just-uploaded config and starts gsx-node.
#
# This script is idempotent — rerun if anything fails.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TF="$ROOT/terraform/perf"
export AWS_PROFILE=gsn

echo "[provision] 1/7 — generate keys + genesis manifest"
"$ROOT/scripts/perf/gen-genesis.py" --out-dir "$ROOT/target/perf/keys"

echo "[provision] 2/7 — terraform apply via deploy-aws.sh"
"$ROOT/scripts/deploy-aws.sh" apply perf

# After apply, harvest the bucket name once. Used by every subsequent step.
BUCKET="$(cd "$TF" && terraform output -raw artifact_bucket)"
echo "[provision] artifact bucket: $BUCKET"

echo "[provision] 3/7 — ensure gsx-db deploy key in SSM"
if ! aws ssm get-parameter --name /gsx-perf/gsx-db-deploy-key --region us-east-1 >/dev/null 2>&1; then
  KEY_PATH="${GSX_DB_DEPLOY_KEY:-$HOME/.ssh/gsx-db-deploy}"
  if [ ! -f "$KEY_PATH" ]; then
    echo "error: SSM parameter /gsx-perf/gsx-db-deploy-key not present and" >&2
    echo "       no key file at $KEY_PATH to upload. Either set" >&2
    echo "       GSX_DB_DEPLOY_KEY=/path/to/private-key, or upload manually:" >&2
    echo "  aws ssm put-parameter --name /gsx-perf/gsx-db-deploy-key \\" >&2
    echo "    --type SecureString --value \"\$(cat ~/.ssh/gsx-db-deploy)\" \\" >&2
    echo "    --profile gsn --region us-east-1" >&2
    exit 1
  fi
  echo "[provision]   uploading deploy key from $KEY_PATH"
  aws ssm put-parameter \
    --name /gsx-perf/gsx-db-deploy-key \
    --type SecureString \
    --value "$(cat "$KEY_PATH")" \
    --region us-east-1 \
    --overwrite >/dev/null
fi

echo "[provision] 4/7 — run CodeBuild musl build"
"$ROOT/scripts/perf/build.sh"

echo "[provision] 5/7 — render per-region node.toml from terraform outputs"
"$ROOT/scripts/perf/render-configs.sh"

echo "[provision] 6/7 — upload genesis + configs + keys to S3"
aws s3 cp "$ROOT/target/perf/keys/genesis.toml" "s3://$BUCKET/genesis/genesis.toml" --region us-east-1
for region in us-east-1 us-west-2 eu-west-1 ap-northeast-1 ap-southeast-2 sa-east-1; do
  aws s3 cp "$ROOT/target/perf/configs/$region/node.toml" "s3://$BUCKET/configs/$region/node.toml" --region us-east-1
  aws s3 cp "$ROOT/target/perf/keys/$region/mldsa.sk" "s3://$BUCKET/keys/$region/mldsa.sk" --region us-east-1
  aws s3 cp "$ROOT/target/perf/keys/$region/bls.sk" "s3://$BUCKET/keys/$region/bls.sk" --region us-east-1
done

echo "[provision] 7/7 — kick gsx-bootstrap.service on every validator"
SSH_KEY="${SSH_KEY:-$HOME/.ssh/id_ed25519}"
VAL_JSON="$(cd "$TF" && terraform output -json validators)"
for region in $(echo "$VAL_JSON" | jq -r 'keys[]'); do
  ip=$(echo "$VAL_JSON" | jq -r --arg r "$region" '.[$r].public_ip')
  echo "[provision]   $region @ $ip"
  ssh -i "$SSH_KEY" -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10 \
    "ubuntu@$ip" \
    "sudo systemctl restart gsx-bootstrap && sudo systemctl restart gsx-node" \
    || echo "[provision]   WARN: ssh to $region failed; retry after cloud-init finishes"
done

echo "[provision] done. SSH check:"
echo "  ssh -i $SSH_KEY ubuntu@\$(scripts/deploy-aws.sh output perf -json validators | jq -r '.\"us-east-1\".public_ip') sudo journalctl -u gsx-node -n 50"
