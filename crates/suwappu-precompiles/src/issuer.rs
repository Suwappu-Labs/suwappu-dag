//! Registered-issuer precompile — mint and burn surface (paper §8.2).
//!
//! "A single registered-issuer precompile is the mint and burn surface
//! for institutional-grade assets. Each registered issuer carries a
//! principal identifier authorized under the governance phase, a
//! per-issuer delegation cap, an attached reserve-attestation schema
//! version, and a set of policy-vocabulary rules. A two-phase burn
//! pattern with hard SLA reversal and payment-receipt attestation
//! closes the duplicate-redemption attack vector."
//!
//! ## Two-phase burn
//!
//! 1. **`initiate_burn(asset, amount)` → `BurnId`.** Reserves `amount`
//!    out of the issuer's outstanding supply; the tokens are no longer
//!    transferable but have not yet been retired. Records an SLA
//!    deadline `initiated_at + sla_window_rounds`.
//! 2. **`finalize_burn(burn_id, attestation)`.** Permanently retires
//!    the reserved tokens, requiring a `PaymentReceiptAttestation`
//!    that proves the off-chain payment was settled. Each `BurnId`
//!    can finalize at most once — the duplicate-redemption attack
//!    vector is closed by the burn-id uniqueness invariant.
//! 3. **`reverse_burn(burn_id)`.** After the SLA deadline, anyone may
//!    reverse a pending burn, returning the reserved tokens to
//!    circulating supply.
//!
//! Sprint scope (DAG-S13): the issuer registry, supply book-keeping,
//! and the two-phase burn semantics. The reserve-coverage circuit
//! breaker of paper §8.3 is DAG-S14.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::did::Did;

/// Issuer identifier — index into the registered-issuer registry.
pub type IssuerId = u32;

/// Asset identifier — 32-byte content-addressed handle for the asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AssetId(pub [u8; 32]);

/// Burn identifier — globally monotonic counter per registry.
pub type BurnId = u64;

/// Default SLA window for a pending burn, in consensus rounds.
///
/// Paper §8.2 doesn't fix a numeric SLA; this is a phase-1 default that
/// the governance layer will eventually reconfigure per asset class.
pub const DEFAULT_BURN_SLA_ROUNDS: u64 = 1_000;

/// A registered institutional issuer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issuer {
    /// Issuer identifier.
    pub id: IssuerId,
    /// Principal DID authorized to invoke mint/burn on behalf of this
    /// issuer. Resolved through the `suwappu-precompiles::did_resolver`
    /// (paper §8.1).
    pub principal_did: Did,
    /// Per-issuer delegation cap — maximum outstanding supply (mint
    /// minus burn) across all assets that this issuer controls.
    pub delegation_cap: u128,
    /// Reserve-attestation schema version (paper §8.3).
    pub reserve_schema_version: u32,
    /// Policy-vocabulary version (paper §8).
    pub policy_vocabulary_version: u32,
}

/// Reserved-but-not-retired burn record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingBurn {
    /// Unique burn identifier.
    pub burn_id: BurnId,
    /// Asset being burned.
    pub asset: AssetId,
    /// Issuer of the asset.
    pub issuer: IssuerId,
    /// Amount reserved for the burn.
    pub amount: u128,
    /// Consensus round at which the burn was initiated.
    pub initiated_at: u64,
    /// Consensus round at which the SLA expires; any time strictly
    /// after this round, the burn may be reversed.
    pub sla_deadline: u64,
}

/// Off-chain payment-receipt attestation that proves the redemption was
/// settled. Phase-1 carries an opaque 32-byte digest; production lifts
/// this to a Solidity registry signature aggregate per paper §8.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PaymentReceiptAttestation {
    /// 32-byte attestation digest.
    pub digest: [u8; 32],
}

/// Mint/burn book-keeping per (asset, issuer).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetSupply {
    /// Total minted across the asset's lifetime.
    pub total_minted: u128,
    /// Total successfully burned (finalized).
    pub total_burned: u128,
    /// Currently reserved (pending burn).
    pub reserved_for_burn: u128,
}

impl AssetSupply {
    /// Circulating supply = minted - burned - reserved.
    pub fn circulating(&self) -> u128 {
        self.total_minted
            .saturating_sub(self.total_burned)
            .saturating_sub(self.reserved_for_burn)
    }

