#!/usr/bin/env python3
"""
Generate the 7-validator testnet genesis manifest + per-region
ML-DSA / BLS keypairs + a seeded faucet authority.

Forked from scripts/devnet/gen-genesis.py with these diffs:
  * 7 seed validator regions instead of 4 (matches paper §10.2's
    7-of-9 LTP corridor).
  * `network_id = "gsx-testnet-v1"` (devnet is "gsx-devnet").
  * `rounds_per_epoch = 4096` (4× devnet — longer epochs reduce
    governance churn during the 12-month testnet life).
  * Faucet authority_id = 7 (devnet used id=4 with 4 validators).

Output layout (in --out-dir, default ./target/testnet/keys):

    genesis.toml
    us-east-1/mldsa.sk + bls.sk
    us-west-2/...
    eu-west-1/...
    eu-central-1/...
    ap-southeast-1/...
    ap-northeast-1/...
    sa-east-1/...
    faucet/mldsa.sk + mldsa.pk   <-- real ML-DSA-65
    prebalances.toml             <-- faucet's initial GSX balance
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
    ("eu-central-1", 3),
    ("ap-southeast-1", 4),
    ("ap-northeast-1", 5),
    ("sa-east-1", 6),
]

FAUCET_AUTHORITY_ID = 7
FAUCET_LABEL = "faucet"


def placeholder_key(seed: bytes, length: int) -> bytes:
    """Deterministic byte stream from a seed. Matches the devnet's
    posture — validator-side ML-DSA isn't verified on the
    validator-to-validator wire today. The faucet key (below) IS
    minted for real because the client-submit gate verifies it."""
    out = b""
    counter = 0
    while len(out) < length:
        out += hashlib.blake2b(seed + counter.to_bytes(4, "big")).digest()
        counter += 1
    return out[:length]


def mint_real_faucet_key(out_dir: Path) -> tuple[bytes, bytes]:
    """Real ML-DSA-65 keypair via gsx-keygen. Falls back to a
    placeholder + loud warning if gsx-keygen isn't on PATH."""
    faucet_dir = out_dir / "faucet"
    faucet_dir.mkdir(parents=True, exist_ok=True)
    sk_path = faucet_dir / "mldsa.sk"
    pk_path = faucet_dir / "mldsa.pk"

    if shutil.which("gsx-keygen") is not None:
        subprocess.run(
            [
                "gsx-keygen", "--algo", "mldsa",
                "--sk", str(sk_path),
                "--pk", str(pk_path),
            ],
            check=True,
        )
        os.chmod(sk_path, 0o600)
        return sk_path.read_bytes(), pk_path.read_bytes()

    print(
        "WARNING: gsx-keygen not found on PATH; emitting placeholder faucet "
        "key. The faucet binary will reject every drip until a real ML-DSA-65 "
        "keypair is placed in faucet/mldsa.{sk,pk}. Build gsx-keygen with: "
        "  cargo build --release -p gsx-crypto --bin gsx-keygen",
        file=sys.stderr,
    )
    sk = placeholder_key(b"FAUCET-PLACEHOLDER-SK-testnet", 4032)
    pk = placeholder_key(b"FAUCET-PLACEHOLDER-PK-testnet", 1952)
    sk_path.write_bytes(sk)
    pk_path.write_bytes(pk)
    os.chmod(sk_path, 0o600)
    return sk, pk


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out-dir", type=Path, default=Path("./target/testnet/keys"))
    ap.add_argument("--network-id", default="gsx-testnet-v1")
    ap.add_argument("--validator-stake-gsx", type=int, default=1_000_000)
    ap.add_argument("--authority-stake-gsx", type=int, default=1_000_000)
    ap.add_argument(
        "--faucet-initial-balance-gsx",
        type=int,
        default=10_000_000_000,
        help="10 billion GSX. Larger than devnet (1B) because testnet runs longer + has more dApps drawing on the faucet.",
    )
    ap.add_argument(
        "--rounds-per-epoch",
        type=int,
        default=4096,
        help="4× devnet — longer epochs reduce governance churn during the 12-month testnet life.",
    )
    args = ap.parse_args()

    args.out_dir.mkdir(parents=True, exist_ok=True)

    MLDSA_SK_LEN = 4032
    BLS_SK_LEN = 32

    validator_entries = []
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

        mldsa_pk = hashlib.blake2b(mldsa_sk, digest_size=32).hexdigest()
        bls_pk = hashlib.blake2b(bls_sk, digest_size=48).hexdigest()
        validator_entries.append((aid, region, mldsa_pk, bls_pk))

    _faucet_sk, faucet_pk = mint_real_faucet_key(args.out_dir)
    faucet_pk_hex = faucet_pk.hex()
    faucet_bls_pk_hex = hashlib.blake2b(b"faucet-bls-placeholder-testnet", digest_size=48).hexdigest()

    genesis = args.out_dir / "genesis.toml"
    with genesis.open("w") as f:
        f.write(f'network_id = "{args.network_id}"\n')
        f.write(f'rounds_per_epoch = {args.rounds_per_epoch}\n\n')
        for aid, region, mldsa_pk, bls_pk in validator_entries:
            f.write("[[validators]]\n")
            f.write(f"authority_id = {aid}\n")
            f.write(f'label = "{region}"\n')
            f.write(f'mldsa_public_key_hex = "{mldsa_pk}"\n')
            f.write(f'bls_public_key_hex = "{bls_pk}"\n')
            f.write(f"validator_stake_gsx = {args.validator_stake_gsx}\n")
            f.write(f"authority_stake_gsx = {args.authority_stake_gsx}\n\n")

        f.write("[[validators]]\n")
        f.write(f"authority_id = {FAUCET_AUTHORITY_ID}\n")
        f.write(f'label = "{FAUCET_LABEL}"\n')
        f.write(f'mldsa_public_key_hex = "{faucet_pk_hex}"\n')
        f.write(f'bls_public_key_hex = "{faucet_bls_pk_hex}"\n')
        # Faucet stake must clear AUTHORITY_STAKE_THRESHOLD_GSX (100,000)
        # so registry.admit() succeeds. Without this, the faucet's
        # pubkey never enters the AuthorityRegistry and every signed
        # drip intent is rejected with UnknownSigner.
        f.write(f"validator_stake_gsx = {args.authority_stake_gsx}\n")
        f.write(f"authority_stake_gsx = {args.authority_stake_gsx}\n\n")

    import blake3 as _b3  # pip install blake3
    faucet_addr_20 = _b3.blake3(faucet_pk).digest()[:20]
    faucet_addr_hex = "0x" + faucet_addr_20.hex()

    with genesis.open("a") as f:
        f.write("# Pre-genesis balances applied before round 0.\n")
        f.write("[[prebalances]]\n")
        f.write(f'address = "{faucet_addr_hex}"\n')
        f.write(f"balance_gsx = {args.faucet_initial_balance_gsx}\n")
        f.write(f'role = "faucet"\n\n')

    print(f"wrote {genesis}", file=sys.stderr)
    for aid, region, _, _ in validator_entries:
        print(f"  validators[{aid}] = {region}", file=sys.stderr)
    print(f"  validators[{FAUCET_AUTHORITY_ID}] = {FAUCET_LABEL} (pk={faucet_pk_hex[:16]}...)", file=sys.stderr)
    print(f"  faucet address = {faucet_addr_hex}", file=sys.stderr)
    print(f"  faucet initial balance = {args.faucet_initial_balance_gsx:,} GSX", file=sys.stderr)
    print(
        "NOTE: validator keys are placeholders (matches devnet/perf); "
        "only the faucet ML-DSA key is real. Do NOT reuse this output "
        "for mainnet.",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
