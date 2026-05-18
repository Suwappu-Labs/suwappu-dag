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

    /// `Intent::CommitL2StateRoot::vk_hash` did not match the
    /// chain-state-pinned `aggregation_vk_hash`. Per the
    /// op-succinct multiBlockVKey pattern this is the
    /// load-bearing security gate.
    #[error(
        "L2 vk_hash mismatch: expected 0x{enc_expected}, got 0x{enc_got}",
        enc_expected = hex::encode(expected),
        enc_got = hex::encode(got),
    )]
    L2VkPinMismatch {
        /// Pinned aggregation_vk_hash from the registry.
        expected: [u8; 32],
        /// vk_hash from the Intent.
        got: [u8; 32],
    },

    /// `Intent::SetL2VerifyingKey` was called with both fields
    /// all-zeros. Defense against accidental "unset" via
    /// rotation; an explicit `Intent::UnsetL2VerifyingKey`
    /// (not currently defined) would be required to truly
    /// unpin.
    #[error("SetL2VerifyingKey rejected: both new_aggregation_vk and new_range_commitment are all-zeros")]
    SetL2VkAllZeros,

    /// `Intent::AddBridgeAsset` re-adds an asset already in
    /// the registry (the canonical `asset_id` derived from
    /// `source_chain` + `source_contract` already exists).
    #[error("bridge asset already registered: 0x{enc_id}", enc_id = hex::encode(asset_id))]
    BridgeAssetAlreadyRegistered {
        /// The asset_id that already exists.
        asset_id: [u8; 32],
    },

    /// `Intent::PauseBridgeAsset` or `Intent::RemoveBridgeAsset`
    /// referenced an asset_id not present in the registry.
    #[error("bridge asset not found: 0x{enc_id}", enc_id = hex::encode(asset_id))]
    BridgeAssetNotFound {
        /// The asset_id the Intent pointed at.
        asset_id: [u8; 32],
    },

    /// `Intent::AddBridgeAsset` carried a `source_contract`,
    /// `name`, or `symbol` field exceeding the configured
    /// maximum width.
    #[error("bridge asset {field} too long: {got} > {max}")]
    BridgeAssetFieldTooLong {
        /// Which field exceeded the limit ("source_contract",
        /// "name", or "symbol").
        field: &'static str,
        /// Observed width.
        got: usize,
        /// Configured maximum.
        max: usize,
    },

    /// `Intent::EjectSequencer` referenced an obligation
    /// that's not in `ObligationStatus::Slashed`. The
    /// ejection prerequisite is a prior slashing — Pending
    /// / Honored / Ejected (already) obligations cannot
    /// be ejected.
    #[error(
        "force-include obligation 0x{enc_id} is not slashed (current status: {status:?})",
        enc_id = hex::encode(obligation_id),
    )]
    ForceIncludeNotSlashed {
        /// The obligation_id pointed at.
        obligation_id: [u8; 32],
        /// The obligation's current (non-Slashed) status.
        status: crate::force_include::ObligationStatus,
    },

    /// `Intent::EjectSequencer` already has a record for
    /// this obligation_id. Replay defense: an ejection is
    /// a one-shot event per obligation.
    #[error(
        "sequencer ejection already recorded for obligation 0x{enc_id}",
        enc_id = hex::encode(obligation_id),
    )]
    SequencerEjectionAlreadyRecorded {
        /// The obligation_id that was already ejected.
        obligation_id: [u8; 32],
    },

    /// `Intent::L1Lock` or `Intent::L2BurnProven` referenced
    /// an asset that exists in the registry but is not in
    /// `AssetStatus::Active` state (i.e., it's Paused or
    /// Removed). Bridge ops on inactive assets are rejected.
    #[error(
        "bridge asset 0x{enc_id} is not active (status: {status:?})",
        enc_id = hex::encode(asset_id),
    )]
    BridgeAssetNotActive {
        /// The asset_id the bridge op pointed at.
        asset_id: [u8; 32],
        /// The asset's current (non-Active) status.
        status: crate::asset_registry::AssetStatus,
    },
}
