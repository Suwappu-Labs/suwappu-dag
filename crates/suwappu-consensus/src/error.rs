//! Consensus error types.

use thiserror::Error;

use crate::cert::{CertHash, Round};

/// Errors produced by the consensus DAG store and voting rule.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConsensusError {
    /// A certificate referenced a parent hash that is not in the store.
    #[error("unknown parent certificate: {0:?}")]
    UnknownParent(CertHash),

    /// A certificate's round is not strictly greater than every parent's
    /// round. Round monotonicity is a hard invariant of the certificate
    /// DAG (paper §6.1).
    #[error(
        "round monotonicity violated: certificate at round {child} \
         references parent at round {parent}"
    )]
    NonMonotonicRound {
        /// The child certificate's round.
        child: Round,
        /// The parent certificate's round.
        parent: Round,
    },

    /// The same certificate hash is already in the store. Duplicate insert
    /// is rejected to keep DagStore append-only.
    #[error("duplicate certificate insert: {0:?}")]
    DuplicateCertificate(CertHash),

    /// A round-zero certificate carried parent hashes. Genesis certificates
    /// must have an empty parent set.
    #[error("genesis (round 0) certificate carried non-empty parent set")]
    GenesisWithParents,

    /// The certificate's round is at or below the store's garbage
    /// collection round (IQ-008 D1). Such a certificate is obsolete: it
    /// can no longer appear in any committed sub-DAG, so it is neither
    /// stored nor treated as an orphan.
    #[error("certificate at round {round} is at or below the gc round {gc_round}")]
    BelowGcRound {
        /// The rejected certificate's round.
        round: Round,
        /// The store's current garbage collection round.
        gc_round: Round,
    },
}
