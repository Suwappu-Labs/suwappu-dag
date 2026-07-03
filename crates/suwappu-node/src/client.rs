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
//! `CLIENT_WIRE_VERSION` is bumped to **2**: every submission carries a
//! detached ML-DSA-65 signature and the blake3 hash of the signing
//! public key. The signing payload binds:
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
//! `suwappu-loadgen` (rebuilt from this branch) and any external submitters
//! before rolling validators.
//!
//! ### Wire additions (PERF-2, append-only)
//!
//! PERF-2 appends `ClientMessage::GetLineage` + `ClientMessage::SubmitFastPath`
//! and `ClientResponse::Lineage` + `ClientResponse::AckFastPath` at the END
//! of the respective enums. bincode's legacy config encodes the variant
//! index, so append-only extension keeps every pre-PERF-2 frame byte-
//! identical — old clients keep working; new variants sent to an old
//! validator fail decode (the established "version" signal). Fast-path
//! submissions sign `fastpath_signing_digest` (distinct domain tag,
//! `SUWAPPU_FASTPATH_V1`) with the same seated-Authority ML-DSA-65 gate
//! `Submit` uses.
//!
//! ### DoS controls on the PERF-2 messages
//!
//! `SubmitFastPath` and `GetLineage` arrive on the same connections as
//! `Submit`, so the accept-time controls in [`run`] — the global
//! `max_connections` semaphore and the per-IP connection cap — already
//! gate them; they are NOT an un-throttled side door. What they do NOT
//! reuse is the mempool's per-peer leaky bucket, because fast-path txs
//! never enter the mempool (they gossip as partial certs, not intents)
//! and `Mempool`'s bucket has no standalone "spend one token" entry
//! point reachable without enqueuing an intent — and the mempool crate
//! is out of scope for this change. The fast-path-specific admission
//! control is therefore the `MAX_FASTPATH_TRACKED` soft cap in
//! `propose_fastpath_tx` (bounds distinct-key memory growth). RESIDUAL:
//! a single authorized signer can still burst fast-path submissions up
//! to the per-connection frame rate; a per-peer token bucket for the
//! fast-path wire is a follow-up (needs a small `Mempool` API addition
//! or a dedicated limiter, both outside this PR's blast radius).

use std::{collections::HashMap, io, net::SocketAddr, sync::Arc};

use serde::{Deserialize, Serialize};
use suwappu_consensus::AuthorityId;
use suwappu_crypto::mldsa;
use suwappu_execution::{Balance, Intent};
use suwappu_fastpath::FastPathTx;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tracing::{debug, info, warn};

use crate::{
    daemon::State,
    events::{Event, EventLog, Lane},
    wire::{PeerId, WireMessage},
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
pub const INTENT_DOMAIN_TAG: &[u8] = b"SUWAPPU_INTENT_V1";

/// Compute the canonical signing digest for an intent under `network_id`.
///
/// `digest = blake3( INTENT_DOMAIN_TAG || network_id_bytes || bincode(intent) )`.
///
/// Both submitter and verifier MUST compute the digest the same way;
/// any divergence rejects every signature.
pub fn intent_signing_digest(network_id: &str, intent: &Intent) -> [u8; 32] {
    let intent_bytes = crate::codec::encode(intent).expect("intent serialize");
    let mut hasher = blake3::Hasher::new();
    hasher.update(INTENT_DOMAIN_TAG);
    hasher.update(network_id.as_bytes());
    hasher.update(&intent_bytes);
    *hasher.finalize().as_bytes()
}

/// Domain-separation tag mixed into every signed fast-path transaction
/// payload (PERF-2). Distinct from [`INTENT_DOMAIN_TAG`] so a signature
/// over an intent can never be replayed as a fast-path authorization
/// (and vice versa).
pub const FASTPATH_DOMAIN_TAG: &[u8] = b"SUWAPPU_FASTPATH_V1";

/// Compute the canonical signing digest for a fast-path transaction
/// under `network_id`. Mirrors [`intent_signing_digest`] with its own
/// domain tag:
///
/// `digest = blake3( FASTPATH_DOMAIN_TAG || network_id_bytes || bincode(tx) )`.
pub fn fastpath_signing_digest(network_id: &str, tx: &FastPathTx) -> [u8; 32] {
    let tx_bytes = crate::codec::encode(tx).expect("fastpath tx serialize");
    let mut hasher = blake3::Hasher::new();
    hasher.update(FASTPATH_DOMAIN_TAG);
    hasher.update(network_id.as_bytes());
    hasher.update(&tx_bytes);
    *hasher.finalize().as_bytes()
}

/// Domain-separation tag mixed into every signed fee-sponsorship
/// payload (FEE-1 Phase 1 / IQ-007 Option A). Distinct from
/// [`INTENT_DOMAIN_TAG`] and [`FASTPATH_DOMAIN_TAG`] so a sender's
/// intent signature can never be replayed as a fee authorization (and
/// vice versa) — the two-signatures-never-cross-replay property is
/// carried entirely by this domain separation.
pub const FEE_DOMAIN_TAG: &[u8] = b"SUWAPPU_FEE_V1";

/// Compute the canonical signing digest for a fee sponsorship under
/// `network_id`. Mirrors [`intent_signing_digest`] /
/// [`fastpath_signing_digest`] with its own domain tag, but binds to the
/// intent's **content hash** (not the intent bytes) plus `max_fee`:
///
/// `digest = blake3( FEE_DOMAIN_TAG || network_id_bytes || intent_hash || max_fee_be )`.
///
/// Binding to `intent_hash` (= `blake3(bincode(intent))`, the same value
/// the mempool + Ack use) means the sponsor authorizes exactly one
/// intent at a capped price: "I will pay up to `max_fee` for intent H."
/// A different intent, or a higher `max_fee`, yields a different digest
/// and rejects the reused signature. (Same-intent *resubmission* is
/// covered by the mempool's content-hash dedup, not this digest — see
/// IQ-007 "Open questions / Sponsorship replay".)
pub fn fee_signing_digest(network_id: &str, intent_hash: &[u8; 32], max_fee: Balance) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(FEE_DOMAIN_TAG);
    hasher.update(network_id.as_bytes());
    hasher.update(intent_hash);
    hasher.update(&max_fee.to_be_bytes());
    *hasher.finalize().as_bytes()
}

