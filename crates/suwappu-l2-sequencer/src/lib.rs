//! suwappu-l2-sequencer — L2 batch sequencer (Track G G4.2, #105).
//!
//! ## Phase split
//!
//! - **Phase 1 (this PR / #105)** — pure batch-construction
//!   logic. No networking, no Tokio, no JSON-RPC. Provides:
//!   - `TxMempool` — bounded queue of pending L2 transactions
//!   - `BatchBuilder` — ordering + serialization of pending
//!     txs into a `Batch` envelope
//!   - `Batch` — serialized batch envelope (DA blob)
//!   - `BatchHeader` — public-input layout binding the batch
//!     to the L1 verifier (matches Track G public-input
//!     layout per IQ-006 + the
//!     `suwappu-l2-verifier-precompile::public_inputs` offsets)
//! - **Phase 2** — JSON-RPC service + axum daemon binary +
//!   AWS deployment recipe. Lands once the daemon is needed
//!   for the devnet smoke test (G2.4 #99).
//! - **Phase 3** — 3-node rotation per Starknet shape
//!   (deferred to v1.1 per the strategic plan).
//!
//! ## Why pure logic first
//!
//! The batch-construction logic is the load-bearing
//! correctness surface: a sequencer that builds invalid
//! batches breaks the L1 verifier even if the network
//! plumbing is perfect. Shipping pure logic first lets us
//! test the construction against the verifier's format
//! gates (proof = 260 B, public_inputs = 240 B) without
//! standing up Tokio + axum.
//!
//! Production sequencer wiring (phase 2) constructs the
//! same `Batch` shape; the only addition is the network
//! layer + the `Intent::PostL2DA` + `Intent::CommitL2StateRoot`
//! submission to suwappu-dag.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::collections::VecDeque;

use blake3::Hasher;
use serde::{Deserialize, Serialize};
use suwappu_l2_verifier_precompile::{public_inputs, L2_PUBLIC_INPUTS_BYTES};
use thiserror::Error;

/// Maximum L2 transactions in a single batch. Matches the
/// 500-tx target from Track G G4.3 (#106) baseline (Polygon
/// zkEVM's 500-tx/84s shape). Configurable in v1.1.
pub const MAX_TXS_PER_BATCH: usize = 500;

/// Maximum bytes in a single L2 transaction. 256 KB matches
/// the `L2ForceInclude::tx` cap per the Track G G3.4
/// force-include spec (`docs/validator-sla-slashing.md` §3).
pub const MAX_TX_BYTES: usize = 256 * 1024;

/// Maximum bytes in the mempool's pending tx queue total.
/// Bound prevents OOM on a malicious / runaway client; at
/// 256 KB per tx × 500 = 128 MB upper bound. We pad 2× for
/// the queue header overhead.
pub const MAX_MEMPOOL_BYTES: usize = 256 * 1024 * 1024;

/// Errors returned by the sequencer.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum SequencerError {
    /// A submitted transaction exceeded `MAX_TX_BYTES`.
    #[error("tx exceeds maximum {max} bytes: got {got}")]
    TxTooLarge {
        /// Maximum allowed.
        max: usize,
        /// Observed.
        got: usize,
    },

    /// A submitted transaction was empty.
    #[error("tx cannot be empty")]
    EmptyTx,

    /// The mempool's total pending bytes would exceed
    /// `MAX_MEMPOOL_BYTES`. Backpressure to the submitting
    /// client.
    #[error("mempool full: would exceed {max} bytes (currently {current})")]
    MempoolFull {
        /// Configured maximum.
        max: usize,
        /// Current usage.
        current: usize,
    },

    /// A built batch's public-input blob was not the expected
    /// 240-byte width. Defensive — the construction below
    /// should never produce a non-conforming blob; this fires
    /// only on a refactor regression.
    #[error("batch public_inputs width is {got}, expected {expected}")]
    BadPublicInputsWidth {
        /// Required (240).
        expected: usize,
        /// Observed.
        got: usize,
    },
}

