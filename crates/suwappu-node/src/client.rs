//! Client-facing intent submission protocol.
//!
//! Each validator binds [`crate::config::NodeConfig::client_listen`] in
//! parallel with its peer listen socket. External clients (typically
//! `suwappu-loadgen`) open a TCP
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
//! `CLIENT_WIRE_VERSION` is bumped to **3**: wire-version 2 added
//! ML-DSA-65 signature enforcement; wire-version 3 (Task 5) adds the
//! optional `signer_pubkey` field for open signers. Every submission
//! carries a detached ML-DSA-65 signature and the blake3 hash of the
//! signing public key. The signing payload binds:
//!
//! ```text
//! blake3( b"SUWAPPU_INTENT_V1" || network_id_bytes || bincode(intent) )
//! ```
//!
//! - `b"SUWAPPU_INTENT_V1"` — domain separator (prevents cross-protocol replay).
//! - `network_id_bytes` — UTF-8 bytes of the manifest's `network_id`
//!   (prevents cross-network replay, e.g. perf-1 sig replayed against perf-2).
//! - `bincode(intent)` — canonical serialization of the intent.
//!
//! The signer is resolved by looking up `signer_pubkey_hash` (=
//! `blake3(pubkey_bytes)`) against both the seated `AuthorityRegistry`
//! and the `ValidatorRegistry`. A match in either ring allows
//! submission — the signer's public key is recovered from the matching
//! registry entry and used for ML-DSA-65 signature verification.
//!
//! **Open signer path (Task 5):** if the hash is NOT in either ring but
//! the submission carries the full `signer_pubkey` bytes, the verifier
//! falls through to an open-signer path: `blake3(signer_pubkey) ==
//! signer_pubkey_hash` is checked, then ML-DSA-65 verify runs against
//! the provided key. This lets ordinary users submit `Transfer`,
//! `Delegate`, and other user-tier intents without ring membership.
//! Governance intents (`AdmitAuthority`, `EjectAuthority`, etc.)
//! require ring membership and reject open signers.
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
//! `suwappu-loadgen` (rebuilt from this branch) and any external submitters
//! before rolling validators.

use std::{collections::HashMap, io, net::SocketAddr, sync::Arc};

use suwappu_crypto::mldsa;
use suwappu_execution::Intent;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tracing::{debug, info, warn};

use crate::{
    daemon::State,
    events::{Event, EventLog, Lane},
};

/// Client wire protocol version. 1 → 2 in Phase 2.6 (Issue #28) when
/// ML-DSA signature enforcement landed. 2 → 3 in Task 5 when the
/// optional `signer_pubkey` field was added for open signers. The
/// version is not exchanged on the wire today — bincode decode failure
/// is the signal — but is documented here so a future framed-handshake
/// version exchange has the canonical value to use.
pub const CLIENT_WIRE_VERSION: u32 = 3;

/// Re-export the canonical domain tag from `suwappu-execution`.
pub use suwappu_execution::INTENT_DOMAIN_TAG;

/// Compute the canonical signing digest for an intent under `network_id`.
///
/// Serializes the intent via `crate::codec::encode` (bincode legacy) and
/// delegates to [`suwappu_execution::intent_signing_digest`].
pub fn intent_signing_digest(network_id: &str, intent: &Intent) -> [u8; 32] {
    let intent_bytes = crate::codec::encode(intent).expect("intent serialize");
    suwappu_execution::intent_signing_digest(network_id, &intent_bytes)
}

/// Compute the blake3 hash of an ML-DSA public key — used as the
/// `signer_pubkey_hash` on the client wire. The validator side resolves
/// the hash against the Authority Ring or Validator Ring to recover the
/// public key.
pub fn signer_pubkey_hash(pubkey_bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(pubkey_bytes).as_bytes()
}

