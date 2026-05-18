//! Authority Ring registry storage.
//!
//! Tracks the active Authority Ring set — paper §4.2 — in
//! a single bytes_state record at the reserved
//! `authority_registry_address`. Each `authority_id` slot
//! maps to an `AuthorityRecord` carrying:
//!
//! - `mldsa_public_key` (ML-DSA-65, ~1952 B)
//! - `bls_public_key` (BLS12-381 G1 compressed, 48 B)
//! - declared stake (`stake_gsx`)
//! - actual bonded stake (`deposited_stake`)
//! - exit-cooldown anchor (`exit_block_height`)
//! - current `AuthorityStatus`: Active / Exiting / Ejected
//!
//! ## Encoding
//!
//! ```text
//! u32::BE(VERSION = 3) ||
//! u32::BE(count) ||
//!   foreach (authority_id, rec) in ascending authority_id order:
//!     authority_id (u32::BE, 4 B) ||
//!     stake_gsx (u64::BE, 8 B) ||
//!     deposited_stake (u64::BE, 8 B) ||
//!     exit_block_height (u64::BE, 8 B) ||
//!     status (1 B: 0=Active, 1=Exiting, 2=Ejected) ||
//!     u32::BE(mldsa_pk_len) || mldsa_pk ||
//!     u32::BE(bls_pk_len) || bls_pk
//! ```

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{error::ExecutionError, reserved};

/// Current encoding version. Bumped to v3 to add the
/// `exit_block_height` field that anchors the
/// `EXIT_COOLDOWN_BLOCKS` gate on `WithdrawAuthorityStake`.
/// v2 records decode with `exit_block_height = 0`. v1 records
/// decode with `deposited_stake = 0` and `exit_block_height = 0`.
pub const AUTHORITY_REGISTRY_VERSION: u32 = 3;

/// Legacy v2 encoding version. Decoder accepts v2 bytes
/// (lifts `exit_block_height` to 0).
pub const AUTHORITY_REGISTRY_VERSION_V2: u32 = 2;

/// Legacy v1 encoding version. Decoder accepts v1 bytes
/// (lifts both `deposited_stake` and `exit_block_height` to 0).
pub const AUTHORITY_REGISTRY_VERSION_V1: u32 = 1;

/// Max ML-DSA-65 public-key bytes accepted in
/// `AdmitAuthority` (canonical is 1952 B; cap at 2048 B for
/// defensive headroom against parameter drift).
pub const MAX_MLDSA_PK_BYTES: usize = 2048;

/// Max BLS12-381 G1 public-key bytes accepted (canonical is
/// 48 B compressed; cap at 128 B for headroom).
pub const MAX_BLS_PK_BYTES: usize = 128;

/// Encoded-header bytes: `version (4) + count (4) = 8`.
pub const ENCODED_HEADER_BYTES: usize = 4 + 4;

/// V1 fixed per-entry overhead: `authority_id (4) +
/// stake_gsx (8) + status (1) + 2 × u32-length-prefixes (8)
/// = 21 B` (plus the variable mldsa/bls pubkey bytes).
pub const ENTRY_FIXED_BYTES_V1: usize = 4 + 8 + 1 + 4 + 4;

/// V2 fixed per-entry overhead: V1 + `deposited_stake (8) = 29 B`.
pub const ENTRY_FIXED_BYTES_V2: usize = ENTRY_FIXED_BYTES_V1 + 8;

/// V3 fixed per-entry overhead: V2 + `exit_block_height (8) = 37 B`.
pub const ENTRY_FIXED_BYTES: usize = ENTRY_FIXED_BYTES_V2 + 8;

/// Lifecycle status for an Authority Ring slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum AuthorityStatus {
    /// Active in the Authority Ring; full voting weight.
    Active,
    /// Exiting voluntarily — `ExitAuthority` posted.
    /// Daemon-side set rotation removes the slot at the
    /// next epoch boundary.
    Exiting,
    /// Ejected on confirmed equivocation. 100% bonded
    /// stake forfeit (handled separately via the
    /// slashing waterfall).
    Ejected,
}

impl AuthorityStatus {
    fn as_byte(&self) -> u8 {
        match self {
            AuthorityStatus::Active => 0,
            AuthorityStatus::Exiting => 1,
            AuthorityStatus::Ejected => 2,
        }
    }

    fn from_byte(b: u8) -> Result<Self, ExecutionError> {
        match b {
            0 => Ok(AuthorityStatus::Active),
            1 => Ok(AuthorityStatus::Exiting),
            2 => Ok(AuthorityStatus::Ejected),
            _ => Err(ExecutionError::CorruptStateRecord {
                addr: reserved::authority_registry_address(),
                reason: "authority status out of range",
            }),
        }
    }
}

