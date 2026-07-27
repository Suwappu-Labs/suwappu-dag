//! SP1 zkVM circuit: reserve-coverage composition proof (paper §8.3).
//!
//! Proves: "there exists a hidden multiset of reserve line-item amounts
//! that (a) sums to `total_reserves` and (b) hashes (with a private
//! blinding salt) to `commitment`" — without revealing the individual
//! amounts or how many there are. This is the F1 (aggregate-only)
//! disclosure tier from `reserve.rs`'s `DisclosureTier` enum: only the
//! aggregate total is ever disclosed, never the composition.
//!
//! `total_reserves` and `commitment` are NOT read as inputs to check
//! against — they're computed from the private witness and *committed
//! as public outputs*. The verifier (host or on-chain) checks those
//! outputs match the `ReserveAttestation` it already has; there's
//! nothing to "get wrong" by supplying inconsistent inputs, because the
//! circuit derives them itself.
//!
//! `predicate_satisfied()` in `reserve.rs` still runs in the clear
//! against the (now proven-genuine) `total_reserves` — this circuit
//! only proves the *aggregation* is real, not the coverage-rule
//! arithmetic, which doesn't need hiding.
//!
//! F2–F5 (broad-category through CUSIP-level disclosure) are NOT
//! implemented here — this circuit fixes the amount-only, F1 case.
//! Extending to richer tiers means committing per-item
//! (amount, category, CUSIP) leaves into a Merkle tree instead of a
//! flat hash, so higher tiers can selectively open individual leaves.
//! That's a real follow-up, not attempted in this pass.

#![no_main]
sp1_zkvm::entrypoint!(main);

use sha3::{Digest, Sha3_256};

/// Must byte-for-byte match `suwappu_crypto::hash::sha3_256_domain`'s
/// layout (SHA3-256(u32_be(tag.len()) || tag || data)) so a proof's
/// commitment output is directly comparable to anything computed with
/// the real crypto crate off-chain, without depending on it here (that
/// crate pulls in blst/pqcrypto C code that isn't RISC-V/zkVM-portable).
const DOMAIN_TAG: &[u8] = b"SUWAPPU-RESERVE-COMMIT-V1";

fn sha3_256_domain(tag: &[u8], data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha3_256::new();
    hasher.update((tag.len() as u32).to_be_bytes());
    hasher.update(tag);
    hasher.update(data);
    let out = hasher.finalize();
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&out);
    bytes
}

pub fn main() {
    // -------------------------------------------------------------
    // Private witnesses.
    // -------------------------------------------------------------
    let salt: Vec<u8> = sp1_zkvm::io::read_vec(); // 32-byte blinding factor
    let amounts_blob: Vec<u8> = sp1_zkvm::io::read_vec(); // item_count(4 BE) || amount_i(16 BE)*

    assert_eq!(salt.len(), 32, "salt must be 32 bytes");
    assert!(amounts_blob.len() >= 4, "amounts blob missing item_count header");

    let item_count = u32::from_be_bytes(amounts_blob[0..4].try_into().unwrap()) as usize;
    assert_eq!(
        amounts_blob.len(),
        4 + item_count * 16,
        "amounts blob length doesn't match item_count"
    );
    assert!(item_count > 0, "at least one reserve line item is required");

    // -------------------------------------------------------------
    // Sum the hidden amounts (u128, checked — overflow aborts the
    // guest, which means no proof is ever produced for an
    // overflowing composition: soundness by construction, not by a
    // runtime check someone could skip).
    // -------------------------------------------------------------
    let mut total_reserves: u128 = 0;
    for i in 0..item_count {
        let start = 4 + i * 16;
        let amount = u128::from_be_bytes(amounts_blob[start..start + 16].try_into().unwrap());
        total_reserves = total_reserves
            .checked_add(amount)
            .expect("reserve composition sum overflowed u128");
    }

    // -------------------------------------------------------------
    // Commitment: SHA3-256(salt || amounts_blob), domain-separated.
    // -------------------------------------------------------------
    let mut preimage = Vec::with_capacity(32 + amounts_blob.len());
    preimage.extend_from_slice(&salt);
    preimage.extend_from_slice(&amounts_blob);
    let commitment = sha3_256_domain(DOMAIN_TAG, &preimage);

    // -------------------------------------------------------------
    // Public outputs.
    // -------------------------------------------------------------
    sp1_zkvm::io::commit_slice(&total_reserves.to_be_bytes()); // 16 bytes
    sp1_zkvm::io::commit_slice(&commitment); // 32 bytes
}
