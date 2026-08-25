//! Validator compute-incentive settlement — stablecoin rewards for
//! proven work, gated by the reserve-coverage circuit breaker.
//!
//! This is the chain-side half of the deferred-token architecture
//! (`suwappu-lattice-protocol/docs/economics/DEFERRED_TOKEN_ARCHITECTURE.md`):
//! validators are not paid in a speculative native token, they are paid
//! in a registered-issuer stablecoin — and a payout can only be minted
//! while the issuer's reserve attestation proves the *post-mint*
//! outstanding supply is still fully covered. No coverage, no payout.
//! The breaker failing closed is the whole point: a compute reward
//! exists only if the stablecoin backing it does.
//!
//! ## Flow per epoch
//!
//! 1. Off-consensus infrastructure (the validator-program probes, the
//!    DA layer, the LTP corridor) produces one [`ComputeReceipt`] per
//!    provider: certificates signed, uptime samples, corridor
//!    attestations contributed, DA bytes served. Only *observed* work
//!    appears in a receipt — a provider with zero uptime samples has
//!    proven nothing and earns nothing.
//! 2. [`RewardSettlement::settle_epoch`] prices the receipts under
//!    [`RewardParams`], clamps the total to the epoch budget pro-rata,
//!    checks the reserve-coverage predicate at the projected post-mint
//!    outstanding via [`crate::reserve::mint_with_coverage`], and mints
//!    exactly the settled total.
//! 3. The returned [`EpochSettlement`] recipient list is shaped for the
//!    execution layer's `Intent::DistributeRewards` (recipients are
//!    20-byte substrate addresses), which performs the actual balance
//!    credits with its own per-epoch replay guard.
//!
//! Uptime gating mirrors the public testnet points contract
//! (`suwappu-validator-program/src/score.rs`, `docs/testnet/POINTS.md`):
//! ≥ 99% uptime earns the full rate, ≥ 95% earns half, below that the
//! epoch earns nothing at all.
//!
//! ## Replay defense
//!
//! Epochs settle strictly monotonically (same rule as the substrate's
//! `Intent::MintInflation`): a settled epoch can never settle again,
//! and a *failed* settlement (breaker tripped, cap exceeded) leaves the
//! epoch unsettled and retryable once a fresh reserve attestation
//! lands.
//!
//! That guard is `last_settled_epoch` on [`RewardSettlement`], which is
//! ordinary struct state: it is only as durable as whatever hosts the
//! engine. Under the execution layer the value lives in chain state and
//! the guarantee is real; a host that reconstructs the engine
//! per-process — a settlement daemon, a test harness — re-opens the
//! replay window on restart and must persist `last_settled_epoch`
//! alongside the balances it minted. The same durability discipline
//! applies to the off-chain side of this loop; see
//! `suwappu-lattice-protocol/docs/economics/BILLING_LEDGER_GAP_ANALYSIS.md`.
//!
//! ## What a receipt proves
//!
//! A [`ComputeReceipt`] carries work *observed by the probes*, not work
//! self-reported by the provider being paid — that distinction is what
//! makes proof-gating meaningful, and it is load-bearing. Pricing
//! receipts a provider fills in for itself would make this an honour
//! system with a mint attached, however carefully the arithmetic is
//! clamped.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::issuer::{AssetId, IssuerError, IssuerId, IssuerRegistry};
use crate::reserve::{mint_with_coverage, CoverageError, GatedMintError, ReserveCoverageChecker};

/// 20-byte recipient address, layout-compatible with the execution
/// layer's `Address` (BLAKE3-derived account addresses).
pub type RewardRecipient = [u8; 20];

/// One GiB, for the DA serving rate denominator.
pub const GIB: u64 = 1 << 30;

/// Uptime (in basis points) at and above which the full rate is earned.
pub const UPTIME_FULL_RATE_BPS: u32 = 9_900;

/// Uptime (in basis points) at and above which half the rate is earned.
/// Below this threshold the epoch earns nothing.
pub const UPTIME_HALF_RATE_BPS: u32 = 9_500;

