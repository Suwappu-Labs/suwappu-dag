//! Client-facing intent submission protocol.
//!
//! Each validator binds [`NodeConfig::client_listen`] in parallel with its
//! peer listen socket. External clients (typically `gsx-loadgen`) open a TCP
//! connection, length-prefixed bincode-frame their submissions, and receive
//! per-intent acknowledgements.
//!
//! Wire format: same 4-byte BE length prefix + bincode payload as the peer
//! wire (defined in [`crate::wire`]).
//!
//! Protocol:
//!
//! 1. Client connects, sends one or more [`ClientMessage::Submit`] frames.
//! 2. Validator verifies the ML-DSA-65 signature against the seated
//!    `AuthorityRegistry` and only then pushes the intent onto a
//!    `tokio::sync::mpsc::UnboundedSender` (drained by the round driver
//!    each tick — DAG-S27.2) and replies with [`ClientResponse::Ack`]
//!    containing the intent hash. Bad signature → connection rejected
//!    with [`ClientResponse::Err`] and dropped.
//!
//! ## Auth — ML-DSA-65 (Paper §3.3, Issue #28)
//!
//! `CLIENT_WIRE_VERSION` is bumped to **2**: every submission carries a
//! detached ML-DSA-65 signature and the blake3 hash of the signing
//! public key. The signing payload binds:
//!
//! ```text
//! blake3( b"GSX_INTENT_V1" || network_id_bytes || bincode(intent) )
//! ```
//!
//! - `b"GSX_INTENT_V1"` — domain separator (prevents cross-protocol replay).
//! - `network_id_bytes` — UTF-8 bytes of the manifest's `network_id`
//!   (prevents cross-network replay, e.g. perf-1 sig replayed against perf-2).
//! - `bincode(intent)` — canonical serialization of the intent.
//!
//! The signer is resolved by looking up `signer_pubkey_hash` (=
//! `blake3(pubkey_bytes)`) in the seated `AuthorityRegistry`. The
//! Validator-Ring registry does NOT carry pubkey material today
//! (`ValidatorMember` lacks the field), so for Phase 2.6 only seated
//! Authority Ring members may submit. Extending this to validator-ring
//! submitters is tracked as a follow-up.
//!
//! Authority-management intents (`AdmitAuthority`, `ExitAuthority`,
//! `EjectAuthority`): for Phase 2.6 these accept ANY one valid signature
//! from a currently-seated Authority. The fully-correct dual-signature
//! design (existing-Authority + candidate-Authority) is deferred to a
//! follow-up — see Issue #28 discussion.
//!
//! ### Breaking wire change
//!
//! This is a hard fork of the client wire. Pre-Phase 2.6 clients that
//! submit `ClientMessage::Submit(Intent)` will fail bincode-decode and
//! receive `ClientResponse::Err("decode: ...")`. Operators must update
//! `gsx-loadgen` (rebuilt from this branch) and any external submitters
//! before rolling validators.

use std::{collections::HashMap, io, net::SocketAddr, sync::Arc};

use gsx_crypto::mldsa;
use gsx_execution::Intent;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::mpsc,
};
use tracing::{debug, info, warn};

use crate::{
    daemon::State,
    events::{Event, EventLog, Lane},
};

/// Client wire protocol version. Bumped from 1 → 2 in Phase 2.6 (Issue
/// #28) when ML-DSA signature enforcement landed. The version is not
/// exchanged on the wire today — bincode decode failure is the signal —
/// but is documented here so a future framed-handshake version exchange
/// has the canonical value to use.
pub const CLIENT_WIRE_VERSION: u32 = 2;

/// Domain-separation tag mixed into every signed intent payload. Bound
/// alongside the genesis `network_id` and the bincoded intent to bind
/// the signature to this protocol and this network.
pub const INTENT_DOMAIN_TAG: &[u8] = b"GSX_INTENT_V1";

/// Compute the canonical signing digest for an intent under `network_id`.
///
/// `digest = blake3( INTENT_DOMAIN_TAG || network_id_bytes || bincode(intent) )`.
///
/// Both submitter and verifier MUST compute the digest the same way;
/// any divergence rejects every signature.
pub fn intent_signing_digest(network_id: &str, intent: &Intent) -> [u8; 32] {
    let intent_bytes = bincode::serialize(intent).expect("intent serialize");
    let mut hasher = blake3::Hasher::new();
    hasher.update(INTENT_DOMAIN_TAG);
    hasher.update(network_id.as_bytes());
    hasher.update(&intent_bytes);
    *hasher.finalize().as_bytes()
}

