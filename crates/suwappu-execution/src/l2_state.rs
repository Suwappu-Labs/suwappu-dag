//! L2 registry storage shape (Track G G2.2 phase 2,
//! revised for multi-chain VK pinning).
//!
//! The substrate's bytes-state map at the reserved
//! `l2_registry_address` (per
//! `docs/iq/IQ-006-l2-state-root-commitment-surface.md`) stores a
//! single `L2Registry` value containing:
//!
//! - The chain-pinned VK pair (`aggregation_vk_hash`,
//!   `range_vk_commitment`) per the op-succinct multiBlockVKey
//!   pattern — now keyed by `l2_chain_id_hash` so each L2
//!   chain has its own VK. Rotated via
//!   `Intent::SetL2VerifyingKey { chain_id_hash, .. }`.
//! - The per-batch state-roots map (`L2BatchKey ->
//!   L2StateRootRecord`). Each successful
//!   `Intent::CommitL2StateRoot` inserts here.
//!
//! ## Encoding (v2 — multi-chain VKs)
//!
//! ```text
//! u32::BE(VERSION = 2) ||
//! u32::BE(chain_vk_count) ||
//!   foreach (chain_id_hash, vks) in ascending chain_id_hash order:
//!     chain_id_hash (32 B) ||
//!     vks.aggregation_vk_hash (32 B) ||
//!     vks.range_vk_commitment (32 B)
//! u32::BE(state_root_count) ||
//!   foreach (key, record) in ascending key order:
//!     key.l2_chain_id_hash (32 B) ||
//!     key.batch_id (u64::BE, 8 B) ||
//!     record.state_root (32 B) ||
//!     record.committed_at_l1_height (u64::BE, 8 B) ||
//!     record.vk_hash (32 B) ||
//!     record.da_commitment (32 B)
//! ```
//!
//! Per chain-VK entry: 32 + 32 + 32 = **96 B**.
//! Per state-root entry: 32 + 8 + 32 + 8 + 32 + 32 = **144 B**.
//! Encoded header minimum: 4 (version) + 4 (chain_vk_count) +
//!                          4 (state_root_count) = **12 B**.
//!
//! ## Migration from v1
//!
//! V1 encoded a single `(aggregation_vk_hash, range_vk_commitment)`
//! pair at the top of the registry, implicitly applying to
//! `chain_id_hash = [0u8; 32]`. The v2 decoder transparently
//! reads v1 bytes by lifting the v1 VK pair into the v2 chain_vks
//! map under the `[0u8; 32]` key. New encodes always emit v2.
//!
//! ## Scalability
//!
//! Phase 1: every commit decodes the full map, mutates, re-encodes.
//! O(N) per commit. Fine for ≤ 10k batches per L2 chain across ≤ 10
//! chains. Phase 2 may switch to an append-only log + index tree
//! once scaling pressure justifies the complexity.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{error::ExecutionError, reserved};

/// Current L2 registry encoding version. Bumped to v2 in this PR
/// when the VK pair moved from a single global value to a
/// `BTreeMap<chain_id_hash, ChainVks>`.
pub const L2_REGISTRY_VERSION: u32 = 2;

/// Legacy v1 encoding version. The decoder still accepts v1 bytes
/// (lifting the global VK pair into the [0; 32] chain_vks entry)
/// but the encoder always emits v2.
pub const L2_REGISTRY_VERSION_V1: u32 = 1;

/// Length of one encoded `L2StateRootRecord` in bytes.
/// `state_root (32) + committed_at_l1_height (8) + vk_hash (32) +
///  da_commitment (32) = 104`.
pub const L2_STATE_ROOT_RECORD_BYTES: usize = 32 + 8 + 32 + 32;

/// Length of one encoded `(L2BatchKey, L2StateRootRecord)` pair.
/// `key (40) + record (104) = 144`.
pub const ENCODED_STATE_ROOT_ENTRY_BYTES: usize = 32 + 8 + L2_STATE_ROOT_RECORD_BYTES;

/// Length of one encoded `(chain_id_hash, ChainVks)` pair.
/// `32 + 32 + 32 = 96`.
pub const ENCODED_CHAIN_VKS_ENTRY_BYTES: usize = 32 + 32 + 32;