/// One provider's proven work for one epoch.
///
/// Every field is an *observation* made by infrastructure the provider
/// does not control: certificate counts come from committed DAG rounds,
/// uptime samples from external probes, attestation counts from
/// corridor quorum membership, DA bytes from SLA-checked retrievals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputeReceipt {
    /// Reward recipient (execution-layer account address).
    pub recipient: RewardRecipient,
    /// Epoch this receipt covers.
    pub epoch: u64,
    /// Certificates this provider signed that reached commit.
    pub certificates_signed: u64,
    /// Uptime probe samples answered successfully.
    pub uptime_ok_samples: u64,
    /// Total uptime probe samples issued.
    pub uptime_total_samples: u64,
    /// LTP corridor attestations this provider contributed a witness
    /// signature to (§10 super-node duty).
    pub attestations_signed: u64,
    /// DA bytes served within the SLA latency window (§10 commitment-
    /// node duty).
    pub da_bytes_served: u64,
}

/// Pricing and budget parameters, denominated in the reward asset's
/// base unit (e.g. micro-units of a 6-decimal stablecoin).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewardParams {
    /// Base units per committed certificate signed.
    pub per_certificate: u128,
    /// Base units per corridor attestation witnessed.
    pub per_attestation: u128,
    /// Base units per GiB of DA bytes served within SLA.
    pub per_gib_served: u128,
    /// Hard ceiling on the total minted per epoch. When proven work
    /// prices out above the budget, payouts scale down pro-rata.
    pub epoch_budget: u128,
}

/// Per-recipient payout plus the settlement totals for one epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpochSettlement {
    /// Epoch settled.
    pub epoch: u64,
    /// Recipient payouts, in receipt order, zero-payout entries elided.
    /// Shaped for `Intent::DistributeRewards`.
    pub payouts: Vec<(RewardRecipient, u128)>,
    /// Total minted — always equal to the sum of `payouts`, and never
    /// above `RewardParams::epoch_budget`.
    pub total_minted: u128,
}