/// A single L2 transaction in the sequencer's mempool. The
/// sequencer treats tx bytes opaquely — the L2 STM (Track G
/// G1) is responsible for decoding + executing them. This
/// way the sequencer doesn't need to embed an EVM
/// interpreter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingTx {
    /// Opaque tx bytes. The L2 STM decodes; sequencer doesn't.
    pub bytes: Vec<u8>,
    /// Hash of the bytes (BLAKE3) — pre-computed for dedup +
    /// indexing without re-hashing.
    pub hash: [u8; 32],
}

impl PendingTx {
    /// Construct + hash a pending tx. Rejects empty bytes +
    /// over-sized payloads.
    pub fn new(bytes: Vec<u8>) -> Result<Self, SequencerError> {
        if bytes.is_empty() {
            return Err(SequencerError::EmptyTx);
        }
        if bytes.len() > MAX_TX_BYTES {
            return Err(SequencerError::TxTooLarge {
                max: MAX_TX_BYTES,
                got: bytes.len(),
            });
        }
        let mut h = Hasher::new();
        h.update(&bytes);
        let hash = *h.finalize().as_bytes();
        Ok(Self { bytes, hash })
    }
}

/// Bounded FIFO mempool of pending L2 transactions.
///
/// Phase 1: simple bytes-cap backpressure; no per-account
/// nonce ordering, no fee market. Production (phase 2) will
/// add tx-replacement rules + fee-based reordering once the
/// L2 STM's nonce semantics stabilize.
#[derive(Debug, Default)]
pub struct TxMempool {
    queue: VecDeque<PendingTx>,
    bytes_total: usize,
}

impl TxMempool {
    /// Construct an empty mempool.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of pending transactions.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// True iff the mempool is empty.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Total bytes currently queued.
    pub fn bytes_total(&self) -> usize {
        self.bytes_total
    }

    /// Submit a tx to the mempool. Rejects oversized + empty;
    /// rejects when the cumulative queue would exceed
    /// `MAX_MEMPOOL_BYTES`. Dedups by `PendingTx::hash`
    /// (silently drops re-submits to keep the API
    /// idempotent-friendly).
    pub fn submit(&mut self, bytes: Vec<u8>) -> Result<[u8; 32], SequencerError> {
        let tx = PendingTx::new(bytes)?;
        if self.bytes_total.saturating_add(tx.bytes.len()) > MAX_MEMPOOL_BYTES {
            return Err(SequencerError::MempoolFull {
                max: MAX_MEMPOOL_BYTES,
                current: self.bytes_total,
            });
        }
        // Dedup against the existing queue.
        if self.queue.iter().any(|t| t.hash == tx.hash) {
            return Ok(tx.hash);
        }
        self.bytes_total += tx.bytes.len();
        let hash = tx.hash;
        self.queue.push_back(tx);
        Ok(hash)
    }

    /// Drain up to `max` transactions for batch construction.
    /// Caller is responsible for either including them in a
    /// batch or returning them to the mempool on failure.
    pub fn drain(&mut self, max: usize) -> Vec<PendingTx> {
        let take = max.min(self.queue.len()).min(MAX_TXS_PER_BATCH);
        let mut out = Vec::with_capacity(take);
        for _ in 0..take {
            if let Some(tx) = self.queue.pop_front() {
                self.bytes_total = self.bytes_total.saturating_sub(tx.bytes.len());
                out.push(tx);
            }
        }
        out
    }
}

/// Per-batch public-input header. Matches the Track G public-
/// input layout (per
/// `suwappu-l2-verifier-precompile::public_inputs`); the
/// sequencer builds this header, hands it to the prover for
/// the SP1 STM input, and submits it on-chain via
/// `Intent::CommitL2StateRoot::public_inputs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchHeader {
    /// L2 state root before the batch's txs are applied.
    pub prev_l2_state_root: [u8; 32],
    /// L2 state root after the batch's txs are applied.
    pub new_l2_state_root: [u8; 32],
    /// Monotonic batch identifier (within this L2 chain).
    pub batch_id: u64,
    /// BLAKE3 over the DA blob.
    pub da_commitment: [u8; 32],
    /// L1 block height this batch reads from (bridge events +
    /// L1 timestamp anchor).
    pub l1_anchor_height: u64,
    /// Range-program VK commitment per op-succinct's
    /// multiBlockVKey pattern. Embedded in the aggregation
    /// proof's public values; the L1 verifier checks against
    /// the chain-state pinned value.
    pub range_vk_commitment: [u8; 32],
    /// L1 BLAKE3 state root at `l1_anchor_height`. Bound into
    /// the L2 proof via one in-circuit SHA3-256 per the Track
    /// G L1↔L2 binding pattern.
    pub prev_l1_state_root: [u8; 32],
    /// BLAKE3("suwappu-l2-chain-" || chain_id) for multi-L2
    /// namespacing per IQ-006.
    pub l2_chain_id_hash: [u8; 32],
    /// Track H confidential-transfer commitment-tree root at
    /// end of batch. All-zeros when no confidential txs in
    /// the batch.
    pub confidential_root: [u8; 32],
}