/// VK pair pinned for a single L2 chain (op-succinct
/// multiBlockVKey pattern). Both fields default to `[0u8; 32]`
/// for "no VK pinned yet" — the substrate rejects
/// `CommitL2StateRoot` Intents whose `vk_hash` doesn't match
/// the pinned `aggregation_vk_hash`, so an all-zeros pin
/// blocks commits until governance rotates in a real VK.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainVks {
    /// SP1 aggregation VK hash for this chain.
    pub aggregation_vk_hash: [u8; 32],
    /// Per-batch range-program VK commitment (op-succinct
    /// multiBlockVKey pattern) for this chain.
    pub range_vk_commitment: [u8; 32],
}

/// Per-batch L2 state-root record. Stored at the reserved
/// `l2_registry_address` keyed by `(l2_chain_id_hash, batch_id)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct L2StateRootRecord {
    /// L2 state root committed by this batch (EVM MPT root per
    /// Open Item #8 EVM flip).
    pub state_root: [u8; 32],
    /// L1 block height at which this commit landed.
    pub committed_at_l1_height: u64,
    /// SP1 verifying-key hash this proof verified under.
    pub vk_hash: [u8; 32],
    /// DA commitment binding the batch's DA blob.
    pub da_commitment: [u8; 32],
}

/// Composite key identifying an L2 batch within the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct L2BatchKey {
    /// L2 chain identifier hash (multi-L2 namespacing per IQ-006).
    pub l2_chain_id_hash: [u8; 32],
    /// Monotonic per-L2-chain batch identifier.
    pub batch_id: u64,
}

/// The full L2 registry stored at `l2_registry_address`. Owns
/// the per-chain VK pinning map + the per-batch state-roots map.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct L2Registry {
    /// Per-chain VK pinning. Each L2 chain has its own
    /// `(aggregation_vk_hash, range_vk_commitment)` pair.
    /// `Intent::SetL2VerifyingKey { chain_id_hash, .. }`
    /// rotates the entry for a single chain. A v1-style
    /// single-L2 chain uses the `[0u8; 32]` key.
    pub chain_vks: BTreeMap<[u8; 32], ChainVks>,
    /// Per-batch state-roots map keyed by `(l2_chain_id_hash, batch_id)`.
    pub state_roots: BTreeMap<L2BatchKey, L2StateRootRecord>,
}

impl L2Registry {
    /// Read the pinned `aggregation_vk_hash` for the given chain.
    /// Returns `[0u8; 32]` if no VK is pinned for that chain.
    pub fn aggregation_vk_hash(&self, chain_id_hash: &[u8; 32]) -> [u8; 32] {
        self.chain_vks
            .get(chain_id_hash)
            .map(|v| v.aggregation_vk_hash)
            .unwrap_or([0u8; 32])
    }

    /// Read the pinned `range_vk_commitment` for the given chain.
    /// Returns `[0u8; 32]` if no VK is pinned for that chain.
    pub fn range_vk_commitment(&self, chain_id_hash: &[u8; 32]) -> [u8; 32] {
        self.chain_vks
            .get(chain_id_hash)
            .map(|v| v.range_vk_commitment)
            .unwrap_or([0u8; 32])
    }

    /// Pin the VK pair for a single chain. Overwrites any
    /// existing entry.
    pub fn set_chain_vks(
        &mut self,
        chain_id_hash: [u8; 32],
        aggregation_vk_hash: [u8; 32],
        range_vk_commitment: [u8; 32],
    ) {
        self.chain_vks.insert(
            chain_id_hash,
            ChainVks {
                aggregation_vk_hash,
                range_vk_commitment,
            },
        );
    }
}