/// Errors emitted by reward settlement.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum RewardError {
    /// The epoch was already settled, or is not strictly greater than
    /// the last settled epoch.
    #[error("epoch {epoch} not settleable: last settled {last:?}")]
    EpochNotMonotonic {
        /// Requested epoch.
        epoch: u64,
        /// Last settled epoch, if any.
        last: Option<u64>,
    },

    /// A receipt's `epoch` field disagrees with the epoch being settled.
    #[error("receipt epoch {receipt_epoch} does not match settling epoch {epoch}")]
    ReceiptEpochMismatch {
        /// Epoch on the offending receipt.
        receipt_epoch: u64,
        /// Epoch being settled.
        epoch: u64,
    },

    /// Two receipts in the same settlement name the same recipient.
    #[error("duplicate recipient in settlement batch")]
    DuplicateRecipient,

    /// A receipt reported more successful samples than total samples.
    #[error("receipt reports uptime_ok_samples > uptime_total_samples")]
    MalformedUptime,

    /// The reserve-coverage breaker refused the mint (stale or missing
    /// attestation, or projected outstanding not covered). The epoch
    /// stays unsettled and retryable.
    #[error("reserve coverage refused reward mint: {0}")]
    Coverage(#[from] CoverageError),

    /// The issuer registry refused the mint (unknown issuer, delegation
    /// cap, overflow). The epoch stays unsettled and retryable.
    #[error("issuer registry refused reward mint: {0}")]
    Issuer(#[from] IssuerError),

    /// Arithmetic overflow while pricing receipts.
    #[error("reward arithmetic overflow")]
    Overflow,
}

impl From<GatedMintError> for RewardError {
    fn from(err: GatedMintError) -> Self {
        match err {
            GatedMintError::Coverage(e) => RewardError::Coverage(e),
            GatedMintError::Issuer(e) => RewardError::Issuer(e),
        }
    }
}

/// Uptime gate: full rate at ≥ 99%, half rate at ≥ 95%, zero below.
///
/// Zero total samples means the provider proved no liveness at all and
/// earns nothing — unproven work is unpaid work.
pub fn uptime_multiplier_bps(ok_samples: u64, total_samples: u64) -> u32 {
    if total_samples == 0 || ok_samples > total_samples {
        return 0;
    }
    // ok / total >= threshold/10_000  <=>  ok * 10_000 >= total * threshold
    let scaled = (ok_samples as u128) * 10_000;
    if scaled >= (total_samples as u128) * (UPTIME_FULL_RATE_BPS as u128) {
        10_000
    } else if scaled >= (total_samples as u128) * (UPTIME_HALF_RATE_BPS as u128) {
        5_000
    } else {
        0
    }
}

/// Price one receipt under `params`, before any budget clamp.
///
/// Returns the base-unit payout, or `RewardError::Overflow`.
pub fn price_receipt(receipt: &ComputeReceipt, params: &RewardParams) -> Result<u128, RewardError> {
    if receipt.uptime_ok_samples > receipt.uptime_total_samples {
        return Err(RewardError::MalformedUptime);
    }
    let certs = (receipt.certificates_signed as u128)
        .checked_mul(params.per_certificate)
        .ok_or(RewardError::Overflow)?;
    let attests = (receipt.attestations_signed as u128)
        .checked_mul(params.per_attestation)
        .ok_or(RewardError::Overflow)?;
    let served = (receipt.da_bytes_served as u128)
        .checked_mul(params.per_gib_served)
        .ok_or(RewardError::Overflow)?
        / (GIB as u128);
    let raw = certs
        .checked_add(attests)
        .and_then(|s| s.checked_add(served))
        .ok_or(RewardError::Overflow)?;
    let multiplier =
        uptime_multiplier_bps(receipt.uptime_ok_samples, receipt.uptime_total_samples) as u128;
    raw.checked_mul(multiplier)
        .map(|scaled| scaled / 10_000)
        .ok_or(RewardError::Overflow)
}

/// Per-epoch reward settlement engine.
///
/// Holds the reward issuer/asset binding and the monotonic settled-
/// epoch watermark. The issuer registry and coverage checker are passed
/// per call because they are shared chain state owned elsewhere.
#[derive(Debug, Clone)]
pub struct RewardSettlement {
    params: RewardParams,
    issuer: IssuerId,
    asset: AssetId,
    last_settled_epoch: Option<u64>,
}

impl RewardSettlement {
    /// Construct a settlement engine paying rewards in `(issuer, asset)`.
    pub fn new(params: RewardParams, issuer: IssuerId, asset: AssetId) -> Self {
        Self {
            params,
            issuer,
            asset,
            last_settled_epoch: None,
        }
    }

    /// The last epoch successfully settled, if any.
    pub fn last_settled_epoch(&self) -> Option<u64> {
        self.last_settled_epoch
    }

    /// Borrow the pricing parameters.
    pub fn params(&self) -> &RewardParams {
        &self.params
    }

    /// Settle one epoch of compute receipts.
    ///
    /// On success the settled total has been minted through the
    /// registered-issuer registry, the coverage predicate held at the
    /// projected post-mint outstanding, and the watermark advanced. On
    /// any error nothing was minted and the epoch remains settleable.
    pub fn settle_epoch(
        &mut self,
        epoch: u64,
        receipts: &[ComputeReceipt],
        issuers: &mut IssuerRegistry,
        coverage: &ReserveCoverageChecker,
        now_round: u64,
    ) -> Result<EpochSettlement, RewardError> {
        if let Some(last) = self.last_settled_epoch {
            if epoch <= last {
                return Err(RewardError::EpochNotMonotonic {
                    epoch,
                    last: Some(last),
                });
            }
        }

        // Validate the batch before touching any supply state.
        let mut seen: BTreeSet<RewardRecipient> = BTreeSet::new();
        for receipt in receipts {
            if receipt.epoch != epoch {
                return Err(RewardError::ReceiptEpochMismatch {
                    receipt_epoch: receipt.epoch,
                    epoch,
                });
            }
            if !seen.insert(receipt.recipient) {
                return Err(RewardError::DuplicateRecipient);
            }
        }

        // Price everything, then clamp to the epoch budget pro-rata.
        let mut raw: Vec<(RewardRecipient, u128)> = Vec::with_capacity(receipts.len());
        let mut raw_total: u128 = 0;
        for receipt in receipts {
            let amount = price_receipt(receipt, &self.params)?;
            raw_total = raw_total.checked_add(amount).ok_or(RewardError::Overflow)?;
            raw.push((receipt.recipient, amount));
        }

        let budget = self.params.epoch_budget;
        let mut payouts: Vec<(RewardRecipient, u128)> = Vec::with_capacity(raw.len());
        let mut total: u128 = 0;
        for (recipient, amount) in raw {
            let clamped = if raw_total > budget {
                // Floor division keeps the sum <= budget and preserves
                // monotonicity in each provider's own work.
                amount.checked_mul(budget).ok_or(RewardError::Overflow)? / raw_total
            } else {
                amount
            };
            if clamped > 0 {
                payouts.push((recipient, clamped));
                total += clamped;
            }
        }

        // Coverage-gated mint: predicate must hold at the projected
        // post-mint outstanding, else nothing mints and the epoch stays
        // retryable.
        if total > 0 {
            mint_with_coverage(issuers, coverage, self.issuer, self.asset, total, now_round)?;
        }

        self.last_settled_epoch = Some(epoch);
        Ok(EpochSettlement {
            epoch,
            payouts,
            total_minted: total,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::did::Did;
    use crate::issuer::Issuer;
    use crate::reserve::{CoverageRule, DisclosureTier, ReserveAttestation};

    const ASSET: AssetId = AssetId([0xAA; 32]);
    const ISSUER: IssuerId = 7;

    fn setup(reserves: u128, cap: u128) -> (IssuerRegistry, ReserveCoverageChecker) {
        let mut issuers = IssuerRegistry::new();
        issuers
            .register(Issuer {
                id: ISSUER,
                principal_did: Did([7; 32]),
                delegation_cap: cap,
                reserve_schema_version: 1,
                policy_vocabulary_version: 1,
            })
            .unwrap();
        let mut coverage = ReserveCoverageChecker::with_ttl(1_000);
        coverage.set_rule(ISSUER, ASSET, CoverageRule::OneToOnePar);
        coverage
            .submit_attestation(ReserveAttestation {
                issuer: ISSUER,
                asset: ASSET,
                total_reserves: reserves,
                outstanding_at_attestation: 0,
                attested_at: 0,
                schema_version: 1,
                tier: DisclosureTier::F1Aggregate,
                commitment: [0; 32],
                proof: Vec::new(),
            })
            .unwrap();
        (issuers, coverage)
    }

    fn params(budget: u128) -> RewardParams {
        RewardParams {
            per_certificate: 10,
            per_attestation: 25,
            per_gib_served: 100,
            epoch_budget: budget,
        }
    }

    fn receipt(seed: u8, epoch: u64, certs: u64) -> ComputeReceipt {
        ComputeReceipt {
            recipient: [seed; 20],
            epoch,
            certificates_signed: certs,
            uptime_ok_samples: 100,
            uptime_total_samples: 100,
            attestations_signed: 0,
            da_bytes_served: 0,
        }
    }

    #[test]
    fn uptime_tiers() {
        assert_eq!(uptime_multiplier_bps(100, 100), 10_000);
        assert_eq!(uptime_multiplier_bps(99, 100), 10_000);
        assert_eq!(uptime_multiplier_bps(98, 100), 5_000);
        assert_eq!(uptime_multiplier_bps(95, 100), 5_000);
        assert_eq!(uptime_multiplier_bps(94, 100), 0);
        assert_eq!(uptime_multiplier_bps(0, 0), 0);
    }

    #[test]
    fn no_uptime_samples_earn_nothing() {
        let r = ComputeReceipt {
            uptime_ok_samples: 0,
            uptime_total_samples: 0,
            ..receipt(1, 1, 1_000)
        };
        assert_eq!(price_receipt(&r, &params(u128::MAX)).unwrap(), 0);
    }

    #[test]
    fn settle_mints_and_pays() {
        let (mut issuers, coverage) = setup(1_000_000, 1_000_000);
        let mut settlement = RewardSettlement::new(params(1_000_000), ISSUER, ASSET);
        let out = settlement
            .settle_epoch(1, &[receipt(1, 1, 100)], &mut issuers, &coverage, 10)
            .unwrap();
        assert_eq!(out.total_minted, 1_000);
        assert_eq!(out.payouts, vec![([1; 20], 1_000)]);
        assert_eq!(issuers.supply(ASSET, ISSUER).outstanding(), 1_000);
    }

    #[test]
    fn budget_clamps_pro_rata() {
        let (mut issuers, coverage) = setup(1_000_000, 1_000_000);
        let mut settlement = RewardSettlement::new(params(150), ISSUER, ASSET);
        let out = settlement
            .settle_epoch(
                1,
                &[receipt(1, 1, 20), receipt(2, 1, 10)], // raw 200 + 100
                &mut issuers,
                &coverage,
                10,
            )
            .unwrap();
        assert_eq!(out.payouts, vec![([1; 20], 100), ([2; 20], 50)]);
        assert_eq!(out.total_minted, 150);
    }

    #[test]
    fn coverage_failure_mints_nothing_and_epoch_retryable() {
        let (mut issuers, coverage) = setup(500, 1_000_000);
        let mut settlement = RewardSettlement::new(params(1_000_000), ISSUER, ASSET);
        let err = settlement.settle_epoch(1, &[receipt(1, 1, 100)], &mut issuers, &coverage, 10);
        assert!(matches!(err, Err(RewardError::Coverage(_))));
        assert_eq!(issuers.supply(ASSET, ISSUER).outstanding(), 0);
        assert_eq!(settlement.last_settled_epoch(), None);

        // Re-attest with sufficient reserves; the same epoch settles.
        let (_, coverage_ok) = setup(1_000_000, 1_000_000);
        settlement
            .settle_epoch(1, &[receipt(1, 1, 100)], &mut issuers, &coverage_ok, 10)
            .unwrap();
    }

    #[test]
    fn epoch_replay_rejected() {
        let (mut issuers, coverage) = setup(1_000_000, 1_000_000);
        let mut settlement = RewardSettlement::new(params(1_000_000), ISSUER, ASSET);
        settlement
            .settle_epoch(1, &[receipt(1, 1, 10)], &mut issuers, &coverage, 10)
            .unwrap();
        let before = issuers.supply(ASSET, ISSUER).outstanding();
        let err = settlement.settle_epoch(1, &[receipt(1, 1, 10)], &mut issuers, &coverage, 10);
        assert!(matches!(err, Err(RewardError::EpochNotMonotonic { .. })));
        assert_eq!(issuers.supply(ASSET, ISSUER).outstanding(), before);
    }

    #[test]
    fn duplicate_recipient_rejected() {
        let (mut issuers, coverage) = setup(1_000_000, 1_000_000);
        let mut settlement = RewardSettlement::new(params(1_000_000), ISSUER, ASSET);
        let err = settlement.settle_epoch(
            1,
            &[receipt(1, 1, 10), receipt(1, 1, 20)],
            &mut issuers,
            &coverage,
            10,
        );
        assert!(matches!(err, Err(RewardError::DuplicateRecipient)));
    }

    #[test]
    fn receipt_epoch_mismatch_rejected() {
        let (mut issuers, coverage) = setup(1_000_000, 1_000_000);
        let mut settlement = RewardSettlement::new(params(1_000_000), ISSUER, ASSET);
        let err = settlement.settle_epoch(2, &[receipt(1, 1, 10)], &mut issuers, &coverage, 10);
        assert!(matches!(err, Err(RewardError::ReceiptEpochMismatch { .. })));
    }

    #[test]
    fn zero_work_epoch_settles_without_minting() {
        let (mut issuers, coverage) = setup(1_000_000, 1_000_000);
        let mut settlement = RewardSettlement::new(params(1_000_000), ISSUER, ASSET);
        let out = settlement
            .settle_epoch(1, &[receipt(1, 1, 0)], &mut issuers, &coverage, 10)
            .unwrap();
        assert_eq!(out.total_minted, 0);
        assert!(out.payouts.is_empty());
        assert_eq!(settlement.last_settled_epoch(), Some(1));
    }
}
