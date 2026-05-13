//! Structured event log.
//!
//! Every consensus state transition emits one NDJSON line to disk. `gsx-metrics`
//! tails this file, joins per-event timestamps across regions, and produces
//! the latency CDF for each lane.
//!
//! Schema (one JSON object per line):
//!
//! ```json
//! {"t_ms":1715553210123,"region":"us-east-1","lane":"main","event":"proposed","round":42,"cert_hash":"abc..."}
//! ```
//!
//! Fields:
//!
//! - `t_ms`     — unix millis at which the event happened
//! - `region`   — `self_id` from config (e.g. `"us-east-1"`)
//! - `lane`     — one of `"main"`, `"fastpath"`, `"ltp"`, `"client"`
//! - `event`    — verb (`"proposed"`, `"received"`, `"voted"`, `"committed"`,
//!   `"certified"`, `"attested"`, `"submitted"`)
//! - `round`    — optional consensus round (main / fastpath only)
//! - `cert_hash`/`tx_hash` — optional content-addressed reference
//! - `peer`     — optional peer label (for `"received"` events)

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tokio::{
    io::AsyncWriteExt,
    sync::{mpsc, Mutex},
};

/// Which lane a given event belongs to. Matches the paper's three on-chain
/// commitment surfaces plus a synthetic "client" lane for load-generator IO.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lane {
    /// Main DAG lane — certs, votes, commits.
    Main,
    /// Single-owner fast path (paper §6.4).
    FastPath,
    /// Lattice Transfer Protocol corridor attestations (paper §10.2).
    Ltp,
    /// Load-generator IO (intent submission acks). Not on-chain.
    Client,
}

/// One log line.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    /// Unix millis.
    pub t_ms: u64,
    /// Validator region label (matches `NodeConfig::self_id`).
    pub region: String,
    /// Lane the event belongs to.
    pub lane: Lane,
    /// Action verb.
    pub event: String,
    /// Optional consensus round.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub round: Option<u64>,
    /// Optional cert hash (hex).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cert_hash: Option<String>,
    /// Optional tx hash (hex).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_hash: Option<String>,
    /// Optional remote peer label (set for `"received"` events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer: Option<String>,
}

impl Event {
    /// Construct an event tagged with the current wall-clock millis.
    pub fn now(region: impl Into<String>, lane: Lane, event: impl Into<String>) -> Self {
        let t_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        Self {
            t_ms,
            region: region.into(),
            lane,
            event: event.into(),
            round: None,
            cert_hash: None,
            tx_hash: None,
            peer: None,
        }
    }

    /// Builder: attach a round number.
    pub fn with_round(mut self, round: u64) -> Self {
        self.round = Some(round);
        self
    }

    /// Builder: attach a cert hash as hex.
    pub fn with_cert_hash(mut self, bytes: &[u8; 32]) -> Self {
        self.cert_hash = Some(hex::encode(bytes));
        self
    }

    /// Builder: attach a transaction hash as hex.
    pub fn with_tx_hash(mut self, bytes: &[u8; 32]) -> Self {
        self.tx_hash = Some(hex::encode(bytes));
        self
    }

    /// Builder: attach a peer label.
    pub fn with_peer(mut self, peer: impl Into<String>) -> Self {
        self.peer = Some(peer.into());
        self
    }
}

/// Async NDJSON event writer. One per daemon process.
///
/// The writer task buffers up to 4096 events; if it can't keep up, oldest
/// events are dropped (matches the "best-effort metrics" model — we'd rather
/// keep the validator running than block consensus on log flushing).
#[derive(Clone)]
pub struct EventLog {
    tx: mpsc::Sender<Event>,
}

impl EventLog {
    /// Start a writer task for the given path. Returns the handle plus a
    /// `JoinHandle` the daemon should hold for the lifetime of the process.
    pub async fn start(
        path: impl AsRef<Path>,
    ) -> std::io::Result<(Self, tokio::task::JoinHandle<()>)> {
        let path: PathBuf = path.as_ref().to_path_buf();
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                tokio::fs::create_dir_all(dir).await?;
            }
        }
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;
        let file = Arc::new(Mutex::new(file));

        let (tx, mut rx) = mpsc::channel::<Event>(4096);
        let handle = tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                let mut line = match serde_json::to_string(&ev) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(err = %e, "event-log: serialize");
                        continue;
                    }
                };
                line.push('\n');
                let mut f = file.lock().await;
                if let Err(e) = f.write_all(line.as_bytes()).await {
                    tracing::warn!(err = %e, "event-log: write");
                }
            }
        });

        Ok((Self { tx }, handle))
    }

    /// Send an event. Non-blocking; drops if the writer is back-pressured.
    pub fn emit(&self, ev: Event) {
        let _ = self.tx.try_send(ev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn writes_two_events_to_disk() {
        let tmp = tempfile_path();
        let (log, handle) = EventLog::start(&tmp).await.unwrap();
        log.emit(Event::now("us-east-1", Lane::Main, "proposed").with_round(7));
        log.emit(
            Event::now("us-east-1", Lane::FastPath, "certified")
                .with_tx_hash(&[0xAB; 32])
                .with_peer("eu-west-1"),
        );
        drop(log);
        handle.await.unwrap();

        let contents = tokio::fs::read_to_string(&tmp).await.unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2);

        let first: Event = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first.lane, Lane::Main);
        assert_eq!(first.event, "proposed");
        assert_eq!(first.round, Some(7));

        let second: Event = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second.lane, Lane::FastPath);
        assert_eq!(second.peer.as_deref(), Some("eu-west-1"));
        assert!(second.tx_hash.is_some());

        let _ = tokio::fs::remove_file(&tmp).await;
    }

    fn tempfile_path() -> PathBuf {
        // Unique per invocation: pid + nanos. Prior version reused the same
        // path across runs in the same process, which made the test flaky
        // when a stale empty file was present from a previous interrupted
        // run (the read happened before the writer task flushed both lines).
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "gsx-events-{}-{}.ndjson",
            std::process::id(),
            nanos
        ));
        // Make sure the file does not exist before the test runs.
        let _ = std::fs::remove_file(&p);
        p
    }
}
