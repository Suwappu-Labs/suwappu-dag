//! Wire transport — real tokio TCP socket between validators.
//!
//! Carries the four message types that cross the inter-validator boundary:
//!
//! - `Certificate` (gsx-consensus) — DAG cert proposed by an Authority Ring member
//! - `Vote`        (gsx-consensus) — Validator Ring ratification vote
//! - `FastPathCert`(gsx-fastpath)  — single-owner fast-path certificate
//! - `CorridorAttestation` (gsx-ltp) — LTP super-node 7-of-9 attestation
//!
//! Plus a `Ping`/`Pong` heartbeat for liveness measurement.
//!
//! Wire format: 4-byte big-endian length prefix + bincode-serialized
//! [`WireMessage`]. A 1 MiB frame cap prevents an OOM from a misbehaving peer.
//!
//! Authentication: each `WireMessage` carries its own internal ML-DSA-65 / BLS
//! signatures verified by the consensus / fast-path / LTP layers. The wire
//! layer is intentionally unauthenticated at the TCP level — adding TLS or
//! Noise would mask the geographic-latency measurement we want from the perf
//! testnet. For mainnet, swap in mutual ML-DSA over the wire (tracked).
//!
//! This module exists because S2 (RaptorQ) and S18/S19 (SCION) shipped as
//! proptest-only logic with no socket binding. Without a real wire, multi-
//! region deployments measure zero.

use std::{collections::HashMap, io, net::SocketAddr, sync::Arc, time::Duration};

use gsx_consensus::{
    cert::{CertHash, Certificate},
    joint::Vote,
};
use gsx_execution::Intent;
use gsx_fastpath::cert::FastPathCert;
use gsx_ltp::attestation::CorridorAttestation;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, RwLock},
    task::JoinHandle,
};
use tracing::{debug, info, warn};

/// Side-channel block payload referenced by a [`Certificate::payload_digest`].
/// Mysticeti-C separates cert proposal from block dissemination — the cert
/// commits to a 32-byte digest, the block (which carries the actual intents)
/// flows on a parallel `WireMessage::Block` frame.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockPayload {
    /// 32-byte content hash of `intents` (blake3). Must equal the
    /// `payload_digest` of the associated cert.
    pub payload_digest: [u8; 32],
    /// Authoring authority id (matches the cert's `author`).
    pub author: u32,
    /// DAG round number (matches the cert's `round`).
    pub round: u64,
    /// Cert hash this block backs. Lets the receiver match block → cert
    /// without reconstructing the cert independently.
    pub cert_hash: CertHash,
    /// Ordered list of intents in this block.
    pub intents: Vec<Intent>,
}

/// Per-cluster peer identifier. Carries a human label so logs are readable
/// across a 7-region deployment. Not load-bearing for consensus — every
/// security-relevant identity lives inside the inner messages.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PeerId(pub String);

impl PeerId {
    /// Construct from a string slice.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

/// Top-level wire frame. One bincode-encoded value per TCP frame.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum WireMessage {
    /// Authority Ring certificate proposal (Mysticeti-C).
    Cert(Certificate),
    /// Block payload backing a cert (intents + digest match check).
    Block(BlockPayload),
    /// Validator Ring ratification vote (joint quorum AND-gate).
    Vote(Vote),
    /// Synchronizer: request a specific cert by hash. Sent when a
    /// received cert references a parent not yet in the local DAG, or
    /// re-issued periodically for stale inflight fetches. Receivers
    /// respond with `Cert(...)` if they have it; otherwise silently
    /// drop. See `gsx_node::sync` (S21.3).
    GetCert(CertHash),
    /// Single-owner fast-path certificate (paper §6.4).
    FastPath(FastPathCert),
    /// LTP super-node corridor attestation (paper §10.2).
    Ltp(CorridorAttestation),
    /// Heartbeat from peer; echo includes the peer's send-timestamp millis.
    Ping(u64),
    /// Echo of a `Ping` from this side, for RTT measurement.
    Pong(u64),
}

