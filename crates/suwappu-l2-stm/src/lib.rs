//! `suwappu-l2-stm` — L2 state transition function. **Native reference
//! implementation.** The SP1 RISC-V guest program that proves this
//! STM under SP1 Groth16 BN254 lands alongside this crate under
//! `program/` in a follow-up PR (Phase 1.1 of the L2 mainnet-ready
//! arc per `~/.claude/plans/validated-prancing-curry.md`).
//!
//! ## What this crate ships today
//!
//! 1. **Type surface** — `BatchInput`, `BatchOutput`,
//!    `BatchTransaction` — the inputs the prover sees and the
//!    outputs the verifier reads. Fixed byte layouts where they
//!    interface with the verifier (`to_public_inputs` is the
//!    canonical 240-byte serialization).
//!
//! 2. **Native reference STM** — [`execute_batch`]. Takes a
//!    `BatchInput` (previous L2 state, raw DA blob, anchor info),
//!    decodes the batch's transactions, applies them to a simple
//!    `BTreeMap<Address, Account>` ledger, and returns a
//!    `BatchOutput` with the new state root + da_commitment +
//!    confidential_root + range_vk_commitment.
//!
//! 3. **Public-input serialization** — [`to_public_inputs`].
//!    Produces the exact 240-byte blob that `sp1_verifier::
//!    Groth16Verifier::verify` expects, ordered per
//!    `suwappu_l2_verifier_precompile::public_inputs::*`. Determinism
//!    + the layout invariant are property-tested at 10k cases.
//!
//! ## What this crate does NOT ship yet
//!
//! - **Real EVM execution.** Transfer-only for now. revm-in-guest
//!   integration lands in a v1 follow-up; until then the L2 is
//!   "value-transfer only" — useful for the testnet dogfood pass
//!   (Phase A6 of the operator plan) but not a developer-facing
//!   smart-contract chain.
//! - **The SP1 guest binary.** This crate is host-side only.
//!   The guest at `crates/suwappu-l2-stm/program/` (forthcoming)
//!   imports this same lib, calls [`execute_batch`], and emits
//!   [`to_public_inputs`] as its `sp1_zkvm::io::commit` output.
//!   Once that lands, the prover daemon (Phase 2.1) shells out
//!   to `cargo prove` against the guest ELF.
//! - **Confidential transactions.** `confidential_root` is
//!   plumbed through the type surface but defaults to all-zeros
//!   until Track H's nullifier-set updates fold into the STM
//!   (Phase 1.1 v1.1 sub-task).
//!
//! ## DA blob format (matches `suwappu-l2-sequencer`)
//!
//! ```text
//! u64::BE(batch_id) ||
//!   foreach tx in batch.txs:
//!     u32::BE(tx_len) ||
//!     tx_bytes
//! ```
//!
//! Each `tx_bytes` is the canonical serialization of one
//! `BatchTransaction` (see [`BatchTransaction::decode`]).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::collections::BTreeMap;

use suwappu_l2_verifier_precompile::{public_inputs as pi_offsets, L2_PUBLIC_INPUTS_BYTES};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 20-byte L2 address. Same shape as the L1 substrate's
/// `suwappu_execution::Address` so the bridge can credit
/// L2-recipient addresses without translation.
pub type Address = [u8; 20];

/// L2 balance type. u128 to match the L1 substrate's
/// `suwappu_execution::Balance`.
pub type Balance = u128;

/// 32-byte hash type (state root, commitment, vk hash, chain id).
pub type Hash32 = [u8; 32];

// ---- Account model ---------------------------------------------------------

/// Minimal L2 account state. Pre-EVM; expanded to include code,
/// storage root, and EVM-style nonce when the revm-in-guest
/// integration lands.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// Account balance.
    pub balance: Balance,
    /// Replay-defense nonce. Increments on every successful
    /// `Transfer` from this address.
    pub nonce: u64,
}

// ---- Transaction model -----------------------------------------------------

/// One L2 transaction. Transfer-only in v1; the `#[non_exhaustive]`
/// marker signals SDK consumers must match with a wildcard arm so
/// adding variants (revm `Call`, contract `Create`, etc.) doesn't
/// break them. Same protective-enum pattern as
/// `suwappu_execution::Intent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum BatchTransaction {
    /// Move `amount` from `from` to `to`. Replay-defended by
    /// `from`'s current account nonce — tx is rejected unless
    /// `nonce == from_account.nonce`.
    Transfer {
        /// Sender address.
        from: Address,
        /// Recipient address.
        to: Address,
        /// Amount to move.
        amount: Balance,
        /// Expected nonce for `from`. Must equal the account's
        /// stored nonce or the tx rejects.
        nonce: u64,
    },
    /// Burn `amount` from `from` on L2; record a withdrawal claim
    /// for `l1_recipient` on L1. Pairs with the L1-side
    /// `Intent::L2BurnProven`: once this batch's state root is
    /// committed on L1, the user (or anyone with the burn-leaf
    /// merkle proof) can submit `L2BurnProven` to release the
    /// matching amount from the L1 bridge escrow.
    ///
    /// IQ-008 defines the leaf encoding the L1 substrate verifies
    /// against; this STM is the producer side. The collected
    /// burns for a batch are surfaced as
    /// [`BatchOutput::withdrawals`] in canonical (insertion) order
    /// and committed via [`BatchOutput::withdrawals_root`].
    ///
    /// Replay-defended by `from`'s account nonce just like
    /// `Transfer`; balance underflow rejects.
    Burn {
        /// L2 sender address (debited).
        from: Address,
        /// L1 address that will be credited when this burn is
        /// later claimed via `Intent::L2BurnProven`.
        l1_recipient: Address,
        /// Amount being burned on L2 / unlocked on L1.
        amount: Balance,
        /// Asset selector. `None` = native SUWAPPU, `Some(asset_id)`
        /// = a registered bridge asset (matches the L1-side
        /// `L2BurnProven::asset_id` semantics).
        #[serde(default)]
        asset_id: Option<[u8; 32]>,
        /// Expected nonce for `from`. Must equal the account's
        /// stored nonce or the tx rejects.
        nonce: u64,
    },
}

/// One withdrawal claim produced by a successful `Burn` tx. The
/// per-batch withdrawals tree is built from these entries in
/// canonical (insertion) order; off-chain bridge UIs reconstruct
/// the same tree from this list + `BatchInput.l2_chain_id_hash` +
/// `BatchInput.batch_id` to produce L1-side `L2BurnProven` proofs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BurnEntry {
    /// L1 address to credit on the eventual withdrawal claim.
    pub l1_recipient: Address,
    /// Amount being unlocked on L1.
    pub amount: Balance,
    /// Asset selector. `None` = native SUWAPPU.
    pub asset_id: Option<[u8; 32]>,
}

impl BatchTransaction {
    /// Canonical wire encoding. Determines what goes into the
    /// per-tx slot of the DA blob. Stable across the
    /// host / sequencer / STM-guest surfaces.
    ///
    /// Encoding for `Transfer` (variant tag = `0x01`, 65 bytes):
    /// ```text
    /// 0x01 || from (20) || to (20) || amount (u128::BE, 16) || nonce (u64::BE, 8)
    /// ```
    ///
    /// Encoding for `Burn` (variant tag = `0x02`, 66 or 98 bytes):
    /// ```text
    /// 0x02 || from (20) || l1_recipient (20) || amount (u128::BE, 16) ||
    ///   nonce (u64::BE, 8) || asset_present (u8, 1) ||
    ///   asset_id (32 bytes, only when asset_present == 1)
    /// ```
    pub fn encode(&self) -> Vec<u8> {
        match self {
            BatchTransaction::Transfer {
                from,
                to,
                amount,
                nonce,
            } => {
                let mut buf = Vec::with_capacity(65);
                buf.push(0x01);
                buf.extend_from_slice(from);
                buf.extend_from_slice(to);
                buf.extend_from_slice(&amount.to_be_bytes());
                buf.extend_from_slice(&nonce.to_be_bytes());
                buf
            }
            BatchTransaction::Burn {
                from,
                l1_recipient,
                amount,
                asset_id,
                nonce,
            } => {
                let mut buf = Vec::with_capacity(if asset_id.is_some() { 98 } else { 66 });
                buf.push(0x02);
                buf.extend_from_slice(from);
                buf.extend_from_slice(l1_recipient);
                buf.extend_from_slice(&amount.to_be_bytes());
                buf.extend_from_slice(&nonce.to_be_bytes());
                buf.push(u8::from(asset_id.is_some()));
                if let Some(id) = asset_id {
                    buf.extend_from_slice(id);
                }
                buf
            }
        }
    }

