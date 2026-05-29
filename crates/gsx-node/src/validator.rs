//! `Validator` — the full integration of every prior crate into one
//! composable in-memory node. DAG-S20 exit gate.
//!
//! A `Validator` is a single Authority Ring member. Multiple
//! `Validator`s, in the same `AuthorityRegistry`, can be composed in a
//! test driver to exercise the full genesis-and-step flow end-to-end
//! against the in-memory substrate and the in-memory consensus DAG.
//!
//! What the genesis flow exercises:
//!
//! 1. Each validator authors a round-0 `Certificate` (paper §6.1, S3).
//! 2. Round-1 certificates reference every round-0 cert as parent.
//! 3. The Mysticeti-C commit rule (S4) commits the round-0 leader once
//!    `quorum_threshold(n)` round-1 supporters exist.
//! 4. Each validator runs the same `Block` through the `Substrate`
//!    (S10), producing identical post-state roots — the
//!    cross-validator state-root agreement.
//! 5. Validators each sign the joint state checkpoint (S11) and the
//!    set is ratified against the Authority registry's quorum.
//!
//! Each of the above pieces is independently property-tested at 10k
//! cases in its own sprint; S20 confirms they compose without seams.

use gsx_authority::{AuthorityMember, AuthorityRegistry, AUTHORITY_STAKE_THRESHOLD_GSX};
use gsx_consensus::{
    cert_at, commit_leader, AuthorityId, CertHash, Certificate, DagStore, Round, ValidatorId, Vote,
};
use gsx_crypto::mldsa;
use gsx_execution::{
    execute_block, ratify_checkpoint, sign_checkpoint, Block, Checkpoint, CheckpointSignature,
    Checkpointer, CoSignedCheckpoint, ExecutionReport, InMemorySubstrate, Intent, Substrate,
};
use gsx_validator::ValidatorRegistry;

