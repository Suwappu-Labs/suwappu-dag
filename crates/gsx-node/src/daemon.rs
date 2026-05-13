//! Validator daemon — main DAG lane.
//!
//! Composes the wire transport, DAG store, joint-quorum voter, and block
//! executor into a single running process. Drives Mysticeti-C rounds on a
//! tokio interval and surfaces per-event timestamps to the [`EventLog`].
//!
//! Scope of this module is the **main lane only**: cert proposal, vote
//! handling, joint-commit, block execution. Fast-path and LTP integration
//! live in their own modules (added in follow-on commits) and tap the same
//! Wire / EventLog instances.
//!
//! Event-log lines emitted:
//!
//! - `lane=main event=proposed`  — round driver authored a cert
//! - `lane=main event=received`  — peer cert arrived
//! - `lane=main event=voted`     — local validator emitted a Vote
//! - `lane=main event=committed` — joint quorum fired; cert committed

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use gsx_consensus::{
    cert::{CertHash, Certificate},
    commit::{cert_at, leader as round_leader, quorum_threshold},
    dag::DagStore,
    joint::{joint_commit, StakeTable, Vote},
    AuthorityId,
};
#[cfg(test)]
use gsx_execution::Substrate;
use gsx_execution::{execute_block, Block, InMemorySubstrate};
use tokio::sync::Mutex;
use tracing::debug;

use crate::{
    config::{GenesisManifest, NodeConfig},
    events::{Event, EventLog, Lane},
    wire::{BlockPayload, PeerId, Wire, WireConfig, WireEvent, WireMessage, WireSplit},
};

/// Pending main-lane state. Shared between the inbox handler and the round
/// driver, both of which mutate it.
pub(crate) struct State {
    pub(crate) dag: DagStore,
    pub(crate) substrate: InMemorySubstrate,
    pub(crate) stake_table: StakeTable,
    pub(crate) votes: HashMap<CertHash, Vec<Vote>>,
    pub(crate) blocks: HashMap<CertHash, BlockPayload>,
    pub(crate) committed: HashSet<CertHash>,
    pub(crate) last_authored_round: Option<u64>,
    /// Highest round number observed in any cert inserted into the local
    /// DAG — own or peer. The round driver snaps `target_round` up to this
    /// value + 1 (Mysticeti-C "max observed round" pattern) so a slow
    /// validator catches up by skipping rounds rather than stalling at R+1
    /// of its own last authored round.
    pub(crate) max_observed_round: u64,
    pub(crate) pending_intents: Vec<gsx_execution::Intent>,
    pub(crate) n_authorities: u32,
}

impl State {
    fn new(manifest: &GenesisManifest) -> Self {
        let mut stake_table = StakeTable::new();
        for v in &manifest.validators {
            stake_table.insert(v.authority_id, v.validator_stake_gsx as u128);
        }
        let n = manifest.validators.len() as u32;
        Self {
            dag: DagStore::new(),
            substrate: InMemorySubstrate::new(),
            stake_table,
            votes: HashMap::new(),
            blocks: HashMap::new(),
            committed: HashSet::new(),
            last_authored_round: None,
            max_observed_round: 0,
            pending_intents: Vec::new(),
            n_authorities: n,
        }
    }

    /// Count distinct authors that have a cert at `round` in the local DAG.
    /// Mysticeti-C admits a cert at round R+1 once `quorum_threshold(n)`
    /// distinct authors are observed at round R.
    fn distinct_authors_at(&self, round: u64) -> u32 {
        (0..self.n_authorities)
            .filter(|a| cert_at(&self.dag, round, *a).is_some())
            .count() as u32
    }

    /// Round R parents = every cert at round R-1 the local DAG has observed.
    fn parents_for_round(&self, round: u64) -> Vec<CertHash> {
        if round == 0 {
            return Vec::new();
        }
        (0..self.n_authorities)
            .filter_map(|a| cert_at(&self.dag, round - 1, a))
            .collect()
    }

