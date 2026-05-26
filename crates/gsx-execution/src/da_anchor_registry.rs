//! DA-anchor registry storage (Track G G3.3 hardening).
//!
//! Records the BLAKE3 hash of each per-batch DA blob anchored
//! via `Intent::PostL2DAv2`. Before this registry, the original
//! `Intent::PostL2DA` was a substrate-level no-op — the blob
//! lived in L1 calldata (consensus-level state) but the substrate
//! had no deterministic record that the blob was anchored at all.
//!
//! `PostL2DA` (no-op) remains unchanged for wire-format
//! compatibility per [IQ-007](../../docs/iq/IQ-007-intent-discriminant-stability.md);
//! the deterministic anchoring lives on the new
//! `Intent::PostL2DAv2` variant.
//!
//! ## What it defends against
//!
//! Inconsistency between the actual DA blob bytes anchored
//! to L1 calldata and the `da_commitment` value claimed in a
//! later `CommitL2StateRoot` proof. The substrate now keeps
//! `BLAKE3(da_blob)` (matching the sequencer's
//! `da_commitment` formula by construction) keyed by
//! `(l2_chain_id_hash, batch_id)`. Off-chain auditors can
//! cross-check: blob bytes from L1 calldata → BLAKE3 →
//! registry value → `da_commitment` in the matching
//! `L2StateRootRecord`. All three hashes are the same 32
//! bytes by construction.
//!
//! ## Replay defense
//!
//! Re-anchoring the same `(chain, batch)` rejects with
//! `DaAnchorAlreadyRecorded`. Once anchored the blob hash
//! is immutable — guarantees off-chain auditors can rely
//! on a single canonical commitment per batch.
//!
//! ## Encoding
//!
//! ```text
//! u32::BE(VERSION = 1) ||
//! u32::BE(count) ||
//!   foreach ((l2_chain_id_hash, batch_id), blob_hash) in
//!           ascending order:
//!     l2_chain_id_hash (32 B) ||
//!     batch_id (u64::BE, 8 B) ||
//!     blob_hash (32 B)
//! ```

use std::collections::BTreeMap;

use crate::{error::ExecutionError, l2_state::L2BatchKey, reserved};

/// Current encoding version.
pub const DA_ANCHOR_REGISTRY_VERSION: u32 = 1;

/// Encoded-header bytes: `version (4) + count (4) = 8`.
pub const ENCODED_HEADER_BYTES: usize = 4 + 4;

/// Encoded bytes per entry: key (32 + 8) + value (32) = 72.
pub const ENTRY_BYTES: usize = 32 + 8 + 32;

/// Encode the DA-anchor map to the canonical byte sequence.
pub fn encode(map: &BTreeMap<L2BatchKey, [u8; 32]>) -> Vec<u8> {
    let mut buf = Vec::with_capacity(ENCODED_HEADER_BYTES + map.len() * ENTRY_BYTES);
    buf.extend_from_slice(&DA_ANCHOR_REGISTRY_VERSION.to_be_bytes());
    buf.extend_from_slice(&(map.len() as u32).to_be_bytes());
    for (key, hash) in map {
        buf.extend_from_slice(&key.l2_chain_id_hash);
        buf.extend_from_slice(&key.batch_id.to_be_bytes());
        buf.extend_from_slice(hash);
    }
    buf
}

/// Decode the byte sequence back into the DA-anchor map.
pub fn decode(bytes: &[u8]) -> Result<BTreeMap<L2BatchKey, [u8; 32]>, ExecutionError> {
    if bytes.is_empty() {
        return Ok(BTreeMap::new());
    }
    if bytes.len() < ENCODED_HEADER_BYTES {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::da_anchor_registry_address(),
            reason: "DA anchor registry header missing",
        });
    }
    let version = u32::from_be_bytes(bytes[0..4].try_into().unwrap());
    if version != DA_ANCHOR_REGISTRY_VERSION {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::da_anchor_registry_address(),
            reason: "DA anchor registry version mismatch",
        });
    }
    let count = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
    // `count` is bounded by u32::MAX. On 64-bit `count * ENTRY_BYTES`
    // fits in usize, but the SP1 guest builds for riscv32 (32-bit usize)
    // where a crafted header could wrap and pass the length check,
    // then panic in the loop's slice indexing. Same hardening as
    // unbonding_registry (#230 P1 #1).
    let expected_len = count
        .checked_mul(ENTRY_BYTES)
        .and_then(|payload| payload.checked_add(ENCODED_HEADER_BYTES))
        .ok_or(ExecutionError::CorruptStateRecord {
            addr: reserved::da_anchor_registry_address(),
            reason: "DA anchor registry count overflows usize",
        })?;
    if bytes.len() != expected_len {
        return Err(ExecutionError::CorruptStateRecord {
            addr: reserved::da_anchor_registry_address(),
            reason: "DA anchor registry size mismatch",
        });
    }
    let mut map = BTreeMap::new();
    let mut cursor = ENCODED_HEADER_BYTES;
    for _ in 0..count {
        let mut l2_chain_id_hash = [0u8; 32];
        l2_chain_id_hash.copy_from_slice(&bytes[cursor..cursor + 32]);
        cursor += 32;
        let batch_id = u64::from_be_bytes(bytes[cursor..cursor + 8].try_into().unwrap());
        cursor += 8;
        let mut blob_hash = [0u8; 32];
        blob_hash.copy_from_slice(&bytes[cursor..cursor + 32]);
        cursor += 32;
        let key = L2BatchKey {
            l2_chain_id_hash,
            batch_id,
        };
        if map.insert(key, blob_hash).is_some() {
            return Err(ExecutionError::CorruptStateRecord {
                addr: reserved::da_anchor_registry_address(),
                reason: "DA anchor registry has duplicate keys",
            });
        }
    }
    Ok(map)
}

