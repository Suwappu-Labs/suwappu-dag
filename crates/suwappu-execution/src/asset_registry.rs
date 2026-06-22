//! Asset registry storage (Track I I.5, #166).
//!
//! Substrate-side chain-state authoritative list of bridged
//! assets per the strategic plan Track I §"Asset whitelist".
//! Stores `AssetId -> AssetRecord` records at the reserved
//! `asset_registry_address` via the bytes_state surface (PR
//! #189).
//!
//! ## Flow
//!
//! - Foundation governance authorizes `Intent::AddBridgeAsset`
//!   to whitelist a new bridge asset; substrate stores the
//!   record as `Active`.
//! - `Intent::PauseBridgeAsset` flips status to `Paused`
//!   (emergency response: stop bridge traffic for an asset
//!   without removing the registry record).
//! - `Intent::RemoveBridgeAsset` flips status to `Removed`
//!   (irreversible; record stays for audit but no further
//!   bridge operations accepted).
//!
//! ## Asset ID derivation
//!
//! Per IQ-006 + the strategic plan Track I:
//!
//! ```text
//! asset_id = BLAKE3("suwappu-asset-v1" || u64_be(source_chain) ||
//!                   u32_be(source_contract.len()) || source_contract)
//! ```
//!
//! `source_contract` is variable-width (20 B for EVM contracts,
//! 32 B for SVM program-derived addresses, etc.). The length-
//! prefix keeps the encoding unambiguous.
//!
//! ## Phase 1 scope
//!
//! This PR lands the registry + the three governance Intents.
//! Wiring `Intent::L1Lock` / `L2BurnProven` to validate against
//! the whitelist is a follow-up — those Intents currently
//! implicitly bridge the native SUWAPPU; adding asset support
//! requires either new `L1LockAsset` / `L2BurnProvenAsset`
//! variants or extending the existing ones with an
//! `Option<AssetId>` field. Deferred to keep this PR contained.

use std::collections::BTreeMap;

use blake3::Hasher;
use serde::{Deserialize, Serialize};

use crate::{error::ExecutionError, reserved};

/// Current asset-registry encoding version. Bump on
/// incompatible changes; decoder rejects any other value.
pub const ASSET_REGISTRY_VERSION: u32 = 1;

/// Maximum bytes in an asset's `source_contract` field. EVM
/// contracts are 20 B; Solana program-derived addresses 32 B;
/// future destinations may go higher. Cap at 64 B as
/// belt-and-suspenders.
pub const MAX_SOURCE_CONTRACT_BYTES: usize = 64;

/// Maximum bytes in the human-readable `name` field.
pub const MAX_ASSET_NAME_BYTES: usize = 64;

/// Maximum bytes in the human-readable `symbol` field.
pub const MAX_ASSET_SYMBOL_BYTES: usize = 16;

/// Length of the fixed-portion encoding per entry (excluding
/// the variable-length source_contract / name / symbol).
/// `asset_id (32) + source_chain (8) + decimals (1) + status (1) +
///  3 length prefixes (3 × u32 = 12) = 54`.
pub const ENTRY_FIXED_HEADER_BYTES: usize = 32 + 8 + 1 + 1 + 12;

/// Length of the encoded registry header:
/// `version (4) + count (4) = 8`.
pub const ENCODED_HEADER_BYTES: usize = 4 + 4;

/// Status of a registered bridge asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum AssetStatus {
    /// Asset is whitelisted; bridge operations accepted.
    Active,
    /// Asset is temporarily paused; bridge operations rejected
    /// but the registry record persists.
    Paused,
    /// Asset is permanently removed from the whitelist; record
    /// stays for audit but bridge operations rejected.
    Removed,
}

impl AssetStatus {
    fn as_byte(&self) -> u8 {
        match self {
            AssetStatus::Active => 0,
            AssetStatus::Paused => 1,
            AssetStatus::Removed => 2,
        }
    }

