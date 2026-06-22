//! Reserve-coverage circuit breaker (paper §8.3).
//!
//! "A PlonK-based reserve-coverage zero-knowledge predicate verifies the
//! issuer-class-applicable coverage rule (1:1 par for GENIUS Act payment
//! stablecoins and MiCA electronic-money tokens; NAV strike for
//! tokenized fund interests; jurisdictional rules elsewhere). Predicate
//! failure pauses minting until a fresh passing attestation is posted.
//! The predicate's circuit is structured so that the attested reserve
//! composition can remain confidential under a five-tier disclosure
//! schema (F1 aggregate-only through F5 CUSIP-level threshold-multi-
//! signature) while the coverage relation is publicly verifiable."
//!
//! ## Sprint scope (DAG-S14)
//!
//! - `CoverageRule` enum capturing the three rule families from §8.3.
//! - `ReserveAttestation` carrying the public inputs (total reserves,
//!   attested-at round, schema version, disclosure tier) plus an opaque
//!   `proof` field.
//! - `DisclosureTier` enum F1–F5.
//! - `predicate_satisfied(rule, reserves, outstanding)` — the pure math
//!   that the on-chain verifier checks. The Plonky3/SP1 circuit will
//!   prove the same relation against a hidden reserve composition; for
//!   phase-1 we treat the `proof` field as a placeholder and verify
//!   directly against the public `total_reserves` input.
//! - `ReserveCoverageChecker` — submit_attestation, can_mint, TTL.
//!
//! Mint integration (binding `ReserveCoverageChecker` into
//! `IssuerRegistry::mint`) is a follow-up; today this sprint delivers
//! the predicate logic and the breaker state machine.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::issuer::{AssetId, IssuerId};

/// Issuer-class-applicable coverage rule (paper §8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CoverageRule {
    /// 1:1 par. `reserves ≥ outstanding`.
    /// Applies to GENIUS Act payment stablecoins and MiCA EMTs.
    OneToOnePar,
    /// NAV strike. `reserves × 10_000 ≥ outstanding × basis_points`.
    /// Applies to tokenized fund interests; `basis_points` parameterizes
    /// the NAV ratio (10_000 = par, 9_500 = 95%, etc.).
    NavStrike {
        /// NAV ratio in basis points, where 10_000 = 100% par.
        basis_points: u32,
    },
    /// Jurisdiction-specific rule, parameterized as a basis-point ratio
    /// like `NavStrike` but flagged as jurisdictionally-defined.
    Jurisdiction {
        /// Required reserve ratio in basis points.
        ratio_bps: u32,
    },
}

/// Five-tier reserve disclosure schema (paper §8.3).
///
/// The tier governs how much of the attested composition is disclosed
/// publicly; the coverage predicate is publicly verifiable regardless
/// of tier because the ZK circuit reveals only the aggregate input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DisclosureTier {
    /// Aggregate-only — only the total reserves are disclosed.
    F1Aggregate,
    /// Broad-category disclosure (e.g. "cash + treasuries").
    F2BroadCategory,
    /// Detailed-category disclosure (e.g. "T-bills 3M / 6M / 1Y").
    F3DetailedCategory,
    /// CUSIP-level disclosure under signature.
    F4Cusip,
    /// CUSIP-level disclosure under threshold multi-signature.
    F5CusipThreshold,
}

/// Reserve-coverage attestation.
///
/// Phase-1 carries the public inputs directly and treats `proof` as an
/// opaque placeholder. The launch-readiness sprint replaces `proof`
/// with a Plonky3/SP1 proof that the hidden reserve composition
/// satisfies the rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReserveAttestation {
    /// Issuer covered.
    pub issuer: IssuerId,
    /// Asset covered.
    pub asset: AssetId,
    /// Total reserves backing this asset, in the asset's unit.
    pub total_reserves: u128,
    /// Outstanding supply at attestation time.
    pub outstanding_at_attestation: u128,
    /// Consensus round at which the attestation was sealed.
    pub attested_at: u64,
    /// Reserve-attestation schema version (paper §8.3).
    pub schema_version: u32,
    /// Disclosure tier (F1–F5).
    pub tier: DisclosureTier,
    /// 32-byte commitment to the hidden reserve composition. Public
    /// input to the ZK circuit.
    pub commitment: [u8; 32],
    /// ZK proof bytes. Phase-1 placeholder; production uses Plonky3 / SP1.
    pub proof: Vec<u8>,
}

