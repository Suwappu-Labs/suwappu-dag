#!/usr/bin/env python3
"""
Generate a local-devnet genesis manifest + per-validator config and
key files. Output is consumed by `docker-compose.yml` + the
`suwappu-node` binary's `--config` flag.

Unlike the perf-testnet `gen-genesis.py` (which is keyed on AWS
regions), this script is parameterized only on validator count and
emits a flat directory tree:

    <out-dir>/
        genesis.toml
        v0/
            node.toml
            mldsa.sk
            bls.sk
        v1/
            ...
        faucet/
            mldsa.sk       <-- real ML-DSA-65 key (the faucet binary loads this)
            mldsa.pk       <-- matching public key
            address.hex    <-- 0x<20-byte hex> derived via blake3(pk)[:20]

Keys: validator-side keys are REAL keypairs minted by `suwappu-keygen`.
The daemon verifies every certificate's ML-DSA-65 signature against
the seated genesis pubkey on ingest (hardened in #267), so a
validator's `mldsa.sk` MUST be the secret half of its
`mldsa_public_key_hex` in genesis — otherwise every peer rejects its
round-0 certs with "certificate signature invalid" and the chain
wedges at round 0, never reaching quorum. A real BLS12-381 keypair is
minted per validator in the same pass for the quorum-cert / checkpoint
co-signature path. The faucet authority is likewise real because the
daemon's `verify_signed_intent` gate checks the signature on every
drip. This script invokes `suwappu-keygen` (built from
`crates/suwappu-crypto/src/bin/suwappu-keygen.rs`) for all of these keypairs
and writes a `[[prebalances]]` entry funding the faucet's address.

**Acceptable only for a LOCAL devnet that never accepts external
traffic.** For any public testnet / mainnet, use the
`scripts/devnet/gen-genesis.py` or `scripts/testnet/gen-genesis.py`
paths.

Usage:
    ./scripts/gen-devnet-genesis.py --num-nodes 4 --out-dir target/devnet

Requires `suwappu-keygen` on PATH:
    cargo build --release -p suwappu-crypto --bin suwappu-keygen
    export PATH="$PWD/target/release:$PATH"
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
from pathlib import Path

DEFAULT_NETWORK_ID = "suwappu-devnet-local"
FAUCET_LABEL = "faucet"


# Real key sizes, for reference (suwappu-keygen emits exactly these):
#   ML-DSA-65 public key: 1,952 bytes (FIPS 204).
#   ML-DSA-65 secret key: 4,032 bytes.
#   BLS12-381 G1 public key: 48 bytes.
#   BLS12-381 secret key: 32 bytes.


def _require_keygen() -> None:
    if shutil.which("suwappu-keygen") is None:
        print(
            "error: suwappu-keygen not found on PATH. Build it first:\n"
            "    cargo build --release -p suwappu-crypto --bin suwappu-keygen\n"
            "    export PATH=\"$PWD/target/release:$PATH\"",
            file=sys.stderr,
        )
        sys.exit(2)


def mint_keypair(algo: str, sk_path: Path, pk_path: Path) -> str:
    """Invoke `suwappu-keygen` to mint a real `algo` keypair to disk.

    Writes the secret key to `sk_path` and the public key to `pk_path`,
    and returns the public key as a hex string. `algo` is one of
    `"mldsa"` / `"bls"` — the same loader the node uses, so the sk/pk
    pair is guaranteed to round-trip through cert / vote verification.
    """
    sk_path.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [
            "suwappu-keygen",
            "--algo", algo,
            "--sk", str(sk_path),
            "--pk", str(pk_path),
        ],
        check=True,
    )
    return pk_path.read_bytes().hex()


def mint_faucet_keypair(out_dir: Path) -> tuple[str, str]:
    """Invoke `suwappu-keygen` to mint a real ML-DSA-65 faucet keypair.

    Returns `(faucet_pk_hex, faucet_address_hex)` where the address is
    the canonical `blake3(pk)[:20]` recipe used by
    `suwappu_faucet::address_from_pubkey` at runtime.
    """
    faucet_dir = out_dir / "faucet"
    faucet_dir.mkdir(parents=True, exist_ok=True)
    sk_path = faucet_dir / "mldsa.sk"
    pk_path = faucet_dir / "mldsa.pk"
    addr_path = faucet_dir / "address.hex"

    subprocess.run(
        [
            "suwappu-keygen",
            "--algo", "mldsa",
            "--sk", str(sk_path),
            "--pk", str(pk_path),
            "--addr", str(addr_path),
        ],
        check=True,
    )

    faucet_pk_hex = pk_path.read_bytes().hex()
    faucet_addr_hex = addr_path.read_text().strip()
    return faucet_pk_hex, faucet_addr_hex


def main() -> int:
    ap = argparse.ArgumentParser(description="Generate a local devnet genesis.")
    ap.add_argument(
        "--num-nodes",
        type=int,
        default=4,
        help="Number of validators (default 4 — Mysticeti n=4 minimum).",
    )
    ap.add_argument(
        "--out-dir",
        type=Path,
        default=Path("target/devnet"),
        help="Output directory (default ./target/devnet).",
    )
    ap.add_argument(
        "--network-id",
        type=str,
        default=DEFAULT_NETWORK_ID,
        help=f"Network id binding signed intents (default {DEFAULT_NETWORK_ID!r}).",
    )
    ap.add_argument(
        "--seed",
        type=str,
        default="suwappu-devnet-2026",
        help="Accepted for backwards compatibility; ignored. Validator keys "
             "are now minted via suwappu-keygen (no deterministic seeding).",
    )
    ap.add_argument(
        "--validator-stake-suwappu",
        type=int,
        default=150_000,
        help="Per-validator stake (default 150_000 — above AUTHORITY_STAKE_THRESHOLD_SUWAPPU).",
    )
    ap.add_argument(
        "--faucet-initial-balance-suwappu",
        type=int,
        default=1_000_000_000,
        help="Genesis-time balance of the faucet address (1 billion SUWAPPU by default — "
             "enough for ~10 million drips at 100 SUWAPPU/drip).",
    )
    ap.add_argument(
        "--rounds-per-epoch",
        type=int,
        default=1024,
        help="Rounds per epoch for Phase-G governance (default 1024).",
    )
    args = ap.parse_args()

    if args.num_nodes < 4:
        print(
            "warning: <4 nodes selected; Mysticeti-C BFT needs n=3f+1 ≥ 4 for liveness",
            file=sys.stderr,
        )
    if args.num_nodes > 16:
        print(
            "warning: >16 nodes — single-host devnet may run out of ports/CPU",
            file=sys.stderr,
        )

    out = args.out_dir
    out.mkdir(parents=True, exist_ok=True)

    _require_keygen()

    validators = []
    for i in range(args.num_nodes):
        label = f"v{i}"
        node_dir = out / label
        node_dir.mkdir(parents=True, exist_ok=True)

        # Real keypairs — the node signs certs/votes with these secret
        # keys and peers verify against the pubkeys seated below. The pk
        # MUST be the secret key's true public half (suwappu-keygen guarantees
        # this) or every peer rejects the certs and the chain wedges at
        # round 0. suwappu-keygen has no --seed, so these are random per run
        # (the --seed flag now only labels the run, like the faucet key).
        mldsa_pk_hex = mint_keypair(
            "mldsa", node_dir / "mldsa.sk", node_dir / "mldsa.pk"
        )
        bls_pk_hex = mint_keypair(
            "bls", node_dir / "bls.sk", node_dir / "bls.pk"
        )

        validators.append(
            {
                "authority_id": i,
                "label": label,
                "mldsa_public_key_hex": mldsa_pk_hex,
                "bls_public_key_hex": bls_pk_hex,
                "validator_stake_suwappu": args.validator_stake_suwappu,
                "authority_stake_suwappu": args.validator_stake_suwappu,
            }
        )

    # Faucet authority — needs a REAL ML-DSA-65 keypair because every drip
    # passes through `verify_signed_intent`. No BLS key: the faucet is a
    # [[signers]] entry (AuthorityRegistry only), not a consensus validator.
    faucet_pk_hex, faucet_addr_hex = mint_faucet_keypair(out)
    faucet_authority_id = args.num_nodes

    # Render genesis.toml.
    lines = [
        f'network_id = "{args.network_id}"',
        f"rounds_per_epoch = {args.rounds_per_epoch}",
        "",
    ]
    for v in validators:
        lines.extend(
            [
                "[[validators]]",
                f'authority_id = {v["authority_id"]}',
                f'label = "{v["label"]}"',
                f'mldsa_public_key_hex = "{v["mldsa_public_key_hex"]}"',
                f'bls_public_key_hex = "{v["bls_public_key_hex"]}"',
                f'validator_stake_suwappu = {v["validator_stake_suwappu"]}',
                f'authority_stake_suwappu = {v["authority_stake_suwappu"]}',
                "",
            ]
        )

    # Faucet signer — seated ONLY in the AuthorityRegistry via a [[signers]]
    # entry, NOT as a consensus validator. Seating the faucet as a
    # [[validators]] entry inflates committee size n (= validators.len()) and
    # the quorum thresholds; since the faucet runs no node it never proposes
    # or votes, so finalization starves. As a signer it still resolves
    # `verify_signed_intent`'s pubkey hash. stake_suwappu must clear
    # AUTHORITY_STAKE_THRESHOLD_SUWAPPU (100,000) or `admit` silently drops it and
    # drips hit UnknownSigner.
    lines.extend(
        [
            "[[signers]]",
            f"authority_id = {faucet_authority_id}",
            f'label = "{FAUCET_LABEL}"',
            f'mldsa_public_key_hex = "{faucet_pk_hex}"',
            "stake_suwappu = 100000",
            "",
        ]
    )

    # Pre-balance the faucet so drips have something to spend. Loaded by
    # `suwappu_node::config::GenesisManifest::prebalances` at startup.
    lines.extend(
        [
            "[[prebalances]]",
            f'address = "{faucet_addr_hex}"',
            f"balance_suwappu = {args.faucet_initial_balance_suwappu}",
            'role = "faucet"',
            "",
        ]
    )

    (out / "genesis.toml").write_text("\n".join(lines))

    print(f"devnet genesis written to {out}/")
    print(f"  validators: {args.num_nodes}")
    print(f"  network_id: {args.network_id}")
    print(f"  per-node keys: {out}/v{{0..{args.num_nodes - 1}}}/{{mldsa,bls}}.sk")
    print(f"  faucet authority_id: {faucet_authority_id}")
    print(f"  faucet address: {faucet_addr_hex}")
    print(f"  faucet initial balance: {args.faucet_initial_balance_suwappu:,} SUWAPPU")
    print()
    print("WARNING: these are real keypairs but unmanaged on-disk — devnet ONLY.")
    print("Never reuse any of these keys (validators or faucet) for a non-devnet")
    print("deployment.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
