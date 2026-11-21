//! Checkpoint cadence + Authority joint co-signature.
//!
//! Paper §7.3 / Construction 1: cross-VM writes are batched for
//! execution at the next Authority-Ring-Signature checkpoint, when both
//! VM states are simultaneously knowable to all validators. At
//! checkpoint boundary `t ≡ 0 (mod C)` the Authority Ring co-signs the
//! new joint state `(Σ_EVM, Σ_Move)`. Phase-1 represents the joint state
//! by the single `Substrate::state_root` (the polymorphic balance map of
//! §7.2 is one canonical map, exposed identically to both VMs).
//!
//! Sprint scope (DAG-S11):
//!
//! - `Checkpoint { height, round, state_root, prev_checkpoint }` and its
//!   domain-separated BLAKE3 canonical hash.
//! - `sign_checkpoint(authority, sk, checkpoint) -> Signature` using
//!   ML-DSA-65 from `suwappu-crypto`.
//! - `ratify_checkpoint(checkpoint, sigs, registry) -> CoSigned` that
//!   verifies each signature under the registry's published
//!   `public_key_bytes` and asserts `|sigs| ≥ registry.quorum_threshold()`.
//! - `Checkpointer` that produces checkpoints at every `cadence` rounds
//!   and chains them by `prev_checkpoint`.
//!
//! Exit gate `joint_state_commitment_signed` (10k): for any honest
//! checkpoint co-signed by ≥ quorum-threshold Authority Ring members,
//! every signature verifies; tampering with any field breaks
//! verification; below-quorum signature sets are rejected.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use suwappu_authority::{AuthorityId, AuthorityRegistry};
use suwappu_consensus::Round;
use suwappu_crypto::mldsa;
use thiserror::Error;

/// Sequence number of a checkpoint. Increments monotonically from 0.
pub type CheckpointHeight = u64;

/// A checkpoint over the joint state of the execution substrate.
///
/// V2 (DAG-S34, IQ-008 D5) adds `registry_root`: a checkpoint commits to
/// the committee (Authority Ring, Validator Ring, stake table, epoch) as
/// well as to the substrate, so a syncing node can walk a chain of
/// checkpoints from the genesis committee and learn each successor
/// committee from the checkpoint the previous one signed — the Sui
/// checkpoint-verification method. Nothing was deployed under V1, so the
/// hash domain simply moves to `SUWAPPU-CHECKPOINT-V2`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Sequence number; increments by 1 per checkpoint.
    pub height: CheckpointHeight,
    /// DagBft round at which this checkpoint was taken.
    pub round: Round,
    /// `Substrate::state_root` immediately after the block at `round`
    /// was applied.
    pub state_root: [u8; 32],
    /// BLAKE3 hash of the previous checkpoint, or `[0; 32]` at `height = 0`.
    pub prev_checkpoint: [u8; 32],
    /// Commitment to the committee state in force after `round` (the
    /// node computes it over the registries + stake table + epoch; this
    /// crate treats it as opaque bytes). `[0; 32]` when unused.
    #[serde(default)]
    pub registry_root: [u8; 32],
    /// Commitment to the commit-derived body of the state snapshot
    /// served for this checkpoint (leader frontier, gc round, commit
    /// marks, queued governance) so a joiner cannot be fed a snapshot
    /// whose `state_root` is honest but whose commit marks or gc round
    /// are not (IQ-008 D5; consensus-review finding on S34.4). The node
    /// computes it; this crate treats it as opaque bytes. `[0; 32]` when
    /// unused.
    #[serde(default)]
    pub snapshot_root: [u8; 32],
}