/// Encode the registry to the canonical v2 byte sequence.
pub fn encode(reg: &L2Registry) -> Vec<u8> {
    let mut buf = Vec::with_capacity(
        4 + 4
            + reg.chain_vks.len() * ENCODED_CHAIN_VKS_ENTRY_BYTES
            + 4
            + reg.state_roots.len() * ENCODED_STATE_ROOT_ENTRY_BYTES,
    );
    buf.extend_from_slice(&L2_REGISTRY_VERSION.to_be_bytes());
    buf.extend_from_slice(&(reg.chain_vks.len() as u32).to_be_bytes());
    for (chain, vks) in &reg.chain_vks {
        buf.extend_from_slice(chain);
        buf.extend_from_slice(&vks.aggregation_vk_hash);
        buf.extend_from_slice(&vks.range_vk_commitment);
    }
    buf.extend_from_slice(&(reg.state_roots.len() as u32).to_be_bytes());
    for (key, rec) in &reg.state_roots {
        buf.extend_from_slice(&key.l2_chain_id_hash);
        buf.extend_from_slice(&key.batch_id.to_be_bytes());
        buf.extend_from_slice(&rec.state_root);
        buf.extend_from_slice(&rec.committed_at_l1_height.to_be_bytes());
        buf.extend_from_slice(&rec.vk_hash);
        buf.extend_from_slice(&rec.da_commitment);
    }
    buf
}

/// Decode the byte sequence back into a registry. Empty bytes
/// decode to a default (empty) registry. Both v1 and v2 encodings
/// are accepted; v1 bytes lift the global VK pair into the v2
/// chain_vks entry under `[0u8; 32]`.
pub fn decode(bytes: &[u8]) -> Result<L2Registry, ExecutionError> {
    if bytes.is_empty() {
        return Ok(L2Registry::default());
    }
    if bytes.len() < 4 {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::l2_registry_address(),
            reason: "L2 registry header missing",
        });
    }
    let version = u32::from_be_bytes(bytes[0..4].try_into().unwrap());
    match version {
        L2_REGISTRY_VERSION => decode_v2(bytes),
        L2_REGISTRY_VERSION_V1 => decode_v1(bytes),
        _ => Err(ExecutionError::CorruptStateRecord {
            addr: reserved::l2_registry_address(),
            reason: "L2 registry version mismatch",
        }),
    }
}

fn decode_v2(bytes: &[u8]) -> Result<L2Registry, ExecutionError> {
    // Header: version (4) + chain_vk_count (4) = 8.
    if bytes.len() < 8 {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::l2_registry_address(),
            reason: "L2 registry v2 header missing",
        });
    }
    let chain_vk_count = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let mut cursor = 8usize;
    let chain_vks_bytes = chain_vk_count
        .checked_mul(ENCODED_CHAIN_VKS_ENTRY_BYTES)
        .ok_or(ExecutionError::CorruptStateRecord {
            addr: reserved::l2_registry_address(),
            reason: "L2 registry chain_vks size overflow",
        })?;
    if bytes.len() < cursor + chain_vks_bytes + 4 {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::l2_registry_address(),
            reason: "L2 registry chain_vks truncated",
        });
    }
    let mut chain_vks = BTreeMap::new();
    for _ in 0..chain_vk_count {
        let mut chain = [0u8; 32];
        chain.copy_from_slice(&bytes[cursor..cursor + 32]);
        cursor += 32;
        let mut aggregation_vk_hash = [0u8; 32];
        aggregation_vk_hash.copy_from_slice(&bytes[cursor..cursor + 32]);
        cursor += 32;
        let mut range_vk_commitment = [0u8; 32];
        range_vk_commitment.copy_from_slice(&bytes[cursor..cursor + 32]);
        cursor += 32;
        if chain_vks
            .insert(
                chain,
                ChainVks {
                    aggregation_vk_hash,
                    range_vk_commitment,
                },
            )
            .is_some()
        {
            return Err(ExecutionError::CorruptStateRecord {
                addr: reserved::l2_registry_address(),
                reason: "L2 registry has duplicate chain_vks",
            });
        }
    }
    let state_root_count =
        u32::from_be_bytes(bytes[cursor..cursor + 4].try_into().unwrap()) as usize;
    cursor += 4;
    let state_roots_bytes = state_root_count
        .checked_mul(ENCODED_STATE_ROOT_ENTRY_BYTES)
        .ok_or(ExecutionError::CorruptStateRecord {
            addr: reserved::l2_registry_address(),
            reason: "L2 registry state_roots size overflow",
        })?;
    if bytes.len() != cursor + state_roots_bytes {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::l2_registry_address(),
            reason: "L2 registry size mismatch",
        });
    }
    let state_roots = decode_state_roots(bytes, cursor, state_root_count)?;
    Ok(L2Registry {
        chain_vks,
        state_roots,
    })
}

