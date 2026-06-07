#!/usr/bin/env python3
"""
Generate the 4-validator devnet genesis manifest + per-region ML-DSA / BLS
keypairs + a seeded faucet authority.

Output layout (in --out-dir, default ./target/devnet/keys):

    genesis.toml
    us-east-1/mldsa.sk
    us-east-1/bls.sk
    eu-west-1/mldsa.sk
    eu-west-1/bls.sk
    ap-southeast-1/mldsa.sk
    ap-southeast-1/bls.sk
    sa-east-1/mldsa.sk
    sa-east-1/bls.sk
    faucet/mldsa.sk       <-- real ML-DSA-65 key (the faucet binary loads this)
    faucet/mldsa.pk       <-- matching public key

Validator-side keys (entries 0-3) use deterministic placeholders for now —
this matches the perf testnet's posture and reflects that suwappu-node does
NOT currently verify ML-DSA signatures on validator-to-validator wire
traffic (only on client-submitted intents at the verify_signed_intent
gate). If/when on-wire validator-side ML-DSA verification lands, this
script must switch to real keys via the suwappu-keygen binary.

The faucet's authority entry (id = 4) MUST have a real ML-DSA-65 keypair
because every faucet drip submits a signed Transfer intent through the
intent_signing_digest path, which IS verified by the validators.
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
    ("eu-west-1", 1),
    ("ap-southeast-1", 2),
    ("sa-east-1", 3),
]

FAUCET_AUTHORITY_ID = 4
FAUCET_LABEL = "faucet"


def placeholder_key(seed: bytes, length: int) -> bytes:
    """Deterministic byte stream from a seed. Not cryptographically random.
    Acceptable only for validator-side keys on this devnet, where the
    validator-to-validator wire doesn't currently verify ML-DSA."""
    out = b""
    counter = 0
    while len(out) < length:
        out += hashlib.blake2b(seed + counter.to_bytes(4, "big")).digest()
        counter += 1
    return out[:length]


