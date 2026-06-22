//! LTP Commitment Node data-availability SLA (paper §10.2).
//!
//! "Variable-size attestation payloads live on content-addressed
//! Commitment Nodes under governed DA service-level agreements."
//!
//! The constant 1,600-byte on-chain commitment of paper §10.2 leaves
//! the variable-size payload off-chain. Commitment Nodes host that
//! payload, content-addressed by its SHA3-256 digest, under a
//! per-commitment DA SLA that bounds:
//!
//! - `retention_rounds` — how long the node must retain the payload.
//! - `max_retrieval_latency_rounds` — bound on retrieval response
//!   latency from a request.
//!
//! The SLA predicate is publicly verifiable: given a retrieval-response
//! `(payload_returned, responded_at)` and the original `Commitment`,
//! the predicate either accepts (content hashes to the CID and
//! retrieval was within window) or surfaces a precise breach reason
//! that downstream slashing can consume.

use std::collections::BTreeMap;

use suwappu_crypto::hash;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Content identifier — SHA3-256 of the payload bytes with a
/// `SUWAPPU-LTP-CID-V1` domain tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Cid(pub [u8; 32]);

impl Cid {
    /// Compute the CID of `payload`.
    pub fn of(payload: &[u8]) -> Self {
        Cid(hash::sha3_256_domain(b"GSX-LTP-CID-V1", payload))
    }
}

/// Data-availability SLA terms for a single commitment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DaSla {
    /// Number of rounds the Commitment Node must retain the payload
    /// starting from `Commitment::stored_at`. After
    /// `stored_at + retention_rounds`, the commitment is eligible for
    /// pruning.
    pub retention_rounds: u64,
    /// Maximum latency, in rounds, between a retrieval request and the
    /// node's response. Late responses trip the SLA.
    pub max_retrieval_latency_rounds: u32,
}

impl DaSla {
    /// Default phase-1 SLA: 100k-round retention, 16-round retrieval
    /// latency. Governance will retune per asset class.
    pub const DEFAULT: DaSla = DaSla {
        retention_rounds: 100_000,
        max_retrieval_latency_rounds: 16,
    };
}

/// A commitment hosted on the Commitment Node network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commitment {
    /// Content identifier.
    pub cid: Cid,
    /// Payload size in bytes (clamped at u64::MAX).
    pub size_bytes: u64,
    /// Round at which the commitment was stored.
    pub stored_at: u64,
    /// SLA governing this commitment.
    pub sla: DaSla,
}

impl Commitment {
    /// Round at which the SLA retention window expires.
    pub fn retention_until(&self) -> u64 {
        self.stored_at.saturating_add(self.sla.retention_rounds)
    }

    /// `true` iff the retention window has expired at `now`.
    pub fn retention_expired(&self, now: u64) -> bool {
        now > self.retention_until()
    }
}

/// In-memory Commitment Node network. Phase-1 keeps payloads and
/// commitments side-by-side; production splits storage (typically IPFS
/// or a Filecoin-class network) from the on-chain commitment metadata.
#[derive(Debug, Default, Clone)]
pub struct CommitmentNetwork {
    commitments: BTreeMap<Cid, Commitment>,
    payloads: BTreeMap<Cid, Vec<u8>>,
}

/// Errors emitted by the DA SLA predicate and storage layer.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum DaError {
    /// No commitment exists for the given CID.
    #[error("commitment not found for cid")]
    CommitmentNotFound,

    /// The returned payload's CID does not match the requested CID.
    /// Slashable: the node returned forged content.
    #[error("payload mismatch: cid disagreement")]
    PayloadMismatch,

    /// The commitment's retention window has expired and the node has
    /// pruned the payload — not a slashable breach but a signal that
    /// the requestor should re-anchor a fresh commitment.
    #[error("retention expired at round {expired_at}")]
    RetentionExpired {
        /// Round at which retention expired.
        expired_at: u64,
    },

    /// The node's response arrived later than
    /// `requested_at + max_retrieval_latency_rounds`. Slashable.
    #[error(
        "retrieval latency exceeded: responded {responded_at}, \
         requested {requested_at}, max latency {max_latency}"
    )]
    LatencyExceeded {
        /// Round of the retrieval request.
        requested_at: u64,
        /// Round of the node's response.
        responded_at: u64,
        /// SLA latency window.
        max_latency: u32,
    },
}

/// Verify a `(payload_returned, responded_at)` retrieval response
/// against `commitment` and the original request round.
///
/// SLA pass iff:
/// - `responded_at - requested_at ≤ commitment.sla.max_retrieval_latency_rounds`,
/// - `responded_at` is within the retention window, and
/// - `Cid::of(payload_returned) == commitment.cid`.
pub fn verify_sla(
    commitment: &Commitment,
    requested_at: u64,
    responded_at: u64,
    payload_returned: &[u8],
) -> Result<(), DaError> {
    if responded_at > commitment.retention_until() {
        return Err(DaError::RetentionExpired {
            expired_at: commitment.retention_until(),
        });
    }
    let latency = responded_at.saturating_sub(requested_at);
    if latency > commitment.sla.max_retrieval_latency_rounds as u64 {
        return Err(DaError::LatencyExceeded {
            requested_at,
            responded_at,
            max_latency: commitment.sla.max_retrieval_latency_rounds,
        });
    }
    if Cid::of(payload_returned) != commitment.cid {
        return Err(DaError::PayloadMismatch);
    }
    Ok(())
}

