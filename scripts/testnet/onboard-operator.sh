#!/usr/bin/env bash
# Onboard an external validator operator to the testnet points
# program (Track B). Creates an IAM user scoped to upload events
# under their authority's S3 prefix, prints the access keys + a
# pointer to docs/testnet/VALIDATOR-OPERATORS.md for next steps.
#
# Usage: scripts/testnet/onboard-operator.sh <authority_id> <operator_label>
#
# Example: scripts/testnet/onboard-operator.sh 8 acme-validator-co

set -euo pipefail

if [[ $# -lt 2 ]]; then
    cat <<EOF
usage: $(basename "$0") <authority_id> <operator_label>

Creates an IAM user suwappu-testnet-operator-<label> with write access
to s3://suwappu-dag-testnet-validator-uploads/uploads/<authority_id>/.

The operator runs their validator on their own hardware, configures
it to upload rotated events.ndjson to that prefix every hour, and
gets points credited automatically.

Pre-reqs:
  - <authority_id> already governance-admitted on testnet (see
    OPERATIONS.md § 3 — admit governance Intent).
  - <operator_label> alphanumeric + hyphens only; becomes part of the
    IAM user name.
EOF
    exit 1
fi

AUTHORITY_ID="$1"
LABEL="$2"

if ! [[ "$AUTHORITY_ID" =~ ^[0-9]+$ ]]; then
    echo "error: authority_id must be a non-negative integer" >&2
    exit 1
fi
if ! [[ "$LABEL" =~ ^[a-z0-9][a-z0-9-]*$ ]]; then
    echo "error: operator_label must be alphanumeric + hyphens, starting with a letter or digit" >&2
    exit 1
fi

USER_NAME="suwappu-testnet-operator-${LABEL}"
POLICY_NAME="suwappu-testnet-operator-${LABEL}-upload"
BUCKET="suwappu-dag-testnet-validator-uploads"

# Verify the authority_id is actually admitted on-chain before minting
# IAM credentials. Without this check, a foundation operator can mint
# an orphaned IAM user whose authority_id was never registered, leaving
# stale credentials lying around in IAM. The RPC endpoint is the
# CloudFront wildcard once Phase 1 fronting is live; falls back to the
# us-east-1 validator EIP if the wildcard isn't resolving yet.
RPC_URL="${SUWAPPU_TESTNET_RPC_URL:-https://rpc.testnet.suwappu.globalsettlement.com}"
echo "[onboard-operator] verifying authority_id=${AUTHORITY_ID} is in the Authority Ring (rpc=${RPC_URL})"
REGISTRY_JSON=$(curl -fsS --max-time 10 -X POST -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"suwappu_getAuthorityRegistry"}' \
    "$RPC_URL" 2>&1) || {
    echo "error: failed to reach $RPC_URL — set SUWAPPU_TESTNET_RPC_URL if the wildcard isn't live yet (e.g. http://52.5.240.86:9092)" >&2
    exit 1
}
# `suwappu_getAuthorityRegistry` returns the authority array directly in
# `.result`, not nested under `.result.members`. The prior path
# always evaluated to null and made `jq -e` fail, so this script
# rejected every authority_id as "NOT in the Authority Ring" even
# when admission had succeeded. (Codex #228 P1.)
if ! echo "$REGISTRY_JSON" | jq -e --argjson aid "$AUTHORITY_ID" '.result[] | select(.id == $aid)' >/dev/null 2>&1; then
    echo "error: authority_id=${AUTHORITY_ID} is NOT in the Authority Ring." >&2
    echo "  Submit an AdmitAuthority Intent first via scripts/testnet/admit-operator.sh." >&2
    echo "  Registry snapshot: $(echo "$REGISTRY_JSON" | jq -c '.result // []')" >&2
    exit 1
fi

# Render the policy from the template (which has AUTHORITY_ID_PLACEHOLDER).
# s3:PutObject is the only permission needed — operators upload but
# never list, read, or modify ACLs. Tightest possible scope.
POLICY_DOC=$(cat <<EOF
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Action": ["s3:PutObject"],
      "Resource": ["arn:aws:s3:::${BUCKET}/uploads/${AUTHORITY_ID}/*"]
    }
  ]
}
EOF
)

echo "[onboard-operator] creating IAM user ${USER_NAME}"
AWS_PROFILE=gsn aws iam create-user --user-name "$USER_NAME" \
    --tags Key=testnet:role,Value=external-operator Key=authority_id,Value="$AUTHORITY_ID" \
    >/dev/null

echo "[onboard-operator] attaching scoped upload policy"
AWS_PROFILE=gsn aws iam put-user-policy --user-name "$USER_NAME" \
    --policy-name "$POLICY_NAME" \
    --policy-document "$POLICY_DOC"

echo "[onboard-operator] creating access key"
KEY_JSON=$(AWS_PROFILE=gsn aws iam create-access-key --user-name "$USER_NAME")

ACCESS_KEY_ID=$(echo "$KEY_JSON" | jq -r '.AccessKey.AccessKeyId')
SECRET_KEY=$(echo "$KEY_JSON" | jq -r '.AccessKey.SecretAccessKey')

cat <<EOF
================================================================
Onboarded: ${USER_NAME} (authority_id=${AUTHORITY_ID})
================================================================
Send this block to the operator out-of-band (Signal / 1Password
secure share). It is the ONLY copy of the secret key — AWS won't
let us read it back.

  AWS_ACCESS_KEY_ID=${ACCESS_KEY_ID}
  AWS_SECRET_ACCESS_KEY=${SECRET_KEY}
  AWS_REGION=us-east-1
  UPLOAD_PREFIX=s3://${BUCKET}/uploads/${AUTHORITY_ID}/

  Next steps for the operator:
    1. Read docs/testnet/VALIDATOR-OPERATORS.md
    2. Configure their suwappu-node to rotate events.ndjson hourly
    3. Configure the upload sidecar with the credentials above
================================================================
EOF
