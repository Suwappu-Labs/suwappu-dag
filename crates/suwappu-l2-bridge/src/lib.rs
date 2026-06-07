//! suwappu-l2-bridge — L1↔L2 bridge off-chain validation library
//! (Track G G3.2, issue #101).
//!
//! ## Split of responsibility
//!
//! - **Substrate-side accounting** (debit user / credit escrow
//!   on `L1Lock`; debit escrow / credit recipient on
//!   `L2BurnProven`) lives in `suwappu-execution::substrate` and
//!   uses the existing `credit_unchecked` helper + the
//!   reserved-address `bridge_escrow_address`. This split
//!   keeps the cryptographic + state-mutation surface in one
//!   place (substrate) and avoids a circular crate dep.
//! - **Off-chain validation** (this crate): payload sanity
//!   checks the sequencer, prover, bridge UI, or any other
//!   off-chain consumer should run BEFORE submitting an
//!   Intent. Catches obviously-malformed payloads at the
//!   submission boundary instead of after the consensus round.
//!
//! ## Phase split
//!
//! - **Phase 1 (this PR / #101)** — payload validation for
//!   `L1Lock` (deposit) + `L2BurnProven` (withdrawal); zero
//!   `amount` is rejected; max-amount sanity floor is
//!   enforced; `merkle_path` is validated for byte-shape only.
//! - **Phase 2** — full Merkle proof verification once the
//!   L2 state-root storage lands (depends on G2.2 phase 2).
//!   The validation surface stays stable; only the proof body
//!   check moves from "byte-shape only" to "Merkle inclusion
//!   against the proven L2 state".
//! - **Phase 3** — `L2ForceInclude` payload validation
//!   (Track G G3.4 #103 force-include slashing test).
//!
//! ## Bridge accounting invariant
//!
//! The substrate-side enforcement is the load-bearing
//! guarantee:
//!
//! > At every L1 block boundary,
//! >   `balance(bridge_escrow_address)
//! >     == sum_of_unwithdrawn_L2_deposits`.
//!
//! Off-chain validators (this crate) cannot enforce this
//! invariant directly — they don't see the chain state. But
//! they CAN catch local-payload bugs (zero amount, malformed
//! recipient, missing merkle_path) before they hit the
//! mempool, reducing wasted block-space + RPC noise.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 20-byte EVM-compatible address. Mirrors
/// `suwappu_execution::substrate::Address` without taking a
/// crate dep (avoids the circularity in the G3.2 split).
pub type Address = [u8; 20];

/// Balance type. Mirrors `suwappu_execution::substrate::Balance`.
pub type Balance = u128;

/// Maximum reasonable bridge payload size in bytes for the
/// `L2BurnProven::merkle_path` field. The phase-2 merkle-path
/// encoding is bounded at log2(L2_NULLIFIER_TREE_DEPTH) × 32
/// = 32 levels × 32 B/digest = 1024 B max. We allow 4× slack
/// for any envelope overhead.
pub const MAX_MERKLE_PATH_BYTES: usize = 4096;

/// Maximum supported merkle tree depth for `L2BurnProven` inclusion
/// verification. Bounded at 32 levels per IQ-008 (sanity cap; the
/// byte-shape limit allows up to 128 levels). One direction bit per
/// level, packed LSB-first into `path_directions`.
pub const MAX_BURN_TREE_LEVELS: usize = 32;

/// Domain tag for `L2BurnProven` merkle leaves. Length-distinguished
/// from the inner-node tag so a leaf hash CANNOT be misread as an
/// inner node. See IQ-008.
pub const BURN_LEAF_DOMAIN_TAG: &[u8] = b"suwappu-l2-burn-leaf-v1";

/// Domain tag for `L2BurnProven` merkle inner nodes. Length-
/// distinguished from the leaf tag. See IQ-008.
pub const BURN_NODE_DOMAIN_TAG: &[u8] = b"suwappu-l2-burn-node-v1";

/// Minimum bridge deposit amount in SUWAPPU (smallest unit).
/// Below this is dust — the L1 gas cost of locking + the L2
/// gas cost of crediting exceed the deposit value. The
/// substrate accepts dust deposits (no on-chain rule against
/// them) but the off-chain UI should warn the user.
pub const DUST_THRESHOLD_SUWAPPU: Balance = 100; // 100 base units