    /// Highest round R in [0, max_observed_round] at which the local DAG
    /// already has at least `threshold` distinct authors. Returns None if
    /// no round satisfies it. Used by the round driver's snap-up: rather
    /// than jumping to `max_observed_round + 1` (where we may have only
    /// one author's cert), we jump to the highest round where the parents
    /// gate is actually satisfiable.
    fn highest_round_with(&self, threshold: u32) -> Option<u64> {
        let max = self.max_observed_round;
        (0..=max)
            .rev()
            .find(|r| self.distinct_authors_at(*r) >= threshold)
    }
}

/// Running daemon handle. Drop to stop all background tasks.
pub struct Daemon {
    /// Held so the writer task survives until the daemon is dropped.
    _log_task: tokio::task::JoinHandle<()>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    /// Internal state. Exposed to crate-local code (integration tests, plus
    /// the upcoming load-generator + metrics modules that will read commit
    /// progress and inject intents).
    #[allow(dead_code)]
    pub(crate) state: Arc<Mutex<State>>,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        for t in self.tasks.drain(..) {
            t.abort();
        }
    }
}

impl Daemon {
    /// Bootstrap the daemon: wire up, load registries, spawn the round driver
    /// and inbox handler. Returns once the wire is bound and tasks are live —
    /// does not block until shutdown. Drop the returned handle to stop.
    pub async fn start(cfg: NodeConfig, manifest: GenesisManifest) -> anyhow::Result<Self> {
        manifest.validate_against(&cfg)?;
        let self_id: AuthorityId = cfg.authority_id;
        let self_label = cfg.self_id.clone();
        let round_ms = cfg.round_ms;

        let (log, log_task) = EventLog::start(&cfg.event_log_path).await?;
        let wire = Wire::start(WireConfig {
            self_id: PeerId::new(self_label.clone()),
            listen: cfg.listen,
            peers: cfg
                .peers
                .iter()
                .map(|p| (PeerId::new(p.id.clone()), p.addr))
                .collect(),
        })
        .await?;
        let WireSplit {
            inbox,
            outbound,
            tasks: mut wire_tasks,
        } = wire.split();
        let outbound = Arc::new(outbound);
        let state = Arc::new(Mutex::new(State::new(&manifest)));

        let mut tasks = Vec::new();

        // Inbox handler: per-message dispatch.
        {
            let state = state.clone();
            let outbound = outbound.clone();
            let log = log.clone();
            let self_label = self_label.clone();
            tasks.push(tokio::spawn(async move {
                run_inbox(self_label, self_id, state, outbound, log, inbox).await;
            }));
        }

        // Round driver.
        {
            let state = state.clone();
            let outbound = outbound.clone();
            let log = log.clone();
            let self_label = self_label.clone();
            tasks.push(tokio::spawn(async move {
                run_round_driver(self_label, self_id, round_ms, state, outbound, log).await;
            }));
        }

        // Take ownership of the wire's accept/dialer tasks so they're aborted
        // when this daemon is dropped.
        tasks.append(&mut wire_tasks);

        // Client listener: load generator submits intents over this socket.
        {
            let client_task = crate::client::run(
                cfg.client_listen,
                self_label.clone(),
                state.clone(),
                log.clone(),
            )
            .await?;
            tasks.push(client_task);
        }

        Ok(Self {
            _log_task: log_task,
            tasks,
            state,
        })
    }
}