impl CommitmentNetwork {
    /// Construct an empty network.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of stored commitments.
    pub fn len(&self) -> usize {
        self.commitments.len()
    }

    /// `true` iff no commitments are stored.
    pub fn is_empty(&self) -> bool {
        self.commitments.is_empty()
    }

    /// Store `payload` under `sla` at round `now`. Returns the
    /// content-addressed [`Cid`]. Re-storing the same CID is idempotent
    /// and updates the SLA terms in-place.
    pub fn store(&mut self, payload: Vec<u8>, sla: DaSla, now: u64) -> Cid {
        let cid = Cid::of(&payload);
        let size_bytes = payload.len() as u64;
        self.payloads.insert(cid, payload);
        self.commitments.insert(
            cid,
            Commitment {
                cid,
                size_bytes,
                stored_at: now,
                sla,
            },
        );
        cid
    }

    /// Retrieve the payload for `cid` at round `requested_at`.
    /// Honest-path: returns `Ok((commitment, payload))`. The caller
    /// then invokes `verify_sla(...)` to confirm the SLA was met.
    pub fn retrieve(&self, cid: Cid) -> Result<(Commitment, Vec<u8>), DaError> {
        let commitment = self
            .commitments
            .get(&cid)
            .cloned()
            .ok_or(DaError::CommitmentNotFound)?;
        let payload = self
            .payloads
            .get(&cid)
            .cloned()
            .ok_or(DaError::CommitmentNotFound)?;
        Ok((commitment, payload))
    }

    /// Prune commitments whose retention window has expired at `now`.
    /// Returns the count pruned.
    pub fn prune_expired(&mut self, now: u64) -> usize {
        let expired: Vec<Cid> = self
            .commitments
            .iter()
            .filter(|(_, c)| c.retention_expired(now))
            .map(|(cid, _)| *cid)
            .collect();
        for cid in &expired {
            self.commitments.remove(cid);
            self.payloads.remove(cid);
        }
        expired.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cid_is_content_addressed() {
        let p1 = b"alpha";
        let p2 = b"beta";
        assert_eq!(Cid::of(p1), Cid::of(p1));
        assert_ne!(Cid::of(p1), Cid::of(p2));
    }

    #[test]
    fn store_then_retrieve() {
        let mut net = CommitmentNetwork::new();
        let payload = b"institutional payload, ~4KB in practice".to_vec();
        let cid = net.store(payload.clone(), DaSla::DEFAULT, 10);
        let (commitment, returned) = net.retrieve(cid).unwrap();
        assert_eq!(commitment.cid, cid);
        assert_eq!(returned, payload);
        assert_eq!(commitment.size_bytes, payload.len() as u64);
    }

    #[test]
    fn verify_sla_happy_path() {
        let mut net = CommitmentNetwork::new();
        let payload = b"happy".to_vec();
        let cid = net.store(payload.clone(), DaSla::DEFAULT, 100);
        let (commitment, returned) = net.retrieve(cid).unwrap();
        // Requested at 1000, responded at 1010 — within 16-round window.
        verify_sla(&commitment, 1000, 1010, &returned).unwrap();
    }

    #[test]
    fn verify_sla_late_response_breach() {
        let mut net = CommitmentNetwork::new();
        let payload = b"late".to_vec();
        let cid = net.store(
            payload.clone(),
            DaSla {
                retention_rounds: 10_000,
                max_retrieval_latency_rounds: 5,
            },
            10,
        );
        let (commitment, returned) = net.retrieve(cid).unwrap();
        let err = verify_sla(&commitment, 100, 200, &returned);
        assert!(matches!(err, Err(DaError::LatencyExceeded { .. })));
    }

    #[test]
    fn verify_sla_payload_mismatch_breach() {
        let mut net = CommitmentNetwork::new();
        let payload = b"original".to_vec();
        let cid = net.store(payload, DaSla::DEFAULT, 10);
        let (commitment, _returned) = net.retrieve(cid).unwrap();
        // The node returns a forged payload that hashes to a different CID.
        let err = verify_sla(&commitment, 100, 100, b"forged");
        assert_eq!(err, Err(DaError::PayloadMismatch));
    }

    #[test]
    fn retention_expiry_pruned() {
        let mut net = CommitmentNetwork::new();
        net.store(
            b"stale".to_vec(),
            DaSla {
                retention_rounds: 100,
                max_retrieval_latency_rounds: 16,
            },
            10,
        );
        assert_eq!(net.len(), 1);
        let pruned = net.prune_expired(150);
        assert_eq!(pruned, 1);
        assert!(net.is_empty());
    }

    #[test]
    fn retrieve_after_expiry_via_verify() {
        let mut net = CommitmentNetwork::new();
        let cid = net.store(
            b"x".to_vec(),
            DaSla {
                retention_rounds: 100,
                max_retrieval_latency_rounds: 16,
            },
            10,
        );
        let (commitment, returned) = net.retrieve(cid).unwrap();
        let err = verify_sla(&commitment, 200, 200, &returned);
        assert!(matches!(err, Err(DaError::RetentionExpired { .. })));
    }
}