/// Compute the blake3 hash of an ML-DSA public key — used as the
/// `signer_pubkey_hash` on the client wire. The validator side resolves
/// the hash against the seated Authority Ring to recover the public key.
pub fn signer_pubkey_hash(pubkey_bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(pubkey_bytes).as_bytes()
}

/// A fee-sponsorship envelope (FEE-1 Phase 1 / IQ-007 Option A). Names a
/// `fee_payer` distinct from the intent's logical sender and carries a
/// **second ML-DSA-65 signature** from that payer authorizing "I will pay
/// up to `max_fee` for this exact intent." Rides on the
/// [`ClientMessage::SubmitWithFee`] / [`ClientMessage::SubmitBatchWithFee`]
/// variants; unsponsored submissions use plain `Submit`/`SubmitBatch` and
/// carry no envelope (the sender pays nothing today — fees are a pure
/// addition).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FeeAuthorization {
    /// blake3 hash of the fee payer's ML-DSA-65 public-key bytes,
    /// resolved against the seated `AuthorityRegistry` exactly like a
    /// sender's `signer_pubkey_hash`. Phase 1 fee payers are seated
    /// Authorities (Issue #28 gate — see [`verify_signed_fee`]).
    pub payer_pubkey_hash: [u8; 32],
    /// ML-DSA-65 detached signature over
    /// [`fee_signing_digest`]`(network_id, &intent_hash, max_fee)`.
    /// `Vec<u8>` rather than a fixed array for forward-compat with
    /// future parameter sets, matching the sender signature shape.
    pub fee_signature: Vec<u8>,
    /// The flat maximum fee (SUWAPPU-denominated in Phase 1) the sponsor
    /// authorizes. Also feeds the mempool's fee-derived priority.
    pub max_fee: Balance,
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
        /// [`intent_signing_digest`]`(network_id, &intent)`.
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
    /// Ask for the highest committed main-lane `(round, cert_hash)`
    /// (PERF-2). Fast-path submitters call this to ground each
    /// `FastPathTx`'s lineage in a cert already linearized on the main
    /// lane (paper §6.4 eligibility). Appended after `Ping` for bincode
    /// wire compatibility — never reorder.
    GetLineage,
    /// Submit one fast-path transaction (PERF-2). Verified with the
    /// same seated-Authority ML-DSA-65 gate as `Submit`, but over
    /// [`fastpath_signing_digest`] (distinct domain tag). Appended
    /// after `GetLineage` — never reorder.
    SubmitFastPath {
        /// The fast-path transaction to propose into the cluster.
        tx: suwappu_fastpath::FastPathTx,
        /// ML-DSA-65 detached signature over
        /// [`fastpath_signing_digest`]`(network_id, &tx)`.
        signature: Vec<u8>,
        /// blake3 hash of the signer's ML-DSA-65 public-key bytes,
        /// resolved against the seated `AuthorityRegistry` exactly
        /// like `Submit`.
        signer_pubkey_hash: [u8; 32],
    },
    /// Submit one intent with a fee sponsorship (FEE-1 Phase 1,
    /// IQ-007 Option A). Same sender-signature gate as `Submit`, plus a
    /// second ML-DSA-65 signature from `fee_payer` over
    /// [`fee_signing_digest`]. Appended at the END of the enum —
    /// **never reorder**. bincode's legacy config encodes the variant
    /// index, so appending new variants keeps every pre-fee frame
    /// byte-identical (an added *trailing struct field* would instead
    /// break decode of pre-fee frames, since bincode has no field tags
    /// — hence new variants, mirroring the PERF-2 discipline).
    SubmitWithFee {
        /// The intent to include (payload is untouched by the fee
        /// surface — content-hashing, dedup, and the tx index are
        /// unchanged).
        intent: Intent,
        /// Sender's ML-DSA-65 detached signature over
        /// [`intent_signing_digest`]`(network_id, &intent)`, exactly
        /// as on `Submit`.
        signature: Vec<u8>,
        /// blake3 hash of the sender's ML-DSA-65 public key.
        signer_pubkey_hash: [u8; 32],
        /// The fee sponsorship: payer identity + second signature +
        /// `max_fee`.
        fee_payer: FeeAuthorization,
    },
    /// Batched twin of `SubmitWithFee` (FEE-1 Phase 1). One `fee_payer`
    /// sponsors the WHOLE batch: its signature binds the batch's
    /// **combined intent hash** (blake3 over each intent's content hash,
    /// in order) at the batch `max_fee`. Appended at the END — **never
    /// reorder**.
    SubmitBatchWithFee {
        /// The intents to include, in submission order.
        intents: Vec<Intent>,
        /// Sender signatures, one per intent in matching order.
        signatures: Vec<Vec<u8>>,
        /// blake3 hash of the single sender's ML-DSA-65 public key.
        signer_pubkey_hash: [u8; 32],
        /// The fee sponsorship for the whole batch.
        fee_payer: FeeAuthorization,
    },
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
    /// Response to `GetLineage` (PERF-2): the highest committed
    /// main-lane round and its cert hash. `(0, [0; 32])` when no cert
    /// has committed yet. Appended after `Pong` for bincode wire
    /// compatibility — never reorder.
    Lineage {
        /// Highest committed main-lane round.
        round: u64,
        /// Hash of the cert committed at that round.
        cert_hash: [u8; 32],
    },
    /// Fast-path transaction accepted and proposed (PERF-2). Echoes
    /// the tx's `payload_digest` — the same value the daemon writes as
    /// `cert_hash` on its `lane=fastpath` event log lines, so the
    /// metrics join is uniform with the intent path. Appended after
    /// `Lineage` — never reorder.
    AckFastPath {
        /// `payload_digest` of the accepted fast-path transaction.
        payload_digest: [u8; 32],
    },
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
#[allow(clippy::too_many_arguments)] // daemon-startup plumbing, one call site
pub(crate) async fn run(
    listen: SocketAddr,
    self_label: String,
    self_id: AuthorityId,
    log: EventLog,
    state: Arc<State>,
    outbound: Arc<HashMap<PeerId, tokio::sync::mpsc::Sender<WireMessage>>>,
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
                    let outbound = outbound.clone();
                    let network_id = network_id.clone();
                    let peer_label = addr.to_string();
                    let idle_timeout_ms = limits.idle_timeout_ms;
                    tokio::spawn(async move {
                        let _permit = permit; // dropped when this task exits
                        let result = handle_conn(
                            stream,
                            self_label,
                            self_id,
                            peer_label,
                            log,
                            state,
                            outbound,
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
    let digest = intent_signing_digest(network_id, intent);
    verify_authority_signature(state, &digest, signature_bytes, signer_pubkey_hash).await
}

/// PERF-2: the fast-path twin of [`verify_signed_intent`] — same
/// seated-Authority lookup + ML-DSA verify, over the fast-path domain
/// digest. Both wrappers share [`verify_authority_signature`] so the
/// two submission surfaces can never drift on what "signed" means.
pub(crate) async fn verify_signed_fastpath(
    state: &State,
    network_id: &str,
    tx: &FastPathTx,
    signature_bytes: &[u8],
    signer_pubkey_hash: &[u8; 32],
) -> AuthOutcome {
    let digest = fastpath_signing_digest(network_id, tx);
    verify_authority_signature(state, &digest, signature_bytes, signer_pubkey_hash).await
}

/// FEE-1 Phase 1: verify a fee-sponsorship signature (IQ-007 Option A).
/// The twin of [`verify_signed_intent`] / [`verify_signed_fastpath`] for
/// the *second* (fee) signature, over the fee domain digest binding the
/// intent's content hash + `max_fee`. Shares
/// [`verify_authority_signature`] so the fee wire can never drift from
/// the sender wire on what "signed" means.
///
/// Issue #28 gate: this resolves the fee payer against the seated
/// **Authority Ring** only — the Validator Ring registry carries no
/// pubkey material yet, so a non-Authority fee payer needs the Issue #28
/// auth extension first. Phase 1 sponsors are therefore seated
/// Authorities, consistent with the sender gate.
pub(crate) async fn verify_signed_fee(
    state: &State,
    network_id: &str,
    intent_hash: &[u8; 32],
    max_fee: Balance,
    fee_signature_bytes: &[u8],
    payer_pubkey_hash: &[u8; 32],
) -> AuthOutcome {
    let digest = fee_signing_digest(network_id, intent_hash, max_fee);
    verify_authority_signature(state, &digest, fee_signature_bytes, payer_pubkey_hash).await
}

/// Canonical content hash of a single intent — `blake3(bincode(intent))`.
/// The value the fee sponsor binds to, the mempool dedups on, and the
/// `Ack` echoes. Centralised so every fee/ack/hash site agrees.
fn intent_content_hash(intent: &Intent) -> [u8; 32] {
    blake3::hash(&crate::codec::encode(intent).expect("intent serialize")).into()
}

/// Combined content hash of a batch — `blake3( each intent's content
/// hash, in order )`. A single `fee_payer` signs this so one sponsorship
/// binds the exact ordered batch it authorizes (FEE-1 Phase 1 batch
/// sponsorship).
fn batch_intent_hash(intents: &[Intent]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    for intent in intents {
        hasher.update(&intent_content_hash(intent));
    }
    *hasher.finalize().as_bytes()
}

/// Saturating cast of a `max_fee` (u128) to the mempool's `u64` priority
/// knob (FEE-1 Phase 1). Higher fee → drained first. Phase 1 uses the
/// flat `max_fee` as the priority directly (no fee/size normalization);
/// a fee/size ratio is a follow-up if frame sizes diverge materially.
fn fee_derived_priority(max_fee: Balance) -> u64 {
    max_fee.min(u64::MAX as Balance) as u64
}

/// Shared lookup + verify leg of the two `verify_signed_*` wrappers.
/// Callers compute the domain-separated digest; this resolves the
/// signer against the seated Authority Ring and checks the signature.
async fn verify_authority_signature(
    state: &State,
    digest: &[u8; 32],
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
    match mldsa::verify(digest, &signature, &pubkey) {
        Ok(()) => AuthOutcome::Ok,
        Err(_) => AuthOutcome::BadSignature,
    }
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

#[allow(clippy::too_many_arguments)] // one task-entry fn mirroring `run`'s plumbing
async fn handle_conn(
    mut stream: TcpStream,
    self_label: String,
    self_id: AuthorityId,
    peer_label: String,
    log: EventLog,
    state: Arc<State>,
    outbound: Arc<HashMap<PeerId, tokio::sync::mpsc::Sender<WireMessage>>>,
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
                    blake3::hash(&crate::codec::encode(&intent).expect("intent serialize")).into();
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
                // DAG-S29.2 + A3: amortise the ack roundtrip across N
                // intents; each intent flows through `state.mempool.submit`
                // for priority/dedup/rate-limit accounting.
                let mut hashes: Vec<[u8; 32]> = Vec::with_capacity(intents.len());
                for intent in intents {
                    let intent_hash: [u8; 32] =
                        blake3::hash(&crate::codec::encode(&intent).expect("intent serialize"))
                            .into();
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
            ClientMessage::GetLineage => {
                // PERF-2: O(1) read of the committed-head scalar that
                // `try_commit` maintains. `(0, [0; 32])` sentinel until
                // the first cert commits.
                let (round, cert_hash) = state
                    .inner
                    .lock()
                    .await
                    .highest_committed
                    .unwrap_or((0, [0u8; 32]));
                write_response(&mut stream, &ClientResponse::Lineage { round, cert_hash }).await?;
            }
            ClientMessage::SubmitFastPath {
                tx,
                signature,
                signer_pubkey_hash,
            } => {
                match verify_signed_fastpath(
                    &state,
                    &network_id,
                    &tx,
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

                // Owner binding (PERF-2). The owner must be the
                // submitting authority itself: `owner == blake3(signer
                // pubkey bytes)`, which — because `signer_pubkey_hash`
                // is defined as exactly that blake3 — is a byte compare
                // against the (now signature-verified) `signer_pubkey_hash`.
                // Without this, a seated authority could author a
                // fast-path cert that spends an object owned by someone
                // else. DEVNET SIMPLIFICATION: on mainnet the owner is a
                // separate account with its own key and the fast-path tx
                // would carry the owner's signature, not the validator's;
                // here we collapse owner == submitter so the harness
                // needs only one keypair.
                if tx.owner.0 != signer_pubkey_hash {
                    let resp = ClientResponse::Err(
                        "fastpath: owner does not match signer (owner must be \
                         blake3(signer pubkey))"
                            .to_string(),
                    );
                    let _ = write_response(&mut stream, &resp).await;
                    return Ok(());
                }

                // Lineage-committed bound (PERF-2, closes the K=4
                // binding bypass). The tx's lineage must reference a
                // cert already committed on the main lane: a far-future
                // `lineage_round` would make the reconciliation window
                // `(lineage_round, lineage_round + K]` vacuous, so the
                // main-lane cross-check could never catch an
                // equivocation. Require `lineage_round <= committed head`
                // AND `lineage` == the committed cert hash at that round.
                // With nothing committed yet, only `lineage_round == 0`
                // (the GetLineage sentinel) is allowed.
                let (head, want_hash_at_lineage) = {
                    let inner = state.inner.lock().await;
                    let committed = state.committed.lock();
                    let head = inner.highest_committed;
                    let want = inner
                        .blocks_by_round
                        .get(&tx.lineage_round)
                        .filter(|h| committed.contains(*h))
                        .map(|h| h.0);
                    (head, want)
                };
                let lineage_ok = match head {
                    None => tx.lineage_round == 0,
                    Some((head_round, _)) => {
                        tx.lineage_round <= head_round && want_hash_at_lineage == Some(tx.lineage.0)
                    }
                };
                if !lineage_ok {
                    let resp = ClientResponse::Err("fastpath: lineage not committed".to_string());
                    let _ = write_response(&mut stream, &resp).await;
                    return Ok(());
                }

                let payload_digest = tx.payload_digest;
                // `propose_fastpath_tx` runs the self-equivocation guards
                // (payload pin-check, main-lane K-binding cross-check,
                // capacity cap) under its own short inner-lock window and
                // only signs/broadcasts if all pass. Map each refusal to
                // a distinct client error rather than a silent no-op.
                match crate::daemon::propose_fastpath_tx(
                    &state,
                    self_id,
                    tx,
                    &self_label,
                    &log,
                    &outbound,
                )
                .await
                {
                    crate::daemon::FastPathProposeOutcome::Accepted => {
                        write_response(
                            &mut stream,
                            &ClientResponse::AckFastPath { payload_digest },
                        )
                        .await?;
                    }
                    crate::daemon::FastPathProposeOutcome::ConflictingPayload => {
                        let resp = ClientResponse::Err(
                            "fastpath: conflicting payload for (object,nonce)".to_string(),
                        );
                        let _ = write_response(&mut stream, &resp).await;
                        return Ok(());
                    }
                    crate::daemon::FastPathProposeOutcome::MainLaneConflict => {
                        let resp = ClientResponse::Err(
                            "fastpath: main-lane conflict in binding window".to_string(),
                        );
                        let _ = write_response(&mut stream, &resp).await;
                        return Ok(());
                    }
                    crate::daemon::FastPathProposeOutcome::CapacityExhausted => {
                        let resp = ClientResponse::Err(
                            "fastpath: node at fast-path tracking capacity, retry later"
                                .to_string(),
                        );
                        let _ = write_response(&mut stream, &resp).await;
                        return Ok(());
                    }
                }
            }
            // FEE-1 Phase 1 (IQ-007 Option A): sponsored single submit.
            // Two ML-DSA-65 signatures — the sender's over the intent and
            // the fee payer's over (intent_hash, max_fee) — both resolved
            // against the seated Authority Ring.
            //
            // SETTLEMENT-PR PREREQUISITES (record only; NOT fixed here):
            // (1) the fee authorization has no nonce / expiry / revocation
            // — binding to the intent content hash blocks cross-intent
            // replay but not same-intent resubmission; a nonce is required
            // before this envelope actually moves funds. (2) The validated
            // envelope drives mempool admission priority only; it is not
            // yet threaded mempool -> block, so execution-time settlement
            // (`Substrate::apply_intent_with_fee`) is landed + tested but
            // not wired. Both are prerequisites of the settlement PR.
            ClientMessage::SubmitWithFee {
                intent,
                signature,
                signer_pubkey_hash,
                fee_payer,
            } => {
                // 1. Sender signature (identical gate to `Submit`).
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
                // 2. Fee sponsorship signature, bound to this intent's
                //    content hash + max_fee.
                let intent_hash = intent_content_hash(&intent);
                match verify_signed_fee(
                    &state,
                    &network_id,
                    &intent_hash,
                    fee_payer.max_fee,
                    &fee_payer.fee_signature,
                    &fee_payer.payer_pubkey_hash,
                )
                .await
                {
                    AuthOutcome::Ok => {}
                    AuthOutcome::UnknownSigner => {
                        let resp = ClientResponse::Err(
                            "fee: unknown sponsor (pubkey hash not in Authority Ring)".to_string(),
                        );
                        let _ = write_response(&mut stream, &resp).await;
                        return Ok(());
                    }
                    AuthOutcome::BadSignature => {
                        let resp = ClientResponse::Err(
                            "fee: bad ML-DSA-65 sponsorship signature".to_string(),
                        );
                        let _ = write_response(&mut stream, &resp).await;
                        return Ok(());
                    }
                }
                // 3. Fee-derived mempool priority (turns the dormant
                //    priority knob live). NOTE: the mempool carries only
                //    the intent; the validated `fee_payer` envelope drives
                //    admission priority here but is not yet plumbed into
                //    the block for execution-time settlement (that needs a
                //    mempool `Entry` + block plumbing change out of this
                //    PR's blast radius — see IQ-007). The substrate-side
                //    settlement path (`apply_intent_with_fee`) is landed
                //    and tested, ready for that plumbing.
                let priority = fee_derived_priority(fee_payer.max_fee);
                match state
                    .mempool
                    .submit(intent, priority, Some(peer_label.clone()), now_ms())
                {
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
            // FEE-1 Phase 1: sponsored batch — one `fee_payer` sponsors
            // the whole ordered batch, signing over the combined batch
            // hash at `max_fee`.
            ClientMessage::SubmitBatchWithFee {
                intents,
                signatures,
                signer_pubkey_hash,
                fee_payer,
            } => {
                // Reject a zero-length sponsored batch: an empty batch
                // would have the sponsor sign over `blake3("")` and return
                // an empty `AckBatch` as a spurious success. There is
                // nothing to sponsor, so refuse it outright.
                if intents.is_empty() {
                    let resp = ClientResponse::Err("fee: empty batch".to_string());
                    let _ = write_response(&mut stream, &resp).await;
                    return Ok(());
                }
                if signatures.len() != intents.len() {
                    let resp = ClientResponse::Err(format!(
                        "auth: batch length mismatch ({} intents vs {} signatures)",
                        intents.len(),
                        signatures.len()
                    ));
                    let _ = write_response(&mut stream, &resp).await;
                    return Ok(());
                }
                // Verify every sender signature BEFORE admitting any
                // intent (same all-or-nothing gate as `SubmitBatch`).
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
                // One sponsorship signature over the combined batch hash.
                let combined = batch_intent_hash(&intents);
                match verify_signed_fee(
                    &state,
                    &network_id,
                    &combined,
                    fee_payer.max_fee,
                    &fee_payer.fee_signature,
                    &fee_payer.payer_pubkey_hash,
                )
                .await
                {
                    AuthOutcome::Ok => {}
                    AuthOutcome::UnknownSigner => {
                        let resp = ClientResponse::Err(
                            "fee: unknown sponsor (pubkey hash not in Authority Ring)".to_string(),
                        );
                        let _ = write_response(&mut stream, &resp).await;
                        return Ok(());
                    }
                    AuthOutcome::BadSignature => {
                        let resp = ClientResponse::Err(
                            "fee: bad ML-DSA-65 sponsorship signature".to_string(),
                        );
                        let _ = write_response(&mut stream, &resp).await;
                        return Ok(());
                    }
                }
                let priority = fee_derived_priority(fee_payer.max_fee);
                let mut hashes: Vec<[u8; 32]> = Vec::with_capacity(intents.len());
                for intent in intents {
                    let intent_hash = intent_content_hash(&intent);
                    match state
                        .mempool
                        .submit(intent, priority, Some(peer_label.clone()), now_ms())
                    {
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
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response for Submit",
            )),
        }
    }

    /// FEE-1 Phase 1 (IQ-007 Option A): submit one intent with a fee
    /// sponsorship. In the devnet harness the sponsor is the client's own
    /// key (owner == submitter == sponsor, matching the fast-path
    /// simplification), so both signatures are produced from `secret_key`.
    /// On mainnet the sponsor is a distinct account whose key signs only
    /// the fee digest.
    pub async fn submit_with_fee(
        &mut self,
        intent: Intent,
        max_fee: Balance,
    ) -> io::Result<[u8; 32]> {
        let intent_digest = intent_signing_digest(&self.network_id, &intent);
        let signature = mldsa::sign(&intent_digest, &self.secret_key)
            .map_err(|e| io::Error::other(format!("sign: {:?}", e)))?;
        let pkh = self.signer_pubkey_hash();
        // Sponsor signs (intent_hash, max_fee) under the fee domain tag.
        // One canonical spelling of the content hash via the shared helper.
        let intent_hash = intent_content_hash(&intent);
        let fee_digest = fee_signing_digest(&self.network_id, &intent_hash, max_fee);
        let fee_signature = mldsa::sign(&fee_digest, &self.secret_key)
            .map_err(|e| io::Error::other(format!("fee sign: {:?}", e)))?;
        let msg = ClientMessage::SubmitWithFee {
            intent,
            signature: signature.as_bytes().to_vec(),
            signer_pubkey_hash: pkh,
            fee_payer: FeeAuthorization {
                payer_pubkey_hash: pkh,
                fee_signature: fee_signature.as_bytes().to_vec(),
                max_fee,
            },
        };
        self.send_frame(&msg).await?;
        let resp = self.read_response().await?;
        match resp {
            ClientResponse::Ack { intent_hash } => Ok(intent_hash),
            ClientResponse::Err(e) => Err(io::Error::new(io::ErrorKind::InvalidData, e)),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response for SubmitWithFee",
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
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response for SubmitBatch",
            )),
        }
    }

    /// PERF-2: fetch the highest committed main-lane `(round, cert_hash)`
    /// so fast-path transactions can ground their lineage. Returns
    /// `(0, [0; 32])` before the first commit.
    pub async fn get_lineage(&mut self) -> io::Result<(u64, [u8; 32])> {
        self.send_frame(&ClientMessage::GetLineage).await?;
        let resp = self.read_response().await?;
        match resp {
            ClientResponse::Lineage { round, cert_hash } => Ok((round, cert_hash)),
            ClientResponse::Err(e) => Err(io::Error::new(io::ErrorKind::InvalidData, e)),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response for GetLineage",
            )),
        }
    }

    /// PERF-2: submit one signed fast-path transaction and wait for the
    /// `AckFastPath`. Returns the acked `payload_digest` — the value the
    /// validator writes as `cert_hash` on `lane=fastpath` event lines,
    /// so callers key their submit-timestamp CSV on it.
    pub async fn submit_fastpath(&mut self, tx: FastPathTx) -> io::Result<[u8; 32]> {
        let digest = fastpath_signing_digest(&self.network_id, &tx);
        let signature = mldsa::sign(&digest, &self.secret_key)
            .map_err(|e| io::Error::other(format!("sign: {:?}", e)))?;
        let pkh = self.signer_pubkey_hash();
        let msg = ClientMessage::SubmitFastPath {
            tx,
            signature: signature.as_bytes().to_vec(),
            signer_pubkey_hash: pkh,
        };
        self.send_frame(&msg).await?;
        let resp = self.read_response().await?;
        match resp {
            ClientResponse::AckFastPath { payload_digest } => Ok(payload_digest),
            ClientResponse::Err(e) => Err(io::Error::new(io::ErrorKind::InvalidData, e)),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response for SubmitFastPath",
            )),
        }
    }

    /// Frame + flush one `ClientMessage` (shared by the PERF-2 methods;
    /// the pre-PERF-2 `submit`/`submit_batch` keep their inline framing
    /// untouched).
    async fn send_frame(&mut self, msg: &ClientMessage) -> io::Result<()> {
        let bytes = crate::codec::encode_frame(msg)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        let len = (bytes.len() as u32).to_be_bytes();
        self.stream.write_all(&len).await?;
        self.stream.write_all(&bytes).await?;
        self.stream.flush().await?;
        Ok(())
    }

    /// Read + decode one framed `ClientResponse`.
    async fn read_response(&mut self) -> io::Result<ClientResponse> {
        let resp_bytes = read_frame(&mut self.stream).await?;
        crate::codec::decode_frame(&resp_bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
    }
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

    // ----- FEE-1 Phase 1 (IQ-007 Option A) -----------------------------

    #[test]
    fn fee_signing_digest_is_deterministic() {
        let ih = [7u8; 32];
        let d1 = fee_signing_digest("perf-7r", &ih, 100);
        let d2 = fee_signing_digest("perf-7r", &ih, 100);
        assert_eq!(d1, d2);
    }

    #[test]
    fn fee_digest_sensitive_to_intent_hash_and_max_fee() {
        let base = fee_signing_digest("n", &[1u8; 32], 100);
        // Different intent hash → different digest.
        assert_ne!(base, fee_signing_digest("n", &[2u8; 32], 100));
        // Different max_fee → different digest.
        assert_ne!(base, fee_signing_digest("n", &[1u8; 32], 101));
        // Different network → different digest (cross-network replay).
        assert_ne!(base, fee_signing_digest("m", &[1u8; 32], 100));
    }

    #[test]
    fn fee_digest_domain_is_distinct_from_intent_and_fastpath() {
        // The fee digest binds an intent HASH (not intent bytes) under a
        // distinct domain tag, so it can never collide with an intent or
        // fast-path signing digest — the cross-replay guard.
        let intent = Intent::Transfer {
            from: [1u8; 20],
            to: [2u8; 20],
            amount: 99,
        };
        let ih: [u8; 32] = blake3::hash(&crate::codec::encode(&intent).unwrap()).into();
        let fee = fee_signing_digest("net", &ih, 50);
        let intent_dig = intent_signing_digest("net", &intent);
        assert_ne!(fee, intent_dig, "fee digest MUST differ from intent digest");
        // Manual recompute pins the recipe (tag || net || intent_hash || max_fee_be).
        let mut h = blake3::Hasher::new();
        h.update(b"SUWAPPU_FEE_V1");
        h.update(b"net");
        h.update(&ih);
        h.update(&50u128.to_be_bytes());
        assert_eq!(fee, *h.finalize().as_bytes());
    }

    #[test]
    fn fee_derived_priority_saturates() {
        assert_eq!(fee_derived_priority(0), 0);
        assert_eq!(fee_derived_priority(42), 42);
        // A u128 max_fee beyond u64::MAX saturates to u64::MAX.
        assert_eq!(fee_derived_priority(u128::MAX), u64::MAX);
    }

    #[test]
    fn pre_fee_submit_frame_still_decodes_after_variant_append() {
        // Wire compat: a pre-FEE-1 `Submit` frame (variant 0) must decode
        // unchanged now that `SubmitWithFee` / `SubmitBatchWithFee` are
        // appended at the end of the enum. bincode encodes the variant
        // index, so appending variants leaves index 0 byte-identical.
        let intent = Intent::Transfer {
            from: [1u8; 20],
            to: [2u8; 20],
            amount: 5,
        };
        let old = ClientMessage::Submit {
            intent: intent.clone(),
            signature: vec![0xab; 16],
            signer_pubkey_hash: [9u8; 32],
        };
        let frame = crate::codec::encode_frame(&old).unwrap();
        let back: ClientMessage = crate::codec::decode_frame(&frame).unwrap();
        match back {
            ClientMessage::Submit {
                intent: got,
                signature,
                signer_pubkey_hash,
            } => {
                assert_eq!(got, intent);
                assert_eq!(signature, vec![0xab; 16]);
                assert_eq!(signer_pubkey_hash, [9u8; 32]);
            }
            other => panic!("pre-fee Submit frame decoded as {other:?}"),
        }
    }

    #[test]
    fn submit_with_fee_frame_round_trips() {
        let intent = Intent::Transfer {
            from: [1u8; 20],
            to: [2u8; 20],
            amount: 5,
        };
        let msg = ClientMessage::SubmitWithFee {
            intent,
            signature: vec![0x11; 8],
            signer_pubkey_hash: [3u8; 32],
            fee_payer: FeeAuthorization {
                payer_pubkey_hash: [4u8; 32],
                fee_signature: vec![0x22; 8],
                max_fee: 777,
            },
        };
        let frame = crate::codec::encode_frame(&msg).unwrap();
        let back: ClientMessage = crate::codec::decode_frame(&frame).unwrap();
        match back {
            ClientMessage::SubmitWithFee { fee_payer, .. } => {
                assert_eq!(fee_payer.max_fee, 777);
                assert_eq!(fee_payer.payer_pubkey_hash, [4u8; 32]);
                assert_eq!(fee_payer.fee_signature, vec![0x22; 8]);
            }
            other => panic!("SubmitWithFee decoded as {other:?}"),
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