    /// Outstanding supply = minted - burned (includes reserved-but-not-
    /// retired tokens that count toward the issuer's delegation cap).
    pub fn outstanding(&self) -> u128 {
        self.total_minted.saturating_sub(self.total_burned)
    }
}

/// Registered-issuer registry and supply book-keeping.
///
/// Phase-1 keeps the registry in-memory; production migrates it to a
/// Move resource at a deterministic address.
#[derive(Debug, Clone, Default)]
pub struct IssuerRegistry {
    issuers: BTreeMap<IssuerId, Issuer>,
    supply: BTreeMap<(AssetId, IssuerId), AssetSupply>,
    pending: BTreeMap<BurnId, PendingBurn>,
    next_burn_id: BurnId,
    sla_window_rounds: u64,
}

/// Errors emitted by the registered-issuer precompile.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum IssuerError {
    /// An issuer with the given id is already registered.
    #[error("issuer {0} already registered")]
    IssuerAlreadyRegistered(IssuerId),

    /// No issuer with the given id is registered.
    #[error("issuer {0} not registered")]
    IssuerNotRegistered(IssuerId),

    /// Mint would push the issuer's outstanding supply above its
    /// `delegation_cap`.
    #[error(
        "delegation cap exceeded for issuer {issuer}: \
         outstanding-after {outstanding}, cap {cap}"
    )]
    DelegationCapExceeded {
        /// Issuer.
        issuer: IssuerId,
        /// Outstanding supply post-mint.
        outstanding: u128,
        /// Configured cap.
        cap: u128,
    },

    /// `initiate_burn` requested an amount above the asset's
    /// circulating supply.
    #[error("burn exceeds circulating supply for asset")]
    InsufficientCirculating,

    /// `finalize_burn` or `reverse_burn` referenced an unknown burn id.
    #[error("unknown burn id {0}")]
    UnknownBurn(BurnId),

    /// `reverse_burn` was called before the SLA deadline.
    #[error("burn {burn_id} not yet reversible: round {now}, deadline {deadline}")]
    SlaNotExpired {
        /// Burn id.
        burn_id: BurnId,
        /// Current round.
        now: u64,
        /// Deadline.
        deadline: u64,
    },

    /// Arithmetic overflow on minted/burned/reserved counters.
    #[error("supply arithmetic overflow")]
    Overflow,
}

impl IssuerRegistry {
    /// Construct a registry with the default SLA window.
    pub fn new() -> Self {
        Self::with_sla(DEFAULT_BURN_SLA_ROUNDS)
    }

    /// Construct with a custom SLA window (in rounds).
    pub fn with_sla(sla_window_rounds: u64) -> Self {
        Self {
            issuers: BTreeMap::new(),
            supply: BTreeMap::new(),
            pending: BTreeMap::new(),
            next_burn_id: 0,
            sla_window_rounds,
        }
    }

    /// Register a new issuer.
    pub fn register(&mut self, issuer: Issuer) -> Result<(), IssuerError> {
        if self.issuers.contains_key(&issuer.id) {
            return Err(IssuerError::IssuerAlreadyRegistered(issuer.id));
        }
        self.issuers.insert(issuer.id, issuer);
        Ok(())
    }

    /// Borrow an issuer.
    pub fn issuer(&self, id: IssuerId) -> Option<&Issuer> {
        self.issuers.get(&id)
    }

    /// Borrow the supply record for an `(asset, issuer)` pair.
    pub fn supply(&self, asset: AssetId, issuer: IssuerId) -> AssetSupply {
        self.supply
            .get(&(asset, issuer))
            .cloned()
            .unwrap_or_default()
    }