/// Errors returned by off-chain bridge-payload validation.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum BridgeError {
    /// `amount` was zero. The substrate accepts zero-amount
    /// Intents as no-ops, but the off-chain UI should reject
    /// them as user error (the user paid gas for nothing).
    #[error("bridge amount must be > 0")]
    ZeroAmount,

    /// `amount` was below the dust threshold. Soft warning;
    /// callers may choose to override.
    #[error("bridge amount {amount} below dust threshold {threshold}")]
    BelowDustThreshold {
        /// Amount the caller tried to bridge.
        amount: Balance,
        /// Configured dust threshold.
        threshold: Balance,
    },

    /// `merkle_path` was empty.
    #[error("merkle_path must not be empty")]
    EmptyMerklePath,

    /// `merkle_path` exceeded the maximum reasonable size.
    /// Bound at 4 KiB; full L2 nullifier-tree paths fit in
    /// well under 1 KiB.
    #[error("merkle_path exceeds maximum {max} bytes: got {got}")]
    MerklePathTooLong {
        /// Configured maximum.
        max: usize,
        /// Observed length.
        got: usize,
    },

    /// `merkle_path` length was not a multiple of 32 bytes
    /// (each level of the path is one 32-byte digest).
    /// Phase-1 byte-shape check; phase-2 will additionally
    /// validate the path proves inclusion under the
    /// committed L2 state root.
    #[error("merkle_path length {got} is not a multiple of 32 bytes")]
    MerklePathAlignment {
        /// Observed length.
        got: usize,
    },
}

/// Errors emitted by [`verify_burn_inclusion`]. Per IQ-008, every
/// path-shape misalignment and every root mismatch surfaces here so
/// the substrate apply arm can map this set 1-to-1 onto
/// `ExecutionError::L2BurnMerkleProofRejected`.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum MerkleError {
    /// `merkle_path` length was not a multiple of 32 bytes. The byte-
    /// shape gate in [`L2BurnProvenPayload::validate`] should catch
    /// this earlier; the verifier rejects again so the substrate path
    /// is self-sufficient (no implicit precondition on the off-chain
    /// validator).
    #[error("merkle_path length {got} is not a multiple of 32 bytes")]
    PathAlignment {
        /// Observed length.
        got: usize,
    },
    /// `merkle_path` declared more levels than the supported maximum
    /// ([`MAX_BURN_TREE_LEVELS`]). Caps the verifier's work + matches
    /// the IQ-008 sanity bound.
    #[error("merkle path declares {got} levels, max is {max}")]
    TooManyLevels {
        /// Observed level count.
        got: usize,
        /// Configured maximum.
        max: usize,
    },
    /// `path_directions` had the wrong byte length for the declared
    /// level count: must be `ceil(levels / 8)`. A mismatched byte
    /// length is a wire malformation.
    #[error(
        "path_directions length {got} does not match the {levels}-level path \
         (expected ceil(levels / 8) = {expected})"
    )]
    DirectionsByteLength {
        /// Observed length.
        got: usize,
        /// Declared level count from the merkle_path.
        levels: usize,
        /// Expected length.
        expected: usize,
    },
    /// `path_directions` had non-zero bits past the declared level
    /// count. Padding MUST be zero — non-zero padding is a
    /// malleability vector (two different `path_directions` would
    /// otherwise verify the same proof, allowing forged
    /// `burn_id`s for the same logical burn). See IQ-008.
    #[error(
        "path_directions has non-zero padding past {levels} levels (byte {byte_index} bit {bit_index})"
    )]
    NonZeroDirectionPadding {
        /// Declared level count.
        levels: usize,
        /// Index of the offending byte in `path_directions`.
        byte_index: usize,
        /// Index of the offending bit within that byte.
        bit_index: u8,
    },
    /// Final computed root did not equal the committed L2 state root.
    /// The proof does not bind the claimed leaf to the state.
    #[error("merkle root mismatch: computed {computed:?}, expected {expected:?}")]
    RootMismatch {
        /// Root computed by walking `merkle_path` from the leaf.
        computed: [u8; 32],
        /// Root the verifier was supposed to match (from the L2
        /// registry record for this `(l2_chain_id_hash, batch_id)`).
        expected: [u8; 32],
    },
}