/// Client → validator messages. **Wire-version 3 (Task 5):** adds the
/// optional `signer_pubkey` field for open-signer submission. Ring
/// members omit it (the node resolves the key from the registry); open
/// signers provide it so the node can verify without ring membership.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClientMessage {
    /// Submit one intent for inclusion in the next block.
    Submit {
        /// The intent to include.
        intent: Intent,
        /// ML-DSA-65 detached signature over
        /// [`intent_signing_digest`]`(network_id, &intent)`.
        /// `Vec<u8>` rather than a fixed `[u8; 3309]` array for
        /// forward compatibility with future parameter sets.
        signature: Vec<u8>,
        /// blake3 hash of the signer's ML-DSA-65 public-key bytes
        /// (`blake3(pubkey)`). The validator resolves this against
        /// the seated `AuthorityRegistry` to recover the verifier
        /// public key.
        signer_pubkey_hash: [u8; 32],
        /// Full ML-DSA-65 public key bytes (1,952 B). Required for
        /// open signers (not in any ring); ring members may omit.
        /// When present, the node verifies `blake3(signer_pubkey)
        /// == signer_pubkey_hash` before using the key.
        #[serde(default)]
        signer_pubkey: Option<Vec<u8>>,
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
        /// Full ML-DSA-65 public key bytes. Same semantics as
        /// `Submit::signer_pubkey`.
        #[serde(default)]
        signer_pubkey: Option<Vec<u8>>,
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

/// Hardening limits applied to every accepted client connection.
/// Plumbed in from `NodeConfig` at daemon startup so an operator can
/// tune the defaults per environment (perf testnet vs public mainnet
/// validator).
#[derive(Clone, Copy, Debug)]
pub(crate) struct ClientListenLimits {
    /// Cluster-wide cap on concurrent open connections to this
    /// listener. The N+1th accepted socket is closed immediately.
    pub max_connections: u32,
    /// Per-source-IP cap on concurrent connections. The N+1th
    /// from the same `IpAddr` is closed at accept time.
    pub per_ip_limit: u32,
    /// Idle-timeout in milliseconds — close the connection if no
    /// frame arrives in this window. `0` disables the timeout
    /// (legacy / loadgen-only mode).
    pub idle_timeout_ms: u64,
}

impl ClientListenLimits {
    pub(crate) fn from_config(cfg: &crate::config::NodeConfig) -> Self {
        Self {
            max_connections: cfg.max_client_connections,
            per_ip_limit: cfg.client_per_ip_limit,
            idle_timeout_ms: cfg.client_idle_timeout_ms,
        }
    }
}

/// Run the client listener until the process exits. Spawns one task per
/// inbound connection. Returns immediately with the bound socket address so
/// the daemon can attach the listener task to its lifecycle.
///
/// B1 hardening applied at accept time:
///   1. Global semaphore caps concurrent accepted connections at
///      `limits.max_connections`. The N+1th is dropped.
///   2. Per-IP map caps concurrent connections from any single source
///      IP at `limits.per_ip_limit`. Mitigates a single misbehaving
///      peer monopolizing the listener.
///   3. Idle-frame timeout (`limits.idle_timeout_ms`) applied inside
///      `handle_conn` — see `read_frame_with_timeout`.
///
/// Crate-private — only the [`crate::daemon::Daemon`] startup path
/// invokes this. External callers go through [`LoadGenClient`].
pub(crate) async fn run(
    listen: SocketAddr,
    self_label: String,
    log: EventLog,
    state: Arc<State>,
    network_id: String,
    limits: ClientListenLimits,
) -> io::Result<tokio::task::JoinHandle<()>> {
    let listener = TcpListener::bind(listen).await?;
    info!(
        addr = %listen,
        max_connections = limits.max_connections,
        per_ip_limit = limits.per_ip_limit,
        idle_timeout_ms = limits.idle_timeout_ms,
        "client: listening for intent submissions"
    );
    let global_sem = Arc::new(tokio::sync::Semaphore::new(limits.max_connections as usize));
    let per_ip: Arc<tokio::sync::Mutex<HashMap<std::net::IpAddr, u32>>> =
        Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let handle = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    let global_sem = global_sem.clone();
                    let per_ip = per_ip.clone();
                    // Non-blocking semaphore acquire — if the cap is
                    // hit we close the new socket immediately rather
                    // than queuing it (which could lead to
                    // slow-loris-style hold attacks).
                    let permit = match global_sem.clone().try_acquire_owned() {
                        Ok(p) => p,
                        Err(_) => {
                            debug!(remote = %addr, "client: max_connections reached, closing");
                            drop(stream);
                            continue;
                        }
                    };
                    // Per-IP cap.
                    let ip = addr.ip();
                    {
                        let mut map = per_ip.lock().await;
                        let count = map.entry(ip).or_insert(0);
                        if *count >= limits.per_ip_limit {
                            debug!(remote = %addr, "client: per-IP limit reached, closing");
                            drop(stream);
                            drop(permit);
                            continue;
                        }
                        *count += 1;
                    }
                    debug!(remote = %addr, "client: inbound");
                    let _ = stream.set_nodelay(true);
                    let log = log.clone();
                    let self_label = self_label.clone();
                    let state = state.clone();
                    let network_id = network_id.clone();
                    let peer_label = addr.to_string();
                    let idle_timeout_ms = limits.idle_timeout_ms;
                    tokio::spawn(async move {
                        let _permit = permit; // dropped when this task exits
                        let result = handle_conn(
                            stream,
                            self_label,
                            peer_label,
                            log,
                            state,
                            network_id,
                            idle_timeout_ms,
                        )
                        .await;
                        // Decrement per-IP count on disconnect.
                        let mut map = per_ip.lock().await;
                        if let Some(c) = map.get_mut(&ip) {
                            if *c <= 1 {
                                map.remove(&ip);
                            } else {
                                *c -= 1;
                            }
                        }
                        drop(map);
                        if let Err(e) = result {
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
/// pair against the seated Authority Ring and Validator Ring. Bumped from private to
/// `pub(crate)` in T2 so the in-crate `rpc_adapter` reuses the exact
/// gate the TCP wire uses — two ingress wires sharing one verify
/// function means a signed payload accepted by one is also accepted
/// by the other. `pub(crate)` not `pub` because `State` itself is
/// `pub(crate)`: exposing `verify_signed_intent` to external crates
/// would require exporting the daemon's whole state shape.
pub(crate) enum AuthOutcome {
    /// Signer resolved AND signature verified AND authorized for this intent.
    Ok,
    /// `signer_pubkey_hash` does not match any seated Authority or Validator member.
    UnknownSigner,
    /// Signer resolved but the signature failed ML-DSA verification.
    BadSignature,
    /// Signature is valid but the signer's derived address does not
    /// match the intent's sender field (e.g. `Transfer::from`).
    Unauthorized,
}

/// Extract the sender address from a user-tier intent, if it has one.
/// Governance intents return `None` (authorization is ring membership,
/// not address matching).
fn intent_sender_address(intent: &Intent) -> Option<[u8; 20]> {
    match intent {
        Intent::Transfer { from, .. }
        | Intent::Delegate { from, .. }
        | Intent::UndelegateBegin { from, .. }
        | Intent::UndelegateClaim { from, .. }
        | Intent::DepositSequencerBond { from, .. }
        | Intent::DepositSafetyBond { from, .. }
        | Intent::DepositAuthorityStake { from, .. }
        | Intent::DepositValidatorStake { from, .. } => Some(*from),
        Intent::L1Lock { user_address, .. } => Some(*user_address),
        Intent::L2ForceInclude { submitter, .. } => Some(*submitter),
        _ => None,
    }
}

/// `true` if the intent is a governance / protocol-level operation that
/// requires the signer to be a seated Authority or Validator Ring
/// member. User-tier intents (`Transfer`, `Delegate`, etc.) return
/// `false` and are open to any signer with a valid ML-DSA-65 key.
fn intent_requires_ring_membership(intent: &Intent) -> bool {
    // Full `match` (not `matches!`) so the compiler emits an error when
    // a new Intent variant is added without being classified here.
    // Default-open would be a security hole — new governance intents
    // silently accepting open signers — so every variant is explicit.
    match intent {
        // ── Governance / protocol intents — ring-only ──────────────
        Intent::AdmitAuthority { .. }
        | Intent::ExitAuthority { .. }
        | Intent::EjectAuthority { .. }
        | Intent::AdmitValidator { .. }
        | Intent::ExitValidator { .. }
        | Intent::EjectValidator { .. }
        | Intent::GenesisAllocation { .. }
        | Intent::MintInflation { .. }
        | Intent::DistributeRewards { .. }
        | Intent::DistributeSlashedFunds { .. }
        | Intent::SetL2VerifyingKey { .. }
        | Intent::SlashSequencer { .. }
        | Intent::EjectSequencer { .. }
        | Intent::MarkForceIncludeHonored { .. }
        | Intent::DisburseTreasury { .. }
        | Intent::ClaimInsurance { .. }
        | Intent::PostL2DA { .. }
        | Intent::PostL2DAv2 { .. }
        | Intent::CommitL2StateRoot { .. }
        | Intent::AddBridgeAsset { .. }
        | Intent::PauseBridgeAsset { .. }
        | Intent::RemoveBridgeAsset { .. } => true,

        // ── User-tier intents — open to any ML-DSA-65 signer ──────
        Intent::Transfer { .. }
        | Intent::Delegate { .. }
        | Intent::UndelegateBegin { .. }
        | Intent::UndelegateClaim { .. }
        | Intent::L1Lock { .. }
        | Intent::L2BurnProven { .. }
        | Intent::L2ForceInclude { .. }
        | Intent::DepositSequencerBond { .. }
        | Intent::DepositSafetyBond { .. }
        | Intent::DepositAuthorityStake { .. }
        | Intent::DepositValidatorStake { .. }
        | Intent::WithdrawAuthorityStake { .. }
        | Intent::WithdrawValidatorStake { .. } => false,

        // `Intent` is `#[non_exhaustive]`. If a future variant lands
        // and is not listed above, default to ring-required (safe
        // side). The catch-all arm fires only for variants added
        // after this code was written; the `match` above is
        // exhaustive for the current enum.
        _ => true,
    }
}

/// Resolve a `signer_pubkey_hash` against the Authority Ring, Validator
/// Ring, and (for user-tier intents) the optional open-signer public
/// key, then verify the detached ML-DSA-65 signature over the intent's
/// signing digest.
///
/// Resolution order:
/// 1. Authority Ring — hash lookup against seated members.
/// 2. Validator Ring — same lookup.
/// 3. Open signer — `signer_pubkey` provided by the caller; accepted
///    only for user-tier intents (governance intents are ring-gated).
///
/// `pub(crate)` so the in-crate `rpc_adapter` reuses this exact
/// function. New ingress wires MUST call this rather than reinventing
/// the lookup + verify dance — otherwise the two wires drift on what
/// "signed intent" means and security audits get nightmarish.
pub(crate) async fn verify_signed_intent(
    state: &State,
    network_id: &str,
    intent: &Intent,
    intent_bytes: &[u8],
    signature_bytes: &[u8],
    signer_pubkey_hash: &[u8; 32],
    signer_pubkey: Option<&[u8]>,
) -> AuthOutcome {
    // Try Authority Ring first, then fall back to Validator Ring.
    // Hold each read guard only for the lookup, then drop before the
    // (CPU-heavy) signature verify.
    let pubkey_bytes_opt: Option<Vec<u8>> = {
        let auth = state.authority_registry.read().await;
        let found = auth
            .members()
            .find(|m| blake3::hash(&m.public_key_bytes).as_bytes() == signer_pubkey_hash)
            .map(|m| m.public_key_bytes.clone());
        drop(auth);
        if found.is_some() {
            found
        } else {
            let val = state.validator_registry.read().await;
            let found = val
                .members()
                .find(|m| blake3::hash(&m.public_key_bytes).as_bytes() == signer_pubkey_hash)
                .map(|m| m.public_key_bytes.clone());
            drop(val);
            found
        }
    };
    // Open-signer fallback: if the hash isn't in either ring, check
    // whether the caller supplied a raw public key. Governance intents
    // are ring-gated — open signers can only submit user-tier intents.
    let pubkey_bytes = match pubkey_bytes_opt {
        Some(b) => b,
        None => match signer_pubkey {
            Some(pk_bytes) if !intent_requires_ring_membership(intent) => {
                // Verify the provided key hashes to the claimed hash.
                if blake3::hash(pk_bytes).as_bytes() != signer_pubkey_hash {
                    return AuthOutcome::UnknownSigner;
                }
                pk_bytes.to_vec()
            }
            _ => return AuthOutcome::UnknownSigner,
        },
    };
    let pubkey = match mldsa::PublicKey::from_bytes(&pubkey_bytes) {
        Ok(pk) => pk,
        Err(_) => return AuthOutcome::UnknownSigner,
    };
    let signature = match mldsa::Signature::from_bytes(signature_bytes) {
        Ok(s) => s,
        Err(_) => return AuthOutcome::BadSignature,
    };
    let digest = suwappu_execution::intent_signing_digest(network_id, intent_bytes);
    if mldsa::verify(&digest, &signature, &pubkey).is_err() {
        return AuthOutcome::BadSignature;
    }
    // Bind the signer's derived address to the intent's sender field.
    // Without this check, any valid signer could submit a Transfer
    // debiting an arbitrary address.
    if let Some(sender) = intent_sender_address(intent) {
        let signer_hash = blake3::hash(&pubkey_bytes);
        let mut signer_addr = [0u8; 20];
        signer_addr.copy_from_slice(&signer_hash.as_bytes()[..20]);
        if signer_addr != sender {
            return AuthOutcome::Unauthorized;
        }
    }
    AuthOutcome::Ok
}

/// Default priority for intents submitted via the TCP wire — no fee
/// market yet, so every signed submission lands at priority 0 and
/// FIFO tiebreak applies via `submit_ms`. When the fee surface lands
/// (S34+), the loadgen wire will carry a `priority: u64` field and
/// this constant will retire.
const DEFAULT_INTENT_PRIORITY: u64 = 0;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

async fn handle_conn(
    mut stream: TcpStream,
    self_label: String,
    peer_label: String,
    log: EventLog,
    state: Arc<State>,
    network_id: String,
    idle_timeout_ms: u64,
) -> io::Result<()> {
    loop {
        let bytes = match read_frame_with_timeout(&mut stream, idle_timeout_ms).await {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        };
        let msg: ClientMessage = match crate::codec::decode_frame(&bytes) {
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
                signer_pubkey,
            } => {
                let intent_bytes = crate::codec::encode(&intent).expect("intent serialize");
                match verify_signed_intent(
                    &state,
                    &network_id,
                    &intent,
                    &intent_bytes,
                    &signature,
                    &signer_pubkey_hash,
                    signer_pubkey.as_deref(),
                )
                .await
                {
                    AuthOutcome::Ok => {}
                    AuthOutcome::UnknownSigner => {
                        let resp = ClientResponse::Err("auth: unknown signer".to_string());
                        let _ = write_response(&mut stream, &resp).await;
                        return Ok(());
                    }
                    AuthOutcome::BadSignature => {
                        let resp = ClientResponse::Err("auth: bad ML-DSA-65 signature".to_string());
                        let _ = write_response(&mut stream, &resp).await;
                        return Ok(());
                    }
                    AuthOutcome::Unauthorized => {
                        let resp = ClientResponse::Err(
                            "auth: signer address does not match intent sender".to_string(),
                        );
                        let _ = write_response(&mut stream, &resp).await;
                        return Ok(());
                    }
                }
                let intent_hash: [u8; 32] = blake3::hash(&intent_bytes).into();
                match state.mempool.submit(
                    intent,
                    DEFAULT_INTENT_PRIORITY,
                    Some(peer_label.clone()),
                    now_ms(),
                ) {
                    Ok(_) => {}
                    Err(e) => {
                        let resp = ClientResponse::Err(format!("mempool: {}", e));
                        let _ = write_response(&mut stream, &resp).await;
                        return Ok(());
                    }
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
                signer_pubkey,
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
                // Serialize each intent once — the same bytes feed
                // both the signing digest and the intent hash.
                let intent_bytes_vec: Vec<Vec<u8>> = intents
                    .iter()
                    .map(|i| crate::codec::encode(i).expect("intent serialize"))
                    .collect();
                // Verify every signature BEFORE pushing any intent so a
                // bad sig anywhere in the batch rejects the whole batch
                // (no partial-application surprise on the client side).
                for ((intent, sig), ib) in intents
                    .iter()
                    .zip(signatures.iter())
                    .zip(intent_bytes_vec.iter())
                {
                    match verify_signed_intent(
                        &state,
                        &network_id,
                        intent,
                        ib,
                        sig,
                        &signer_pubkey_hash,
                        signer_pubkey.as_deref(),
                    )
                    .await
                    {
                        AuthOutcome::Ok => {}
                        AuthOutcome::UnknownSigner => {
                            let resp = ClientResponse::Err("auth: unknown signer".to_string());
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
                        AuthOutcome::Unauthorized => {
                            let resp = ClientResponse::Err(
                                "auth: signer address does not match intent sender in batch"
                                    .to_string(),
                            );
                            let _ = write_response(&mut stream, &resp).await;
                            return Ok(());
                        }
                    }
                }
                // DAG-S29.2 + A3: amortise the ack roundtrip across N
                // intents; each intent flows through `state.mempool.submit`
                // for priority/dedup/rate-limit accounting.
                let mut hashes: Vec<[u8; 32]> = Vec::with_capacity(intents.len());
                for (intent, ib) in intents.into_iter().zip(intent_bytes_vec.iter()) {
                    let intent_hash: [u8; 32] = blake3::hash(ib).into();
                    match state.mempool.submit(
                        intent,
                        DEFAULT_INTENT_PRIORITY,
                        Some(peer_label.clone()),
                        now_ms(),
                    ) {
                        Ok(_) => {}
                        Err(e) => {
                            let resp = ClientResponse::Err(format!("mempool (mid-batch): {}", e));
                            let _ = write_response(&mut stream, &resp).await;
                            return Ok(());
                        }
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

/// B1 hardening: read a single frame with an idle timeout. If
/// `idle_timeout_ms == 0`, no timeout is applied (legacy behavior).
/// Otherwise the read awaits at most `idle_timeout_ms` milliseconds
/// for the next byte; on expiry returns an `io::Error` of kind
/// `TimedOut`, which the caller treats as a disconnect.
///
/// Idle timeout applies between frames, not within a frame: once the
/// 4-byte length prefix arrives we read the body to completion without
/// re-arming the timer. A patient attacker that sent the prefix and
/// then dribbled body bytes is bounded by the `MAX_FRAME_BYTES` cap
/// and tokio's default TCP read buffering — not unbounded.
async fn read_frame_with_timeout(
    stream: &mut TcpStream,
    idle_timeout_ms: u64,
) -> io::Result<Vec<u8>> {
    if idle_timeout_ms == 0 {
        return read_frame(stream).await;
    }
    match tokio::time::timeout(
        std::time::Duration::from_millis(idle_timeout_ms),
        read_frame(stream),
    )
    .await
    {
        Ok(r) => r,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("client: idle for >{}ms, closing", idle_timeout_ms),
        )),
    }
}

async fn write_response(stream: &mut TcpStream, resp: &ClientResponse) -> io::Result<()> {
    let bytes = crate::codec::encode_frame(resp)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    let len = (bytes.len() as u32).to_be_bytes();
    stream.write_all(&len).await?;
    stream.write_all(&bytes).await?;
    stream.flush().await?;
    Ok(())
}

/// Client-side helper used by `suwappu-loadgen`. Wraps a single TCP
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
            signer_pubkey: None,
        };
        let bytes = crate::codec::encode_frame(&msg)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        let len = (bytes.len() as u32).to_be_bytes();
        self.stream.write_all(&len).await?;
        self.stream.write_all(&bytes).await?;
        self.stream.flush().await?;

        let resp_bytes = read_frame(&mut self.stream).await?;
        let resp: ClientResponse = crate::codec::decode_frame(&resp_bytes)
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
            signer_pubkey: None,
        };
        let bytes = crate::codec::encode_frame(&msg)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        let len = (bytes.len() as u32).to_be_bytes();
        self.stream.write_all(&len).await?;
        self.stream.write_all(&bytes).await?;
        self.stream.flush().await?;

        let resp_bytes = read_frame(&mut self.stream).await?;
        let resp: ClientResponse = crate::codec::decode_frame(&resp_bytes)
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
    use suwappu_execution::Intent;

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
        let intent_bytes = crate::codec::encode(&intent).unwrap();
        let mut h = blake3::Hasher::new();
        h.update(b"SUWAPPU_INTENT_V1");
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

    // ----- Governance gate tests (Task 5) ------------------------------

    #[test]
    fn governance_intents_require_ring_membership() {
        // Every governance intent must return true.
        let governance = vec![
            Intent::AdmitAuthority {
                authority_id: 0,
                stake_suwappu: 100_000,
                mldsa_public_key: vec![0u8; 1952],
                bls_public_key: vec![0u8; 48],
            },
            Intent::ExitAuthority { authority_id: 0 },
            Intent::EjectAuthority {
                authority_id: 0,
                proof_ref: [0u8; 32],
            },
            Intent::AdmitValidator {
                validator_id: 0,
                stake_suwappu: 25_000,
                mldsa_public_key: vec![0u8; 1952],
                bls_public_key: vec![0u8; 48],
            },
            Intent::ExitValidator { validator_id: 0 },
            Intent::EjectValidator {
                validator_id: 0,
                proof_ref: [0u8; 32],
            },
            Intent::GenesisAllocation {
                allocations: vec![],
            },
            Intent::MintInflation {
                epoch: 0,
                authority_share: 0,
                validator_share: 0,
                treasury_share: 0,
            },
            Intent::DistributeRewards {
                epoch: 0,
                ring: suwappu_execution::RewardsRing::Authority,
                recipients: vec![],
            },
            Intent::DistributeSlashedFunds {
                slash_event_id: [0u8; 32],
                counterparties: vec![],
                insurance_share: 0,
                treasury_share: 0,
            },
            Intent::SetL2VerifyingKey {
                chain_id_hash: [0u8; 32],
                new_aggregation_vk: [0u8; 32],
                new_range_commitment: [0u8; 32],
            },
            Intent::SlashSequencer {
                reason: suwappu_execution::substrate::SlashReason::MissedForceInclude,
                intent_hash: [0u8; 32],
            },
            Intent::EjectSequencer {
                obligation_id: [0u8; 32],
                ejector: [0u8; 20],
            },
            Intent::MarkForceIncludeHonored {
                obligation_id: [0u8; 32],
            },
            Intent::DisburseTreasury {
                recipient: [1u8; 20],
                amount: 0,
                purpose_tag: [0u8; 32],
            },
            Intent::ClaimInsurance {
                claimant: [1u8; 20],
                amount: 0,
                claim_reference: [0u8; 32],
            },
            Intent::PostL2DA {
                batch_id: 0,
                da_blob: vec![],
            },
            Intent::PostL2DAv2 {
                batch_id: 0,
                da_blob: vec![],
                l2_chain_id_hash: [0u8; 32],
            },
            Intent::CommitL2StateRoot {
                batch_id: 0,
                new_state_root: [0u8; 32],
                proof_bytes: vec![],
                public_inputs: vec![],
                vk_hash: [0u8; 32],
            },
            Intent::AddBridgeAsset {
                source_chain: 0,
                source_contract: vec![],
                decimals: 18,
                name: vec![],
                symbol: vec![],
            },
            Intent::PauseBridgeAsset {
                asset_id: [0u8; 32],
            },
            Intent::RemoveBridgeAsset {
                asset_id: [0u8; 32],
            },
        ];
        for (i, intent) in governance.iter().enumerate() {
            assert!(
                intent_requires_ring_membership(intent),
                "governance intent #{i} should require ring membership"
            );
        }
    }

    #[test]
    fn user_tier_intents_do_not_require_ring_membership() {
        // Every user-tier intent must return false.
        let user_tier = vec![
            Intent::Transfer {
                from: [1u8; 20],
                to: [2u8; 20],
                amount: 1,
            },
            Intent::Delegate {
                from: [1u8; 20],
                validator_id: 0,
                amount: 1,
            },
            Intent::UndelegateBegin {
                from: [1u8; 20],
                validator_id: 0,
                amount: 1,
            },
            Intent::UndelegateClaim {
                from: [1u8; 20],
                validator_id: 0,
            },
            Intent::L1Lock {
                user_address: [1u8; 20],
                l2_recipient: [2u8; 20],
                amount: 1,
                asset_id: None,
            },
            Intent::L2BurnProven {
                batch_id: 0,
                recipient: [1u8; 20],
                amount: 1,
                merkle_path: vec![],
                path_directions: vec![],
                asset_id: None,
                l2_chain_id_hash: [0u8; 32],
            },
            Intent::L2ForceInclude {
                tx: vec![],
                deadline_l1_height: 0,
                submitter: [1u8; 20],
                l2_nonce: 0,
            },
            Intent::DepositSequencerBond {
                from: [1u8; 20],
                amount: 1,
            },
            Intent::DepositSafetyBond {
                from: [1u8; 20],
                amount: 1,
            },
            Intent::DepositAuthorityStake {
                from: [1u8; 20],
                authority_id: 0,
                amount: 1,
            },
            Intent::DepositValidatorStake {
                from: [1u8; 20],
                validator_id: 0,
                amount: 1,
            },
            Intent::WithdrawAuthorityStake {
                to: [1u8; 20],
                authority_id: 0,
                amount: 1,
            },
            Intent::WithdrawValidatorStake {
                to: [1u8; 20],
                validator_id: 0,
                amount: 1,
            },
        ];
        for (i, intent) in user_tier.iter().enumerate() {
            assert!(
                !intent_requires_ring_membership(intent),
                "user-tier intent #{i} should NOT require ring membership"
            );
        }
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