fn decode_v1(bytes: &[u8]) -> Result<L2Registry, ExecutionError> {
    // V1 header: version (4) + agg_vk (32) + range_vk (32) + count (4) = 72.
    const V1_HEADER_BYTES: usize = 4 + 32 + 32 + 4;
    if bytes.len() < V1_HEADER_BYTES {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::l2_registry_address(),
            reason: "L2 registry v1 header missing",
        });
    }
    let mut aggregation_vk_hash = [0u8; 32];
    aggregation_vk_hash.copy_from_slice(&bytes[4..36]);
    let mut range_vk_commitment = [0u8; 32];
    range_vk_commitment.copy_from_slice(&bytes[36..68]);
    let state_root_count = u32::from_be_bytes(bytes[68..72].try_into().unwrap()) as usize;
    let state_roots_bytes = state_root_count
        .checked_mul(ENCODED_STATE_ROOT_ENTRY_BYTES)
        .ok_or(ExecutionError::CorruptStateRecord {
            addr: reserved::l2_registry_address(),
            reason: "L2 registry v1 state_roots size overflow",
        })?;
    if bytes.len() != V1_HEADER_BYTES + state_roots_bytes {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::l2_registry_address(),
            reason: "L2 registry v1 size mismatch",
        });
    }
    let mut chain_vks = BTreeMap::new();
    // Lift v1 single-VK pair into the [0; 32] chain entry —
    // unless both are zeros, in which case leave the map empty
    // so v1's "no VK pinned" sentinel is preserved.
    if aggregation_vk_hash != [0u8; 32] || range_vk_commitment != [0u8; 32] {
        chain_vks.insert(
            [0u8; 32],
            ChainVks {
                aggregation_vk_hash,
                range_vk_commitment,
            },
        );
    }
    let state_roots = decode_state_roots(bytes, V1_HEADER_BYTES, state_root_count)?;
    Ok(L2Registry {
        chain_vks,
        state_roots,
    })
}

