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

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use gsx_consensus::cert::Certificate;
use gsx_consensus::joint::Vote;
use gsx_fastpath::cert::FastPathCert;
use gsx_ltp::attestation::CorridorAttestation;

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
    /// Validator Ring ratification vote (joint quorum AND-gate).
    Vote(Vote),
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
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

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
    /// bincode serialization or deserialization failure.
    #[error("codec: {0}")]
    Codec(#[from] bincode::Error),
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
    /// Inbound channel: every received message from any peer arrives here.
    pub inbox: mpsc::Receiver<WireEvent>,
    /// Per-peer outbound channels. Send a message into the channel and the
    /// dialer task will write it to the peer's TCP socket. Slow peers drop
    /// excess sends (bounded channel) — this matches the consensus paper's
    /// "best-effort gossip, retries via re-broadcast" model.
    outbound: HashMap<PeerId, mpsc::Sender<WireMessage>>,
    /// Background task handles. Aborted on drop.
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for Wire {
    fn drop(&mut self) {
        for t in self.tasks.drain(..) {
            t.abort();
        }
    }
}

impl Wire {
    /// Bind the listen socket, dial every peer, return a running handle.
    pub async fn start(cfg: WireConfig) -> Result<Self, WireError> {
        let (inbound_tx, inbound_rx) = mpsc::channel::<WireEvent>(4096);

        // Bind first so the caller knows the port is up before we return.
        let listener = TcpListener::bind(cfg.listen).await?;
        info!(addr = %cfg.listen, peer_id = %cfg.self_id.0, "wire: listening");

        let mut tasks = Vec::new();

        // Accept loop. Each inbound connection spawns a per-conn reader.
        {
            let inbound_tx = inbound_tx.clone();
            let self_id = cfg.self_id.clone();
            tasks.push(tokio::spawn(async move {
                accept_loop(listener, inbound_tx, self_id).await;
            }));
        }

        // Dial each configured peer. Each dialer is its own task with its own
        // bounded send channel.
        let mut outbound = HashMap::new();
        for (peer, addr) in cfg.peers.iter().cloned() {
            let (tx, rx) = mpsc::channel::<WireMessage>(1024);
            outbound.insert(peer.clone(), tx);
            let self_id = cfg.self_id.clone();
            tasks.push(tokio::spawn(async move {
                dialer_loop(peer, addr, rx, self_id).await;
            }));
        }

        Ok(Self {
            inbox: inbound_rx,
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
    inbound_tx: mpsc::Sender<WireEvent>,
    self_id: PeerId,
) {
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                debug!(remote = %addr, "wire: inbound");
                let _ = stream.set_nodelay(true);
                let inbound_tx = inbound_tx.clone();
                let self_id = self_id.clone();
                tokio::spawn(async move {
                    if let Err(e) = read_loop(stream, inbound_tx, self_id, addr).await {
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
async fn read_loop(
    mut stream: TcpStream,
    inbound_tx: mpsc::Sender<WireEvent>,
    _self_id: PeerId,
    remote_addr: SocketAddr,
) -> Result<(), WireError> {
    let hello = read_frame(&mut stream).await?;
    let from: PeerId = bincode::deserialize(&hello)?;
    debug!(peer = %from.0, addr = %remote_addr, "wire: hello");

    loop {
        let bytes = read_frame(&mut stream).await?;
        let msg: WireMessage = bincode::deserialize(&bytes)?;
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
                let hello_bytes = match bincode::serialize(&self_id) {
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
                    if let Ok(bytes) = bincode::serialize(&msg) {
                        let _ = write_frame(&mut stream, &bytes).await;
                    }
                }

                // Steady-state: pull from channel, write frame.
                loop {
                    match rx.recv().await {
                        Some(msg) => match bincode::serialize(&msg) {
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

        let from_a = tokio::time::timeout(Duration::from_secs(2), b.inbox.recv())
            .await
            .expect("b receive timed out")
            .expect("b inbox closed");
        assert_eq!(from_a.from.0, "a");
        assert!(matches!(from_a.msg, WireMessage::Ping(42)));

        let from_b = tokio::time::timeout(Duration::from_secs(2), a.inbox.recv())
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
            peers: vec![
                (PeerId::new("b"), b_addr),
                (PeerId::new("c"), c_addr),
            ],
        })
        .await
        .unwrap();

        let mut b = Wire::start(WireConfig {
            self_id: PeerId::new("b"),
            listen: b_addr,
            peers: vec![
                (PeerId::new("a"), a_addr),
                (PeerId::new("c"), c_addr),
            ],
        })
        .await
        .unwrap();

        let mut c = Wire::start(WireConfig {
            self_id: PeerId::new("c"),
            listen: c_addr,
            peers: vec![
                (PeerId::new("a"), a_addr),
                (PeerId::new("b"), b_addr),
            ],
        })
        .await
        .unwrap();

        tokio::time::sleep(Duration::from_millis(300)).await;

        let sent = a.broadcast(WireMessage::Ping(7)).await;
        assert_eq!(sent, 2);

        let ev_b = tokio::time::timeout(Duration::from_secs(2), b.inbox.recv())
            .await
            .expect("b timed out")
            .expect("b closed");
        let ev_c = tokio::time::timeout(Duration::from_secs(2), c.inbox.recv())
            .await
            .expect("c timed out")
            .expect("c closed");

        assert_eq!(ev_b.from.0, "a");
        assert_eq!(ev_c.from.0, "a");
        assert!(matches!(ev_b.msg, WireMessage::Ping(7)));
        assert!(matches!(ev_c.msg, WireMessage::Ping(7)));
    }
}