    /// Decode a single tx from its canonical wire form. Returns
    /// `Err` on unknown variant tags or length mismatches.
    pub fn decode(bytes: &[u8]) -> Result<Self, StmError> {
        if bytes.is_empty() {
            return Err(StmError::MalformedTx("empty tx bytes"));
        }
        match bytes[0] {
            0x01 => {
                if bytes.len() != 65 {
                    return Err(StmError::MalformedTx(
                        "Transfer tx must be exactly 65 bytes",
                    ));
                }
                let mut from = [0u8; 20];
                from.copy_from_slice(&bytes[1..21]);
                let mut to = [0u8; 20];
                to.copy_from_slice(&bytes[21..41]);
                let amount = u128::from_be_bytes(bytes[41..57].try_into().unwrap());
                let nonce = u64::from_be_bytes(bytes[57..65].try_into().unwrap());
                Ok(BatchTransaction::Transfer {
                    from,
                    to,
                    amount,
                    nonce,
                })
            }
            0x02 => {
                // Burn: variable length depending on asset_present flag.
                // Minimum 66 B (no asset); with asset, 98 B.
                if bytes.len() < 66 {
                    return Err(StmError::MalformedTx(
                        "Burn tx is at least 66 bytes (1 tag + 20 from + 20 recipient + 16 amount + 8 nonce + 1 asset-flag)",
                    ));
                }
                let mut from = [0u8; 20];
                from.copy_from_slice(&bytes[1..21]);
                let mut l1_recipient = [0u8; 20];
                l1_recipient.copy_from_slice(&bytes[21..41]);
                let amount = u128::from_be_bytes(bytes[41..57].try_into().unwrap());
                let nonce = u64::from_be_bytes(bytes[57..65].try_into().unwrap());
                let asset_present = bytes[65];
                let asset_id = match asset_present {
                    0 => {
                        if bytes.len() != 66 {
                            return Err(StmError::MalformedTx(
                                "Burn tx with asset_present=0 must be exactly 66 bytes",
                            ));
                        }
                        None
                    }
                    1 => {
                        if bytes.len() != 98 {
                            return Err(StmError::MalformedTx(
                                "Burn tx with asset_present=1 must be exactly 98 bytes",
                            ));
                        }
                        let mut id = [0u8; 32];
                        id.copy_from_slice(&bytes[66..98]);
                        Some(id)
                    }
                    _ => {
                        return Err(StmError::MalformedTx(
                            "Burn tx asset_present byte must be 0 or 1",
                        ));
                    }
                };
                Ok(BatchTransaction::Burn {
                    from,
                    l1_recipient,
                    amount,
                    asset_id,
                    nonce,
                })
            }
            tag => Err(StmError::UnknownTxVariant(tag)),
        }
    }
}

// ---- Batch input/output ----------------------------------------------------

/// Inputs to the STM. The SP1 guest receives these via
/// `sp1_zkvm::io::read`; the native reference takes them as a
/// borrowed `&BatchInput`.
///
/// `da_blob` is the raw bytes the sequencer posted via
/// `Intent::PostL2DA`. The STM decodes it into the same
/// `BatchTransaction` stream the sequencer encoded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchInput {
    /// L2 state root BEFORE this batch applies. MUST equal
    /// `compute_state_root(&ledger)` — `execute_batch` verifies this
    /// at function entry and rejects with `PreStateRootMismatch`
    /// on disagreement. The check is load-bearing for ZK soundness:
    /// `to_public_inputs` commits this value, so without the check
    /// a malicious prover could pair an arbitrary ledger witness
    /// with a claimed `prev_l2_state_root` and produce a "valid"
    /// transition over an unanchored pre-state.
    pub prev_l2_state_root: Hash32,
    /// Per-batch monotonic id within the L2 chain.
    pub batch_id: u64,
    /// Raw DA blob (the sequencer's batch encoding). The STM
    /// decodes this to recover the tx stream + recomputes the
    /// `da_commitment` from it.
    pub da_blob: Vec<u8>,
    /// L1 state root at the anchor height. Plumbed through the
    /// public inputs so the proof binds the L2 transition to a
    /// specific L1 view (op-succinct anchor pattern).
    pub prev_l1_state_root: Hash32,
    /// Multi-L2 namespacing. `[0u8; 32]` for the v1 default chain.
    pub l2_chain_id_hash: Hash32,
    /// L1 block height at which this batch's commit lands. Goes
    /// straight into the public-input blob; the STM doesn't
    /// validate it (the L1-side `CommitL2StateRoot` apply arm
    /// validates against `current_block_height` at apply time).
    pub l1_anchor_height: u64,
    /// op-succinct "multiBlockVKey" trick — embedded VK
    /// commitment so the verifier can confirm the proof
    /// matches the chain's pinned `range_vk_commitment`. The
    /// STM passes this through unmodified; the prover bakes
    /// the right value into the guest at compile time.
    pub range_vk_commitment: Hash32,
    /// Pre-batch L2 ledger snapshot. Real production proving
    /// would carry a sparse Merkle witness here; the native
    /// reference takes the whole ledger as a `BTreeMap` for
    /// simplicity. Replaced when revm-in-guest lands.
    pub ledger: BTreeMap<Address, Account>,
}

/// Outputs from the STM. The guest commits these via
/// `sp1_zkvm::io::commit`; together with the
/// `prev_*` and `batch_id` fields from `BatchInput` they form
/// the verifier's 240-byte public-input blob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchOutput {
    /// L2 state root AFTER applying this batch.
    ///
    /// **Pre-burn-variant:** the BLAKE3 commitment over the post-
    /// batch ledger ([`compute_state_root`]).
    ///
    /// **Post-burn-variant (this PR):** unchanged on the
    /// `new_l2_state_root` field — the field still carries the
    /// ledger state root. Burns produce a SEPARATE
    /// [`Self::withdrawals_root`] published below; binding that
    /// into the consensus state root (i.e. making the burn
    /// gate at `Intent::L2BurnProven` actually accept real
    /// proofs) is the next PR — see IQ-008's cutover criterion.
    pub new_l2_state_root: Hash32,
    /// `BLAKE3(da_blob)`. Binds the public-input commitment
    /// to the exact DA payload the prover saw.
    pub da_commitment: Hash32,
    /// Confidential nullifier-set commitment (Track H).
    /// `[0u8; 32]` when no confidential txs in the batch.
    pub confidential_root: Hash32,
    /// Withdrawals collected from successful `Burn` txs in this
    /// batch, in canonical (insertion) order. Off-chain bridge
    /// UIs reconstruct the per-batch withdrawals tree from this
    /// list (plus `BatchInput.l2_chain_id_hash` + batch_id) and
    /// use the merkle proof at the matching index to populate
    /// `Intent::L2BurnProven`. Host-side only — the SP1 guest
    /// commits only the root (via [`Self::withdrawals_root`]).
    #[serde(default)]
    pub withdrawals: Vec<BurnEntry>,
    /// Merkle root of [`Self::withdrawals`] per IQ-008. Equals
    /// [`EMPTY_WITHDRAWALS_ROOT`] when this batch contained no
    /// successful burns. Once bound into `new_l2_state_root`
    /// (next PR), this is what `Intent::L2BurnProven` verifies
    /// inclusion against.
    #[serde(default)]
    pub withdrawals_root: Hash32,
    /// Post-batch ledger. Carried in the host-side BatchOutput
    /// for sequencer-side bookkeeping; the SP1 guest does NOT
    /// commit this (only its hash, via `new_l2_state_root`).
    pub ledger: BTreeMap<Address, Account>,
}

