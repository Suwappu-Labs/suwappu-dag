//! On-chain ML-DSA-65 (FIPS 204) signature-verification precompile core.
//!
//! This crate implements the load-bearing, security-critical part of Suwappu's
//! on-chain post-quantum signature verification (audit program P5b, Phase 1):
//! given `pubkey || signature || message`, it returns a 32-byte EVM word that
//! is `1` iff the ML-DSA-65 detached signature is valid, else `0`.
//!
//! It wraps the same NIST PQC reference verifier (`pqcrypto-mldsa`, ML-DSA-65)
//! used by `suwappu-crypto` for validator/consensus signatures, so it is genuinely
//! FIPS-204 post-quantum sound — no SNARK wrapper, no scheme substitution
//! (contrast the SP1→Groth16/BN254 path, which is Shor-broken; see
//! `docs/security/audits/suwappu/P5b_ONCHAIN_PQ.md` in suwappu-lattice-protocol).
//!
//! Two consumers:
//!   1. **Suwappu DAG EVM precompile** (via `suwappu-revm`): register at a fixed address
//!      so bridge contracts can `staticcall` it during mint/unlock/finalize.
//!   2. **Suwappu DAG intent handler** (ship-now path while the EVM substrate is
//!      finished): call [`verify`] directly from the execution substrate.
//!
//! The verifier is pure and stateless: output is a deterministic function of
//! the input bytes, never panics, and rejects all malformed inputs as `0`.

#![forbid(unsafe_code)]

use pqcrypto_mldsa::mldsa65;
// `from_bytes` is a trait associated function called via path
// (`mldsa65::PublicKey::from_bytes`); the traits must be in scope for
// resolution, but rustc's unused-import lint does not count path-form
// associated-fn calls as usage (known false positive). Removing these
// breaks compilation, so suppress the spurious warning.
#[allow(unused_imports)]
use pqcrypto_traits::sign::{DetachedSignature, PublicKey};

/// ML-DSA-65 (FIPS 204) public-key length in bytes.
pub const PK_LEN: usize = 1952;
/// ML-DSA-65 (FIPS 204) detached-signature length in bytes.
pub const SIG_LEN: usize = 3309;

/// Minimum valid precompile input length (`pubkey || signature`, empty message).
pub const MIN_INPUT_LEN: usize = PK_LEN + SIG_LEN;

/// EVM word for a verified signature (`0x00..01`).
pub const WORD_TRUE: [u8; 32] = {
    let mut w = [0u8; 32];
    w[31] = 1;
    w
};
/// EVM word for a rejected signature (`0x00..00`).
pub const WORD_FALSE: [u8; 32] = [0u8; 32];

/// Verify an ML-DSA-65 detached signature from a flat precompile input.
///
/// Input ABI (tight, no padding):
/// ```text
///   pubkey   : bytes[0          .. 1952]            (PK_LEN)
///   signature: bytes[1952       .. 1952+3309]       (SIG_LEN)
///   message  : bytes[1952+3309  .. ]                (variable, may be empty)
/// ```
/// Returns [`WORD_TRUE`] iff the signature is valid for `message` under
/// `pubkey`, else [`WORD_FALSE`]. Never panics; any malformed input
/// (short, bad key/sig encoding, invalid signature) maps to [`WORD_FALSE`].
#[must_use]
pub fn verify(input: &[u8]) -> [u8; 32] {
    if input.len() < MIN_INPUT_LEN {
        return WORD_FALSE;
    }
    let pk_bytes = &input[..PK_LEN];
    let sig_bytes = &input[PK_LEN..MIN_INPUT_LEN];
    let message = &input[MIN_INPUT_LEN..];

    let pk = match mldsa65::PublicKey::from_bytes(pk_bytes) {
        Ok(p) => p,
        Err(_) => return WORD_FALSE,
    };
    let sig = match mldsa65::DetachedSignature::from_bytes(sig_bytes) {
        Ok(s) => s,
        Err(_) => return WORD_FALSE,
    };
    match mldsa65::verify_detached_signature(&sig, message, &pk) {
        Ok(()) => WORD_TRUE,
        // VerificationError is #[non_exhaustive]; any error rejects.
        Err(_) => WORD_FALSE,
    }
}

