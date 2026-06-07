#!/usr/bin/env bash
# End-to-end wrapper to admit a new external validator operator onto
# the testnet Authority Ring and onboard them with IAM credentials.
#
# Pipeline:
#   1. Pull the foundation signer's ML-DSA keypair from AWS Secrets
#      Manager (today: the faucet authority — same key the faucet
#      uses to sign Transfer intents; it's already in the Authority
#      Ring so submissions clear the UnknownSigner gate).
#   2. Invoke examples/rust/admit_authority binary to build, sign, and
#      submit an Intent::AdmitAuthority for the candidate.
#   3. Poll the on-chain registry until the candidate appears.
#   4. Call scripts/testnet/onboard-operator.sh to mint their IAM
#      credentials.
#   5. Register the operator in the points-accumulator daemon via the
#      /admin/operators endpoint.
#   6. Print the "operator packet" to forward via Signal / 1Password.
#
# Usage:
#   scripts/testnet/admit-operator.sh \
#       --authority-id 8 \
#       --label acme-validator-co \
#       --mldsa-pk-hex f515ad3a...ce441 \
#       --bls-pk-hex   050b11c0...d7e8b
#
# Pre-reqs:
#   - AWS_PROFILE=gsn resolves to account 492042618949.
#   - Foundation faucet ML-DSA secret already in Secrets Manager at
#     suwappu-testnet/faucet/mldsa-secret-key (populated during the
#     testnet bringup; see OPERATIONS.md § 10.1 step 4).
#   - The points-accumulator daemon admin bearer token is in
#     Secrets Manager at suwappu-testnet/program/admin-token (set
#     during § 10.3 deploy).
#   - examples/rust workspace builds (one-time:
#     `cd examples/rust && cargo build --release --bin admit_authority`).
#   - jq, curl, aws CLI on PATH.
#   - For the KYC precheck (unless --skip-kyc-check): a hex decoder
#     (`xxd` from vim-common, or `python3`) AND a blake3 hasher
#     (`b3sum`, or the Python `blake3` module). The script falls back
#     xxd→python3 and b3sum→python-blake3, erroring clearly if neither
#     is present.

set -euo pipefail

# Defaults pulled from the testnet stack.
RPC_URL_DEFAULT="https://rpc.testnet.suwappu.globalsettlement.com"
PROGRAM_URL_DEFAULT="https://program.testnet.suwappu.globalsettlement.com"
NETWORK_ID="suwappu-testnet-v1"
FAUCET_SECRET_ID="suwappu-testnet/faucet/mldsa-secret-key"
PROGRAM_ADMIN_SECRET_ID="suwappu-testnet/program/admin-token"
DEFAULT_STAKE_SUWAPPU=100000
KYC_APPLICATIONS_TABLE="suwappu_testnet_applications"

usage() {
    cat <<EOF
usage: $(basename "$0") \\
    --authority-id <N> \\
    --label <slug> \\
    --mldsa-pk-hex <3904 hex chars> \\
    --bls-pk-hex <96 hex chars> \\
    [--stake-suwappu <amount>]   default=${DEFAULT_STAKE_SUWAPPU} (floor: 100,000) \\
    [--rpc-url <url>]        default=${RPC_URL_DEFAULT} \\
    [--program-url <url>]    default=${PROGRAM_URL_DEFAULT} \\
    [--skip-program-register]  don't POST to /admin/operators \\
    [--skip-onboard]         don't mint IAM creds (governance-only admit) \\
    [--skip-kyc-check]       don't query the KYC applications table
                               (use for friendly-first-operator
                                dogfood; foundation engineer has
                                already verified out-of-band)

Submits an Intent::AdmitAuthority for the candidate, waits for it to
land, then runs onboard-operator.sh and registers them with the
points daemon. Idempotent in spirit: if the authority is already
admitted, skips the submit step and proceeds.
EOF
    exit 1
}