impl Checkpoint {
    /// Canonical hash of this checkpoint.
    ///
    /// Encoding: `BLAKE3("SUWAPPU-CHECKPOINT-V2" || height (8 BE) || round (8 BE)
    /// || state_root || prev_checkpoint || registry_root || snapshot_root)`.
    pub fn hash(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"SUWAPPU-CHECKPOINT-V2");
        hasher.update(&self.height.to_be_bytes());
        hasher.update(&self.round.to_be_bytes());
        hasher.update(&self.state_root);
        hasher.update(&self.prev_checkpoint);
        hasher.update(&self.registry_root);
        hasher.update(&self.snapshot_root);
        let mut out = [0u8; 32];
        out.copy_from_slice(hasher.finalize().as_bytes());
        out
    }
}

/// A single Authority Ring member's signature over a checkpoint hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointSignature {
    /// Signing Authority Node.
    pub authority: AuthorityId,
    /// ML-DSA-65 detached signature bytes.
    pub signature: Vec<u8>,
}

/// A checkpoint plus an Authority-Ring-quorum of signatures verifying it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoSignedCheckpoint {
    /// The checkpoint itself.
    pub checkpoint: Checkpoint,
    /// At least `registry.quorum_threshold()` signatures, all verified.
    pub signatures: Vec<CheckpointSignature>,
}

/// Errors from the checkpoint ratification pipeline.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CheckpointError {
    /// Fewer than the registry's quorum-threshold count of signatures.
    #[error("checkpoint quorum below threshold: have {have}, need {need}")]
    BelowQuorum {
        /// Distinct signers supplied.
        have: u32,
        /// Quorum threshold required.
        need: u32,
    },
    /// A signature carried an authority id absent from the registry.
    #[error("signature from unknown authority {0}")]
    UnknownSigner(AuthorityId),
    /// A signature failed verification under the authority's public key.
    #[error("signature verification failed for authority {0}")]
    InvalidSignature(AuthorityId),
    /// The supplied `public_key_bytes` could not be parsed as ML-DSA-65.
    #[error("malformed public key for authority {0}")]
    MalformedKey(AuthorityId),
}

/// Sign `checkpoint` under `sk`. Phase-1 takes the ML-DSA-65 secret
/// key directly; the on-chain registry binding lands with the
/// validator-set sprint.
pub fn sign_checkpoint(
    authority: AuthorityId,
    sk: &mldsa::SecretKey,
    checkpoint: &Checkpoint,
) -> Result<CheckpointSignature, suwappu_crypto::CryptoError> {
    let digest = checkpoint.hash();
    let sig = mldsa::sign(&digest, sk)?;
    Ok(CheckpointSignature {
        authority,
        signature: sig.as_bytes().to_vec(),
    })
}

/// Ratify a checkpoint: verify every signature against the registry's
/// published public keys, dedup by `authority` id, and enforce that
/// the distinct-signer count meets the registry's quorum threshold.
pub fn ratify_checkpoint(
    checkpoint: Checkpoint,
    signatures: Vec<CheckpointSignature>,
    registry: &AuthorityRegistry,
) -> Result<CoSignedCheckpoint, CheckpointError> {
    let digest = checkpoint.hash();
    let need = registry.quorum_threshold();

    // Verify each signature and dedup by signer.
    let mut verified: BTreeMap<AuthorityId, CheckpointSignature> = BTreeMap::new();
    for sig in signatures {
        let member = registry
            .get(sig.authority)
            .ok_or(CheckpointError::UnknownSigner(sig.authority))?;
        let pk = mldsa::PublicKey::from_bytes(&member.public_key_bytes)
            .map_err(|_| CheckpointError::MalformedKey(sig.authority))?;
        let sig_typed = mldsa::Signature::from_bytes(&sig.signature)
            .map_err(|_| CheckpointError::InvalidSignature(sig.authority))?;
        mldsa::verify(&digest, &sig_typed, &pk)
            .map_err(|_| CheckpointError::InvalidSignature(sig.authority))?;
        verified.insert(sig.authority, sig);
    }

    let have = verified.len() as u32;
    if have < need {
        return Err(CheckpointError::BelowQuorum { have, need });
    }
    Ok(CoSignedCheckpoint {
        checkpoint,
        signatures: verified.into_values().collect(),
    })
}

