#!/usr/bin/env python3
"""
Generate the 7-validator testnet genesis manifest + per-region
ML-DSA / BLS keypairs + a seeded faucet authority.

Forked from scripts/devnet/gen-genesis.py with these diffs:
  * 7 seed validator regions instead of 4 (matches paper §10.2's
    7-of-9 LTP corridor).
  * `network_id = "suwappu-testnet-v1"` (devnet is "suwappu-devnet").
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
    prebalances.toml             <-- faucet's initial SUWAPPU balance
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
    """Real ML-DSA-65 keypair via suwappu-keygen. Falls back to a
    placeholder + loud warning if suwappu-keygen isn't on PATH."""
    faucet_dir = out_dir / "faucet"
    faucet_dir.mkdir(parents=True, exist_ok=True)
    sk_path = faucet_dir / "mldsa.sk"
    pk_path = faucet_dir / "mldsa.pk"

    if shutil.which("suwappu-keygen") is not None:
        subprocess.run(
            [
                "suwappu-keygen", "--algo", "mldsa",
                "--sk", str(sk_path),
                "--pk", str(pk_path),
            ],
            check=True,
        )
        os.chmod(sk_path, 0o600)
        return sk_path.read_bytes(), pk_path.read_bytes()

    print(
        "WARNING: suwappu-keygen not found on PATH; emitting placeholder faucet "
        "key. The faucet binary will reject every drip until a real ML-DSA-65 "
        "keypair is placed in faucet/mldsa.{sk,pk}. Build suwappu-keygen with: "
        "  cargo build --release -p suwappu-crypto --bin suwappu-keygen",
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
    ap.add_argument("--network-id", default="suwappu-testnet-v1")
    ap.add_argument("--validator-stake-suwappu", type=int, default=1_000_000)
    ap.add_argument("--authority-stake-suwappu", type=int, default=1_000_000)
    ap.add_argument(
        "--faucet-initial-balance-suwappu",
        type=int,
        default=10_000_000_000,
        help="10 billion SUWAPPU. Larger than devnet (1B) because testnet runs longer + has more dApps drawing on the faucet.",
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
            f.write(f"validator_stake_suwappu = {args.validator_stake_suwappu}\n")
            f.write(f"authority_stake_suwappu = {args.authority_stake_suwappu}\n\n")

        # The faucet sits in the Authority Ring purely as a registered
        # signer (so client-submitted Transfer intents from
        # authority_id = 7 pass the UnknownSigner gate). It does NOT
        # vote on consensus — so it MUST be a [[signers]] entry, not a
        # [[validators]] entry. Seating it as a validator inflates the
        # committee size n (= validators.len()) and both quorum
        # thresholds; since the faucet runs no node it never proposes or
        # votes, which starves finalization. stake_suwappu must still clear
        # AUTHORITY_STAKE_THRESHOLD_SUWAPPU = 100_000 or AuthorityRegistry::admit
        # silently drops it and submissions hit UnknownSigner.
        f.write("[[signers]]\n")
        f.write(f"authority_id = {FAUCET_AUTHORITY_ID}\n")
        f.write(f'label = "{FAUCET_LABEL}"\n')
        f.write(f'mldsa_public_key_hex = "{faucet_pk_hex}"\n')
        f.write(f"stake_suwappu = 100000\n\n")

        # Inline pre-balances in the genesis manifest itself so every
        # validator applies the same allocations to its substrate
        # before consensus begins. The standalone prebalances.toml
        # below is kept as a human-readable audit trail; the runtime
        # reads ONLY this inline block.
        #
        # Address derivation: blake3(pk)[:20] — MUST match
        # `suwappu_faucet::address_from_pubkey` in
        # `crates/suwappu-faucet/src/lib.rs:90`. An earlier blake2b recipe
        # here drifted from the faucet binary's blake3 and required a
        # `SUWAPPU_FAUCET_ADDRESS` env override to bridge — that workaround
        # was retired alongside this fix. Requires the `blake3` Python
        # package: `pip install blake3`.
        import blake3 as _b3
        _faucet_addr_20 = _b3.blake3(faucet_pk).digest()[:20]
        _faucet_addr_hex = "0x" + _faucet_addr_20.hex()
        f.write("[[prebalances]]\n")
        f.write(f'address = "{_faucet_addr_hex}"\n')
        f.write(f"balance_suwappu = {args.faucet_initial_balance_suwappu}\n")
        f.write(f'role = "faucet"\n\n')

    # Keep the standalone prebalances.toml as an operator-readable
    # artifact. Not consumed by the runtime anymore; preserved so
    # legacy tooling that grep'd this file still works.
    faucet_addr_hex = _faucet_addr_hex
    prebalances = args.out_dir / "prebalances.toml"
    with prebalances.open("w") as f:
        f.write("# Testnet pre-balances applied at genesis.\n")
        f.write("# NOTE: the runtime reads pre-balances from\n")
        f.write("# genesis.toml's [[prebalances]] block, not from this\n")
        f.write("# file. Kept here for operator-readable audit only.\n\n")
        f.write("[[balances]]\n")
        f.write(f'address = "{faucet_addr_hex}"\n')
        f.write(f"balance_suwappu = {args.faucet_initial_balance_suwappu}\n")
        f.write(f'role = "faucet"\n\n')

    print(f"wrote {genesis}", file=sys.stderr)
    for aid, region, _, _ in validator_entries:
        print(f"  validators[{aid}] = {region}", file=sys.stderr)
    print(f"  signers[{FAUCET_AUTHORITY_ID}] = {FAUCET_LABEL} (pk={faucet_pk_hex[:16]}...)", file=sys.stderr)
    print(f"  faucet address = {faucet_addr_hex}", file=sys.stderr)
    print(f"  faucet initial balance = {args.faucet_initial_balance_suwappu:,} SUWAPPU", file=sys.stderr)
    print(f"wrote {prebalances}", file=sys.stderr)
    print(
        "NOTE: validator keys are placeholders (matches devnet/perf); "
        "only the faucet ML-DSA key is real. Do NOT reuse this output "
        "for mainnet.",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