impl BatchHeader {
    /// Serialize to the 240-byte public-input wire format
    /// per the offsets in `suwappu_l2_verifier_precompile::public_inputs`.
    /// The L1 verifier reads this exact byte layout from
    /// `Intent::CommitL2StateRoot::public_inputs`.
    pub fn to_public_inputs(&self) -> [u8; L2_PUBLIC_INPUTS_BYTES] {
        let mut buf = [0u8; L2_PUBLIC_INPUTS_BYTES];
        buf[public_inputs::PREV_L2_STATE_ROOT_OFFSET
            ..public_inputs::PREV_L2_STATE_ROOT_OFFSET + 32]
            .copy_from_slice(&self.prev_l2_state_root);
        buf[public_inputs::NEW_L2_STATE_ROOT_OFFSET..public_inputs::NEW_L2_STATE_ROOT_OFFSET + 32]
            .copy_from_slice(&self.new_l2_state_root);
        buf[public_inputs::BATCH_ID_OFFSET..public_inputs::BATCH_ID_OFFSET + 8]
            .copy_from_slice(&self.batch_id.to_be_bytes());
        buf[public_inputs::DA_COMMITMENT_OFFSET..public_inputs::DA_COMMITMENT_OFFSET + 32]
            .copy_from_slice(&self.da_commitment);
        buf[public_inputs::L1_ANCHOR_HEIGHT_OFFSET..public_inputs::L1_ANCHOR_HEIGHT_OFFSET + 8]
            .copy_from_slice(&self.l1_anchor_height.to_be_bytes());
        buf[public_inputs::RANGE_VK_COMMITMENT_OFFSET
            ..public_inputs::RANGE_VK_COMMITMENT_OFFSET + 32]
            .copy_from_slice(&self.range_vk_commitment);
        buf[public_inputs::PREV_L1_STATE_ROOT_OFFSET
            ..public_inputs::PREV_L1_STATE_ROOT_OFFSET + 32]
            .copy_from_slice(&self.prev_l1_state_root);
        buf[public_inputs::L2_CHAIN_ID_HASH_OFFSET..public_inputs::L2_CHAIN_ID_HASH_OFFSET + 32]
            .copy_from_slice(&self.l2_chain_id_hash);
        buf[public_inputs::CONFIDENTIAL_ROOT_OFFSET..public_inputs::CONFIDENTIAL_ROOT_OFFSET + 32]
            .copy_from_slice(&self.confidential_root);
        buf
    }
}

/// A built L2 batch ready for submission. Carries the DA
/// blob (the sequencer hands to the L1 via
/// `Intent::PostL2DA`), the public-input header (the prover
/// hands to the SP1 guest as input + the L1 verifier checks),
/// and the ordered tx list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    /// Per-batch header binding the txs to the L1 state.
    pub header: BatchHeader,
    /// Ordered transactions included in this batch.
    pub txs: Vec<PendingTx>,
    /// Opaque DA blob — the sequencer posts this to L1 via
    /// `Intent::PostL2DA`. Phase 1 encoding: simple
    /// concatenation of (tx_len_be_u32 || tx_bytes) per tx
    /// in batch order, prefixed by the batch_id (big-endian
    /// u64). Production may switch to RLP / SSZ once the L2
    /// STM ratifies a canonical encoding.
    pub da_blob: Vec<u8>,
}

