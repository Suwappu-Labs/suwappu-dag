//! Track H Phase 2 — the auditable memo envelope (CONF-1).
//!
//! Implements the encryption / selective-disclosure half of
//! `docs/iq/IQ-013-pq-confidential-transfers.md` Option B. This is the
//! primitive that gives confidential transfers their **auditable**
//! property: a note's secret fields are sealed to BOTH the transfer
//! recipient AND a designated auditor/regulator, so either party can
//! independently recover the cleartext while the public sees only
//! ciphertext. There is **no key escrow** of the recipient's key — the
//! auditor holds a second, wholly independent decryption path.
//!
//! ## Construction (hybrid PQ encryption, per slot)
//!
//! For each of the two viewing keys, independently:
//!
//! 1. `mlkem::encapsulate(viewing_pk)` → `(kem_ct, shared_secret)`.
//!    ML-KEM-768 is IND-CCA2 (FIPS 203); the shared secret is fresh and
//!    high-entropy.
//! 2. `aead_key   = hkdf_sha3_256(SUWAPPU_L2_MEMO_AEAD_V1, ss, "memo-aead-key",   32)`
//!    `aead_nonce = hkdf_sha3_256(SUWAPPU_L2_MEMO_AEAD_V1, ss, "memo-aead-nonce", 12)`.
//! 3. ChaCha20-Poly1305 seal of the canonical `NotePlaintext` bytes under
//!    `(aead_key, aead_nonce)` with an AAD binding the envelope domain and
//!    the slot role (`recipient` / `auditor`), so a slot's ciphertext
//!    cannot be replayed into the other slot.
//!
//! ## Nonce single-use safety (load-bearing)
//!
//! ChaCha20-Poly1305 is catastrophically broken under (key, nonce) reuse.
//! Here `encapsulate` is randomized, so every envelope draws a fresh KEM
//! shared secret; the KDF is deterministic in that secret, so the derived
//! `(key, nonce)` pair is **single-use by construction** — a fresh secret
//! per encryption yields a fresh key, hence the deterministic (KDF-derived)
//! nonce is never paired with the same key twice. We derive rather than
//! random-sample the nonce precisely so the whole envelope is reproducible
//! from `(viewing_pk, plaintext, kem_randomness)` with no separate nonce to
//! transport. Do NOT reuse a KEM shared secret across two seals.
//!
//! ## Consumer obligation (note-binding is NOT in the AAD yet)
//!
//! The per-slot AAD binds the envelope domain + role, but **not** the note
//! commitment `cm`. A slot ciphertext is therefore not self-bound to a
//! specific on-chain note: a lifted slot authenticates and yields *some*
//! valid `(v, r, pk_owner)`. This is safe **only** under the standard
//! note-discovery discipline — any consumer that decrypts a memo MUST
//! recompute `cm = commit_note(v, r, pk_owner)` and match it against the
//! on-chain commitment before trusting the plaintext; a transplanted slot
//! then yields a `cm` that does not match and is rejected. When the on-chain
//! consumer lands, bind `cm` (or a note id) into the AAD to make the memo
//! self-authenticating and remove the caller-discipline dependency
//! (crypto-review LOW/MED; tracked with the on-chain viewing-key
//! registration follow-up).
//!
//! ## Invariant notes
//!
//! - **Invariant 2 (PQ-conservative):** ML-KEM-768 (FIPS 203) +
//!   HKDF-SHA3-256 (FIPS 202) + ChaCha20-Poly1305 (256-bit symmetric,
//!   PQ-safe). No classical asymmetric crypto, no BN254/Groth16, no TEE.
//! - **Invariant 3 (constant-size LTP commitment):** the memo envelope is
//!   an **off-chain / DA artifact**. It adds NO bytes to the on-chain note
//!   commitment surface (`Commitment` stays 32 B) and does NOT touch the
//!   `suwappu-ltp::ON_CHAIN_COMMITMENT_BYTES = 1_600` LTP commitment. The
//!   envelope rides the DA layer / L2 memo field.
//!
//! ## Out of scope (documented follow-ups, deliberately NOT built here)
//!
//! - **Seeded / deterministic viewing-key derivation.** The underlying
//!   `pqcrypto-mlkem` `keypair()` uses OS randomness and exposes no
//!   derandomized keygen, so viewing keys are treated here as **registered
//!   ML-KEM-768 public keys** (generated once via `generate_viewing_keypair`)
//!   passed INTO the envelope as input. Deriving them from an account seed
//!   via `SHA3-256-domain(SUWAPPU_L2_VIEWING_KEY_V1, sk_seed)` (the tag
//!   already exists in `suwappu-crypto::hash`) requires a FIPS-203
//!   derandomized keygen path not yet available in the binding — see
//!   IQ-013 open question 5. This module is agnostic to how keys are made.
//! - **On-chain viewing-key registration** (publishing the account's
//!   viewing public key so senders can seal to it).
//! - **Phase 3 STARK** range / balance-conservation proof over the
//!   cleartext `v`. That is the soundness half of Option B and lands in the
//!   L2 STM circuit work (H.3 / G1 / G2); this module is the confidentiality
//!   half only.

