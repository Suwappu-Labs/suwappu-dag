//! `suwappu-keygen` — mint a real PQ / classical keypair and write raw bytes to disk.
//!
//! Used by `scripts/{devnet,testnet}/gen-genesis.py` to mint the faucet's
//! ML-DSA-65 keypair (the validator binary verifies faucet drips against this
//! key on the client-submit gate; placeholders are rejected). Validator-side
//! ML-DSA keys are deterministic placeholders today (matches paper §3.3
//! exception zone — validator-to-validator wire doesn't verify ML-DSA yet),
//! so this tool intentionally writes raw bytes that
//! `suwappu_crypto::mldsa::SecretKey::from_bytes` round-trips.

use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

use clap::{Parser, ValueEnum};
use suwappu_crypto::{bls, mldsa};

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Algo {
    /// ML-DSA-65 (FIPS 204) — the post-quantum signing key (faucet, intents).
    Mldsa,
    /// BLS12-381 (min-pubkey-size) — the validator/corridor co-signature key.
    Bls,
}

#[derive(Parser, Debug)]
#[command(
    name = "suwappu-keygen",
    about = "Mint a real keypair (ML-DSA-65 / FIPS 204, or BLS12-381) and write raw bytes."
)]
struct Args {
    #[arg(long, value_enum)]
    algo: Algo,

    #[arg(long, help = "Output path for the secret key (chmod 0600).")]
    sk: PathBuf,

    #[arg(long, help = "Output path for the public key.")]
    pk: PathBuf,

    /// Optional output path for the 20-byte canonical address derived
    /// from the public key. Writes `0x<hex>` so `gen-devnet-genesis.py`
    /// can lift the same blake3 truncation `suwappu_faucet::address_from_pubkey`
    /// uses at runtime without re-implementing blake3 in Python.
    #[arg(long)]
    addr: Option<PathBuf>,
}

fn main() -> std::io::Result<()> {
    let args = Args::parse();
    match args.algo {
        Algo::Mldsa => {
            let (pk, sk) = mldsa::keypair();
            if let Some(dir) = args.sk.parent() {
                fs::create_dir_all(dir)?;
            }
            if let Some(dir) = args.pk.parent() {
                fs::create_dir_all(dir)?;
            }
            fs::write(&args.sk, sk.as_bytes())?;
            fs::set_permissions(&args.sk, fs::Permissions::from_mode(0o600))?;
            fs::write(&args.pk, pk.as_bytes())?;
            eprintln!(
                "suwappu-keygen: wrote ml-dsa-65 sk={} ({} B) pk={} ({} B)",
                args.sk.display(),
                sk.as_bytes().len(),
                args.pk.display(),
                pk.as_bytes().len(),
            );

            if let Some(addr_path) = args.addr.as_ref() {
                let digest = blake3::hash(pk.as_bytes());
                let addr_bytes = &digest.as_bytes()[..20];
                let addr_hex = format!("0x{}", hex::encode(addr_bytes));
                if let Some(dir) = addr_path.parent() {
                    fs::create_dir_all(dir)?;
                }
                fs::write(addr_path, &addr_hex)?;
                eprintln!(
                    "suwappu-keygen: wrote address {}={}",
                    addr_path.display(),
                    addr_hex,
                );
            }
        }
        Algo::Bls => {
            // BLS12-381 (min-pubkey-size): sk = 32 B, pk = 48 B. Raw blst
            // serialization, matching what the validator/corridor co-sign
            // path loads. VALIDATOR-OPERATORS.md documents this for
            // `--algo bls`; without it the documented onboarding step fails.
            let (pk, sk) = bls::keypair();
            if let Some(dir) = args.sk.parent() {
                fs::create_dir_all(dir)?;
            }
            if let Some(dir) = args.pk.parent() {
                fs::create_dir_all(dir)?;
            }
            fs::write(&args.sk, sk.to_bytes())?;
            fs::set_permissions(&args.sk, fs::Permissions::from_mode(0o600))?;
            fs::write(&args.pk, pk.to_bytes())?;
            eprintln!(
                "suwappu-keygen: wrote bls12-381 sk={} ({} B) pk={} ({} B)",
                args.sk.display(),
                sk.to_bytes().len(),
                args.pk.display(),
                pk.to_bytes().len(),
            );
            // `--addr` is the ML-DSA faucet-address helper; not meaningful for BLS.
            if args.addr.is_some() {
                eprintln!("suwappu-keygen: --addr is ignored for --algo bls");
            }
        }
    }
    Ok(())
}
