//! gsx-keygen — generate real ML-DSA-65 + BLS12-381 keypairs for a local devnet.
//!
//! For each of N validators, emits:
//!   <out-dir>/v<i>/mldsa.sk   — raw ML-DSA-65 secret-key bytes (4 032 B)
//!   <out-dir>/v<i>/bls.sk     — raw BLS12-381 secret-key bytes (32 B)
//!   <out-dir>/genesis.toml    — GenesisManifest with real mldsa_public_key_hex
//!                               and bls_public_key_hex entries
//!
//! The sk file for node i is the EXACT keypair that produced the pk hex stored
//! in genesis for node i — BridgeHeaderSigner::from_config's sk↔pk probe passes.
//!
//! Usage:
//!   gsx-keygen --num-nodes 4 --out-dir target/devnet-real
//!   gsx-keygen --num-nodes 4 --out-dir target/devnet-real --network-id gsx-devnet-local

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use gsx_crypto::{bls, mldsa};

#[derive(Parser, Debug)]
#[command(
    name = "gsx-keygen",
    version,
    about = "Generate real ML-DSA-65 + BLS12-381 keypairs for a gsx-dag devnet"
)]
struct Args {
    /// Number of validators to generate keys for (Mysticeti-C BFT needs >= 4).
    #[arg(long, default_value_t = 4)]
    num_nodes: u32,

    /// Output directory.  Will be created if it does not exist.
    #[arg(long, default_value = "target/devnet-real")]
    out_dir: PathBuf,

    /// Network identifier embedded in every signed payload.
    #[arg(long, default_value = "gsx-devnet-local")]
    network_id: String,

    /// Per-validator stake in GSX (must be >= AUTHORITY_STAKE_THRESHOLD_GSX=100_000
    /// and VALIDATOR_STAKE_THRESHOLD_GSX=25_000; default 150_000 clears both floors).
    #[arg(long, default_value_t = 150_000u64)]
    stake_gsx: u64,

    /// Rounds per epoch.
    #[arg(long, default_value_t = 1024u64)]
    rounds_per_epoch: u64,
}

fn main() -> Result<()> {
    let args = Args::parse();

    if args.num_nodes < 4 {
        eprintln!("warning: <4 nodes; Mysticeti-C BFT requires n=3f+1 >= 4 for liveness");
    }

    std::fs::create_dir_all(&args.out_dir)
        .with_context(|| format!("create output directory {}", args.out_dir.display()))?;

    let mut validator_entries: Vec<String> = Vec::new();

    for i in 0..args.num_nodes {
        let label = format!("v{i}");
        let node_dir = args.out_dir.join(&label);
        std::fs::create_dir_all(&node_dir)
            .with_context(|| format!("create node dir {}", node_dir.display()))?;

        // Generate REAL ML-DSA-65 keypair.  sk and pk come from the SAME call —
        // this is the invariant that makes BridgeHeaderSigner::from_config pass.
        let (mldsa_pk, mldsa_sk) = mldsa::keypair();

        // Generate REAL BLS12-381 keypair.
        let (bls_pk_bytes, bls_sk_bytes) = bls::keypair_bytes();

        // Write secret keys as raw bytes.
        let mldsa_sk_path = node_dir.join("mldsa.sk");
        std::fs::write(&mldsa_sk_path, mldsa_sk.as_bytes())
            .with_context(|| format!("write {}", mldsa_sk_path.display()))?;

        let bls_sk_path = node_dir.join("bls.sk");
        std::fs::write(&bls_sk_path, &bls_sk_bytes)
            .with_context(|| format!("write {}", bls_sk_path.display()))?;

        let mldsa_pk_hex = hex::encode(mldsa_pk.as_bytes());
        let bls_pk_hex = hex::encode(&bls_pk_bytes);

        println!(
            "v{i}: mldsa pk {} bytes | bls pk {} bytes",
            mldsa_pk.as_bytes().len(),
            bls_pk_bytes.len()
        );

        validator_entries.push(format!(
            "[[validators]]\n\
             authority_id = {i}\n\
             label = \"{label}\"\n\
             mldsa_public_key_hex = \"{mldsa_pk_hex}\"\n\
             bls_public_key_hex = \"{bls_pk_hex}\"\n\
             validator_stake_gsx = {stake}\n\
             authority_stake_gsx = {stake}\n",
            stake = args.stake_gsx
        ));
    }

    // Write genesis.toml.
    let mut genesis = format!(
        "network_id = \"{}\"\n\
         rounds_per_epoch = {}\n\n",
        args.network_id, args.rounds_per_epoch
    );
    for entry in &validator_entries {
        genesis.push_str(entry);
        genesis.push('\n');
    }

    let genesis_path = args.out_dir.join("genesis.toml");
    std::fs::write(&genesis_path, &genesis)
        .with_context(|| format!("write {}", genesis_path.display()))?;

    println!();
    println!(
        "Real-key devnet genesis written to {}",
        args.out_dir.display()
    );
    println!("  validators : {}", args.num_nodes);
    println!("  network_id : {}", args.network_id);
    println!("  genesis    : {}", genesis_path.display());
    println!(
        "  keys       : {}/v{{0..{}}}/{{mldsa,bls}}.sk",
        args.out_dir.display(),
        args.num_nodes - 1
    );
    Ok(())
}
