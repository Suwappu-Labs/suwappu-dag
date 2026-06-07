//! Force-include obligation tracking (Track G G3.4, #103).
//!
//! Substrate-side support for the L1-quorum-enforced
//! force-inclusion mechanism per `docs/validator-sla-slashing.md`
//! §3 + the Track G section of the strategic plan.
//!
//! ## Flow
//!
//! 1. **User submits `Intent::L2ForceInclude { tx, deadline_l1_height,
//!    submitter, l2_nonce }`** when they suspect the sequencer is
//!    censoring their L2 transaction.
//! 2. **Substrate registers the obligation** at
//!    `reserved::force_include_registry_address()`, keyed by
//!    `obligation_id(tx, deadline_l1_height, submitter, l2_nonce)`
//!    — a deterministic BLAKE3 derivation that the snitch can
//!    compute independently to reference the obligation.
//! 3. **If the sequencer doesn't include the tx by the deadline**,
//!    any snitch (the submitter, typically) can submit
//!    `Intent::SlashSequencer { reason: MissedForceInclude,
//!    intent_hash: obligation_id }`. The substrate validates the
//!    obligation exists + is `Pending` + drains the sequencer's
//!    liveness bond. (The deadline check happens at the daemon's
//!    authority-quorum-vote gate before SlashSequencer reaches
//!    the substrate — per the §3 design that authority quorum
//!    authorizes slashing.)
//! 4. **If the sequencer DOES include the tx**, the substrate
//!    marks the obligation `Honored` and the obligation cannot
//!    be slashed against (replay defense + griefing defense).
//!
//! ## Replay defense
//!
//! Three layers per `docs/validator-sla-slashing.md` §3:
//!
//! - L1 dedup hash: this module's `obligation_id` collision-
//!   resists against re-submission via `(tx, deadline, submitter,
//!   l2_nonce)` uniqueness
//! - L2 nonce: the included tx must carry an L2 nonce; re-
//!   execution after deadline is a no-op at the STM level
//!   (enforced inside the SP1 STM, not here)
//! - Deadline expiry: obligations auto-evict at `deadline +
//!   1 day` (phase 2; phase 1 keeps them indefinitely for
//!   forensic audit)

use std::collections::BTreeMap;

use blake3::Hasher;
use serde::{Deserialize, Serialize};

use crate::{error::ExecutionError, reserved, substrate::Address};

/// Length of one encoded `ForceIncludeObligation` in bytes:
/// `tx_hash (32) + deadline_l1_height (8) + submitter (20) +
///  l2_nonce (8) + status (1) = 69`.
pub const OBLIGATION_BYTES: usize = 32 + 8 + 20 + 8 + 1;

/// Length of one encoded `(obligation_id, ForceIncludeObligation)`
/// pair: `id (32) + obligation (69) = 101`.
pub const ENCODED_ENTRY_BYTES: usize = 32 + OBLIGATION_BYTES;

/// Length of the encoded map header (`u32::BE(count)`).
pub const ENCODED_HEADER_BYTES: usize = 4;

/// Status of a registered force-include obligation. Drives the
/// substrate's accept/reject behavior for subsequent
/// `Intent::SlashSequencer` events referencing the same
/// `obligation_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ObligationStatus {
    /// Newly registered; sequencer must include by deadline.
    Pending,
    /// Sequencer included the tx; slashing rejected.
    Honored,
    /// Sequencer missed the deadline; slashing applied.
    Slashed,
}

impl ObligationStatus {
    /// One-byte wire encoding for the obligation map.
    fn as_byte(&self) -> u8 {
        match self {
            ObligationStatus::Pending => 0,
            ObligationStatus::Honored => 1,
            ObligationStatus::Slashed => 2,
        }
    }

    fn from_byte(b: u8) -> Result<Self, ExecutionError> {
        match b {
            0 => Ok(ObligationStatus::Pending),
            1 => Ok(ObligationStatus::Honored),
            2 => Ok(ObligationStatus::Slashed),
            _ => Err(ExecutionError::CorruptStateRecord {
                addr: reserved::force_include_registry_address(),
                reason: "force-include obligation status out of range",
            }),
        }
    }
}

