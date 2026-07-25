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

Keys: real ML-DSA-65 + BLS12-381 keypairs minted via `suwappu-keygen`
(build with `cargo build --release -p suwappu-crypto --bin
suwappu-keygen`). This matters beyond "nice to have": any client
(e.g. `suwappu-loadgen`) submitting signed intents through
`verify_signed_intent` must sign with a keypair that verifies against
one of these genesis-registered public keys, so non-corresponding
placeholder sk/pk pairs make the devnet unable to accept any signed
traffic at all — confirmed empirically (fake-keypair sign+verify
round trip fails, see suwappu-dag PR for the TPS benchmark harness).
Falls back to deterministic (non-cryptographic) placeholder bytes with
a loud warning if `suwappu-keygen` isn't on PATH — that fallback mode
can start a cluster but cannot accept signed intents.

Usage:
    ./scripts/gen-devnet-genesis.py --num-nodes 4 --out-dir target/devnet
"""

from __future__ import annotations

import argparse
import hashlib
import os
import shutil
import subprocess
import sys
from pathlib import Path

DEFAULT_NETWORK_ID = "suwappu-devnet-local"


def placeholder_key(seed: bytes, length: int) -> bytes:
    """Deterministic byte stream from a seed. Not cryptographically
    random — only used as a fallback when suwappu-keygen isn't on PATH."""
    out = b""
    counter = 0
    while len(out) < length:
        out += hashlib.blake2b(seed + counter.to_bytes(4, "big")).digest()
        counter += 1
    return out[:length]


def mint_keypair(sk_path: Path, pk_path: Path, algo: str, seed_root: bytes) -> tuple[bytes, bytes]:
    """Real keypair via suwappu-keygen (--algo mldsa|bls). Falls back to a
    placeholder + loud warning if suwappu-keygen isn't on PATH."""
    sk_path.parent.mkdir(parents=True, exist_ok=True)

    if shutil.which("suwappu-keygen") is not None:
        subprocess.run(
            ["suwappu-keygen", "--algo", algo, "--sk", str(sk_path), "--pk", str(pk_path)],
            check=True,
        )
        os.chmod(sk_path, 0o600)
        return sk_path.read_bytes(), pk_path.read_bytes()

    print(
        f"WARNING: suwappu-keygen not found on PATH; emitting non-functional "
        f"placeholder {algo} key for {sk_path.parent.name} (signed intents from "
        "this validator's keypair will NOT verify). Build suwappu-keygen with: "
        "  cargo build --release -p suwappu-crypto --bin suwappu-keygen",
        file=sys.stderr,
    )
    key_len = ML_DSA_SK_BYTES if algo == "mldsa" else BLS_SK_BYTES
    pk_len = ML_DSA_PK_BYTES if algo == "mldsa" else BLS_PK_BYTES
    sk = placeholder_key(seed_root + f"-{algo}-sk".encode(), key_len)
    pk = placeholder_key(seed_root + f"-{algo}-pk".encode(), pk_len)
    sk_path.write_bytes(sk)
    pk_path.write_bytes(pk)
    os.chmod(sk_path, 0o600)
    return sk, pk


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
        _mldsa_sk, mldsa_pk = mint_keypair(
            node_dir / "mldsa.sk", node_dir / "mldsa.pk", "mldsa", seed_root
        )
        _bls_sk, bls_pk = mint_keypair(
            node_dir / "bls.sk", node_dir / "bls.pk", "bls", seed_root
        )

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
    if shutil.which("suwappu-keygen") is not None:
        print("Real ML-DSA-65 + BLS12-381 keypairs (via suwappu-keygen). Devnet-only: never expose to the public.")
    else:
        print("WARNING: suwappu-keygen not on PATH — placeholder keys emitted; signed intents will NOT verify.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