async fn run_inbox(
    self_label: String,
    self_id: AuthorityId,
    state: Arc<Mutex<State>>,
    outbound: Arc<HashMap<PeerId, tokio::sync::mpsc::Sender<WireMessage>>>,
    log: EventLog,
    mut inbox: tokio::sync::mpsc::Receiver<WireEvent>,
) {
    while let Some(ev) = inbox.recv().await {
        let WireEvent { from, msg } = ev;
        match msg {
            WireMessage::Cert(cert) => {
                let h = cert.hash();
                let round = cert.round;
                log.emit(
                    Event::now(&self_label, Lane::Main, "received")
                        .with_round(round)
                        .with_cert_hash(&h.0)
                        .with_peer(from.0.clone()),
                );
                let mut s = state.lock().await;
                if s.dag.insert(cert).is_err() {
                    debug!(peer = %from.0, "inbox: duplicate or invalid cert");
                    continue;
                }
                if round > s.max_observed_round {
                    s.max_observed_round = round;
                }
                let vote = Vote {
                    validator: self_id,
                    candidate: h,
                };
                s.votes.entry(h).or_default().push(vote);
                drop(s);

                log.emit(
                    Event::now(&self_label, Lane::Main, "voted")
                        .with_round(round)
                        .with_cert_hash(&h.0),
                );
                broadcast(&outbound, WireMessage::Vote(vote));
                // After voting we may have just enabled a commit ourselves
                // (e.g. quorum_threshold reached on the round-0 leader).
                let mut s = state.lock().await;
                try_commit(&mut s, &self_label, &log);
            }
            WireMessage::Block(block) => {
                let mut s = state.lock().await;
                s.blocks.insert(block.cert_hash, block);
            }
            WireMessage::Vote(vote) => {
                let mut s = state.lock().await;
                s.votes.entry(vote.candidate).or_default().push(vote);
                try_commit(&mut s, &self_label, &log);
            }
            WireMessage::FastPath(_) | WireMessage::Ltp(_) => {
                // Lanes handled in follow-on commit. Ignored on the main lane.
            }
            WireMessage::Ping(t) => {
                if let Some(tx) = outbound.get(&from) {
                    let _ = tx.send(WireMessage::Pong(t)).await;
                }
            }
            WireMessage::Pong(_) => {}
        }
    }
}

/// Best-effort broadcast: drop on full channel, never block.
///
/// Previously this used `tx.send(...).await`, which queues if the per-peer
/// outbound mpsc is full. A single slow peer (which couldn't drain its
/// 1024-slot channel fast enough) would back-pressure the broadcasting
/// validator's round driver to a complete halt, deadlocking the whole
/// cluster's progress. `try_send` matches the "best-effort gossip" model
/// the wire transport advertises — retries happen via natural cert
/// re-broadcast at the next round.
fn broadcast(
    outbound: &HashMap<PeerId, tokio::sync::mpsc::Sender<WireMessage>>,
    msg: WireMessage,
) {
    for tx in outbound.values() {
        let _ = tx.try_send(msg.clone());
    }
}

fn try_commit(s: &mut State, self_label: &str, log: &EventLog) {
    // Find rounds with at least one voted-on cert. For each such round, ask
    // joint_commit whether the round's leader fires.
    let candidate_rounds: BTreeSet<u64> = s
        .votes
        .keys()
        .filter_map(|h| s.dag.get(h).map(|c| c.round))
        .collect();

    for round in candidate_rounds {
        let votes_flat: Vec<Vote> = s.votes.values().flatten().copied().collect();
        let Some(committed) =
            joint_commit(&s.dag, round, s.n_authorities, &s.stake_table, &votes_flat)
        else {
            continue;
        };
        if !s.committed.insert(committed) {
            continue;
        }
        let intents = s
            .blocks
            .get(&committed)
            .map(|b| b.intents.clone())
            .unwrap_or_default();
        let block = Block { round, intents };
        let _ = execute_block(&mut s.substrate, &block);
        log.emit(
            Event::now(self_label, Lane::Main, "committed")
                .with_round(round)
                .with_cert_hash(&committed.0),
        );
        s.votes.remove(&committed);
    }
}

/// Byzantine fault tolerance: f = floor((n-1)/3). The minimum number of
/// honest parents needed to make safe progress under partial synchrony is
/// `f + 1` — Mysticeti-C §6.2 fallback.
fn f_plus_one(n: u32) -> u32 {
    (n - 1) / 3 + 1
}