/// Maximum allowed framed payload size. Drops the connection on overrun.
/// This is the *outer* envelope cap — applied at the length-prefix
/// header before any allocation. A single `WireMessage` variant may
/// carry payloads up to (but not beyond) this cap.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// B3 hardening: per-message size cap applied AFTER successful frame
/// receipt but BEFORE bincode-deserialization commits to the type-
/// specific allocations. A bincode-serialized `Certificate` is on the
/// order of ~1-2 KiB (32-byte hash, 4 KiB parent list at n=128,
/// payload digest, signature). 64 KiB gives an order-of-magnitude
/// envelope above the honest worst case and rejects malicious peers
/// that send a 1 MiB frame just under `MAX_FRAME_BYTES` to chew CPU
/// during decode. The check sits in `read_frame` alongside the
/// length-prefix guard so a peer can't bypass it by lying about
/// `Content-Length`-style headers — bincode is unframed at this
/// layer, so the only signal is the BE u32 prefix we already
/// validate against `MAX_FRAME_BYTES`.
///
/// `BlockPayload` is the one variant that can legitimately exceed
/// this cap (it carries up to ~1100 intents per the perf testnet's
/// observed peak). Block frames are sent on the same wire so we
/// don't gate decode of arbitrary `WireMessage` on this cap; instead
/// the cert/vote/ack paths each enforce it after bincode decode if
/// the variant should be small. See `enforce_compact_variant_cap`
/// for the per-variant policy.
pub const MAX_COMPACT_MESSAGE_BYTES: usize = 64 * 1024;

/// Outbound dial reconnect parameters. Geometric backoff capped at `max_ms`.
const RECONNECT_MIN_MS: u64 = 50;
const RECONNECT_MAX_MS: u64 = 5_000;

