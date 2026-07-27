//! Host-side witness building, execution, proving, and verification for
//! `reserve-coverage-verifier`.
//!
//! Two distinct operations, deliberately kept separate:
//!
//! - [`execute`]: runs the real guest program through SP1's executor and
//!   returns its real public outputs (`total_reserves`, `commitment`) —
//!   fast, low memory, but produces NO cryptographic proof. Use this to
//!   check witness/circuit correctness (does the sum match? does the
//!   hash match?) without paying proving's RAM/time cost.
//! - [`prove`]: runs the full SP1 STARK prover and returns a real,
//!   verifiable [`SP1ProofWithPublicValues`]. This is the actual
//!   zero-knowledge proof — but it is genuinely RAM-hungry; see
//!   `suwappu-lattice-protocol#50` for the exact failure mode
//!   (OOM/swap-exhaustion) observed on a 15GB-RAM sandbox for a
//!   comparably-sized circuit. Run this on a machine with enough RAM
//!   (SP1's own guidance: 32GB+ is realistic for real circuits), or via
//!   SP1's Network prover.

use sha3::{Digest, Sha3_256};
use sp1_sdk::{
    Elf, Prover, ProverClient, ProvingKey, SP1ProofWithPublicValues, SP1Stdin, SP1VerifyingKey,
};

/// Re-exported so downstream crates (e.g. `suwappu-precompiles`'s
/// `zk-proofs` feature) reference the exact `sp1-sdk` version this
/// crate was built against, rather than pulling in their own
/// independently-resolved copy that could drift out of sync.
pub use sp1_sdk;

/// Compiled guest ELF, embedded at build time. Regenerate with
/// `cd ../reserve-coverage-verifier && cargo prove build`.
pub const ELF: &[u8] = include_bytes!(
    "../../reserve-coverage-verifier/target/elf-compilation/riscv64im-succinct-zkvm-elf/release/reserve-coverage-verifier"
);

/// Must match `reserve-coverage-verifier/src/main.rs`'s `DOMAIN_TAG` and
/// `suwappu_crypto::hash::sha3_256_domain`'s byte layout exactly.
const DOMAIN_TAG: &[u8] = b"SUWAPPU-RESERVE-COMMIT-V1";

/// Build the two length-prefixed witness vectors the guest reads, in
/// its expected order: `(salt, amounts_blob)`.
///
/// `amounts_blob` layout: `item_count(4B BE) || amount_i(16B BE)*`.
pub fn build_stdin(salt: [u8; 32], amounts: &[u128]) -> SP1Stdin {
    let mut amounts_blob = Vec::with_capacity(4 + amounts.len() * 16);
    amounts_blob.extend_from_slice(&(amounts.len() as u32).to_be_bytes());
    for a in amounts {
        amounts_blob.extend_from_slice(&a.to_be_bytes());
    }

    let mut stdin = SP1Stdin::new();
    stdin.write_vec(salt.to_vec());
    stdin.write_vec(amounts_blob);
    stdin
}

/// Off-circuit reference implementation of the same computation the
/// guest performs — used to sanity-check [`execute`]'s output against
/// an independently-computed expectation, and to precompute
/// `total_reserves`/`commitment` for a `ReserveAttestation` before a
/// proof is even generated.
pub fn compute_reference(salt: [u8; 32], amounts: &[u128]) -> (u128, [u8; 32]) {
    let total_reserves = amounts
        .iter()
        .try_fold(0u128, |acc, &a| acc.checked_add(a))
        .expect("reserve composition sum overflowed u128");

    let mut amounts_blob = Vec::with_capacity(4 + amounts.len() * 16);
    amounts_blob.extend_from_slice(&(amounts.len() as u32).to_be_bytes());
    for a in amounts {
        amounts_blob.extend_from_slice(&a.to_be_bytes());
    }
    let mut preimage = Vec::with_capacity(32 + amounts_blob.len());
    preimage.extend_from_slice(&salt);
    preimage.extend_from_slice(&amounts_blob);

    let mut hasher = Sha3_256::new();
    hasher.update((DOMAIN_TAG.len() as u32).to_be_bytes());
    hasher.update(DOMAIN_TAG);
    hasher.update(&preimage);
    let out = hasher.finalize();
    let mut commitment = [0u8; 32];
    commitment.copy_from_slice(&out);

    (total_reserves, commitment)
}

/// Parse the guest's committed public outputs: `total_reserves(16B) ||
/// commitment(32B)`.
pub fn parse_public_values(public_values: &[u8]) -> (u128, [u8; 32]) {
    assert_eq!(public_values.len(), 48, "unexpected public values length");
    let total_reserves = u128::from_be_bytes(public_values[0..16].try_into().unwrap());
    let mut commitment = [0u8; 32];
    commitment.copy_from_slice(&public_values[16..48]);
    (total_reserves, commitment)
}