fn decode_state_roots(
    bytes: &[u8],
    start: usize,
    count: usize,
) -> Result<BTreeMap<L2BatchKey, L2StateRootRecord>, ExecutionError> {
    let mut state_roots = BTreeMap::new();
    let mut cursor = start;
    for _ in 0..count {
        let mut l2_chain_id_hash = [0u8; 32];
        l2_chain_id_hash.copy_from_slice(&bytes[cursor..cursor + 32]);
        cursor += 32;
        let batch_id = u64::from_be_bytes(bytes[cursor..cursor + 8].try_into().unwrap());
        cursor += 8;
        let key = L2BatchKey {
            l2_chain_id_hash,
            batch_id,
        };
        let mut state_root = [0u8; 32];
        state_root.copy_from_slice(&bytes[cursor..cursor + 32]);
        cursor += 32;
        let committed_at_l1_height =
            u64::from_be_bytes(bytes[cursor..cursor + 8].try_into().unwrap());
        cursor += 8;
        let mut vk_hash = [0u8; 32];
        vk_hash.copy_from_slice(&bytes[cursor..cursor + 32]);
        cursor += 32;
        let mut da_commitment = [0u8; 32];
        da_commitment.copy_from_slice(&bytes[cursor..cursor + 32]);
        cursor += 32;
        let rec = L2StateRootRecord {
            state_root,
            committed_at_l1_height,
            vk_hash,
            da_commitment,
        };
        if state_roots.insert(key, rec).is_some() {
            return Err(ExecutionError::CorruptStateRecord {
                addr: reserved::l2_registry_address(),
                reason: "L2 registry has duplicate keys",
            });
        }
    }
    Ok(state_roots)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(seed: u8) -> L2StateRootRecord {
        L2StateRootRecord {
            state_root: [seed; 32],
            committed_at_l1_height: 100 + seed as u64,
            vk_hash: [seed.wrapping_add(1); 32],
            da_commitment: [seed.wrapping_add(2); 32],
        }
    }

    fn key(chain_seed: u8, batch_id: u64) -> L2BatchKey {
        L2BatchKey {
            l2_chain_id_hash: [chain_seed; 32],
            batch_id,
        }
    }

    fn empty_registry() -> L2Registry {
        L2Registry::default()
    }

    #[test]
    fn empty_registry_round_trips() {
        let r = empty_registry();
        let bytes = encode(&r);
        // v2 header: 4 (version) + 4 (chain_vk_count) + 4 (state_root_count) = 12 B
        assert_eq!(bytes.len(), 12);
        let r2 = decode(&bytes).unwrap();
        assert_eq!(r, r2);
    }

    #[test]
    fn empty_bytes_decodes_to_empty_registry() {
        let r = decode(&[]).unwrap();
        assert!(r.state_roots.is_empty());
        assert!(r.chain_vks.is_empty());
    }

    #[test]
    fn single_chain_vk_round_trips() {
        let mut r = L2Registry::default();
        r.set_chain_vks([0u8; 32], [0xab; 32], [0xcd; 32]);
        let bytes = encode(&r);
        let r2 = decode(&bytes).unwrap();
        assert_eq!(r, r2);
        assert_eq!(r2.aggregation_vk_hash(&[0u8; 32]), [0xab; 32]);
        assert_eq!(r2.range_vk_commitment(&[0u8; 32]), [0xcd; 32]);
    }

    #[test]
    fn multi_chain_vks_round_trip() {
        let mut r = L2Registry::default();
        r.set_chain_vks([0u8; 32], [0xa1; 32], [0xa2; 32]);
        r.set_chain_vks([0xff; 32], [0xb1; 32], [0xb2; 32]);
        let bytes = encode(&r);
        let r2 = decode(&bytes).unwrap();
        assert_eq!(r, r2);
        assert_eq!(r2.aggregation_vk_hash(&[0u8; 32]), [0xa1; 32]);
        assert_eq!(r2.aggregation_vk_hash(&[0xff; 32]), [0xb1; 32]);
        // Unset chain returns [0; 32].
        assert_eq!(r2.aggregation_vk_hash(&[0x55; 32]), [0u8; 32]);
    }

    #[test]
    fn single_state_root_round_trips() {
        let mut r = empty_registry();
        r.state_roots.insert(key(0x01, 5), record(0xab));
        let bytes = encode(&r);
        // 12 (header) + 144 (one state root entry)
        assert_eq!(bytes.len(), 12 + ENCODED_STATE_ROOT_ENTRY_BYTES);
        let r2 = decode(&bytes).unwrap();
        assert_eq!(r, r2);
    }

    #[test]
    fn chain_vks_and_state_roots_round_trip_together() {
        let mut r = L2Registry::default();
        r.set_chain_vks([0u8; 32], [0xa0; 32], [0xa1; 32]);
        r.set_chain_vks([0xff; 32], [0xb0; 32], [0xb1; 32]);
        r.state_roots.insert(key(0x01, 0), record(0xa1));
        r.state_roots.insert(key(0x02, 7), record(0xb2));
        let bytes = encode(&r);
        let expected = 12 + 2 * ENCODED_CHAIN_VKS_ENTRY_BYTES + 2 * ENCODED_STATE_ROOT_ENTRY_BYTES;
        assert_eq!(bytes.len(), expected);
        let r2 = decode(&bytes).unwrap();
        assert_eq!(r, r2);
    }

    #[test]
    fn encoding_is_deterministic_across_runs() {
        let mut r = empty_registry();
        r.set_chain_vks([0u8; 32], [0xa1; 32], [0xa2; 32]);
        r.state_roots.insert(key(0x01, 1), record(0xab));
        r.state_roots.insert(key(0x02, 2), record(0xcd));
        assert_eq!(encode(&r), encode(&r));
    }

    /// V1 bytes still decode — the v1 VK pair lifts into the
    /// [0; 32] chain_vks entry.
    #[test]
    fn v1_bytes_decode_to_v2_registry() {
        // Build v1 bytes by hand.
        let mut bytes = vec![0, 0, 0, 1]; // version = 1
        bytes.extend_from_slice(&[0xab; 32]); // agg_vk
        bytes.extend_from_slice(&[0xcd; 32]); // range_vk
        bytes.extend_from_slice(&[0, 0, 0, 0]); // state_root count = 0

        let r = decode(&bytes).unwrap();
        assert_eq!(r.chain_vks.len(), 1);
        assert_eq!(r.aggregation_vk_hash(&[0u8; 32]), [0xab; 32]);
        assert_eq!(r.range_vk_commitment(&[0u8; 32]), [0xcd; 32]);
        assert!(r.state_roots.is_empty());
    }

    /// V1 bytes with empty VK pair decode to empty chain_vks
    /// map (preserve "no VK pinned" sentinel).
    #[test]
    fn v1_empty_vks_decode_to_empty_chain_vks() {
        let mut bytes = vec![0, 0, 0, 1];
        bytes.extend_from_slice(&[0u8; 32]);
        bytes.extend_from_slice(&[0u8; 32]);
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        let r = decode(&bytes).unwrap();
        assert!(r.chain_vks.is_empty());
    }

    /// V1 bytes with state roots round-trip into v2 (state
    /// roots preserved, VK pair lifted).
    #[test]
    fn v1_state_roots_preserved_on_decode() {
        let mut bytes = vec![0, 0, 0, 1];
        bytes.extend_from_slice(&[0xab; 32]);
        bytes.extend_from_slice(&[0xcd; 32]);
        bytes.extend_from_slice(&[0, 0, 0, 1]); // 1 state root
                                                // Key: l2_chain_id_hash (32) + batch_id (8) = 40
        bytes.extend_from_slice(&[0u8; 32]);
        bytes.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 5]); // batch_id = 5
                                                            // Record: state_root (32) + height (8) + vk_hash (32) + da (32) = 104
        bytes.extend_from_slice(&[0x11; 32]);
        bytes.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 100]);
        bytes.extend_from_slice(&[0xab; 32]);
        bytes.extend_from_slice(&[0xee; 32]);

        let r = decode(&bytes).unwrap();
        assert_eq!(r.state_roots.len(), 1);
        let key = L2BatchKey {
            l2_chain_id_hash: [0u8; 32],
            batch_id: 5,
        };
        let rec = r.state_roots.get(&key).unwrap();
        assert_eq!(rec.state_root, [0x11; 32]);
        assert_eq!(rec.committed_at_l1_height, 100);
        assert_eq!(rec.vk_hash, [0xab; 32]);
        assert_eq!(rec.da_commitment, [0xee; 32]);
    }

    #[test]
    fn decode_rejects_size_mismatch() {
        // V2 header claims 2 state roots but provides 1.
        let mut bytes = vec![0, 0, 0, 2]; // version
        bytes.extend_from_slice(&[0, 0, 0, 0]); // chain_vk count = 0
        bytes.extend_from_slice(&[0, 0, 0, 2]); // state_root count = 2
        bytes.extend_from_slice(&[0u8; ENCODED_STATE_ROOT_ENTRY_BYTES]); // only 1 entry
        assert!(matches!(
            decode(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }

    #[test]
    fn decode_rejects_short_header() {
        // 8 bytes — less than the minimum 12-byte v2 header.
        assert!(matches!(
            decode(&[0u8; 8]),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }

    #[test]
    fn decode_rejects_wrong_version() {
        let mut bytes = vec![0, 0, 0, 99]; // version = 99
        bytes.extend_from_slice(&[0u8; 8]);
        assert!(matches!(
            decode(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }

    #[test]
    fn entry_size_constants_match_packed_layout() {
        assert_eq!(L2_STATE_ROOT_RECORD_BYTES, 32 + 8 + 32 + 32);
        assert_eq!(
            ENCODED_STATE_ROOT_ENTRY_BYTES,
            32 + 8 + L2_STATE_ROOT_RECORD_BYTES
        );
        assert_eq!(ENCODED_CHAIN_VKS_ENTRY_BYTES, 32 + 32 + 32);
    }
}