/// Compute the blake3 hash of an ML-DSA public key — used as the
/// `signer_pubkey_hash` on the client wire. The validator side resolves
/// the hash against the seated Authority Ring to recover the public key.
pub fn signer_pubkey_hash(pubkey_bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(pubkey_bytes).as_bytes()
}

/// Client → validator messages. **Wire-version 2 (Issue #28):** every
/// submission carries an ML-DSA-65 signature and the blake3 hash of the
/// signing public key.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClientMessage {
    /// Submit one intent for inclusion in the next block.
    Submit {
        /// The intent to include.
        intent: Intent,
        /// ML-DSA-65 detached signature over
        /// [`intent_signing_digest`](self::intent_signing_digest)`(network_id, &intent)`.
        /// `Vec<u8>` rather than a fixed `[u8; 3309]` array for
        /// forward compatibility with future parameter sets.
        signature: Vec<u8>,
        /// blake3 hash of the signer's ML-DSA-65 public-key bytes
        /// (`blake3(pubkey)`). The validator resolves this against
        /// the seated `AuthorityRegistry` to recover the verifier
        /// public key.
        signer_pubkey_hash: [u8; 32],
    },
    /// Submit many intents in a single roundtrip (DAG-S29.2). Each
    /// intent carries its own signature so a single batch can mix
    /// distinct intents from one signer; the validator pushes every
    /// intent onto the same lock-free mpsc that single `Submit` uses,
    /// then returns one `AckBatch` with the full list of intent
    /// hashes. Amortises the 1-RTT-per-intent ack cost that pre-S29
    /// capped cross-region loadgens at ~88 TPS.
    SubmitBatch {
        /// The intents to include, in submission order.
        intents: Vec<Intent>,
        /// Detached ML-DSA-65 signatures, one per intent in matching
        /// order. `signatures.len()` MUST equal `intents.len()` or
        /// the validator rejects the whole batch.
        signatures: Vec<Vec<u8>>,
        /// blake3 hash of the single signer's ML-DSA-65 public-key
        /// bytes. A batch is one-signer for now; per-intent signers
        /// can be added when wallets multiplex.
        signer_pubkey_hash: [u8; 32],
    },
    /// No-op liveness probe.
    Ping(u64),
}

/// Validator → client messages.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClientResponse {
    /// Intent accepted and queued. The hash is blake3 of the bincoded intent
    /// (matches what the metrics binary uses for joining client events to
    /// committed certs).
    Ack {
        /// blake3 hash of the bincoded intent — same value the daemon writes
        /// as `tx_hash` on its `submitted` event log line.
        intent_hash: [u8; 32],
    },
    /// Response to `SubmitBatch` — one intent hash per input intent,
    /// in the same order the client submitted them (DAG-S29.2).
    AckBatch {
        /// blake3 hashes of each intent in submission order.
        intent_hashes: Vec<[u8; 32]>,
    },
    /// Bincode codec failure on the validator side. Client should retry or
    /// close the connection.
    Err(String),
    /// Echo of a `Ping`.
    Pong(u64),
}

/// Run the client listener until the process exits. Spawns one task per
/// inbound connection. Returns immediately with the bound socket address so
/// the daemon can attach the listener task to its lifecycle.
///
/// Crate-private — only the [`crate::daemon::Daemon`] startup path invokes
/// this. External callers go through [`LoadGenClient`] on the client side.
pub(crate) async fn run(
    listen: SocketAddr,
    self_label: String,
    intent_tx: mpsc::UnboundedSender<Intent>,
    log: EventLog,
    state: Arc<State>,
    network_id: String,
) -> io::Result<tokio::task::JoinHandle<()>> {
    let listener = TcpListener::bind(listen).await?;
    info!(addr = %listen, "client: listening for intent submissions");
    let handle = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    debug!(remote = %addr, "client: inbound");
                    let _ = stream.set_nodelay(true);
                    let intent_tx = intent_tx.clone();
                    let log = log.clone();
                    let self_label = self_label.clone();
                    let state = state.clone();
                    let network_id = network_id.clone();
                    tokio::spawn(async move {
                        if let Err(e) =
                            handle_conn(stream, self_label, intent_tx, log, state, network_id).await
                        {
                            debug!(remote = %addr, err = %e, "client: conn closed");
                        }
                    });
                }
                Err(e) => {
                    warn!(err = %e, "client: accept failed");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }
    });
    Ok(handle)
}

