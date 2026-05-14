#!/usr/bin/env bash
# Wrapper for terraform operations against the gsn AWS profile.
#
# The default stack is `root` (terraform/) — production validator infra
# that is intentionally append-only: `destroy` is blocked here AND by the
# Claude Code denylist (claude-code/settings.json).
#
# The `perf` stack (terraform/perf/) is the geographically distributed
# performance testnet. It exists to be torn down at the end of each
# campaign, so `destroy` IS allowed there.

set -euo pipefail

CMD="${1:-}"
STACK="${2:-root}"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

case "${STACK}" in
    root) TF_DIR="${REPO_ROOT}/terraform" ;;
    perf) TF_DIR="${REPO_ROOT}/terraform/perf" ;;
    *)
        echo "error: unknown stack '${STACK}' (root|perf)" >&2
        exit 1
        ;;
esac

if [[ -z "${CMD}" ]]; then
    cat <<EOF
usage: $(basename "$0") <plan|apply|destroy|status|output> [stack]

  plan     — terraform plan (safe; read-only)
  apply    — terraform apply (requires interactive confirmation)
  destroy  — terraform destroy (perf stack only; root stack is locked)
  status   — print current AWS identity + active stack
  output   — terraform output for the chosen stack

  stack:
    root  — terraform/ (production; default; destroy blocked)
    perf  — terraform/perf/ (geo-perf testnet; destroy allowed)

destroy on the root stack is intentionally blocked by this wrapper and by
the Claude Code denylist (claude-code/settings.json).
EOF
    exit 1
fi

# Identity sanity check — refuse to act on an unexpected account.
EXPECTED_ACCOUNT="492042618949"
ACTUAL_ACCOUNT=$(AWS_PROFILE=gsn aws sts get-caller-identity --query Account --output text 2>/dev/null || echo "")
if [[ "${ACTUAL_ACCOUNT}" != "${EXPECTED_ACCOUNT}" ]]; then
    echo "error: AWS_PROFILE=gsn resolved to account '${ACTUAL_ACCOUNT}', expected '${EXPECTED_ACCOUNT}'" >&2
    exit 1
fi

# Per-stack variable assembly. The perf stack needs operator IPs + SSH pubkey.
TF_VARS=()
if [[ "${STACK}" == "perf" && ( "${CMD}" == "plan" || "${CMD}" == "apply" || "${CMD}" == "destroy" ) ]]; then
    # Operator IP allowlist. By default the script auto-detects the current
    # public IP and uses that as the only entry. Override with
    # OPERATOR_CIDRS="1.2.3.4/32,5.6.7.8/32" (comma-separated) to keep
    # multiple networks reachable across re-applies — useful when the same
    # operator works from home + mobile hotspot + office.
    if [[ -z "${OPERATOR_CIDRS:-}" ]]; then
        # checkip.amazonaws.com is IPv4-only — the security group's
        # cidr_blocks field requires a v4 CIDR, and ifconfig.me returns v6
        # on dual-stack hosts even with curl -4.
        OPERATOR_CIDRS="$(curl -fsS https://checkip.amazonaws.com)/32"
        echo "[deploy-aws] detected operator IP: ${OPERATOR_CIDRS}"
    fi
    # Convert the comma-separated env value into a HCL list literal.
    CIDR_LIST="["
    OLDIFS="${IFS}"
    IFS=','
    for c in ${OPERATOR_CIDRS}; do
        CIDR_LIST+="\"${c}\","
    done
    IFS="${OLDIFS}"
    CIDR_LIST="${CIDR_LIST%,}]"

    SSH_PUB="${SSH_PUB:-$HOME/.ssh/id_ed25519.pub}"
    if [[ ! -f "${SSH_PUB}" ]]; then
        echo "error: SSH public key not found at ${SSH_PUB}" >&2
        echo "  generate one with: ssh-keygen -t ed25519 -f \$HOME/.ssh/gsx-perf -N \"\"" >&2
        exit 1
    fi
    TF_VARS+=(-var "operator_ip_cidrs=${CIDR_LIST}")
    TF_VARS+=(-var "ssh_public_key=$(cat "${SSH_PUB}")")
fi

case "${CMD}" in
    plan)
        cd "${TF_DIR}"
        AWS_PROFILE=gsn terraform init -input=false -upgrade
        AWS_PROFILE=gsn terraform plan -out=plan.tfplan "${TF_VARS[@]}"
        ;;
    apply)
        cd "${TF_DIR}"
        AWS_PROFILE=gsn terraform init -input=false -upgrade
        AWS_PROFILE=gsn terraform plan -out=plan.tfplan "${TF_VARS[@]}"
        echo ""
        echo "================================================================"
        echo "  Stack:   ${STACK} (${TF_DIR})"
        echo "  Account: ${ACTUAL_ACCOUNT}"
        echo "================================================================"
        echo "Plan written to plan.tfplan. About to apply against AWS gsn."
        read -r -p "Type 'apply' to confirm: " confirmation
        if [[ "${confirmation}" != "apply" ]]; then
            echo "aborted" >&2
            exit 1
        fi
        AWS_PROFILE=gsn terraform apply plan.tfplan
        rm -f plan.tfplan
        ;;
    destroy)
        if [[ "${STACK}" == "root" ]]; then
            echo "error: destroy on the root stack is blocked by this wrapper." >&2
            exit 1
        fi
        cd "${TF_DIR}"
        AWS_PROFILE=gsn terraform init -input=false -upgrade
        AWS_PROFILE=gsn terraform plan -destroy -out=plan.tfplan "${TF_VARS[@]}"
        echo ""
        echo "================================================================"
        echo "  Stack:   ${STACK} (${TF_DIR})"
        echo "  Action:  DESTROY"
        echo "  Account: ${ACTUAL_ACCOUNT}"
        echo "================================================================"
        read -r -p "Type 'destroy' to confirm: " confirmation
        if [[ "${confirmation}" != "destroy" ]]; then
            echo "aborted" >&2
            exit 1
        fi
        AWS_PROFILE=gsn terraform apply plan.tfplan
        rm -f plan.tfplan
        ;;
    output)
        cd "${TF_DIR}"
        AWS_PROFILE=gsn terraform output "${@:3}"
        ;;
    status)
        AWS_PROFILE=gsn aws sts get-caller-identity
        echo "stack: ${STACK} (${TF_DIR})"
        ;;
    *)
        echo "unknown command: ${CMD}" >&2
        exit 1
        ;;
esac