AUTHORITY_ID=""
LABEL=""
MLDSA_PK_HEX=""
BLS_PK_HEX=""
STAKE_SUWAPPU="$DEFAULT_STAKE_SUWAPPU"
RPC_URL="$RPC_URL_DEFAULT"
PROGRAM_URL="$PROGRAM_URL_DEFAULT"
SKIP_PROGRAM_REGISTER="0"
SKIP_ONBOARD="0"
SKIP_KYC_CHECK="0"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --authority-id)         AUTHORITY_ID="$2"; shift 2 ;;
        --label)                LABEL="$2"; shift 2 ;;
        --mldsa-pk-hex)         MLDSA_PK_HEX="$2"; shift 2 ;;
        --bls-pk-hex)           BLS_PK_HEX="$2"; shift 2 ;;
        --stake-suwappu)            STAKE_SUWAPPU="$2"; shift 2 ;;
        --rpc-url)              RPC_URL="$2"; shift 2 ;;
        --program-url)          PROGRAM_URL="$2"; shift 2 ;;
        --skip-program-register) SKIP_PROGRAM_REGISTER="1"; shift ;;
        --skip-onboard)         SKIP_ONBOARD="1"; shift ;;
        --skip-kyc-check)       SKIP_KYC_CHECK="1"; shift ;;
        -h|--help)              usage ;;
        *) echo "unknown arg: $1" >&2; usage ;;
    esac
done

for f in AUTHORITY_ID LABEL MLDSA_PK_HEX BLS_PK_HEX; do
    if [[ -z "${!f}" ]]; then
        echo "error: --$(echo "$f" | tr '[:upper:]_' '[:lower:]-') is required" >&2
        usage
    fi
done
if ! [[ "$AUTHORITY_ID" =~ ^[0-9]+$ ]] || (( AUTHORITY_ID < 8 )); then
    echo "error: --authority-id must be ≥ 8 (0..6 are seeds, 7 is the faucet)" >&2
    exit 1
fi

# Validate the candidate fields BEFORE any AWS call or the on-chain
# AdmitAuthority submit. AdmitAuthority is irreversible: a malformed
# label or pubkey that slips through lands in the registry and only
# fails later in onboard-operator.sh (whose label regex is
# `^[a-z0-9][a-z0-9-]*$`, line 44), leaving the operator half-admitted.
# Reject early instead. (Codex #228 P2.)
if ! [[ "$LABEL" =~ ^[a-z0-9][a-z0-9-]*$ ]]; then
    echo "error: --label must match ^[a-z0-9][a-z0-9-]*\$ (lowercase alnum + hyphens, no leading hyphen) — got '${LABEL}'" >&2
    exit 1
fi
# ML-DSA-65 public key: 1952 bytes = 3904 hex chars. BLS12-381 G1
# public key (min-pubkey-size): 48 bytes = 96 hex chars. Accept an
# optional 0x prefix; require exact length + hex alphabet so the Rust
# admit binary doesn't fail mid-submit on a truncated paste.
MLDSA_PK_NOPREFIX="${MLDSA_PK_HEX#0x}"
BLS_PK_NOPREFIX="${BLS_PK_HEX#0x}"
if ! [[ "$MLDSA_PK_NOPREFIX" =~ ^[0-9a-fA-F]{3904}$ ]]; then
    echo "error: --mldsa-pk-hex must be exactly 3904 hex chars (1952-byte ML-DSA-65 pubkey); got ${#MLDSA_PK_NOPREFIX}" >&2
    exit 1
fi
if ! [[ "$BLS_PK_NOPREFIX" =~ ^[0-9a-fA-F]{96}$ ]]; then
    echo "error: --bls-pk-hex must be exactly 96 hex chars (48-byte BLS12-381 pubkey); got ${#BLS_PK_NOPREFIX}" >&2
    exit 1
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