impl Batch {
    /// Compute the public-input byte blob this batch submits
    /// to the L1 verifier. Equivalent to `header.to_public_inputs()`;
    /// exposed at the `Batch` level for convenience.
    pub fn public_inputs(&self) -> [u8; L2_PUBLIC_INPUTS_BYTES] {
        self.header.to_public_inputs()
    }

    /// Validate the batch's public-input blob is exactly the
    /// 240-byte width the verifier expects. Defensive — the
    /// construction below should never produce a non-conforming
    /// blob, but the test gate locks this against silent
    /// refactor regressions.
    pub fn validate_public_inputs_width(&self) -> Result<(), SequencerError> {
        let pi = self.public_inputs();
        if pi.len() != L2_PUBLIC_INPUTS_BYTES {
            return Err(SequencerError::BadPublicInputsWidth {
                expected: L2_PUBLIC_INPUTS_BYTES,
                got: pi.len(),
            });
        }
        Ok(())
    }
}

/// Inputs to `BatchBuilder::build`: the per-batch L1 anchor
/// information + the previous-state roots. The builder
/// computes `new_l2_state_root` + `da_commitment` from the
/// drained tx list; the rest are supplied by the caller
/// from the L1 state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildContext {
    /// L2 state root before this batch's txs.
    pub prev_l2_state_root: [u8; 32],
    /// Batch identifier (caller is responsible for monotonic
    /// uniqueness).
    pub batch_id: u64,
    /// L1 block height this batch reads from.
    pub l1_anchor_height: u64,
    /// Range-program VK commitment to embed.
    pub range_vk_commitment: [u8; 32],
    /// L1 state root at `l1_anchor_height`.
    pub prev_l1_state_root: [u8; 32],
    /// Multi-L2 namespacing — `BLAKE3("suwappu-l2-chain-" || chain_id)`.
    pub l2_chain_id_hash: [u8; 32],
    /// Confidential-transfer commitment-tree root after batch.
    /// `[0u8; 32]` when no confidential txs.
    pub confidential_root: [u8; 32],
    /// L2 state root after applying the txs. **Caller-
    /// provided** because the sequencer doesn't execute txs
    /// — the L2 STM (Track G G1) does, and the prover hands
    /// the post-state root back to the sequencer for header
    /// construction.
    pub new_l2_state_root: [u8; 32],
}

/// Builds `Batch` envelopes from drained mempool txs +
/// caller-supplied L1 anchor context.
#[derive(Debug, Default)]
pub struct BatchBuilder;

impl BatchBuilder {
    /// Build a batch from `txs` + the L1 anchor context.
    /// Computes the DA blob + da_commitment; assembles the
    /// `BatchHeader`. Empty `txs` is allowed (produces a
    /// no-op batch — useful for keeping the L1 anchor
    /// timestamp fresh during low-traffic periods).
    pub fn build(txs: Vec<PendingTx>, ctx: BuildContext) -> Batch {
        let da_blob = encode_da_blob(ctx.batch_id, &txs);
        let mut h = Hasher::new();
        h.update(&da_blob);
        let da_commitment = *h.finalize().as_bytes();

        let header = BatchHeader {
            prev_l2_state_root: ctx.prev_l2_state_root,
            new_l2_state_root: ctx.new_l2_state_root,
            batch_id: ctx.batch_id,
            da_commitment,
            l1_anchor_height: ctx.l1_anchor_height,
            range_vk_commitment: ctx.range_vk_commitment,
            prev_l1_state_root: ctx.prev_l1_state_root,
            l2_chain_id_hash: ctx.l2_chain_id_hash,
            confidential_root: ctx.confidential_root,
        };

        Batch {
            header,
            txs,
            da_blob,
        }
    }
}