    fn from_byte(b: u8) -> Result<Self, ExecutionError> {
        match b {
            0 => Ok(AssetStatus::Active),
            1 => Ok(AssetStatus::Paused),
            2 => Ok(AssetStatus::Removed),
            _ => Err(ExecutionError::CorruptStateRecord {
                addr: reserved::asset_registry_address(),
                reason: "asset status out of range",
            }),
        }
    }
}

/// A whitelisted bridge asset. Stored at the reserved
/// `asset_registry_address` keyed by `AssetId`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetRecord {
    /// Source-chain identifier (Ethereum mainnet = 1, Solana
    /// mainnet-beta = some canonical id, etc.).
    pub source_chain: u64,
    /// Source-chain contract / program address. Variable
    /// width: 20 B for EVM, 32 B for SVM.
    pub source_contract: Vec<u8>,
    /// Asset decimals (e.g., USDC = 6, ETH = 18).
    pub decimals: u8,
    /// Human-readable asset name (e.g., "USD Coin").
    pub name: Vec<u8>,
    /// Human-readable asset symbol (e.g., "USDC").
    pub symbol: Vec<u8>,
    /// Current status.
    pub status: AssetStatus,
}

/// Compute the canonical `asset_id` for a given source-chain
/// contract:
/// `BLAKE3("suwappu-asset-v1" || u64_be(source_chain) ||
///         u32_be(source_contract.len()) || source_contract)`.
pub fn asset_id(source_chain: u64, source_contract: &[u8]) -> [u8; 32] {
    let mut h = Hasher::new();
    h.update(b"gsx-asset-v1");
    h.update(&source_chain.to_be_bytes());
    h.update(&(source_contract.len() as u32).to_be_bytes());
    h.update(source_contract);
    *h.finalize().as_bytes()
}

/// The full asset registry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssetRegistry {
    /// Map of asset_id → asset record.
    pub assets: BTreeMap<[u8; 32], AssetRecord>,
}

/// Encode the registry to the canonical byte sequence.
pub fn encode(reg: &AssetRegistry) -> Vec<u8> {
    let mut buf = Vec::with_capacity(ENCODED_HEADER_BYTES);
    buf.extend_from_slice(&ASSET_REGISTRY_VERSION.to_be_bytes());
    buf.extend_from_slice(&(reg.assets.len() as u32).to_be_bytes());
    for (id, rec) in &reg.assets {
        buf.extend_from_slice(id);
        buf.extend_from_slice(&rec.source_chain.to_be_bytes());
        buf.push(rec.decimals);
        buf.push(rec.status.as_byte());
        buf.extend_from_slice(&(rec.source_contract.len() as u32).to_be_bytes());
        buf.extend_from_slice(&rec.source_contract);
        buf.extend_from_slice(&(rec.name.len() as u32).to_be_bytes());
        buf.extend_from_slice(&rec.name);
        buf.extend_from_slice(&(rec.symbol.len() as u32).to_be_bytes());
        buf.extend_from_slice(&rec.symbol);
    }
    buf
}