use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    ChaCha20Poly1305, Nonce,
};
use serde::{Deserialize, Serialize};
use suwappu_crypto::hash::hkdf_sha3_256;
use suwappu_crypto::mlkem::{self, Ciphertext, PublicKey, SecretKey};
use zeroize::Zeroizing;

use crate::{ConfidentialError, Note};

/// HKDF salt for the memo AEAD key/nonce derivation. A **dedicated**
/// domain, distinct from `SUWAPPU_L2_VIEWING_KEY_V1` (which `hash.rs`
/// documents as the viewing-*keypair* derivation prefix) — the two must
/// not be semantically conflated even though a KEM shared secret is never
/// reused across subsystems. Per crypto-review, this makes the memo AEAD
/// material derivation self-documenting rather than borrowing an
/// overloaded tag.
const SUWAPPU_L2_MEMO_AEAD_V1: &[u8] = b"suwappu-l2-memo-aead-v1";

/// AEAD key width (ChaCha20-Poly1305 key = 256 bits).
const AEAD_KEY_LEN: usize = 32;
/// AEAD nonce width (ChaCha20-Poly1305 nonce = 96 bits).
const AEAD_NONCE_LEN: usize = 12;

/// HKDF `info` label for the per-slot AEAD key. MUST differ from the
/// nonce label so key and nonce are cryptographically independent
/// outputs of the same KEM shared secret.
const AEAD_KEY_INFO: &[u8] = b"memo-aead-key";
/// HKDF `info` label for the per-slot AEAD nonce.
const AEAD_NONCE_INFO: &[u8] = b"memo-aead-nonce";

/// AAD domain prefix binding a ciphertext to this envelope construction
/// and version.
const MEMO_AAD_DOMAIN: &[u8] = b"suwappu-l2-memo-envelope-v1";
/// AAD role label for the recipient slot.
const SLOT_RECIPIENT_LABEL: &[u8] = b"recipient";
/// AAD role label for the auditor slot.
const SLOT_AUDITOR_LABEL: &[u8] = b"auditor";

/// A registered per-account ML-KEM-768 **viewing public key**. Published
/// (off-chain registration is a follow-up); lets senders seal note memos
/// to the account.
///
/// Thin newtype over `mlkem::PublicKey` so it cannot be confused with the
/// LTP sealed-session public keys that share the same primitive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewingPublicKey(pub PublicKey);

/// A per-account ML-KEM-768 **viewing secret key**. Held by the account
/// owner; the *selective-disclosure* mechanism is handing a copy to an
/// authorized auditor for read-only recovery — it carries no spend
/// authority (spend is a separate ML-DSA-65 key, Phase 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewingSecretKey(pub SecretKey);

impl ViewingPublicKey {
    /// Borrow the inner ML-KEM-768 public key.
    pub fn as_inner(&self) -> &PublicKey {
        &self.0
    }
}

impl ViewingSecretKey {
    /// Borrow the inner ML-KEM-768 secret key.
    pub fn as_inner(&self) -> &SecretKey {
        &self.0
    }
}

