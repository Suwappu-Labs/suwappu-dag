# suwappu-top — implementation contract

A LIVE terminal dashboard for the suwappu-dag local baremetal devnet. Binary name
`suwappu-top`. Crate path `crates/suwappu-top`.

This document is the CANONICAL reference for parallel implementation. Each file
below has a single responsibility and an exact public API. Fill files
independently; do not change a public signature without updating this file and
`DESIGN.md` is the source of truth that all agents read.

## Architecture (locked)

- **Sync threads, no async runtime.** One OS thread per data source. Each
  source thread parses its input and sends `Msg` values over a single
  `std::sync::mpsc::Sender<Msg>`. The UI thread owns the only `AppState`, drains
  the channel, and redraws on a ~250ms tick.
- **`AppState::apply(&mut self, msg: Msg)` is the SOLE mutation point.** Source
  threads are pure parsers; they never compute derived state. ALL aggregation
  (commits/s, settled-matching, peer-mesh, p50 latency, uptime, drops) lives in
  `state.rs`, inside `apply` or in `&self` getters on `AppState`.
- **The settled-match (acked tx_hash ∈ a committed block's intent_hashes) lives
  in `apply`**, because it needs both the loadgen stream and the events stream.
  `loadgen.rs` only emits `Msg::LoadAck`; it never matches.
- **Snapshot mode shares the identical source layer.** `--snapshot` spawns the
  same source threads, collects ~3s, then prints `AppState::snapshot_text()`
  (and `--json` prints `serde_json`), and exits 0 WITHOUT entering crossterm raw
  mode. TUI and snapshot differ only in the consumer.

## Ownership split (load-bearing)

- Per-node ground-truth committed round, header round, commits/s, committed
  total, recent rounds: ALL fed by **events.rs `committed` events**, NOT RPC
  `latest_committed_round` (RPC overstates because it advances on own-propose).
- RPC feeds: epoch (`current`, `last_boundary_round`, `rounds_per_epoch`), role
  (which registry an id appears in → AUTH/VAL), and node reachability/liveness.
- `network_id`: read ONCE at startup from `target/devnet/genesis.toml`, in
  `cli.rs`. Not from RPC.
- mempool depth: there is NO RPC method or event for it today. Field is
  `Option<u64>`, always `None`, renders blank. Do not hunt for a method.
- peer-mesh denominator = `N-1` where `N` = discovered node count from config.
- drops = acked − settled, reported only after a per-run grace window elapses;
  `None` until then.

## Integration checks (design to the stated contract now; verify once at wiring)

- id ↔ node index: registries return `id:u32`; events self-id `"v0"`. Default
  `id == node_index`. One-line check at integration; non-blocking.
- tx_hash == intent_hash: loadgen emits `hex::encode([u8;32])` (lowercase, no
  `0x`); `intent_hashes` are hex no `0x`. Match by lowercasing both. Put
  normalization in one helper.

---

## Cargo.toml

```toml
[package]
name = "suwappu-top"
version = { workspace = true }
edition = { workspace = true }
rust-version = { workspace = true }
license = { workspace = true }
repository = { workspace = true }
authors = { workspace = true }
publish = false

[[bin]]
name = "suwappu-top"
path = "src/main.rs"

[dependencies]
ratatui = "0.28"
crossterm = "0.28"
ureq = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
clap = { version = "4", features = ["derive"] }
hex = "0.4"
tracing = "0.1"
```

Also add `"crates/suwappu-top"` to the workspace `members` list in the root
`Cargo.toml`. (Versions: ureq/serde/serde_json/anyhow/clap/hex/tracing already
in Cargo.lock at these majors; ratatui+crossterm are new at 0.28.)

---

## File layout

```
crates/suwappu-top/
  Cargo.toml
  DESIGN.md            (this file)
  src/
    main.rs            entrypoint: dispatch snapshot vs tui vs load
    cli.rs             clap CLI, source discovery, network_id, config struct
    state.rs           ALL shared types + Msg + AppState + aggregation + snapshot_text
    sources/
      mod.rs           `pub mod rpc; pub mod events; pub mod loadgen;`
      rpc.rs           RPC poller thread -> Msg
      events.rs        events.ndjson tailer thread -> Msg
      loadgen.rs       spawn suwappu-loadgen subprocess, parse CSV -> Msg
    ui/
      mod.rs           `pub mod panels;`
      panels.rs        ratatui rendering only (pure &AppState read)
```

---

## src/state.rs — types, aggregation, snapshot (depends on NOTHING else in crate)

This is the spine. Every other module depends only on this file.

```rust
use std::collections::{HashMap, HashSet, VecDeque};
use serde::Serialize;

// ---- tuning constants -----------------------------------------------------

/// Width (samples) of the commits/s and tps sparkline ring buffers.
pub const SPARK_WIDTH: usize = 60;
/// A peer counts toward peer-mesh if a `received(peer=..)` arrived within this
/// many ms of "now".
pub const PEER_MESH_WINDOW_MS: u64 = 5_000;
/// Number of recent committed rounds to retain for the CONSENSUS panel.
pub const RECENT_ROUNDS: usize = 16;
/// After a load run ends, wait this long before declaring un-settled acks as
/// drops (lets late commits land).
pub const LOAD_GRACE_MS: u64 = 8_000;
/// Width of one commits/s and tps bucket.
pub const BUCKET_MS: u64 = 1_000;

// ---- roles ----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Role {
    /// id present in the authority registry.
    Auth,
    /// id present in the validator registry (and not authority).
    Val,
    /// not yet classified.
    Unknown,
}

// ---- per-node stats -------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct NodeStat {
    /// node index parsed from self-id "v<i>" (== registry id by default).
    pub id: u32,
    /// self-id string, e.g. "v0".
    pub label: String,
    pub role: Role,
    /// ground-truth committed round = max round from this node's `committed`
    /// events. None until first committed event seen.
    pub committed_round: Option<u64>,
    /// mempool depth — no source today; always None (renders blank).
    pub mempool: Option<u64>,
    /// t_ms of the first event ever seen from this node (for uptime).
    pub first_seen_ms: Option<u64>,
    /// peer label -> last t_ms we saw a `received(peer=..)` naming this peer,
    /// recorded on the RECEIVING node. Used for peer-mesh recency.
    pub peers_last_ms: HashMap<String, u64>,
    /// last time ANY event from this node arrived (liveness).
    pub last_event_ms: Option<u64>,
    /// true if last RPC poll reached this node's rpc port.
    pub rpc_reachable: bool,
}

impl NodeStat {
    pub fn new(id: u32, label: String) -> Self;
    /// Seconds since first_seen, given `now_ms`. 0 if never seen.
    pub fn uptime_secs(&self, now_ms: u64) -> u64;
    /// Count of distinct peers with a `received` within PEER_MESH_WINDOW_MS of
    /// `now_ms`.
    pub fn active_peers(&self, now_ms: u64) -> usize;
}

// ---- consensus ------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ConsensusStat {
    /// total `committed` events seen across all nodes.
    pub committed_total: u64,
    /// recent committed (round) values, most-recent last, capped RECENT_ROUNDS.
    pub recent_rounds: VecDeque<u64>,
    /// per-second commit-count buckets, oldest..newest, capped SPARK_WIDTH.
    #[serde(skip)]
    pub commit_buckets: VecDeque<u64>,
    /// t_ms of the newest open bucket's start.
    #[serde(skip)]
    pub cur_bucket_start_ms: u64,
}

impl ConsensusStat {
    pub fn new() -> Self;
    /// Record one committed event at `t_ms` for `round`.
    pub fn record_commit(&mut self, round: u64, t_ms: u64);
    /// commits/s of the most recent completed bucket.
    pub fn commits_per_sec(&self) -> f64;
}

// ---- LTP / fastpath -------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize)]
pub struct LtpStat {
    pub verified: u64,
    pub invalid: u64,
    pub unverified: u64,
    /// latest LTP state_root (cert_hash) seen, hex no 0x.
    pub latest_state_root: Option<String>,
    /// source height of the latest LTP event (round field).
    pub latest_height: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct FastpathStat {
    pub signed: u64,
    pub committed: u64,
    pub slashed: u64,
}

// ---- load -----------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct LoadStat {
    /// true between LoadStarted and LoadEnded+grace; controls panel visibility.
    pub active: bool,
    /// configured tps from the launch.
    pub rate: Option<u64>,
    /// total acks received from loadgen CSV.
    pub acked: u64,
    /// acked tx_hashes that have appeared in a committed block's intent_hashes.
    pub settled: u64,
    /// t_ms the run started (first ack or LoadStarted).
    pub started_ms: Option<u64>,
    /// t_ms LoadEnded arrived (loadgen process exited). None while running.
    pub ended_ms: Option<u64>,
    /// pending acks awaiting settlement: normalized tx_hash -> ack send_ms.
    #[serde(skip)]
    pub pending: HashMap<String, u64>,
    /// settle latencies (ms) for matched tx, for p50. Ring, capped 4096.
    #[serde(skip)]
    pub settle_latencies_ms: VecDeque<u64>,
    /// per-second settled-count buckets for the tps sparkline.
    #[serde(skip)]
    pub tps_buckets: VecDeque<u64>,
    #[serde(skip)]
    pub cur_bucket_start_ms: u64,
}

impl LoadStat {
    pub fn new() -> Self;
    /// p50 of settle latencies, ms. None if no settled tx yet.
    pub fn p50_settle_ms(&self) -> Option<u64>;
    /// drops = acked - settled, only AFTER grace window past ended_ms.
    /// None while running or within grace.
    pub fn drops(&self, now_ms: u64) -> Option<u64>;
}

// ---- header / chain -------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize)]
pub struct ChainStat {
    /// highest committed round seen across nodes (events ground truth).
    pub round: u64,
    pub epoch: u64,
    pub last_boundary_round: u64,
    pub rounds_per_epoch: u64,
    pub network_id: Option<String>,
}

// ---- the message enum (source -> UI) --------------------------------------
//
// Each variant is annotated with the SOLE source that emits it. Sources never
// emit variants belonging to another source.

#[derive(Debug, Clone)]
pub enum Msg {
    // ---- rpc.rs ----
    /// epoch view from suwappu_getEpoch (any one reachable node).
    Epoch { current: u64, last_boundary_round: u64, rounds_per_epoch: u64 },
    /// registry membership: ids in authority and validator registries.
    Registries { authority_ids: Vec<u32>, validator_ids: Vec<u32> },
    /// liveness: whether node `id`'s rpc port answered this poll.
    RpcReachable { id: u32, reachable: bool },

    // ---- events.rs ----
    /// first time we observe ANY event from node `id` (sets uptime origin).
    NodeFirstSeen { id: u32, label: String, t_ms: u64 },
    /// main-lane `committed` event from node `id`.
    Committed { id: u32, round: u64, t_ms: u64, intent_hashes: Vec<String> },
    /// main-lane `received(peer=..)` recorded on receiving node `id`.
    PeerRecv { id: u32, peer: String, t_ms: u64 },
    /// any event from node `id` at t_ms (updates last_event_ms liveness).
    NodeTick { id: u32, t_ms: u64 },
    /// ltp-lane event. kind = verified|invalid|unverified.
    Ltp { kind: LtpKind, height: Option<u64>, state_root: Option<String>, t_ms: u64 },
    /// fastpath-lane lifecycle event.
    Fastpath { kind: FastpathKind, t_ms: u64 },

    // ---- loadgen.rs ----
    /// loadgen process launched with configured rate.
    LoadStarted { rate: u64, t_ms: u64 },
    /// one CSV ack line: normalized (lowercased) tx_hash + client_submitted_ms.
    LoadAck { tx_hash: String, send_ms: u64 },
    /// loadgen process exited.
    LoadEnded { t_ms: u64 },

    // ---- control ----
    /// fatal source error, surfaced to status line (non-panic).
    SourceError { source: &'static str, msg: String },
}

#[derive(Debug, Clone, Copy)]
pub enum LtpKind { Verified, Invalid, Unverified }

#[derive(Debug, Clone, Copy)]
pub enum FastpathKind { Signed, Committed, Slashed }

// ---- AppState — sole mutation point ---------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct AppState {
    pub chain: ChainStat,
    /// node id -> stats, sorted for display by id.
    pub nodes: HashMap<u32, NodeStat>,
    pub consensus: ConsensusStat,
    pub ltp: LtpStat,
    pub fastpath: FastpathStat,
    pub load: LoadStat,
    /// total discovered node count N (peer-mesh denominator = N-1).
    pub node_count: usize,
    /// last non-fatal source error for the status line.
    pub last_error: Option<String>,
    /// wall-clock t_ms of the last apply (used as "now" for recency getters).
    pub now_ms: u64,
}

impl AppState {
    /// `node_count` and `network_id` come from resolved config.
    pub fn new(node_count: usize, network_id: Option<String>) -> Self;

    /// THE sole mutation point. All aggregation happens here.
    /// - Committed: bump consensus totals/buckets/recent, set node
    ///   committed_round + chain.round = max, AND run the settled-match against
    ///   load.pending (lowercased) updating load.settled / latencies / tps.
    /// - LoadAck: insert into load.pending, bump acked, set started/active.
    /// - LoadEnded: set ended_ms (drops resolve after grace).
    /// - PeerRecv: record peer recency on receiving node.
    /// etc.
    pub fn apply(&mut self, msg: Msg);

    /// Update now_ms to the larger of current and `t`. Called by the UI tick
    /// and by apply for each timestamped msg.
    pub fn touch_now(&mut self, t_ms: u64);

    /// Nodes sorted by id ascending (for table rendering).
    pub fn nodes_sorted(&self) -> Vec<&NodeStat>;

    /// commits/s sparkline samples, oldest..newest (ratatui Sparkline wants &[u64]).
    pub fn commits_sparkline(&self) -> Vec<u64>;
    /// settled-tps sparkline samples, oldest..newest.
    pub fn tps_sparkline(&self) -> Vec<u64>;

    /// Human-readable multi-line snapshot for --snapshot (no terminal).
    pub fn snapshot_text(&self) -> String;
}
```

---

## src/cli.rs — CLI parsing + source discovery (depends on: state.rs)

```rust
use std::path::PathBuf;
use anyhow::Result;
use clap::{Parser, Subcommand};

/// Default rpc port for node v<i> = 9092 + 10*i.
pub fn default_rpc_port(node_index: u32) -> u16; // 9092 + 10*i
/// Default client TCP port for node v<i> = 9091 + 10*i (loadgen target).
pub fn default_client_port(node_index: u32) -> u16; // 9091 + 10*i

#[derive(Parser, Debug)]
#[command(name = "suwappu-top", version, about = "Live TUI for the suwappu-dag devnet")]
pub struct Cli {
    /// Repo root or devnet dir to auto-discover target/devnet/v*/ from.
    /// Defaults to CWD.
    #[arg(long, default_value = ".")]
    pub root: PathBuf,
    /// Explicit RPC base url, repeatable (overrides discovery).
    #[arg(long = "rpc")]
    pub rpc: Vec<String>,
    /// Explicit events.ndjson path, repeatable (overrides discovery).
    #[arg(long = "event-log")]
    pub event_log: Vec<PathBuf>,
    /// Non-interactive: collect ~3s then print AppState and exit 0.
    #[arg(long)]
    pub snapshot: bool,
    /// With --snapshot, also print JSON.
    #[arg(long)]
    pub json: bool,
    /// Collection window for --snapshot, ms.
    #[arg(long, default_value_t = 3000)]
    pub snapshot_ms: u64,
    /// UI redraw tick, ms.
    #[arg(long, default_value_t = 250)]
    pub tick_ms: u64,
    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Launch suwappu-loadgen against the devnet and stream into the LOAD panel.
    Load(LoadArgs),
}

#[derive(Parser, Debug)]
pub struct LoadArgs {
    #[arg(long, default_value_t = 400)]
    pub rate: u64,
    /// seconds; if absent, run --continuous until quit.
    #[arg(long)]
    pub duration: Option<u64>,
    #[arg(long, default_value_t = 100)]
    pub batch_size: u64,
    #[arg(long, default_value_t = 1)]
    pub amount: u64,
}

/// One resolved RPC endpoint.
#[derive(Debug, Clone)]
pub struct RpcTarget { pub id: u32, pub url: String }

/// Fully resolved runtime configuration shared by sources + UI.
#[derive(Debug, Clone)]
pub struct Config {
    /// resolved devnet dir (root/target/devnet) if it exists.
    pub devnet_dir: Option<PathBuf>,
    pub rpc_targets: Vec<RpcTarget>,
    pub event_logs: Vec<PathBuf>,
    /// node count N = max(rpc_targets, event_logs, discovered v* dirs).
    pub node_count: usize,
    pub network_id: Option<String>,
    /// for loadgen: client TCP "host:port" targets, ordered by node id.
    pub client_targets: Vec<String>,
    /// faucet key paths (mldsa.sk, mldsa.pk) if discovered.
    pub faucet_sk: Option<PathBuf>,
    pub faucet_pk: Option<PathBuf>,
    pub tick_ms: u64,
    pub snapshot_ms: u64,
}

impl Cli {
    /// Resolve a Config from flags + auto-discovery.
    /// Discovery: if --rpc/--event-log given, use them; else scan
    /// `root/target/devnet/v*/` for events.ndjson and infer rpc/client ports
    /// from the v-index. Read network_id from root/target/devnet/genesis.toml.
    /// Locate faucet keys at root/target/devnet/faucet/mldsa.{sk,pk}.
    pub fn resolve(&self) -> Result<Config>;
}

/// Parse "v<i>" -> i. Returns None if not of that form.
pub fn parse_node_index(self_id: &str) -> Option<u32>;
```

---

## src/sources/rpc.rs — RPC poller (depends on: state.rs, cli.rs)

Uniform source signature. Polls every second on its own thread.

```rust
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;
use crate::cli::RpcTarget;
use crate::state::Msg;

/// Spawn the RPC poll thread. Polls each target ~1/s:
///   - suwappu_getEpoch on the first reachable target -> Msg::Epoch
///   - suwappu_getAuthorityRegistry + suwappu_getValidatorRegistry -> Msg::Registries
///   - per target reachability -> Msg::RpcReachable
/// Uses ureq with a short timeout. NEVER emits Committed/round counts
/// (RPC latest_committed_round is intentionally ignored — events are truth).
/// Stops when `tx` send fails (UI gone). Errors -> Msg::SourceError, no panic.
pub fn spawn(targets: Vec<RpcTarget>, tx: Sender<Msg>) -> JoinHandle<()>;

/// POST a JSON-RPC 2.0 call, return the `result` value.
pub fn rpc_call(url: &str, method: &str, params: serde_json::Value)
    -> anyhow::Result<serde_json::Value>;
```

---

## src/sources/events.rs — events.ndjson tailer (depends on: state.rs, cli.rs)

```rust
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;
use crate::state::Msg;

/// Spawn ONE tailer thread per event log. Each thread:
///   - opens the file, seeks to end? NO — reads from start so committed_total
///     and uptime reflect the full run (snapshot needs history). For TUI on a
///     huge file this is acceptable on local devnet; see `from_start`.
///   - follows the file (poll for appended bytes ~100ms), parses each complete
///     line as one Event, maps to Msg, sends.
/// Mapping:
///   first line from a node       -> Msg::NodeFirstSeen
///   every line                   -> Msg::NodeTick
///   lane=main event=committed    -> Msg::Committed{ intent_hashes }
///   lane=main event=received     -> Msg::PeerRecv{ peer }
///   lane=ltp  event=verified|..  -> Msg::Ltp
///   lane=fastpath signed|..      -> Msg::Fastpath
/// Unknown lanes/events -> NodeTick only. Bad JSON -> skip (no panic).
pub fn spawn(event_logs: Vec<PathBuf>, tx: Sender<Msg>, from_start: bool)
    -> Vec<JoinHandle<()>>;

/// Raw NDJSON line shape (serde). All fields except t_ms/region/lane/event
/// optional.
#[derive(serde::Deserialize)]
pub struct EventLine {
    pub t_ms: u64,
    pub region: String,         // self id e.g. "v0"
    pub lane: String,           // main|fastpath|ltp|client
    pub event: String,
    pub round: Option<u64>,
    pub cert_hash: Option<String>,
    pub tx_hash: Option<String>,
    pub peer: Option<String>,
    pub intent_hashes: Option<Vec<String>>,
    pub authority_id: Option<u32>,
    pub kind: Option<String>,
}
```

---

## src/sources/loadgen.rs — suwappu-loadgen subprocess (depends on: state.rs, cli.rs)

```rust
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;
use crate::cli::{Config, LoadArgs};
use crate::state::Msg;

/// Spawn suwappu-loadgen as a child process and stream its CSV stdout into Msgs.
/// Command (release binary, fall back to debug):
///   suwappu-loadgen --targets <client_targets joined ,> --rate R
///     [--duration D | --continuous] --batch-size B --amount A
///     --network-id <cfg.network_id>
///     --mldsa-secret-key <cfg.faucet_sk> --mldsa-public-key <cfg.faucet_pk>
/// On launch: send Msg::LoadStarted{ rate }.
/// stdout: skip the "client_submitted_ms,tx_hash,target_idx" header; for each
///   data line send Msg::LoadAck{ tx_hash: lowercased, send_ms }.
/// On child exit: send Msg::LoadEnded. Errors -> Msg::SourceError.
/// Returns the reader thread handle. The child is killed on Handle drop via
/// the returned `LoadgenChild`.
pub fn spawn(cfg: &Config, args: &LoadArgs, tx: Sender<Msg>)
    -> anyhow::Result<LoadgenChild>;

/// Owns the child process; killing it on drop / via stop().
pub struct LoadgenChild {
    pub handle: JoinHandle<()>,
    // child kept internally
}

impl LoadgenChild {
    /// Kill the loadgen child (e.g. on quit).
    pub fn stop(&mut self);
}

/// Resolve the suwappu-loadgen binary path: prefer
/// <root>/target/release/suwappu-loadgen, else target/debug, else "suwappu-loadgen".
pub fn loadgen_bin(root: &std::path::Path) -> std::path::PathBuf;
```

---

## src/ui/panels.rs — rendering only (depends on: state.rs)

Pure `&AppState` reads. No mutation, no I/O. ratatui 0.28 widgets.

```rust
use ratatui::Frame;
use crate::state::AppState;

/// Draw the full dashboard into the frame: header, NODES table, CONSENSUS,
/// LTP/FASTPATH, LOAD (only when state.load.active), and a status line showing
/// state.last_error. Lays out with ratatui Layout matching the approved
/// wireframe.
pub fn draw(f: &mut Frame, state: &AppState);

// Internal helpers (not part of the cross-file contract; keep private):
//   fn header(...)  fn nodes_table(...)  fn consensus_panel(...)
//   fn ltp_fastpath_panel(...)  fn load_panel(...)  fn peer_mesh_bar(...)
```

---

## src/main.rs — entrypoint (depends on: ALL)

```rust
fn main() -> anyhow::Result<()>;
```

Responsibilities:

1. Parse `Cli`, call `cli.resolve()` -> `Config`.
2. Build `mpsc::channel::<Msg>()`. Spawn `rpc::spawn`, `events::spawn`.
3. If `Cmd::Load(args)`: also `loadgen::spawn`. If invoked under `--snapshot`
   with `load`, still works (snapshot consumer).
4. Create `AppState::new(cfg.node_count, cfg.network_id)`.
5. **Snapshot path** (`cli.snapshot`): drain channel in a loop until
   `cfg.snapshot_ms` elapsed (applying every Msg), then `println!` the
   `snapshot_text()` and, if `--json`, `serde_json::to_string_pretty(&state)`.
   Return Ok(()) — NEVER enter raw mode.
6. **TUI path**: enter crossterm raw mode + alternate screen, loop:
   drain all pending Msgs (`try_recv`) → `apply`; on each `tick_ms` call
   `state.touch_now(now)` and `panels::draw`; handle `q`/Ctrl-C to quit and
   `l` to toggle a default load run. On exit, restore terminal in a guard so a
   panic never leaves the terminal broken.

---

## CLI summary (for the StructuredOutput `cli` field)

```
suwappu-top [--root <dir>] [--rpc <url>]... [--event-log <path>]...
        [--snapshot] [--json] [--snapshot-ms <ms>] [--tick-ms <ms>]
suwappu-top load --rate <tps> [--duration <s>] [--batch-size <n>] [--amount <n>]
        (+ all global flags; e.g. `suwappu-top --snapshot load --rate 400`)
```