# KYC precheck. The candidate-pubkey-hash field in the
# applications table is keyed by blake3(mldsa_pk)[:32]; we compute
# the same here and assert Persona returned `status = approved`
# (or the row is missing in dogfood mode with --skip-kyc-check).
if [[ "$SKIP_KYC_CHECK" == "1" ]]; then
    echo "[admit-operator] --skip-kyc-check set — bypassing applications table query"
else
    # Compute blake3 of the candidate's ML-DSA pubkey. The Lambda
    # writes lowercase hex without 0x prefix; match that format.
    MLDSA_PK_BIN="$TMPDIR/candidate.pk.bin"
    # Decode the hex pubkey to bytes. Prefer `xxd` (in vim-common,
    # not in the documented prereq list); fall back to python3
    # (which is also our b3sum fallback below). Either path
    # succeeds on the foundation operator hosts; failing both
    # produces a clear error instead of `xxd: command not found`
    # under `set -euo pipefail`. (Codex #228 P2.)
    if command -v xxd >/dev/null 2>&1; then
        echo -n "${MLDSA_PK_HEX#0x}" | xxd -r -p > "$MLDSA_PK_BIN"
    elif command -v python3 >/dev/null 2>&1; then
        python3 -c "import sys; h=sys.argv[1]; h=h[2:] if h.lower().startswith('0x') else h; open(sys.argv[2],'wb').write(bytes.fromhex(h))" "$MLDSA_PK_HEX" "$MLDSA_PK_BIN"
    else
        echo "error: neither xxd nor python3 available — install one or pass --skip-kyc-check" >&2
        exit 1
    fi
    # b3sum if present; fall back to python blake3 module.
    if command -v b3sum >/dev/null 2>&1; then
        CANDIDATE_PKH=$(b3sum --no-names "$MLDSA_PK_BIN" | head -c 64)
    elif python3 -c "import blake3" 2>/dev/null; then
        CANDIDATE_PKH=$(python3 -c "import blake3, sys; print(blake3.blake3(open(sys.argv[1],'rb').read()).hexdigest())" "$MLDSA_PK_BIN")
    else
        echo "error: neither b3sum nor Python blake3 module available — install one or pass --skip-kyc-check" >&2
        exit 1
    fi
    echo "[admit-operator] candidate pubkey hash = 0x${CANDIDATE_PKH}"
    KYC_ROW=$(AWS_PROFILE=gsn aws dynamodb get-item --region us-east-1 \
        --table-name "$KYC_APPLICATIONS_TABLE" \
        --key "{\"candidate_pubkey_hash\":{\"S\":\"${CANDIDATE_PKH}\"}}" \
        --query 'Item' --output json 2>/tmp/kyc.err) || {
        echo "error: failed to query KYC table $KYC_APPLICATIONS_TABLE — pass --skip-kyc-check to bypass" >&2
        cat /tmp/kyc.err >&2
        exit 1
    }
    if [[ "$KYC_ROW" == "null" || -z "$KYC_ROW" ]]; then
        echo "error: no KYC row found for candidate pubkey hash 0x${CANDIDATE_PKH}" >&2
        echo "  Applicant must complete the Persona inquiry at apply.${RPC_URL##*://rpc.}/" >&2
        echo "  Use --skip-kyc-check for the friendly-first-operator dogfood." >&2
        exit 1
    fi
    KYC_STATUS=$(echo "$KYC_ROW" | jq -r '.status.S // empty')
    if [[ "$KYC_STATUS" != "approved" ]]; then
        echo "error: KYC status is '${KYC_STATUS:-unknown}', not 'approved' — refusing to admit" >&2
        echo "  Inquiry id: $(echo "$KYC_ROW" | jq -r '.inquiry_id.S // empty')" >&2
        echo "  Either wait for Persona approval / human review, or pass --skip-kyc-check to bypass." >&2
        exit 1
    fi
    echo "[admit-operator] KYC: approved (inquiry $(echo "$KYC_ROW" | jq -r '.inquiry_id.S // empty'))"
fi