/// Errors emitted by the coverage checker.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum CoverageError {
    /// No coverage rule registered for (issuer, asset).
    #[error("no coverage rule registered for issuer/asset")]
    NoRule,

    /// Attestation's `total_reserves` fails the coverage rule against
    /// the provided outstanding supply.
    #[error(
        "coverage predicate failed: reserves {reserves}, \
         outstanding {outstanding}, rule {rule:?}"
    )]
    PredicateFailed {
        /// Attested total reserves.
        reserves: u128,
        /// Outstanding supply at predicate-check time.
        outstanding: u128,
        /// Rule that failed.
        rule: CoverageRule,
    },

    /// No attestation has been submitted for (issuer, asset) yet, or
    /// the latest attestation has aged beyond the TTL window.
    #[error("no fresh attestation for issuer/asset")]
    NoFreshAttestation,

    /// Arithmetic overflow on the predicate's basis-point multiplication.
    #[error("coverage arithmetic overflow")]
    Overflow,
}

/// Pure-math predicate: does `reserves` cover `outstanding` under `rule`?
///
/// Returns `Ok(())` if covered, `Err(PredicateFailed)` otherwise.
/// Overflow on the basis-point multiplications is reported explicitly.
pub fn predicate_satisfied(
    rule: CoverageRule,
    reserves: u128,
    outstanding: u128,
) -> Result<(), CoverageError> {
    let covered = match rule {
        CoverageRule::OneToOnePar => reserves >= outstanding,
        CoverageRule::NavStrike { basis_points } => {
            let lhs = reserves
                .checked_mul(10_000)
                .ok_or(CoverageError::Overflow)?;
            let rhs = outstanding
                .checked_mul(basis_points as u128)
                .ok_or(CoverageError::Overflow)?;
            lhs >= rhs
        }
        CoverageRule::Jurisdiction { ratio_bps } => {
            let lhs = reserves
                .checked_mul(10_000)
                .ok_or(CoverageError::Overflow)?;
            let rhs = outstanding
                .checked_mul(ratio_bps as u128)
                .ok_or(CoverageError::Overflow)?;
            lhs >= rhs
        }
    };
    if covered {
        Ok(())
    } else {
        Err(CoverageError::PredicateFailed {
            reserves,
            outstanding,
            rule,
        })
    }
}

/// Default attestation TTL — minting pauses if the latest attestation
/// is older than this many rounds. Phase-1 places no normative weight
/// on the numeric value; governance will tune it per asset class.
pub const DEFAULT_ATTESTATION_TTL_ROUNDS: u64 = 10_000;

/// Per-(issuer, asset) coverage state.
#[derive(Debug, Clone)]
pub struct ReserveCoverageChecker {
    rules: BTreeMap<(IssuerId, AssetId), CoverageRule>,
    latest: BTreeMap<(IssuerId, AssetId), ReserveAttestation>,
    ttl_rounds: u64,
}

impl Default for ReserveCoverageChecker {
    fn default() -> Self {
        Self::new()
    }
}

impl ReserveCoverageChecker {
    /// Construct with the default TTL.
    pub fn new() -> Self {
        Self::with_ttl(DEFAULT_ATTESTATION_TTL_ROUNDS)
    }

    /// Construct with a custom TTL window.
    pub fn with_ttl(ttl_rounds: u64) -> Self {
        Self {
            rules: BTreeMap::new(),
            latest: BTreeMap::new(),
            ttl_rounds,
        }
    }

    /// Register or update the coverage rule for `(issuer, asset)`.
    pub fn set_rule(&mut self, issuer: IssuerId, asset: AssetId, rule: CoverageRule) {
        self.rules.insert((issuer, asset), rule);
    }

    /// Submit a fresh attestation. Returns `Ok(())` if the attestation
    /// satisfies the registered rule against `outstanding_at_attestation`.
    /// On failure, the attestation is NOT stored as the latest — minting
    /// stays paused until a passing attestation arrives.
    pub fn submit_attestation(
        &mut self,
        attestation: ReserveAttestation,
    ) -> Result<(), CoverageError> {
        let rule = self
            .rules
            .get(&(attestation.issuer, attestation.asset))
            .copied()
            .ok_or(CoverageError::NoRule)?;
        predicate_satisfied(
            rule,
            attestation.total_reserves,
            attestation.outstanding_at_attestation,
        )?;
        self.latest
            .insert((attestation.issuer, attestation.asset), attestation);
        Ok(())
    }

