#!/usr/bin/env bash
# Devnet deploy wrapper — thin alias over `scripts/deploy-aws.sh devnet`
# that exists so the deploy path for devnet matches the perf path
# documentation-wise (`./scripts/devnet/deploy.sh apply` vs.
# `./scripts/perf/provision.sh`).
#
# All real work happens in scripts/deploy-aws.sh; this just adds:
#   * A pre-flight check that BILLING_ALARM_EMAIL is set (the devnet
#     billing-cap alarm requires a subscriber).
#   * A reminder about the post-apply steps (render configs, upload
#     genesis + keys, restart bootstrap on each validator).

set -euo pipefail

CMD="${1:-}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

if [[ -z "${CMD}" ]]; then
    cat <<EOF
usage: $(basename "$0") <plan|apply|destroy|status|output>

  plan     — terraform plan (safe; read-only)
  apply    — terraform apply (requires BILLING_ALARM_EMAIL env)
  destroy  — BLOCKED. The devnet is long-lived; an accidental destroy
             wipes chain history. To wipe-and-rebuild, manually delete
             the EBS state volumes after taking a snapshot.
  status   — print current AWS identity
  output   — terraform output

Env vars:
  BILLING_ALARM_EMAIL  Required for apply/destroy. SNS topic subscriber.
  OPERATOR_CIDRS       Optional. Comma-separated CIDRs allowed SSH.
                       Default: auto-detect public IP via checkip.amazonaws.com.
  SSH_PUB              Optional. Path to operator SSH pubkey. Default: ~/.ssh/id_ed25519.pub.
EOF
    exit 1
fi

case "${CMD}" in
    apply|plan|destroy)
        if [[ "${CMD}" == "destroy" ]]; then
            echo "error: devnet destroy is blocked." >&2
            echo "  EBS state volumes have prevent_destroy = true; even with --force" >&2
            echo "  this command would fail. To intentionally wipe devnet:" >&2
            echo "    1. Snapshot every state volume:" >&2
            echo "       aws ec2 create-snapshot --volume-id <id> --description 'pre-wipe'" >&2
            echo "    2. Remove the lifecycle prevent_destroy in terraform/devnet/modules/validator/main.tf" >&2
            echo "    3. terraform apply to update the lifecycle setting" >&2
            echo "    4. Run scripts/deploy-aws.sh destroy devnet" >&2
            exit 1
        fi
        if [[ "${CMD}" == "apply" && -z "${BILLING_ALARM_EMAIL:-}" ]]; then
            echo "error: BILLING_ALARM_EMAIL env var required" >&2
            echo "  example: BILLING_ALARM_EMAIL=ops@globalsettlement.com $0 apply" >&2
            exit 1
        fi
        ;;
esac

exec "${REPO_ROOT}/scripts/deploy-aws.sh" "${CMD}" devnet "${@:2}"