/// A registered force-include obligation. Stored at the
/// reserved `force_include_registry_address` keyed by
/// `obligation_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForceIncludeObligation {
    /// BLAKE3 of the L2 tx bytes (the tx itself isn't stored to
    /// keep the obligation small).
    pub tx_hash: [u8; 32],
    /// L1 block height at which the obligation expires.
    pub deadline_l1_height: u64,
    /// Address that submitted the obligation (snitch-reward
    /// target per §3 of the SLA doc; production reads this for
    /// the treasury bounty payout).
    pub submitter: Address,
    /// Belt-and-suspenders L2 nonce dedup.
    pub l2_nonce: u64,
    /// Current status of the obligation.
    pub status: ObligationStatus,
}

/// Deterministic obligation identifier. Computed from the
/// L2ForceInclude Intent's user-supplied fields; the snitch
/// reconstructs the same id off-chain to reference the
/// obligation in `Intent::SlashSequencer::intent_hash`.
///
/// Recipe:
/// `BLAKE3("suwappu-force-include-v1" || u32_be(tx.len()) || tx ||
///         deadline_be(8) || submitter (20) || l2_nonce_be(8))`.
pub fn obligation_id(
    tx: &[u8],
    deadline_l1_height: u64,
    submitter: &Address,
    l2_nonce: u64,
) -> [u8; 32] {
    let mut h = Hasher::new();
    h.update(b"suwappu-force-include-v1");
    h.update(&(tx.len() as u32).to_be_bytes());
    h.update(tx);
    h.update(&deadline_l1_height.to_be_bytes());
    h.update(submitter);
    h.update(&l2_nonce.to_be_bytes());
    *h.finalize().as_bytes()
}

/// Hash the L2 tx bytes for storage. Same recipe as the
/// mempool's content-hash + same primitive (BLAKE3); keeps
/// the wire format stable across the surface.
pub fn tx_hash(tx: &[u8]) -> [u8; 32] {
    *blake3::hash(tx).as_bytes()
}

/// Encode the obligation map into the on-disk byte sequence.
/// Deterministic in BTreeMap ascending-key order.
pub fn encode_map(map: &BTreeMap<[u8; 32], ForceIncludeObligation>) -> Vec<u8> {
    let mut buf = Vec::with_capacity(ENCODED_HEADER_BYTES + map.len() * ENCODED_ENTRY_BYTES);
    buf.extend_from_slice(&(map.len() as u32).to_be_bytes());
    for (id, ob) in map {
        buf.extend_from_slice(id);
        buf.extend_from_slice(&ob.tx_hash);
        buf.extend_from_slice(&ob.deadline_l1_height.to_be_bytes());
        buf.extend_from_slice(&ob.submitter);
        buf.extend_from_slice(&ob.l2_nonce.to_be_bytes());
        buf.push(ob.status.as_byte());
    }
    buf
}