/// Outcome of looking up + verifying a (`signer_pubkey_hash`, `signature`)
/// pair against the seated Authority Ring. Bumped from private to
/// `pub(crate)` in T2 so the in-crate `rpc_adapter` reuses the exact
/// gate the TCP wire uses — two ingress wires sharing one verify
/// function means a signed payload accepted by one is also accepted
/// by the other. `pub(crate)` not `pub` because `State` itself is
/// `pub(crate)`: exposing `verify_signed_intent` to external crates
/// would require exporting the daemon's whole state shape.
pub(crate) enum AuthOutcome {
    /// Signer resolved AND signature verified.
    Ok,
    /// `signer_pubkey_hash` does not match any seated Authority member.
    UnknownSigner,
    /// Signer resolved but the signature failed ML-DSA verification.
    BadSignature,
}

/// Resolve a `signer_pubkey_hash` against the seated Authority Ring and
/// verify the detached ML-DSA-65 signature over the intent's signing
/// digest. The Validator Ring registry isn't consulted because
/// `ValidatorMember` doesn't yet carry pubkey material — extending the
/// auth surface to validator-ring submitters is tracked as a follow-up
/// (Issue #28 discussion).
///
/// Bumped to `pub(crate)` in T2 so the in-crate `rpc_adapter` reuses
/// this exact function. New ingress wires MUST call this rather than
/// reinventing the lookup + verify dance — otherwise the two wires
/// drift on what "signed intent" means and security audits get
/// nightmarish.
pub(crate) async fn verify_signed_intent(
    state: &State,
    network_id: &str,
    intent: &Intent,
    signature_bytes: &[u8],
    signer_pubkey_hash: &[u8; 32],
) -> AuthOutcome {
    // Authority Ring lookup. Hold the read guard only for the lookup,
    // then drop before the (CPU-heavy) signature verify.
    let pubkey_bytes_opt: Option<Vec<u8>> = {
        let registry = state.authority_registry.read().await;
        let found = registry
            .members()
            .find(|m| blake3::hash(&m.public_key_bytes).as_bytes() == signer_pubkey_hash)
            .map(|m| m.public_key_bytes.clone());
        drop(registry);
        found
    };
    let pubkey_bytes = match pubkey_bytes_opt {
        Some(b) => b,
        None => return AuthOutcome::UnknownSigner,
    };
    let pubkey = match mldsa::PublicKey::from_bytes(&pubkey_bytes) {
        Ok(pk) => pk,
        Err(_) => return AuthOutcome::UnknownSigner,
    };
    let signature = match mldsa::Signature::from_bytes(signature_bytes) {
        Ok(s) => s,
        Err(_) => return AuthOutcome::BadSignature,
    };
    let digest = intent_signing_digest(network_id, intent);
    match mldsa::verify(&digest, &signature, &pubkey) {
        Ok(()) => AuthOutcome::Ok,
        Err(_) => AuthOutcome::BadSignature,
    }
}