/// Generate a fresh registered viewing keypair from system randomness.
///
/// Convenience over `mlkem::keypair()` returning the newtyped pair. Seeded
/// / deterministic derivation from an account seed is a documented
/// follow-up (see module docs); the envelope does not depend on how the
/// keypair was produced.
pub fn generate_viewing_keypair() -> (ViewingPublicKey, ViewingSecretKey) {
    let (pk, sk) = mlkem::keypair();
    (ViewingPublicKey(pk), ViewingSecretKey(sk))
}

/// The confidential fields a note commitment hides — exactly the inputs
/// `commit_note` binds (`v`, `r`, `pk_owner`). This is the AEAD payload the
/// memo carries so a viewer can recompute `cm` and recognize the note.
///
/// `position` is intentionally excluded: it is a public tree index, not a
/// confidential field, and is not bound by `commit_note`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotePlaintext {
    /// Cleartext amount.
    pub v: u64,
    /// Hiding randomness (blinding factor).
    pub r: [u8; 32],
    /// ML-DSA-65 public key of the note's owner (canonically 1312 bytes).
    pub pk_owner: Vec<u8>,
}

impl NotePlaintext {
    /// Extract the confidential fields from a `Note`.
    pub fn from_note(note: &Note) -> Self {
        Self {
            v: note.v,
            r: note.r,
            pk_owner: note.pk_owner.clone(),
        }
    }

    /// Canonical, fixed-layout, little-endian serialization used as the
    /// AEAD payload. Deterministic and injective:
    ///
    /// ```text
    ///   v            : u64 little-endian        ( 8 bytes)
    ///   r            : raw bytes                (32 bytes)
    ///   pk_owner_len : u32 little-endian        ( 4 bytes)
    ///   pk_owner     : raw bytes                (pk_owner_len bytes)
    /// ```
    ///
    /// The explicit length prefix plus the exact-length check on decode
    /// (`from_canonical_bytes`) make the encoding a bijection with
    /// `NotePlaintext` — no trailing bytes, no ambiguity — independent of
    /// any serde framing.
    pub fn to_canonical_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(8 + 32 + 4 + self.pk_owner.len());
        buf.extend_from_slice(&self.v.to_le_bytes());
        buf.extend_from_slice(&self.r);
        buf.extend_from_slice(&(self.pk_owner.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.pk_owner);
        buf
    }

    /// Inverse of `to_canonical_bytes`. Rejects any input that is not
    /// exactly one canonical encoding (short header, or trailing bytes past
    /// the declared `pk_owner_len`).
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ConfidentialError> {
        const HEADER: usize = 8 + 32 + 4;
        if bytes.len() < HEADER {
            return Err(ConfidentialError::MalformedMemoPlaintext);
        }
        let v = u64::from_le_bytes(bytes[0..8].try_into().expect("8-byte slice"));
        let mut r = [0u8; 32];
        r.copy_from_slice(&bytes[8..40]);
        let pk_len = u32::from_le_bytes(bytes[40..44].try_into().expect("4-byte slice")) as usize;
        // Exact-length check: no trailing bytes beyond the declared pk_owner.
        if bytes.len() != HEADER + pk_len {
            return Err(ConfidentialError::MalformedMemoPlaintext);
        }
        let pk_owner = bytes[HEADER..].to_vec();
        Ok(Self { v, r, pk_owner })
    }
}

/// One encryption of the note plaintext to a single viewing key: the
/// ML-KEM-768 ciphertext (sealed KEM shared secret) plus the
/// ChaCha20-Poly1305 ciphertext (with appended 16-byte Poly1305 tag).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoSlot {
    /// ML-KEM-768 ciphertext (≈1,568 B).
    pub kem_ciphertext: Vec<u8>,
    /// ChaCha20-Poly1305 ciphertext of the canonical `NotePlaintext`.
    pub aead_ciphertext: Vec<u8>,
}

/// The auditable memo envelope: TWO independent encryptions of the SAME
/// note plaintext — one to the recipient viewing key, one to the auditor
/// viewing key. Serializable so it can ride the DA layer / L2 memo field.
///
/// Off-chain / DA artifact only — see module docs re: Invariant 3.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoEnvelope {
    /// Encryption to the transfer recipient's viewing key.
    pub recipient: MemoSlot,
    /// Encryption to the designated auditor's viewing key (the
    /// selective-disclosure path).
    pub auditor: MemoSlot,
}

