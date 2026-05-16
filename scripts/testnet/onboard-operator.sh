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

Creates an IAM user gsx-testnet-operator-<label> with write access
to s3://gsx-dag-testnet-validator-uploads/uploads/<authority_id>/.

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

USER_NAME="gsx-testnet-operator-${LABEL}"
POLICY_NAME="gsx-testnet-operator-${LABEL}-upload"
BUCKET="gsx-dag-testnet-validator-uploads"

# Render the policy from the template (which has AUTHORITY_ID_PLACEHOLDER).
POLICY_DOC=$(cat <<EOF
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Action": ["s3:PutObject", "s3:PutObjectAcl"],
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
    2. Configure their gsx-node to rotate events.ndjson hourly
    3. Configure the upload sidecar with the credentials above
================================================================
EOF