// ---- STM ------------------------------------------------------------------

/// Errors emitted during STM execution.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StmError {
    /// DA blob was shorter than the 8-byte batch_id header.
    #[error("da_blob too short for batch header")]
    DaBlobTooShort,
    /// DA blob's encoded batch_id didn't match the
    /// caller-supplied `BatchInput.batch_id`.
    #[error("da_blob batch_id mismatch: expected {expected}, got {got}")]
    DaBlobBatchIdMismatch {
        /// What the input declared.
        expected: u64,
        /// What the DA blob encoded.
        got: u64,
    },
    /// DA blob's tx-length prefix overflowed the remaining bytes.
    #[error("da_blob truncated mid-transaction at offset {0}")]
    DaBlobTruncated(usize),
    /// A transaction body was structurally invalid.
    #[error("malformed tx: {0}")]
    MalformedTx(&'static str),
    /// Unknown tx variant tag.
    #[error("unknown tx variant tag: 0x{0:02x}")]
    UnknownTxVariant(u8),
    /// Transfer rejected because the sender's nonce didn't match.
    #[error("nonce mismatch for sender 0x{from}: expected {expected}, got {got}", from = hex::encode(from))]
    NonceMismatch {
        /// Sender whose nonce mismatched.
        from: Address,
        /// What the account stores.
        expected: u64,
        /// What the tx declared.
        got: u64,
    },
    /// Transfer rejected because the sender's balance was lower
    /// than the requested amount.
    #[error("insufficient balance for sender 0x{from}: have {have}, need {need}", from = hex::encode(from))]
    InsufficientBalance {
        /// Sender whose balance was insufficient.
        from: Address,
        /// Current balance.
        have: Balance,
        /// Tx amount.
        need: Balance,
    },
    /// Transfer arithmetic overflowed (recipient balance plus
    /// amount exceeds `u128::MAX`). Should be unreachable under
    /// realistic supply caps; surfaced rather than silently
    /// saturated so the proof rejects on impossible inputs.
    #[error("balance overflow crediting recipient 0x{to}", to = hex::encode(to))]
    CreditOverflow {
        /// Recipient that would have overflowed.
        to: Address,
    },
    /// Sender's nonce was already at `u64::MAX`, so incrementing it
    /// would overflow. In debug builds the `+= 1` would panic and in
    /// release it would silently wrap to 0 — either is a
    /// consensus-critical divergence on a transition that is supposed
    /// to be a pure, deterministic `Result`. Surfaced explicitly so
    /// the proof rejects rather than diverging. Unreachable under any
    /// realistic tx volume, but the STM must be total.
    #[error("nonce overflow for sender 0x{from}: nonce already at u64::MAX", from = hex::encode(from))]
    NonceOverflow {
        /// Sender whose nonce would have overflowed.
        from: Address,
    },
    /// `BatchInput.prev_l2_state_root` did not match
    /// `compute_state_root(&input.ledger)`. Without this check the
    /// SP1 guest would happily commit a caller-provided claimed
    /// root while running over an unanchored ledger witness — a
    /// classic soundness break for ZK rollups.
    #[error(
        "prev_l2_state_root mismatch: ledger hashes to 0x{actual}, claimed 0x{claimed}",
        actual = hex::encode(actual),
        claimed = hex::encode(claimed),
    )]
    PreStateRootMismatch {
        /// What `compute_state_root(&input.ledger)` produced.
        actual: Hash32,
        /// What `BatchInput.prev_l2_state_root` declared.
        claimed: Hash32,
    },
}

/// Apply one batch to the input ledger and return the new state
/// root + outputs. **Native reference**; the SP1 guest will call
/// this exact function from inside `main`.
pub fn execute_batch(input: &BatchInput) -> Result<BatchOutput, StmError> {
    // 0. Pre-state-root soundness check. `to_public_inputs` commits
    //    the caller-provided `prev_l2_state_root` verbatim, so if the
    //    ledger witness doesn't actually hash to that root, a
    //    malicious prover could prove a transition over an unanchored
    //    pre-state. Reject before doing any work.
    let actual_prev_root = compute_state_root(&input.ledger);
    if actual_prev_root != input.prev_l2_state_root {
        return Err(StmError::PreStateRootMismatch {
            actual: actual_prev_root,
            claimed: input.prev_l2_state_root,
        });
    }

    // 1. Decode the DA blob.
    let txs = decode_da_blob(&input.da_blob, input.batch_id)?;

    // 2. Snapshot + mutate the ledger. Clone so the input arg
    // stays untouched (the STM is a pure function). Successful
    // `Burn` txs append a `BurnEntry` to `withdrawals` (in
    // canonical insertion order) for the per-batch withdrawals
    // tree built below.
    let mut ledger = input.ledger.clone();
    let mut withdrawals: Vec<BurnEntry> = Vec::new();
    for tx in &txs {
        apply_tx(&mut ledger, &mut withdrawals, tx)?;
    }

    // 3. Compute the new state root + commitments.
    let new_l2_state_root = compute_state_root(&ledger);
    let da_commitment = *blake3::hash(&input.da_blob).as_bytes();
    // Track H confidential support is plumbed but not yet wired —
    // see lib-level doc.
    let confidential_root = [0u8; 32];
    let withdrawals_root =
        compute_withdrawals_root(&withdrawals, &input.l2_chain_id_hash, input.batch_id);

    Ok(BatchOutput {
        new_l2_state_root,
        da_commitment,
        confidential_root,
        withdrawals,
        withdrawals_root,
        ledger,
    })
}

/// Serialize the (input, output) pair to the 240-byte public-
/// input blob the SP1 verifier expects. Field order + offsets
/// MUST match
/// `suwappu_l2_verifier_precompile::public_inputs::*` byte-for-byte.
pub fn to_public_inputs(input: &BatchInput, output: &BatchOutput) -> [u8; L2_PUBLIC_INPUTS_BYTES] {
    let mut buf = [0u8; L2_PUBLIC_INPUTS_BYTES];
    buf[pi_offsets::PREV_L2_STATE_ROOT_OFFSET..pi_offsets::PREV_L2_STATE_ROOT_OFFSET + 32]
        .copy_from_slice(&input.prev_l2_state_root);
    buf[pi_offsets::NEW_L2_STATE_ROOT_OFFSET..pi_offsets::NEW_L2_STATE_ROOT_OFFSET + 32]
        .copy_from_slice(&output.new_l2_state_root);
    buf[pi_offsets::BATCH_ID_OFFSET..pi_offsets::BATCH_ID_OFFSET + 8]
        .copy_from_slice(&input.batch_id.to_be_bytes());
    buf[pi_offsets::DA_COMMITMENT_OFFSET..pi_offsets::DA_COMMITMENT_OFFSET + 32]
        .copy_from_slice(&output.da_commitment);
    buf[pi_offsets::L1_ANCHOR_HEIGHT_OFFSET..pi_offsets::L1_ANCHOR_HEIGHT_OFFSET + 8]
        .copy_from_slice(&input.l1_anchor_height.to_be_bytes());
    buf[pi_offsets::RANGE_VK_COMMITMENT_OFFSET..pi_offsets::RANGE_VK_COMMITMENT_OFFSET + 32]
        .copy_from_slice(&input.range_vk_commitment);
    buf[pi_offsets::PREV_L1_STATE_ROOT_OFFSET..pi_offsets::PREV_L1_STATE_ROOT_OFFSET + 32]
        .copy_from_slice(&input.prev_l1_state_root);
    buf[pi_offsets::L2_CHAIN_ID_HASH_OFFSET..pi_offsets::L2_CHAIN_ID_HASH_OFFSET + 32]
        .copy_from_slice(&input.l2_chain_id_hash);
    buf[pi_offsets::CONFIDENTIAL_ROOT_OFFSET..pi_offsets::CONFIDENTIAL_ROOT_OFFSET + 32]
        .copy_from_slice(&output.confidential_root);
    buf
}