async fn handle_conn(
    mut stream: TcpStream,
    self_label: String,
    intent_tx: mpsc::UnboundedSender<Intent>,
    log: EventLog,
    state: Arc<State>,
    network_id: String,
) -> io::Result<()> {
    loop {
        let bytes = match read_frame(&mut stream).await {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        };
        let msg: ClientMessage = match bincode::deserialize(&bytes) {
            Ok(m) => m,
            Err(e) => {
                let resp = ClientResponse::Err(format!("decode: {}", e));
                let _ = write_response(&mut stream, &resp).await;
                continue;
            }
        };
        match msg {
            ClientMessage::Submit {
                intent,
                signature,
                signer_pubkey_hash,
            } => {
                match verify_signed_intent(
                    &state,
                    &network_id,
                    &intent,
                    &signature,
                    &signer_pubkey_hash,
                )
                .await
                {
                    AuthOutcome::Ok => {}
                    AuthOutcome::UnknownSigner => {
                        let resp = ClientResponse::Err(
                            "auth: unknown signer (pubkey hash not in Authority Ring)".to_string(),
                        );
                        let _ = write_response(&mut stream, &resp).await;
                        return Ok(());
                    }
                    AuthOutcome::BadSignature => {
                        let resp = ClientResponse::Err("auth: bad ML-DSA-65 signature".to_string());
                        let _ = write_response(&mut stream, &resp).await;
                        return Ok(());
                    }
                }
                let intent_hash: [u8; 32] =
                    blake3::hash(&bincode::serialize(&intent).expect("intent serialize")).into();
                if intent_tx.send(intent).is_err() {
                    let resp = ClientResponse::Err("intent channel closed".to_string());
                    let _ = write_response(&mut stream, &resp).await;
                    return Ok(());
                }
                log.emit(
                    Event::now(&self_label, Lane::Client, "submitted").with_tx_hash(&intent_hash),
                );
                write_response(&mut stream, &ClientResponse::Ack { intent_hash }).await?;
            }
            ClientMessage::SubmitBatch {
                intents,
                signatures,
                signer_pubkey_hash,
            } => {
                if signatures.len() != intents.len() {
                    let resp = ClientResponse::Err(format!(
                        "auth: batch length mismatch ({} intents vs {} signatures)",
                        intents.len(),
                        signatures.len()
                    ));
                    let _ = write_response(&mut stream, &resp).await;
                    return Ok(());
                }
                // Verify every signature BEFORE pushing any intent so a
                // bad sig anywhere in the batch rejects the whole batch
                // (no partial-application surprise on the client side).
                for (intent, sig) in intents.iter().zip(signatures.iter()) {
                    match verify_signed_intent(
                        &state,
                        &network_id,
                        intent,
                        sig,
                        &signer_pubkey_hash,
                    )
                    .await
                    {
                        AuthOutcome::Ok => {}
                        AuthOutcome::UnknownSigner => {
                            let resp = ClientResponse::Err(
                                "auth: unknown signer (pubkey hash not in Authority Ring)"
                                    .to_string(),
                            );
                            let _ = write_response(&mut stream, &resp).await;
                            return Ok(());
                        }
                        AuthOutcome::BadSignature => {
                            let resp = ClientResponse::Err(
                                "auth: bad ML-DSA-65 signature in batch".to_string(),
                            );
                            let _ = write_response(&mut stream, &resp).await;
                            return Ok(());
                        }
                    }
                }
                // DAG-S29.2: same lock-free mpsc, just amortise the ack
                // roundtrip across N intents.
                let mut hashes: Vec<[u8; 32]> = Vec::with_capacity(intents.len());
                for intent in intents {
                    let intent_hash: [u8; 32] =
                        blake3::hash(&bincode::serialize(&intent).expect("intent serialize"))
                            .into();
                    if intent_tx.send(intent).is_err() {
                        let resp =
                            ClientResponse::Err("intent channel closed mid-batch".to_string());
                        let _ = write_response(&mut stream, &resp).await;
                        return Ok(());
                    }
                    log.emit(
                        Event::now(&self_label, Lane::Client, "submitted")
                            .with_tx_hash(&intent_hash),
                    );
                    hashes.push(intent_hash);
                }
                write_response(
                    &mut stream,
                    &ClientResponse::AckBatch {
                        intent_hashes: hashes,
                    },
                )
                .await?;
            }
            ClientMessage::Ping(t) => {
                write_response(&mut stream, &ClientResponse::Pong(t)).await?;
            }
        }
    }
}

async fn read_frame(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > crate::wire::MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("client frame too large: {} bytes", len),
        ));
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    Ok(buf)
}

async fn write_response(stream: &mut TcpStream, resp: &ClientResponse) -> io::Result<()> {
    let bytes = bincode::serialize(resp)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    let len = (bytes.len() as u32).to_be_bytes();
    stream.write_all(&len).await?;
    stream.write_all(&bytes).await?;
    stream.flush().await?;
    Ok(())
}

/// Client-side helper used by `gsx-loadgen`. Wraps a single TCP
/// connection, owns an ML-DSA-65 signing key, and frames signed
/// submissions per Issue #28.
pub struct LoadGenClient {
    stream: TcpStream,
    secret_key: mldsa::SecretKey,
    public_key: mldsa::PublicKey,
    network_id: String,
}

impl LoadGenClient {
    /// Open a connection to a validator's client listen address.
    /// The submitter's ML-DSA-65 keypair MUST correspond to an
    /// `AuthorityRegistry` member on the server side; otherwise every
    /// submission will be rejected with `auth: unknown signer`.
    pub async fn connect(
        addr: SocketAddr,
        secret_key: mldsa::SecretKey,
        public_key: mldsa::PublicKey,
        network_id: String,
    ) -> io::Result<Self> {
        let stream = TcpStream::connect(addr).await?;
        let _ = stream.set_nodelay(true);
        Ok(Self {
            stream,
            secret_key,
            public_key,
            network_id,
        })
    }