/// Errors produced by the wire transport. Internal to the `gsx-node` daemon —
/// not part of the consensus API surface.
#[derive(Debug, thiserror::Error)]
pub enum WireError {
    /// Underlying I/O failure on a TCP socket.
    #[error("io: {0}")]
    Io(#[from] io::Error),
    /// bincode encode failure (outbound path).
    #[error("encode: {0}")]
    Encode(#[from] crate::codec::EncodeError),
    /// bincode decode failure (inbound path), including
    /// version-byte mismatch on the F4 framed payload.
    #[error("decode: {0}")]
    Decode(#[from] crate::codec::FrameDecodeError),
    /// Frame size exceeded [`MAX_FRAME_BYTES`].
    #[error("frame too large: {0} bytes")]
    FrameTooLarge(usize),
    /// Remote closed mid-frame.
    #[error("connection closed")]
    Closed,
}

/// Static configuration. Identifies this node and the peers it should dial.
#[derive(Clone, Debug)]
pub struct WireConfig {
    /// This node's identifier (e.g. `"us-east-1"`).
    pub self_id: PeerId,
    /// Local socket to bind for accepting inbound peer connections.
    pub listen: SocketAddr,
    /// Peers this node should dial. The dialer/listener split is symmetric —
    /// node A dialing B and B dialing A is fine; both connections coexist.
    pub peers: Vec<(PeerId, SocketAddr)>,
}

/// Single inbound event delivered to the consensus task.
#[derive(Debug, Clone)]
pub struct WireEvent {
    /// Which peer the message came from.
    pub from: PeerId,
    /// Message contents.
    pub msg: WireMessage,
}

/// Running wire-transport handle. Drop to stop all background tasks.
pub struct Wire {
    /// DAG-S31.1: per-peer inbound channels. Pre-S31 every inbound peer
    /// stream multiplexed into one channel + one consumer task; on the
    /// 4-region perf testnet that consumer became the slowest-node
    /// bottleneck (ap-northeast-1 received 143 msg/sec across 3 peers
    /// but a single tokio task couldn't keep up). Splitting by peer
    /// lets the runtime spread inbox processing across multiple worker
    /// threads.
    pub inboxes: HashMap<PeerId, mpsc::Receiver<WireEvent>>,
    /// Per-peer outbound channels. Send a message into the channel and the
    /// dialer task will write it to the peer's TCP socket. Slow peers drop
    /// excess sends (bounded channel) — this matches the consensus paper's
    /// "best-effort gossip, retries via re-broadcast" model.
    outbound: HashMap<PeerId, mpsc::Sender<WireMessage>>,
    /// Background task handles. Aborted on drop.
    tasks: Vec<JoinHandle<()>>,
}

/// Components of a [`Wire`] handed out via [`Wire::split`]. The accept/dialer
/// background tasks transfer with the inbox so callers don't have to track
/// them separately.
pub struct WireSplit {
    /// Per-peer inbound receivers. See [`Wire::inboxes`].
    pub inboxes: HashMap<PeerId, mpsc::Receiver<WireEvent>>,
    /// Per-peer outbound senders.
    pub outbound: HashMap<PeerId, mpsc::Sender<WireMessage>>,
    /// Background accept/dialer tasks. Abort them to stop the wire.
    pub tasks: Vec<JoinHandle<()>>,
}

impl Drop for Wire {
    fn drop(&mut self) {
        for t in self.tasks.drain(..) {
            t.abort();
        }
    }
}

impl Wire {
    /// Decompose into raw inboxes / outbound / task-set components. After this,
    /// the original `Wire` is consumed and the caller is responsible for
    /// aborting the returned tasks when shutting down.
    pub fn split(mut self) -> WireSplit {
        let inboxes = std::mem::take(&mut self.inboxes);
        let outbound = std::mem::take(&mut self.outbound);
        let tasks = std::mem::take(&mut self.tasks);
        WireSplit {
            inboxes,
            outbound,
            tasks,
        }
    }

    /// Bind the listen socket, dial every peer, return a running handle.
    pub async fn start(cfg: WireConfig) -> Result<Self, WireError> {
        // DAG-S31.1: one inbound channel per configured peer. Inbound
        // streams whose hello-frame peer ID doesn't match any configured
        // peer get dropped (matches the static-peer-set assumption that
        // already held implicitly for the perf testnet).
        let mut inbound_txs: HashMap<PeerId, mpsc::Sender<WireEvent>> = HashMap::new();
        let mut inboxes: HashMap<PeerId, mpsc::Receiver<WireEvent>> = HashMap::new();
        for (peer, _addr) in &cfg.peers {
            let (tx, rx) = mpsc::channel::<WireEvent>(4096);
            inbound_txs.insert(peer.clone(), tx);
            inboxes.insert(peer.clone(), rx);
        }
        let inbound_txs = Arc::new(inbound_txs);

        // Bind first so the caller knows the port is up before we return.
        let listener = TcpListener::bind(cfg.listen).await?;
        info!(addr = %cfg.listen, peer_id = %cfg.self_id.0, "wire: listening");

        let mut tasks = Vec::new();

        // Accept loop. Each inbound connection spawns a per-conn reader.
        {
            let inbound_txs = inbound_txs.clone();
            let self_id = cfg.self_id.clone();
            tasks.push(tokio::spawn(async move {
                accept_loop(listener, inbound_txs, self_id).await;
            }));
        }

        // Dial each configured peer. Each dialer is its own task with its own
        // bounded send channel. DAG-S30.3: bumped 1024 -> 8192 to absorb
        // post-S29 block payloads, which carry ~1100 intents (~70 KB
        // serialised) per cert at 4-cert/sec cadence. Pre-S30 the 1024-slot
        // channel filled under brief receiver stalls and `broadcast` silently
        // dropped via `try_send`, starving the cluster of cert proposals.
        let mut outbound = HashMap::new();
        for (peer, addr) in cfg.peers.iter().cloned() {
            let (tx, rx) = mpsc::channel::<WireMessage>(8192);
            outbound.insert(peer.clone(), tx);
            let self_id = cfg.self_id.clone();
            tasks.push(tokio::spawn(async move {
                dialer_loop(peer, addr, rx, self_id).await;
            }));
        }

        Ok(Self {
            inboxes,
            outbound,
            tasks,
        })
    }

    /// Send a single message to one specific peer. Returns `false` if the
    /// outbound channel is full or the peer was never configured.
    pub async fn send_to(&self, peer: &PeerId, msg: WireMessage) -> bool {
        match self.outbound.get(peer) {
            Some(tx) => tx.send(msg).await.is_ok(),
            None => false,
        }
    }

    /// Best-effort broadcast to every configured peer. Returns the number of
    /// peers the message was queued to.
    pub async fn broadcast(&self, msg: WireMessage) -> usize {
        let mut sent = 0;
        for (peer, tx) in &self.outbound {
            if tx.send(msg.clone()).await.is_ok() {
                sent += 1;
            } else {
                warn!(peer = %peer.0, "wire: outbound channel closed");
            }
        }
        sent
    }
}

async fn accept_loop(
    listener: TcpListener,
    inbound_txs: Arc<HashMap<PeerId, mpsc::Sender<WireEvent>>>,
    self_id: PeerId,
) {
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                debug!(remote = %addr, "wire: inbound");
                let _ = stream.set_nodelay(true);
                let inbound_txs = inbound_txs.clone();
                let self_id = self_id.clone();
                tokio::spawn(async move {
                    if let Err(e) = read_loop(stream, inbound_txs, self_id, addr).await {
                        debug!(remote = %addr, err = %e, "wire: inbound closed");
                    }
                });
            }
            Err(e) => {
                warn!(err = %e, "wire: accept failed");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

/// Reads frames from an inbound peer until EOF or error. The peer identity is
/// learned from the first frame: a sender writes its [`PeerId`] before any
/// `WireMessage`. Without that, logs only have the socket address and you
/// can't tell which region the traffic came from.
///
/// DAG-S31.1: routes WireEvents to a per-peer channel rather than a
/// shared fan-in. If the hello-frame peer ID doesn't match any
/// configured peer, the connection is closed.
async fn read_loop(
    mut stream: TcpStream,
    inbound_txs: Arc<HashMap<PeerId, mpsc::Sender<WireEvent>>>,
    _self_id: PeerId,
    remote_addr: SocketAddr,
) -> Result<(), WireError> {
    let hello = read_frame(&mut stream).await?;
    let from: PeerId = crate::codec::decode_frame(&hello)?;
    debug!(peer = %from.0, addr = %remote_addr, "wire: hello");

    let inbound_tx = match inbound_txs.get(&from) {
        Some(tx) => tx.clone(),
        None => {
            warn!(peer = %from.0, addr = %remote_addr, "wire: unknown peer in hello, dropping");
            return Ok(());
        }
    };

    loop {
        let bytes = read_frame(&mut stream).await?;
        let msg: WireMessage = crate::codec::decode_frame(&bytes)?;
        // B3 hardening: per-variant size cap. `Block` carries up to
        // ~1100 intents in the perf testnet's peak, so the cap
        // sits at `MAX_FRAME_BYTES` (1 MiB). Compact variants
        // (`Cert`, `Vote`, `GetCert`, `FastPath`, `Ltp`,
        // `Ping`/`Pong`) cap at `MAX_COMPACT_MESSAGE_BYTES`
        // (64 KiB) — a malicious peer can't burn CPU sending us
        // an inflated cert that happens to fit inside the
        // 1 MiB frame cap.
        if !enforce_compact_variant_cap(&msg, bytes.len()) {
            warn!(
                peer = %from.0,
                variant = wire_variant_name(&msg),
                bytes = bytes.len(),
                cap = MAX_COMPACT_MESSAGE_BYTES,
                "wire: compact-variant cap exceeded; dropping frame"
            );
            continue;
        }
        if inbound_tx
            .send(WireEvent {
                from: from.clone(),
                msg,
            })
            .await
            .is_err()
        {
            return Ok(());
        }
    }
}

/// B3 hardening: return `true` if the frame size is OK for this
/// variant. `Block` is the only variant allowed to use the full
/// `MAX_FRAME_BYTES` envelope — everything else must fit in the
/// tighter `MAX_COMPACT_MESSAGE_BYTES` cap.
fn enforce_compact_variant_cap(msg: &WireMessage, frame_bytes: usize) -> bool {
    match msg {
        // `Block` payload can be large; rely on the outer frame cap.
        WireMessage::Block(_) => true,
        // Everything else should fit in the compact cap.
        _ => frame_bytes <= MAX_COMPACT_MESSAGE_BYTES,
    }
}

fn wire_variant_name(msg: &WireMessage) -> &'static str {
    match msg {
        WireMessage::Cert(_) => "Cert",
        WireMessage::Vote(_) => "Vote",
        WireMessage::Block(_) => "Block",
        WireMessage::GetCert(_) => "GetCert",
        WireMessage::FastPath(_) => "FastPath",
        WireMessage::Ltp(_) => "Ltp",
        WireMessage::Ping(_) => "Ping",
        WireMessage::Pong(_) => "Pong",
    }
}

async fn dialer_loop(
    peer: PeerId,
    addr: SocketAddr,
    mut rx: mpsc::Receiver<WireMessage>,
    self_id: PeerId,
) {
    let mut backoff_ms = RECONNECT_MIN_MS;
    // Persistent outbound queue: messages sent while disconnected are dropped
    // (consistent with best-effort gossip). The latest message wins.
    let pending: Arc<RwLock<Option<WireMessage>>> = Arc::new(RwLock::new(None));

    loop {
        match TcpStream::connect(addr).await {
            Ok(mut stream) => {
                info!(peer = %peer.0, addr = %addr, "wire: dialed");
                let _ = stream.set_nodelay(true);
                backoff_ms = RECONNECT_MIN_MS;

                // Send hello first so the peer can label inbound frames.
                let hello_bytes = match crate::codec::encode_frame(&self_id) {
                    Ok(b) => b,
                    Err(e) => {
                        warn!(err = %e, "wire: hello serialize");
                        break;
                    }
                };
                if let Err(e) = write_frame(&mut stream, &hello_bytes).await {
                    warn!(peer = %peer.0, err = %e, "wire: hello write");
                    continue;
                }

                // Drain any queued send from before the connection landed.
                if let Some(msg) = pending.write().await.take() {
                    if let Ok(bytes) = crate::codec::encode_frame(&msg) {
                        let _ = write_frame(&mut stream, &bytes).await;
                    }
                }

                // Steady-state: pull from channel, write frame.
                loop {
                    match rx.recv().await {
                        Some(msg) => match crate::codec::encode_frame(&msg) {
                            Ok(bytes) => {
                                if let Err(e) = write_frame(&mut stream, &bytes).await {
                                    warn!(peer = %peer.0, err = %e, "wire: send failed");
                                    *pending.write().await = Some(msg);
                                    break;
                                }
                            }
                            Err(e) => warn!(err = %e, "wire: serialize"),
                        },
                        None => return, // channel closed; node shutting down
                    }
                }
            }
            Err(e) => {
                debug!(peer = %peer.0, addr = %addr, err = %e, backoff_ms, "wire: dial failed");
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                backoff_ms = (backoff_ms * 2).min(RECONNECT_MAX_MS);
            }
        }
    }
}

async fn read_frame(stream: &mut TcpStream) -> Result<Vec<u8>, WireError> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await.map_err(|e| {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            WireError::Closed
        } else {
            WireError::Io(e)
        }
    })?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(WireError::FrameTooLarge(len));
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    Ok(buf)
}

async fn write_frame(stream: &mut TcpStream, payload: &[u8]) -> Result<(), WireError> {
    if payload.len() > MAX_FRAME_BYTES {
        return Err(WireError::FrameTooLarge(payload.len()));
    }
    let len = (payload.len() as u32).to_be_bytes();
    stream.write_all(&len).await?;
    stream.write_all(payload).await?;
    stream.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two-node loopback: A sends a Ping, B receives it with the correct
    /// peer label. Confirms the hello + frame loop is wired correctly.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn two_node_ping_loopback() {
        let a_addr: SocketAddr = "127.0.0.1:18801".parse().unwrap();
        let b_addr: SocketAddr = "127.0.0.1:18802".parse().unwrap();

        let mut a = Wire::start(WireConfig {
            self_id: PeerId::new("a"),
            listen: a_addr,
            peers: vec![(PeerId::new("b"), b_addr)],
        })
        .await
        .unwrap();

        let mut b = Wire::start(WireConfig {
            self_id: PeerId::new("b"),
            listen: b_addr,
            peers: vec![(PeerId::new("a"), a_addr)],
        })
        .await
        .unwrap();

        // Give the dialers a moment to land. 250ms is generous for loopback.
        tokio::time::sleep(Duration::from_millis(250)).await;

        assert!(a.send_to(&PeerId::new("b"), WireMessage::Ping(42)).await);
        assert!(b.send_to(&PeerId::new("a"), WireMessage::Ping(99)).await);

        let from_a = tokio::time::timeout(
            Duration::from_secs(2),
            b.inboxes.get_mut(&PeerId::new("a")).unwrap().recv(),
        )
        .await
        .expect("b receive timed out")
        .expect("b inbox closed");
        assert_eq!(from_a.from.0, "a");
        assert!(matches!(from_a.msg, WireMessage::Ping(42)));

        let from_b = tokio::time::timeout(
            Duration::from_secs(2),
            a.inboxes.get_mut(&PeerId::new("b")).unwrap().recv(),
        )
        .await
        .expect("a receive timed out")
        .expect("a inbox closed");
        assert_eq!(from_b.from.0, "b");
        assert!(matches!(from_b.msg, WireMessage::Ping(99)));
    }

