#!/usr/bin/env bash
# check-crypto-boundary.sh — crypto lane-separation gate.
#
# Referenced by deny.toml ("[bans] … Enforced separately by
# scripts/check-crypto-boundary.sh in CI") and by load-bearing invariant 2
# in CLAUDE.md (PQ-conservative crypto surface, paper §3.3 / §12):
# primitive crypto libraries may only be direct dependencies of the
# audited crypto boundary crates listed below. Everything else must go
# through `suwappu-crypto`'s API so the PQ migration surface stays in
# one reviewed place.
#
# Pure-manifest check: greps [dependencies] sections of every crate
# manifest, so it needs no toolchain, no network, and no access to the
# private suwappu-db dependency. Runs in CI as the `crypto-boundary`
# job in ci.yml.
#
# To add a consumer: extend the allow-list below IN THE SAME PR, with a
# comment citing the paper section or IQ decision that authorizes it.

set -euo pipefail

cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# dep → crates allowed to depend on it directly (space-separated manifest paths).
# Seeded from the audited state of the tree (2026-08-15).
declare -A ALLOWED=(
    # FIPS 204 signatures — crypto boundary + the ML-DSA precompile (paper §9).
    [pqcrypto-mldsa]="crates/suwappu-crypto/Cargo.toml crates/suwappu-mldsa-precompile/Cargo.toml"
    # FIPS 203 KEM — crypto boundary only.
    [pqcrypto-mlkem]="crates/suwappu-crypto/Cargo.toml"
    # BLS12-381 aggregate signatures — documented classical exception zone:
    # the constant-size LTP commitment (paper §10.2) carries a 96 B BLS
    # aggregate, so suwappu-ltp consumes blst alongside the boundary crate.
    [blst]="crates/suwappu-crypto/Cargo.toml crates/suwappu-ltp/Cargo.toml"
    # SHA3-256 — payload roots and certificate digests. Consensus hashes
    # certificates directly (hot path); the SP1 zkVM guest/host pair hashes
    # the reserve-coverage payload root (paper §10.2, DAG-S17).
    [sha3]="crates/suwappu-crypto/Cargo.toml crates/suwappu-consensus/Cargo.toml zkvm/reserve-coverage-host/Cargo.toml zkvm/reserve-coverage-verifier/Cargo.toml"
    # HKDF — key derivation, crypto boundary only.
    [hkdf]="crates/suwappu-crypto/Cargo.toml"
)

# Classical / unaudited crypto crates that must never appear outside the
# crypto boundary. Zero legitimate consumers today outside suwappu-crypto.
CLASSICAL_DENY=(
    k256 secp256k1 bls12_381 ed25519-dalek curve25519-dalek x25519-dalek
    ring openssl rsa aes-gcm chacha20poly1305 p256 p384
    ark-bn254 ark-groth16 ark-ec ark-ff
)

MANIFESTS=$(ls crates/*/Cargo.toml clients/rust-sdk/Cargo.toml zkvm/*/Cargo.toml fuzz/Cargo.toml 2>/dev/null)

fail=0

dep_consumers() {
    # Direct-dependency declarations only: "<dep> =" or "<dep>.workspace"
    # at line start inside a manifest.
    local dep="$1"
    grep -l -E "^${dep}[[:space:]]*[.=]" ${MANIFESTS} 2>/dev/null || true
}

for dep in "${!ALLOWED[@]}"; do
    allowed=" ${ALLOWED[$dep]} "
    for manifest in $(dep_consumers "$dep"); do
        if [[ "$allowed" != *" $manifest "* ]]; then
            echo "VIOLATION: $manifest declares direct dependency on '$dep'" >&2
            echo "  '$dep' is restricted to: ${ALLOWED[$dep]}" >&2
            echo "  Route through suwappu-crypto, or extend the allow-list with a citation." >&2
            fail=1
        fi
    done
done

for dep in "${CLASSICAL_DENY[@]}"; do
    for manifest in $(dep_consumers "$dep"); do
        if [[ "$manifest" != "crates/suwappu-crypto/Cargo.toml" ]]; then
            echo "VIOLATION: $manifest declares classical/unaudited crypto crate '$dep'" >&2
            echo "  Classical primitives live only in suwappu-crypto's documented exception zones (paper §3.3)." >&2
            fail=1
        fi
    done
done

if [[ "$fail" -ne 0 ]]; then
    echo "" >&2
    echo "check-crypto-boundary: FAILED" >&2
    exit 1
fi

echo "check-crypto-boundary: OK (lane separation holds across $(echo "$MANIFESTS" | wc -w) manifests)"