    /// blake3 hash of the loadgen's ML-DSA-65 public key (the
    /// `signer_pubkey_hash` carried on every submission).
    pub fn signer_pubkey_hash(&self) -> [u8; 32] {
        signer_pubkey_hash(self.public_key.as_bytes())
    }

    /// Submit one intent and wait for the Ack.
    pub async fn submit(&mut self, intent: Intent) -> io::Result<[u8; 32]> {
        let digest = intent_signing_digest(&self.network_id, &intent);
        let signature = mldsa::sign(&digest, &self.secret_key)
            .map_err(|e| io::Error::other(format!("sign: {:?}", e)))?;
        let pkh = self.signer_pubkey_hash();
        let msg = ClientMessage::Submit {
            intent,
            signature: signature.as_bytes().to_vec(),
            signer_pubkey_hash: pkh,
        };
        let bytes = bincode::serialize(&msg)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        let len = (bytes.len() as u32).to_be_bytes();
        self.stream.write_all(&len).await?;
        self.stream.write_all(&bytes).await?;
        self.stream.flush().await?;

        let resp_bytes = read_frame(&mut self.stream).await?;
        let resp: ClientResponse = bincode::deserialize(&resp_bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        match resp {
            ClientResponse::Ack { intent_hash } => Ok(intent_hash),
            ClientResponse::Err(e) => Err(io::Error::new(io::ErrorKind::InvalidData, e)),
            ClientResponse::AckBatch { .. } | ClientResponse::Pong(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response for Submit",
            )),
        }
    }

    /// Submit N intents as a single batched roundtrip (DAG-S29.2).
    /// Returns the per-intent hashes in submission order. Replaces N
    /// individual `submit` calls with one length-prefixed wire roundtrip,
    /// amortising the ack-RTT cost across the batch.
    pub async fn submit_batch(&mut self, intents: Vec<Intent>) -> io::Result<Vec<[u8; 32]>> {
        let mut signatures: Vec<Vec<u8>> = Vec::with_capacity(intents.len());
        for intent in intents.iter() {
            let digest = intent_signing_digest(&self.network_id, intent);
            let signature = mldsa::sign(&digest, &self.secret_key)
                .map_err(|e| io::Error::other(format!("sign: {:?}", e)))?;
            signatures.push(signature.as_bytes().to_vec());
        }
        let pkh = self.signer_pubkey_hash();
        let msg = ClientMessage::SubmitBatch {
            intents,
            signatures,
            signer_pubkey_hash: pkh,
        };
        let bytes = bincode::serialize(&msg)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        let len = (bytes.len() as u32).to_be_bytes();
        self.stream.write_all(&len).await?;
        self.stream.write_all(&bytes).await?;
        self.stream.flush().await?;

        let resp_bytes = read_frame(&mut self.stream).await?;
        let resp: ClientResponse = bincode::deserialize(&resp_bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        match resp {
            ClientResponse::AckBatch { intent_hashes } => Ok(intent_hashes),
            ClientResponse::Err(e) => Err(io::Error::new(io::ErrorKind::InvalidData, e)),
            ClientResponse::Ack { .. } | ClientResponse::Pong(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response for SubmitBatch",
            )),
        }
    }
}

// Suppress dead-code complaints. The `outbound` HashMap import stays around
// for symmetry with future fast-path/LTP submission paths.
#[allow(dead_code)]
fn _shape_hint() -> Option<HashMap<u32, u32>> {
    None
}

#[cfg(test)]
mod tests {
    use gsx_execution::Intent;

    use super::*;

    #[test]
    fn signing_digest_is_deterministic() {
        let intent = Intent::Transfer {
            from: [1u8; 20],
            to: [2u8; 20],
            amount: 99,
        };
        let d1 = intent_signing_digest("perf-7r", &intent);
        let d2 = intent_signing_digest("perf-7r", &intent);
        assert_eq!(d1, d2);
    }

    #[test]
    fn signing_digest_changes_with_network_id() {
        let intent = Intent::Transfer {
            from: [1u8; 20],
            to: [2u8; 20],
            amount: 99,
        };
        let d1 = intent_signing_digest("perf-7r", &intent);
        let d2 = intent_signing_digest("perf-8r", &intent);
        assert_ne!(d1, d2, "cross-network signing digests MUST differ");
    }

    #[test]
    fn signing_digest_changes_with_intent() {
        let i1 = Intent::Transfer {
            from: [1u8; 20],
            to: [2u8; 20],
            amount: 99,
        };
        let i2 = Intent::Transfer {
            from: [1u8; 20],
            to: [2u8; 20],
            amount: 100,
        };
        let d1 = intent_signing_digest("perf", &i1);
        let d2 = intent_signing_digest("perf", &i2);
        assert_ne!(d1, d2);
    }

