//! Validator delegation registry.
//!
//! Tracks delegator → validator stake routing per Tokenomics §4
//! (delegated PoS): users delegate SUWAPPU to a Validator Ring slot
//! to share in its rewards without running validator infrastructure
//! themselves. Delegated funds bond into `validator_stake_pool_address`
//! alongside the validator's own `deposited_stake` and count toward
//! the validator's effective stake for reward-share computation.
//!
//! ## Storage shape
//!
//! Stored at the reserved `validator_delegation_registry_address` as
//! a flat list of `(validator_id, delegator_address, amount)` tuples.
//! Canonical ordering: ascending `(validator_id, delegator_address)`.
//!
//! ## Encoding (V1)
//!
//! ```text
//! u32::BE(VERSION = 1) ||
//! u32::BE(count) ||
//!   foreach (validator_id, delegator, amount) in canonical order:
//!     validator_id (u32::BE, 4 B) ||
//!     delegator_address (20 B) ||
//!     amount (u64::BE, 8 B)
//! ```
//!
//! Per-entry fixed width = 32 B. Header = 8 B.

use std::collections::BTreeMap;

use crate::{error::ExecutionError, reserved, substrate::Address};

/// Current encoding version.
pub const DELEGATION_REGISTRY_VERSION: u32 = 1;

/// Header bytes: `version (4) + count (4) = 8`.
pub const ENCODED_HEADER_BYTES: usize = 4 + 4;

/// Per-entry fixed bytes: `validator_id (4) + delegator (20) + amount (8) = 32`.
pub const ENTRY_BYTES: usize = 4 + 20 + 8;

/// Decoded delegation map: `(validator_id, delegator) → amount`.
pub type DelegationMap = BTreeMap<(u32, Address), u64>;

/// Encode the delegation map to canonical bytes. Zero-amount
/// entries are skipped (zero-is-absent convention matches the
/// balance map).
pub fn encode(map: &DelegationMap) -> Vec<u8> {
    let mut buf = Vec::new();
    let entries: Vec<_> = map.iter().filter(|(_, &amt)| amt > 0).collect();
    buf.extend_from_slice(&DELEGATION_REGISTRY_VERSION.to_be_bytes());
    buf.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for ((vid, delegator), amount) in entries {
        buf.extend_from_slice(&vid.to_be_bytes());
        buf.extend_from_slice(delegator);
        buf.extend_from_slice(&amount.to_be_bytes());
    }
    buf
}

/// Decode the byte sequence back into a delegation map.
pub fn decode(bytes: &[u8]) -> Result<DelegationMap, ExecutionError> {
    if bytes.is_empty() {
        return Ok(BTreeMap::new());
    }
    if bytes.len() < ENCODED_HEADER_BYTES {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::validator_delegation_registry_address(),
            reason: "delegation registry header missing",
        });
    }
    let version = u32::from_be_bytes(bytes[0..4].try_into().unwrap());
    if version != DELEGATION_REGISTRY_VERSION {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::validator_delegation_registry_address(),
            reason: "delegation registry version mismatch",
        });
    }
    let count = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let expected_len = ENCODED_HEADER_BYTES + count * ENTRY_BYTES;
    if bytes.len() != expected_len {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::validator_delegation_registry_address(),
            reason: "delegation registry length mismatch",
        });
    }
    let mut map = BTreeMap::new();
    let mut cursor = ENCODED_HEADER_BYTES;
    for _ in 0..count {
        let vid = u32::from_be_bytes(bytes[cursor..cursor + 4].try_into().unwrap());
        cursor += 4;
        let mut delegator = [0u8; 20];
        delegator.copy_from_slice(&bytes[cursor..cursor + 20]);
        cursor += 20;
        let amount = u64::from_be_bytes(bytes[cursor..cursor + 8].try_into().unwrap());
        cursor += 8;
        if amount == 0 {
            return Err(ExecutionError::CorruptStateRecord {
                addr: reserved::validator_delegation_registry_address(),
                reason: "delegation registry has zero-amount entry",
            });
        }
        if map.insert((vid, delegator), amount).is_some() {
            return Err(ExecutionError::CorruptStateRecord {
                addr: reserved::validator_delegation_registry_address(),
                reason: "delegation registry has duplicate (validator, delegator) pair",
            });
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(seed: u8) -> Address {
        [seed; 20]
    }

    #[test]
    fn empty_round_trip() {
        let m = DelegationMap::new();
        assert_eq!(decode(&encode(&m)).unwrap(), m);
    }

    #[test]
    fn single_entry_round_trip() {
        let mut m = DelegationMap::new();
        m.insert((7, d(1)), 1_000_000);
        let decoded = decode(&encode(&m)).unwrap();
        assert_eq!(decoded.get(&(7, d(1))), Some(&1_000_000));
    }

    #[test]
    fn multi_entry_round_trip_canonical_order() {
        let mut m1 = DelegationMap::new();
        m1.insert((0, d(3)), 1);
        m1.insert((0, d(1)), 2);
        m1.insert((5, d(2)), 3);
        let mut m2 = DelegationMap::new();
        m2.insert((5, d(2)), 3);
        m2.insert((0, d(1)), 2);
        m2.insert((0, d(3)), 1);
        // Insertion-order independence — BTreeMap iterates in
        // ascending key order, so two maps with the same logical
        // contents encode identically.
        assert_eq!(encode(&m1), encode(&m2));
    }

    #[test]
    fn zero_amount_entries_are_dropped_on_encode() {
        let mut m = DelegationMap::new();
        m.insert((1, d(1)), 0);
        m.insert((1, d(2)), 5);
        let bytes = encode(&m);
        // Header(8) + 1 entry (32) = 40
        assert_eq!(bytes.len(), 40);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.get(&(1, d(1))), None);
        assert_eq!(decoded.get(&(1, d(2))), Some(&5));
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
        bytes.extend_from_slice(&DELEGATION_REGISTRY_VERSION.to_be_bytes());
        bytes.extend_from_slice(&1u32.to_be_bytes());
        // Claims 1 entry but no payload.
        assert!(matches!(
            decode(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }

    #[test]
    fn decode_rejects_trailing_bytes() {
        let mut bytes = encode(&DelegationMap::new());
        bytes.push(0xff);
        assert!(matches!(
            decode(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }
}