/// Errors emitted by the node-integration layer.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum NodeError {
    /// The genesis flow could not seat the validator into the Authority
    /// registry.
    #[error("authority admission failed: {0}")]
    Admission(#[from] gsx_authority::AdmissionError),

    /// DAG insertion failed (parent missing, round monotonicity, etc).
    #[error("dag insert failed: {0}")]
    Dag(#[from] gsx_consensus::ConsensusError),

    /// Checkpoint ratification failed.
    #[error("checkpoint ratification failed: {0}")]
    Checkpoint(#[from] gsx_execution::CheckpointError),

    /// Certificate signature verification failed or malformed.
    #[error("certificate signature invalid for authority {0}")]
    CertSignature(AuthorityId),

    /// Certificate authored by an authority not in the registry.
    #[error("unknown authority {0}")]
    UnknownAuthority(AuthorityId),

    /// Vote signature verification failed or malformed.
    #[error("vote signature invalid for validator {0}")]
    VoteSignature(ValidatorId),

    /// Vote cast by a validator not in the registry.
    #[error("unknown validator {0}")]
    UnknownValidator(ValidatorId),
}

/// One Authority Ring member, fully wired.
pub struct Validator {
    /// Authority Ring identifier of this validator.
    pub id: AuthorityId,
    /// ML-DSA-65 public key, cached at construction for registry seating.
    pub mldsa_pk: mldsa::PublicKey,
    /// ML-DSA-65 secret key for signing certificates and checkpoints.
    pub mldsa_sk: mldsa::SecretKey,
    /// Local view of the certificate DAG.
    pub dag: DagStore,
    /// Local execution substrate.
    pub substrate: InMemorySubstrate,
    /// Per-validator checkpointer.
    pub checkpointer: Checkpointer,
}

impl Validator {
    /// Construct a fresh validator with the given id and an empty
    /// substrate / DAG / checkpointer at the supplied cadence.
    pub fn new(id: AuthorityId, checkpoint_cadence: u32) -> Self {
        let (pk, sk) = mldsa::keypair();
        Self {
            id,
            mldsa_pk: pk,
            mldsa_sk: sk,
            dag: DagStore::new(),
            substrate: InMemorySubstrate::new(),
            checkpointer: Checkpointer::new(checkpoint_cadence),
        }
    }

    /// Borrow the ML-DSA-65 public key bytes for this validator. Used
    /// when seating into an `AuthorityRegistry`.
    pub fn public_key_bytes(&self) -> Vec<u8> {
        self.mldsa_pk.as_bytes().to_vec()
    }

    /// Produce a signed genesis certificate (round 0, no parents).
    pub fn produce_genesis(&mut self, payload_digest: [u8; 32], network_id: &str) -> Certificate {
        let mut cert = Certificate::genesis(self.id, payload_digest);
        let sig = mldsa::sign(cert.hash(network_id).as_bytes(), &self.mldsa_sk)
            .expect("signing with own key cannot fail");
        cert.signature = sig.as_bytes().to_vec();
        cert
    }

    /// Author and sign a round-`round` certificate referencing every
    /// parent in `parents`.
    pub fn author_round(
        &mut self,
        round: Round,
        parents: Vec<CertHash>,
        payload_digest: [u8; 32],
        network_id: &str,
    ) -> Certificate {
        let mut cert = Certificate {
            author: self.id,
            round,
            parents,
            payload_digest,
            signature: vec![],
        };
        let sig = mldsa::sign(cert.hash(network_id).as_bytes(), &self.mldsa_sk)
            .expect("signing with own key cannot fail");
        cert.signature = sig.as_bytes().to_vec();
        cert
    }

    /// Insert a certificate (own or peer) into the local DAG.
    pub fn observe(&mut self, cert: Certificate, network_id: &str) -> Result<CertHash, NodeError> {
        Ok(self.dag.insert(cert, network_id)?)
    }

    /// Look up the cert hash this validator authored at `round`, if any.
    pub fn own_cert_at(&self, round: Round) -> Option<CertHash> {
        cert_at(&self.dag, round, self.id)
    }

    /// Execute a block through the local substrate and return both the
    /// report and the post-state root.
    pub fn execute(&mut self, block: &Block) -> (ExecutionReport, [u8; 32]) {
        let report = execute_block(&mut self.substrate, block);
        let root = self.substrate.state_root();
        (report, root)
    }

    /// Produce a checkpoint at `round` (if the checkpointer's cadence
    /// fires here) covering the current substrate state, and sign it.
    pub fn emit_and_sign_checkpoint(
        &mut self,
        round: Round,
    ) -> Option<(Checkpoint, CheckpointSignature)> {
        let state_root = self.substrate.state_root();
        let ck = self.checkpointer.maybe_emit(round, state_root)?;
        let sig = sign_checkpoint(self.id, &self.mldsa_sk, &ck).ok()?;
        Some((ck, sig))
    }
}

/// Sign a vote in place using the provided ML-DSA-65 secret key.
/// Sets `vote.signature` to the detached signature over
/// `vote_digest(network_id, vote.validator, &vote.candidate)`.
pub fn sign_vote(vote: &mut Vote, sk: &mldsa::SecretKey, network_id: &str) {
    let digest = gsx_consensus::vote_digest(network_id, vote.validator, &vote.candidate);
    let sig = mldsa::sign(&digest, sk).expect("ML-DSA-65 signing cannot fail");
    vote.signature = sig.as_bytes().to_vec();
}

/// Verify that a vote's ML-DSA-65 signature is valid against the
/// voter's public key in the Validator Registry.
///
/// Returns `Ok(())` if the signature is valid. Returns an error if the
/// voter is not in the registry, the key bytes are malformed, the
/// signature bytes are malformed, or the signature does not verify.
pub fn verify_vote_signature(
    vote: &Vote,
    registry: &ValidatorRegistry,
    network_id: &str,
) -> Result<(), NodeError> {
    let member = registry
        .get(vote.validator)
        .ok_or(NodeError::UnknownValidator(vote.validator))?;
    let pk = mldsa::PublicKey::from_bytes(&member.public_key_bytes)
        .map_err(|_| NodeError::VoteSignature(vote.validator))?;
    let sig = mldsa::Signature::from_bytes(&vote.signature)
        .map_err(|_| NodeError::VoteSignature(vote.validator))?;
    let digest = gsx_consensus::vote_digest(network_id, vote.validator, &vote.candidate);
    mldsa::verify(&digest, &sig, &pk).map_err(|_| NodeError::VoteSignature(vote.validator))?;
    Ok(())
}

/// Sign a certificate in place using the provided ML-DSA-65 secret key.
/// Sets `cert.signature` to the detached signature over `cert.hash(network_id)`.
pub fn sign_cert(cert: &mut Certificate, sk: &mldsa::SecretKey, network_id: &str) {
    let sig =
        mldsa::sign(cert.hash(network_id).as_bytes(), sk).expect("ML-DSA-65 signing cannot fail");
    cert.signature = sig.as_bytes().to_vec();
}

/// Verify that a certificate's ML-DSA-65 signature is valid against the
/// author's public key in the Authority Registry.
///
/// Returns `Ok(())` if the signature is valid. Returns an error if the
/// author is not in the registry, the key bytes are malformed, the
/// signature bytes are malformed, or the signature does not verify.
pub fn verify_cert_signature(
    cert: &Certificate,
    registry: &AuthorityRegistry,
    network_id: &str,
) -> Result<(), NodeError> {
    let member = registry
        .get(cert.author)
        .ok_or(NodeError::UnknownAuthority(cert.author))?;
    let pk = mldsa::PublicKey::from_bytes(&member.public_key_bytes)
        .map_err(|_| NodeError::CertSignature(cert.author))?;
    let sig = mldsa::Signature::from_bytes(&cert.signature)
        .map_err(|_| NodeError::CertSignature(cert.author))?;
    mldsa::verify(cert.hash(network_id).as_bytes(), &sig, &pk)
        .map_err(|_| NodeError::CertSignature(cert.author))?;
    Ok(())
}

/// Build an `AuthorityRegistry` seating `n` validators and return the
/// ML-DSA-65 public-key bytes for each id. Useful for E2E tests.
pub fn seed_registry(n: u32) -> (AuthorityRegistry, Vec<mldsa::SecretKey>) {
    let mut registry = AuthorityRegistry::new();
    let mut sks = Vec::with_capacity(n as usize);
    for i in 0..n {
        let (pk, sk) = mldsa::keypair();
        registry
            .admit(AuthorityMember {
                id: i,
                stake_gsx: AUTHORITY_STAKE_THRESHOLD_GSX,
                public_key_bytes: pk.as_bytes().to_vec(),
            })
            .expect("seed");
        sks.push(sk);
    }
    (registry, sks)
}

/// Variant of `run_genesis_flow` that uses a registry-bound set of SKs
/// so the ratification step verifies against the real public keys.
pub fn run_genesis_flow_with_keys(
    n: u32,
    registry: &AuthorityRegistry,
    sks: &[mldsa::SecretKey],
    payload_seed: u8,
    network_id: &str,
) -> Result<Option<(CertHash, [u8; 32], CoSignedCheckpoint)>, NodeError> {
    assert_eq!(sks.len() as u32, n);
    let mut dags: Vec<DagStore> = (0..n).map(|_| DagStore::new()).collect();
    let mut substrates: Vec<InMemorySubstrate> = (0..n).map(|_| InMemorySubstrate::new()).collect();

    // Round 0.
    let mut round_0_hashes = Vec::with_capacity(n as usize);
    for i in 0..n {
        let mut payload = [0u8; 32];
        payload[0] = i as u8;
        payload[1] = payload_seed;
        let mut cert = Certificate::genesis(i, payload);
        sign_cert(&mut cert, &sks[i as usize], network_id);
        round_0_hashes.push(cert.hash(network_id));
        for dag in &mut dags {
            dag.insert(cert.clone(), network_id)?;
        }
    }

    // Round 1.
    for i in 0..n {
        let mut payload = [0xAB; 32];
        payload[0] = i as u8;
        let mut cert = Certificate {
            author: i,
            round: 1,
            parents: round_0_hashes.clone(),
            payload_digest: payload,
            signature: vec![],
        };
        sign_cert(&mut cert, &sks[i as usize], network_id);
        for dag in &mut dags {
            dag.insert(cert.clone(), network_id)?;
        }
    }

    let leader = commit_leader(&dags[0], 0, n);

    // Execute empty block; all substrates begin empty, so all state
    // roots agree trivially.
    let block = Block {
        round: 0,
        intents: Vec::<Intent>::new(),
    };
    let mut roots: Vec<[u8; 32]> = Vec::with_capacity(n as usize);
    for substrate in &mut substrates {
        let _ = execute_block(substrate, &block);
        roots.push(substrate.state_root());
    }
    let canonical = roots[0];
    for r in &roots {
        if *r != canonical {
            return Ok(None);
        }
    }

    // Co-sign + ratify.
    let ck = Checkpoint {
        height: 0,
        round: 0,
        state_root: canonical,
        prev_checkpoint: [0u8; 32],
    };
    let mut sigs = Vec::with_capacity(n as usize);
    for (i, sk) in sks.iter().enumerate() {
        sigs.push(sign_checkpoint(i as AuthorityId, sk, &ck).expect("sign"));
    }
    let cosigned = ratify_checkpoint(ck, sigs, registry)?;
    Ok(leader.map(|h| (h, canonical, cosigned)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NET: &str = "test";

    #[test]
    fn genesis_flow_runs_end_to_end() {
        let n = 4u32;
        let (registry, sks) = seed_registry(n);
        let (leader, root, cosigned) = run_genesis_flow_with_keys(n, &registry, &sks, 0xAB, NET)
            .unwrap()
            .unwrap();
        // The leader is the cert authored by authority 0 at round 0
        // (round-robin pick).
        let mut payload = [0u8; 32];
        payload[0] = 0;
        payload[1] = 0xAB;
        let expected_leader = Certificate::genesis(0, payload).hash(NET);
        assert_eq!(leader, expected_leader);
        // Every validator's substrate is empty → identical state root.
        assert_eq!(root, InMemorySubstrate::new().state_root());
        // Ratification carried every validator's signature.
        assert_eq!(cosigned.signatures.len(), n as usize);
    }
}