/// Compute the canonical anchor hash of a DA blob — plain
/// `BLAKE3(da_blob)`. Matches the sequencer's `da_commitment`
/// recipe (`crates/gsx-l2-sequencer/src/lib.rs`'s
/// `BatchHeader.da_commitment` field) by construction, so
/// off-chain auditors can directly compare the registry value
/// to the `da_commitment` in the matching `L2StateRootRecord`:
/// L1 calldata bytes → BLAKE3 → registry value (this fn) →
/// `da_commitment` claimed in the proof. All four are the
/// same 32 bytes.
///
/// An earlier draft used a domain-tagged form
/// (`BLAKE3("gsx-da-blob-v1" || da_blob)`), which is better
/// hash hygiene in isolation but does NOT match the producer's
/// emitted commitment — Codex flagged that mismatch as a
/// load-bearing-invariant violation since the registry's
/// stated purpose IS the cross-check against `da_commitment`.
pub fn da_blob_hash(da_blob: &[u8]) -> [u8; 32] {
    *blake3::hash(da_blob).as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(chain: u8, batch_id: u64) -> L2BatchKey {
        L2BatchKey {
            l2_chain_id_hash: [chain; 32],
            batch_id,
        }
    }

    #[test]
    fn empty_round_trip() {
        let m: BTreeMap<L2BatchKey, [u8; 32]> = BTreeMap::new();
        assert_eq!(decode(&encode(&m)).unwrap(), m);
    }

    #[test]
    fn single_record_round_trip() {
        let mut m = BTreeMap::new();
        m.insert(key(0, 7), [0xaa; 32]);
        assert_eq!(decode(&encode(&m)).unwrap(), m);
    }

    #[test]
    fn multi_record_round_trip_deterministic_order() {
        let mut m1 = BTreeMap::new();
        m1.insert(key(1, 0), [0xa1; 32]);
        m1.insert(key(2, 0), [0xa2; 32]);
        m1.insert(key(2, 5), [0xb2; 32]);

        let mut m2 = BTreeMap::new();
        m2.insert(key(2, 5), [0xb2; 32]);
        m2.insert(key(1, 0), [0xa1; 32]);
        m2.insert(key(2, 0), [0xa2; 32]);

        assert_eq!(encode(&m1), encode(&m2));
        assert_eq!(decode(&encode(&m1)).unwrap(), m1);
    }

    #[test]
    fn da_blob_hash_is_deterministic() {
        let a = da_blob_hash(b"hello");
        let b = da_blob_hash(b"hello");
        assert_eq!(a, b);
    }

    #[test]
    fn da_blob_hash_distinguishes_inputs() {
        assert_ne!(da_blob_hash(b"hello"), da_blob_hash(b"world"));
        assert_ne!(da_blob_hash(b""), da_blob_hash(b"\x00"));
    }

    /// The anchor hash MUST equal plain `BLAKE3(da_blob)` —
    /// any divergence from the sequencer's `da_commitment`
    /// formula breaks the load-bearing cross-check invariant
    /// off-chain auditors rely on.
    #[test]
    fn da_blob_hash_matches_plain_blake3() {
        let plain = *blake3::hash(b"hello").as_bytes();
        let anchored = da_blob_hash(b"hello");
        assert_eq!(anchored, plain);
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
        bytes.extend_from_slice(&DA_ANCHOR_REGISTRY_VERSION.to_be_bytes());
        bytes.extend_from_slice(&1u32.to_be_bytes());
        assert!(matches!(
            decode(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }

    /// Regression: a crafted header with `count = u32::MAX` and an
    /// empty body must reject cleanly (CorruptStateRecord) and never
    /// panic — protects 32-bit guest targets (riscv32, SP1) from a
    /// length-check wrap that would let the parse loop run past
    /// the end of the slice.
    #[test]
    fn decode_rejects_huge_count_without_panic() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&DA_ANCHOR_REGISTRY_VERSION.to_be_bytes());
        bytes.extend_from_slice(&u32::MAX.to_be_bytes());
        // body intentionally empty (bytes.len() == 8 == header only)
        assert!(matches!(
            decode(&bytes),
            Err(ExecutionError::CorruptStateRecord { .. })
        ));
    }
}
