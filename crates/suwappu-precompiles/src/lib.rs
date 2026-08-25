//! suwappu-precompiles — Application-layer protocol primitives.
//!
//! Implements §8 of the *SUWAPPU DAG Layer 1* paper:
//!
//! - §8.1 Identity on the account — DID field + credential proofs (W3C DID 2022) ✅
//! - §8.2 Registered-issuer precompile — mint/burn surface for institutional-grade
//!   assets, with reserve-attestation schema and policy-vocabulary rules
//! - §8.3 Reserve-coverage circuit breaker — PlonK-based zero-knowledge predicate
//!   verifying issuer-class-applicable coverage rule (1:1 par for GENIUS Act
//!   payment stablecoins and MiCA EMTs; NAV strike for tokenized fund interests)
//!
//! Sprint scope:
//!
//! - DAG-S12: DID resolver + W3C DID document validation ✅
//! - DAG-S13: registered-issuer mint/burn surface ✅
//! - DAG-S14: reserve-coverage circuit breaker (predicate + state machine) ✅
//! - Compute-incentive settlement (`rewards`): stablecoin validator
//!   rewards for proven work, minted through the §8.2 issuer surface
//!   only when the §8.3 breaker attests coverage at the projected
//!   post-mint outstanding supply (`reserve::mint_with_coverage`) ✅
//!
//! Phased rollout (paper §8.1):
//!
//! - Phase 2 (Q2–Q4 2026): commitment-log primary
//! - Mainnet Beta (Q1 2027): on-chain primary with federation issuance
//! - Phase 2 onward: node/operator DIDs → institutional-user DIDs

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod did;
pub mod did_resolver;
pub mod issuer;
pub mod reserve;
pub mod rewards;

pub use did::{
    Did, DidDocument, DidError, KeyAlgorithm, Service, VerificationMethod, VerificationMethodId,
    VerificationRelationship,
};
pub use did_resolver::InMemoryDidResolver;
pub use issuer::{
    AssetId, AssetSupply, BurnId, Issuer, IssuerError, IssuerId, IssuerRegistry,
    PaymentReceiptAttestation, PendingBurn, DEFAULT_BURN_SLA_ROUNDS,
};
pub use reserve::{
    mint_with_coverage, predicate_satisfied, CoverageError, CoverageRule, DisclosureTier,
    GatedMintError, ReserveAttestation, ReserveCoverageChecker, DEFAULT_ATTESTATION_TTL_ROUNDS,
};
pub use rewards::{
    price_receipt, uptime_multiplier_bps, ComputeReceipt, EpochSettlement, RewardError,
    RewardParams, RewardRecipient, RewardSettlement, UPTIME_FULL_RATE_BPS, UPTIME_HALF_RATE_BPS,
};