    /// Decide whether minting is permitted for `(issuer, asset)` at the
    /// supplied `outstanding` and `now_round`.
    ///
    /// Permitted iff (a) a rule is registered, (b) the latest stored
    /// attestation is within TTL, and (c) the rule passes against
    /// `outstanding` evaluated *now* (not at attestation time — so a
    /// supply increase past the attestation snapshot causes the breaker
    /// to trip).
    pub fn can_mint(
        &self,
        issuer: IssuerId,
        asset: AssetId,
        outstanding: u128,
        now_round: u64,
    ) -> Result<(), CoverageError> {
        let rule = self
            .rules
            .get(&(issuer, asset))
            .copied()
            .ok_or(CoverageError::NoRule)?;
        let attestation = self
            .latest
            .get(&(issuer, asset))
            .ok_or(CoverageError::NoFreshAttestation)?;
        // TTL freshness — attestation must not have aged past the
        // window.
        if now_round.saturating_sub(attestation.attested_at) > self.ttl_rounds {
            return Err(CoverageError::NoFreshAttestation);
        }
        predicate_satisfied(rule, attestation.total_reserves, outstanding)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(
        issuer: IssuerId,
        asset: AssetId,
        reserves: u128,
        outstanding: u128,
        round: u64,
    ) -> ReserveAttestation {
        ReserveAttestation {
            issuer,
            asset,
            total_reserves: reserves,
            outstanding_at_attestation: outstanding,
            attested_at: round,
            schema_version: 1,
            tier: DisclosureTier::F1Aggregate,
            commitment: [0xAB; 32],
            proof: Vec::new(),
        }
    }

    #[test]
    fn one_to_one_par_passes_when_reserves_meet_outstanding() {
        predicate_satisfied(CoverageRule::OneToOnePar, 100, 100).unwrap();
        predicate_satisfied(CoverageRule::OneToOnePar, 200, 100).unwrap();
    }

    #[test]
    fn one_to_one_par_fails_when_under() {
        let err = predicate_satisfied(CoverageRule::OneToOnePar, 99, 100);
        assert!(matches!(err, Err(CoverageError::PredicateFailed { .. })));
    }

    #[test]
    fn nav_strike_basis_points_math() {
        // 9_500 bps NAV: reserves × 10_000 ≥ outstanding × 9_500 →
        // reserves ≥ outstanding × 0.95.
        let rule = CoverageRule::NavStrike {
            basis_points: 9_500,
        };
        predicate_satisfied(rule, 95, 100).unwrap();
        predicate_satisfied(rule, 100, 100).unwrap();
        let err = predicate_satisfied(rule, 94, 100);
        assert!(matches!(err, Err(CoverageError::PredicateFailed { .. })));
    }

    #[test]
    fn submit_then_can_mint() {
        let asset = AssetId([1; 32]);
        let mut c = ReserveCoverageChecker::with_ttl(1_000);
        c.set_rule(0, asset, CoverageRule::OneToOnePar);
        c.submit_attestation(att(0, asset, 100, 100, 10)).unwrap();
        c.can_mint(0, asset, 100, 50).unwrap();
    }

    #[test]
    fn failing_attestation_not_stored() {
        let asset = AssetId([1; 32]);
        let mut c = ReserveCoverageChecker::with_ttl(1_000);
        c.set_rule(0, asset, CoverageRule::OneToOnePar);
        let bad = att(0, asset, 50, 100, 10); // 50 < 100
        let err = c.submit_attestation(bad);
        assert!(matches!(err, Err(CoverageError::PredicateFailed { .. })));
        assert!(matches!(
            c.can_mint(0, asset, 100, 50),
            Err(CoverageError::NoFreshAttestation)
        ));
    }

    #[test]
    fn ttl_expiry_pauses_minting() {
        let asset = AssetId([1; 32]);
        let mut c = ReserveCoverageChecker::with_ttl(100);
        c.set_rule(0, asset, CoverageRule::OneToOnePar);
        c.submit_attestation(att(0, asset, 100, 100, 10)).unwrap();
        // 10 + 100 = 110; at round 111 the TTL has expired.
        let err = c.can_mint(0, asset, 100, 111);
        assert_eq!(err, Err(CoverageError::NoFreshAttestation));
    }

    #[test]
    fn current_supply_above_attestation_trips_breaker() {
        let asset = AssetId([1; 32]);
        let mut c = ReserveCoverageChecker::with_ttl(1_000);
        c.set_rule(0, asset, CoverageRule::OneToOnePar);
        // Attestation: reserves 100, outstanding 100 — passes at submit.
        c.submit_attestation(att(0, asset, 100, 100, 10)).unwrap();
        // But current outstanding has grown to 200; reserves 100 < 200.
        let err = c.can_mint(0, asset, 200, 50);
        assert!(matches!(err, Err(CoverageError::PredicateFailed { .. })));
    }
}