/// L1→L2 deposit payload. Validates the parameters a caller
/// is about to submit as `Intent::L1Lock`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct L1LockPayload {
    /// L1 address being debited.
    pub user_address: Address,
    /// L2 address being credited (may differ from
    /// `user_address`; some bridge UIs let the user specify
    /// a different L2 recipient).
    pub l2_recipient: Address,
    /// Amount being bridged (SUWAPPU base units).
    pub amount: Balance,
}

impl L1LockPayload {
    /// Validate the payload's shape. Returns `Err` on
    /// strict (always-rejected) issues + `Ok(Some(warning))`
    /// on soft warnings (dust threshold). Callers should
    /// surface soft warnings to the user before submitting.
    pub fn validate(&self) -> Result<Option<BridgeError>, BridgeError> {
        if self.amount == 0 {
            return Err(BridgeError::ZeroAmount);
        }
        if self.amount < DUST_THRESHOLD_SUWAPPU {
            return Ok(Some(BridgeError::BelowDustThreshold {
                amount: self.amount,
                threshold: DUST_THRESHOLD_SUWAPPU,
            }));
        }
        Ok(None)
    }
}

/// L2→L1 withdrawal payload. Validates the parameters a
/// caller is about to submit as `Intent::L2BurnProven`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct L2BurnProvenPayload {
    /// L2 batch id whose committed state root proves the burn.
    pub batch_id: u64,
    /// L1 address receiving the unlocked balance.
    pub recipient: Address,
    /// Amount being unlocked (SUWAPPU base units).
    pub amount: Balance,
    /// Merkle proof binding the burn to the proven L2 state.
    /// Phase 1: byte-shape check only. Phase 2: full
    /// inclusion verification against the L2 state root
    /// stored at `l2_registry_address`.
    pub merkle_path: Vec<u8>,
}

impl L2BurnProvenPayload {
    /// Validate the payload's shape. Phase-1 byte-shape
    /// checks; phase-2 will additionally invoke the Merkle
    /// proof verifier once the L2 state-root storage lands
    /// (G2.2 phase 2).
    pub fn validate(&self) -> Result<(), BridgeError> {
        if self.amount == 0 {
            return Err(BridgeError::ZeroAmount);
        }
        if self.merkle_path.is_empty() {
            return Err(BridgeError::EmptyMerklePath);
        }
        if self.merkle_path.len() > MAX_MERKLE_PATH_BYTES {
            return Err(BridgeError::MerklePathTooLong {
                max: MAX_MERKLE_PATH_BYTES,
                got: self.merkle_path.len(),
            });
        }
        if self.merkle_path.len() % 32 != 0 {
            return Err(BridgeError::MerklePathAlignment {
                got: self.merkle_path.len(),
            });
        }
        Ok(())
    }
}

/// The leaf-content side of an `L2BurnProven` merkle proof — the
/// fields that participate in the leaf hash per IQ-008. Constructed
/// from the `Intent::L2BurnProven` payload before calling
/// [`verify_burn_inclusion`]. Borrowed-slice shape so the verifier
/// can be called from the substrate without allocating a fresh
/// owned wrapper.
#[derive(Debug, Clone, Copy)]
pub struct BurnLeaf<'a> {
    /// Multi-L2 namespacing key (`Intent::L2BurnProven::l2_chain_id_hash`).
    pub l2_chain_id_hash: &'a [u8; 32],
    /// L2 batch id whose committed state root proves the burn.
    pub batch_id: u64,
    /// L1 address receiving the unlocked balance.
    pub recipient: &'a Address,
    /// Amount being unlocked.
    pub amount: Balance,
    /// Asset selector. `None` for native SUWAPPU; `Some` for a
    /// registered bridge asset.
    pub asset_id: Option<&'a [u8; 32]>,
}