/// One link of a checkpoint chain handed to a syncing node (IQ-008 D5):
/// a checkpoint, the signatures over it, and the committee that
/// checkpoint establishes (bound by `checkpoint.registry_root`, which the
/// caller recomputes from the full registry set as `next_registry_root`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainLink {
    /// The checkpoint.
    pub checkpoint: Checkpoint,
    /// Signatures claimed over it.
    pub signatures: Vec<CheckpointSignature>,
    /// Authority Ring in force after this checkpoint.
    pub next_committee: AuthorityRegistry,
    /// Caller-computed root of the full registry set `next_committee`
    /// belongs to; must equal `checkpoint.registry_root`.
    pub next_registry_root: [u8; 32],
}

/// Why a checkpoint chain was rejected.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ChainError {
    /// A link's signatures did not verify under the committee the
    /// previous link established.
    #[error("link {index}: {source}")]
    Link {
        /// Position in the chain.
        index: usize,
        /// The ratification failure.
        #[source]
        source: CheckpointError,
    },
    /// A link's `registry_root` does not bind the registry set supplied
    /// with it.
    #[error("link {index}: registry root does not match the supplied committee")]
    RegistryRootMismatch {
        /// Position in the chain.
        index: usize,
    },
    /// Heights or rounds are not strictly increasing.
    #[error("link {index}: height/round not strictly increasing")]
    NotMonotone {
        /// Position in the chain.
        index: usize,
    },
    /// A link at height `h + 1` does not name the verified link at
    /// height `h` as its predecessor, or the first link (height 0) has a
    /// non-zero `prev_checkpoint`.
    #[error("link {index}: prev_checkpoint does not match the preceding link")]
    BrokenLink {
        /// Position in the chain.
        index: usize,
    },
}

/// Verify a chain of co-signed checkpoints starting from a trusted
/// committee (the genesis Authority Ring), returning the verified
/// checkpoints in order (IQ-008 D5, invariant candidate I-CK1).
///
/// For each link, in order: the signatures must ratify under the
/// committee established by the *previous* link (the genesis committee
/// for the first), the `registry_root` must match the registry set
/// supplied with the link, and heights and rounds must strictly
/// increase. After a link verifies, its `next_committee` becomes the
/// committee for the next link. This is Sui's checkpoint-verification
/// walk: "the client needs to know the committee to verify checkpoints,
/// but also learns the committee through checkpoint validation."
///
/// The chain need not be contiguous: links may skip checkpoints whose
/// committee did not change, because only committee transitions have to
/// be witnessed. `prev_checkpoint` links are therefore not checked here;
/// the trust comes from each link being quorum-signed by the committee
/// the previous link established.
pub fn verify_checkpoint_chain(
    genesis_committee: &AuthorityRegistry,
    links: &[ChainLink],
) -> Result<Vec<CoSignedCheckpoint>, ChainError> {
    let mut committee = genesis_committee;
    let mut out = Vec::with_capacity(links.len());
    let mut last: Option<(CheckpointHeight, Round, [u8; 32])> = None;
    for (index, link) in links.iter().enumerate() {
        match last {
            Some((h, r, _)) if link.checkpoint.height <= h || link.checkpoint.round <= r => {
                return Err(ChainError::NotMonotone { index });
            }
            // Consecutive heights must chain by hash. The served chain is
            // deliberately sparse (committee transitions plus the latest),
            // so a gap is legal: every link is still signed by a quorum of
            // a committee that was legitimately in force, and the joiner
            // ends on the committee the last link binds. What a gap can
            // hide is only *which* legitimate committee signed; see IQ-008
            // Residual 6 (long-range keys) for why that is accepted.
            Some((h, _, prev_hash))
                if link.checkpoint.height == h + 1
                    && link.checkpoint.prev_checkpoint != prev_hash =>
            {
                return Err(ChainError::BrokenLink { index });
            }
            None if link.checkpoint.height == 0 && link.checkpoint.prev_checkpoint != [0u8; 32] => {
                return Err(ChainError::BrokenLink { index });
            }
            _ => {}
        }
        if link.checkpoint.registry_root != link.next_registry_root {
            return Err(ChainError::RegistryRootMismatch { index });
        }
        let cosigned =
            ratify_checkpoint(link.checkpoint.clone(), link.signatures.clone(), committee)
                .map_err(|source| ChainError::Link { index, source })?;
        last = Some((
            link.checkpoint.height,
            link.checkpoint.round,
            link.checkpoint.hash(),
        ));
        out.push(cosigned);
        committee = &link.next_committee;
    }
    Ok(out)
}

