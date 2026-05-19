//! gsx-l2-verifier-precompile — L1-side verifier for the SP1-zkVM
//! L2 (Track G Phase G2, issue #97).
//!
//! Stateless `verify_l2_batch(proof, public_inputs, vk_hash)`
//! function matching the Filecoin / op-succinct shape (per the
//! `docs/iq/IQ-006-l2-state-root-commitment-surface.md` design).
//! Verify time target: **2–5 ms single-core** once the real
//! `sp1-verifier` backend lands.
//!
//! ## Phase status
//!
//! G2.2 phase 1 (format gates) AND phase 2 (real Groth16 BN254
//! pairing check via `sp1-verifier`) are both live. The dispatch
//! function `verify_l2_batch` runs the four checks below in order:
//!
//! 1. Proof byte width (260 B, Succinct's wrapped Groth16 format)
//! 2. Public-input byte width (240 B, fixed-offset layout below)
//! 3. `vk_hash` non-zero (sentinel for "no VK pinned" rejection)
//! 4. **`sp1_verifier::Groth16Verifier::verify`** — the actual
//!    BN254 pairing check. Application VK hash is the caller's
//!    `vk_hash` arg; SP1's verifier-circuit VK is the constant
//!    `sp1_verifier::GROTH16_VK_BYTES`.
//!
//! Verify time on a single core is dominated by the pairing
//! check (~2–5 ms); the format gates are O(1) byte comparisons.
//!
//! ## Why a separate crate (not a module in gsx-execution)
//!
//! Per the internal repo audit (PR #169 IQ-006 §2): precompiles
//! are NOT currently a `apply_intent` dispatch registry. The L2
//! verifier precompile is a NEW dispatch surface added at the
//! Substrate-impl level, not an extension of `gsx-precompiles/`
//! (which ships standalone validation modules, not Intent
//! handlers). Putting the verifier in its own crate keeps the
//! cryptographic-dependency footprint isolated — when SP1
//! lands, only this crate (and its consumers via Cargo.lock)
//! see the BN254 transitive deps.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use thiserror::Error;

/// Size in bytes of an SP1 Groth16 BN254 proof. Per the Succinct
/// docs (proof-types §, op-succinct architecture book), Groth16
/// wrapped proofs are exactly **260 bytes**. This crate rejects
/// any proof bytes that don't match this width — the actual
/// pairing check runs only on validly-sized inputs.
pub const SP1_GROTH16_BN254_PROOF_BYTES: usize = 260;

/// Length in bytes of the L2 public-input blob per the Track G
/// design. Fixed-offset SSZ layout: prev_l2_state_root (32) +
/// new_l2_state_root (32) + batch_id (8) + da_commitment (32) +
/// l1_anchor_height (8) + range_vk_commitment (32) +
/// prev_l1_state_root (32) + l2_chain_id_hash (32) +
/// confidential_root (32) = **240 bytes**.
pub const L2_PUBLIC_INPUTS_BYTES: usize = 240;

/// Field offsets into the 240-byte public-input blob. Stable
/// across the L2 STM / sequencer / prover / verifier surfaces.
pub mod public_inputs {
    /// Offset of `prev_l2_state_root` (32 bytes).
    pub const PREV_L2_STATE_ROOT_OFFSET: usize = 0;
    /// Offset of `new_l2_state_root` (32 bytes).
    pub const NEW_L2_STATE_ROOT_OFFSET: usize = 32;
    /// Offset of `batch_id` (8 bytes, big-endian u64).
    pub const BATCH_ID_OFFSET: usize = 64;
    /// Offset of `da_commitment` (32 bytes — `BLAKE3(da_blob)`).
    pub const DA_COMMITMENT_OFFSET: usize = 72;
    /// Offset of `l1_anchor_height` (8 bytes, big-endian u64).
    pub const L1_ANCHOR_HEIGHT_OFFSET: usize = 104;
    /// Offset of `range_vk_commitment` (32 bytes — embedded per
    /// the op-succinct "multiBlockVKey" trick).
    pub const RANGE_VK_COMMITMENT_OFFSET: usize = 112;
    /// Offset of `prev_l1_state_root` (32 bytes — bound to the
    /// L1 state via one in-circuit SHA3-256).
    pub const PREV_L1_STATE_ROOT_OFFSET: usize = 144;
    /// Offset of `l2_chain_id_hash` (32 bytes — multi-L2
    /// namespacing).
    pub const L2_CHAIN_ID_HASH_OFFSET: usize = 176;
    /// Offset of `confidential_root` (32 bytes — Track H; all
    /// zeros when no confidential txs in the batch).
    pub const CONFIDENTIAL_ROOT_OFFSET: usize = 208;
}

