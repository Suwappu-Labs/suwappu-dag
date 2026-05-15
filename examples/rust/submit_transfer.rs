//! Example: build a signed Transfer intent and submit it via JSON-RPC.
//!
//! Run:
//!     cd examples/rust && cargo run --bin submit_transfer
//!
//! ## Devnet key caveat
//!
//! By default this example generates a FRESH ML-DSA-65 keypair at
//! runtime. The freshly-generated key's blake3 hash is NOT in the
//! devnet's seated `AuthorityRegistry`, so the submission will be
//! REJECTED by `verify_signed_intent` with `UnknownSigner`. That's
//! expected — the value here is showing the full client-side
//! construct-sign-submit pipeline, not actually moving balances.
//!
//! To actually land an intent, the example would need a key whose
//! `blake3(public_key_bytes)` matches a seated Authority. That
//! requires regenerating the devnet genesis with the example's
//! public key seated; a `gsx-keygen` helper that automates this
//! is tracked as a follow-up. Until then this example is
//! "demonstrate the wire shape" not "demonstrate working submission."

use anyhow::Result;
use gsx_execution::Intent;

#[tokio::main]
async fn main() -> Result<()> {
    let network_id = "gsx-devnet-local";
    let rpc_url = "http://127.0.0.1:9092";

    // 1. Build the intent. A Transfer is the simplest variant —
    //    moves `amount` from `from` to `to`. Addresses are 20 bytes
    //    (post-S10 the substrate uses an EVM-compatible address shape).
    let intent = Intent::Transfer {
        from: [0x01; 20],
        to: [0x02; 20],
        amount: 42,
    };

    // 2. Bincode-serialize for both the digest and the wire.
    let intent_bincode = bincode::serialize(&intent)?;

    // 3. Compute the signing digest:
    //      blake3( b"GSX_INTENT_V1" || network_id_bytes || intent_bincode )
    //    Both submitter and validator MUST compute the digest the
    //    same way; any divergence rejects the signature.
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"GSX_INTENT_V1");
    hasher.update(network_id.as_bytes());
    hasher.update(&intent_bincode);
    let digest = *hasher.finalize().as_bytes();

    // 4. Generate a fresh ML-DSA-65 keypair + sign the digest. In
    //    real usage you'd load the secret key from a seated
    //    operator's HSM / encrypted file; here we generate fresh so
    //    the example is self-contained. See the header note on the
    //    devnet "UnknownSigner" rejection that follows.
    let (pubkey, secret_key) = gsx_crypto::mldsa::keypair();
    let signature = gsx_crypto::mldsa::sign(&digest, &secret_key)
        .map_err(|e| anyhow::anyhow!("sign failed: {:?}", e))?;

    // 5. Compute the signer_pubkey_hash that the validator uses to
    //    look the signer up in the AuthorityRegistry.
    let signer_pubkey_hash: [u8; 32] = *blake3::hash(pubkey.as_bytes()).as_bytes();

    println!("constructed intent:        {:?}", intent);
    println!("intent_bincode bytes:      {}", intent_bincode.len());
    println!(
        "signature bytes:           {} (ML-DSA-65 fixed = 3309)",
        signature.as_bytes().len()
    );
    println!(
        "signer_pubkey_hash:        0x{}\n",
        hex::encode(signer_pubkey_hash)
    );

    // 6. Submit via the Rust SDK's raw submit path. Expect
    //    `UnknownSigner` (see header) on a fresh devnet.
    let client = gsx_client::Client::new(rpc_url);
    match client
        .submit_intent_raw(
            intent_bincode,
            signature.as_bytes().to_vec(),
            signer_pubkey_hash,
        )
        .await
    {
        Ok(intent_hash) => {
            println!(
                "✅ submitted; intent hash: 0x{}",
                hex::encode(intent_hash)
            );
        }
        Err(e) => {
            println!("❌ submission rejected: {}", e);
            println!(
                "   (this is expected on a fresh devnet — see this file's header)"
            );
        }
    }
    Ok(())
}
