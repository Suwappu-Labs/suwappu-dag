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


def blake3_address(pubkey: bytes) -> str:
    """CANONICAL SUWAPPU address derivation: blake3(pubkey_bytes)[:20],
    0x-prefixed hex.

    This MUST byte-match `suwappu_faucet::address_from_pubkey` (blake3
    truncated to 20 bytes) and the chain-wide reserved-address scheme in
    `suwappu_execution::reserved` ("leading 20 bytes of BLAKE3(...)").
    Do NOT substitute blake2b here — hashlib has no blake3, so this
    helper needs the `blake3` pip package or the `b3sum` CLI. A wrong
    hash silently funds an address the faucet never spends from.
    """
    try:
        import blake3  # type: ignore

        digest = blake3.blake3(pubkey).digest()
    except ImportError:
        b3sum = shutil.which("b3sum")
        if b3sum is None:
            sys.exit(
                "ERROR: computing the faucet address requires blake3 "
                "(canonical address derivation is blake3(pk)[:20]; see "
                "crates/suwappu-faucet/src/lib.rs address_from_pubkey). "
                "Install with `pip install blake3` or put `b3sum` on PATH."
            )
        out = subprocess.run(
            [b3sum, "--no-names"], input=pubkey, capture_output=True, check=True
        )
        digest = bytes.fromhex(out.stdout.decode().strip())
    return "0x" + digest[:20].hex()


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


def mint_real_faucet_key(out_dir: Path) -> tuple[bytes, bytes]:
    """Real ML-DSA-65 keypair for the faucet — see `mint_keypair`."""
    faucet_dir = out_dir / "faucet"
    return mint_keypair(faucet_dir / "mldsa.sk", faucet_dir / "mldsa.pk", "mldsa", "faucet-testnet")


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

    validator_entries = []
    for region, aid in REGIONS:
        region_dir = args.out_dir / region
        region_dir.mkdir(parents=True, exist_ok=True)

        _mldsa_sk, mldsa_pk_bytes = mint_keypair(
            region_dir / "mldsa.sk", region_dir / "mldsa.pk", "mldsa", f"{region}-mldsa-testnet"
        )
        _bls_sk, bls_pk_bytes = mint_keypair(
            region_dir / "bls.sk", region_dir / "bls.pk", "bls", f"{region}-bls-testnet"
        )
        mldsa_pk = mldsa_pk_bytes.hex()
        bls_pk = bls_pk_bytes.hex()
        validator_entries.append((aid, region, mldsa_pk, bls_pk))

    # Faucet — needs a REAL ML-DSA-65 keypair (client-submit gate verifies
    # it). BLS isn't used for faucet signing, but the manifest schema
    # requires the field, so mint a real one too.
    _faucet_sk, faucet_pk = mint_real_faucet_key(args.out_dir)
    faucet_pk_hex = faucet_pk.hex()
    _faucet_bls_sk, faucet_bls_pk = mint_keypair(
        args.out_dir / "faucet" / "bls.sk", args.out_dir / "faucet" / "bls.pk", "bls", "faucet-bls-testnet"
    )
    faucet_bls_pk_hex = faucet_bls_pk.hex()

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

        f.write("[[validators]]\n")
        f.write(f"authority_id = {FAUCET_AUTHORITY_ID}\n")
        f.write(f'label = "{FAUCET_LABEL}"\n')
        f.write(f'mldsa_public_key_hex = "{faucet_pk_hex}"\n')
        f.write(f'bls_public_key_hex = "{faucet_bls_pk_hex}"\n')
        f.write(f"validator_stake_suwappu = 1\n")
        f.write(f"authority_stake_suwappu = 1\n\n")

        # Genesis pre-balances — embedded in genesis.toml (the source of
        # truth: the daemon's GenesisManifest parses [[prebalances]] and
        # State::new credits each entry via Intent::GenesisAllocation at
        # height 0). Field names must match
        # crates/suwappu-node/src/config.rs GenesisPrebalance exactly.
        # Address derivation is the canonical blake3(pk)[:20] — see
        # blake3_address().
        faucet_addr_hex = blake3_address(faucet_pk)
        f.write("[[prebalances]]\n")
        f.write(f'address = "{faucet_addr_hex}"\n')
        f.write(f"balance_suwappu = {args.faucet_initial_balance_suwappu}\n")
        f.write(f'role = "faucet"\n\n')

    # Standalone prebalances.toml — kept for tooling that still reads it
    # (e.g. OPERATIONS.md references). genesis.toml above is the source
    # of truth; this file is a derived convenience copy.
    prebalances = args.out_dir / "prebalances.toml"
    with prebalances.open("w") as f:
        f.write("# Testnet pre-balances applied at genesis.\n")
        f.write("# DERIVED COPY — genesis.toml's [[prebalances]] is the source of truth.\n\n")
        f.write("[[balances]]\n")
        f.write(f'address = "{faucet_addr_hex}"\n')
        f.write(f"balance_suwappu = {args.faucet_initial_balance_suwappu}\n")
        f.write(f'role = "faucet"\n\n')

    print(f"wrote {genesis}", file=sys.stderr)
    for aid, region, _, _ in validator_entries:
        print(f"  validators[{aid}] = {region}", file=sys.stderr)
    print(f"  validators[{FAUCET_AUTHORITY_ID}] = {FAUCET_LABEL} (pk={faucet_pk_hex[:16]}...)", file=sys.stderr)
    print(f"  faucet address = {faucet_addr_hex}", file=sys.stderr)
    print(f"  faucet initial balance = {args.faucet_initial_balance_suwappu:,} SUWAPPU", file=sys.stderr)
    print(f"wrote {prebalances}", file=sys.stderr)
    print(
        "NOTE: all keys are real ML-DSA-65/BLS12-381 keypairs minted via "
        "suwappu-keygen (unless it wasn't on PATH, in which case per-key "
        "WARNINGs above flag placeholder fallbacks). Do NOT reuse this "
        "output for mainnet regardless — testnet keys are not access-"
        "controlled or backed up.",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
