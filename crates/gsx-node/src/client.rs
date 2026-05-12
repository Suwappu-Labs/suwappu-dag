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
//! 2. Validator pushes each intent into `state.pending_intents` (picked up
//!    by the round driver in the next tick) and replies with
//!    [`ClientResponse::Ack`] containing the intent hash + the round into
//!    which it was queued.
//!
//! Auth: none at the wire level for the perf testnet. The intent itself
//! carries the sender's identity (`Intent::Transfer { from, .. }`); mainnet
//! would gate this with an ML-DSA signature over the intent bytes.

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use gsx_execution::Intent;

use crate::daemon::State;
use crate::events::{Event, EventLog, Lane};

/// Client → validator messages.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClientMessage {
    /// Submit one transfer intent for inclusion in the next block.
    Submit(Intent),
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
    state: Arc<Mutex<State>>,
    log: EventLog,
) -> io::Result<tokio::task::JoinHandle<()>> {
    let listener = TcpListener::bind(listen).await?;
    info!(addr = %listen, "client: listening for intent submissions");
    let handle = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    debug!(remote = %addr, "client: inbound");
                    let _ = stream.set_nodelay(true);
                    let state = state.clone();
                    let log = log.clone();
                    let self_label = self_label.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_conn(stream, self_label, state, log).await {
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

async fn handle_conn(
    mut stream: TcpStream,
    self_label: String,
    state: Arc<Mutex<State>>,
    log: EventLog,
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
            ClientMessage::Submit(intent) => {
                let intent_hash: [u8; 32] = blake3::hash(
                    &bincode::serialize(&intent).expect("intent serialize"),
                )
                .into();
                {
                    let mut s = state.lock().await;
                    s.pending_intents.push(intent);
                }
                log.emit(
                    Event::now(&self_label, Lane::Client, "submitted")
                        .with_tx_hash(&intent_hash),
                );
                write_response(&mut stream, &ClientResponse::Ack { intent_hash }).await?;
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

/// Client-side helper used by `gsx-loadgen`. Wraps a single TCP connection
/// and handles framing.
pub struct LoadGenClient {
    stream: TcpStream,
}

impl LoadGenClient {
    /// Open a connection to a validator's client listen address.
    pub async fn connect(addr: SocketAddr) -> io::Result<Self> {
        let stream = TcpStream::connect(addr).await?;
        let _ = stream.set_nodelay(true);
        Ok(Self { stream })
    }

    /// Submit one intent and wait for the Ack.
    pub async fn submit(&mut self, intent: Intent) -> io::Result<[u8; 32]> {
        let bytes = bincode::serialize(&ClientMessage::Submit(intent))
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
            ClientResponse::Pong(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected Pong for Submit",
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