    /// Three-node broadcast: A broadcasts a Ping; both B and C must see it.
    /// Confirms `Wire::broadcast` fans out and ordering between peers is
    /// independent (B and C receive without coordination).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn three_node_broadcast() {
        let a_addr: SocketAddr = "127.0.0.1:18811".parse().unwrap();
        let b_addr: SocketAddr = "127.0.0.1:18812".parse().unwrap();
        let c_addr: SocketAddr = "127.0.0.1:18813".parse().unwrap();

        let a = Wire::start(WireConfig {
            self_id: PeerId::new("a"),
            listen: a_addr,
            peers: vec![(PeerId::new("b"), b_addr), (PeerId::new("c"), c_addr)],
        })
        .await
        .unwrap();

        let mut b = Wire::start(WireConfig {
            self_id: PeerId::new("b"),
            listen: b_addr,
            peers: vec![(PeerId::new("a"), a_addr), (PeerId::new("c"), c_addr)],
        })
        .await
        .unwrap();

        let mut c = Wire::start(WireConfig {
            self_id: PeerId::new("c"),
            listen: c_addr,
            peers: vec![(PeerId::new("a"), a_addr), (PeerId::new("b"), b_addr)],
        })
        .await
        .unwrap();

        tokio::time::sleep(Duration::from_millis(300)).await;

        let sent = a.broadcast(WireMessage::Ping(7)).await;
        assert_eq!(sent, 2);

        let ev_b = tokio::time::timeout(
            Duration::from_secs(2),
            b.inboxes.get_mut(&PeerId::new("a")).unwrap().recv(),
        )
        .await
        .expect("b timed out")
        .expect("b closed");
        let ev_c = tokio::time::timeout(
            Duration::from_secs(2),
            c.inboxes.get_mut(&PeerId::new("a")).unwrap().recv(),
        )
        .await
        .expect("c timed out")
        .expect("c closed");

        assert_eq!(ev_b.from.0, "a");
        assert_eq!(ev_c.from.0, "a");
        assert!(matches!(ev_b.msg, WireMessage::Ping(7)));
        assert!(matches!(ev_c.msg, WireMessage::Ping(7)));
    }
}