# Track whether THIS run actually submitted the AdmitAuthority
# Intent. The onboarding path below runs `aws iam create-user`,
# which exits non-zero on existing users — so re-executing the
# whole script against an already-admitted operator would otherwise
# fail at the IAM step before reaching program registration.
# (Codex #228 P2 — admit-operator.sh:228.)
NEWLY_ADMITTED=0

# Check if already admitted — short-circuit if same operator, hard error
# if a DIFFERENT pubkey already sits in this slot. Without the pubkey
# match, a typo in --authority-id or a re-used id from another operator
# would silently mint S3-upload creds + points-program rows under the
# wrong authority's prefix, allowing one operator to overwrite another
# operator's points data. (Codex #228 P1 — admit-operator.sh:168.)
echo "[admit-operator] checking registry for authority_id=${AUTHORITY_ID} (rpc=${RPC_URL})"
REGISTRY_JSON=$(curl -fsS --max-time 10 -X POST -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"suwappu_getAuthorityRegistry"}' \
    "$RPC_URL")
EXISTING_MEMBER_JSON=$(echo "$REGISTRY_JSON" | jq -c --argjson aid "$AUTHORITY_ID" '.result[] | select(.id == $aid)' 2>/dev/null || true)
if [[ -n "$EXISTING_MEMBER_JSON" ]]; then
    EXISTING_PK_HEX=$(echo "$EXISTING_MEMBER_JSON" | jq -r '.public_key_hex // .mldsa_public_key_hex // empty' | tr '[:upper:]' '[:lower:]')
    CANDIDATE_PK_HEX_LC=$(echo "${MLDSA_PK_HEX#0x}" | tr '[:upper:]' '[:lower:]')
    if [[ -z "$EXISTING_PK_HEX" ]]; then
        echo "error: registry member for authority_id=${AUTHORITY_ID} exists but exposes no public_key_hex — refusing to short-circuit (cannot verify operator identity)" >&2
        echo "  Registry member: $EXISTING_MEMBER_JSON" >&2
        exit 1
    fi
    if [[ "$EXISTING_PK_HEX" != "$CANDIDATE_PK_HEX_LC" ]]; then
        echo "error: authority_id=${AUTHORITY_ID} is already seated under a DIFFERENT public key." >&2
        echo "  Seated pubkey:    $EXISTING_PK_HEX" >&2
        echo "  Candidate pubkey: $CANDIDATE_PK_HEX_LC" >&2
        echo "  Refusing to mint S3-upload credentials + points-program row for the wrong operator." >&2
        echo "  Either fix --authority-id, or use a new free slot (≥ 8)." >&2
        exit 1
    fi
    echo "[admit-operator] authority_id=${AUTHORITY_ID} already in registry with matching pubkey; skipping AdmitAuthority submit"
else
    NEWLY_ADMITTED=1
    # Pull the foundation signer keypair from Secrets Manager.
    echo "[admit-operator] pulling foundation signer keypair from Secrets Manager (${FAUCET_SECRET_ID})"
    SIGNER_SK="$TMPDIR/signer.sk"
    SIGNER_PK="$TMPDIR/signer.pk"
    AWS_PROFILE=gsn aws secretsmanager get-secret-value --region us-east-1 \
        --secret-id "$FAUCET_SECRET_ID" --query SecretBinary --output text \
        | base64 -d > "$SIGNER_SK"
    chmod 600 "$SIGNER_SK"
    # The matching pubkey lives in S3 (uploaded during bringup).
    AWS_PROFILE=gsn aws s3 cp "s3://suwappu-dag-testnet-artifacts/keys/faucet/mldsa.pk" "$SIGNER_PK" >/dev/null

    # Submit via the Rust example binary.
    echo "[admit-operator] building examples/rust/admit_authority (release)"
    (
        cd "$REPO_ROOT/examples/rust"
        env -u GH_TOKEN -u GITHUB_TOKEN \
            CARGO_NET_GIT_FETCH_WITH_CLI=true \
            cargo build --release --bin admit_authority >&2
    )
    ADMIT_BIN="$REPO_ROOT/examples/rust/target/release/admit_authority"

    echo "[admit-operator] submitting Intent::AdmitAuthority"
    SUBMIT_OUT=$("$ADMIT_BIN" \
        --rpc-url "$RPC_URL" \
        --network-id "$NETWORK_ID" \
        --signer-sk "$SIGNER_SK" \
        --signer-pk "$SIGNER_PK" \
        --authority-id "$AUTHORITY_ID" \
        --stake-suwappu "$STAKE_SUWAPPU" \
        --candidate-mldsa-pk-hex "$MLDSA_PK_HEX" \
        --candidate-bls-pk-hex "$BLS_PK_HEX")
    TX_HASH=$(echo "$SUBMIT_OUT" | jq -r '.tx_hash')
    echo "[admit-operator] submitted: tx_hash=${TX_HASH}; waiting for registry confirmation"
