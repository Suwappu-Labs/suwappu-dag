//! Cross-implementation interoperability test for ML-DSA-65 (FIPS 204).
//!
//! `proptest_roundtrips.rs` proves this crate's `mldsa::sign`/`mldsa::verify`
//! agree with *themselves* — a bug shared by both sides of that round trip
//! (e.g. a subtly wrong domain separator, or a non-conformant but internally
//! consistent encoding) would pass every test in that file and still be
//! wrong. This file instead cross-checks against OpenSSL's independent
//! ML-DSA-65 implementation (`id-ml-dsa-65`, OID 2.16.840.1.101.3.4.3.18,
//! native since OpenSSL 3.5) in both directions:
//!
//!   1. OpenSSL generates a keypair and signs a message; this crate's
//!      `mldsa::verify` must accept it.
//!   2. This crate generates a keypair and signs a message; OpenSSL's
//!      `pkeyutl -verify` must accept it.
//!
//! Agreement in both directions is real evidence this crate's ML-DSA-65 is
//! byte-for-byte conformant with the FIPS 204 spec (as OpenSSL implements
//! it), not just internally self-consistent. This is not a substitute for
//! NIST ACVP known-answer-test vectors (deterministic seed -> exact expected
//! signature bytes) — those require network access to fetch NIST's
//! published KAT files, unavailable in this environment — but a real
//! independent-implementation interop pass is strictly stronger evidence
//! than the self-consistency round trips alone.
//!
//! Skips (prints a message, does not fail) if `openssl` isn't on PATH or
//! the installed version predates ML-DSA-65 support (OpenSSL < 3.5) — this
//! keeps CI green on older toolchains without silently deleting the check;
//! anyone who can run a current `openssl` gets real coverage.

use std::io::Write;
use std::process::Command;

use suwappu_crypto::mldsa;

/// SPKI DER header OpenSSL wraps around a raw ML-DSA-65 public key:
/// `SEQUENCE { SEQUENCE { OID 2.16.840.1.101.3.4.3.18 }, BIT STRING (0
/// unused bits) { <1952 raw bytes> } }`. Verified empirically against a
/// real `openssl genpkey -algorithm ML-DSA-65` key — see the comment at
/// the call site.
const SPKI_HEADER: [u8; 22] = [
    0x30, 0x82, 0x07, 0xb2, 0x30, 0x0b, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x03,
    0x12, 0x03, 0x82, 0x07, 0xa1, 0x00,
];

fn openssl_supports_mldsa65() -> bool {
    let Ok(out) = Command::new("openssl").args(["list", "-signature-algorithms"]).output() else {
        return false;
    };
    out.status.success() && String::from_utf8_lossy(&out.stdout).contains("ML-DSA-65")
}

fn write_temp(dir: &std::path::Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(bytes).unwrap();
    path
}

/// Direction 1: OpenSSL keygen + sign, this crate verifies.
#[test]
fn openssl_signed_message_verifies_with_suwappu_crypto() {
    if !openssl_supports_mldsa65() {
        eprintln!(
            "SKIP: openssl not on PATH or lacks ML-DSA-65 support (needs OpenSSL >= 3.5). \
             This test provides real cross-implementation interop coverage when it CAN run; \
             it does not fail closed on older toolchains."
        );
        return;
    }

    let dir = std::env::temp_dir().join(format!("suwappu-mldsa-interop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let sk_path = dir.join("sk.pem");
    let pk_der_path = dir.join("pk.der");
    let msg_path = write_temp(&dir, "msg.bin", b"suwappu openssl-interop direction 1");
    let sig_path = dir.join("sig.bin");

    let status = Command::new("openssl")
        .args(["genpkey", "-algorithm", "ML-DSA-65", "-out"])
        .arg(&sk_path)
        .status()
        .unwrap();
    assert!(status.success(), "openssl genpkey failed");

    let status = Command::new("openssl")
        .args(["pkey", "-in"])
        .arg(&sk_path)
        .args(["-pubout", "-outform", "DER", "-out"])
        .arg(&pk_der_path)
        .status()
        .unwrap();
    assert!(status.success(), "openssl pkey (pubout) failed");

    let status = Command::new("openssl")
        .args(["pkeyutl", "-sign", "-inkey"])
        .arg(&sk_path)
        .args(["-rawin", "-in"])
        .arg(&msg_path)
        .args(["-out"])
        .arg(&sig_path)
        .status()
        .unwrap();
    assert!(status.success(), "openssl pkeyutl -sign failed");

    // Strip the 22-byte SPKI DER header OpenSSL wraps the raw pubkey in.
    let pk_der = std::fs::read(&pk_der_path).unwrap();
    assert_eq!(pk_der.len(), 22 + 1952, "unexpected SPKI DER length for ML-DSA-65 pubkey");
    assert_eq!(&pk_der[..22], &SPKI_HEADER[..], "unexpected SPKI DER header — OpenSSL encoding changed?");
    let raw_pk = &pk_der[22..];

    let msg = std::fs::read(&msg_path).unwrap();
    let sig_bytes = std::fs::read(&sig_path).unwrap();

    let pk = mldsa::PublicKey::from_bytes(raw_pk).expect("openssl pubkey must parse as valid ML-DSA-65 pk");
    let sig = mldsa::Signature::from_bytes(&sig_bytes).expect("openssl signature must parse as valid ML-DSA-65 sig");
    mldsa::verify(&msg, &sig, &pk)
        .expect("suwappu_crypto::mldsa::verify must accept an OpenSSL-produced signature");

    let _ = std::fs::remove_dir_all(&dir);
}

/// Direction 2: this crate keygen + sign, OpenSSL verifies.
#[test]
fn suwappu_crypto_signed_message_verifies_with_openssl() {
    if !openssl_supports_mldsa65() {
        eprintln!("SKIP: openssl not on PATH or lacks ML-DSA-65 support (needs OpenSSL >= 3.5).");
        return;
    }

    let (pk, sk) = mldsa::keypair();
    let msg = b"suwappu openssl-interop direction 2".to_vec();
    let sig = mldsa::sign(&msg, &sk).expect("sign must succeed");

    let dir = std::env::temp_dir().join(format!("suwappu-mldsa-interop2-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    // Re-wrap this crate's raw pubkey bytes in the same SPKI DER header so
    // openssl's -pubin/-keyform DER accepts it.
    let mut pk_der = Vec::with_capacity(22 + 1952);
    pk_der.extend_from_slice(&SPKI_HEADER);
    pk_der.extend_from_slice(pk.as_bytes());
    assert_eq!(pk_der.len(), 22 + 1952);

    let pk_der_path = write_temp(&dir, "pk.der", &pk_der);
    let msg_path = write_temp(&dir, "msg.bin", &msg);
    let sig_path = write_temp(&dir, "sig.bin", sig.as_bytes());

    let out = Command::new("openssl")
        .args(["pkeyutl", "-verify", "-pubin", "-inkey"])
        .arg(&pk_der_path)
        .args(["-keyform", "DER", "-rawin", "-in"])
        .arg(&msg_path)
        .args(["-sigfile"])
        .arg(&sig_path)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("Signature Verified Successfully"),
        "openssl must verify a suwappu_crypto-produced ML-DSA-65 signature; \
         status={:?} stdout={stdout:?} stderr={:?}",
        out.status,
        String::from_utf8_lossy(&out.stderr),
    );

    let _ = std::fs::remove_dir_all(&dir);
}
