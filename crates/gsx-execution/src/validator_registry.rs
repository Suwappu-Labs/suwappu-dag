//! Validator Ring registry storage.
//!
//! Mirror of `authority_registry` for the Tier B Validator
//! set (per the strategic plan / Tokenomics §4: 200 Validator
//! Ring slots at TGE, expanding to 500 by Y5). Tracks
//! `validator_id → ValidatorRecord` mappings in a single
//! bytes_state record at the reserved
//! `validator_registry_address`.
//!
//! ## Lifecycle (mirrors AuthorityRegistry)
//!
//! - `AdmitValidator`: insert a new record in `Active`.
//!   Rejects on duplicate slot; pubkey widths capped.
//! - `ExitValidator`: `Active → Exiting` (voluntary).
//!   Daemon-side rotates set at next epoch boundary.
//! - `EjectValidator`: any → Ejected (forced).
//!
//! ## Why a separate module from authority_registry
//!
//! The Authority Ring (40 slots, joint-quorum safety) and
//! Validator Ring (200 slots, PoS economic security) have
//! independent on-chain identity spaces. A single
//! `authority_id` namespace can't represent both rings
//! cleanly. Separate registries also let the daemon's
//! quorum-threshold logic compute per-ring active counts
//! without filtering.
//!
//! ## Encoding (same shape as authority_registry v1)
//!
//! ```text
//! u32::BE(VERSION = 1) ||
//! u32::BE(count) ||
//!   foreach (validator_id, rec) in ascending validator_id order:
//!     validator_id (u32::BE, 4 B) ||
//!     stake_gsx (u64::BE, 8 B) ||
//!     status (1 B: 0=Active, 1=Exiting, 2=Ejected) ||
//!     u32::BE(mldsa_pk_len) || mldsa_pk ||
//!     u32::BE(bls_pk_len) || bls_pk
//! ```

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{error::ExecutionError, reserved};

/// Current encoding version. Bumped to v2 to add the
/// `deposited_stake` field tracking real economic bonding
/// per-slot. v1 records decode with `deposited_stake = 0`.
pub const VALIDATOR_REGISTRY_VERSION: u32 = 2;

/// Legacy v1 encoding version. The decoder still accepts v1
/// bytes; the encoder always emits v2.
pub const VALIDATOR_REGISTRY_VERSION_V1: u32 = 1;

/// Same caps as authority_registry — canonical ML-DSA-65 =
/// 1952 B, BLS12-381 G1 compressed = 48 B; defensive
/// headroom for parameter drift.
pub const MAX_MLDSA_PK_BYTES: usize = 2048;

/// Max BLS12-381 G1 public-key bytes.
pub const MAX_BLS_PK_BYTES: usize = 128;

/// Encoded-header bytes: `version (4) + count (4) = 8`.
pub const ENCODED_HEADER_BYTES: usize = 4 + 4;

/// V1 fixed per-entry overhead.
pub const ENTRY_FIXED_BYTES_V1: usize = 4 + 8 + 1 + 4 + 4;

/// V2 fixed per-entry overhead (V1 + `deposited_stake (8)`).
pub const ENTRY_FIXED_BYTES: usize = ENTRY_FIXED_BYTES_V1 + 8;

/// Lifecycle status for a Validator Ring slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ValidatorStatus {
    /// Active in the Validator Ring; full voting weight.
    Active,
    /// Exiting voluntarily.
    Exiting,
    /// Ejected on confirmed offense.
    Ejected,
}

impl ValidatorStatus {
    fn as_byte(&self) -> u8 {
        match self {
            ValidatorStatus::Active => 0,
            ValidatorStatus::Exiting => 1,
            ValidatorStatus::Ejected => 2,
        }
    }

    fn from_byte(b: u8) -> Result<Self, ExecutionError> {
        match b {
            0 => Ok(ValidatorStatus::Active),
            1 => Ok(ValidatorStatus::Exiting),
            2 => Ok(ValidatorStatus::Ejected),
            _ => Err(ExecutionError::CorruptStateRecord {
                addr: reserved::validator_registry_address(),
                reason: "validator status out of range",
            }),
        }
    }
}

/// Per-slot Validator Ring record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidatorRecord {
    /// ML-DSA-65 public key bytes.
    pub mldsa_public_key: Vec<u8>,
    /// BLS12-381 G1 compressed pubkey bytes.
    pub bls_public_key: Vec<u8>,
    /// Declared stake at admission (the minimum the
    /// candidate committed to bond when admitted).
    pub stake_gsx: u64,
    /// Actual bonded stake — running total credited via
    /// `Intent::DepositValidatorStake`. `0` for v1-decoded
    /// records.
    #[serde(default)]
    pub deposited_stake: u64,
    /// Current lifecycle status.
    pub status: ValidatorStatus,
}