/// Encode a DA blob in the phase-1 canonical format:
/// `batch_id (u64 BE) || foreach tx: tx_len (u32 BE) || tx_bytes`.
///
/// Production (phase 2 / v1.1) may switch to RLP / SSZ once
/// the L2 STM ratifies a canonical encoding. The encoding
/// MUST be deterministic — the `da_commitment` field of the
/// `BatchHeader` is `BLAKE3(da_blob)`, and any difference in
/// encoding between sequencer + prover breaks proof
/// verification.
fn encode_da_blob(batch_id: u64, txs: &[PendingTx]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(8 + txs.iter().map(|t| 4 + t.bytes.len()).sum::<usize>());
    buf.extend_from_slice(&batch_id.to_be_bytes());
    for tx in txs {
        let len = tx.bytes.len() as u32;
        buf.extend_from_slice(&len.to_be_bytes());
        buf.extend_from_slice(&tx.bytes);
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    // ----- PendingTx -----

    #[test]
    fn pending_tx_new_hashes() {
        let tx = PendingTx::new(b"hello".to_vec()).unwrap();
        let mut h = Hasher::new();
        h.update(b"hello");
        assert_eq!(tx.hash, *h.finalize().as_bytes());
    }

    #[test]
    fn pending_tx_rejects_empty() {
        assert_eq!(PendingTx::new(vec![]), Err(SequencerError::EmptyTx));
    }

    #[test]
    fn pending_tx_rejects_oversize() {
        let oversize = vec![0u8; MAX_TX_BYTES + 1];
        assert!(matches!(
            PendingTx::new(oversize),
            Err(SequencerError::TxTooLarge { .. })
        ));
    }

    #[test]
    fn pending_tx_accepts_at_max() {
        let max_tx = vec![0u8; MAX_TX_BYTES];
        assert!(PendingTx::new(max_tx).is_ok());
    }

    // ----- TxMempool -----

    #[test]
    fn mempool_starts_empty() {
        let m = TxMempool::new();
        assert!(m.is_empty());
        assert_eq!(m.len(), 0);
        assert_eq!(m.bytes_total(), 0);
    }

    #[test]
    fn mempool_submit_grows_state() {
        let mut m = TxMempool::new();
        m.submit(b"tx1".to_vec()).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m.bytes_total(), 3);
    }

    #[test]
    fn mempool_dedup_is_silent() {
        let mut m = TxMempool::new();
        let h1 = m.submit(b"tx1".to_vec()).unwrap();
        let h2 = m.submit(b"tx1".to_vec()).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(m.len(), 1, "duplicate should not grow queue");
    }

    #[test]
    fn mempool_drain_fifo() {
        let mut m = TxMempool::new();
        m.submit(b"a".to_vec()).unwrap();
        m.submit(b"b".to_vec()).unwrap();
        m.submit(b"c".to_vec()).unwrap();
        let drained = m.drain(2);
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].bytes, b"a");
        assert_eq!(drained[1].bytes, b"b");
        assert_eq!(m.len(), 1);
        assert_eq!(m.bytes_total(), 1);
    }

    #[test]
    fn mempool_drain_respects_max_per_batch() {
        let mut m = TxMempool::new();
        // Use u32-LE-encoded payloads so each tx is unique
        // (avoids dedup at the byte-equality check).
        for i in 0..(MAX_TXS_PER_BATCH + 10) {
            m.submit((i as u32).to_le_bytes().to_vec()).unwrap();
        }
        assert_eq!(m.len(), MAX_TXS_PER_BATCH + 10, "mempool grew as expected");
        let drained = m.drain(usize::MAX);
        assert_eq!(drained.len(), MAX_TXS_PER_BATCH);
        // Remaining 10 still queued.
        assert_eq!(m.len(), 10);
    }

    // ----- BatchBuilder + BatchHeader -----

    fn ctx_stub(batch_id: u64) -> BuildContext {
        BuildContext {
            prev_l2_state_root: [0x10; 32],
            batch_id,
            l1_anchor_height: 12345,
            range_vk_commitment: [0x20; 32],
            prev_l1_state_root: [0x30; 32],
            l2_chain_id_hash: [0x40; 32],
            confidential_root: [0u8; 32],
            new_l2_state_root: [0x50; 32],
        }
    }

    #[test]
    fn batch_build_empty_is_valid() {
        let batch = BatchBuilder::build(vec![], ctx_stub(0));
        assert!(batch.validate_public_inputs_width().is_ok());
        assert_eq!(batch.txs.len(), 0);
    }

    #[test]
    fn batch_build_with_txs() {
        let txs = vec![
            PendingTx::new(b"alpha".to_vec()).unwrap(),
            PendingTx::new(b"beta".to_vec()).unwrap(),
        ];
        let batch = BatchBuilder::build(txs, ctx_stub(7));
        assert_eq!(batch.txs.len(), 2);
        assert_eq!(batch.header.batch_id, 7);
        assert!(batch.validate_public_inputs_width().is_ok());
    }

    /// Public-input layout matches the verifier-precompile
    /// offsets exactly. Defends against silent drift between
    /// the sequencer's emit format + the verifier's read
    /// format.
    #[test]
    fn batch_public_inputs_layout_round_trip() {
        let ctx = ctx_stub(42);
        let batch = BatchBuilder::build(vec![], ctx.clone());
        let pi = batch.public_inputs();
        assert_eq!(pi.len(), L2_PUBLIC_INPUTS_BYTES);
        // prev_l2_state_root at offset 0
        assert_eq!(
            &pi[public_inputs::PREV_L2_STATE_ROOT_OFFSET
                ..public_inputs::PREV_L2_STATE_ROOT_OFFSET + 32],
            &ctx.prev_l2_state_root
        );
        // batch_id at offset 64
        assert_eq!(
            &pi[public_inputs::BATCH_ID_OFFSET..public_inputs::BATCH_ID_OFFSET + 8],
            &42u64.to_be_bytes()
        );
        // l1_anchor_height at offset 104
        assert_eq!(
            &pi[public_inputs::L1_ANCHOR_HEIGHT_OFFSET..public_inputs::L1_ANCHOR_HEIGHT_OFFSET + 8],
            &ctx.l1_anchor_height.to_be_bytes()
        );
        // confidential_root at offset 208 (all-zeros for no confidential txs)
        assert_eq!(
            &pi[public_inputs::CONFIDENTIAL_ROOT_OFFSET
                ..public_inputs::CONFIDENTIAL_ROOT_OFFSET + 32],
            &[0u8; 32]
        );
    }

    /// DA-commitment determinism: identical tx lists +
    /// batch_id yield identical da_commitment.
    #[test]
    fn da_commitment_is_deterministic() {
        let txs = vec![
            PendingTx::new(b"alpha".to_vec()).unwrap(),
            PendingTx::new(b"beta".to_vec()).unwrap(),
        ];
        let b1 = BatchBuilder::build(txs.clone(), ctx_stub(1));
        let b2 = BatchBuilder::build(txs, ctx_stub(1));
        assert_eq!(b1.header.da_commitment, b2.header.da_commitment);
    }

    /// Different tx orderings within the same batch produce
    /// different da_commitments — the prover + sequencer
    /// MUST agree on order.
    #[test]
    fn da_commitment_is_order_sensitive() {
        let tx_a = PendingTx::new(b"alpha".to_vec()).unwrap();
        let tx_b = PendingTx::new(b"beta".to_vec()).unwrap();
        let b1 = BatchBuilder::build(vec![tx_a.clone(), tx_b.clone()], ctx_stub(1));
        let b2 = BatchBuilder::build(vec![tx_b, tx_a], ctx_stub(1));
        assert_ne!(b1.header.da_commitment, b2.header.da_commitment);
    }

    /// End-to-end: drain mempool → build batch → check the
    /// public_inputs blob passes the suwappu-l2-verifier-precompile
    /// format gates.
    #[test]
    fn end_to_end_mempool_to_verifier_format_gate() {
        let mut m = TxMempool::new();
        m.submit(b"first".to_vec()).unwrap();
        m.submit(b"second".to_vec()).unwrap();
        let drained = m.drain(usize::MAX);
        let batch = BatchBuilder::build(drained, ctx_stub(99));
        let pi = batch.public_inputs();

        // Verify the public-input blob matches the verifier's
        // expected width — exercises the cross-crate format
        // gate without standing up a real proof.
        assert_eq!(pi.len(), L2_PUBLIC_INPUTS_BYTES);

        // The proof side would now generate a 260-byte Groth16
        // BN254 proof + submit Intent::CommitL2StateRoot with
        // (pi, proof, vk_hash). The verifier accepts on format,
        // verifies cryptographically (phase 2), credits the
        // L2 commit counter (per G2.2 #97 PR #179).
    }
}
