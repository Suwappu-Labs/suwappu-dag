//! Validator unbonding registry.
//!
//! Holds delegator-initiated undelegations during the
//! `EXIT_COOLDOWN_BLOCKS` window between
//! `Intent::UndelegateBegin` (move delegation → unbonding) and
//! `Intent::UndelegateClaim` (drain matured unbondings to the
//! delegator).
//!
//! ## Why a separate registry from `delegation_registry`
//!
//! Active delegations and pending unbondings have different
//! risk surfaces: an Active delegation continues to back the
//! validator (rewards + slashing), while an unbonding
//! delegation is in cool-off (no rewards, no further
//! delegation accumulation, but still slashable during the
//! window). Keeping them in separate registries makes the
//! state surface auditable and avoids ambiguity at slashing
//! time.
//!
//! ## Storage shape
//!
//! Flat list of `(validator_id, delegator, unbonding_height,
//! amount)` tuples. Canonical ordering: ascending
//! `(validator_id, delegator, unbonding_height)`.
//!
//! ## Encoding (V1)
//!
//! ```text
//! u32::BE(VERSION = 1) ||
//! u32::BE(count) ||
//!   foreach (validator_id, delegator, unbonding_height, amount) in canonical order:
//!     validator_id (u32::BE, 4 B) ||
//!     delegator_address (20 B) ||
//!     unbonding_height (u64::BE, 8 B) ||
//!     amount (u64::BE, 8 B)
//! ```
//!
//! Per-entry fixed width = 40 B. Header = 8 B.

use std::collections::BTreeMap;

use crate::{error::ExecutionError, reserved, substrate::Address};

/// Current encoding version.
pub const UNBONDING_REGISTRY_VERSION: u32 = 1;

/// Header bytes: `version (4) + count (4) = 8`.
pub const ENCODED_HEADER_BYTES: usize = 4 + 4;

/// Per-entry fixed bytes: `validator_id (4) + delegator (20)
/// + unbonding_height (8) + amount (8) = 40`.
pub const ENTRY_BYTES: usize = 4 + 20 + 8 + 8;

/// Decoded unbonding map: `(validator_id, delegator,
/// unbonding_height) → amount`.
pub type UnbondingMap = BTreeMap<(u32, Address, u64), u64>;

/// Encode the unbonding map to canonical bytes. Zero-amount
/// entries are skipped (matches `delegation_registry`).
pub fn encode(map: &UnbondingMap) -> Vec<u8> {
    let mut buf = Vec::new();
    let entries: Vec<_> = map.iter().filter(|(_, &amt)| amt > 0).collect();
    buf.extend_from_slice(&UNBONDING_REGISTRY_VERSION.to_be_bytes());
    buf.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for ((vid, delegator, height), amount) in entries {
        buf.extend_from_slice(&vid.to_be_bytes());
        buf.extend_from_slice(delegator);
        buf.extend_from_slice(&height.to_be_bytes());
        buf.extend_from_slice(&amount.to_be_bytes());
    }
    buf
}

/// Decode the byte sequence back into an unbonding map.
pub fn decode(bytes: &[u8]) -> Result<UnbondingMap, ExecutionError> {
    if bytes.is_empty() {
        return Ok(BTreeMap::new());
    }
    if bytes.len() < ENCODED_HEADER_BYTES {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::validator_unbonding_registry_address(),
            reason: "unbonding registry header missing",
        });
    }
    let version = u32::from_be_bytes(bytes[0..4].try_into().unwrap());
    if version != UNBONDING_REGISTRY_VERSION {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::validator_unbonding_registry_address(),
            reason: "unbonding registry version mismatch",
        });
    }
    let count = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let expected_len = ENCODED_HEADER_BYTES + count * ENTRY_BYTES;
    if bytes.len() != expected_len {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::validator_unbonding_registry_address(),
            reason: "unbonding registry length mismatch",
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
        let height = u64::from_be_bytes(bytes[cursor..cursor + 8].try_into().unwrap());
        cursor += 8;
        let amount = u64::from_be_bytes(bytes[cursor..cursor + 8].try_into().unwrap());
        cursor += 8;
        if amount == 0 {
            return Err(ExecutionError::CorruptStateRecord {
                addr: reserved::validator_unbonding_registry_address(),
                reason: "unbonding registry has zero-amount entry",
            });
        }
        if map.insert((vid, delegator, height), amount).is_some() {
            return Err(ExecutionError::CorruptStateRecord {
                addr: reserved::validator_unbonding_registry_address(),
                reason: "unbonding registry has duplicate (vid, delegator, height) tuple",
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
        let m = UnbondingMap::new();
        assert_eq!(decode(&encode(&m)).unwrap(), m);
    }

    #[test]
    fn single_entry_round_trip() {
        let mut m = UnbondingMap::new();
        m.insert((7, d(1), 12345), 1_000_000);
        let decoded = decode(&encode(&m)).unwrap();
        assert_eq!(decoded.get(&(7, d(1), 12345)), Some(&1_000_000));
    }

    #[test]
    fn multiple_unbondings_same_pair_different_heights_coexist() {
        let mut m = UnbondingMap::new();
        m.insert((0, d(1), 100), 50);
        m.insert((0, d(1), 200), 75);
        m.insert((0, d(1), 300), 100);
        let decoded = decode(&encode(&m)).unwrap();
        assert_eq!(decoded.len(), 3);
        assert_eq!(decoded.get(&(0, d(1), 100)), Some(&50));
        assert_eq!(decoded.get(&(0, d(1), 200)), Some(&75));
        assert_eq!(decoded.get(&(0, d(1), 300)), Some(&100));
    }

    #[test]
    fn zero_amount_entries_are_dropped_on_encode() {
        let mut m = UnbondingMap::new();
        m.insert((1, d(1), 100), 0);
        m.insert((1, d(1), 200), 5);
        let bytes = encode(&m);
        // Header(8) + 1 entry (40) = 48
        assert_eq!(bytes.len(), 48);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.get(&(1, d(1), 100)), None);
        assert_eq!(decoded.get(&(1, d(1), 200)), Some(&5));
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
        bytes.extend_from_slice(&UNBONDING_REGISTRY_VERSION.to_be_bytes());
        bytes.extend_from_slice(&1u32.to_be_bytes());
        assert!(matches!(
            decode(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }

    #[test]
    fn decode_rejects_trailing_bytes() {
        let mut bytes = encode(&UnbondingMap::new());
        bytes.push(0xff);
        assert!(matches!(
            decode(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }
}