/// Cadence-driven checkpoint producer.
///
/// Emits a `Checkpoint` at every round `r` where `r % cadence == 0`
/// (and at `r = 0`). Maintains the `prev_checkpoint` chain so a
/// downstream verifier can walk the sequence back to genesis.
#[derive(Debug, Clone)]
pub struct Checkpointer {
    /// Cadence in rounds. Must be ≥ 1.
    cadence: u32,
    /// Next checkpoint sequence number.
    next_height: CheckpointHeight,
    /// Hash of the most recently emitted checkpoint, or `[0; 32]` if none.
    last_hash: [u8; 32],
}

impl Checkpointer {
    /// Construct a fresh checkpointer with the given cadence.
    pub fn new(cadence: u32) -> Self {
        assert!(cadence >= 1, "checkpoint cadence must be ≥ 1");
        Self {
            cadence,
            next_height: 0,
            last_hash: [0u8; 32],
        }
    }

    /// Cadence in rounds.
    pub fn cadence(&self) -> u32 {
        self.cadence
    }

    /// Most recent checkpoint's hash (`[0; 32]` before the first emission).
    pub fn last_hash(&self) -> [u8; 32] {
        self.last_hash
    }

    /// Number of checkpoints emitted so far.
    pub fn next_height(&self) -> CheckpointHeight {
        self.next_height
    }

    /// If `round` is a checkpoint boundary, build and return the next
    /// checkpoint over `state_root`, and advance internal state.
    /// Otherwise return `None`.
    pub fn maybe_emit(&mut self, round: Round, state_root: [u8; 32]) -> Option<Checkpoint> {
        self.maybe_emit_with_registry(round, state_root, [0u8; 32])
    }

    /// [`Self::maybe_emit`] with an explicit committee commitment
    /// (IQ-008 D5). The daemon passes the root over its registries so a
    /// joiner can learn committee changes from co-signed checkpoints.
    pub fn maybe_emit_with_registry(
        &mut self,
        round: Round,
        state_root: [u8; 32],
        registry_root: [u8; 32],
    ) -> Option<Checkpoint> {
        let is_boundary = (round % self.cadence as Round) == 0;
        if !is_boundary {
            return None;
        }
        let ck = Checkpoint {
            height: self.next_height,
            round,
            state_root,
            prev_checkpoint: self.last_hash,
            registry_root,
            snapshot_root: [0; 32],
        };
        self.last_hash = ck.hash();
        self.next_height += 1;
        Some(ck)
    }
}

#[cfg(test)]
mod tests {
    use suwappu_authority::{AuthorityMember, AUTHORITY_STAKE_THRESHOLD_SUWAPPU};

    use super::*;

    fn make_member(id: AuthorityId, pk_bytes: Vec<u8>) -> AuthorityMember {
        AuthorityMember {
            id,
            stake_suwappu: AUTHORITY_STAKE_THRESHOLD_SUWAPPU,
            public_key_bytes: pk_bytes,
        }
    }