/// Round-driver fallback timing (in `round_ms` multiples). Three tiers:
///
/// 1. `STRICT_OK_AFTER_ROUNDS`: if `quorum_threshold` parents are observed
///    sooner, advance immediately.
/// 2. `LEADER_FALLBACK_ROUNDS`: after this elapses, accept fewer parents
///    (≥ f+1) **as long as the previous round's leader is among them**.
///    Mysticeti-C commits the round-R leader retroactively when ≥ quorum
///    round-(R+1) certs include the leader's hash as a parent — so the
///    leader-as-parent invariant is what makes commits fire under
///    partial-synchrony.
/// 3. `LEADERLESS_FALLBACK_ROUNDS`: extreme fallback when the leader itself
///    is the laggy peer. Accept ≥ f+1 parents without the leader. Some
///    round-R leaders never commit in this case, which is acceptable;
///    later rounds with a healthy leader will commit and pull the missed
///    rounds along via causal history.
const LEADER_FALLBACK_ROUNDS: u32 = 8;
const LEADERLESS_FALLBACK_ROUNDS: u32 = 32;

async fn run_round_driver(
    self_label: String,
    self_id: AuthorityId,
    round_ms: u64,
    state: Arc<Mutex<State>>,
    outbound: Arc<HashMap<PeerId, tokio::sync::mpsc::Sender<WireMessage>>>,
    log: EventLog,
) {
    let mut tick = tokio::time::interval(Duration::from_millis(round_ms));
    // Wall-clock when the round driver last advanced. Two fallback deadlines:
    // - leader fallback: f+1 parents that *include the previous leader*
    // - leaderless fallback: f+1 parents, leader can be missing
    let mut round_started_at = tokio::time::Instant::now();
    let leader_fallback_after =
        Duration::from_millis(round_ms * LEADER_FALLBACK_ROUNDS as u64);
    let leaderless_fallback_after =
        Duration::from_millis(round_ms * LEADERLESS_FALLBACK_ROUNDS as u64);

    loop {
        tick.tick().await;
        let mut s = state.lock().await;
        let n = s.n_authorities;
        // Snap-up: jump to the highest round R such that we have at least
        // f+1 parents at R, then author at R+1. This ensures the
        // parents-at-prev-round gate is satisfiable when we advance —
        // jumping naively to max_observed_round + 1 strands the snap-up at
        // the leading edge where we typically have only the one peer's
        // cert that informed us of the new round.
        let next_own = s.last_authored_round.map(|p| p + 1).unwrap_or(0);
        let next_snap = s
            .highest_round_with(f_plus_one(n))
            .map(|r| r + 1)
            .unwrap_or(0);
        let candidate_round = next_own.max(next_snap);
        let prev_round = candidate_round.saturating_sub(1);

        let target_round = if s.last_authored_round.is_none() {
            // Round 0: bootstrap, no gating.
            0u64
        } else {
            // Use the prev round of the snapped-up candidate, not just
            // last_authored - 1, so the readiness check matches the round
            // we'd actually author next.
            let parents_count = s.distinct_authors_at(prev_round);
            let prev_leader = round_leader(prev_round, n);
            let leader_observed = cert_at(&s.dag, prev_round, prev_leader).is_some();
            let elapsed = round_started_at.elapsed();

            let strict_ok = parents_count >= quorum_threshold(n);
            let leader_fallback_ok = parents_count >= f_plus_one(n)
                && leader_observed
                && elapsed >= leader_fallback_after;
            let leaderless_fallback_ok =
                parents_count >= f_plus_one(n) && elapsed >= leaderless_fallback_after;

            if !strict_ok && !leader_fallback_ok && !leaderless_fallback_ok {
                debug!(
                    candidate_round,
                    prev_round,
                    parents = parents_count,
                    quorum = quorum_threshold(n),
                    leader_observed,
                    elapsed_ms = elapsed.as_millis() as u64,
                    "round driver: waiting"
                );
                continue;
            }
            if candidate_round > next_own {
                tracing::info!(
                    skip_from = next_own,
                    skip_to = candidate_round,
                    "round driver: snap-up engaged"
                );
            }
            if !strict_ok {
                tracing::warn!(
                    round = candidate_round,
                    parents = parents_count,
                    quorum = quorum_threshold(n),
                    leader_observed,
                    mode = if leader_fallback_ok {
                        "leader-fallback"
                    } else {
                        "leaderless-fallback"
                    },
                    "round driver: fallback engaged"
                );
            }
            candidate_round
        };
        round_started_at = tokio::time::Instant::now();
        let parents = s.parents_for_round(target_round);
        let intents = std::mem::take(&mut s.pending_intents);
        let payload_digest: [u8; 32] =
            blake3::hash(&bincode::serialize(&intents).expect("intents serialize")).into();
        let cert = Certificate {
            author: self_id,
            round: target_round,
            parents,
            payload_digest,
        };
        let cert_hash = cert.hash();
        s.last_authored_round = Some(target_round);
        if target_round > s.max_observed_round {
            s.max_observed_round = target_round;
        }
        let _ = s.dag.insert(cert.clone());
        let block = BlockPayload {
            payload_digest,
            author: self_id,
            round: target_round,
            cert_hash,
            intents,
        };
        s.blocks.insert(cert_hash, block.clone());
        drop(s);

        log.emit(
            Event::now(&self_label, Lane::Main, "proposed")
                .with_round(target_round)
                .with_cert_hash(&cert_hash.0),
        );
        broadcast(&outbound, WireMessage::Block(block));
        broadcast(&outbound, WireMessage::Cert(cert));
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use super::*;
    use crate::config::{GenesisValidator, Peer};

    #[test]
    fn f_plus_one_matches_byzantine_threshold() {
        // Standard BFT: f = floor((n-1)/3).
        assert_eq!(f_plus_one(3), 1); // f=0
        assert_eq!(f_plus_one(4), 2); // f=1
        assert_eq!(f_plus_one(6), 2); // f=1 — perf testnet
        assert_eq!(f_plus_one(7), 3); // f=2 — 7-of-9 LTP corridor
        assert_eq!(f_plus_one(10), 4); // f=3
        // Strict quorum threshold should always exceed f+1.
        for n in 3u32..=20 {
            assert!(
                quorum_threshold(n) >= f_plus_one(n),
                "quorum {} < fallback {} at n={}",
                quorum_threshold(n),
                f_plus_one(n),
                n
            );
        }
    }

    // A more thorough fallback-engagement test would need artificial network
    // delay between loopback peers (e.g. tc-netem on Linux), which is
    // platform-specific and unsuitable for cargo test. The fallback path is
    // verified in production via the `f+1 fallback engaged` warning in
    // tracing output during partial-network conditions.

    /// Submit one transfer intent over the client listener, give it time to
    /// land in `state.pending_intents`, and verify it was queued.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn client_listener_accepts_intent() {
        let n = 1u32;
        let base_port: u16 = 19_500;
        let manifest = GenesisManifest {
            network_id: "client-1n".into(),
            validators: (0..n)
                .map(|i| GenesisValidator {
                    authority_id: i,
                    label: format!("v{}", i),
                    mldsa_public_key_hex: "00".into(),
                    bls_public_key_hex: "00".into(),
                    validator_stake_gsx: 1_000,
                    authority_stake_gsx: 1_000,
                })
                .collect(),
        };
        let cfg = NodeConfig {
            self_id: "v0".into(),
            authority_id: 0,
            listen: format!("127.0.0.1:{}", base_port).parse().unwrap(),
            client_listen: format!("127.0.0.1:{}", base_port + 100).parse().unwrap(),
            peers: vec![],
            round_ms: 500,
            checkpoint_cadence_rounds: 1,
            mldsa_secret_key_path: "/dev/null".into(),
            bls_secret_key_path: "/dev/null".into(),
            genesis_manifest_path: "/dev/null".into(),
            event_log_path: std::env::temp_dir().join("gsx-client-test.ndjson"),
        };
        let d = Daemon::start(cfg.clone(), manifest).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut client = crate::client::LoadGenClient::connect(cfg.client_listen)
            .await
            .unwrap();
        let intent = gsx_execution::Intent::Transfer {
            from: [1u8; 20],
            to: [2u8; 20],
            amount: 42,
        };
        let _hash = client.submit(intent).await.unwrap();
        // Give the daemon a moment to process the submission.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // The intent is briefly in pending_intents before being drained by the
        // next round tick. Either we catch it before the tick, or the next
        // round (within 500ms) carries it into the block cache.
        let s = d.state.lock().await;
        let queued_or_committed = !s.pending_intents.is_empty()
            || s.blocks.values().any(|b| {
                b.intents
                    .iter()
                    .any(|i| matches!(i, gsx_execution::Intent::Transfer { amount: 42, .. }))
            });
        assert!(queued_or_committed, "intent was not queued or blocked");
    }

    /// 4 daemons on loopback, all dialing each other. Within 3 seconds at
    /// 100 ms round cadence, every validator should have committed at least
    /// one round-0 cert, and all 4 substrates should agree on the post-commit
    /// state root.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn four_node_main_lane_commits() {
        let n = 4u32;
        let base_port: u16 = 19_000;

        let manifest = GenesisManifest {
            network_id: "test-4n".into(),
            validators: (0..n)
                .map(|i| GenesisValidator {
                    authority_id: i,
                    label: format!("v{}", i),
                    mldsa_public_key_hex: "00".into(),
                    bls_public_key_hex: "00".into(),
                    validator_stake_gsx: 1_000,
                    authority_stake_gsx: 1_000,
                })
                .collect(),
        };

        let mut daemons = Vec::new();
        for i in 0..n {
            let peers: Vec<Peer> = (0..n)
                .filter(|j| *j != i)
                .map(|j| Peer {
                    id: format!("v{}", j),
                    addr: format!("127.0.0.1:{}", base_port + j as u16)
                        .parse::<SocketAddr>()
                        .unwrap(),
                })
                .collect();
            let cfg = NodeConfig {
                self_id: format!("v{}", i),
                authority_id: i,
                listen: format!("127.0.0.1:{}", base_port + i as u16)
                    .parse::<SocketAddr>()
                    .unwrap(),
                client_listen: format!("127.0.0.1:{}", base_port + 100 + i as u16)
                    .parse::<SocketAddr>()
                    .unwrap(),
                peers,
                round_ms: 100,
                checkpoint_cadence_rounds: 1,
                mldsa_secret_key_path: "/dev/null".into(),
                bls_secret_key_path: "/dev/null".into(),
                genesis_manifest_path: "/dev/null".into(),
                event_log_path: std::env::temp_dir().join(format!("gsx-daemon-test-v{}.ndjson", i)),
            };
            let d = Daemon::start(cfg, manifest.clone()).await.unwrap();
            daemons.push(d);
        }

        // Give the network time to bring up TCP, propose round 0, vote, and
        // commit. 3s @ 100ms cadence = 30 rounds of headroom.
        tokio::time::sleep(Duration::from_secs(3)).await;

        // Assert every daemon committed at least one cert and all agree on
        // the substrate state root.
        let mut state_roots = Vec::new();
        for d in &daemons {
            let s = d.state.lock().await;
            assert!(
                !s.committed.is_empty(),
                "daemon {:?} did not commit any cert",
                s.last_authored_round
            );
            state_roots.push(s.substrate.state_root());
        }
        let first = state_roots[0];
        for r in &state_roots[1..] {
            assert_eq!(*r, first, "state roots disagree across daemons");
        }
    }
}