/// Errors emitted by the verifier precompile.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum VerifyError {
    /// Proof bytes were not the expected Groth16 BN254 width.
    #[error("proof byte length mismatch: expected {expected}, got {got}")]
    BadProofLength {
        /// Required width.
        expected: usize,
        /// Observed width.
        got: usize,
    },

    /// Public inputs were not the expected fixed-offset width.
    #[error("public inputs byte length mismatch: expected {expected}, got {got}")]
    BadPublicInputsLength {
        /// Required width.
        expected: usize,
        /// Observed width.
        got: usize,
    },

    /// `vk_hash` was all-zeros — no VK has been pinned in
    /// chain state. The verifier rejects until governance
    /// rotates a real VK via `Intent::SetL2VerifyingKey`.
    #[error("vk_hash is all-zeros (no VK pinned in chain state)")]
    UnpinnedVerifyingKey,

    /// Groth16 BN254 pairing check failed. Fires only after
    /// the format gates pass. The actual pairing check lands
    /// in G2.2 phase 2 once `sp1-verifier` is wired; this
    /// variant is reserved for that wiring.
    #[error("groth16 bn254 verification failed")]
    Groth16Failed,
}

/// Stateless L1-side L2-batch verifier. Filecoin / op-succinct
/// shape. Validates:
///
/// 1. `proof` is exactly `SP1_GROTH16_BN254_PROOF_BYTES` (260 B).
/// 2. `public_inputs` is exactly `L2_PUBLIC_INPUTS_BYTES` (240 B).
/// 3. `vk_hash` is not all-zeros (an all-zeros hash means the
///    chain-state `aggregation_vk_hash` has never been set via
///    `Intent::SetL2VerifyingKey`, so no proof can validate).
/// 4. The Groth16 BN254 pairing check via
///    [`sp1_verifier::Groth16Verifier::verify`]. Application VK
///    hash comes from the caller (`vk_hash`); the SP1 verifier
///    circuit's VK is the bundled
///    [`sp1_verifier::GROTH16_VK_BYTES`] constant.
///
/// The substrate-side `Intent::CommitL2StateRoot` arm calls
/// this; on `Ok(())` it credits the L2 registry account
/// (`gsx_execution::reserved::l2_registry_address()`) and emits
/// a future `Event::L2StateRootCommitted`.
pub fn verify_l2_batch(
    proof: &[u8],
    public_inputs: &[u8],
    vk_hash: &[u8; 32],
) -> Result<(), VerifyError> {
    // Format gate 1: proof width.
    if proof.len() != SP1_GROTH16_BN254_PROOF_BYTES {
        return Err(VerifyError::BadProofLength {
            expected: SP1_GROTH16_BN254_PROOF_BYTES,
            got: proof.len(),
        });
    }

    // Format gate 2: public-input width.
    if public_inputs.len() != L2_PUBLIC_INPUTS_BYTES {
        return Err(VerifyError::BadPublicInputsLength {
            expected: L2_PUBLIC_INPUTS_BYTES,
            got: public_inputs.len(),
        });
    }

    // Format gate 3: pinned-VK gate. All-zeros hash is the
    // sentinel for "no VK has been pinned yet" — reject so a
    // misconfigured / fresh chain doesn't accept arbitrary
    // proofs against a zero VK.
    if vk_hash == &[0u8; 32] {
        return Err(VerifyError::UnpinnedVerifyingKey);
    }

    // Groth16 BN254 pairing check. sp1-verifier expects the
    // application VK hash as a 0x-prefixed hex string (its API
    // mirrors the on-chain ABI); we keep it as a `[u8; 32]` in
    // our public surface because that's what every other hash on
    // the substrate uses, and convert here at the boundary.
    let vk_hash_hex = format!("0x{}", hex::encode(vk_hash));
    sp1_verifier::Groth16Verifier::verify(
        proof,
        public_inputs,
        &vk_hash_hex,
        &sp1_verifier::GROTH16_VK_BYTES,
    )
    .map_err(|_| VerifyError::Groth16Failed)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_inputs() -> (Vec<u8>, Vec<u8>, [u8; 32]) {
        (
            vec![0u8; SP1_GROTH16_BN254_PROOF_BYTES],
            vec![0u8; L2_PUBLIC_INPUTS_BYTES],
            [0x42u8; 32],
        )
    }

    #[test]
    fn verify_rejects_zero_proof_at_pairing_check() {
        // Correctly-shaped inputs (proof = 260 B zeros, public_inputs
        // = 240 B zeros, vk_hash = nonzero) clear the format gates but
        // MUST fail at the Groth16 BN254 pairing check — the all-zero
        // byte string is not a valid Groth16 proof for any non-trivial
        // statement.
        //
        // This replaces an earlier positive test that asserted Ok(())
        // for zero-byte inputs, valid only when the pairing check was
        // stubbed out. Real positive coverage needs an SP1 fixture
        // (deferred until the gsx-l2-stm circuit produces one).
        let (proof, pi, vk) = valid_inputs();
        assert!(matches!(
            verify_l2_batch(&proof, &pi, &vk),
            Err(VerifyError::Groth16Failed)
        ));
    }

    #[test]
    fn verify_rejects_short_proof() {
        let (mut proof, pi, vk) = valid_inputs();
        proof.truncate(SP1_GROTH16_BN254_PROOF_BYTES - 1);
        assert!(matches!(
            verify_l2_batch(&proof, &pi, &vk),
            Err(VerifyError::BadProofLength {
                expected: SP1_GROTH16_BN254_PROOF_BYTES,
                got: _,
            })
        ));
    }

    #[test]
    fn verify_rejects_long_proof() {
        let (_, pi, vk) = valid_inputs();
        let proof = vec![0u8; SP1_GROTH16_BN254_PROOF_BYTES + 1];
        assert!(matches!(
            verify_l2_batch(&proof, &pi, &vk),
            Err(VerifyError::BadProofLength { .. })
        ));
    }

    #[test]
    fn verify_rejects_short_public_inputs() {
        let (proof, mut pi, vk) = valid_inputs();
        pi.truncate(L2_PUBLIC_INPUTS_BYTES - 1);
        assert!(matches!(
            verify_l2_batch(&proof, &pi, &vk),
            Err(VerifyError::BadPublicInputsLength {
                expected: L2_PUBLIC_INPUTS_BYTES,
                got: _,
            })
        ));
    }

    #[test]
    fn verify_rejects_long_public_inputs() {
        let (proof, _, vk) = valid_inputs();
        let pi = vec![0u8; L2_PUBLIC_INPUTS_BYTES + 7];
        assert!(matches!(
            verify_l2_batch(&proof, &pi, &vk),
            Err(VerifyError::BadPublicInputsLength { .. })
        ));
    }

    #[test]
    fn verify_rejects_all_zeros_vk_hash() {
        let (proof, pi, _) = valid_inputs();
        let vk = [0u8; 32];
        assert!(matches!(
            verify_l2_batch(&proof, &pi, &vk),
            Err(VerifyError::UnpinnedVerifyingKey)
        ));
    }

    #[test]
    fn public_input_offsets_partition_240_bytes() {
        // Each offset + its field width must equal the next offset
        // (and the final field must end at the buffer end).
        // Defensive check that the offsets don't gap or overlap.
        let offsets = [
            (public_inputs::PREV_L2_STATE_ROOT_OFFSET, 32),
            (public_inputs::NEW_L2_STATE_ROOT_OFFSET, 32),
            (public_inputs::BATCH_ID_OFFSET, 8),
            (public_inputs::DA_COMMITMENT_OFFSET, 32),
            (public_inputs::L1_ANCHOR_HEIGHT_OFFSET, 8),
            (public_inputs::RANGE_VK_COMMITMENT_OFFSET, 32),
            (public_inputs::PREV_L1_STATE_ROOT_OFFSET, 32),
            (public_inputs::L2_CHAIN_ID_HASH_OFFSET, 32),
            (public_inputs::CONFIDENTIAL_ROOT_OFFSET, 32),
        ];
        let mut cursor = 0;
        for (off, width) in offsets {
            assert_eq!(off, cursor, "gap or overlap at offset {off}");
            cursor += width;
        }
        assert_eq!(
            cursor, L2_PUBLIC_INPUTS_BYTES,
            "offsets did not partition the 240-byte blob"
        );
    }

    #[test]
    fn constants_match_track_g_spec() {
        // Locked: changing these widths requires updating the L2
        // STM prover + sequencer + verifier in lockstep. Pin them
        // in the test so a refactor doesn't silently drift.
        assert_eq!(SP1_GROTH16_BN254_PROOF_BYTES, 260);
        assert_eq!(L2_PUBLIC_INPUTS_BYTES, 240);
    }
}