/// Decode the byte sequence back into a registry.
pub fn decode(bytes: &[u8]) -> Result<AssetRegistry, ExecutionError> {
    if bytes.is_empty() {
        return Ok(AssetRegistry::default());
    }
    if bytes.len() < ENCODED_HEADER_BYTES {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::asset_registry_address(),
            reason: "asset registry header missing",
        });
    }
    let version = u32::from_be_bytes(bytes[0..4].try_into().unwrap());
    if version != ASSET_REGISTRY_VERSION {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::asset_registry_address(),
            reason: "asset registry version mismatch",
        });
    }
    let count = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let mut assets = BTreeMap::new();
    let mut cursor = ENCODED_HEADER_BYTES;

    for _ in 0..count {
        // Fixed header: id (32) + chain (8) + decimals (1) + status (1) = 42 B
        // Plus 3 length-prefixes (4 B each) for the variable fields.
        if cursor + 42 + 4 > bytes.len() {
            return Err(ExecutionError::CorruptStateRecord {
                addr: reserved::asset_registry_address(),
                reason: "asset registry entry truncated (fixed header)",
            });
        }
        let mut id = [0u8; 32];
        id.copy_from_slice(&bytes[cursor..cursor + 32]);
        cursor += 32;
        let source_chain = u64::from_be_bytes(bytes[cursor..cursor + 8].try_into().unwrap());
        cursor += 8;
        let decimals = bytes[cursor];
        cursor += 1;
        let status = AssetStatus::from_byte(bytes[cursor])?;
        cursor += 1;

        let source_contract = read_length_prefixed(bytes, &mut cursor, MAX_SOURCE_CONTRACT_BYTES)?;
        let name = read_length_prefixed(bytes, &mut cursor, MAX_ASSET_NAME_BYTES)?;
        let symbol = read_length_prefixed(bytes, &mut cursor, MAX_ASSET_SYMBOL_BYTES)?;

        let rec = AssetRecord {
            source_chain,
            source_contract,
            decimals,
            name,
            symbol,
            status,
        };
        if assets.insert(id, rec).is_some() {
            return Err(ExecutionError::CorruptStateRecord {
                addr: reserved::asset_registry_address(),
                reason: "asset registry has duplicate ids",
            });
        }
    }

    if cursor != bytes.len() {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::asset_registry_address(),
            reason: "asset registry has trailing bytes",
        });
    }

    Ok(AssetRegistry { assets })
}

