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

Real ML-DSA-65 + BLS12-381 keys would normally be produced by gsx-crypto
itself. Since this script is host-side and must work without compiling the
Rust crate, we use an external tool (gsx-keygen) if available, otherwise we
fall back to deterministic placeholder bytes — *only* acceptable for a
private perf testnet where the cluster isn't exposed to the public.

For production / mainnet, never use the placeholder branch. The genesis
manifest validator id <-> public key binding is load-bearing.
"""

from __future__ import annotations

import argparse
import hashlib
import os
import sys
from pathlib import Path

REGIONS = [
    ("us-east-1", 0),
    ("us-west-2", 1),
    ("eu-west-1", 2),
    ("ap-northeast-1", 3),
    ("ap-southeast-2", 4),
    ("sa-east-1", 5),
    # ("af-south-1", 6),  # disabled: requires AWS account region opt-in.
]


def placeholder_key(seed: bytes, length: int) -> bytes:
    """Deterministic byte stream from a seed. Not cryptographically random.
    Acceptable only for the closed perf testnet."""
    out = b""
    counter = 0
    while len(out) < length:
        out += hashlib.blake2b(seed + counter.to_bytes(4, "big")).digest()
        counter += 1
    return out[:length]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out-dir", type=Path, default=Path("./target/perf/keys"))
    ap.add_argument("--network-id", default="gsx-perf-7r")
    ap.add_argument(
        "--validator-stake-gsx",
        type=int,
        default=1_000_000,
        help="Per-validator stake in GSX (paper Definition 1).",
    )
    ap.add_argument(
        "--authority-stake-gsx",
        type=int,
        default=1_000_000,
        help="Per-validator Authority Ring stake in GSX.",
    )
    args = ap.parse_args()

    args.out_dir.mkdir(parents=True, exist_ok=True)

    # ML-DSA-65 secret key is 4032 bytes; BLS12-381 secret scalar is 32 bytes.
    # We write the placeholder bytes for now; replace with real keys produced
    # by `cargo run --bin gsx-keygen` once that binary lands.
    MLDSA_SK_LEN = 4032
    BLS_SK_LEN = 32

    entries = []
    for region, aid in REGIONS:
        region_dir = args.out_dir / region
        region_dir.mkdir(parents=True, exist_ok=True)

        seed_mldsa = f"{args.network_id}-{region}-mldsa".encode()
        seed_bls = f"{args.network_id}-{region}-bls".encode()
        mldsa_sk = placeholder_key(seed_mldsa, MLDSA_SK_LEN)
        bls_sk = placeholder_key(seed_bls, BLS_SK_LEN)

        (region_dir / "mldsa.sk").write_bytes(mldsa_sk)
        (region_dir / "bls.sk").write_bytes(bls_sk)
        os.chmod(region_dir / "mldsa.sk", 0o600)
        os.chmod(region_dir / "bls.sk", 0o600)

        # Public key derivation requires the real crypto. For the perf
        # testnet, we just expose a hash of the secret key as a "public key"
        # surrogate — the daemon doesn't currently verify ML-DSA on the wire,
        # so this is a placeholder that satisfies the manifest schema.
        mldsa_pk = hashlib.blake2b(mldsa_sk, digest_size=32).hexdigest()
        bls_pk = hashlib.blake2b(bls_sk, digest_size=48).hexdigest()

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
            f.write(f"validator_stake_gsx = {args.validator_stake_gsx}\n")
            f.write(f"authority_stake_gsx = {args.authority_stake_gsx}\n\n")

    print(f"wrote {genesis}", file=sys.stderr)
    for aid, region, _, _ in entries:
        print(f"  validators[{aid}] = {region}", file=sys.stderr)
    print(
        "NOTE: placeholder keys — closed perf testnet only. Do not reuse for mainnet.",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