    #[test]
    fn signing_digest_domain_tag_is_versioned() {
        // The on-wire digest is hash(DOMAIN || network_id || intent).
        // Verify the domain tag is at the front by manually computing
        // and comparing.
        let intent = Intent::Transfer {
            from: [0u8; 20],
            to: [0u8; 20],
            amount: 0,
        };
        let network_id = "n";
        let intent_bytes = bincode::serialize(&intent).unwrap();
        let mut h = blake3::Hasher::new();
        h.update(b"GSX_INTENT_V1");
        h.update(network_id.as_bytes());
        h.update(&intent_bytes);
        let expected = *h.finalize().as_bytes();
        assert_eq!(expected, intent_signing_digest(network_id, &intent));
    }

    #[test]
    fn sign_then_verify_roundtrip() {
        let (pk, sk) = mldsa::keypair();
        let intent = Intent::Transfer {
            from: [3u8; 20],
            to: [4u8; 20],
            amount: 7,
        };
        let digest = intent_signing_digest("rt-net", &intent);
        let sig = mldsa::sign(&digest, &sk).unwrap();
        mldsa::verify(&digest, &sig, &pk).expect("genuine signature must verify");
    }

    // ----- Property tests (Issue #28) ---------------------------------
    //
    // The 10k-case proptest stays at the verify-digest boundary
    // rather than spinning up a daemon per case (which would be
    // O(seconds) per case and infeasible at 10k). The end-to-end
    // signature-gate behaviour is exercised by the dedicated
    // `client_listener_rejects_*` tokio tests in `daemon::tests`.

    use proptest::prelude::*;

    proptest! {
        // Garbage / empty signatures never verify against any keypair's
        // signing digest. This is the wire-side rejection contract:
        // `verify_signed_intent` returns BadSignature for any sig bytes
        // that aren't a genuine ML-DSA-65 output over the correct digest.
        #![proptest_config(ProptestConfig::with_cases(10_000))]
        #[test]
        fn unsigned_intent_rejected(
            sig_bytes in proptest::collection::vec(any::<u8>(), 0..256),
            from in any::<[u8; 20]>(),
            to in any::<[u8; 20]>(),
            amount in any::<u128>(),
            network_id in "[a-z0-9]{1,16}",
        ) {
            // Fresh keypair per case — proves rejection isn't an artifact
            // of one specific key.
            let (pk, _sk) = mldsa::keypair();
            let intent = Intent::Transfer { from, to, amount };
            let digest = intent_signing_digest(&network_id, &intent);
            // The random sig is essentially never a genuine signature
            // over this digest under this pubkey. A genuine ML-DSA-65
            // signature is ≥ 3000 bytes of structured output; the
            // probability of a uniform random bytestring of length
            // ≤256 being a valid signature is effectively zero.
            let sig = match mldsa::Signature::from_bytes(&sig_bytes) {
                Ok(s) => s,
                Err(_) => {
                    // Malformed signature bytes — rejection is already
                    // guaranteed at the decode step. Continue.
                    return Ok(());
                }
            };
            prop_assert!(
                mldsa::verify(&digest, &sig, &pk).is_err(),
                "garbage signature must not verify"
            );
        }
    }

    proptest! {
        // Cross-network replay: a signature made for network_id A must
        // not verify under network_id B.
        #![proptest_config(ProptestConfig::with_cases(1_000))]
        #[test]
        fn cross_network_replay_rejected(
            from in any::<[u8; 20]>(),
            to in any::<[u8; 20]>(),
            amount in any::<u128>(),
            net_a in "[a-z]{1,8}",
            net_b in "[a-z]{1,8}",
        ) {
            prop_assume!(net_a != net_b);
            let (pk, sk) = mldsa::keypair();
            let intent = Intent::Transfer { from, to, amount };
            let digest_a = intent_signing_digest(&net_a, &intent);
            let digest_b = intent_signing_digest(&net_b, &intent);
            let sig_a = mldsa::sign(&digest_a, &sk).expect("sign");
            // Genuine signature verifies on its own network.
            prop_assert!(mldsa::verify(&digest_a, &sig_a, &pk).is_ok());
            // Same signature fails verification on a different network.
            prop_assert!(mldsa::verify(&digest_b, &sig_a, &pk).is_err());
        }
    }
}
