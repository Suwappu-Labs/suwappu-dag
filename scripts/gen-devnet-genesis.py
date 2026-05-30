#!/usr/bin/env python3
"""
Generate a local-devnet genesis manifest + per-validator config and
key files. Output is consumed by `docker-compose.yml` + the
`gsx-node` binary's `--config` flag.

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

Keys: validator-side keys are deterministic placeholder bytes — the
validator-to-validator wire doesn't verify ML-DSA today (paper §3.3
exception). The faucet authority is different: its ML-DSA-65 keypair
MUST be real because the daemon's `verify_signed_intent` gate checks
the signature on every drip. This script invokes `gsx-keygen` (built
from `crates/gsx-crypto/src/bin/gsx-keygen.rs`) for that one keypair
and writes a `[[prebalances]]` entry funding the faucet's address.

**Acceptable only for a LOCAL devnet that never accepts external
traffic.** For any public testnet / mainnet, use the
`scripts/devnet/gen-genesis.py` or `scripts/testnet/gen-genesis.py`
paths.

Usage:
    ./scripts/gen-devnet-genesis.py --num-nodes 4 --out-dir target/devnet

Requires `gsx-keygen` on PATH:
    cargo build --release -p gsx-crypto --bin gsx-keygen
    export PATH="$PWD/target/release:$PATH"
"""

from __future__ import annotations

import argparse
import hashlib
import shutil
import subprocess
import sys
from pathlib import Path

DEFAULT_NETWORK_ID = "gsx-devnet-local"
FAUCET_LABEL = "faucet"


def placeholder_key(seed: bytes, length: int) -> bytes:
    """Deterministic byte stream from a seed. Not cryptographically
    random — devnet ONLY."""
    out = b""
    counter = 0
    while len(out) < length:
        out += hashlib.blake2b(seed + counter.to_bytes(4, "big")).digest()
        counter += 1
    return out[:length]


# Real key sizes:
#   ML-DSA-65 public key: 1,952 bytes (FIPS 204).
#   ML-DSA-65 secret key: 4,032 bytes.
#   BLS12-381 G1 public key: 48 bytes.
#   BLS12-381 secret key: 32 bytes.
ML_DSA_PK_BYTES = 1952
ML_DSA_SK_BYTES = 4032
BLS_PK_BYTES = 48
BLS_SK_BYTES = 32


def mint_faucet_keypair(out_dir: Path) -> tuple[str, str]:
    """Invoke `gsx-keygen` to mint a real ML-DSA-65 faucet keypair.

    Returns `(faucet_pk_hex, faucet_address_hex)` where the address is
    the canonical `blake3(pk)[:20]` recipe used by
    `gsx_faucet::address_from_pubkey` at runtime.
    """
    if shutil.which("gsx-keygen") is None:
        print(
            "error: gsx-keygen not found on PATH. Build it first:\n"
            "    cargo build --release -p gsx-crypto --bin gsx-keygen\n"
            "    export PATH=\"$PWD/target/release:$PATH\"",
            file=sys.stderr,
        )
        sys.exit(2)

    faucet_dir = out_dir / "faucet"
    faucet_dir.mkdir(parents=True, exist_ok=True)
    sk_path = faucet_dir / "mldsa.sk"
    pk_path = faucet_dir / "mldsa.pk"
    addr_path = faucet_dir / "address.hex"

    subprocess.run(
        [
            "gsx-keygen",
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
        default="gsx-devnet-2026",
        help="Deterministic seed for placeholder keys.",
    )
    ap.add_argument(
        "--validator-stake-gsx",
        type=int,
        default=150_000,
        help="Per-validator stake (default 150_000 — above AUTHORITY_STAKE_THRESHOLD_GSX).",
    )
    ap.add_argument(
        "--faucet-initial-balance-gsx",
        type=int,
        default=1_000_000_000,
        help="Genesis-time balance of the faucet address (1 billion GSX by default — "
             "enough for ~10 million drips at 100 GSX/drip).",
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

    validators = []
    for i in range(args.num_nodes):
        label = f"v{i}"
        node_dir = out / label
        node_dir.mkdir(parents=True, exist_ok=True)

        seed_root = f"{args.seed}-{label}".encode()
        mldsa_sk = placeholder_key(seed_root + b"-mldsa-sk", ML_DSA_SK_BYTES)
        mldsa_pk = placeholder_key(seed_root + b"-mldsa-pk", ML_DSA_PK_BYTES)
        bls_sk = placeholder_key(seed_root + b"-bls-sk", BLS_SK_BYTES)
        bls_pk = placeholder_key(seed_root + b"-bls-pk", BLS_PK_BYTES)

        (node_dir / "mldsa.sk").write_bytes(mldsa_sk)
        (node_dir / "bls.sk").write_bytes(bls_sk)

        validators.append(
            {
                "authority_id": i,
                "label": label,
                "mldsa_public_key_hex": mldsa_pk.hex(),
                "bls_public_key_hex": bls_pk.hex(),
                "validator_stake_gsx": args.validator_stake_gsx,
                "authority_stake_gsx": args.validator_stake_gsx,
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
                f'validator_stake_gsx = {v["validator_stake_gsx"]}',
                f'authority_stake_gsx = {v["authority_stake_gsx"]}',
                "",
            ]
        )

    # Faucet signer — seated ONLY in the AuthorityRegistry via a [[signers]]
    # entry, NOT as a consensus validator. Seating the faucet as a
    # [[validators]] entry inflates committee size n (= validators.len()) and
    # the quorum thresholds; since the faucet runs no node it never proposes
    # or votes, so finalization starves. As a signer it still resolves
    # `verify_signed_intent`'s pubkey hash. stake_gsx must clear
    # AUTHORITY_STAKE_THRESHOLD_GSX (100,000) or `admit` silently drops it and
    # drips hit UnknownSigner.
    lines.extend(
        [
            "[[signers]]",
            f"authority_id = {faucet_authority_id}",
            f'label = "{FAUCET_LABEL}"',
            f'mldsa_public_key_hex = "{faucet_pk_hex}"',
            "stake_gsx = 100000",
            "",
        ]
    )

    # Pre-balance the faucet so drips have something to spend. Loaded by
    # `gsx_node::config::GenesisManifest::prebalances` at startup.
    lines.extend(
        [
            "[[prebalances]]",
            f'address = "{faucet_addr_hex}"',
            f"balance_gsx = {args.faucet_initial_balance_gsx}",
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
    print(f"  faucet initial balance: {args.faucet_initial_balance_gsx:,} GSX")
    print()
    print("WARNING: validator keys are placeholders — devnet ONLY. The faucet key")
    print("is real but never reuse it for any non-devnet deployment.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