/// Decode the byte sequence back into a map.
pub fn decode_map(
    bytes: &[u8],
) -> Result<BTreeMap<[u8; 32], ForceIncludeObligation>, ExecutionError> {
    if bytes.is_empty() {
        return Ok(BTreeMap::new());
    }
    if bytes.len() < ENCODED_HEADER_BYTES {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::force_include_registry_address(),
            reason: "force-include map header missing",
        });
    }
    let count = u32::from_be_bytes(bytes[0..ENCODED_HEADER_BYTES].try_into().unwrap()) as usize;
    let expected_len = ENCODED_HEADER_BYTES + count * ENCODED_ENTRY_BYTES;
    if bytes.len() != expected_len {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::force_include_registry_address(),
            reason: "force-include map size mismatch",
        });
    }
    let mut map = BTreeMap::new();
    let mut cursor = ENCODED_HEADER_BYTES;
    for _ in 0..count {
        let mut id = [0u8; 32];
        id.copy_from_slice(&bytes[cursor..cursor + 32]);
        cursor += 32;
        let mut tx_hash = [0u8; 32];
        tx_hash.copy_from_slice(&bytes[cursor..cursor + 32]);
        cursor += 32;
        let deadline_l1_height = u64::from_be_bytes(bytes[cursor..cursor + 8].try_into().unwrap());
        cursor += 8;
        let mut submitter = [0u8; 20];
        submitter.copy_from_slice(&bytes[cursor..cursor + 20]);
        cursor += 20;
        let l2_nonce = u64::from_be_bytes(bytes[cursor..cursor + 8].try_into().unwrap());
        cursor += 8;
        let status = ObligationStatus::from_byte(bytes[cursor])?;
        cursor += 1;
        let ob = ForceIncludeObligation {
            tx_hash,
            deadline_l1_height,
            submitter,
            l2_nonce,
            status,
        };
        if map.insert(id, ob).is_some() {
            return Err(ExecutionError::CorruptStateRecord {
                addr: reserved::force_include_registry_address(),
                reason: "force-include map has duplicate ids",
            });
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ob(seed: u8, status: ObligationStatus) -> ForceIncludeObligation {
        ForceIncludeObligation {
            tx_hash: [seed; 32],
            deadline_l1_height: 1000 + seed as u64,
            submitter: [seed.wrapping_add(1); 20],
            l2_nonce: 5 + seed as u64,
            status,
        }
    }

    #[test]
    fn obligation_id_deterministic() {
        let id1 = obligation_id(b"hello", 100, &[0x01; 20], 5);
        let id2 = obligation_id(b"hello", 100, &[0x01; 20], 5);
        assert_eq!(id1, id2);
    }

    #[test]
    fn obligation_id_distinguishes_fields() {
        let base = obligation_id(b"hello", 100, &[0x01; 20], 5);
        let diff_tx = obligation_id(b"world", 100, &[0x01; 20], 5);
        let diff_dl = obligation_id(b"hello", 200, &[0x01; 20], 5);
        let diff_sub = obligation_id(b"hello", 100, &[0x02; 20], 5);
        let diff_nonce = obligation_id(b"hello", 100, &[0x01; 20], 7);
        for d in [diff_tx, diff_dl, diff_sub, diff_nonce] {
            assert_ne!(base, d);
        }
    }

    #[test]
    fn obligation_id_length_prefixes_tx() {
        // Defends against ambiguity: tx="abc" || deadline=0x..."d"
        // should NOT collide with tx="abcd" || deadline=0x...
        // Length-prefixing tx makes that impossible.
        let id_a = obligation_id(b"abc", 0x64, &[0x01; 20], 0);
        let id_b = obligation_id(b"abcd", 0x00, &[0x01; 20], 0);
        assert_ne!(id_a, id_b);
    }

    #[test]
    fn empty_map_round_trips() {
        let m = BTreeMap::new();
        let bytes = encode_map(&m);
        assert_eq!(bytes, [0, 0, 0, 0]);
        let m2 = decode_map(&bytes).unwrap();
        assert_eq!(m, m2);
    }

    #[test]
    fn single_entry_round_trips() {
        let mut m = BTreeMap::new();
        m.insert([0xaa; 32], ob(1, ObligationStatus::Pending));
        let bytes = encode_map(&m);
        assert_eq!(bytes.len(), ENCODED_HEADER_BYTES + ENCODED_ENTRY_BYTES);
        let m2 = decode_map(&bytes).unwrap();
        assert_eq!(m, m2);
    }

    #[test]
    fn multi_entry_round_trips_all_statuses() {
        let mut m = BTreeMap::new();
        m.insert([0x01; 32], ob(1, ObligationStatus::Pending));
        m.insert([0x02; 32], ob(2, ObligationStatus::Honored));
        m.insert([0x03; 32], ob(3, ObligationStatus::Slashed));
        let bytes = encode_map(&m);
        assert_eq!(bytes.len(), ENCODED_HEADER_BYTES + 3 * ENCODED_ENTRY_BYTES);
        let m2 = decode_map(&bytes).unwrap();
        assert_eq!(m, m2);
    }

    #[test]
    fn decode_rejects_corrupt_status() {
        // Write a valid header (1 entry) + a payload with an
        // out-of-range status byte (255).
        let mut bytes = vec![0, 0, 0, 1];
        bytes.extend_from_slice(&[0u8; OBLIGATION_BYTES + 32 - 1]); // pad up to last byte
        bytes.push(255); // bogus status
        assert!(matches!(
            decode_map(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }

    #[test]
    fn decode_rejects_size_mismatch() {
        // Claim 2 entries but provide bytes for 1.
        let mut bytes = vec![0, 0, 0, 2];
        bytes.extend_from_slice(&[0u8; ENCODED_ENTRY_BYTES]);
        assert!(matches!(
            decode_map(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }

    #[test]
    fn tx_hash_is_deterministic() {
        assert_eq!(tx_hash(b"abc"), tx_hash(b"abc"));
        assert_ne!(tx_hash(b"abc"), tx_hash(b"abd"));
    }
}