// ---- Internals ------------------------------------------------------------

/// Decode the sequencer's DA blob into a tx stream.
fn decode_da_blob(blob: &[u8], expected_batch_id: u64) -> Result<Vec<BatchTransaction>, StmError> {
    if blob.len() < 8 {
        return Err(StmError::DaBlobTooShort);
    }
    let got_batch_id = u64::from_be_bytes(blob[0..8].try_into().unwrap());
    if got_batch_id != expected_batch_id {
        return Err(StmError::DaBlobBatchIdMismatch {
            expected: expected_batch_id,
            got: got_batch_id,
        });
    }
    let mut cursor = 8;
    let mut out = Vec::new();
    while cursor < blob.len() {
        if cursor + 4 > blob.len() {
            return Err(StmError::DaBlobTruncated(cursor));
        }
        let tx_len = u32::from_be_bytes(blob[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 4;
        if cursor + tx_len > blob.len() {
            return Err(StmError::DaBlobTruncated(cursor));
        }
        out.push(BatchTransaction::decode(&blob[cursor..cursor + tx_len])?);
        cursor += tx_len;
    }
    Ok(out)
}

/// Apply one tx to the ledger. Mutates in place; the caller has
/// snapshotted before calling. For `Burn` txs, also pushes a
/// `BurnEntry` into `withdrawals` on success — so the caller's
/// withdrawals tree includes every successfully-applied burn in
/// canonical (insertion) order.
fn apply_tx(
    ledger: &mut BTreeMap<Address, Account>,
    withdrawals: &mut Vec<BurnEntry>,
    tx: &BatchTransaction,
) -> Result<(), StmError> {
    match tx {
        BatchTransaction::Transfer {
            from,
            to,
            amount,
            nonce,
        } => {
            // Atomic check-and-mutate: validate nonce + balance
            // BEFORE any state writes. Rust's early-return on `?`
            // implicitly preserves atomicity.
            let from_account = ledger.entry(*from).or_default();
            if from_account.nonce != *nonce {
                return Err(StmError::NonceMismatch {
                    from: *from,
                    expected: from_account.nonce,
                    got: *nonce,
                });
            }
            if from_account.balance < *amount {
                return Err(StmError::InsufficientBalance {
                    from: *from,
                    have: from_account.balance,
                    need: *amount,
                });
            }
            // Checked nonce increment: at `u64::MAX` a plain `+= 1`
            // would panic in debug and wrap to 0 in release — a
            // consensus-critical divergence on what must be a pure
            // deterministic transition. Compute the next nonce before
            // any mutation so the early return leaves state untouched.
            let next_nonce = from_account
                .nonce
                .checked_add(1)
                .ok_or(StmError::NonceOverflow { from: *from })?;
            from_account.balance -= *amount;
            from_account.nonce = next_nonce;

            let to_account = ledger.entry(*to).or_default();
            to_account.balance = to_account
                .balance
                .checked_add(*amount)
                .ok_or(StmError::CreditOverflow { to: *to })?;
            Ok(())
        }
        BatchTransaction::Burn {
            from,
            l1_recipient,
            amount,
            asset_id,
            nonce,
        } => {
            // Same atomicity discipline as Transfer: validate
            // nonce + balance before any state writes. Burn has
            // no recipient on L2 — the credit lands on L1 via a
            // later `L2BurnProven` claim against the withdrawals
            // tree built below.
            let from_account = ledger.entry(*from).or_default();
            if from_account.nonce != *nonce {
                return Err(StmError::NonceMismatch {
                    from: *from,
                    expected: from_account.nonce,
                    got: *nonce,
                });
            }
            if from_account.balance < *amount {
                return Err(StmError::InsufficientBalance {
                    from: *from,
                    have: from_account.balance,
                    need: *amount,
                });
            }
            from_account.balance -= *amount;
            from_account.nonce += 1;

            // Record the withdrawal claim. The L1 substrate's
            // `Intent::L2BurnProven` apply arm verifies a merkle
            // proof against the per-batch withdrawals tree built
            // from this list (IQ-008 leaf scheme).
            withdrawals.push(BurnEntry {
                l1_recipient: *l1_recipient,
                amount: *amount,
                asset_id: *asset_id,
            });
            Ok(())
        }
    }
}

/// Canonical root of an empty withdrawals tree. The STM emits
/// this on batches that contain no successful `Burn` txs;
/// off-chain consumers compare against it to detect "no burns
/// claimable from this batch" without re-running the builder.
///
/// Concretely: `BLAKE3("suwappu-l2-empty-withdrawals-v1")`. The
/// domain tag is length-distinguished from the leaf / inner-node
/// tags in `suwappu-l2-bridge` so a degenerate tree (e.g. an
/// adversarially-crafted single-leaf hash) can never collide
/// with the empty root.
pub fn empty_withdrawals_root() -> Hash32 {
    let mut h = blake3::Hasher::new();
    h.update(b"suwappu-l2-empty-withdrawals-v1");
    *h.finalize().as_bytes()
}

/// Build the per-batch withdrawals merkle tree and return its
/// root, per IQ-008. Empty `burns` → [`empty_withdrawals_root`].
/// Otherwise: leaves in insertion order, padded to the next
/// power of two with a sentinel padding leaf (so the tree shape
/// is deterministic for the proof builder + the substrate
/// verifier).
///
/// Leaf encoding (matches `suwappu-l2-bridge::BurnLeaf::hash`):
/// `BLAKE3("suwappu-l2-burn-leaf-v1" || l2_chain_id_hash (32) ||
/// u64_BE(batch_id) || recipient (20) || u128_BE(amount) ||
/// u8(asset_id.is_some()) || asset_id (32 if present))`.
///
/// Inner-node encoding (matches `suwappu-l2-bridge::hash_inner_node`):
/// `BLAKE3("suwappu-l2-burn-node-v1" || left (32) || right (32))`.
///
/// Padding-leaf encoding: `BLAKE3("suwappu-l2-burn-pad-v1")`. A
/// dedicated length-distinguished tag — a padding leaf MUST NOT
/// collide with any real burn leaf nor with an inner-node hash,
/// so an attacker cannot smuggle a padding sentinel into the
/// real-leaf prefix and forge a proof.
pub fn compute_withdrawals_root(
    burns: &[BurnEntry],
    l2_chain_id_hash: &Hash32,
    batch_id: u64,
) -> Hash32 {
    if burns.is_empty() {
        return empty_withdrawals_root();
    }
    let mut layer: Vec<Hash32> = burns
        .iter()
        .map(|b| burn_entry_leaf_hash(b, l2_chain_id_hash, batch_id))
        .collect();
    // Pad to the next power of two with the canonical padding
    // leaf so the tree shape is deterministic. A 1-leaf tree
    // gets paired with one padding leaf to form depth 1; a
    // 3-leaf tree gets one padding leaf to reach 4; a 5-leaf
    // tree gets three padding leaves to reach 8; etc.
    let target = layer.len().next_power_of_two();
    let pad = padding_leaf_hash();
    while layer.len() < target {
        layer.push(pad);
    }
    // Pairwise hash up the tree until one root remains.
    while layer.len() > 1 {
        let mut next = Vec::with_capacity(layer.len() / 2);
        for chunk in layer.chunks(2) {
            next.push(suwappu_l2_bridge::hash_inner_node(&chunk[0], &chunk[1]));
        }
        layer = next;
    }
    layer[0]
}

/// Construct the merkle proof binding the `index`-th burn in
/// `burns` to the same withdrawals root [`compute_withdrawals_root`]
/// produces. Returns `(merkle_path, path_directions)` ready to
/// pass to `suwappu-l2-bridge::verify_burn_inclusion` or to populate
/// `Intent::L2BurnProven`.
///
/// `path_directions` packs one direction bit per level
/// LSB-first, matching IQ-008's verification rule: bit `0` means
/// the sibling is on the RIGHT (running hash is the LEFT child);
/// bit `1` means the sibling is on the LEFT.
///
/// # Errors
///
/// Returns `Err(WithdrawalsProofError::IndexOutOfRange)` if
/// `index >= burns.len()`. Empty `burns` returns this error
/// regardless of `index` (there's no burn to prove inclusion of).
pub fn merkle_proof_for_burn(
    burns: &[BurnEntry],
    index: usize,
    l2_chain_id_hash: &Hash32,
    batch_id: u64,
) -> Result<(Vec<u8>, Vec<u8>), WithdrawalsProofError> {
    if index >= burns.len() {
        return Err(WithdrawalsProofError::IndexOutOfRange {
            index,
            len: burns.len(),
        });
    }
    // Build the leaf layer + pad to the next power of two.
    let mut layer: Vec<Hash32> = burns
        .iter()
        .map(|b| burn_entry_leaf_hash(b, l2_chain_id_hash, batch_id))
        .collect();
    let target = layer.len().next_power_of_two();
    let pad = padding_leaf_hash();
    while layer.len() < target {
        layer.push(pad);
    }
    // Walk up, capturing the sibling at the running position
    // each step. The direction bit is the LSB of the index at
    // that level — if even, the running hash is the LEFT child
    // (sibling on the RIGHT, direction bit = 0); if odd, the
    // running hash is the RIGHT child (sibling on the LEFT,
    // direction bit = 1).
    let mut path = Vec::new();
    let mut directions = Vec::new();
    let mut pos = index;
    let mut level = 0usize;
    while layer.len() > 1 {
        let sibling_index = pos ^ 1; // flip the LSB
        path.extend_from_slice(&layer[sibling_index]);
        let direction_bit: u8 = u8::from(pos & 1 == 1);
        // Lazily extend `directions` by one byte every 8 levels.
        if level % 8 == 0 {
            directions.push(0);
        }
        let dir_byte_index = level / 8;
        directions[dir_byte_index] |= direction_bit << (level % 8);
        // Advance one level: collapse the layer to parents, halve pos.
        let mut next = Vec::with_capacity(layer.len() / 2);
        for chunk in layer.chunks(2) {
            next.push(suwappu_l2_bridge::hash_inner_node(&chunk[0], &chunk[1]));
        }
        layer = next;
        pos /= 2;
        level += 1;
    }
    Ok((path, directions))
}

/// Errors produced by [`merkle_proof_for_burn`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WithdrawalsProofError {
    /// Asked for a proof at an `index` that doesn't exist in the
    /// batch's withdrawals. Hits both the empty-batch case
    /// (`len == 0`) and the past-end case.
    #[error("burn index {index} out of range (batch has {len} withdrawals)")]
    IndexOutOfRange {
        /// Caller-supplied index.
        index: usize,
        /// Number of burns in the batch.
        len: usize,
    },
}

/// Hash of one [`BurnEntry`] under IQ-008's leaf scheme. Wraps
/// `suwappu-l2-bridge::BurnLeaf::hash` so the STM + the L1 substrate
/// share a single source of truth.
fn burn_entry_leaf_hash(entry: &BurnEntry, chain_id_hash: &Hash32, batch_id: u64) -> Hash32 {
    let leaf = suwappu_l2_bridge::BurnLeaf {
        l2_chain_id_hash: chain_id_hash,
        batch_id,
        recipient: &entry.l1_recipient,
        amount: entry.amount,
        asset_id: entry.asset_id.as_ref(),
    };
    leaf.hash()
}

/// Canonical padding-leaf hash. Used to round the withdrawals
/// tree to a power of two without colliding with real leaves
/// (length-distinguished domain tag).
fn padding_leaf_hash() -> Hash32 {
    let mut h = blake3::Hasher::new();
    h.update(b"suwappu-l2-burn-pad-v1");
    *h.finalize().as_bytes()
}

/// Canonical state-root computation. BLAKE3 over a deterministic
/// encoding of every non-default account, in ascending Address
/// order. Replaced by a Merkle tree (EVM MPT or Verkle) when
/// revm-in-guest lands.
///
/// Encoding per entry: `addr (20) || balance (u128::BE, 16) || nonce (u64::BE, 8) = 44 B`.
pub fn compute_state_root(ledger: &BTreeMap<Address, Account>) -> Hash32 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"SUWAPPU_L2_STATE_ROOT_V1");
    for (addr, acct) in ledger {
        if acct.balance == 0 && acct.nonce == 0 {
            // Don't fold default-zero entries into the root —
            // matches the empty-account convention so a tx that
            // touches a never-before-seen address and then
            // reverts leaves no state-root delta.
            continue;
        }
        hasher.update(addr);
        hasher.update(&acct.balance.to_be_bytes());
        hasher.update(&acct.nonce.to_be_bytes());
    }
    *hasher.finalize().as_bytes()
}