/// Build the AAD for a slot: `MEMO_AAD_DOMAIN || '/' || role_label`.
/// Distinct per role so a recipient-slot ciphertext cannot authenticate
/// as an auditor-slot ciphertext (or vice versa) even under a
/// hypothetical key coincidence.
fn slot_aad(role_label: &[u8]) -> Vec<u8> {
    let mut aad = Vec::with_capacity(MEMO_AAD_DOMAIN.len() + 1 + role_label.len());
    aad.extend_from_slice(MEMO_AAD_DOMAIN);
    aad.push(b'/');
    aad.extend_from_slice(role_label);
    aad
}

/// Derive `(aead_key, aead_nonce)` from a KEM shared secret via
/// HKDF-SHA3-256 under distinct info labels. See module docs for
/// single-use safety.
fn derive_aead_material(
    shared_secret: &[u8],
) -> (Zeroizing<[u8; AEAD_KEY_LEN]>, [u8; AEAD_NONCE_LEN]) {
    // `key_bytes` is secret; wrap it so it is wiped on drop even on the
    // error/early-return paths below. The nonce is not secret (it is
    // derived, single-use, and effectively public), so it need not be
    // zeroized.
    let key_bytes = Zeroizing::new(hkdf_sha3_256(
        SUWAPPU_L2_MEMO_AEAD_V1,
        shared_secret,
        AEAD_KEY_INFO,
        AEAD_KEY_LEN,
    ));
    let nonce_bytes = hkdf_sha3_256(
        SUWAPPU_L2_MEMO_AEAD_V1,
        shared_secret,
        AEAD_NONCE_INFO,
        AEAD_NONCE_LEN,
    );
    let mut key = Zeroizing::new([0u8; AEAD_KEY_LEN]);
    key.copy_from_slice(&key_bytes);
    let mut nonce = [0u8; AEAD_NONCE_LEN];
    nonce.copy_from_slice(&nonce_bytes);
    (key, nonce)
}

/// Seal `plaintext_bytes` to one viewing key, producing a `MemoSlot`.
fn seal_slot(
    viewing_pk: &PublicKey,
    aad: &[u8],
    plaintext_bytes: &[u8],
) -> Result<MemoSlot, ConfidentialError> {
    let (kem_ct, shared_secret) =
        mlkem::encapsulate(viewing_pk).map_err(|_| ConfidentialError::KemEncapsulationFailed)?;
    let (key, nonce) = derive_aead_material(shared_secret.as_bytes());
    let cipher = ChaCha20Poly1305::new_from_slice(&key[..])
        .expect("32-byte key is the valid ChaCha20 width");
    let aead_ct = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext_bytes,
                aad,
            },
        )
        .map_err(|_| ConfidentialError::AeadEncryptionFailed)?;
    Ok(MemoSlot {
        kem_ciphertext: kem_ct.as_bytes().to_vec(),
        aead_ciphertext: aead_ct,
    })
}

/// Attempt to open one slot with a viewing secret key. Returns the
/// authenticated plaintext, or `MemoDecryptionFailed` for any failure
/// (malformed KEM ct, wrong key ⇒ implicit-rejection secret, or AEAD tag
/// mismatch) — the failure is deliberately opaque.
fn open_slot(
    viewing_sk: &SecretKey,
    slot: &MemoSlot,
    aad: &[u8],
) -> Result<NotePlaintext, ConfidentialError> {
    let kem_ct = Ciphertext::from_bytes(&slot.kem_ciphertext)
        .map_err(|_| ConfidentialError::MemoDecryptionFailed)?;
    let shared_secret = mlkem::decapsulate(&kem_ct, viewing_sk)
        .map_err(|_| ConfidentialError::MemoDecryptionFailed)?;
    let (key, nonce) = derive_aead_material(shared_secret.as_bytes());
    let cipher = ChaCha20Poly1305::new_from_slice(&key[..])
        .expect("32-byte key is the valid ChaCha20 width");
    let plaintext_bytes = cipher
        .decrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &slot.aead_ciphertext,
                aad,
            },
        )
        .map_err(|_| ConfidentialError::MemoDecryptionFailed)?;
    NotePlaintext::from_canonical_bytes(&plaintext_bytes)
}

