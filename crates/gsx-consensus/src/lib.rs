//! gsx-consensus — Mysticeti-C certificate DAG consensus.
//!
//! Implements §6 of the *GSX DAG Layer 1* paper:
//!
//! - §6.1 certificate-DAG topology
//! - §6.2 Mysticeti-C selection rationale (Apache 2.0, Sui-mainnet validated,
//!   uncertified-DAG deterministic finality, post-quantum-friendly hash surface)
//! - §6.3 inter-validator transport boundary (delegated to `gsx-transport`)
//! - §6.4 fast-path lane boundary (delegated to `gsx-fastpath`)
//!
//! Sprint scope:
//!
//! - DAG-S3: certificate types, DAG store, deterministic linearization ✅
//! - DAG-S4: Mysticeti-C commit rule, fork-choice
//! - DAG-S5: joint-quorum AND-gate over Authority Ring + Validator Ring
//!   certificates (paper Theorem 2)
//!
//! All exit gates are 10k-case property tests under `proptest`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod cert;
pub mod commit;
pub mod dag;
pub mod error;

pub use cert::{AuthorityId, CertHash, Certificate, Round};
pub use commit::{
    causal_history, cert_at, commit_leader, finalize, leader, quorum_threshold, CommitteeSize,
};
pub use dag::DagStore;
pub use error::ConsensusError;
