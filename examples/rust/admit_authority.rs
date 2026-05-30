//! Example: build, sign, and submit an `Intent::AdmitAuthority`.
//!
//! Used both as a runnable utility AND as the canonical reference
//! for any downstream tool that needs to submit a governance intent
//! (e.g. the `scripts/testnet/admit-operator.sh` wrapper invokes
//! this binary after pulling the foundation signer's secret key
//! from AWS Secrets Manager).
//!
//! Run:
//!     cd examples/rust && cargo run --bin admit_authority -- \
//!         --rpc-url https://rpc.testnet.gsx.globalsettlement.com \
//!         --network-id gsx-testnet-v1 \
//!         --signer-sk /path/to/foundation/mldsa.sk \
//!         --signer-pk /path/to/foundation/mldsa.pk \
//!         --authority-id 8 \
//!         --stake-gsx 100000 \
//!         --candidate-mldsa-pk-hex f515ad3a... \
//!         --candidate-bls-pk-hex   050b11c0...
//!
//! ## What this does
//!
//! 1. Loads the foundation signer's ML-DSA-65 keypair from disk.
//!    The signer MUST already be in the testnet's Authority Ring
//!    (today: authority_id=7, the faucet) — otherwise the daemon
//!    rejects with `UnknownSigner`.
//! 2. Builds the `Intent::AdmitAuthority` for the new candidate.
//! 3. Bincode-serializes the intent, computes the
//!    `intent_signing_digest`, and signs it.
//! 4. Submits via JSON-RPC `gsx_submitIntent`.
//! 5. Polls `gsx_getAuthorityRegistry` until the new authority_id
//!    appears (or times out at ~5 min).
//!
//! ## Why a separate binary?
//!
//! Bash can shell out to `aws secretsmanager get-secret-value` and
//! `curl https://rpc.testnet.gsx.*/`, but cannot bincode-serialize
//! a Rust enum. Keeping the construct-sign-submit logic in Rust
//! ensures the bash wrapper never drifts from the on-chain
//! representation.

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use gsx_execution::Intent;
use std::fs;
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
#[command(name = "admit_authority")]
struct Args {
    /// JSON-RPC URL of any seated testnet validator. Defaults to the
    /// public wildcard; pass an EIP directly if the wildcard isn't
    /// resolving yet (DNS delegation in flight).
    #[arg(long, default_value = "https://rpc.testnet.gsx.globalsettlement.com")]
    rpc_url: String,

    /// Network id (e.g. `gsx-testnet-v1`). Baked into the
    /// `intent_signing_digest` along with `INTENT_DOMAIN_TAG`.
    #[arg(long)]
    network_id: String,

    /// Path to the foundation signer's ML-DSA-65 secret key (4032 B
    /// raw bytes — same format `gsx-keygen` emits). Must correspond
    /// to a key already in the Authority Ring.
    #[arg(long)]
    signer_sk: String,

    /// Path to the matching public key (1952 B). Used to compute
    /// `signer_pubkey_hash` for the RPC submit.
    #[arg(long)]
    signer_pk: String,

    /// Candidate's zero-indexed authority slot. Must be ≥ 8 for
    /// external operators (0..6 are foundation seeds, 7 is the
    /// faucet).
    #[arg(long)]
    authority_id: u32,

    /// Candidate's stake in whole GSX. Must clear
    /// AUTHORITY_STAKE_THRESHOLD_GSX (100,000). For the testnet
    /// where external operators don't actually post stake (Track B
    /// is points-based), pass 100000 as a nominal floor-clearing
    /// value.
    #[arg(long, default_value_t = 100_000)]
    stake_gsx: u64,

    /// Candidate's ML-DSA-65 public key as a hex string (no `0x`
    /// prefix required; 3904 hex chars = 1952 bytes). The operator
    /// mints this locally; you receive it in their application.
    #[arg(long)]
    candidate_mldsa_pk_hex: String,

    /// Candidate's BLS12-381 G1 public key as a hex string (96 hex
    /// chars = 48 bytes compressed).
    #[arg(long)]
    candidate_bls_pk_hex: String,

    /// Skip the post-submit poll. Useful for dry-runs / testing the
    /// signing pipeline without waiting for chain commit.
    #[arg(long)]
    skip_poll: bool,
}