/// Encrypt a note memo to BOTH a recipient and an auditor viewing key.
///
/// Each slot is an independent ML-KEM-768 + ChaCha20-Poly1305 hybrid
/// encryption of the same canonical `NotePlaintext`. The two KEM shared
/// secrets are drawn fresh and independently, so the two slots leak
/// nothing about each other.
pub fn encrypt_note_memo(
    recipient: &ViewingPublicKey,
    auditor: &ViewingPublicKey,
    plaintext: &NotePlaintext,
) -> Result<MemoEnvelope, ConfidentialError> {
    let plaintext_bytes = plaintext.to_canonical_bytes();
    let recipient_slot = seal_slot(
        recipient.as_inner(),
        &slot_aad(SLOT_RECIPIENT_LABEL),
        &plaintext_bytes,
    )?;
    let auditor_slot = seal_slot(
        auditor.as_inner(),
        &slot_aad(SLOT_AUDITOR_LABEL),
        &plaintext_bytes,
    )?;
    Ok(MemoEnvelope {
        recipient: recipient_slot,
        auditor: auditor_slot,
    })
}

/// Decrypt a note memo with a viewing secret key, trying each slot.
///
/// The holder of the recipient viewing key opens the recipient slot; the
/// holder of the auditor viewing key opens the auditor slot. This is the
/// selective-disclosure mechanism: the auditor path is a second,
/// independent decryption, not an escrow of the recipient's key. Returns
/// the plaintext from whichever slot authenticates; `MemoDecryptionFailed`
/// if neither does (e.g. an unrelated key).
pub fn decrypt_note_memo(
    viewing_sk: &ViewingSecretKey,
    env: &MemoEnvelope,
) -> Result<NotePlaintext, ConfidentialError> {
    if let Ok(plaintext) = open_slot(
        viewing_sk.as_inner(),
        &env.recipient,
        &slot_aad(SLOT_RECIPIENT_LABEL),
    ) {
        return Ok(plaintext);
    }
    if let Ok(plaintext) = open_slot(
        viewing_sk.as_inner(),
        &env.auditor,
        &slot_aad(SLOT_AUDITOR_LABEL),
    ) {
        return Ok(plaintext);
    }
    Err(ConfidentialError::MemoDecryptionFailed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ML_DSA_65_PUBLIC_KEY_BYTES;

    /// A representative confidential plaintext with a canonical-width
    /// ML-DSA-65 owner key.
    fn sample_plaintext() -> NotePlaintext {
        NotePlaintext {
            v: 42_000_000,
            r: [0x5a; 32],
            pk_owner: vec![0x11; ML_DSA_65_PUBLIC_KEY_BYTES],
        }
    }

    // ----- canonical serialization -----

    #[test]
    fn canonical_bytes_round_trip() {
        let pt = sample_plaintext();
        let bytes = pt.to_canonical_bytes();
        assert_eq!(bytes.len(), 8 + 32 + 4 + ML_DSA_65_PUBLIC_KEY_BYTES);
        let back = NotePlaintext::from_canonical_bytes(&bytes).unwrap();
        assert_eq!(pt, back);
    }

    #[test]
    fn canonical_bytes_rejects_trailing() {
        let mut bytes = sample_plaintext().to_canonical_bytes();
        bytes.push(0xff);
        assert_eq!(
            NotePlaintext::from_canonical_bytes(&bytes),
            Err(ConfidentialError::MalformedMemoPlaintext)
        );
    }

    #[test]
    fn canonical_bytes_rejects_short_header() {
        assert_eq!(
            NotePlaintext::from_canonical_bytes(&[0u8; 10]),
            Err(ConfidentialError::MalformedMemoPlaintext)
        );
    }

    #[test]
    fn from_note_extracts_confidential_fields() {
        let note = Note {
            v: 7,
            r: [0x22; 32],
            pk_owner: vec![0x33; ML_DSA_65_PUBLIC_KEY_BYTES],
            position: 99,
        };
        let pt = NotePlaintext::from_note(&note);
        assert_eq!(pt.v, note.v);
        assert_eq!(pt.r, note.r);
        assert_eq!(pt.pk_owner, note.pk_owner);
    }

    // ----- round trips -----

    #[test]
    fn recipient_round_trip() {
        let (recipient_pk, recipient_sk) = generate_viewing_keypair();
        let (auditor_pk, _auditor_sk) = generate_viewing_keypair();
        let pt = sample_plaintext();

        let env = encrypt_note_memo(&recipient_pk, &auditor_pk, &pt).unwrap();
        let recovered = decrypt_note_memo(&recipient_sk, &env).unwrap();
        assert_eq!(recovered, pt);
    }

    /// The selective-disclosure property: the auditor secret key
    /// independently recovers the SAME plaintext the recipient does — a
    /// second decryption path, no escrow of the recipient key.
    #[test]
    fn auditor_round_trip_selective_disclosure() {
        let (recipient_pk, recipient_sk) = generate_viewing_keypair();
        let (auditor_pk, auditor_sk) = generate_viewing_keypair();
        let pt = sample_plaintext();

        let env = encrypt_note_memo(&recipient_pk, &auditor_pk, &pt).unwrap();

        let by_recipient = decrypt_note_memo(&recipient_sk, &env).unwrap();
        let by_auditor = decrypt_note_memo(&auditor_sk, &env).unwrap();

        assert_eq!(by_recipient, pt);
        assert_eq!(by_auditor, pt);
        assert_eq!(by_recipient, by_auditor);
    }

    /// A third, unrelated viewing key opens neither slot — no unintended
    /// disclosure.
    #[test]
    fn unrelated_key_decrypts_nothing() {
        let (recipient_pk, _recipient_sk) = generate_viewing_keypair();
        let (auditor_pk, _auditor_sk) = generate_viewing_keypair();
        let (_stranger_pk, stranger_sk) = generate_viewing_keypair();
        let pt = sample_plaintext();

        let env = encrypt_note_memo(&recipient_pk, &auditor_pk, &pt).unwrap();
        assert_eq!(
            decrypt_note_memo(&stranger_sk, &env),
            Err(ConfidentialError::MemoDecryptionFailed)
        );
    }

    // ----- tamper resistance (AEAD integrity) -----

    #[test]
    fn tamper_recipient_aead_ciphertext_fails() {
        let (recipient_pk, recipient_sk) = generate_viewing_keypair();
        let (auditor_pk, _auditor_sk) = generate_viewing_keypair();
        let pt = sample_plaintext();

        let mut env = encrypt_note_memo(&recipient_pk, &auditor_pk, &pt).unwrap();
        env.recipient.aead_ciphertext[0] ^= 0x01;

        // Recipient slot no longer authenticates; recipient key can't open
        // the auditor slot either, so the whole decrypt fails.
        assert_eq!(
            decrypt_note_memo(&recipient_sk, &env),
            Err(ConfidentialError::MemoDecryptionFailed)
        );
    }

    #[test]
    fn tamper_recipient_kem_ciphertext_fails() {
        let (recipient_pk, recipient_sk) = generate_viewing_keypair();
        let (auditor_pk, _auditor_sk) = generate_viewing_keypair();
        let pt = sample_plaintext();

        let mut env = encrypt_note_memo(&recipient_pk, &auditor_pk, &pt).unwrap();
        // Flip a byte: ML-KEM implicit rejection yields a different shared
        // secret ⇒ wrong AEAD key ⇒ tag verification fails.
        env.recipient.kem_ciphertext[0] ^= 0x01;

        assert_eq!(
            decrypt_note_memo(&recipient_sk, &env),
            Err(ConfidentialError::MemoDecryptionFailed)
        );
    }

    #[test]
    fn tamper_auditor_aead_ciphertext_fails() {
        let (recipient_pk, _recipient_sk) = generate_viewing_keypair();
        let (auditor_pk, auditor_sk) = generate_viewing_keypair();
        let pt = sample_plaintext();

        let mut env = encrypt_note_memo(&recipient_pk, &auditor_pk, &pt).unwrap();
        env.auditor.aead_ciphertext[0] ^= 0x01;

        assert_eq!(
            decrypt_note_memo(&auditor_sk, &env),
            Err(ConfidentialError::MemoDecryptionFailed)
        );
    }

    #[test]
    fn tamper_auditor_kem_ciphertext_fails() {
        let (recipient_pk, _recipient_sk) = generate_viewing_keypair();
        let (auditor_pk, auditor_sk) = generate_viewing_keypair();
        let pt = sample_plaintext();

        let mut env = encrypt_note_memo(&recipient_pk, &auditor_pk, &pt).unwrap();
        env.auditor.kem_ciphertext[0] ^= 0x01;

        assert_eq!(
            decrypt_note_memo(&auditor_sk, &env),
            Err(ConfidentialError::MemoDecryptionFailed)
        );
    }

    /// Tampering the recipient slot does not disturb the auditor slot: the
    /// auditor can still recover the plaintext. (Slots are independent.)
    #[test]
    fn tamper_recipient_slot_leaves_auditor_intact() {
        let (recipient_pk, _recipient_sk) = generate_viewing_keypair();
        let (auditor_pk, auditor_sk) = generate_viewing_keypair();
        let pt = sample_plaintext();

        let mut env = encrypt_note_memo(&recipient_pk, &auditor_pk, &pt).unwrap();
        env.recipient.aead_ciphertext[0] ^= 0x01;

        assert_eq!(decrypt_note_memo(&auditor_sk, &env).unwrap(), pt);
    }

    // ----- non-determinism (fresh KEM randomness) -----

    #[test]
    fn two_encryptions_differ_but_both_decrypt() {
        let (recipient_pk, recipient_sk) = generate_viewing_keypair();
        let (auditor_pk, auditor_sk) = generate_viewing_keypair();
        let pt = sample_plaintext();

        let env_a = encrypt_note_memo(&recipient_pk, &auditor_pk, &pt).unwrap();
        let env_b = encrypt_note_memo(&recipient_pk, &auditor_pk, &pt).unwrap();

        // Fresh KEM randomness per encryption ⇒ ciphertexts differ.
        assert_ne!(env_a, env_b);

        // ...but both decrypt to the same plaintext, from either path.
        assert_eq!(decrypt_note_memo(&recipient_sk, &env_a).unwrap(), pt);
        assert_eq!(decrypt_note_memo(&recipient_sk, &env_b).unwrap(), pt);
        assert_eq!(decrypt_note_memo(&auditor_sk, &env_a).unwrap(), pt);
        assert_eq!(decrypt_note_memo(&auditor_sk, &env_b).unwrap(), pt);
    }

    // ----- serialization -----

    #[test]
    fn envelope_serde_round_trip() {
        let (recipient_pk, recipient_sk) = generate_viewing_keypair();
        let (auditor_pk, _auditor_sk) = generate_viewing_keypair();
        let pt = sample_plaintext();

        let env = encrypt_note_memo(&recipient_pk, &auditor_pk, &pt).unwrap();
        let json = serde_json::to_string(&env).unwrap();
        let back: MemoEnvelope = serde_json::from_str(&json).unwrap();

        assert_eq!(env, back);
        // The deserialized envelope still decrypts.
        assert_eq!(decrypt_note_memo(&recipient_sk, &back).unwrap(), pt);
    }

    // ----- domain / label independence -----

    /// The AEAD key and nonce HKDF info labels are distinct, and produce
    /// distinct derived material from the same shared secret. A collision
    /// would be a catastrophic nonce-equals-key-prefix bug.
    #[test]
    fn key_and_nonce_labels_are_independent() {
        assert_ne!(AEAD_KEY_INFO, AEAD_NONCE_INFO);
        let shared = [0x77u8; 32];
        let (key, nonce) = derive_aead_material(&shared);
        // Compare the overlapping prefix (nonce is 12 B): they must differ.
        assert_ne!(&key[..AEAD_NONCE_LEN], &nonce[..]);
    }

    /// The two slot AADs are distinct, so a ciphertext is bound to its role.
    #[test]
    fn slot_aads_are_distinct() {
        assert_ne!(slot_aad(SLOT_RECIPIENT_LABEL), slot_aad(SLOT_AUDITOR_LABEL));
    }
}