/// Per-slot Authority Ring record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityRecord {
    /// ML-DSA-65 public key bytes (canonical 1952 B).
    pub mldsa_public_key: Vec<u8>,
    /// BLS12-381 G1 compressed pubkey bytes (canonical 48 B).
    pub bls_public_key: Vec<u8>,
    /// Declared stake at admission (the minimum the
    /// candidate committed to bond when admitted). Compare
    /// against `deposited_stake` to check whether the slot
    /// is fully funded.
    pub stake_gsx: u64,
    /// Actual bonded stake — running total credited via
    /// `Intent::DepositAuthorityStake`. Drained on
    /// `EjectAuthority` (slashing waterfall) or
    /// `WithdrawAuthorityStake` (graceful exit). `0` for
    /// v1-decoded records.
    #[serde(default)]
    pub deposited_stake: u64,
    /// Block height at which `ExitAuthority` flipped this
    /// slot to `Exiting`. The exit cooldown gate on
    /// `WithdrawAuthorityStake` enforces
    /// `current_block_height >= exit_block_height +
    /// EXIT_COOLDOWN_BLOCKS`. `0` for slots that never
    /// exited or for v1/v2-decoded records.
    #[serde(default)]
    pub exit_block_height: u64,
    /// Current lifecycle status.
    pub status: AuthorityStatus,
}

/// Encode the authority map to the canonical v3 byte sequence.
pub fn encode(map: &BTreeMap<u32, AuthorityRecord>) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&AUTHORITY_REGISTRY_VERSION.to_be_bytes());
    buf.extend_from_slice(&(map.len() as u32).to_be_bytes());
    for (id, rec) in map {
        buf.extend_from_slice(&id.to_be_bytes());
        buf.extend_from_slice(&rec.stake_gsx.to_be_bytes());
        buf.extend_from_slice(&rec.deposited_stake.to_be_bytes());
        buf.extend_from_slice(&rec.exit_block_height.to_be_bytes());
        buf.push(rec.status.as_byte());
        buf.extend_from_slice(&(rec.mldsa_public_key.len() as u32).to_be_bytes());
        buf.extend_from_slice(&rec.mldsa_public_key);
        buf.extend_from_slice(&(rec.bls_public_key.len() as u32).to_be_bytes());
        buf.extend_from_slice(&rec.bls_public_key);
    }
    buf
}