/// Run the real guest program through SP1's executor (no proof
/// generated) and return its committed public outputs. Fails if the
/// guest panics (e.g. `item_count == 0`, or the sum overflows u128) —
/// that failure IS the circuit's soundness check, same as it would be
/// during real proving.
pub async fn execute(salt: [u8; 32], amounts: &[u128]) -> Result<(u128, [u8; 32]), String> {
    let stdin = build_stdin(salt, amounts);
    let client = ProverClient::builder().cpu().build().await;
    let (public_values, _report) = client
        .execute(Elf::Static(ELF), stdin)
        .await
        .map_err(|e| format!("guest execution failed: {e}"))?;
    Ok(parse_public_values(public_values.as_slice()))
}

/// Generate a real SP1 STARK proof. See the module docs for the RAM
/// caveat — this is not a mock, but it is genuinely resource-heavy.
pub async fn prove(
    salt: [u8; 32],
    amounts: &[u128],
) -> Result<(SP1ProofWithPublicValues, SP1VerifyingKey), String> {
    let stdin = build_stdin(salt, amounts);
    let client = ProverClient::builder().cpu().build().await;
    let pk = client
        .setup(Elf::Static(ELF))
        .await
        .map_err(|e| format!("setup failed: {e}"))?;
    let proof = client
        .prove(&pk, stdin)
        .await
        .map_err(|e| format!("proving failed: {e}"))?;
    Ok((proof, pk.verifying_key().clone()))
}

/// Synchronous variant of [`prove`] — see [`verify_blocking`]'s doc for
/// why a sync entry point exists alongside the async one.
pub fn prove_blocking(
    salt: [u8; 32],
    amounts: &[u128],
) -> Result<(SP1ProofWithPublicValues, SP1VerifyingKey), String> {
    use sp1_sdk::blocking::{
        ProveRequest, Prover as BlockingProver, ProverClient as BlockingProverClient,
    };

    let stdin = build_stdin(salt, amounts);
    let client = BlockingProverClient::builder().cpu().build();
    let pk = BlockingProver::setup(&client, Elf::Static(ELF))
        .map_err(|e| format!("setup failed: {e}"))?;
    let proof = ProveRequest::run(BlockingProver::prove(&client, &pk, stdin))
        .map_err(|e| format!("proving failed: {e}"))?;
    Ok((proof, pk.verifying_key().clone()))
}

/// Verify a proof, and additionally check its committed public outputs
/// match the `(total_reserves, commitment)` the caller expects (e.g.
/// from a stored `ReserveAttestation`) — verifying the STARK alone
/// only proves "some valid execution of this circuit produced these
/// public values," not that those values are the ones you care about.
pub async fn verify(
    proof: &SP1ProofWithPublicValues,
    vk: &SP1VerifyingKey,
    expected_total_reserves: u128,
    expected_commitment: [u8; 32],
) -> Result<(), String> {
    let client = ProverClient::builder().cpu().build().await;
    Prover::verify(&client, proof, vk, None)
        .map_err(|e| format!("proof verification failed: {e}"))?;

    let (total_reserves, commitment) = parse_public_values(proof.public_values.as_slice());
    if total_reserves != expected_total_reserves {
        return Err(format!(
            "proof's total_reserves ({total_reserves}) doesn't match expected ({expected_total_reserves})"
        ));
    }
    if commitment != expected_commitment {
        return Err("proof's commitment doesn't match expected".to_string());
    }
    Ok(())
}

/// Synchronous variant of [`verify`], for embedding into non-async
/// callers (e.g. `suwappu-precompiles`'s `ReserveCoverageChecker`,
/// which is a plain sync API and shouldn't have to grow a tokio
/// runtime dependency just to verify one proof). Uses sp1-sdk's
/// `blocking` client, which manages its own internal runtime.
pub fn verify_blocking(
    proof: &SP1ProofWithPublicValues,
    vk: &SP1VerifyingKey,
    expected_total_reserves: u128,
    expected_commitment: [u8; 32],
) -> Result<(), String> {
    use sp1_sdk::blocking::{Prover as BlockingProver, ProverClient as BlockingProverClient};

    let client = BlockingProverClient::builder().cpu().build();
    BlockingProver::verify(&client, proof, vk, None)
        .map_err(|e| format!("proof verification failed: {e}"))?;

    let (total_reserves, commitment) = parse_public_values(proof.public_values.as_slice());
    if total_reserves != expected_total_reserves {
        return Err(format!(
            "proof's total_reserves ({total_reserves}) doesn't match expected ({expected_total_reserves})"
        ));
    }
    if commitment != expected_commitment {
        return Err("proof's commitment doesn't match expected".to_string());
    }
    Ok(())
}
