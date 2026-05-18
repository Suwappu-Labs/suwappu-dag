//! Sequencer-ejection registry storage (Track G G3.4
//! permissionless-fallback).
//!
//! Per the strategic plan Track G: after the L1 sequencer
//! misses a force-include deadline + the obligation has
//! been Slashed, a 10,000-L1-block grace window passes,
//! AFTER which any address can post `Intent::EjectSequencer`
//! and become the next sequencer for one slot.
//!
//! ## Substrate effect
//!
//! - Requires the named obligation to be in
//!   `ObligationStatus::Slashed`.
//! - Records the ejector address in the ejection registry,
//!   keyed by obligation_id.
//! - Replay defense: re-ejecting the same obligation is
//!   rejected (registry already has an entry).
//! - Pays the snitch bounty from treasury to the ejector
//!   address (reuses the existing `snitch_bounty_amount`
//!   from the slashing path).
//!
//! ## What this is NOT
//!
//! The substrate records the ejection event deterministically.
//! It does NOT track "current sequencer" — that's a daemon-
//! level concern. The daemon consults the ejection registry
//! to rotate the sequencer for the next slot. Substrate's job
//! is the deterministic record of who-ejected-which-obligation.
//!
//! ## 10k-block window
//!
//! The substrate has no view into L1 block height. The
//! daemon-level authority-quorum gate validates the 10k-block
//! delay BEFORE this Intent reaches `apply_intent`. The
//! substrate enforces only that the obligation was Slashed
//! (the prerequisite for ejection eligibility).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{error::ExecutionError, reserved, substrate::Address};

/// Current ejection-registry encoding version.
pub const EJECTION_REGISTRY_VERSION: u32 = 1;

/// Length of the encoded registry header:
/// `version (4) + count (4) = 8`.
pub const ENCODED_HEADER_BYTES: usize = 4 + 4;

/// Length of each entry in the encoded form:
/// `obligation_id (32) + ejector (20) = 52`.
pub const ENTRY_BYTES: usize = 32 + 20;

/// Sequencer-ejection record. One per Slashed obligation
/// that's been ejected. Keyed by obligation_id at the
/// `ejection_registry_address` reserved account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EjectionRecord {
    /// Address that posted the ejection. Receives the
    /// snitch bounty + (daemon-level) becomes the next
    /// sequencer slot's owner.
    pub ejector: Address,
}

/// Encode the ejection map to the canonical byte sequence.
pub fn encode(map: &BTreeMap<[u8; 32], EjectionRecord>) -> Vec<u8> {
    let mut buf = Vec::with_capacity(ENCODED_HEADER_BYTES + map.len() * ENTRY_BYTES);
    buf.extend_from_slice(&EJECTION_REGISTRY_VERSION.to_be_bytes());
    buf.extend_from_slice(&(map.len() as u32).to_be_bytes());
    for (id, rec) in map {
        buf.extend_from_slice(id);
        buf.extend_from_slice(&rec.ejector);
    }
    buf
}

/// Decode the byte sequence back into the ejection map.
pub fn decode(bytes: &[u8]) -> Result<BTreeMap<[u8; 32], EjectionRecord>, ExecutionError> {
    if bytes.is_empty() {
        return Ok(BTreeMap::new());
    }
    if bytes.len() < ENCODED_HEADER_BYTES {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::ejection_registry_address(),
            reason: "ejection registry header missing",
        });
    }
    let version = u32::from_be_bytes(bytes[0..4].try_into().unwrap());
    if version != EJECTION_REGISTRY_VERSION {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::ejection_registry_address(),
            reason: "ejection registry version mismatch",
        });
    }
    let count = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let expected_len = ENCODED_HEADER_BYTES + count * ENTRY_BYTES;
    if bytes.len() != expected_len {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::ejection_registry_address(),
            reason: "ejection registry size mismatch",
        });
    }
    let mut map = BTreeMap::new();
    let mut cursor = ENCODED_HEADER_BYTES;
    for _ in 0..count {
        let mut id = [0u8; 32];
        id.copy_from_slice(&bytes[cursor..cursor + 32]);
        cursor += 32;
        let mut ejector = [0u8; 20];
        ejector.copy_from_slice(&bytes[cursor..cursor + 20]);
        cursor += 20;
        map.insert(id, EjectionRecord { ejector });
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_round_trip() {
        let m: BTreeMap<[u8; 32], EjectionRecord> = BTreeMap::new();
        let bytes = encode(&m);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, m);
    }

    #[test]
    fn single_record_round_trip() {
        let mut m = BTreeMap::new();
        m.insert(
            [0xaa; 32],
            EjectionRecord {
                ejector: [0xbb; 20],
            },
        );
        let bytes = encode(&m);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, m);
    }

    #[test]
    fn multi_record_round_trip_deterministic_order() {
        let mut m1 = BTreeMap::new();
        m1.insert(
            [0x01; 32],
            EjectionRecord {
                ejector: [0xaa; 20],
            },
        );
        m1.insert(
            [0x02; 32],
            EjectionRecord {
                ejector: [0xbb; 20],
            },
        );
        m1.insert(
            [0x03; 32],
            EjectionRecord {
                ejector: [0xcc; 20],
            },
        );
        let bytes1 = encode(&m1);

        // Insert in different order — BTreeMap canonicalizes,
        // so encoded bytes match.
        let mut m2 = BTreeMap::new();
        m2.insert(
            [0x03; 32],
            EjectionRecord {
                ejector: [0xcc; 20],
            },
        );
        m2.insert(
            [0x01; 32],
            EjectionRecord {
                ejector: [0xaa; 20],
            },
        );
        m2.insert(
            [0x02; 32],
            EjectionRecord {
                ejector: [0xbb; 20],
            },
        );
        let bytes2 = encode(&m2);
        assert_eq!(bytes1, bytes2);
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
        // header claims 1 record but bytes ends here
        let mut bytes = vec![];
        bytes.extend_from_slice(&EJECTION_REGISTRY_VERSION.to_be_bytes());
        bytes.extend_from_slice(&1u32.to_be_bytes());
        assert!(matches!(
            decode(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }
}