const INTENT_DOMAIN_TAG: &[u8] = b"GSX_INTENT_V1";

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // 1. Load signer keypair.
    let signer_sk_bytes = fs::read(&args.signer_sk)
        .with_context(|| format!("read signer sk {}", args.signer_sk))?;
    let signer_pk_bytes = fs::read(&args.signer_pk)
        .with_context(|| format!("read signer pk {}", args.signer_pk))?;
    let signer_sk = gsx_crypto::mldsa::SecretKey::from_bytes(&signer_sk_bytes)
        .map_err(|e| anyhow!("signer sk parse: {:?}", e))?;
    let signer_pk = gsx_crypto::mldsa::PublicKey::from_bytes(&signer_pk_bytes)
        .map_err(|e| anyhow!("signer pk parse: {:?}", e))?;
    eprintln!(
        "[admit_authority] signer pubkey_hash = 0x{}",
        hex::encode(blake3::hash(signer_pk.as_bytes()).as_bytes())
    );

    // 2. Parse candidate's hex pubkeys.
    let candidate_mldsa =
        hex::decode(args.candidate_mldsa_pk_hex.trim_start_matches("0x"))
            .context("candidate mldsa pk hex decode")?;
    let candidate_bls =
        hex::decode(args.candidate_bls_pk_hex.trim_start_matches("0x"))
            .context("candidate bls pk hex decode")?;
    if candidate_mldsa.len() != 1952 {
        return Err(anyhow!(
            "candidate_mldsa_pk_hex must decode to 1952 bytes, got {}",
            candidate_mldsa.len()
        ));
    }
    if candidate_bls.len() != 48 {
        return Err(anyhow!(
            "candidate_bls_pk_hex must decode to 48 bytes (BLS12-381 G1 compressed), got {}",
            candidate_bls.len()
        ));
    }

    // 3. Build the intent.
    let intent = Intent::AdmitAuthority {
        authority_id: args.authority_id,
        stake_gsx: args.stake_gsx,
        mldsa_public_key: candidate_mldsa,
        bls_public_key: candidate_bls,
    };
    let intent_bincode = bincode::serialize(&intent)?;
    eprintln!(
        "[admit_authority] intent_bincode = {} bytes",
        intent_bincode.len()
    );

    // 4. Compute digest + sign.
    let mut hasher = blake3::Hasher::new();
    hasher.update(INTENT_DOMAIN_TAG);
    hasher.update(args.network_id.as_bytes());
    hasher.update(&intent_bincode);
    let digest = *hasher.finalize().as_bytes();
    let signature = gsx_crypto::mldsa::sign(&digest, &signer_sk)
        .map_err(|e| anyhow!("sign: {:?}", e))?;
    let signer_pubkey_hash: [u8; 32] = *blake3::hash(signer_pk.as_bytes()).as_bytes();

    // 5. Submit via SDK.
    let client = gsx_client::Client::new(args.rpc_url.clone());
    let tx_hash = client
        .submit_intent_raw(&intent_bincode, signature.as_bytes(), signer_pubkey_hash)
        .await
        .with_context(|| format!("submit_intent_raw @ {}", args.rpc_url))?;
    println!(
        "{{\"tx_hash\":\"0x{}\",\"authority_id\":{},\"stake_gsx\":{}}}",
        hex::encode(tx_hash),
        args.authority_id,
        args.stake_gsx
    );

    if args.skip_poll {
        eprintln!("[admit_authority] --skip-poll set; not waiting for registry update");
        return Ok(());
    }

    // 6. Poll for confirmation via the SDK's get_authority_registry.
    // Authority registry updates apply at the EPOCH BOUNDARY, not within
    // a few rounds: testnet's rounds_per_epoch=4096 at 250ms ≈ a ~17-minute
    // epoch, so an AdmitAuthority submitted just after a boundary waits the
    // better part of an epoch to land. A 5-minute deadline timed out on the
    // very first onboarding even though the tx eventually settled (Codex P1),
    // so wait at least one full epoch + margin.
    let deadline = Instant::now() + Duration::from_secs(1200);
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        if Instant::now() > deadline {
            return Err(anyhow!(
                "timed out after 20 min waiting for authority_id={} to appear in registry ({} polls)",
                args.authority_id,
                attempts
            ));
        }
        let members = client.get_authority_registry().await?;
        if members.iter().any(|m| m.id == args.authority_id) {
            eprintln!(
                "[admit_authority] authority_id={} confirmed in registry after {} polls",
                args.authority_id, attempts
            );
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}
