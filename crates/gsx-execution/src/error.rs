//! Execution error types.

use thiserror::Error;

use crate::substrate::{Address, Balance};

/// Errors produced by the block executor and the substrate adapter.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ExecutionError {
    /// Transfer source lacked sufficient balance.
    #[error(
        "insufficient balance: source 0x{from} has {have}, need {need}",
        from = hex::encode(from),
    )]
    InsufficientBalance {
        /// Source address.
        from: Address,
        /// Available balance.
        have: Balance,
        /// Amount the intent required.
        need: Balance,
    },

    /// Receiving address would overflow `u128`. In production the balance
    /// type is the canonical `BalanceSlot::u128` of `gsx-db`; we surface
    /// the overflow defensively even though it is unreachable under
    /// realistic GSX supply caps.
    #[error(
        "balance overflow: target 0x{to}",
        to = hex::encode(to),
    )]
    BalanceOverflow {
        /// Destination address whose balance would overflow.
        to: Address,
    },

    /// A user `Intent::Transfer` named a reserved registry address
    /// as `from` or `to`. Reserved addresses (per
    /// `crates/gsx-execution/src/reserved.rs`: L2 registry,
    /// insurance pool, treasury) are protocol-owned and only mutated
    /// by dedicated substrate arms — not by user transfers.
    #[error(
        "reserved address mutated by transfer: 0x{addr}",
        addr = hex::encode(addr),
    )]
    ReservedAddressTransferDenied {
        /// The reserved address the transfer named.
        addr: Address,
    },

    /// `DistributeSlashedFunds` named a counterparty address that
    /// is itself a reserved registry address. Counterparty
    /// reimbursement may NOT be redirected into the insurance pool
    /// or treasury (those have their own dedicated shares in the
    /// same Intent).
    #[error(
        "reserved address in counterparties list: 0x{addr}",
        addr = hex::encode(addr),
    )]
    ReservedAddressInCounterparties {
        /// The reserved address that appeared in `counterparties`.
        addr: Address,
    },

    /// `DistributeSlashedFunds` accounting overflowed when crediting
    /// a counterparty / insurance pool / treasury.
    #[error(
        "distribution overflow on credit: target 0x{to}",
        to = hex::encode(to),
    )]
    DistributionOverflow {
        /// Destination whose credit would overflow.
        to: Address,
    },

    /// L2 verifier precompile rejected the proof (Track G G2.2).
    /// Wraps `gsx_l2_verifier_precompile::VerifyError`'s display
    /// for diagnostics; the substrate side does not further
    /// classify (the per-variant error is preserved as text).
    #[error("l2 verifier rejected proof: {reason}")]
    L2VerifierRejected {
        /// Human-readable reason from the verifier crate.
        reason: String,
    },

    /// A bytes-state record stored at `addr` is malformed and
    /// cannot be decoded. Indicates either a state-corruption
    /// regression or an encoder/decoder version drift; either
    /// way, the only recoverable behavior is to refuse the
    /// Intent that would have written to this record (the
    /// substrate would otherwise propagate the corruption).
    #[error("corrupt state record at 0x{enc_addr}: {reason}", enc_addr = hex::encode(addr))]
    CorruptStateRecord {
        /// Address of the corrupt record.
        addr: Address,
        /// Static reason string for diagnostics.
        reason: &'static str,
    },

    /// `Intent::L2ForceInclude` re-registers an obligation that
    /// already exists in the force-include registry. Replay
    /// defense — per the SLA doc §3, the L1 dedup hash blocks
    /// re-submission of `(tx, deadline, submitter, l2_nonce)`.
    #[error("force-include obligation already registered: 0x{enc_id}", enc_id = hex::encode(obligation_id))]
    ForceIncludeAlreadyRegistered {
        /// The deterministic obligation_id that already exists.
        obligation_id: [u8; 32],
    },

    /// `Intent::SlashSequencer` referenced an obligation_id
    /// not present in the registry. Either the snitch made a
    /// mistake or the obligation expired + was evicted.
    #[error("force-include obligation not found: 0x{enc_id}", enc_id = hex::encode(obligation_id))]
    ForceIncludeNotFound {
        /// The obligation_id the SlashSequencer pointed at.
        obligation_id: [u8; 32],
    },

    /// `Intent::SlashSequencer` referenced an obligation that
    /// is no longer `Pending` (it was already honored or
    /// slashed). Replay defense against double-slashing.
    #[error(
        "force-include obligation 0x{enc_id} is not pending (current status: {status:?})",
        enc_id = hex::encode(obligation_id),
    )]
    ForceIncludeNotPending {
        /// The obligation_id pointed at.
        obligation_id: [u8; 32],
        /// The obligation's current status.
        status: crate::force_include::ObligationStatus,
    },
}
