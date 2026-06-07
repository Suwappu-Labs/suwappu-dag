#!/usr/bin/env bash
# Offboard an external validator operator. Inverse of
# `onboard-operator.sh` — deletes the IAM user, every attached
# access key, and the inline upload policy. Idempotent: succeeds
# silently if the user doesn't exist or has already been
# partially torn down.
#
# Usage: scripts/testnet/offboard-operator.sh <operator_label>
#
# Example: scripts/testnet/offboard-operator.sh acme-validator-co
#
# When to run:
#   - Operator submitted an exit ticket and the foundation has
#     committed the matching `ExitAuthority` Intent
#     (see OPERATIONS.md § 10.4).
#   - An IAM credential leak is suspected and we need to revoke
#     fast — this script is safe to run pre-emptively.
#   - An onboarding attempt failed midway and left dangling state.
#
# What survives:
#   - S3 objects under uploads/<authority_id>/ — those expire
#     naturally via the 14-day lifecycle rule (see
#     `terraform/testnet/external-uploads.tf`). Keeping them lets
#     the points daemon process the operator's last hour or two
#     of uptime before the points window closes.
#   - The operator's `epoch_points` rows in the validator-program
#     DB — historical record; don't delete.

set -euo pipefail

if [[ $# -lt 1 ]]; then
    cat <<EOF
usage: $(basename "$0") <operator_label>

Removes IAM user suwappu-testnet-operator-<label>, all its access
keys, and the inline upload policy.

Idempotent — running twice is safe.
EOF
    exit 1
fi

LABEL="$1"
if ! [[ "$LABEL" =~ ^[a-z0-9][a-z0-9-]*$ ]]; then
    echo "error: operator_label must be alphanumeric + hyphens, starting with a letter or digit" >&2
    exit 1
fi

USER_NAME="suwappu-testnet-operator-${LABEL}"
POLICY_NAME="suwappu-testnet-operator-${LABEL}-upload"

# Helper: only run `aws iam ...` and treat NoSuchEntity as a no-op.
# Other errors (permission denied, throttle) still propagate.
run_idempotent() {
    local what="$1"
    shift
    if ! AWS_PROFILE=gsn aws "$@" 2>/tmp/offboard.err; then
        if grep -q "NoSuchEntity" /tmp/offboard.err; then
            echo "[offboard-operator] ${what}: already gone (NoSuchEntity)"
        else
            echo "error: ${what} failed" >&2
            cat /tmp/offboard.err >&2
            exit 1
        fi
    else
        echo "[offboard-operator] ${what}: ok"
    fi
}

# 1. Drop every access key (zero, one, or many — depending on rotations).
echo "[offboard-operator] listing access keys for ${USER_NAME}"
# Capture the result, distinguishing "user gone" (clean exit) from a
# real failure (permission denied / throttle). The previous `|| true`
# masked every error as an empty key set. (Codex #228 P2.)
if ! KEY_IDS=$(AWS_PROFILE=gsn aws iam list-access-keys --user-name "$USER_NAME" \
    --query 'AccessKeyMetadata[].AccessKeyId' --output text 2>/tmp/offboard.err); then
    if grep -q "NoSuchEntity" /tmp/offboard.err; then
        echo "[offboard-operator] user already deleted — nothing to do"
        exit 0
    fi
    echo "error: failed to list access keys for ${USER_NAME}" >&2
    cat /tmp/offboard.err >&2
    exit 1
fi
for kid in $KEY_IDS; do
    # `aws --output text` prints the literal "None" when the user has
    # zero keys; skip it (and any empty token) so we don't issue
    # delete-access-key for a bogus id, which would break the
    # idempotent re-run path. (Codex #228 P2.)
    [[ "$kid" == "None" || -z "$kid" ]] && continue
    run_idempotent "delete access key $kid" \
        iam delete-access-key --user-name "$USER_NAME" --access-key-id "$kid"
done

# 2. Detach the inline policy.
run_idempotent "delete inline policy $POLICY_NAME" \
    iam delete-user-policy --user-name "$USER_NAME" --policy-name "$POLICY_NAME"

# 3. Delete the user.
run_idempotent "delete user $USER_NAME" \
    iam delete-user --user-name "$USER_NAME"

cat <<EOF
================================================================
Offboarded: ${USER_NAME}
================================================================
IAM tear-down complete. The operator's S3 prefix
uploads/<authority_id>/ stays until the 14-day lifecycle expires
(intentional — gives the points daemon a window to score the
last hour of uptime). Their historical epoch_points rows in the
validator-program DB are preserved.

If the operator was exited via an ExitAuthority Intent, also
remove their row in the validator-program DB if desired:
  DELETE FROM operators WHERE label = '${LABEL}';
(Cascades remove uptime_samples and certs_observed; epoch_points
preserved per the migration schema.)
================================================================
EOF