def mint_real_faucet_key(out_dir: Path) -> tuple[bytes, bytes]:
    """Generate a real ML-DSA-65 keypair for the faucet via suwappu-keygen.

    Falls back to placeholder bytes if suwappu-keygen isn't available, with a
    loud warning — but a placeholder faucet key cannot actually sign
    valid transfers, so the faucet service will reject every drip until
    a real key is dropped in place.
    """
    faucet_dir = out_dir / "faucet"
    faucet_dir.mkdir(parents=True, exist_ok=True)
    sk_path = faucet_dir / "mldsa.sk"
    pk_path = faucet_dir / "mldsa.pk"

    if shutil.which("suwappu-keygen") is not None:
        # suwappu-keygen --algo mldsa --sk <path> --pk <path>
        subprocess.run(
            [
                "suwappu-keygen",
                "--algo", "mldsa",
                "--sk", str(sk_path),
                "--pk", str(pk_path),
            ],
            check=True,
        )
        os.chmod(sk_path, 0o600)
        return sk_path.read_bytes(), pk_path.read_bytes()

    # Fallback — placeholder. Faucet WILL NOT WORK with this key.
    print(
        "WARNING: suwappu-keygen not found on PATH; emitting placeholder faucet "
        "key. The faucet binary will reject every drip until a real ML-DSA-65 "
        "keypair is placed in faucet/mldsa.{sk,pk}. Build suwappu-keygen with: "
        "  cargo build --release -p suwappu-crypto --bin suwappu-keygen",
        file=sys.stderr,
    )
    seed_sk = f"FAUCET-PLACEHOLDER-SK".encode()
    seed_pk = f"FAUCET-PLACEHOLDER-PK".encode()
    sk = placeholder_key(seed_sk, 4032)
    pk = placeholder_key(seed_pk, 1952)
    sk_path.write_bytes(sk)
    pk_path.write_bytes(pk)
    os.chmod(sk_path, 0o600)
    return sk, pk


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out-dir", type=Path, default=Path("./target/devnet/keys"))
    ap.add_argument("--network-id", default="suwappu-devnet")
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
        help="Epoch length in rounds. Devnet default matches paper (1024 = ~256s at 250ms rounds).",
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

        # Public-key surrogate — matches the perf-testnet pattern. The
        # validator-side ML-DSA verifier is not on the wire today; if it
        # ever lands, replace this with the real pk produced by
        # suwappu-keygen.
        mldsa_pk = hashlib.blake2b(mldsa_sk, digest_size=32).hexdigest()
        bls_pk = hashlib.blake2b(bls_sk, digest_size=48).hexdigest()

        validator_entries.append((aid, region, mldsa_pk, bls_pk))

    # Faucet authority — needs a REAL ML-DSA-65 keypair.
    _faucet_sk, faucet_pk = mint_real_faucet_key(args.out_dir)
    faucet_pk_hex = faucet_pk.hex()

    # Faucet address derivation: blake3(pk)[:20] — MUST match the runtime
    # recipe in `suwappu_faucet::address_from_pubkey` (`crates/suwappu-faucet/
    # src/lib.rs:103`). Computed up front so it can be embedded inline in
    # genesis.toml's `[[prebalances]]` block before the manifest file
    # handle closes. Requires the `blake3` Python package
    # (`pip install blake3`).
    import blake3 as _b3
    faucet_addr_20 = _b3.blake3(faucet_pk).digest()[:20]
    faucet_addr_hex = "0x" + faucet_addr_20.hex()

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

        # Faucet signer — registered in the manifest as a [[signers]] entry
        # so its signature gate (`verify_signed_intent` resolves
        # signer_pubkey_hash against the seated AuthorityRegistry) accepts
        # faucet-signed transfers. It does NOT participate in consensus, so
        # it must NOT be a [[validators]] entry: that inflates committee size
        # n (= validators.len()) and the quorum thresholds, and since the
        # faucet runs no node it never proposes or votes, starving
        # finalization. stake_suwappu must clear AUTHORITY_STAKE_THRESHOLD_SUWAPPU
        # (100,000) or AuthorityRegistry::admit silently drops it and every
        # drip hits UnknownSigner (the old `= 1` stake was below the floor).
        f.write("[[signers]]\n")
        f.write(f"authority_id = {FAUCET_AUTHORITY_ID}\n")
        f.write(f'label = "{FAUCET_LABEL}"\n')
        f.write(f'mldsa_public_key_hex = "{faucet_pk_hex}"\n')
        f.write(f"stake_suwappu = 100000\n\n")

        # Inline `[[prebalances]]` block — what the runtime actually reads.
        # `crates/suwappu-node/src/daemon.rs:296` initializes
        # `InMemorySubstrate::from_balances` exclusively from
        # `manifest.prebalances`. Without this block the devnet faucet
        # starts at zero balance and every drip fails on
        # InsufficientBalance. The standalone `prebalances.toml` written
        # below is a legacy operator-readable artifact only. (Codex #228 P1.)
        f.write("[[prebalances]]\n")
        f.write(f'address = "{faucet_addr_hex}"\n')
        f.write(f"balance_suwappu = {args.faucet_initial_balance_suwappu}\n")
        f.write(f'role = "faucet"\n\n')

    prebalances = args.out_dir / "prebalances.toml"
    with prebalances.open("w") as f:
        f.write("# Devnet pre-balances applied at genesis. Each address starts\n")
        f.write("# with the listed balance before round 0. NOTE: the runtime\n")
        f.write("# reads pre-balances from genesis.toml's [[prebalances]]\n")
        f.write("# block, NOT from this file. Preserved as a human-readable\n")
        f.write("# audit artifact only.\n\n")
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
        "NOTE: validator keys are placeholders (matches perf); only the faucet "
        "ML-DSA key is real. Do not reuse this output for mainnet.",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
