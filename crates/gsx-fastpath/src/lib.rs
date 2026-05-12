//! gsx-fastpath — FastPay-style fast-path lane.
//!
//! Implements §6.4 of the *GSX DAG Layer 1* paper:
//!
//! - Parallel to main-lane Mysticeti consensus
//! - Eligible: read-write footprint is a single owned Move object, owner is sole
//!   signer, lineage grounded in a main-lane path
//! - Quorum: ⌈(2/3)|A|⌉ + 1 Authority Ring members
//! - 100–200 ms p95 finality
//! - Equivocation: slashable at 100% Authority-Node bonded stake + expulsion
//!
//! Sprint scope:
//!
//! - DAG-S8: eligibility check, fast-path certificate type,
//!   K=4 main-lane binding window ✅
//! - DAG-S9: equivocation detection and slashing path
//!
//! Property: a fast-path-certified transaction whose main-lane confirmation
//! observes a conflicting ordering within `FAST_PATH_CONFIRMATION_K` rounds
//! yields an equivocation signal; otherwise the certificate is binding.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod binding;
pub mod cert;
pub mod equivocation;
pub mod quorum;
pub mod slashing;

pub use binding::{is_main_lane_consistent, MainLaneTx, FAST_PATH_CONFIRMATION_K};
pub use cert::{FastPathCert, FastPathTx, OwnedObjectId, OwnerAddress};
pub use equivocation::{detect_fast_path_equivocation, FastPathEquivocationProof};
pub use quorum::{certify, fast_path_quorum_size, FastPathError};
pub use slashing::{slash_fast_path_signers, FastPathSlashOutcome};