/// Convenience: `true` iff [`verify`] returns [`WORD_TRUE`].
#[must_use]
pub fn is_valid(input: &[u8]) -> bool {
    verify(input) == WORD_TRUE
}

// ───────────────────────────────────────────────────────────────────────────
// Commit binding (P5b Phase 1): the message the committee actually signs.
// ───────────────────────────────────────────────────────────────────────────

/// Domain-separation tag for a bridged-value authorization, length-prefixed
/// into [`MintCommit::canonical_encoding`].
///
/// This prevents cross-purpose replay **only to the extent that every other
/// use of the signing key is itself domain-separated and prefix-non-colliding**
/// — `mldsa::{sign,verify}` are raw over arbitrary bytes, so the tag alone does
/// not protect a key shared with an un-prefixed protocol. Per IQ-009 the
/// recommended mitigation is key separation: a dedicated mint-authorization
/// committee key, not the raw consensus/DID key. Domain separation here is
/// necessary, not sufficient.
pub const COMMIT_DOMAIN: &[u8] = b"SUWAPPU-MINT-COMMIT-V1";

/// The exact authorization a committee member's ML-DSA-65 key signs to
/// authorize moving bridged value (mint / unlock / finalize) for one
/// cross-chain commit.
///
/// Binding *every* field — plus the domain tag and both chain ids — is what
/// closes the relayer-trust criticals (LTP-A-001 / C1): a relayer can no
/// longer assert a commit it did not witness, because it cannot forge the
/// committee's post-quantum signature over these bytes. The home-chain
/// intent handler is meant to verify this *before* the execution substrate
/// applies the balance mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintCommit<'a> {
    /// Source chain whose lock/burn is being honored.
    pub source_chain: u64,
    /// Target chain that receives the value (this chain).
    pub target_chain: u64,
    /// Unique commit identifier (already binds chain+address on the origin).
    pub commit_id: [u8; 32],
    /// Amount of value authorized, in the smallest unit.
    pub amount: u128,
    /// Recipient address bytes (20 for EVM; length-prefixed when encoded).
    pub recipient: &'a [u8],
}

impl MintCommit<'_> {
    /// Canonical, unambiguous byte encoding of the authorization.
    ///
    /// The domain tag and the variable-length `recipient` are both
    /// length-prefixed so that no two distinct commits — and no message
    /// from another protocol — can ever collide onto the same bytes
    /// (message-space injectivity). The `u32`-BE length-prefixed domain tag
    /// matches the workspace `sha3_256_domain(tag, data)` convention. All
    /// integers are big-endian.
    #[must_use]
    pub fn canonical_encoding(&self) -> Vec<u8> {
        let mut m = Vec::with_capacity(
            4 + COMMIT_DOMAIN.len() + 8 + 8 + 32 + 16 + 8 + self.recipient.len(),
        );
        m.extend_from_slice(&(COMMIT_DOMAIN.len() as u32).to_be_bytes());
        m.extend_from_slice(COMMIT_DOMAIN);
        m.extend_from_slice(&self.source_chain.to_be_bytes());
        m.extend_from_slice(&self.target_chain.to_be_bytes());
        m.extend_from_slice(&self.commit_id);
        m.extend_from_slice(&self.amount.to_be_bytes());
        m.extend_from_slice(&(self.recipient.len() as u64).to_be_bytes());
        m.extend_from_slice(self.recipient);
        m
    }
}

/// Verify that `signature` is a valid committee ML-DSA-65 authorization for
/// `commit` under `committee_pubkey`.
///
/// Returns `true` iff `signature` is a valid ML-DSA-65 signature over
/// `commit.canonical_encoding()`. This is the home-chain intent-handler
/// gate: the execution substrate calls it before applying a mint/unlock/
/// finalize, so value movement requires a post-quantum committee signature
/// bound to the exact commit — no relayer trust, no classical-signature
/// dependence on that path. Never panics; any malformed input is `false`.
#[must_use]
pub fn verify_mint_authorization(
    committee_pubkey: &[u8],
    commit: &MintCommit<'_>,
    signature: &[u8],
) -> bool {
    if committee_pubkey.len() != PK_LEN || signature.len() != SIG_LEN {
        return false;
    }
    let msg = commit.canonical_encoding();
    let mut input = Vec::with_capacity(MIN_INPUT_LEN + msg.len());
    input.extend_from_slice(committee_pubkey);
    input.extend_from_slice(signature);
    input.extend_from_slice(&msg);
    verify(&input) == WORD_TRUE
}