/// Decode the byte sequence back into the authority map.
/// Accepts v1, v2, and v3 encodings; older records lift
/// missing fields to 0.
pub fn decode(bytes: &[u8]) -> Result<BTreeMap<u32, AuthorityRecord>, ExecutionError> {
    if bytes.is_empty() {
        return Ok(BTreeMap::new());
    }
    if bytes.len() < ENCODED_HEADER_BYTES {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::authority_registry_address(),
            reason: "authority registry header missing",
        });
    }
    let version = u32::from_be_bytes(bytes[0..4].try_into().unwrap());
    let (has_deposited, has_exit_height, entry_fixed_bytes) = match version {
        AUTHORITY_REGISTRY_VERSION => (true, true, ENTRY_FIXED_BYTES),
        AUTHORITY_REGISTRY_VERSION_V2 => (true, false, ENTRY_FIXED_BYTES_V2),
        AUTHORITY_REGISTRY_VERSION_V1 => (false, false, ENTRY_FIXED_BYTES_V1),
        _ => {
            return Err(ExecutionError::CorruptStateRecord {
                addr: reserved::authority_registry_address(),
                reason: "authority registry version mismatch",
            });
        }
    };
    let count = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let mut map = BTreeMap::new();
    let mut cursor = ENCODED_HEADER_BYTES;
    for _ in 0..count {
        if cursor + entry_fixed_bytes > bytes.len() {
            return Err(ExecutionError::CorruptStateRecord {
                addr: reserved::authority_registry_address(),
                reason: "authority registry entry truncated",
            });
        }
        let authority_id = u32::from_be_bytes(bytes[cursor..cursor + 4].try_into().unwrap());
        cursor += 4;
        let stake_gsx = u64::from_be_bytes(bytes[cursor..cursor + 8].try_into().unwrap());
        cursor += 8;
        let deposited_stake = if has_deposited {
            let v = u64::from_be_bytes(bytes[cursor..cursor + 8].try_into().unwrap());
            cursor += 8;
            v
        } else {
            0
        };
        let exit_block_height = if has_exit_height {
            let v = u64::from_be_bytes(bytes[cursor..cursor + 8].try_into().unwrap());
            cursor += 8;
            v
        } else {
            0
        };
        let status = AuthorityStatus::from_byte(bytes[cursor])?;
        cursor += 1;

        let mldsa_len = u32::from_be_bytes(bytes[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 4;
        if mldsa_len > MAX_MLDSA_PK_BYTES || cursor + mldsa_len > bytes.len() {
            return Err(ExecutionError::CorruptStateRecord {
                addr: reserved::authority_registry_address(),
                reason: "authority mldsa_pk length invalid",
            });
        }
        let mldsa_public_key = bytes[cursor..cursor + mldsa_len].to_vec();
        cursor += mldsa_len;

        if cursor + 4 > bytes.len() {
            return Err(ExecutionError::CorruptStateRecord {
                addr: reserved::authority_registry_address(),
                reason: "authority bls_pk length missing",
            });
        }
        let bls_len = u32::from_be_bytes(bytes[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 4;
        if bls_len > MAX_BLS_PK_BYTES || cursor + bls_len > bytes.len() {
            return Err(ExecutionError::CorruptStateRecord {
                addr: reserved::authority_registry_address(),
                reason: "authority bls_pk length invalid",
            });
        }
        let bls_public_key = bytes[cursor..cursor + bls_len].to_vec();
        cursor += bls_len;

        if map
            .insert(
                authority_id,
                AuthorityRecord {
                    mldsa_public_key,
                    bls_public_key,
                    stake_gsx,
                    deposited_stake,
                    exit_block_height,
                    status,
                },
            )
            .is_some()
        {
            return Err(ExecutionError::CorruptStateRecord {
                addr: reserved::authority_registry_address(),
                reason: "authority registry has duplicate authority_id",
            });
        }
    }
    if cursor != bytes.len() {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::authority_registry_address(),
            reason: "authority registry trailing bytes",
        });
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(stake: u64, status: AuthorityStatus) -> AuthorityRecord {
        AuthorityRecord {
            mldsa_public_key: vec![0xaa; 1952],
            bls_public_key: vec![0xbb; 48],
            stake_gsx: stake,
            deposited_stake: 0,
            exit_block_height: 0,
            status,
        }
    }

    #[test]
    fn empty_round_trip() {
        let m: BTreeMap<u32, AuthorityRecord> = BTreeMap::new();
        assert_eq!(decode(&encode(&m)).unwrap(), m);
    }

    #[test]
    fn single_record_round_trip() {
        let mut m = BTreeMap::new();
        m.insert(0, rec(15_000_000, AuthorityStatus::Active));
        assert_eq!(decode(&encode(&m)).unwrap(), m);
    }

    #[test]
    fn multi_record_round_trip_deterministic_order() {
        let mut m1 = BTreeMap::new();
        m1.insert(0, rec(1, AuthorityStatus::Active));
        m1.insert(5, rec(2, AuthorityStatus::Exiting));
        m1.insert(99, rec(3, AuthorityStatus::Ejected));

        let mut m2 = BTreeMap::new();
        m2.insert(99, rec(3, AuthorityStatus::Ejected));
        m2.insert(0, rec(1, AuthorityStatus::Active));
        m2.insert(5, rec(2, AuthorityStatus::Exiting));

        assert_eq!(encode(&m1), encode(&m2));
        assert_eq!(decode(&encode(&m1)).unwrap(), m1);
    }

    #[test]
    fn statuses_round_trip() {
        let mut m = BTreeMap::new();
        m.insert(0, rec(1, AuthorityStatus::Active));
        m.insert(1, rec(2, AuthorityStatus::Exiting));
        m.insert(2, rec(3, AuthorityStatus::Ejected));
        let decoded = decode(&encode(&m)).unwrap();
        assert_eq!(decoded.get(&0).unwrap().status, AuthorityStatus::Active);
        assert_eq!(decoded.get(&1).unwrap().status, AuthorityStatus::Exiting);
        assert_eq!(decoded.get(&2).unwrap().status, AuthorityStatus::Ejected);
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
        bytes.extend_from_slice(&AUTHORITY_REGISTRY_VERSION.to_be_bytes());
        bytes.extend_from_slice(&1u32.to_be_bytes());
        // Header claims 1 entry but no payload.
        assert!(matches!(
            decode(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }

    #[test]
    fn decode_rejects_oversized_mldsa_pk() {
        let mut bytes = vec![];
        bytes.extend_from_slice(&AUTHORITY_REGISTRY_VERSION.to_be_bytes());
        bytes.extend_from_slice(&1u32.to_be_bytes());
        bytes.extend_from_slice(&0u32.to_be_bytes()); // authority_id
        bytes.extend_from_slice(&0u64.to_be_bytes()); // stake_gsx
        bytes.extend_from_slice(&0u64.to_be_bytes()); // deposited_stake (v2+)
        bytes.extend_from_slice(&0u64.to_be_bytes()); // exit_block_height (v3)
        bytes.push(0); // Active
        bytes.extend_from_slice(&((MAX_MLDSA_PK_BYTES + 1) as u32).to_be_bytes());
        // even though we don't include the bytes the length check fires
        assert!(matches!(
            decode(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }

    #[test]
    fn decode_rejects_trailing_bytes() {
        let m: BTreeMap<u32, AuthorityRecord> = BTreeMap::new();
        let mut bytes = encode(&m);
        bytes.push(0xff);
        assert!(matches!(
            decode(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }

    /// Hand-rolled v1 bytes (without `deposited_stake` or
    /// `exit_block_height`) decode cleanly, lifting both
    /// fields to 0.
    #[test]
    fn v1_bytes_decode_with_zero_fields() {
        let mut bytes = vec![];
        bytes.extend_from_slice(&AUTHORITY_REGISTRY_VERSION_V1.to_be_bytes());
        bytes.extend_from_slice(&1u32.to_be_bytes());
        bytes.extend_from_slice(&7u32.to_be_bytes()); // authority_id
        bytes.extend_from_slice(&15_000_000u64.to_be_bytes()); // stake
        bytes.push(0); // Active
        bytes.extend_from_slice(&1952u32.to_be_bytes());
        bytes.extend_from_slice(&[0xaa; 1952]);
        bytes.extend_from_slice(&48u32.to_be_bytes());
        bytes.extend_from_slice(&[0xbb; 48]);

        let decoded = decode(&bytes).unwrap();
        let rec = decoded.get(&7).unwrap();
        assert_eq!(rec.stake_gsx, 15_000_000);
        assert_eq!(rec.deposited_stake, 0);
        assert_eq!(rec.exit_block_height, 0);
        assert_eq!(rec.status, AuthorityStatus::Active);
    }

    /// Hand-rolled v2 bytes (with `deposited_stake`, without
    /// `exit_block_height`) decode cleanly.
    #[test]
    fn v2_bytes_decode_to_v3_with_zero_exit_block_height() {
        let mut bytes = vec![];
        bytes.extend_from_slice(&AUTHORITY_REGISTRY_VERSION_V2.to_be_bytes());
        bytes.extend_from_slice(&1u32.to_be_bytes());
        bytes.extend_from_slice(&7u32.to_be_bytes()); // authority_id
        bytes.extend_from_slice(&15_000_000u64.to_be_bytes()); // stake
        bytes.extend_from_slice(&12_500_000u64.to_be_bytes()); // deposited
        bytes.push(1); // Exiting
        bytes.extend_from_slice(&1952u32.to_be_bytes());
        bytes.extend_from_slice(&[0xaa; 1952]);
        bytes.extend_from_slice(&48u32.to_be_bytes());
        bytes.extend_from_slice(&[0xbb; 48]);

        let decoded = decode(&bytes).unwrap();
        let rec = decoded.get(&7).unwrap();
        assert_eq!(rec.deposited_stake, 12_500_000);
        assert_eq!(rec.exit_block_height, 0); // v2 lifted to 0
        assert_eq!(rec.status, AuthorityStatus::Exiting);
    }

    /// Round-trip with a non-zero deposited_stake (v3-native).
    #[test]
    fn deposited_stake_round_trips() {
        let mut m = BTreeMap::new();
        m.insert(
            0,
            AuthorityRecord {
                mldsa_public_key: vec![0xaa; 1952],
                bls_public_key: vec![0xbb; 48],
                stake_gsx: 15_000_000,
                deposited_stake: 12_500_000,
                exit_block_height: 0,
                status: AuthorityStatus::Active,
            },
        );
        let decoded = decode(&encode(&m)).unwrap();
        assert_eq!(decoded.get(&0).unwrap().deposited_stake, 12_500_000);
    }

    /// Round-trip with a non-zero exit_block_height (v3-native).
    #[test]
    fn exit_block_height_round_trips() {
        let mut m = BTreeMap::new();
        m.insert(
            0,
            AuthorityRecord {
                mldsa_public_key: vec![0xaa; 1952],
                bls_public_key: vec![0xbb; 48],
                stake_gsx: 15_000_000,
                deposited_stake: 12_500_000,
                exit_block_height: 1_234_567,
                status: AuthorityStatus::Exiting,
            },
        );
        let decoded = decode(&encode(&m)).unwrap();
        assert_eq!(decoded.get(&0).unwrap().exit_block_height, 1_234_567);
    }
}
