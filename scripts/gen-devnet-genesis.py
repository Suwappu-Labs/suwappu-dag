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

The per-validator `node.toml` is filled in by `scripts/devnet-local.sh`
(or `docker-compose.yml`) at startup since the peer-list IPs depend on
the deployment target.

Keys: this script writes deterministic placeholder bytes derived from
a seed. **Acceptable only for a LOCAL devnet that never accepts
external traffic.** For any public testnet / mainnet, use the
real `suwappu-crypto` keygen path.

Usage:
    ./scripts/gen-devnet-genesis.py --num-nodes 4 --out-dir target/devnet
"""

from __future__ import annotations

import argparse
import hashlib
import sys
from pathlib import Path

DEFAULT_NETWORK_ID = "suwappu-devnet-local"


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


def main() -> int:
    ap = argparse.ArgumentParser(description="Generate a local devnet genesis.")
    ap.add_argument(
        "--num-nodes",
        type=int,
        default=4,
        help="Number of validators (default 4 — DagBft n=4 minimum).",
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
        help="Deterministic seed for placeholder keys.",
    )
    ap.add_argument(
        "--validator-stake-suwappu",
        type=int,
        default=150_000,
        help="Per-validator stake (default 150_000 — above AUTHORITY_STAKE_THRESHOLD_SUWAPPU).",
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
            "warning: <4 nodes selected; DagBft-C BFT needs n=3f+1 ≥ 4 for liveness",
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
                "validator_stake_suwappu": args.validator_stake_suwappu,
                "authority_stake_suwappu": args.validator_stake_suwappu,
            }
        )

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
    (out / "genesis.toml").write_text("\n".join(lines))

    print(f"devnet genesis written to {out}/")
    print(f"  validators: {args.num_nodes}")
    print(f"  network_id: {args.network_id}")
    print(f"  per-node keys: {out}/v{{0..{args.num_nodes - 1}}}/{{mldsa,bls}}.sk")
    print()
    print("WARNING: placeholder keys — devnet ONLY. Never expose to the public.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