/// One signer contributing to a quorum: a registry `index` plus the public key
/// and signature attributed to it.
///
/// **The `pubkey` MUST be resolved by the caller from committed registry state
/// at `index`** — never taken from the witness — so that "a valid signature at
/// index `i`" means "a signature under the key the chain registered for slot
/// `i`", which only the holder of that key can produce.
pub struct QuorumSigner<'a> {
    /// Registry slot id of the signer (identity for distinctness).
    pub index: u32,
    /// Committed registry public key for `index` (caller-resolved, not witness).
    pub pubkey: &'a [u8],
    /// ML-DSA-65 detached signature over `commit.canonical_encoding()`.
    pub signature: &'a [u8],
}

/// Verify a `threshold`-of-n ML-DSA-65 quorum over `commit` from one signer set
/// (e.g. one ring).
///
/// Returns `true` iff at least `threshold` **distinct signer indices** each
/// carry a valid signature over `commit.canonical_encoding()`.
///
/// Distinctness is enforced on `index`, **never on signature bytes** — ML-DSA
/// signing is randomized, so one key produces unboundedly many valid signatures
/// over one message; counting by signature bytes would let a single corrupted
/// key satisfy the whole threshold and collapse k-of-n to 1-of-1. A repeated
/// index is counted once; an invalid signature is not counted; `threshold == 0`
/// is rejected (a quorum gate of zero is never valid). The caller composes the
/// joint-ring AND-gate by requiring this to hold *independently* for the
/// Authority set and the Validator set over disjoint key sets, and is
/// responsible for ring-disjointness, ejection, and snapshot status — this
/// function enforces only within-set distinct-signer threshold + validity.
#[must_use]
pub fn verify_mint_quorum(
    commit: &MintCommit<'_>,
    signers: &[QuorumSigner<'_>],
    threshold: usize,
) -> bool {
    if threshold == 0 {
        return false;
    }
    let msg = commit.canonical_encoding();
    let mut counted: Vec<u32> = Vec::new();
    for s in signers {
        if counted.contains(&s.index) {
            continue; // a signer counts at most once, regardless of how many sigs it sends
        }
        if s.pubkey.len() != PK_LEN || s.signature.len() != SIG_LEN {
            continue;
        }
        let mut input = Vec::with_capacity(MIN_INPUT_LEN + msg.len());
        input.extend_from_slice(s.pubkey);
        input.extend_from_slice(s.signature);
        input.extend_from_slice(&msg);
        if verify(&input) == WORD_TRUE {
            counted.push(s.index);
        }
    }
    counted.len() >= threshold
}

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use pqcrypto_traits::sign::{DetachedSignature, PublicKey};

    fn build(pk: &[u8], sig: &[u8], msg: &[u8]) -> Vec<u8> {
        let mut v = Vec::with_capacity(pk.len() + sig.len() + msg.len());
        v.extend_from_slice(pk);
        v.extend_from_slice(sig);
        v.extend_from_slice(msg);
        v
    }

    const MSG: &[u8] = b"suwappu-commit:chainid=8453|commitId=0xabcd|amount=1000|recipient=0xBEEF";

    #[test]
    fn fips204_sizes_match_constants() {
        let (pk, sk) = mldsa65::keypair();
        assert_eq!(pk.as_bytes().len(), PK_LEN);
        let sig = mldsa65::detached_sign(MSG, &sk);
        assert_eq!(sig.as_bytes().len(), SIG_LEN);
    }

    #[test]
    fn valid_signature_accepted() {
        let (pk, sk) = mldsa65::keypair();
        let sig = mldsa65::detached_sign(MSG, &sk);
        let input = build(pk.as_bytes(), sig.as_bytes(), MSG);
        assert_eq!(verify(&input), WORD_TRUE);
        assert!(is_valid(&input));
    }

    #[test]
    fn tampered_message_rejected() {
        let (pk, sk) = mldsa65::keypair();
        let sig = mldsa65::detached_sign(MSG, &sk);
        let mut m = MSG.to_vec();
        m[0] ^= 0xff;
        assert_eq!(
            verify(&build(pk.as_bytes(), sig.as_bytes(), &m)),
            WORD_FALSE
        );
    }

    #[test]
    fn tampered_signature_rejected() {
        let (pk, sk) = mldsa65::keypair();
        let sig = mldsa65::detached_sign(MSG, &sk);
        let mut s = sig.as_bytes().to_vec();
        s[100] ^= 0x01;
        assert_eq!(verify(&build(pk.as_bytes(), &s, MSG)), WORD_FALSE);
    }

    #[test]
    fn wrong_key_rejected() {
        let (_pk, sk) = mldsa65::keypair();
        let (pk2, _sk2) = mldsa65::keypair();
        let sig = mldsa65::detached_sign(MSG, &sk);
        assert_eq!(
            verify(&build(pk2.as_bytes(), sig.as_bytes(), MSG)),
            WORD_FALSE
        );
    }

    #[test]
    fn truncated_input_rejected_no_panic() {
        let (pk, sk) = mldsa65::keypair();
        let sig = mldsa65::detached_sign(MSG, &sk);
        let full = build(pk.as_bytes(), sig.as_bytes(), MSG);
        assert_eq!(verify(&full[..MIN_INPUT_LEN - 1]), WORD_FALSE);
        assert_eq!(verify(&[]), WORD_FALSE);
        assert_eq!(verify(&[0u8; 10]), WORD_FALSE);
    }

    #[test]
    fn empty_message_signature_accepted() {
        let (pk, sk) = mldsa65::keypair();
        let sig = mldsa65::detached_sign(b"", &sk);
        assert_eq!(
            verify(&build(pk.as_bytes(), sig.as_bytes(), b"")),
            WORD_TRUE
        );
    }

    #[test]
    fn malformed_key_bytes_rejected() {
        // right length, garbage content -> from_bytes may accept but verify fails,
        // or from_bytes rejects; either way WORD_FALSE, no panic.
        let (_pk, sk) = mldsa65::keypair();
        let sig = mldsa65::detached_sign(MSG, &sk);
        let junk_pk = vec![0xABu8; PK_LEN];
        assert_eq!(verify(&build(&junk_pk, sig.as_bytes(), MSG)), WORD_FALSE);
    }

    // ── Commit-binding (verify_mint_authorization) ──────────────────────────
    //
    // Revert-fails discipline: each `*_rejected` test below stays GREEN only
    // because `MintCommit::canonical_encoding` actually binds the named field.
    // Drop that field from the encoding (the "revert") and the matching test
    // goes RED — the bound value would no longer change the signed message.

    const RECIP: &[u8] = &[0xBE; 20];

    fn sample_commit() -> MintCommit<'static> {
        MintCommit {
            source_chain: 1,
            target_chain: 8453,
            commit_id: [0x11; 32],
            amount: 1_000,
            recipient: RECIP,
        }
    }

    #[test]
    fn mint_authorization_valid_accepted() {
        let (pk, sk) = mldsa65::keypair();
        let commit = sample_commit();
        let sig = mldsa65::detached_sign(&commit.canonical_encoding(), &sk);
        assert!(verify_mint_authorization(
            pk.as_bytes(),
            &commit,
            sig.as_bytes()
        ));
    }

    #[test]
    fn mint_authorization_tampered_amount_rejected() {
        let (pk, sk) = mldsa65::keypair();
        let commit = sample_commit();
        let sig = mldsa65::detached_sign(&commit.canonical_encoding(), &sk);
        let forged = MintCommit {
            amount: 1_000_000,
            ..sample_commit()
        };
        assert!(!verify_mint_authorization(
            pk.as_bytes(),
            &forged,
            sig.as_bytes()
        ));
    }

    #[test]
    fn mint_authorization_tampered_recipient_rejected() {
        let (pk, sk) = mldsa65::keypair();
        let commit = sample_commit();
        let sig = mldsa65::detached_sign(&commit.canonical_encoding(), &sk);
        let attacker: &[u8] = &[0xAC; 20];
        let forged = MintCommit {
            recipient: attacker,
            ..sample_commit()
        };
        assert!(!verify_mint_authorization(
            pk.as_bytes(),
            &forged,
            sig.as_bytes()
        ));
    }

    #[test]
    fn mint_authorization_tampered_commit_id_rejected() {
        let (pk, sk) = mldsa65::keypair();
        let commit = sample_commit();
        let sig = mldsa65::detached_sign(&commit.canonical_encoding(), &sk);
        let forged = MintCommit {
            commit_id: [0x22; 32],
            ..sample_commit()
        };
        assert!(!verify_mint_authorization(
            pk.as_bytes(),
            &forged,
            sig.as_bytes()
        ));
    }

    #[test]
    fn mint_authorization_tampered_source_chain_rejected() {
        let (pk, sk) = mldsa65::keypair();
        let commit = sample_commit();
        let sig = mldsa65::detached_sign(&commit.canonical_encoding(), &sk);
        let forged = MintCommit {
            source_chain: 999,
            ..sample_commit()
        };
        assert!(!verify_mint_authorization(
            pk.as_bytes(),
            &forged,
            sig.as_bytes()
        ));
    }

    #[test]
    fn mint_authorization_wrong_committee_key_rejected() {
        let (_pk, sk) = mldsa65::keypair();
        let (pk2, _sk2) = mldsa65::keypair();
        let commit = sample_commit();
        let sig = mldsa65::detached_sign(&commit.canonical_encoding(), &sk);
        assert!(!verify_mint_authorization(
            pk2.as_bytes(),
            &commit,
            sig.as_bytes()
        ));
    }

    #[test]
    fn mint_authorization_malformed_lengths_rejected_no_panic() {
        let (pk, sk) = mldsa65::keypair();
        let commit = sample_commit();
        let sig = mldsa65::detached_sign(&commit.canonical_encoding(), &sk);
        // short pubkey / short signature must be rejected, never panic.
        assert!(!verify_mint_authorization(
            &pk.as_bytes()[..PK_LEN - 1],
            &commit,
            sig.as_bytes()
        ));
        assert!(!verify_mint_authorization(
            pk.as_bytes(),
            &commit,
            &sig.as_bytes()[..SIG_LEN - 1]
        ));
    }

    #[test]
    fn canonical_encoding_is_domain_separated_and_injective() {
        let c = sample_commit();
        let enc = c.canonical_encoding();
        // u32-BE length-prefixed domain tag leads the encoding.
        let mut prefix = (COMMIT_DOMAIN.len() as u32).to_be_bytes().to_vec();
        prefix.extend_from_slice(COMMIT_DOMAIN);
        assert!(
            enc.starts_with(&prefix),
            "must be length-prefixed domain-separated"
        );
        // Length-prefixing the recipient prevents field-boundary collisions:
        // amount=0x...01 + recipient=[0x02,..] must not encode-equal
        // amount=0x...00 + recipient=[0x01,0x02,..].
        let a = MintCommit {
            amount: 1,
            recipient: &[0x02, 0x03],
            ..sample_commit()
        };
        let b = MintCommit {
            amount: 0,
            recipient: &[0x01, 0x02, 0x03],
            ..sample_commit()
        };
        assert_ne!(a.canonical_encoding(), b.canonical_encoding());
    }

    // ── Quorum (verify_mint_quorum) — distinct-signer threshold ──────────────

    #[test]
    fn quorum_three_distinct_signers_meets_threshold() {
        let commit = sample_commit();
        let enc = commit.canonical_encoding();
        let mut keys = Vec::new();
        for _ in 0..3 {
            let (pk, sk) = mldsa65::keypair();
            let sig = mldsa65::detached_sign(&enc, &sk);
            keys.push((pk, sig));
        }
        let signers: Vec<QuorumSigner> = keys
            .iter()
            .enumerate()
            .map(|(i, (pk, sig))| QuorumSigner {
                index: i as u32,
                pubkey: pk.as_bytes(),
                signature: sig.as_bytes(),
            })
            .collect();
        assert!(verify_mint_quorum(&commit, &signers, 3));
        assert!(verify_mint_quorum(&commit, &signers, 2));
        assert!(!verify_mint_quorum(&commit, &signers, 4)); // not enough distinct signers
    }

    #[test]
    fn quorum_rejects_many_sigs_from_one_signer() {
        // The headline attack: ML-DSA signing is randomized, so one key can emit
        // many DISTINCT valid signatures over the same message. Counting those as
        // separate quorum members would collapse k-of-n to 1-of-1. The quorum must
        // count signer 7 exactly once no matter how many signatures it sends.
        let commit = sample_commit();
        let enc = commit.canonical_encoding();
        let (pk, sk) = mldsa65::keypair();
        let sig_a = mldsa65::detached_sign(&enc, &sk);
        let sig_b = mldsa65::detached_sign(&enc, &sk);
        let sig_c = mldsa65::detached_sign(&enc, &sk);
        // Three genuinely different valid signatures, all from index 7.
        assert_ne!(sig_a.as_bytes(), sig_b.as_bytes());
        let signers = [
            QuorumSigner {
                index: 7,
                pubkey: pk.as_bytes(),
                signature: sig_a.as_bytes(),
            },
            QuorumSigner {
                index: 7,
                pubkey: pk.as_bytes(),
                signature: sig_b.as_bytes(),
            },
            QuorumSigner {
                index: 7,
                pubkey: pk.as_bytes(),
                signature: sig_c.as_bytes(),
            },
        ];
        assert!(verify_mint_quorum(&commit, &signers, 1)); // one distinct signer: meets 1
        assert!(!verify_mint_quorum(&commit, &signers, 2)); // but NOT 2 — collapse prevented
        assert!(!verify_mint_quorum(&commit, &signers, 3));
    }

    #[test]
    fn quorum_ignores_invalid_and_rejects_zero_threshold() {
        let commit = sample_commit();
        let enc = commit.canonical_encoding();
        let (pk0, sk0) = mldsa65::keypair();
        let sig0 = mldsa65::detached_sign(&enc, &sk0);
        let (pk1, _sk1) = mldsa65::keypair();
        let (_pk2, sk2) = mldsa65::keypair();
        let sig2_wrongmsg = mldsa65::detached_sign(b"not-the-commit", &sk2);
        let signers = [
            QuorumSigner {
                index: 0,
                pubkey: pk0.as_bytes(),
                signature: sig0.as_bytes(),
            }, // valid
            QuorumSigner {
                index: 1,
                pubkey: pk1.as_bytes(),
                signature: sig0.as_bytes(),
            }, // sig not under pk1
            QuorumSigner {
                index: 2,
                pubkey: pk0.as_bytes(),
                signature: sig2_wrongmsg.as_bytes(),
            }, // wrong msg
        ];
        assert!(verify_mint_quorum(&commit, &signers, 1)); // only index 0 counts
        assert!(!verify_mint_quorum(&commit, &signers, 2));
        // A zero threshold is never a valid gate, even with valid signers present.
        assert!(!verify_mint_quorum(&commit, &signers, 0));
    }

    #[test]
    fn quorum_pubkey_is_caller_resolved_contract() {
        // Load-bearing contract (lib doc + IQ-009): the primitive counts a valid
        // signature under the SUPPLIED pubkey at the supplied index — it does NOT,
        // and CANNOT, check that the pubkey matches the chain-registered key for
        // that index. If the caller passed witness-supplied pubkeys, an attacker
        // would just sign with their own keys at any indices and meet threshold.
        // This test pins that the substrate MUST resolve `pubkey` from committed
        // registry state; here an attacker's own keys at indices 0/1 are counted,
        // demonstrating exactly why.
        let commit = sample_commit();
        let enc = commit.canonical_encoding();
        let (atk0, ask0) = mldsa65::keypair();
        let (atk1, ask1) = mldsa65::keypair();
        let s0 = mldsa65::detached_sign(&enc, &ask0);
        let s1 = mldsa65::detached_sign(&enc, &ask1);
        let attacker_witness = [
            QuorumSigner {
                index: 0,
                pubkey: atk0.as_bytes(),
                signature: s0.as_bytes(),
            },
            QuorumSigner {
                index: 1,
                pubkey: atk1.as_bytes(),
                signature: s1.as_bytes(),
            },
        ];
        // With caller-controlled pubkeys this passes — which is WHY the caller must
        // overwrite `pubkey` with registry[index] before calling. Substrate exit
        // gate must add the "attacker key at claimed index → reject" case.
        assert!(verify_mint_quorum(&commit, &attacker_witness, 2));
    }
}