impl<'a> BurnLeaf<'a> {
    /// Compute the canonical leaf hash per IQ-008's `suwappu-l2-burn-leaf-v1`
    /// scheme. Domain-tagged and length-distinguished so a leaf hash
    /// cannot collide with an inner-node hash.
    #[must_use]
    pub fn hash(&self) -> [u8; 32] {
        let mut h = blake3::Hasher::new();
        h.update(BURN_LEAF_DOMAIN_TAG);
        h.update(self.l2_chain_id_hash);
        h.update(&self.batch_id.to_be_bytes());
        h.update(self.recipient);
        h.update(&self.amount.to_be_bytes());
        let flag: u8 = u8::from(self.asset_id.is_some());
        h.update(&[flag]);
        if let Some(id) = self.asset_id {
            h.update(id);
        }
        h.finalize().into()
    }
}

/// Compute the inner-node hash of two children per IQ-008's
/// `suwappu-l2-burn-node-v1` scheme. Domain tag is length-distinguished
/// from the leaf tag.
#[must_use]
pub fn hash_inner_node(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(BURN_NODE_DOMAIN_TAG);
    h.update(left);
    h.update(right);
    h.finalize().into()
}

/// Verify that the supplied merkle proof binds `leaf` to
/// `expected_root` per IQ-008.
///
/// `merkle_path` is one 32-byte sibling per level, ordered from the
/// leaf upward. `path_directions` packs one bit per level, LSB-first
/// into bytes: bit 0 = level 0 (immediate sibling of the leaf),
/// bit 1 = level 1, etc.
/// - direction bit `0` → sibling is the RIGHT child (running hash is
///   the LEFT child) at this level
/// - direction bit `1` → sibling is the LEFT child (running hash is
///   the RIGHT child)
///
/// Bits past `levels = merkle_path.len() / 32` MUST be zero —
/// non-zero padding is a malleability vector and rejects with
/// [`MerkleError::NonZeroDirectionPadding`].
///
/// `merkle_path` empty + `path_directions` empty is a depth-0 tree
/// (the leaf IS the root). Verifier succeeds iff
/// `leaf.hash() == expected_root`.
///
/// # Errors
///
/// Returns [`MerkleError`] for every malformation: path-length
/// alignment, level-count cap, direction-byte length, non-zero
/// padding, and the final root mismatch.
pub fn verify_burn_inclusion(
    leaf: &BurnLeaf<'_>,
    merkle_path: &[u8],
    path_directions: &[u8],
    expected_root: &[u8; 32],
) -> Result<(), MerkleError> {
    if merkle_path.len() % 32 != 0 {
        return Err(MerkleError::PathAlignment {
            got: merkle_path.len(),
        });
    }
    let levels = merkle_path.len() / 32;
    if levels > MAX_BURN_TREE_LEVELS {
        return Err(MerkleError::TooManyLevels {
            got: levels,
            max: MAX_BURN_TREE_LEVELS,
        });
    }
    let expected_directions_bytes = levels.div_ceil(8);
    if path_directions.len() != expected_directions_bytes {
        return Err(MerkleError::DirectionsByteLength {
            got: path_directions.len(),
            levels,
            expected: expected_directions_bytes,
        });
    }
    // Reject non-zero padding past the declared level count. Without
    // this gate, a forged proof could vary the padding bits to mint
    // distinct `burn_id`s for the same logical burn (the nullifier
    // set keys on the full `merkle_path` + `path_directions`).
    for (byte_index, byte) in path_directions.iter().enumerate() {
        for bit_index in 0u8..8 {
            let bit_position = byte_index * 8 + bit_index as usize;
            if bit_position >= levels && (byte >> bit_index) & 1 != 0 {
                return Err(MerkleError::NonZeroDirectionPadding {
                    levels,
                    byte_index,
                    bit_index,
                });
            }
        }
    }

    let mut running = leaf.hash();
    for level in 0..levels {
        let sibling: [u8; 32] = merkle_path[level * 32..level * 32 + 32]
            .try_into()
            .expect("32-byte sibling guaranteed by the alignment check above");
        let direction_byte = path_directions[level / 8];
        let direction_bit = (direction_byte >> (level % 8)) & 1;
        running = if direction_bit == 0 {
            // Sibling on the right: running is the left child.
            hash_inner_node(&running, &sibling)
        } else {
            // Sibling on the left: running is the right child.
            hash_inner_node(&sibling, &running)
        };
    }

    if running != *expected_root {
        return Err(MerkleError::RootMismatch {
            computed: running,
            expected: *expected_root,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(b: u8) -> Address {
        [b; 20]
    }

    // ----- L1LockPayload -----

    #[test]
    fn l1_lock_valid_passes() {
        let p = L1LockPayload {
            user_address: addr(1),
            l2_recipient: addr(2),
            amount: 1_000_000,
        };
        assert_eq!(p.validate(), Ok(None));
    }

    #[test]
    fn l1_lock_zero_amount_rejected() {
        let p = L1LockPayload {
            user_address: addr(1),
            l2_recipient: addr(2),
            amount: 0,
        };
        assert_eq!(p.validate(), Err(BridgeError::ZeroAmount));
    }

    #[test]
    fn l1_lock_dust_returns_soft_warning() {
        let p = L1LockPayload {
            user_address: addr(1),
            l2_recipient: addr(2),
            amount: DUST_THRESHOLD_SUWAPPU - 1,
        };
        let result = p.validate();
        assert!(matches!(
            result,
            Ok(Some(BridgeError::BelowDustThreshold { .. }))
        ));
    }

    #[test]
    fn l1_lock_at_dust_threshold_passes_clean() {
        let p = L1LockPayload {
            user_address: addr(1),
            l2_recipient: addr(2),
            amount: DUST_THRESHOLD_SUWAPPU,
        };
        assert_eq!(p.validate(), Ok(None));
    }

    // ----- L2BurnProvenPayload -----

    #[test]
    fn l2_burn_valid_passes() {
        let p = L2BurnProvenPayload {
            batch_id: 7,
            recipient: addr(3),
            amount: 1_000_000,
            merkle_path: vec![0u8; 320], // 10 levels × 32 B
        };
        assert!(p.validate().is_ok());
    }

    #[test]
    fn l2_burn_zero_amount_rejected() {
        let p = L2BurnProvenPayload {
            batch_id: 7,
            recipient: addr(3),
            amount: 0,
            merkle_path: vec![0u8; 320],
        };
        assert_eq!(p.validate(), Err(BridgeError::ZeroAmount));
    }

    #[test]
    fn l2_burn_empty_merkle_path_rejected() {
        let p = L2BurnProvenPayload {
            batch_id: 7,
            recipient: addr(3),
            amount: 1_000_000,
            merkle_path: vec![],
        };
        assert_eq!(p.validate(), Err(BridgeError::EmptyMerklePath));
    }

    #[test]
    fn l2_burn_oversized_merkle_path_rejected() {
        let p = L2BurnProvenPayload {
            batch_id: 7,
            recipient: addr(3),
            amount: 1_000_000,
            merkle_path: vec![0u8; MAX_MERKLE_PATH_BYTES + 32],
        };
        assert!(matches!(
            p.validate(),
            Err(BridgeError::MerklePathTooLong { .. })
        ));
    }

    #[test]
    fn l2_burn_misaligned_merkle_path_rejected() {
        let p = L2BurnProvenPayload {
            batch_id: 7,
            recipient: addr(3),
            amount: 1_000_000,
            merkle_path: vec![0u8; 333], // not a multiple of 32
        };
        assert!(matches!(
            p.validate(),
            Err(BridgeError::MerklePathAlignment { got: 333 })
        ));
    }

    #[test]
    fn l2_burn_at_max_merkle_path_passes() {
        let p = L2BurnProvenPayload {
            batch_id: 7,
            recipient: addr(3),
            amount: 1_000_000,
            merkle_path: vec![0u8; MAX_MERKLE_PATH_BYTES],
        };
        assert!(p.validate().is_ok());
    }

    #[test]
    fn l2_burn_minimum_merkle_path_passes() {
        // Single level (32-byte path) is the minimum.
        let p = L2BurnProvenPayload {
            batch_id: 7,
            recipient: addr(3),
            amount: 1_000_000,
            merkle_path: vec![0u8; 32],
        };
        assert!(p.validate().is_ok());
    }

    // ----- verify_burn_inclusion (IQ-008) -----

    fn leaf<'a>(
        chain: &'a [u8; 32],
        batch: u64,
        recipient: &'a Address,
        amount: Balance,
        asset: Option<&'a [u8; 32]>,
    ) -> BurnLeaf<'a> {
        BurnLeaf {
            l2_chain_id_hash: chain,
            batch_id: batch,
            recipient,
            amount,
            asset_id: asset,
        }
    }

    /// Depth-0 tree: the leaf IS the root. `merkle_path` and
    /// `path_directions` are both empty; verification reduces to
    /// "leaf hash equals expected root".
    #[test]
    fn depth_zero_inclusion_accepts() {
        let chain = [0u8; 32];
        let recipient = addr(3);
        let l = leaf(&chain, 7, &recipient, 1_000_000, None);
        let root = l.hash();
        assert_eq!(
            verify_burn_inclusion(&l, &[], &[], &root),
            Ok(()),
            "depth-0 (leaf == root) must accept"
        );
    }

    /// Depth-0 tree with mismatched root must reject.
    #[test]
    fn depth_zero_inclusion_rejects_wrong_root() {
        let chain = [0u8; 32];
        let recipient = addr(3);
        let l = leaf(&chain, 7, &recipient, 1_000_000, None);
        let wrong_root = [0xffu8; 32];
        assert!(matches!(
            verify_burn_inclusion(&l, &[], &[], &wrong_root),
            Err(MerkleError::RootMismatch { .. })
        ));
    }

    /// Depth-1 tree: one sibling, one direction bit. Hand-roll the
    /// root and verify the proof clears.
    #[test]
    fn depth_one_inclusion_accepts_both_directions() {
        let chain = [0u8; 32];
        let recipient = addr(3);
        let l = leaf(&chain, 7, &recipient, 1_000_000, None);
        let leaf_hash = l.hash();
        let sibling = [0xaau8; 32];

        // Direction 0: leaf is LEFT, sibling is RIGHT.
        let root_left = hash_inner_node(&leaf_hash, &sibling);
        let path = sibling.to_vec();
        let directions = vec![0u8]; // bit 0 = 0
        assert_eq!(
            verify_burn_inclusion(&l, &path, &directions, &root_left),
            Ok(())
        );

        // Direction 1: leaf is RIGHT, sibling is LEFT.
        let root_right = hash_inner_node(&sibling, &leaf_hash);
        let directions = vec![0b0000_0001]; // bit 0 = 1
        assert_eq!(
            verify_burn_inclusion(&l, &path, &directions, &root_right),
            Ok(())
        );
    }

    /// Depth-1 tree with a flipped direction bit must reject (the
    /// computed root no longer matches because hash_inner_node is
    /// asymmetric in its two arguments — wrong child order produces
    /// a different parent).
    #[test]
    fn depth_one_inclusion_rejects_flipped_direction() {
        let chain = [0u8; 32];
        let recipient = addr(3);
        let l = leaf(&chain, 7, &recipient, 1_000_000, None);
        let leaf_hash = l.hash();
        let sibling = [0xaau8; 32];
        let root = hash_inner_node(&leaf_hash, &sibling); // leaf LEFT

        // Submit with direction bit FLIPPED to claim leaf is RIGHT.
        let directions = vec![0b0000_0001];
        assert!(matches!(
            verify_burn_inclusion(&l, &sibling, &directions, &root),
            Err(MerkleError::RootMismatch { .. })
        ));
    }

    /// Depth-3 tree (3 levels of siblings + directions) hand-rolled
    /// to verify the bit-packing works correctly across byte
    /// boundaries.
    #[test]
    fn depth_three_inclusion_accepts() {
        let chain = [1u8; 32];
        let recipient = addr(5);
        let asset = [9u8; 32];
        let l = leaf(&chain, 11, &recipient, 12345, Some(&asset));
        let leaf_hash = l.hash();

        let sib0 = [0x11u8; 32];
        let sib1 = [0x22u8; 32];
        let sib2 = [0x33u8; 32];

        // direction bits (LSB-first): 1, 0, 1 → byte = 0b0000_0101 = 0x05
        // Level 0: leaf RIGHT, sib0 LEFT → parent0 = inner(sib0, leaf)
        // Level 1: parent0 LEFT, sib1 RIGHT → parent1 = inner(parent0, sib1)
        // Level 2: parent1 RIGHT, sib2 LEFT → root = inner(sib2, parent1)
        let parent0 = hash_inner_node(&sib0, &leaf_hash);
        let parent1 = hash_inner_node(&parent0, &sib1);
        let root = hash_inner_node(&sib2, &parent1);

        let mut path = Vec::new();
        path.extend_from_slice(&sib0);
        path.extend_from_slice(&sib1);
        path.extend_from_slice(&sib2);
        let directions = vec![0x05u8];

        assert_eq!(verify_burn_inclusion(&l, &path, &directions, &root), Ok(()));
    }

    /// Misaligned `merkle_path` (not a multiple of 32) rejects up
    /// front before any hashing — independent of the byte-shape gate
    /// in `L2BurnProvenPayload::validate`.
    #[test]
    fn misaligned_path_rejects() {
        let chain = [0u8; 32];
        let recipient = addr(3);
        let l = leaf(&chain, 7, &recipient, 1, None);
        let result = verify_burn_inclusion(&l, &[0u8; 31], &[], &[0u8; 32]);
        assert!(matches!(
            result,
            Err(MerkleError::PathAlignment { got: 31 })
        ));
    }

    /// `merkle_path` declaring more levels than the supported maximum
    /// rejects.
    #[test]
    fn too_many_levels_rejects() {
        let chain = [0u8; 32];
        let recipient = addr(3);
        let l = leaf(&chain, 7, &recipient, 1, None);
        let levels = MAX_BURN_TREE_LEVELS + 1;
        let path = vec![0u8; levels * 32];
        let directions = vec![0u8; levels.div_ceil(8)];
        let result = verify_burn_inclusion(&l, &path, &directions, &[0u8; 32]);
        assert!(matches!(
            result,
            Err(MerkleError::TooManyLevels { got, max })
                if got == levels && max == MAX_BURN_TREE_LEVELS
        ));
    }

    /// `path_directions` of the wrong byte length rejects.
    #[test]
    fn wrong_directions_length_rejects() {
        let chain = [0u8; 32];
        let recipient = addr(3);
        let l = leaf(&chain, 7, &recipient, 1, None);
        let path = vec![0u8; 32]; // 1 level → expects 1 byte
        let directions = vec![]; // wrong: empty
        let result = verify_burn_inclusion(&l, &path, &directions, &[0u8; 32]);
        assert!(matches!(
            result,
            Err(MerkleError::DirectionsByteLength {
                got: 0,
                levels: 1,
                expected: 1
            })
        ));
    }

    /// Non-zero bits past the declared level count reject — the
    /// malleability gate from IQ-008.
    #[test]
    fn non_zero_direction_padding_rejects() {
        let chain = [0u8; 32];
        let recipient = addr(3);
        let l = leaf(&chain, 7, &recipient, 1, None);
        let path = vec![0u8; 32]; // 1 level
                                  // 1 byte of directions; level 0 = bit 0; padding bits 1..=7
                                  // must be zero. Flip bit 4 in the padding range.
        let directions = vec![0b0001_0000];
        let result = verify_burn_inclusion(&l, &path, &directions, &[0u8; 32]);
        assert!(matches!(
            result,
            Err(MerkleError::NonZeroDirectionPadding {
                levels: 1,
                byte_index: 0,
                bit_index: 4
            })
        ));
    }

    /// Changing the leaf (different recipient, amount, asset, etc.)
    /// against a fixed root must reject — the path is bound to a
    /// specific leaf.
    #[test]
    fn changed_leaf_against_fixed_root_rejects() {
        let chain = [0u8; 32];
        let recipient_a = addr(3);
        let recipient_b = addr(4);
        let l_a = leaf(&chain, 7, &recipient_a, 100, None);
        let l_b = leaf(&chain, 7, &recipient_b, 100, None);

        // Build the root against leaf A.
        let sib = [0xbbu8; 32];
        let root_a = hash_inner_node(&l_a.hash(), &sib);
        // Verify leaf B against root A — must reject.
        let result = verify_burn_inclusion(&l_b, &sib, &[0u8], &root_a);
        assert!(matches!(result, Err(MerkleError::RootMismatch { .. })));
    }

    /// Asset selector participates in the leaf — same fields but
    /// different `asset_id` produce different leaf hashes.
    #[test]
    fn asset_selector_changes_leaf_hash() {
        let chain = [0u8; 32];
        let recipient = addr(3);
        let asset = [0x77u8; 32];
        let l_native = leaf(&chain, 7, &recipient, 100, None);
        let l_asset = leaf(&chain, 7, &recipient, 100, Some(&asset));
        assert_ne!(
            l_native.hash(),
            l_asset.hash(),
            "asset selector must disambiguate the leaf hash"
        );
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    fn arb_addr() -> impl Strategy<Value = Address> {
        proptest::array::uniform20(any::<u8>())
    }

    fn arb_hash() -> impl Strategy<Value = [u8; 32]> {
        proptest::array::uniform32(any::<u8>())
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 256, .. ProptestConfig::default() })]

        /// **Soundness.** For any depth-0 leaf, the only `expected_root`
        /// that verifies is the leaf's own hash. Any other 32-byte
        /// root rejects.
        #[test]
        fn depth_zero_only_verifies_against_leaf_hash(
            chain in arb_hash(),
            batch_id in any::<u64>(),
            recipient in arb_addr(),
            amount in any::<u128>(),
            asset in proptest::option::of(arb_hash()),
            wrong_root in arb_hash(),
        ) {
            let asset_ref = asset.as_ref();
            let l = BurnLeaf {
                l2_chain_id_hash: &chain,
                batch_id,
                recipient: &recipient,
                amount,
                asset_id: asset_ref,
            };
            let leaf_hash = l.hash();
            // Accept against the leaf hash.
            prop_assert_eq!(
                verify_burn_inclusion(&l, &[], &[], &leaf_hash),
                Ok(())
            );
            // Reject against any different root.
            if wrong_root != leaf_hash {
                let r = verify_burn_inclusion(&l, &[], &[], &wrong_root);
                let is_root_mismatch = matches!(r, Err(MerkleError::RootMismatch { .. }));
                prop_assert!(is_root_mismatch, "expected RootMismatch, got {:?}", r);
            }
        }

        /// **Cross-root non-malleability.** A burn leaf valid against
        /// root R never verifies against any different root R'. The
        /// "construct" leg builds a single sibling path and the root
        /// against it; the "flip" leg perturbs ONE byte of the root.
        #[test]
        fn proof_against_root_r_rejects_against_root_r_prime(
            chain in arb_hash(),
            batch_id in any::<u64>(),
            recipient in arb_addr(),
            amount in any::<u128>(),
            sibling in arb_hash(),
            direction_bit in any::<bool>(),
            flip_byte in 0usize..32,
            flip_value in 1u8..=255,
        ) {
            let l = BurnLeaf {
                l2_chain_id_hash: &chain,
                batch_id,
                recipient: &recipient,
                amount,
                asset_id: None,
            };
            let leaf_hash = l.hash();
            let root = if direction_bit {
                hash_inner_node(&sibling, &leaf_hash)
            } else {
                hash_inner_node(&leaf_hash, &sibling)
            };
            let directions = vec![u8::from(direction_bit)];

            // Sanity: the proof verifies against the correct root.
            prop_assert_eq!(
                verify_burn_inclusion(&l, &sibling, &directions, &root),
                Ok(())
            );

            // Flip one byte of the root by XOR with a non-zero value.
            // Result must differ from the original root, so verification
            // MUST reject.
            let mut bad_root = root;
            bad_root[flip_byte] ^= flip_value;
            prop_assert_ne!(bad_root, root);
            let r = verify_burn_inclusion(&l, &sibling, &directions, &bad_root);
            let is_root_mismatch = matches!(r, Err(MerkleError::RootMismatch { .. }));
            prop_assert!(is_root_mismatch, "expected RootMismatch, got {:?}", r);
        }
    }
}