    /// Mint `amount` of `asset` for `issuer`. Enforces the delegation
    /// cap on the issuer's total outstanding supply.
    pub fn mint(
        &mut self,
        issuer: IssuerId,
        asset: AssetId,
        amount: u128,
    ) -> Result<(), IssuerError> {
        let issuer_record = self
            .issuers
            .get(&issuer)
            .ok_or(IssuerError::IssuerNotRegistered(issuer))?;
        let cap = issuer_record.delegation_cap;

        // Compute post-mint outstanding across *every* asset this issuer
        // controls.
        let mut total_outstanding: u128 = 0;
        for ((_a, i), sup) in &self.supply {
            if *i == issuer {
                total_outstanding = total_outstanding
                    .checked_add(sup.outstanding())
                    .ok_or(IssuerError::Overflow)?;
            }
        }
        let outstanding_after = total_outstanding
            .checked_add(amount)
            .ok_or(IssuerError::Overflow)?;
        if outstanding_after > cap {
            return Err(IssuerError::DelegationCapExceeded {
                issuer,
                outstanding: outstanding_after,
                cap,
            });
        }

        let entry = self.supply.entry((asset, issuer)).or_default();
        entry.total_minted = entry
            .total_minted
            .checked_add(amount)
            .ok_or(IssuerError::Overflow)?;
        Ok(())
    }

    /// Initiate a burn: reserve `amount` of `asset` for retirement.
    /// Returns the assigned `BurnId`.
    pub fn initiate_burn(
        &mut self,
        issuer: IssuerId,
        asset: AssetId,
        amount: u128,
        now_round: u64,
    ) -> Result<BurnId, IssuerError> {
        if !self.issuers.contains_key(&issuer) {
            return Err(IssuerError::IssuerNotRegistered(issuer));
        }
        let entry = self.supply.entry((asset, issuer)).or_default();
        if entry.circulating() < amount {
            return Err(IssuerError::InsufficientCirculating);
        }
        entry.reserved_for_burn = entry
            .reserved_for_burn
            .checked_add(amount)
            .ok_or(IssuerError::Overflow)?;

        let burn_id = self.next_burn_id;
        self.next_burn_id += 1;
        self.pending.insert(
            burn_id,
            PendingBurn {
                burn_id,
                asset,
                issuer,
                amount,
                initiated_at: now_round,
                sla_deadline: now_round + self.sla_window_rounds,
            },
        );
        Ok(burn_id)
    }

    /// Finalize a pending burn: permanently retire the reserved tokens.
    pub fn finalize_burn(
        &mut self,
        burn_id: BurnId,
        _attestation: PaymentReceiptAttestation,
    ) -> Result<(), IssuerError> {
        let pending = self
            .pending
            .remove(&burn_id)
            .ok_or(IssuerError::UnknownBurn(burn_id))?;
        let entry = self
            .supply
            .get_mut(&(pending.asset, pending.issuer))
            .expect("pending burn implies a supply record exists");
        entry.reserved_for_burn = entry
            .reserved_for_burn
            .checked_sub(pending.amount)
            .ok_or(IssuerError::Overflow)?;
        entry.total_burned = entry
            .total_burned
            .checked_add(pending.amount)
            .ok_or(IssuerError::Overflow)?;
        Ok(())
    }

    /// Reverse a pending burn after its SLA deadline. Returns the
    /// reserved tokens to circulating supply.
    pub fn reverse_burn(&mut self, burn_id: BurnId, now_round: u64) -> Result<(), IssuerError> {
        let pending = self
            .pending
            .get(&burn_id)
            .ok_or(IssuerError::UnknownBurn(burn_id))?
            .clone();
        if now_round <= pending.sla_deadline {
            return Err(IssuerError::SlaNotExpired {
                burn_id,
                now: now_round,
                deadline: pending.sla_deadline,
            });
        }
        let entry = self
            .supply
            .get_mut(&(pending.asset, pending.issuer))
            .expect("pending burn implies a supply record exists");
        entry.reserved_for_burn = entry
            .reserved_for_burn
            .checked_sub(pending.amount)
            .ok_or(IssuerError::Overflow)?;
        self.pending.remove(&burn_id);
        Ok(())
    }

