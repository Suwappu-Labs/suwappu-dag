//! `gsx-l2-stm` — L2 state transition function. **Native reference
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
//!    `gsx_l2_verifier_precompile::public_inputs::*`. Determinism
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
//!   The guest at `crates/gsx-l2-stm/program/` (forthcoming)
//!   imports this same lib, calls [`execute_batch`], and emits
//!   [`to_public_inputs`] as its `sp1_zkvm::io::commit` output.
//!   Once that lands, the prover daemon (Phase 2.1) shells out
//!   to `cargo prove` against the guest ELF.
//! - **Confidential transactions.** `confidential_root` is
//!   plumbed through the type surface but defaults to all-zeros
//!   until Track H's nullifier-set updates fold into the STM
//!   (Phase 1.1 v1.1 sub-task).
//!
//! ## DA blob format (matches `gsx-l2-sequencer`)
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

use gsx_l2_verifier_precompile::{public_inputs as pi_offsets, L2_PUBLIC_INPUTS_BYTES};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 20-byte L2 address. Same shape as the L1 substrate's
/// `gsx_execution::Address` so the bridge can credit
/// L2-recipient addresses without translation.
pub type Address = [u8; 20];

/// L2 balance type. u128 to match the L1 substrate's
/// `gsx_execution::Balance`.
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
/// `gsx_execution::Intent`.
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
}

impl BatchTransaction {
    /// Canonical wire encoding. Determines what goes into the
    /// per-tx slot of the DA blob. Stable across the
    /// host / sequencer / STM-guest surfaces.
    ///
    /// Encoding for `Transfer` (variant tag = `0x01`):
    /// ```text
    /// 0x01 || from (20) || to (20) || amount (u128::BE, 16) || nonce (u64::BE, 8)  = 65 bytes
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
    pub new_l2_state_root: Hash32,
    /// `BLAKE3(da_blob)`. Binds the public-input commitment
    /// to the exact DA payload the prover saw.
    pub da_commitment: Hash32,
    /// Confidential nullifier-set commitment (Track H).
    /// `[0u8; 32]` when no confidential txs in the batch.
    pub confidential_root: Hash32,
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
    // stays untouched (the STM is a pure function).
    let mut ledger = input.ledger.clone();
    for tx in &txs {
        apply_tx(&mut ledger, tx)?;
    }

    // 3. Compute the new state root + commitments.
    let new_l2_state_root = compute_state_root(&ledger);
    let da_commitment = *blake3::hash(&input.da_blob).as_bytes();
    // Track H confidential support is plumbed but not yet wired —
    // see lib-level doc.
    let confidential_root = [0u8; 32];

    Ok(BatchOutput {
        new_l2_state_root,
        da_commitment,
        confidential_root,
        ledger,
    })
}

/// Serialize the (input, output) pair to the 240-byte public-
/// input blob the SP1 verifier expects. Field order + offsets
/// MUST match
/// `gsx_l2_verifier_precompile::public_inputs::*` byte-for-byte.
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
/// snapshotted before calling.
fn apply_tx(
    ledger: &mut BTreeMap<Address, Account>,
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
    }
}

/// Canonical state-root computation. BLAKE3 over a deterministic
/// encoding of every non-default account, in ascending Address
/// order. Replaced by a Merkle tree (EVM MPT or Verkle) when
/// revm-in-guest lands.
///
/// Encoding per entry: `addr (20) || balance (u128::BE, 16) || nonce (u64::BE, 8) = 44 B`.
pub fn compute_state_root(ledger: &BTreeMap<Address, Account>) -> Hash32 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"GSX_L2_STATE_ROOT_V1");
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
        let err = apply_tx(&mut ledger, &tx).unwrap_err();
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
}