/// Read a `u32_be(len) || data` field; bound `len` at
/// `max_len` for OOM defense.
fn read_length_prefixed(
    bytes: &[u8],
    cursor: &mut usize,
    max_len: usize,
) -> Result<Vec<u8>, ExecutionError> {
    if *cursor + 4 > bytes.len() {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::asset_registry_address(),
            reason: "asset registry field length-prefix truncated",
        });
    }
    let len = u32::from_be_bytes(bytes[*cursor..*cursor + 4].try_into().unwrap()) as usize;
    *cursor += 4;
    if len > max_len {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::asset_registry_address(),
            reason: "asset registry field exceeds maximum length",
        });
    }
    if *cursor + len > bytes.len() {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::asset_registry_address(),
            reason: "asset registry field data truncated",
        });
    }
    let data = bytes[*cursor..*cursor + len].to_vec();
    *cursor += len;
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(suffix: u8) -> AssetRecord {
        AssetRecord {
            source_chain: 1, // Ethereum mainnet
            source_contract: vec![suffix; 20],
            decimals: 6,
            name: format!("Asset-{suffix:02X}").into_bytes(),
            symbol: format!("A{suffix:02X}").into_bytes(),
            status: AssetStatus::Active,
        }
    }

    #[test]
    fn asset_id_is_deterministic() {
        let id1 = asset_id(1, &[0xab; 20]);
        let id2 = asset_id(1, &[0xab; 20]);
        assert_eq!(id1, id2);
    }

    #[test]
    fn asset_id_distinguishes_chain() {
        let id_eth = asset_id(1, &[0xab; 20]);
        let id_sol = asset_id(101, &[0xab; 20]);
        assert_ne!(id_eth, id_sol);
    }

    #[test]
    fn asset_id_distinguishes_contract() {
        let id_a = asset_id(1, &[0xab; 20]);
        let id_b = asset_id(1, &[0xac; 20]);
        assert_ne!(id_a, id_b);
    }

    #[test]
    fn asset_id_length_prefix_disambiguates_contracts() {
        // Defends against ambiguity from concatenation:
        // chain=1 || contract=[0xab; 20] should NOT collide with
        // chain=1 || contract=[0xab; 21].
        let id20 = asset_id(1, &[0xab; 20]);
        let id21 = asset_id(1, &[0xab; 21]);
        assert_ne!(id20, id21);
    }

    #[test]
    fn empty_registry_round_trips() {
        let r = AssetRegistry::default();
        let bytes = encode(&r);
        assert_eq!(bytes.len(), ENCODED_HEADER_BYTES);
        let r2 = decode(&bytes).unwrap();
        assert_eq!(r, r2);
    }

    #[test]
    fn empty_bytes_decodes_to_empty_registry() {
        let r = decode(&[]).unwrap();
        assert!(r.assets.is_empty());
    }

    #[test]
    fn single_entry_round_trips() {
        let mut r = AssetRegistry::default();
        let rec = record(0x01);
        r.assets.insert([0xab; 32], rec);
        let bytes = encode(&r);
        let r2 = decode(&bytes).unwrap();
        assert_eq!(r, r2);
    }

    #[test]
    fn multi_entry_round_trips_all_statuses() {
        let mut r = AssetRegistry::default();
        let mut a = record(0x01);
        a.status = AssetStatus::Active;
        let mut p = record(0x02);
        p.status = AssetStatus::Paused;
        let mut x = record(0x03);
        x.status = AssetStatus::Removed;
        r.assets.insert([0xa1; 32], a);
        r.assets.insert([0xa2; 32], p);
        r.assets.insert([0xa3; 32], x);
        let bytes = encode(&r);
        let r2 = decode(&bytes).unwrap();
        assert_eq!(r, r2);
    }

    #[test]
    fn variable_field_widths_round_trip() {
        // Cover the full range of variable-field widths.
        let mut r = AssetRegistry::default();
        r.assets.insert(
            [0x01; 32],
            AssetRecord {
                source_chain: 1,
                source_contract: vec![], // empty contract
                decimals: 0,
                name: b"".to_vec(),   // empty name
                symbol: b"".to_vec(), // empty symbol
                status: AssetStatus::Active,
            },
        );
        r.assets.insert(
            [0x02; 32],
            AssetRecord {
                source_chain: u64::MAX,
                source_contract: vec![0xff; MAX_SOURCE_CONTRACT_BYTES],
                decimals: 255,
                name: vec![0x41; MAX_ASSET_NAME_BYTES],
                symbol: vec![0x42; MAX_ASSET_SYMBOL_BYTES],
                status: AssetStatus::Paused,
            },
        );
        let bytes = encode(&r);
        let r2 = decode(&bytes).unwrap();
        assert_eq!(r, r2);
    }

    #[test]
    fn decode_rejects_wrong_version() {
        let mut bytes = vec![0, 0, 0, 99]; // version = 99
        bytes.extend_from_slice(&[0, 0, 0, 0]); // count = 0
        assert!(matches!(
            decode(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }

    #[test]
    fn decode_rejects_short_header() {
        assert!(matches!(
            decode(&[0u8; 5]),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }

    #[test]
    fn decode_rejects_oversized_source_contract() {
        // Construct bytes with a source_contract length-prefix
        // exceeding MAX_SOURCE_CONTRACT_BYTES.
        let mut bytes = vec![0, 0, 0, 1]; // version = 1
        bytes.extend_from_slice(&[0, 0, 0, 1]); // count = 1
        bytes.extend_from_slice(&[0u8; 32]); // id
        bytes.extend_from_slice(&[0u8; 8]); // source_chain
        bytes.push(6); // decimals
        bytes.push(0); // status = Active
        bytes.extend_from_slice(&((MAX_SOURCE_CONTRACT_BYTES + 1) as u32).to_be_bytes());
        bytes.resize(bytes.len() + MAX_SOURCE_CONTRACT_BYTES + 1, 0u8);
        // Rest of name/symbol fields omitted (decode fails first).
        assert!(matches!(
            decode(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }

    #[test]
    fn decode_rejects_trailing_bytes() {
        let r = AssetRegistry::default();
        let mut bytes = encode(&r);
        bytes.push(0xff); // trailing garbage
        assert!(matches!(
            decode(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }

    #[test]
    fn deterministic_ascending_order() {
        let mut r1 = AssetRegistry::default();
        r1.assets.insert([0x02; 32], record(0x02));
        r1.assets.insert([0x01; 32], record(0x01));
        let mut r2 = AssetRegistry::default();
        r2.assets.insert([0x01; 32], record(0x01));
        r2.assets.insert([0x02; 32], record(0x02));
        assert_eq!(encode(&r1), encode(&r2));
    }
}