    #[test]
    fn checkpoint_hash_is_deterministic() {
        let ck = Checkpoint {
            height: 0,
            round: 5,
            state_root: [0xAB; 32],
            prev_checkpoint: [0; 32],
            registry_root: [0; 32],
            snapshot_root: [0; 32],
        };
        assert_eq!(ck.hash(), ck.hash());
    }

    #[test]
    fn checkpoint_hash_changes_with_any_field() {
        let base = Checkpoint {
            height: 1,
            round: 5,
            state_root: [0xAB; 32],
            prev_checkpoint: [0xCD; 32],
            registry_root: [0; 32],
            snapshot_root: [0; 32],
        };
        let h0 = base.hash();
        let mut variant = base.clone();
        variant.height = 2;
        assert_ne!(h0, variant.hash());
        let mut variant = base.clone();
        variant.snapshot_root = [1; 32];
        assert_ne!(h0, variant.hash());
        let mut variant = base.clone();
        variant.round = 6;
        assert_ne!(h0, variant.hash());
        let mut variant = base.clone();
        variant.state_root[0] ^= 1;
        assert_ne!(h0, variant.hash());
        let mut variant = base.clone();
        variant.prev_checkpoint[0] ^= 1;
        assert_ne!(h0, variant.hash());
    }

    #[test]
    fn sign_and_verify_roundtrip() {
        let (pk, sk) = mldsa::keypair();
        let mut registry = AuthorityRegistry::new();
        registry
            .admit(make_member(0, pk.as_bytes().to_vec()))
            .unwrap();

        let ck = Checkpoint {
            height: 0,
            round: 0,
            state_root: [0xAA; 32],
            prev_checkpoint: [0; 32],
            registry_root: [0; 32],
            snapshot_root: [0; 32],
        };
        let sig = sign_checkpoint(0, &sk, &ck).unwrap();
        ratify_checkpoint(ck, vec![sig], &registry).unwrap();
    }

    #[test]
    fn below_quorum_is_rejected() {
        let (pk, sk) = mldsa::keypair();
        let mut registry = AuthorityRegistry::new();
        for i in 0..4 {
            // Same PK for simplicity; admission allows it since the
            // registry only enforces unique IDs.
            registry
                .admit(make_member(i, pk.as_bytes().to_vec()))
                .unwrap();
        }

        let ck = Checkpoint {
            height: 0,
            round: 0,
            state_root: [0; 32],
            prev_checkpoint: [0; 32],
            registry_root: [0; 32],
            snapshot_root: [0; 32],
        };
        // 4-member ring: quorum = 2f+1 = 3 (see IQ-001). One signature is below.
        let sig = sign_checkpoint(0, &sk, &ck).unwrap();
        match ratify_checkpoint(ck, vec![sig], &registry) {
            Err(CheckpointError::BelowQuorum { have: 1, need: 3 }) => {}
            other => panic!("expected BelowQuorum, got {:?}", other),
        }
    }

    #[test]
    fn checkpointer_emits_at_boundary_only() {
        let mut cp = Checkpointer::new(4);
        // Round 0 is a boundary (0 % 4 == 0).
        assert!(cp.maybe_emit(0, [0; 32]).is_some());
        // Rounds 1..4 are not.
        for r in 1..4 {
            assert!(cp.maybe_emit(r, [0; 32]).is_none());
        }
        // Round 4 is.
        assert!(cp.maybe_emit(4, [0; 32]).is_some());
    }

    #[test]
    fn checkpoint_chain_links_via_prev_hash() {
        let mut cp = Checkpointer::new(2);
        let c0 = cp.maybe_emit(0, [1; 32]).unwrap();
        let c1 = cp.maybe_emit(2, [2; 32]).unwrap();
        let c2 = cp.maybe_emit(4, [3; 32]).unwrap();

        assert_eq!(c0.prev_checkpoint, [0; 32]);
        assert_eq!(c1.prev_checkpoint, c0.hash());
        assert_eq!(c2.prev_checkpoint, c1.hash());
    }
}
