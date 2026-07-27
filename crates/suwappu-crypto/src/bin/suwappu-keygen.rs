//! Real validator/faucet keypair generator.
//!
//! Generates a real ML-DSA-65 (FIPS 204) or BLS12-381 keypair from system
//! randomness (`suwappu_crypto::mldsa::keypair()` / `bls::keypair()` — the
//! same functions the rest of this crate, and the daemon's consensus/LTP
//! signing surface, use) and writes the raw key bytes to the given paths.
//!
//! This replaces `scripts/{devnet,testnet,perf}/gen-genesis.py`'s prior
//! deterministic hash-derived placeholder keys, which were flagged
//! (correctly) as not real cryptography.

use std::fs;
use std::path::PathBuf;

use clap::{Parser, ValueEnum};

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum Algo {
    Mldsa,
    Bls,
}

/// Generate a real ML-DSA-65 or BLS12-381 keypair and write sk/pk to disk.
#[derive(Parser)]
#[command(name = "suwappu-keygen")]
struct Args {
    /// Which algorithm to generate.
    #[arg(long, value_enum)]
    algo: Algo,

    /// Output path for the secret key (raw bytes). Permissions set to 0600
    /// on write.
    #[arg(long)]
    sk: PathBuf,

    /// Output path for the public key (raw bytes).
    #[arg(long)]
    pk: PathBuf,
}

fn write_key(path: &PathBuf, bytes: &[u8], secret: bool) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)?;
    if secret {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
    }
    Ok(())
}

fn main() {
    let args = Args::parse();

    let (pk_bytes, sk_bytes): (Vec<u8>, Vec<u8>) = match args.algo {
        Algo::Mldsa => {
            let (pk, sk) = suwappu_crypto::mldsa::keypair();
            (pk.as_bytes().to_vec(), sk.as_bytes().to_vec())
        }
        Algo::Bls => {
            let (pk, sk) = suwappu_crypto::bls::keypair();
            (pk.to_bytes().to_vec(), sk.to_bytes().to_vec())
        }
    };

    write_key(&args.sk, &sk_bytes, true).unwrap_or_else(|e| {
        eprintln!(
            "suwappu-keygen: failed to write secret key to {:?}: {e}",
            args.sk
        );
        std::process::exit(1);
    });
    write_key(&args.pk, &pk_bytes, false).unwrap_or_else(|e| {
        eprintln!(
            "suwappu-keygen: failed to write public key to {:?}: {e}",
            args.pk
        );
        std::process::exit(1);
    });

    eprintln!(
        "suwappu-keygen: wrote real {} keypair -> sk={:?} pk={:?} (pk_hex={})",
        match args.algo {
            Algo::Mldsa => "ML-DSA-65",
            Algo::Bls => "BLS12-381",
        },
        args.sk,
        args.pk,
        hex::encode(&pk_bytes),
    );
}
