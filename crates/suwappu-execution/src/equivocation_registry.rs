//! Equivocation registry storage (Track G G3.4 hardening:
//! replay defense for Equivocation / InvalidBatch slashes).
//!
//! Every successful `Intent::SlashSequencer { reason:
//! Equivocation | InvalidBatch, intent_hash, .. }` writes a
//! record keyed by `intent_hash` into this map. Subsequent
//! slashes with the same `intent_hash` reject with
//! `ExecutionError::EquivocationAlreadyRecorded`.
//!
//! ## What it defends against
//!
//! Before this registry, the safety-bond slash was
//! defended only by the bond's current balance: once
//! drained to zero, a second slash was a substrate-level
//! no-op. But if a sequencer operator (or anyone) tops the
//! safety bond back up via `Intent::DepositSafetyBond`, a
//! second slash with the SAME `intent_hash` would drain
//! the refill. The equivocation registry blocks this:
//! once a proof_hash is recorded, no further slash on the
//! same hash succeeds.
//!
//! ## Offense kinds
//!
//! Equivocation and InvalidBatch share the same registry —
//! one slot per `intent_hash` regardless of which offense
//! first claimed it. The `kind` byte records which
//! variant drove the slash for audit purposes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{error::ExecutionError, reserved};

/// Current encoding version.
pub const EQUIVOCATION_REGISTRY_VERSION: u32 = 1;

/// Encoded-header bytes: `version (4) + count (4) = 8`.
pub const ENCODED_HEADER_BYTES: usize = 4 + 4;

/// Encoded bytes per entry:
/// `proof_hash (32) + kind (1) + slashed_at_l1_height (8) = 41`.
pub const ENTRY_BYTES: usize = 32 + 1 + 8;

/// Which sequencer offense produced the slash. The substrate
/// records this for audit; both variants are treated
/// identically for replay-defense purposes (one slot per
/// `proof_hash` regardless of kind).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum OffenseKind {
    /// Sequencer signed two conflicting L2 batches.
    Equivocation,
    /// A subsequent batch's proof surfaced a consensus-rule
    /// violation in this batch.
    InvalidBatch,
}

impl OffenseKind {
    fn as_byte(&self) -> u8 {
        match self {
            OffenseKind::Equivocation => 0,
            OffenseKind::InvalidBatch => 1,
        }
    }

    fn from_byte(b: u8) -> Result<Self, ExecutionError> {
        match b {
            0 => Ok(OffenseKind::Equivocation),
            1 => Ok(OffenseKind::InvalidBatch),
            _ => Err(ExecutionError::CorruptStateRecord {
                addr: reserved::equivocation_registry_address(),
                reason: "equivocation offense kind out of range",
            }),
        }
    }
}

/// Per-offense record. Keyed by `proof_hash` (the
/// `SlashSequencer::intent_hash`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EquivocationRecord {
    /// Which offense drove the slash.
    pub kind: OffenseKind,
    /// L1 block height at which the slash landed. Daemon-
    /// supplied; the substrate has no view into L1 height
    /// directly. `0` for tests / no-height contexts.
    pub slashed_at_l1_height: u64,
}

/// Encode the equivocation map to the canonical byte
/// sequence. BTreeMap iteration is deterministic (ascending
/// proof_hash order).
pub fn encode(map: &BTreeMap<[u8; 32], EquivocationRecord>) -> Vec<u8> {
    let mut buf = Vec::with_capacity(ENCODED_HEADER_BYTES + map.len() * ENTRY_BYTES);
    buf.extend_from_slice(&EQUIVOCATION_REGISTRY_VERSION.to_be_bytes());
    buf.extend_from_slice(&(map.len() as u32).to_be_bytes());
    for (id, rec) in map {
        buf.extend_from_slice(id);
        buf.push(rec.kind.as_byte());
        buf.extend_from_slice(&rec.slashed_at_l1_height.to_be_bytes());
    }
    buf
}