    /// Number of pending burns currently outstanding.
    pub fn pending_burn_count(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issuer(id: IssuerId, cap: u128) -> Issuer {
        Issuer {
            id,
            principal_did: Did([id as u8; 32]),
            delegation_cap: cap,
            reserve_schema_version: 1,
            policy_vocabulary_version: 1,
        }
    }

    #[test]
    fn register_then_lookup() {
        let mut r = IssuerRegistry::new();
        r.register(issuer(0, 1_000_000)).unwrap();
        assert!(r.issuer(0).is_some());
        assert_eq!(
            r.register(issuer(0, 999)),
            Err(IssuerError::IssuerAlreadyRegistered(0)),
        );
    }

    #[test]
    fn mint_below_cap_succeeds() {
        let mut r = IssuerRegistry::new();
        r.register(issuer(0, 100)).unwrap();
        r.mint(0, AssetId([1; 32]), 60).unwrap();
        r.mint(0, AssetId([1; 32]), 40).unwrap();
        let s = r.supply(AssetId([1; 32]), 0);
        assert_eq!(s.total_minted, 100);
        assert_eq!(s.outstanding(), 100);
    }

    #[test]
    fn mint_above_cap_fails() {
        let mut r = IssuerRegistry::new();
        r.register(issuer(0, 100)).unwrap();
        r.mint(0, AssetId([1; 32]), 60).unwrap();
        let err = r.mint(0, AssetId([1; 32]), 41);
        assert!(matches!(
            err,
            Err(IssuerError::DelegationCapExceeded { .. })
        ));
    }

    #[test]
    fn two_phase_burn_round_trip() {
        let asset = AssetId([1; 32]);
        let mut r = IssuerRegistry::with_sla(100);
        r.register(issuer(0, 1_000)).unwrap();
        r.mint(0, asset, 100).unwrap();
        let pre = r.supply(asset, 0);
        assert_eq!(pre.circulating(), 100);
        assert_eq!(pre.outstanding(), 100);

        let burn_id = r.initiate_burn(0, asset, 40, 10).unwrap();
        let mid = r.supply(asset, 0);
        assert_eq!(mid.circulating(), 60);
        // Outstanding still includes the reserved tokens until finalize.
        assert_eq!(mid.outstanding(), 100);

        let att = PaymentReceiptAttestation { digest: [0xCD; 32] };
        r.finalize_burn(burn_id, att).unwrap();
        let post = r.supply(asset, 0);
        assert_eq!(post.total_minted, 100);
        assert_eq!(post.total_burned, 40);
        assert_eq!(post.outstanding(), 60);
        assert_eq!(post.circulating(), 60);
    }

    #[test]
    fn duplicate_finalize_burn_rejected() {
        let asset = AssetId([1; 32]);
        let mut r = IssuerRegistry::with_sla(100);
        r.register(issuer(0, 1_000)).unwrap();
        r.mint(0, asset, 100).unwrap();
        let burn_id = r.initiate_burn(0, asset, 40, 10).unwrap();
        let att = PaymentReceiptAttestation { digest: [0; 32] };
        r.finalize_burn(burn_id, att).unwrap();
        let err = r.finalize_burn(burn_id, att);
        assert_eq!(err, Err(IssuerError::UnknownBurn(burn_id)));
    }

    #[test]
    fn reverse_before_sla_fails() {
        let asset = AssetId([1; 32]);
        let mut r = IssuerRegistry::with_sla(100);
        r.register(issuer(0, 1_000)).unwrap();
        r.mint(0, asset, 100).unwrap();
        let burn_id = r.initiate_burn(0, asset, 40, 10).unwrap();
        let err = r.reverse_burn(burn_id, 50);
        assert!(matches!(err, Err(IssuerError::SlaNotExpired { .. })));
    }

    #[test]
    fn reverse_after_sla_succeeds() {
        let asset = AssetId([1; 32]);
        let mut r = IssuerRegistry::with_sla(100);
        r.register(issuer(0, 1_000)).unwrap();
        r.mint(0, asset, 100).unwrap();
        let burn_id = r.initiate_burn(0, asset, 40, 10).unwrap();
        r.reverse_burn(burn_id, 111).unwrap();
        // After reversal: circulating restored, no pending burn.
        let s = r.supply(asset, 0);
        assert_eq!(s.circulating(), 100);
        assert_eq!(r.pending_burn_count(), 0);
    }

    #[test]
    fn initiate_burn_above_circulating_fails() {
        let asset = AssetId([1; 32]);
        let mut r = IssuerRegistry::new();
        r.register(issuer(0, 1_000)).unwrap();
        r.mint(0, asset, 50).unwrap();
        let err = r.initiate_burn(0, asset, 100, 0);
        assert_eq!(err, Err(IssuerError::InsufficientCirculating));
    }
}
