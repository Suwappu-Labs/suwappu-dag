#!/usr/bin/env bash
#
# run_all.sh — machine-checked invariant obligations (IQ-014, Phase 1).
#
# Runs the Kani bounded-model-checking harnesses over the two load-bearing
# invariants:
#
#   * suwappu-consensus/src/joint.rs   — joint-quorum AND-gate (Theorem 2),
#                                        Validator-leg quorum predicates.
#   * suwappu-ltp/src/attestation.rs   — LTP 7-of-9 quorum predicate
#                                        (Invariant 3, obligation (a) only).
#
# What is and is not discharged — including the (a) protocol-logic vs
# (b) crypto-hardness split and the bounds Kani runs under — is documented
# in docs/audit/formal-verification.md. Read that before citing this.
#
# Requires cargo-kani (https://model-checking.github.io/kani/install-guide.html):
#     cargo install --locked kani-verifier
#     cargo kani setup
#
# Usage:  scripts/verify/run_all.sh
set -euo pipefail

cd "$(dirname "$0")/../.."

if ! cargo kani --version >/dev/null 2>&1; then
  echo "error: cargo-kani not found. Install with:" >&2
  echo "    cargo install --locked kani-verifier && cargo kani setup" >&2
  exit 127
fi

CRATES=(suwappu-consensus suwappu-ltp)

echo "== IQ-014 machine-checked obligations (Kani) =="
for crate in "${CRATES[@]}"; do
  echo
  echo "--> $crate"
  cargo kani -p "$crate"
done

echo
echo "== all obligations discharged =="