/// Decode the byte sequence back into the equivocation map.
pub fn decode(bytes: &[u8]) -> Result<BTreeMap<[u8; 32], EquivocationRecord>, ExecutionError> {
    if bytes.is_empty() {
        return Ok(BTreeMap::new());
    }
    if bytes.len() < ENCODED_HEADER_BYTES {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::equivocation_registry_address(),
            reason: "equivocation registry header missing",
        });
    }
    let version = u32::from_be_bytes(bytes[0..4].try_into().unwrap());
    if version != EQUIVOCATION_REGISTRY_VERSION {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::equivocation_registry_address(),
            reason: "equivocation registry version mismatch",
        });
    }
    let count = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let expected_len = ENCODED_HEADER_BYTES + count * ENTRY_BYTES;
    if bytes.len() != expected_len {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::equivocation_registry_address(),
            reason: "equivocation registry size mismatch",
        });
    }
    let mut map = BTreeMap::new();
    let mut cursor = ENCODED_HEADER_BYTES;
    for _ in 0..count {
        let mut id = [0u8; 32];
        id.copy_from_slice(&bytes[cursor..cursor + 32]);
        cursor += 32;
        let kind = OffenseKind::from_byte(bytes[cursor])?;
        cursor += 1;
        let slashed_at_l1_height =
            u64::from_be_bytes(bytes[cursor..cursor + 8].try_into().unwrap());
        cursor += 8;
        if map
            .insert(
                id,
                EquivocationRecord {
                    kind,
                    slashed_at_l1_height,
                },
            )
            .is_some()
        {
            return Err(ExecutionError::CorruptStateRecord {
                addr: reserved::equivocation_registry_address(),
                reason: "equivocation registry has duplicate keys",
            });
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(kind: OffenseKind, h: u64) -> EquivocationRecord {
        EquivocationRecord {
            kind,
            slashed_at_l1_height: h,
        }
    }

    #[test]
    fn empty_round_trip() {
        let m: BTreeMap<[u8; 32], EquivocationRecord> = BTreeMap::new();
        let bytes = encode(&m);
        assert_eq!(decode(&bytes).unwrap(), m);
    }

    #[test]
    fn single_record_round_trip() {
        let mut m = BTreeMap::new();
        m.insert([0xaa; 32], rec(OffenseKind::Equivocation, 1_000));
        let bytes = encode(&m);
        assert_eq!(decode(&bytes).unwrap(), m);
    }

    #[test]
    fn multi_record_round_trip_deterministic_order() {
        let mut m1 = BTreeMap::new();
        m1.insert([0x01; 32], rec(OffenseKind::Equivocation, 100));
        m1.insert([0x02; 32], rec(OffenseKind::InvalidBatch, 200));
        m1.insert([0x03; 32], rec(OffenseKind::Equivocation, 300));

        let mut m2 = BTreeMap::new();
        m2.insert([0x03; 32], rec(OffenseKind::Equivocation, 300));
        m2.insert([0x01; 32], rec(OffenseKind::Equivocation, 100));
        m2.insert([0x02; 32], rec(OffenseKind::InvalidBatch, 200));

        assert_eq!(encode(&m1), encode(&m2));
        assert_eq!(decode(&encode(&m1)).unwrap(), m1);
    }

    #[test]
    fn offense_kind_round_trips() {
        let mut m = BTreeMap::new();
        m.insert([0x01; 32], rec(OffenseKind::Equivocation, 100));
        m.insert([0x02; 32], rec(OffenseKind::InvalidBatch, 200));
        let decoded = decode(&encode(&m)).unwrap();
        assert_eq!(
            decoded.get(&[0x01; 32]).unwrap().kind,
            OffenseKind::Equivocation
        );
        assert_eq!(
            decoded.get(&[0x02; 32]).unwrap().kind,
            OffenseKind::InvalidBatch
        );
    }

    #[test]
    fn decode_rejects_short_header() {
        let bytes = vec![0u8; 4];
        assert!(matches!(
            decode(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }

    #[test]
    fn decode_rejects_version_mismatch() {
        let mut bytes = vec![];
        bytes.extend_from_slice(&999u32.to_be_bytes());
        bytes.extend_from_slice(&0u32.to_be_bytes());
        assert!(matches!(
            decode(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }

    #[test]
    fn decode_rejects_size_mismatch() {
        let mut bytes = vec![];
        bytes.extend_from_slice(&EQUIVOCATION_REGISTRY_VERSION.to_be_bytes());
        bytes.extend_from_slice(&1u32.to_be_bytes());
        // Header claims 1 entry but no bytes follow.
        assert!(matches!(
            decode(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }

    #[test]
    fn decode_rejects_invalid_offense_kind() {
        let mut bytes = vec![];
        bytes.extend_from_slice(&EQUIVOCATION_REGISTRY_VERSION.to_be_bytes());
        bytes.extend_from_slice(&1u32.to_be_bytes());
        bytes.extend_from_slice(&[0xaa; 32]); // proof_hash
        bytes.push(99); // invalid offense kind
        bytes.extend_from_slice(&100u64.to_be_bytes());
        assert!(matches!(
            decode(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }
}