/// Construct a canonical DA blob from a batch_id + tx stream.
/// Mirror of the sequencer's encoding; lives here so STM tests +
/// SDK consumers can produce DA blobs without depending on the
/// sequencer crate's heavier transitive deps.
pub fn encode_da_blob(batch_id: u64, txs: &[BatchTransaction]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + txs.len() * 70);
    out.extend_from_slice(&batch_id.to_be_bytes());
    for tx in txs {
        let bytes = tx.encode();
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(&bytes);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(n: u8) -> Address {
        let mut a = [0u8; 20];
        a[0] = n;
        a
    }

    #[test]
    fn tx_encode_decode_roundtrip() {
        let tx = BatchTransaction::Transfer {
            from: addr(1),
            to: addr(2),
            amount: 100,
            nonce: 0,
        };
        let bytes = tx.encode();
        assert_eq!(bytes.len(), 65);
        let decoded = BatchTransaction::decode(&bytes).unwrap();
        assert_eq!(tx, decoded);
    }

    #[test]
    fn tx_decode_rejects_unknown_tag() {
        let mut bytes = vec![0u8; 65];
        bytes[0] = 0xFF;
        assert!(matches!(
            BatchTransaction::decode(&bytes),
            Err(StmError::UnknownTxVariant(0xFF))
        ));
    }

    #[test]
    fn da_blob_roundtrip() {
        let txs = vec![
            BatchTransaction::Transfer {
                from: addr(1),
                to: addr(2),
                amount: 50,
                nonce: 0,
            },
            BatchTransaction::Transfer {
                from: addr(2),
                to: addr(3),
                amount: 25,
                nonce: 0,
            },
        ];
        let blob = encode_da_blob(7, &txs);
        let decoded = decode_da_blob(&blob, 7).unwrap();
        assert_eq!(decoded, txs);
    }

    #[test]
    fn da_blob_rejects_batch_id_mismatch() {
        let txs = vec![BatchTransaction::Transfer {
            from: addr(1),
            to: addr(2),
            amount: 1,
            nonce: 0,
        }];
        let blob = encode_da_blob(7, &txs);
        let res = decode_da_blob(&blob, 8);
        assert!(matches!(res, Err(StmError::DaBlobBatchIdMismatch { .. })));
    }

    #[test]
    fn execute_simple_transfer() {
        let mut ledger = BTreeMap::new();
        ledger.insert(
            addr(1),
            Account {
                balance: 1_000,
                nonce: 0,
            },
        );

        let txs = vec![BatchTransaction::Transfer {
            from: addr(1),
            to: addr(2),
            amount: 250,
            nonce: 0,
        }];
        let blob = encode_da_blob(1, &txs);

        let input = BatchInput {
            prev_l2_state_root: compute_state_root(&ledger),
            batch_id: 1,
            da_blob: blob,
            prev_l1_state_root: [0u8; 32],
            l2_chain_id_hash: [0u8; 32],
            l1_anchor_height: 42,
            range_vk_commitment: [0u8; 32],
            ledger,
        };

        let out = execute_batch(&input).unwrap();
        assert_eq!(out.ledger.get(&addr(1)).unwrap().balance, 750);
        assert_eq!(out.ledger.get(&addr(1)).unwrap().nonce, 1);
        assert_eq!(out.ledger.get(&addr(2)).unwrap().balance, 250);
        assert_eq!(out.ledger.get(&addr(2)).unwrap().nonce, 0);
        assert_eq!(out.da_commitment, *blake3::hash(&input.da_blob).as_bytes());
    }

    #[test]
    fn execute_full_balance_spend_retains_zero_balance_nonce_bumped_sender() {
        // A Transfer that spends the sender's ENTIRE balance leaves the
        // sender at balance==0 but nonce==1. compute_state_root only prunes
        // accounts with balance==0 AND nonce==0, so the nonce>0 sender MUST
        // be retained and fold into the state root. Pins the amount==balance
        // edge against a future prune-rule regression that drops it.
        let mut ledger = BTreeMap::new();
        ledger.insert(
            addr(1),
            Account {
                balance: 1_000,
                nonce: 0,
            },
        );

        let txs = vec![BatchTransaction::Transfer {
            from: addr(1),
            to: addr(2),
            amount: 1_000, // spend the entire balance
            nonce: 0,
        }];
        let blob = encode_da_blob(1, &txs);

        let input = BatchInput {
            prev_l2_state_root: compute_state_root(&ledger),
            batch_id: 1,
            da_blob: blob,
            prev_l1_state_root: [0u8; 32],
            l2_chain_id_hash: [0u8; 32],
            l1_anchor_height: 0,
            range_vk_commitment: [0u8; 32],
            ledger,
        };

        let out = execute_batch(&input).unwrap();

        // Sender drained to zero but nonce bumped — and therefore retained.
        let sender = out.ledger.get(&addr(1)).expect("sender must be retained");
        assert_eq!(sender.balance, 0);
        assert_eq!(sender.nonce, 1);
        assert!(out.ledger.contains_key(&addr(1)));
        // Recipient received the full amount.
        assert_eq!(out.ledger.get(&addr(2)).unwrap().balance, 1_000);

        // The retained zero-balance/nonce-bumped sender contributes to the
        // state root: removing it changes the root. (Both ledgers still fold
        // the recipient, so this diff isolates the sender's 44-byte entry.)
        let root_with_sender = compute_state_root(&out.ledger);
        let mut ledger_without_sender = out.ledger.clone();
        ledger_without_sender.remove(&addr(1));
        let root_without_sender = compute_state_root(&ledger_without_sender);
        assert_ne!(
            root_with_sender, root_without_sender,
            "zero-balance nonce-bumped sender must fold into the state root"
        );
        assert_eq!(out.new_l2_state_root, root_with_sender);
    }

    #[test]
    fn execute_rejects_nonce_mismatch() {
        let mut ledger = BTreeMap::new();
        ledger.insert(
            addr(1),
            Account {
                balance: 1_000,
                nonce: 5,
            },
        );
        let txs = vec![BatchTransaction::Transfer {
            from: addr(1),
            to: addr(2),
            amount: 250,
            nonce: 99, // wrong
        }];
        let input = BatchInput {
            prev_l2_state_root: compute_state_root(&ledger),
            batch_id: 1,
            da_blob: encode_da_blob(1, &txs),
            prev_l1_state_root: [0u8; 32],
            l2_chain_id_hash: [0u8; 32],
            l1_anchor_height: 0,
            range_vk_commitment: [0u8; 32],
            ledger,
        };
        let err = execute_batch(&input).unwrap_err();
        assert!(matches!(
            err,
            StmError::NonceMismatch {
                expected: 5,
                got: 99,
                ..
            }
        ));
    }

    #[test]
    fn execute_rejects_nonce_overflow() {
        // Sender sits at u64::MAX. A plain `nonce += 1` would panic in
        // debug / wrap to 0 in release; the STM must instead return a
        // deterministic error and leave the ledger untouched.
        let mut ledger = BTreeMap::new();
        ledger.insert(
            addr(1),
            Account {
                balance: 1_000,
                nonce: u64::MAX,
            },
        );
        let txs = vec![BatchTransaction::Transfer {
            from: addr(1),
            to: addr(2),
            amount: 250,
            nonce: u64::MAX,
        }];
        let input = BatchInput {
            prev_l2_state_root: compute_state_root(&ledger),
            batch_id: 1,
            da_blob: encode_da_blob(1, &txs),
            prev_l1_state_root: [0u8; 32],
            l2_chain_id_hash: [0u8; 32],
            l1_anchor_height: 0,
            range_vk_commitment: [0u8; 32],
            ledger,
        };
        let err = execute_batch(&input).unwrap_err();
        assert_eq!(err, StmError::NonceOverflow { from: addr(1) });
    }

    #[test]
    fn apply_tx_nonce_overflow_leaves_state_untouched() {
        // The checked nonce increment must reject before mutating the
        // sender's balance, so a failed tx is a no-op on the ledger.
        let mut ledger = BTreeMap::new();
        ledger.insert(
            addr(1),
            Account {
                balance: 1_000,
                nonce: u64::MAX,
            },
        );
        let tx = BatchTransaction::Transfer {
            from: addr(1),
            to: addr(2),
            amount: 250,
            nonce: u64::MAX,
        };
        let mut withdrawals: Vec<BurnEntry> = Vec::new();
        let err = apply_tx(&mut ledger, &mut withdrawals, &tx).unwrap_err();
        assert_eq!(err, StmError::NonceOverflow { from: addr(1) });
        // Sender balance + nonce unchanged; recipient never created.
        assert_eq!(ledger.get(&addr(1)).unwrap().balance, 1_000);
        assert_eq!(ledger.get(&addr(1)).unwrap().nonce, u64::MAX);
        assert!(!ledger.contains_key(&addr(2)));
    }

    #[test]
    fn execute_rejects_insufficient_balance() {
        let mut ledger = BTreeMap::new();
        ledger.insert(
            addr(1),
            Account {
                balance: 10,
                nonce: 0,
            },
        );
        let txs = vec![BatchTransaction::Transfer {
            from: addr(1),
            to: addr(2),
            amount: 100,
            nonce: 0,
        }];
        let input = BatchInput {
            prev_l2_state_root: compute_state_root(&ledger),
            batch_id: 1,
            da_blob: encode_da_blob(1, &txs),
            prev_l1_state_root: [0u8; 32],
            l2_chain_id_hash: [0u8; 32],
            l1_anchor_height: 0,
            range_vk_commitment: [0u8; 32],
            ledger,
        };
        let err = execute_batch(&input).unwrap_err();
        assert!(matches!(
            err,
            StmError::InsufficientBalance {
                have: 10,
                need: 100,
                ..
            }
        ));
    }

    #[test]
    fn execute_rejects_pre_state_root_mismatch() {
        let mut ledger = BTreeMap::new();
        ledger.insert(
            addr(1),
            Account {
                balance: 1_000,
                nonce: 0,
            },
        );
        let actual = compute_state_root(&ledger);
        let claimed = [0xAAu8; 32]; // attacker-picked
        let input = BatchInput {
            prev_l2_state_root: claimed,
            batch_id: 1,
            da_blob: encode_da_blob(1, &[]),
            prev_l1_state_root: [0u8; 32],
            l2_chain_id_hash: [0u8; 32],
            l1_anchor_height: 0,
            range_vk_commitment: [0u8; 32],
            ledger,
        };
        let err = execute_batch(&input).unwrap_err();
        assert!(matches!(
            err,
            StmError::PreStateRootMismatch { actual: a, claimed: c }
                if a == actual && c == claimed
        ));
    }

    #[test]
    fn public_inputs_layout_matches_verifier_offsets() {
        let input = BatchInput {
            prev_l2_state_root: [0x11u8; 32],
            batch_id: 0xAABBCCDDEEFF0011,
            da_blob: vec![0xCAu8; 9],
            prev_l1_state_root: [0x22u8; 32],
            l2_chain_id_hash: [0x33u8; 32],
            l1_anchor_height: 0x0123456789ABCDEF,
            range_vk_commitment: [0x44u8; 32],
            ledger: BTreeMap::new(),
        };
        let output = BatchOutput {
            new_l2_state_root: [0x55u8; 32],
            da_commitment: [0x66u8; 32],
            confidential_root: [0x77u8; 32],
            withdrawals: vec![],
            withdrawals_root: empty_withdrawals_root(),
            ledger: BTreeMap::new(),
        };
        let pi = to_public_inputs(&input, &output);
        assert_eq!(pi.len(), L2_PUBLIC_INPUTS_BYTES);
        // Spot-check the offsets per public_inputs::*. Every
        // field's bytes land where the verifier expects them.
        assert_eq!(
            &pi[pi_offsets::PREV_L2_STATE_ROOT_OFFSET..pi_offsets::PREV_L2_STATE_ROOT_OFFSET + 32],
            &[0x11u8; 32]
        );
        assert_eq!(
            &pi[pi_offsets::NEW_L2_STATE_ROOT_OFFSET..pi_offsets::NEW_L2_STATE_ROOT_OFFSET + 32],
            &[0x55u8; 32]
        );
        assert_eq!(
            u64::from_be_bytes(
                pi[pi_offsets::BATCH_ID_OFFSET..pi_offsets::BATCH_ID_OFFSET + 8]
                    .try_into()
                    .unwrap()
            ),
            0xAABBCCDDEEFF0011
        );
        assert_eq!(
            &pi[pi_offsets::DA_COMMITMENT_OFFSET..pi_offsets::DA_COMMITMENT_OFFSET + 32],
            &[0x66u8; 32]
        );
        assert_eq!(
            u64::from_be_bytes(
                pi[pi_offsets::L1_ANCHOR_HEIGHT_OFFSET..pi_offsets::L1_ANCHOR_HEIGHT_OFFSET + 8]
                    .try_into()
                    .unwrap()
            ),
            0x0123456789ABCDEF
        );
        assert_eq!(
            &pi[pi_offsets::RANGE_VK_COMMITMENT_OFFSET
                ..pi_offsets::RANGE_VK_COMMITMENT_OFFSET + 32],
            &[0x44u8; 32]
        );
        assert_eq!(
            &pi[pi_offsets::PREV_L1_STATE_ROOT_OFFSET..pi_offsets::PREV_L1_STATE_ROOT_OFFSET + 32],
            &[0x22u8; 32]
        );
        assert_eq!(
            &pi[pi_offsets::L2_CHAIN_ID_HASH_OFFSET..pi_offsets::L2_CHAIN_ID_HASH_OFFSET + 32],
            &[0x33u8; 32]
        );
        assert_eq!(
            &pi[pi_offsets::CONFIDENTIAL_ROOT_OFFSET..pi_offsets::CONFIDENTIAL_ROOT_OFFSET + 32],
            &[0x77u8; 32]
        );
    }

    // ----- Burn variant + withdrawals tree (IQ-008 follow-up) -----

    fn build_input(
        ledger: BTreeMap<Address, Account>,
        txs: Vec<BatchTransaction>,
        batch_id: u64,
        chain_id_hash: Hash32,
    ) -> BatchInput {
        let prev = compute_state_root(&ledger);
        BatchInput {
            prev_l2_state_root: prev,
            batch_id,
            da_blob: encode_da_blob(batch_id, &txs),
            prev_l1_state_root: [0u8; 32],
            l2_chain_id_hash: chain_id_hash,
            l1_anchor_height: 0,
            range_vk_commitment: [0u8; 32],
            ledger,
        }
    }

    #[test]
    fn burn_tx_encode_decode_roundtrip_no_asset() {
        let tx = BatchTransaction::Burn {
            from: addr(1),
            l1_recipient: addr(2),
            amount: 12345,
            asset_id: None,
            nonce: 7,
        };
        let bytes = tx.encode();
        assert_eq!(bytes.len(), 66);
        let decoded = BatchTransaction::decode(&bytes).unwrap();
        assert_eq!(tx, decoded);
    }

    #[test]
    fn burn_tx_encode_decode_roundtrip_with_asset() {
        let tx = BatchTransaction::Burn {
            from: addr(1),
            l1_recipient: addr(2),
            amount: 12345,
            asset_id: Some([0xabu8; 32]),
            nonce: 7,
        };
        let bytes = tx.encode();
        assert_eq!(bytes.len(), 98);
        let decoded = BatchTransaction::decode(&bytes).unwrap();
        assert_eq!(tx, decoded);
    }

    #[test]
    fn burn_tx_decode_rejects_invalid_asset_flag() {
        let mut bytes = vec![0u8; 66];
        bytes[0] = 0x02;
        bytes[65] = 0xff; // asset_present must be 0 or 1
        assert!(matches!(
            BatchTransaction::decode(&bytes),
            Err(StmError::MalformedTx(_))
        ));
    }

    /// `execute_batch` applies a burn: sender debited, nonce
    /// advances, the burn entry shows up in `withdrawals`, and the
    /// withdrawals root is no longer the empty-tree sentinel.
    #[test]
    fn execute_batch_records_burn_in_withdrawals() {
        let mut ledger = BTreeMap::new();
        ledger.insert(
            addr(1),
            Account {
                balance: 1_000,
                nonce: 0,
            },
        );
        let chain = [0u8; 32];
        let burn = BatchTransaction::Burn {
            from: addr(1),
            l1_recipient: addr(99),
            amount: 250,
            asset_id: None,
            nonce: 0,
        };
        let input = build_input(ledger, vec![burn], 5, chain);

        let output = execute_batch(&input).expect("burn must apply");
        assert_eq!(output.withdrawals.len(), 1);
        assert_eq!(output.withdrawals[0].l1_recipient, addr(99));
        assert_eq!(output.withdrawals[0].amount, 250);
        assert!(output.withdrawals[0].asset_id.is_none());

        // Sender debited + nonce advanced.
        let from_acct = output.ledger.get(&addr(1)).unwrap();
        assert_eq!(from_acct.balance, 750);
        assert_eq!(from_acct.nonce, 1);

        // Withdrawals root is no longer the empty-tree sentinel.
        assert_ne!(output.withdrawals_root, empty_withdrawals_root());
    }

    /// Burn with insufficient balance rejects atomically — no
    /// state mutation, no withdrawals entry.
    #[test]
    fn execute_batch_burn_underflow_rejects() {
        let mut ledger = BTreeMap::new();
        ledger.insert(
            addr(1),
            Account {
                balance: 10,
                nonce: 0,
            },
        );
        let chain = [0u8; 32];
        let burn = BatchTransaction::Burn {
            from: addr(1),
            l1_recipient: addr(99),
            amount: 100,
            asset_id: None,
            nonce: 0,
        };
        let input = build_input(ledger, vec![burn], 5, chain);

        let err = execute_batch(&input).expect_err("underflow must reject");
        assert!(matches!(err, StmError::InsufficientBalance { .. }));
    }

    /// Empty-batch withdrawals root is the canonical sentinel.
    /// Used by off-chain consumers to detect "no burns claimable
    /// from this batch" without running the builder.
    #[test]
    fn empty_batch_has_canonical_empty_withdrawals_root() {
        let ledger = BTreeMap::new();
        let chain = [0u8; 32];
        let input = build_input(ledger, vec![], 5, chain);

        let output = execute_batch(&input).expect("empty batch applies");
        assert!(output.withdrawals.is_empty());
        assert_eq!(output.withdrawals_root, empty_withdrawals_root());
    }

    /// Withdrawals root is deterministic across two STM runs with
    /// the same burn list — load-bearing for the SP1 guest's
    /// proof-vs-witness equivalence.
    #[test]
    fn withdrawals_root_is_deterministic() {
        let burns = vec![
            BurnEntry {
                l1_recipient: addr(10),
                amount: 100,
                asset_id: None,
            },
            BurnEntry {
                l1_recipient: addr(11),
                amount: 200,
                asset_id: Some([0xcdu8; 32]),
            },
        ];
        let chain = [0u8; 32];
        let r1 = compute_withdrawals_root(&burns, &chain, 7);
        let r2 = compute_withdrawals_root(&burns, &chain, 7);
        assert_eq!(r1, r2);
    }

    /// Asset selector participates in the root: same recipients +
    /// amounts but different asset_ids produce different roots.
    /// Mirrors the leaf-disambiguation test in `suwappu-l2-bridge`.
    #[test]
    fn withdrawals_root_disambiguates_asset() {
        let chain = [0u8; 32];
        let burns_native = vec![BurnEntry {
            l1_recipient: addr(10),
            amount: 100,
            asset_id: None,
        }];
        let burns_asset = vec![BurnEntry {
            l1_recipient: addr(10),
            amount: 100,
            asset_id: Some([0xcdu8; 32]),
        }];
        assert_ne!(
            compute_withdrawals_root(&burns_native, &chain, 7),
            compute_withdrawals_root(&burns_asset, &chain, 7),
        );
    }

    /// Batch_id participates in the root: same burn list under
    /// different `batch_id` values produces different roots.
    /// (The leaf hash binds batch_id per IQ-008.)
    #[test]
    fn withdrawals_root_disambiguates_batch_id() {
        let chain = [0u8; 32];
        let burns = vec![BurnEntry {
            l1_recipient: addr(10),
            amount: 100,
            asset_id: None,
        }];
        assert_ne!(
            compute_withdrawals_root(&burns, &chain, 5),
            compute_withdrawals_root(&burns, &chain, 6),
        );
    }

    /// **Round-trip with the L1 substrate verifier.** Build a
    /// real burn batch via the STM; for every burn in the batch,
    /// construct its merkle proof via `merkle_proof_for_burn` and
    /// verify it against `withdrawals_root` via
    /// `suwappu-l2-bridge::verify_burn_inclusion`. This proves the
    /// STM (producer) and the substrate (consumer) agree
    /// byte-for-byte on the IQ-008 scheme.
    #[test]
    fn merkle_proof_roundtrips_against_bridge_verifier() {
        let mut ledger = BTreeMap::new();
        ledger.insert(
            addr(1),
            Account {
                balance: 10_000,
                nonce: 0,
            },
        );
        ledger.insert(
            addr(2),
            Account {
                balance: 10_000,
                nonce: 0,
            },
        );
        ledger.insert(
            addr(3),
            Account {
                balance: 10_000,
                nonce: 0,
            },
        );
        let chain = [0xaau8; 32];
        let txs = vec![
            BatchTransaction::Burn {
                from: addr(1),
                l1_recipient: addr(91),
                amount: 100,
                asset_id: None,
                nonce: 0,
            },
            BatchTransaction::Burn {
                from: addr(2),
                l1_recipient: addr(92),
                amount: 200,
                asset_id: Some([0xcdu8; 32]),
                nonce: 0,
            },
            BatchTransaction::Burn {
                from: addr(3),
                l1_recipient: addr(93),
                amount: 300,
                asset_id: None,
                nonce: 0,
            },
        ];
        let input = build_input(ledger, txs, 42, chain);
        let output = execute_batch(&input).expect("batch applies");
        assert_eq!(output.withdrawals.len(), 3);

        for (idx, entry) in output.withdrawals.iter().enumerate() {
            let (path, directions) =
                merkle_proof_for_burn(&output.withdrawals, idx, &chain, 42).expect("proof builds");
            let leaf = suwappu_l2_bridge::BurnLeaf {
                l2_chain_id_hash: &chain,
                batch_id: 42,
                recipient: &entry.l1_recipient,
                amount: entry.amount,
                asset_id: entry.asset_id.as_ref(),
            };
            suwappu_l2_bridge::verify_burn_inclusion(
                &leaf,
                &path,
                &directions,
                &output.withdrawals_root,
            )
            .unwrap_or_else(|e| {
                panic!("burn {idx} inclusion failed under withdrawals_root: {e:?}")
            });
        }
    }

    /// A proof produced for burn N must NOT verify when claiming
    /// burn M (N != M) — the leaf encoding binds the recipient +
    /// amount, so swapping leaves a path that doesn't match the
    /// claimed `BurnLeaf`.
    #[test]
    fn merkle_proof_is_specific_to_its_burn() {
        let mut ledger = BTreeMap::new();
        ledger.insert(
            addr(1),
            Account {
                balance: 10_000,
                nonce: 0,
            },
        );
        ledger.insert(
            addr(2),
            Account {
                balance: 10_000,
                nonce: 0,
            },
        );
        let chain = [0u8; 32];
        let txs = vec![
            BatchTransaction::Burn {
                from: addr(1),
                l1_recipient: addr(91),
                amount: 100,
                asset_id: None,
                nonce: 0,
            },
            BatchTransaction::Burn {
                from: addr(2),
                l1_recipient: addr(92),
                amount: 200,
                asset_id: None,
                nonce: 0,
            },
        ];
        let input = build_input(ledger, txs, 5, chain);
        let output = execute_batch(&input).expect("batch applies");

        // Construct burn-0's proof, but use burn-1's leaf fields.
        let (path, dirs) = merkle_proof_for_burn(&output.withdrawals, 0, &chain, 5).unwrap();
        let wrong_leaf = suwappu_l2_bridge::BurnLeaf {
            l2_chain_id_hash: &chain,
            batch_id: 5,
            recipient: &output.withdrawals[1].l1_recipient,
            amount: output.withdrawals[1].amount,
            asset_id: None,
        };
        let result = suwappu_l2_bridge::verify_burn_inclusion(
            &wrong_leaf,
            &path,
            &dirs,
            &output.withdrawals_root,
        );
        assert!(matches!(
            result,
            Err(suwappu_l2_bridge::MerkleError::RootMismatch { .. })
        ));
    }

    /// `merkle_proof_for_burn` rejects an index past the end +
    /// the empty-batch case.
    #[test]
    fn merkle_proof_for_burn_index_out_of_range() {
        let chain = [0u8; 32];
        let r = merkle_proof_for_burn(&[], 0, &chain, 5);
        assert!(matches!(
            r,
            Err(WithdrawalsProofError::IndexOutOfRange { index: 0, len: 0 })
        ));

        let burns = vec![BurnEntry {
            l1_recipient: addr(10),
            amount: 1,
            asset_id: None,
        }];
        let r = merkle_proof_for_burn(&burns, 7, &chain, 5);
        assert!(matches!(
            r,
            Err(WithdrawalsProofError::IndexOutOfRange { index: 7, len: 1 })
        ));
    }
}
