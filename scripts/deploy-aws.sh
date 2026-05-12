#!/usr/bin/env bash
# Wrapper for terraform operations against the gsn AWS profile.
# `terraform destroy` and `terraform apply` without prior `plan` are blocked.

set -euo pipefail

CMD="${1:-}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TF_DIR="${REPO_ROOT}/terraform"

if [[ -z "${CMD}" ]]; then
    cat <<EOF
usage: $(basename "$0") <plan|apply|status>

  plan    — terraform plan (safe; read-only)
  apply   — terraform apply (requires interactive confirmation)
  status  — print current AWS identity + last state-bucket touch

terraform destroy is intentionally blocked by this wrapper and by the
Claude Code denylist (claude-code/settings.json).
EOF
    exit 1
fi

case "${CMD}" in
    plan)
        cd "${TF_DIR}"
        AWS_PROFILE=gsn terraform init -input=false -upgrade
        AWS_PROFILE=gsn terraform plan -out=plan.tfplan
        ;;
    apply)
        cd "${TF_DIR}"
        if [[ ! -f plan.tfplan ]]; then
            echo "no plan.tfplan found — run '$0 plan' first" >&2
            exit 1
        fi
        echo "About to apply plan.tfplan against AWS profile gsn (account 492042618949)."
        read -r -p "Type 'apply' to confirm: " confirmation
        if [[ "${confirmation}" != "apply" ]]; then
            echo "aborted" >&2
            exit 1
        fi
        AWS_PROFILE=gsn terraform apply plan.tfplan
        rm -f plan.tfplan
        ;;
    status)
        AWS_PROFILE=gsn aws sts get-caller-identity
        ;;
    destroy)
        echo "terraform destroy is blocked by this wrapper." >&2
        exit 1
        ;;
    *)
        echo "unknown command: ${CMD}" >&2
        exit 1
        ;;
esac
