#!/usr/bin/env bash
# Testnet deploy wrapper — thin alias over `scripts/deploy-aws.sh testnet`.
# Enforces BILLING_ALARM_EMAIL pre-check and BLOCKS destroy (testnet is
# long-lived; the external-uploads bucket + program RDS hold weeks of
# operator points data we don't want to wipe).

set -euo pipefail

CMD="${1:-}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

if [[ -z "${CMD}" ]]; then
    cat <<EOF
usage: $(basename "$0") <plan|apply|destroy|status|output>

  plan     — terraform plan (safe; read-only)
  apply    — terraform apply (requires BILLING_ALARM_EMAIL env)
  destroy  — BLOCKED. Testnet runs forever (until mainnet). To
             intentionally wipe, follow OPERATIONS.md § "Testnet
             tear-down" (snapshots → unlock prevent_destroy →
             scripts/deploy-aws.sh destroy testnet).
  status   — print current AWS identity
  output   — terraform output

Env vars:
  BILLING_ALARM_EMAIL  Required for apply.
  OPERATOR_CIDRS       Optional. Comma-separated CIDRs for SSH allowlist.
  SSH_PUB              Optional. Path to operator SSH pubkey.
EOF
    exit 1
fi

case "${CMD}" in
    apply|plan|destroy)
        if [[ "${CMD}" == "destroy" ]]; then
            echo "error: testnet destroy is blocked." >&2
            echo "  Testnet carries weeks of validator-program points data + chain history" >&2
            echo "  external developers' transactions land in. To intentionally wipe:" >&2
            echo "    1. Snapshot every state volume (OPERATIONS.md § 8)." >&2
            echo "    2. Export the validator_program RDS via pg_dump." >&2
            echo "    3. Edit terraform/devnet/modules/validator/main.tf (testnet" >&2
            echo "       reuses the devnet validator module) — remove" >&2
            echo "       prevent_destroy on aws_ebs_volume.state." >&2
            echo "    4. terraform apply to update the lifecycle setting." >&2
            echo "    5. scripts/deploy-aws.sh destroy testnet" >&2
            exit 1
        fi
        if [[ "${CMD}" == "apply" && -z "${BILLING_ALARM_EMAIL:-}" ]]; then
            echo "error: BILLING_ALARM_EMAIL env var required" >&2
            echo "  example: BILLING_ALARM_EMAIL=ops@suwappu.bot $0 apply" >&2
            exit 1
        fi
        ;;
esac

exec "${REPO_ROOT}/scripts/deploy-aws.sh" "${CMD}" testnet "${@:2}"
