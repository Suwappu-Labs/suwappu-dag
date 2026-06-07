#!/usr/bin/env bash
# Wrapper for terraform operations against the gsn AWS profile.
#
# Stacks:
#   root    — terraform/ (production validator infra; append-only, destroy blocked)
#   perf    — terraform/perf/ (geo-perf testnet; destroy allowed, paused between runs)
#   devnet  — terraform/devnet/ (long-lived public devnet; destroy blocked — devnet
#             state survives months; an accidental `terraform destroy` would wipe
#             the chain history that external developers' transactions land in)
#   testnet — terraform/testnet/ (incentivized public testnet, 7-region seed cluster +
#             external operator points program; destroy blocked — testnet runs until
#             mainnet and carries weeks of points data + chain history)
#
# The destroy-blocked stacks (root, devnet) are protected at THREE layers:
#   1. this wrapper (refuses the command),
#   2. the Claude Code denylist (claude-code/settings.json),
#   3. `prevent_destroy = true` on the EBS state volumes (terraform itself).

set -euo pipefail

CMD="${1:-}"
STACK="${2:-root}"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

case "${STACK}" in
    root)    TF_DIR="${REPO_ROOT}/terraform" ;;
    perf)    TF_DIR="${REPO_ROOT}/terraform/perf" ;;
    devnet)  TF_DIR="${REPO_ROOT}/terraform/devnet" ;;
    testnet) TF_DIR="${REPO_ROOT}/terraform/testnet" ;;
    *)
        echo "error: unknown stack '${STACK}' (root|perf|devnet|testnet)" >&2
        exit 1
        ;;
esac

if [[ -z "${CMD}" ]]; then
    cat <<EOF
usage: $(basename "$0") <plan|apply|destroy|status|output> [stack]

  plan     — terraform plan (safe; read-only)
  apply    — terraform apply (requires interactive confirmation)
  destroy  — terraform destroy (perf stack only; root + devnet are locked)
  status   — print current AWS identity + active stack
  output   — terraform output for the chosen stack

  stack:
    root     — terraform/ (production; default; destroy blocked)
    perf     — terraform/perf/ (geo-perf testnet; destroy allowed)
    devnet   — terraform/devnet/ (public devnet; destroy blocked)
    testnet  — terraform/testnet/ (incentivized public testnet; destroy blocked)

destroy on root + devnet + testnet stacks is intentionally blocked by
this wrapper, the Claude Code denylist (claude-code/settings.json), and
lifecycle prevent_destroy on EBS state volumes.
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

# Per-stack variable assembly. The perf + devnet stacks need operator IPs +
# SSH pubkey. Devnet additionally needs a billing-alarm email subscriber.
TF_VARS=()
if [[ ( "${STACK}" == "perf" || "${STACK}" == "devnet" || "${STACK}" == "testnet" ) && ( "${CMD}" == "plan" || "${CMD}" == "apply" || "${CMD}" == "destroy" ) ]]; then
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
        echo "  generate one with: ssh-keygen -t ed25519 -f \$HOME/.ssh/suwappu-perf -N \"\"" >&2
        exit 1
    fi
    TF_VARS+=(-var "operator_ip_cidrs=${CIDR_LIST}")
    TF_VARS+=(-var "ssh_public_key=$(cat "${SSH_PUB}")")

    # Devnet + testnet require a billing-alarm email subscriber.
    # Without it, the SNS topic has no consumer and the cost-cap
    # alarm is non-functional.
    if [[ "${STACK}" == "devnet" || "${STACK}" == "testnet" ]]; then
        if [[ -z "${BILLING_ALARM_EMAIL:-}" ]]; then
            echo "error: BILLING_ALARM_EMAIL env var required for ${STACK} apply/destroy" >&2
            echo "  example: BILLING_ALARM_EMAIL=ops@globalsettlement.com ./scripts/deploy-aws.sh apply ${STACK}" >&2
            exit 1
        fi
        TF_VARS+=(-var "billing_alarm_email=${BILLING_ALARM_EMAIL}")
    fi
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
        if [[ "${STACK}" == "root" || "${STACK}" == "devnet" || "${STACK}" == "testnet" ]]; then
            echo "error: destroy on the ${STACK} stack is blocked by this wrapper." >&2
            echo "  devnet/testnet state survives months; an accidental destroy" >&2
            echo "  wipes chain history that external developers' transactions" >&2
            echo "  land in, plus the validator-program points data." >&2
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