fi

if [[ "$SKIP_ONBOARD" != "1" ]]; then
    if [[ "$NEWLY_ADMITTED" == "1" ]]; then
        echo "[admit-operator] running onboard-operator.sh"
        "$REPO_ROOT/scripts/testnet/onboard-operator.sh" "$AUTHORITY_ID" "$LABEL"
    else
        # `onboard-operator.sh` calls `aws iam create-user`, which exits
        # non-zero on existing users. Skip it on the already-admitted
        # short-circuit so re-running the full script against an
        # already-onboarded operator doesn't fail before reaching the
        # program-registration step below. If onboarding partially
        # failed on a prior run and you need to re-attempt JUST that
        # step, invoke `onboard-operator.sh` directly. (Codex #228 P2.)
        echo "[admit-operator] already-admitted short-circuit; skipping onboard-operator.sh (re-run directly if needed)"
    fi
fi

if [[ "$SKIP_PROGRAM_REGISTER" != "1" ]]; then
    echo "[admit-operator] registering with points-accumulator daemon"
    PROGRAM_ADMIN_TOKEN=$(AWS_PROFILE=gsn aws secretsmanager get-secret-value --region us-east-1 \
        --secret-id "$PROGRAM_ADMIN_SECRET_ID" --query SecretString --output text)
    HTTP_CODE=$(curl -sS -o "$TMPDIR/program.out" -w '%{http_code}' \
        -X POST -H "Authorization: Bearer ${PROGRAM_ADMIN_TOKEN}" \
        -H 'Content-Type: application/json' \
        -d "{\"authority_id\":${AUTHORITY_ID},\"label\":\"${LABEL}\",\"is_seed\":false}" \
        "${PROGRAM_URL}/admin/operators" || true)
    if [[ "$HTTP_CODE" != "200" && "$HTTP_CODE" != "201" && "$HTTP_CODE" != "204" ]]; then
        echo "warning: program registration returned HTTP ${HTTP_CODE}; check ${PROGRAM_URL}" >&2
        cat "$TMPDIR/program.out" >&2 || true
    else
        echo "[admit-operator] program registration ok (HTTP ${HTTP_CODE})"
    fi
fi

cat <<EOF

================================================================
Admit + onboard complete: authority_id=${AUTHORITY_ID} label=${LABEL}
================================================================
The operator now has:
  - An Authority Ring slot (verified via suwappu_getAuthorityRegistry).
  - IAM credentials scoped to s3://suwappu-dag-testnet-validator-uploads/uploads/${AUTHORITY_ID}/
  - A row in the validator-program daemon's operators table.

Send the operator the credentials block printed by
onboard-operator.sh (above) PLUS the public URLs in
docs/testnet/VALIDATOR-OPERATORS.md.

If the operator subsequently exits via ExitAuthority, run:
  scripts/testnet/offboard-operator.sh ${LABEL}
to revoke the IAM credentials.
================================================================
EOF