/// Encode the validator map to the canonical v2 byte sequence.
pub fn encode(map: &BTreeMap<u32, ValidatorRecord>) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&VALIDATOR_REGISTRY_VERSION.to_be_bytes());
    buf.extend_from_slice(&(map.len() as u32).to_be_bytes());
    for (id, rec) in map {
        buf.extend_from_slice(&id.to_be_bytes());
        buf.extend_from_slice(&rec.stake_gsx.to_be_bytes());
        buf.extend_from_slice(&rec.deposited_stake.to_be_bytes());
        buf.push(rec.status.as_byte());
        buf.extend_from_slice(&(rec.mldsa_public_key.len() as u32).to_be_bytes());
        buf.extend_from_slice(&rec.mldsa_public_key);
        buf.extend_from_slice(&(rec.bls_public_key.len() as u32).to_be_bytes());
        buf.extend_from_slice(&rec.bls_public_key);
    }
    buf
}

/// Decode the byte sequence back into the validator map.
pub fn decode(bytes: &[u8]) -> Result<BTreeMap<u32, ValidatorRecord>, ExecutionError> {
    if bytes.is_empty() {
        return Ok(BTreeMap::new());
    }
    if bytes.len() < ENCODED_HEADER_BYTES {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::validator_registry_address(),
            reason: "validator registry header missing",
        });
    }
    let version = u32::from_be_bytes(bytes[0..4].try_into().unwrap());
    let v2 = match version {
        VALIDATOR_REGISTRY_VERSION => true,
        VALIDATOR_REGISTRY_VERSION_V1 => false,
        _ => {
            return Err(ExecutionError::CorruptStateRecord {
                addr: reserved::validator_registry_address(),
                reason: "validator registry version mismatch",
            });
        }
    };
    let count = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let entry_fixed_bytes = if v2 {
        ENTRY_FIXED_BYTES
    } else {
        ENTRY_FIXED_BYTES_V1
    };
    let mut map = BTreeMap::new();
    let mut cursor = ENCODED_HEADER_BYTES;
    for _ in 0..count {
        if cursor + entry_fixed_bytes > bytes.len() {
            return Err(ExecutionError::CorruptStateRecord {
                addr: reserved::validator_registry_address(),
                reason: "validator registry entry truncated",
            });
        }
        let validator_id = u32::from_be_bytes(bytes[cursor..cursor + 4].try_into().unwrap());
        cursor += 4;
        let stake_gsx = u64::from_be_bytes(bytes[cursor..cursor + 8].try_into().unwrap());
        cursor += 8;
        let deposited_stake = if v2 {
            let v = u64::from_be_bytes(bytes[cursor..cursor + 8].try_into().unwrap());
            cursor += 8;
            v
        } else {
            0
        };
        let status = ValidatorStatus::from_byte(bytes[cursor])?;
        cursor += 1;

        let mldsa_len = u32::from_be_bytes(bytes[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 4;
        if mldsa_len > MAX_MLDSA_PK_BYTES || cursor + mldsa_len > bytes.len() {
            return Err(ExecutionError::CorruptStateRecord {
                addr: reserved::validator_registry_address(),
                reason: "validator mldsa_pk length invalid",
            });
        }
        let mldsa_public_key = bytes[cursor..cursor + mldsa_len].to_vec();
        cursor += mldsa_len;

        if cursor + 4 > bytes.len() {
            return Err(ExecutionError::CorruptStateRecord {
                addr: reserved::validator_registry_address(),
                reason: "validator bls_pk length missing",
            });
        }
        let bls_len = u32::from_be_bytes(bytes[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 4;
        if bls_len > MAX_BLS_PK_BYTES || cursor + bls_len > bytes.len() {
            return Err(ExecutionError::CorruptStateRecord {
                addr: reserved::validator_registry_address(),
                reason: "validator bls_pk length invalid",
            });
        }
        let bls_public_key = bytes[cursor..cursor + bls_len].to_vec();
        cursor += bls_len;

        if map
            .insert(
                validator_id,
                ValidatorRecord {
                    mldsa_public_key,
                    bls_public_key,
                    stake_gsx,
                    deposited_stake,
                    status,
                },
            )
            .is_some()
        {
            return Err(ExecutionError::CorruptStateRecord {
                addr: reserved::validator_registry_address(),
                reason: "validator registry has duplicate validator_id",
            });
        }
    }
    if cursor != bytes.len() {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::validator_registry_address(),
            reason: "validator registry trailing bytes",
        });
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(stake: u64, status: ValidatorStatus) -> ValidatorRecord {
        ValidatorRecord {
            mldsa_public_key: vec![0xaa; 1952],
            bls_public_key: vec![0xbb; 48],
            stake_gsx: stake,
            deposited_stake: 0,
            status,
        }
    }

    #[test]
    fn empty_round_trip() {
        let m: BTreeMap<u32, ValidatorRecord> = BTreeMap::new();
        assert_eq!(decode(&encode(&m)).unwrap(), m);
    }

    #[test]
    fn single_record_round_trip() {
        let mut m = BTreeMap::new();
        m.insert(0, rec(3_000_000, ValidatorStatus::Active));
        assert_eq!(decode(&encode(&m)).unwrap(), m);
    }

    #[test]
    fn multi_record_round_trip_deterministic_order() {
        let mut m1 = BTreeMap::new();
        m1.insert(0, rec(1, ValidatorStatus::Active));
        m1.insert(50, rec(2, ValidatorStatus::Exiting));
        m1.insert(199, rec(3, ValidatorStatus::Ejected));

        let mut m2 = BTreeMap::new();
        m2.insert(199, rec(3, ValidatorStatus::Ejected));
        m2.insert(0, rec(1, ValidatorStatus::Active));
        m2.insert(50, rec(2, ValidatorStatus::Exiting));

        assert_eq!(encode(&m1), encode(&m2));
        assert_eq!(decode(&encode(&m1)).unwrap(), m1);
    }

    #[test]
    fn statuses_round_trip() {
        let mut m = BTreeMap::new();
        m.insert(0, rec(1, ValidatorStatus::Active));
        m.insert(1, rec(2, ValidatorStatus::Exiting));
        m.insert(2, rec(3, ValidatorStatus::Ejected));
        let decoded = decode(&encode(&m)).unwrap();
        assert_eq!(decoded.get(&0).unwrap().status, ValidatorStatus::Active);
        assert_eq!(decoded.get(&1).unwrap().status, ValidatorStatus::Exiting);
        assert_eq!(decoded.get(&2).unwrap().status, ValidatorStatus::Ejected);
    }

    #[test]
    fn decode_rejects_short_header() {
        assert!(matches!(
            decode(&[0u8; 4]),
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
        bytes.extend_from_slice(&VALIDATOR_REGISTRY_VERSION.to_be_bytes());
        bytes.extend_from_slice(&1u32.to_be_bytes());
        assert!(matches!(
            decode(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }

    #[test]
    fn decode_rejects_oversized_mldsa_pk() {
        let mut bytes = vec![];
        bytes.extend_from_slice(&VALIDATOR_REGISTRY_VERSION.to_be_bytes());
        bytes.extend_from_slice(&1u32.to_be_bytes());
        bytes.extend_from_slice(&0u32.to_be_bytes()); // validator_id
        bytes.extend_from_slice(&0u64.to_be_bytes()); // stake_gsx
        bytes.extend_from_slice(&0u64.to_be_bytes()); // deposited_stake (v2)
        bytes.push(0); // Active
        bytes.extend_from_slice(&((MAX_MLDSA_PK_BYTES + 1) as u32).to_be_bytes());
        assert!(matches!(
            decode(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }

    #[test]
    fn decode_rejects_trailing_bytes() {
        let m: BTreeMap<u32, ValidatorRecord> = BTreeMap::new();
        let mut bytes = encode(&m);
        bytes.push(0xff);
        assert!(matches!(
            decode(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }

    /// V1 bytes (no `deposited_stake`) decode cleanly,
    /// lifting `deposited_stake` to 0.
    #[test]
    fn v1_bytes_decode_to_v2_with_zero_deposited_stake() {
        let mut bytes = vec![];
        bytes.extend_from_slice(&VALIDATOR_REGISTRY_VERSION_V1.to_be_bytes());
        bytes.extend_from_slice(&1u32.to_be_bytes());
        bytes.extend_from_slice(&7u32.to_be_bytes()); // validator_id
        bytes.extend_from_slice(&3_000_000u64.to_be_bytes()); // stake
        bytes.push(0); // Active
        bytes.extend_from_slice(&1952u32.to_be_bytes());
        bytes.extend_from_slice(&[0xaa; 1952]);
        bytes.extend_from_slice(&48u32.to_be_bytes());
        bytes.extend_from_slice(&[0xbb; 48]);

        let decoded = decode(&bytes).unwrap();
        let rec = decoded.get(&7).unwrap();
        assert_eq!(rec.stake_gsx, 3_000_000);
        assert_eq!(rec.deposited_stake, 0);
        assert_eq!(rec.status, ValidatorStatus::Active);
    }

    #[test]
    fn deposited_stake_round_trips() {
        let mut m = BTreeMap::new();
        m.insert(
            0,
            ValidatorRecord {
                mldsa_public_key: vec![0xaa; 1952],
                bls_public_key: vec![0xbb; 48],
                stake_gsx: 3_000_000,
                deposited_stake: 2_500_000,
                status: ValidatorStatus::Active,
            },
        );
        let decoded = decode(&encode(&m)).unwrap();
        assert_eq!(decoded.get(&0).unwrap().deposited_stake, 2_500_000);
    }
}
