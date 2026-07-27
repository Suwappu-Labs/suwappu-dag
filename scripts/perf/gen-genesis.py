#!/usr/bin/env python3
"""
Generate the 7-validator genesis manifest + per-region ML-DSA / BLS keypairs
for the perf testnet.

Output layout (in --out-dir, default ./target/perf/keys):

    genesis.toml
    us-east-1/mldsa.sk
    us-east-1/bls.sk
    us-west-2/mldsa.sk
    ...

Real ML-DSA-65 + BLS12-381 keys are produced by `suwappu-keygen` (built from
`suwappu-crypto`: `cargo build --release -p suwappu-crypto --bin
suwappu-keygen`). Since this script is host-side and must work without
compiling the Rust crate at generation time, it shells out to that binary if
it's on PATH, otherwise falls back to deterministic placeholder bytes with a
loud warning — that fallback is *only* acceptable for a private perf testnet
where the cluster isn't exposed to the public.

For production / mainnet, never use the placeholder branch. The genesis
manifest validator id <-> public key binding is load-bearing.
"""

from __future__ import annotations

import argparse
import hashlib
import os
import shutil
import subprocess
import sys
from pathlib import Path

REGIONS = [
    ("us-east-1", 0),
    ("us-west-2", 1),
    ("eu-west-1", 2),
    ("ap-northeast-1", 3),
    # ("ap-southeast-2", 4),  # disabled: round driver stalls on >2s RTT.
    # ("sa-east-1", 5),       # disabled: same.
    # ("af-south-1", 6),      # disabled: requires AWS account region opt-in.
]


def placeholder_key(seed: bytes, length: int) -> bytes:
    """Deterministic byte stream from a seed. Not cryptographically random —
    only used as a fallback when suwappu-keygen isn't on PATH."""
    out = b""
    counter = 0
    while len(out) < length:
        out += hashlib.blake2b(seed + counter.to_bytes(4, "big")).digest()
        counter += 1
    return out[:length]


def mint_keypair(sk_path: Path, pk_path: Path, algo: str, warn_label: str) -> tuple[bytes, bytes]:
    """Real keypair via suwappu-keygen (--algo mldsa|bls). Falls back to a
    placeholder + loud warning if suwappu-keygen isn't on PATH."""
    sk_path.parent.mkdir(parents=True, exist_ok=True)

    if shutil.which("suwappu-keygen") is not None:
        subprocess.run(
            [
                "suwappu-keygen", "--algo", algo,
                "--sk", str(sk_path),
                "--pk", str(pk_path),
            ],
            check=True,
        )
        os.chmod(sk_path, 0o600)
        return sk_path.read_bytes(), pk_path.read_bytes()

    print(
        f"WARNING: suwappu-keygen not found on PATH; emitting placeholder "
        f"{warn_label} key. Build suwappu-keygen with: "
        "  cargo build --release -p suwappu-crypto --bin suwappu-keygen",
        file=sys.stderr,
    )
    key_len = 4032 if algo == "mldsa" else 32
    pk_len = 1952 if algo == "mldsa" else 48
    sk = placeholder_key(f"{warn_label}-PLACEHOLDER-SK".encode(), key_len)
    pk = placeholder_key(f"{warn_label}-PLACEHOLDER-PK".encode(), pk_len)
    sk_path.write_bytes(sk)
    pk_path.write_bytes(pk)
    os.chmod(sk_path, 0o600)
    return sk, pk


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out-dir", type=Path, default=Path("./target/perf/keys"))
    ap.add_argument("--network-id", default="suwappu-perf-7r")
    ap.add_argument(
        "--validator-stake-suwappu",
        type=int,
        default=1_000_000,
        help="Per-validator stake in SUWAPPU (paper Definition 1). u64 in the manifest.",
    )
    ap.add_argument(
        "--authority-stake-suwappu",
        type=int,
        default=1_000_000,
        help="Per-validator Authority Ring stake in SUWAPPU. u64 in the manifest.",
    )
    args = ap.parse_args()

    args.out_dir.mkdir(parents=True, exist_ok=True)

    entries = []
    for region, aid in REGIONS:
        region_dir = args.out_dir / region
        region_dir.mkdir(parents=True, exist_ok=True)

        _mldsa_sk, mldsa_pk_bytes = mint_keypair(
            region_dir / "mldsa.sk", region_dir / "mldsa.pk", "mldsa", f"{region}-mldsa-perf"
        )
        _bls_sk, bls_pk_bytes = mint_keypair(
            region_dir / "bls.sk", region_dir / "bls.pk", "bls", f"{region}-bls-perf"
        )
        mldsa_pk = mldsa_pk_bytes.hex()
        bls_pk = bls_pk_bytes.hex()

        entries.append(
            (aid, region, mldsa_pk, bls_pk)
        )

    genesis = args.out_dir / "genesis.toml"
    with genesis.open("w") as f:
        f.write(f'network_id = "{args.network_id}"\n\n')
        for aid, region, mldsa_pk, bls_pk in entries:
            f.write("[[validators]]\n")
            f.write(f"authority_id = {aid}\n")
            f.write(f'label = "{region}"\n')
            f.write(f'mldsa_public_key_hex = "{mldsa_pk}"\n')
            f.write(f'bls_public_key_hex = "{bls_pk}"\n')
            f.write(f"validator_stake_suwappu = {args.validator_stake_suwappu}\n")
            f.write(f"authority_stake_suwappu = {args.authority_stake_suwappu}\n\n")

    print(f"wrote {genesis}", file=sys.stderr)
    for aid, region, _, _ in entries:
        print(f"  validators[{aid}] = {region}", file=sys.stderr)
    print(
        "NOTE: all keys are real ML-DSA-65/BLS12-381 keypairs minted via "
        "suwappu-keygen (unless it wasn't on PATH, in which case per-key "
        "WARNINGs above flag placeholder fallbacks). Do not reuse this "
        "output for mainnet regardless — closed perf testnet only.",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
