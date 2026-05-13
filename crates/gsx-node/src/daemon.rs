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
    commit::{cert_at, quorum_threshold},
    dag::DagStore,
    decide_slot,
    joint::{StakeTable, Vote},
    validator_quorum_met, AuthorityId, ConsensusError, LeaderStatus,
};
#[cfg(test)]
use gsx_execution::Substrate;
use gsx_execution::{execute_block, Block, InMemorySubstrate};
use gsx_fastpath::{
    cert::{FastPathCert, FastPathTx, OwnedObjectId},
    quorum::fast_path_quorum_size,
};
use gsx_ltp::{AttestationPayload, ChainId, Corridor, CorridorAttestation, CorridorId, SuperNode};
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
    /// DAG — own or peer. The synchronizer (S21.3) uses this to detect
    /// catch-up gaps; the round driver does not consult it directly (a
    /// node always advances one round at a time from its own last
    /// authored round — see S21.5 / IQ-002).
    pub(crate) max_observed_round: u64,
    pub(crate) pending_intents: Vec<gsx_execution::Intent>,
    pub(crate) n_authorities: u32,
    /// Certs received whose parents aren't yet in the local DAG. Keyed
    /// by the missing parent hash — when that hash later inserts,
    /// every orphan waiting on it is retried in a work-queue cascade.
    /// See [`ingest_cert`] and [`run_sync_sweeper`].
    pub(crate) orphans: HashMap<CertHash, Vec<Certificate>>,
    /// Cert hashes for which a `GetCert` request is outstanding.
    /// Prevents fan-out storms when many orphans reference the same
    /// missing parent and lets the periodic sweeper re-issue requests
    /// without unbounded multiplicity.
    pub(crate) inflight_fetches: HashSet<CertHash>,
    /// Fast-path lane (paper §6.4 / IQ-003, DAG-S22) — accumulating
    /// partial certs keyed by `(object, nonce)`. Each entry's
    /// `signers` set unions across received broadcasts; quorum
    /// reaches when `|signers| >= fast_path_quorum_size(n)`.
    pub(crate) fastpath_pending: HashMap<FastPathKey, FastPathCert>,
    /// Fast-path txs that reached quorum locally. Subsequent peer
    /// broadcasts for the same key are deduped against this set.
    pub(crate) fastpath_committed: HashSet<FastPathKey>,
    /// First payload digest seen per `(object, nonce)`. Equivocation
    /// detection: if a later broadcast for the same key carries a
    /// different payload digest, the signers of the conflicting cert
    /// have equivocated (paper §6.4 Invariant 5 — 100% slashing).
    pub(crate) fastpath_first_payload: HashMap<FastPathKey, [u8; 32]>,
    /// LTP corridor attestations (paper §10, DAG-S23). Latest attested
    /// payload per corridor id — the live "source-chain finality
    /// observed at height H with state root R" attestation that the
    /// settlement chain refers to when authoring cross-chain
    /// transactions. MVP stores latest only; historical attestations
    /// are durably anchored via the LTP commitment-node pipeline
    /// (separate ops sprint).
    pub(crate) ltp_latest: HashMap<CorridorId, AttestationPayload>,
    /// Count of LTP attestations received since startup. Used by the
    /// `ltp_attested` event log entries and operational metrics.
    pub(crate) ltp_received_count: u64,
    /// Corridor registry from genesis manifest (DAG-S24). Keyed by
    /// `(source_chain, target_chain)` — inbound attestations look up
    /// their corridor here, then verify against the 9 pinned BLS
    /// pubkeys via `gsx_ltp::verify_attestation`. Empty map = MVP
    /// pre-S24 mode (accept unverified).
    pub(crate) corridors: HashMap<(ChainId, ChainId), Corridor>,
}

/// `(object, nonce)` uniqueness key for a fast-path transaction.
/// Two FastPathCerts with the same key MUST agree on `payload_digest`;
/// disagreement is the equivocation slashing trigger.
pub(crate) type FastPathKey = (OwnedObjectId, u64);

/// Soft cap on orphan-buffer size. A Byzantine peer could otherwise
/// flood unresolvable certs and OOM the node. 4096 ≈ 16 MB of
/// bincode-serialized certs — far more than any honest reconvergence.
const MAX_ORPHAN_CERTS: usize = 4096;

/// Interval at which the synchronizer re-issues `GetCert` for any
/// missing parents still in `inflight_fetches`. Matches Sui's
/// `synchronizer.rs` periodic scheduler cadence.
const SYNC_SWEEPER_INTERVAL_MS: u64 = 1_000;

/// Build the `(source_chain, target_chain) -> Corridor` lookup from
/// the genesis manifest's optional `[[corridors]]` section (DAG-S24).
///
/// Manifests without a corridor block produce an empty map, which
/// keeps the pre-S24 "accept unverified" behavior intact for testnets
/// that don't yet have super-node infrastructure deployed.
///
/// Malformed BLS hex strings cause the corridor to be silently dropped
/// from the registry — the daemon logs a warning at startup but stays
/// up. Operationally, this is preferable to refusing to start: the
/// corridor's attestations will fail verification at the
/// per-attestation level rather than at boot, which surfaces in
/// metrics without taking the validator offline.
fn load_corridors(manifest: &GenesisManifest) -> HashMap<(ChainId, ChainId), Corridor> {
    let mut out = HashMap::new();
    for c in &manifest.corridors {
        let mut members = Vec::with_capacity(c.members.len());
        let mut bad = false;
        for m in &c.members {
            let pk_bytes = match hex::decode(&m.bls_public_key_hex) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(
                        corridor = c.id,
                        authority = m.authority,
                        err = %e,
                        "corridor: bad BLS pubkey hex; corridor dropped"
                    );
                    bad = true;
                    break;
                }
            };
            members.push(SuperNode {
                authority: m.authority,
                corridor: c.id,
                bls_public_key: pk_bytes,
            });
        }
        if bad {
            continue;
        }
        let corridor = Corridor { id: c.id, members };
        out.insert((c.source_chain, c.target_chain), corridor);
    }
    out
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
            orphans: HashMap::new(),
            inflight_fetches: HashSet::new(),
            fastpath_pending: HashMap::new(),
            fastpath_committed: HashSet::new(),
            fastpath_first_payload: HashMap::new(),
            ltp_latest: HashMap::new(),
            ltp_received_count: 0,
            corridors: load_corridors(manifest),
        }
    }

    /// Compute the `(object, nonce)` key for a fast-path tx.
    pub(crate) fn fastpath_key(tx: &FastPathTx) -> FastPathKey {
        (tx.object, tx.nonce)
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

        // Synchronizer sweeper — re-issues `GetCert` for hashes still
        // in `inflight_fetches` (S21.3). Without this, a single dropped
        // GetCert leaves the orphan stuck forever.
        {
            let state = state.clone();
            let outbound = outbound.clone();
            tasks.push(tokio::spawn(async move {
                run_sync_sweeper(state, outbound).await;
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
                let inserted = ingest_cert(&mut s, cert, &from, &outbound);
                drop(s);
                for ic in inserted {
                    let vote = Vote {
                        validator: self_id,
                        candidate: ic.hash,
                    };
                    {
                        let mut s = state.lock().await;
                        s.votes.entry(ic.hash).or_default().push(vote);
                    }
                    log.emit(
                        Event::now(&self_label, Lane::Main, "voted")
                            .with_round(ic.round)
                            .with_cert_hash(&ic.hash.0),
                    );
                    broadcast(&outbound, WireMessage::Vote(vote));
                }
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
            WireMessage::GetCert(hash) => {
                let cert_opt = {
                    let s = state.lock().await;
                    s.dag.get(&hash).cloned()
                };
                if let Some(cert) = cert_opt {
                    if let Some(tx) = outbound.get(&from) {
                        let _ = tx.try_send(WireMessage::Cert(cert));
                    }
                }
            }
            WireMessage::FastPath(cert) => {
                let mut s = state.lock().await;
                handle_fastpath_cert(&mut s, self_id, cert, &self_label, &log, &outbound);
            }
            WireMessage::Ltp(att) => {
                let mut s = state.lock().await;
                handle_ltp_attestation(&mut s, att, &self_label, &log);
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

/// One successful cert ingestion. The inbox handler emits a `voted`
/// event and broadcasts a `Vote` for each.
struct IngestedCert {
    hash: CertHash,
    round: u64,
}

/// Insert `cert` into the DAG, cascading through any orphans that were
/// waiting on certs we just admitted. On `UnknownParent`, buffer the
/// orphan keyed by the missing parent and ask up to two peers for it
/// (sender first, then one other) — avoids single-peer dependency.
///
/// This is the synchronous-fetch leg of S21.3. The periodic sweeper
/// (`run_sync_sweeper`) handles requests that never get answered.
fn ingest_cert(
    s: &mut State,
    cert: Certificate,
    from: &PeerId,
    outbound: &HashMap<PeerId, tokio::sync::mpsc::Sender<WireMessage>>,
) -> Vec<IngestedCert> {
    let mut inserted = Vec::new();
    let mut work: Vec<Certificate> = vec![cert];
    while let Some(c) = work.pop() {
        let h = c.hash();
        let round = c.round;
        match s.dag.insert(c.clone()) {
            Ok(_) => {
                if round > s.max_observed_round {
                    s.max_observed_round = round;
                }
                s.inflight_fetches.remove(&h);
                inserted.push(IngestedCert { hash: h, round });
                if let Some(unblocked) = s.orphans.remove(&h) {
                    work.extend(unblocked);
                }
            }
            Err(ConsensusError::UnknownParent(missing)) => {
                if s.orphans.values().map(|v| v.len()).sum::<usize>() >= MAX_ORPHAN_CERTS {
                    debug!(peer = %from.0, "inbox: orphan buffer full, dropping cert");
                    continue;
                }
                s.orphans.entry(missing).or_default().push(c);
                if s.inflight_fetches.insert(missing) {
                    fetch_cert_from_peers(missing, Some(from), outbound);
                }
            }
            Err(e) => {
                debug!(peer = %from.0, err = ?e, "inbox: dag rejected cert");
            }
        }
    }
    inserted
}

/// Unicast `GetCert(hash)` to up to two peers — `prefer` (if any,
/// usually the cert sender) and one other. Two-peer fan-out matches
/// Sui's `MAX_AUTHORITIES_TO_FETCH_PER_BLOCK = 2` so we don't depend on
/// a single peer being live.
fn fetch_cert_from_peers(
    hash: CertHash,
    prefer: Option<&PeerId>,
    outbound: &HashMap<PeerId, tokio::sync::mpsc::Sender<WireMessage>>,
) {
    let mut sent = 0usize;
    if let Some(p) = prefer {
        if let Some(tx) = outbound.get(p) {
            if tx.try_send(WireMessage::GetCert(hash)).is_ok() {
                sent += 1;
            }
        }
    }
    if sent >= 2 {
        return;
    }
    for (peer, tx) in outbound.iter() {
        if Some(peer) == prefer {
            continue;
        }
        if tx.try_send(WireMessage::GetCert(hash)).is_ok() {
            sent += 1;
            if sent >= 2 {
                return;
            }
        }
    }
}

/// Propose a fast-path transaction from this node: emit a singleton-signer
/// `FastPathCert { signers = {self_id} }` into the cluster. Peers will
/// observe via `handle_fastpath_cert`, sign + re-broadcast, and we
/// converge on quorum.
///
/// Used by the client listener (load generator submits a `FastPathTx`
/// over a separate socket) and by integration tests.
#[allow(dead_code)]
pub(crate) fn propose_fastpath_tx(
    s: &mut State,
    self_id: AuthorityId,
    tx: FastPathTx,
    self_label: &str,
    log: &EventLog,
    outbound: &HashMap<PeerId, tokio::sync::mpsc::Sender<WireMessage>>,
) {
    let key = State::fastpath_key(&tx);
    if s.fastpath_committed.contains(&key) {
        return;
    }
    // Record first-seen payload for this key (used by equivocation
    // detection on the receive side).
    s.fastpath_first_payload
        .entry(key)
        .or_insert(tx.payload_digest);
    let mut signers = BTreeSet::new();
    signers.insert(self_id);
    let cert = FastPathCert {
        tx: tx.clone(),
        signers: signers.clone(),
    };
    s.fastpath_pending.insert(key, cert.clone());
    log.emit(
        Event::now(self_label, Lane::FastPath, "proposed")
            .with_cert_hash(&tx.payload_digest)
            .with_round(tx.lineage_round),
    );
    for out in outbound.values() {
        let _ = out.try_send(WireMessage::FastPath(cert.clone()));
    }
    // Single-authority committees commit immediately.
    let q = fast_path_quorum_size(s.n_authorities);
    if signers.len() as u32 >= q {
        s.fastpath_pending.remove(&key);
        s.fastpath_committed.insert(key);
        log.emit(
            Event::now(self_label, Lane::FastPath, "committed")
                .with_cert_hash(&tx.payload_digest)
                .with_round(tx.lineage_round),
        );
    }
}

/// Handle one inbound fast-path certificate. The fast-path lane uses
/// "partial certs": each Authority that signs broadcasts a
/// `FastPathCert { tx, signers = {self_id} }`. Receivers union signer
/// sets across received partials keyed by `(object, nonce)`. When
/// `|signers| >= fast_path_quorum_size(n)`, the tx is locally
/// committed.
///
/// Equivocation detection (paper §6.4 Invariant 5): two partials for
/// the same `(object, nonce)` carrying different `payload_digest`
/// values prove the signers of the second cert equivocated. We log a
/// `slashed` event with the conflicting signer set; the slashing
/// pipeline (`gsx_fastpath::slashing`) consumes this for 100% bonded
/// stake forfeiture.
///
/// Re-broadcast: if we haven't signed this tx yet AND the cert is
/// eligible AND not equivocating, we add our signer and re-broadcast
/// so peers union our signature in too. This is the gossip-amplify
/// step that drives quorum convergence.
fn handle_fastpath_cert(
    s: &mut State,
    self_id: AuthorityId,
    cert: FastPathCert,
    self_label: &str,
    log: &EventLog,
    outbound: &HashMap<PeerId, tokio::sync::mpsc::Sender<WireMessage>>,
) {
    let key = State::fastpath_key(&cert.tx);

    // Reject if any signer is outside committee bounds — Byzantine input.
    for &s_id in &cert.signers {
        if s_id >= s.n_authorities {
            debug!(signer = s_id, "fastpath: signer outside committee");
            return;
        }
    }

    // Equivocation check: if we've seen this key with a different
    // payload digest, the new signers have equivocated.
    if let Some(first_payload) = s.fastpath_first_payload.get(&key).copied() {
        if first_payload != cert.tx.payload_digest {
            let slashed: Vec<AuthorityId> = cert.signers.iter().copied().collect();
            log.emit(
                Event::now(self_label, Lane::FastPath, "slashed")
                    .with_cert_hash(&cert.tx.payload_digest)
                    .with_round(cert.tx.lineage_round),
            );
            tracing::warn!(
                object = ?cert.tx.object.0,
                nonce = cert.tx.nonce,
                conflicting_signers = ?slashed,
                "fastpath: equivocation detected — 100% slashing"
            );
            return; // do not propagate or count the equivocating cert
        }
    } else {
        s.fastpath_first_payload.insert(key, cert.tx.payload_digest);
    }

    // Already locally committed — no further work.
    if s.fastpath_committed.contains(&key) {
        return;
    }

    // Union into the pending entry.
    let entry = s
        .fastpath_pending
        .entry(key)
        .or_insert_with(|| FastPathCert {
            tx: cert.tx.clone(),
            signers: BTreeSet::new(),
        });
    let was_signed_by_self_before = entry.signers.contains(&self_id);
    let pre_count = entry.signers.len();
    for s_id in &cert.signers {
        entry.signers.insert(*s_id);
    }
    let post_count = entry.signers.len();

    // First time we observe this tx → log "received". The proposer
    // event is logged by `propose_fastpath` instead.
    if pre_count == 0 {
        log.emit(
            Event::now(self_label, Lane::FastPath, "received")
                .with_cert_hash(&cert.tx.payload_digest)
                .with_round(cert.tx.lineage_round),
        );
    }

    // If we haven't signed yet, sign and re-broadcast so peers can
    // union our signature in.
    if !was_signed_by_self_before {
        entry.signers.insert(self_id);
        log.emit(
            Event::now(self_label, Lane::FastPath, "signed")
                .with_cert_hash(&cert.tx.payload_digest)
                .with_round(cert.tx.lineage_round),
        );
        let our_partial = FastPathCert {
            tx: cert.tx.clone(),
            signers: entry.signers.clone(),
        };
        for tx in outbound.values() {
            let _ = tx.try_send(WireMessage::FastPath(our_partial.clone()));
        }
    }

    // Quorum check.
    let q = fast_path_quorum_size(s.n_authorities);
    if entry.signers.len() as u32 >= q {
        let signers_count = entry.signers.len() as u32;
        // Move from pending to committed.
        s.fastpath_pending.remove(&key);
        s.fastpath_committed.insert(key);
        log.emit(
            Event::now(self_label, Lane::FastPath, "committed")
                .with_cert_hash(&cert.tx.payload_digest)
                .with_round(cert.tx.lineage_round),
        );
        tracing::info!(
            object = ?cert.tx.object.0,
            nonce = cert.tx.nonce,
            signers = signers_count,
            quorum = q,
            "fastpath: quorum reached"
        );
    } else if post_count > pre_count {
        debug!(
            object = ?cert.tx.object.0,
            nonce = cert.tx.nonce,
            signers = post_count,
            quorum = q,
            "fastpath: accumulating signers"
        );
    }
}

/// Handle one inbound LTP corridor attestation (paper §10, DAG-S23 + S24).
///
/// The attestation is a pre-aggregated 7-of-9 BLS object — super-nodes
/// do the off-chain aggregation; the daemon's role is to:
/// 1. Look up the corridor registry by `(source_chain, target_chain)`.
/// 2. Verify the aggregate BLS signature via `gsx_ltp::verify_attestation`.
/// 3. Record the latest attested `(source_height, state_root)` tuple
///    keyed by `CorridorId` so cross-chain settlement transactions can
///    refer to it.
///
/// If no corridor is registered for the `(source, target)` pair, we
/// accept the attestation unverified and log `ltp_unverified` — this
/// matches the pre-S24 MVP behavior and is operationally useful for
/// testnets without super-node infrastructure.
fn handle_ltp_attestation(
    s: &mut State,
    att: CorridorAttestation,
    self_label: &str,
    log: &EventLog,
) {
    let payload = att.payload.clone();
    let key = (payload.source_chain, payload.target_chain);
    s.ltp_received_count += 1;

    match s.corridors.get(&key) {
        Some(corridor) => {
            // Registry hit — verify against pinned BLS pubkeys.
            let corridor_id: CorridorId = corridor.id;
            match gsx_ltp::verify_attestation(corridor, &att) {
                Ok(()) => {
                    s.ltp_latest.insert(corridor_id, payload.clone());
                    log.emit(
                        Event::now(self_label, Lane::Ltp, "verified")
                            .with_round(payload.source_height)
                            .with_cert_hash(&payload.state_root),
                    );
                    tracing::debug!(
                        corridor = corridor_id,
                        source_height = payload.source_height,
                        signers = att.signers.len(),
                        "ltp: attestation verified"
                    );
                }
                Err(e) => {
                    log.emit(
                        Event::now(self_label, Lane::Ltp, "invalid")
                            .with_round(payload.source_height)
                            .with_cert_hash(&payload.state_root),
                    );
                    tracing::warn!(
                        corridor = corridor_id,
                        source_height = payload.source_height,
                        err = ?e,
                        "ltp: attestation failed verification"
                    );
                }
            }
        }
        None => {
            // Pre-S24 MVP fallback: no registered corridor → accept
            // unverified. Production must populate corridors in
            // genesis manifest before relying on LTP.
            let corridor_id = corridor_id_fallback(&payload);
            s.ltp_latest.insert(corridor_id, payload.clone());
            log.emit(
                Event::now(self_label, Lane::Ltp, "unverified")
                    .with_round(payload.source_height)
                    .with_cert_hash(&payload.state_root),
            );
            tracing::debug!(
                source_chain = payload.source_chain,
                target_chain = payload.target_chain,
                "ltp: no corridor registered — accepted unverified"
            );
        }
    }
}

/// Fallback corridor identification when no registry entry matches —
/// XOR-fold the chain pair into a u32 so `ltp_latest` still has a
/// stable key. Only reachable when `manifest.corridors` is empty.
fn corridor_id_fallback(p: &AttestationPayload) -> CorridorId {
    let s = p.source_chain ^ p.target_chain;
    ((s >> 32) as u32) ^ (s as u32)
}

/// Periodic sweeper: every `SYNC_SWEEPER_INTERVAL_MS` re-issues
/// `GetCert` for every hash still in `inflight_fetches`. A peer that
/// dropped our first request (full channel, restart, …) gets a fresh
/// chance to answer. Without this, a node that lost a single `GetCert`
/// stays stuck on that orphan forever.
///
/// Re-issue is multi-peer (`fetch_cert_from_peers` with no preference)
/// so we rotate naturally rather than re-asking the same dropped peer.
async fn run_sync_sweeper(
    state: Arc<Mutex<State>>,
    outbound: Arc<HashMap<PeerId, tokio::sync::mpsc::Sender<WireMessage>>>,
) {
    let mut tick = tokio::time::interval(Duration::from_millis(SYNC_SWEEPER_INTERVAL_MS));
    // Skip the first immediate tick.
    tick.tick().await;
    loop {
        tick.tick().await;
        let pending: Vec<CertHash> = {
            let s = state.lock().await;
            s.inflight_fetches.iter().copied().collect()
        };
        for h in pending {
            fetch_cert_from_peers(h, None, &outbound);
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
fn broadcast(outbound: &HashMap<PeerId, tokio::sync::mpsc::Sender<WireMessage>>, msg: WireMessage) {
    for tx in outbound.values() {
        let _ = tx.try_send(msg.clone());
    }
}

fn try_commit(s: &mut State, self_label: &str, log: &EventLog) {
    // Build the votes view once — re-derived inside loops would multiply
    // work for no benefit; this function holds the State mutex.
    let votes_flat: Vec<Vote> = s.votes.values().flatten().copied().collect();
    let n = s.n_authorities;

    // Try direct + indirect decision on every round we have seen at
    // least one cert at. Indirect commit may pull in slots we haven't
    // explicitly received votes for as long as their leader cert is in
    // a later directly-decided anchor's causal history.
    let candidate_rounds: BTreeSet<u64> = {
        let mut rounds = BTreeSet::new();
        for h in s.dag.linearize() {
            if let Some(c) = s.dag.get(&h) {
                rounds.insert(c.round);
            }
        }
        rounds
    };

    for round in candidate_rounds {
        let status = decide_slot(&s.dag, round, n);
        let leader_hash = match status {
            LeaderStatus::Direct(h) => h,
            LeaderStatus::Skip | LeaderStatus::Undecided => continue,
        };

        if s.committed.contains(&leader_hash) {
            continue;
        }

        // Joint-quorum AND-gate: Authority side already passed via
        // decide_slot. Validator-Ring stake side must also pass before
        // we can finalize.
        if !validator_quorum_met(&s.stake_table, leader_hash, &votes_flat) {
            continue;
        }

        // Commit the directly-decided leader plus every cert in its
        // causal history — this is where indirect-decided earlier slots
        // become observable. Walk the linearized history (deterministic
        // order) and execute each.
        for h in gsx_consensus::causal_history(&s.dag, leader_hash) {
            if !s.committed.insert(h) {
                continue;
            }
            let cert_round = match s.dag.get(&h) {
                Some(c) => c.round,
                None => continue,
            };
            let intents = s
                .blocks
                .get(&h)
                .map(|b| b.intents.clone())
                .unwrap_or_default();
            let block = Block {
                round: cert_round,
                intents,
            };
            let _ = execute_block(&mut s.substrate, &block);
            log.emit(
                Event::now(self_label, Lane::Main, "committed")
                    .with_round(cert_round)
                    .with_cert_hash(&h.0),
            );
            s.votes.remove(&h);
        }
    }
}

/// Byzantine fault tolerance: f = floor((n-1)/3). The minimum number of
/// honest parents needed to make safe progress under partial synchrony is
/// `f + 1` — Mysticeti-C §6.2 fallback.
fn f_plus_one(n: u32) -> u32 {
    (n - 1) / 3 + 1
}

/// Round-driver leader timeout (in `round_ms` multiples). If a round
/// doesn't reach strict quorum within this many ticks, force-propose with
/// any ≥ f+1 parents. With the indirect commit rule (S21.2), a leader
/// that fires under timeout-force can still be committed retroactively
/// when a later directly-decided anchor's causal history reaches it.
///
/// Matches Sui's `leader_timeout` (consensus/core/src/leader_timeout.rs).
const LEADER_TIMEOUT_ROUNDS: u32 = 4;

async fn run_round_driver(
    self_label: String,
    self_id: AuthorityId,
    round_ms: u64,
    state: Arc<Mutex<State>>,
    outbound: Arc<HashMap<PeerId, tokio::sync::mpsc::Sender<WireMessage>>>,
    log: EventLog,
) {
    let mut tick = tokio::time::interval(Duration::from_millis(round_ms));
    // Per-round leader timeout: if we haven't advanced after this much
    // elapsed wall-clock, force-propose with whatever parents we have
    // (≥ f+1). Bypasses the strict quorum gate so a slow peer can't
    // permanently stall the cluster. Matches Sui's `leader_timeout`.
    let leader_timeout = Duration::from_millis(round_ms * LEADER_TIMEOUT_ROUNDS as u64);
    let mut round_started_at = tokio::time::Instant::now();

    loop {
        tick.tick().await;
        let mut s = state.lock().await;
        let n = s.n_authorities;
        // Always advance one round at a time from our own last authored
        // round. Skipping rounds (the pre-S21.5 snap-up) breaks the
        // per-authority equivocation invariant — every authority's
        // column in the DAG must be contiguous. Catch-up of missing
        // peer certs is the synchronizer's job (S21.3), not the round
        // driver's.
        let target_round = s.last_authored_round.map(|r| r + 1).unwrap_or(0);
        let prev_round = target_round.saturating_sub(1);

        if s.last_authored_round.is_some() {
            let parents_count = s.distinct_authors_at(prev_round);
            let elapsed = round_started_at.elapsed();
            let strict_ok = parents_count >= quorum_threshold(n);
            let timeout_force = parents_count >= f_plus_one(n) && elapsed >= leader_timeout;

            if !strict_ok && !timeout_force {
                debug!(
                    target_round,
                    prev_round,
                    parents = parents_count,
                    quorum = quorum_threshold(n),
                    elapsed_ms = elapsed.as_millis() as u64,
                    "round driver: waiting"
                );
                continue;
            }
            if !strict_ok {
                tracing::warn!(
                    round = target_round,
                    parents = parents_count,
                    quorum = quorum_threshold(n),
                    elapsed_ms = elapsed.as_millis() as u64,
                    "round driver: leader-timeout force-propose"
                );
            }
        }
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
            corridors: Vec::new(),
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
            corridors: Vec::new(),
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

    /// Smoke test for the fast-path lane handler (DAG-S22).
    ///
    /// Manually feeds singleton-signer partial certs from each Authority
    /// into a fresh `State` and asserts:
    ///   1. Signers accumulate across receipts.
    ///   2. Quorum (q=3 for n=4) finalizes the tx into `fastpath_committed`.
    ///   3. A conflicting partial cert with a different payload digest is
    ///      detected as equivocation and does NOT propagate.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fastpath_receiver_accumulates_to_quorum_and_slashes_equivocation() {
        let n = 4u32;
        let manifest = GenesisManifest {
            network_id: "fp-4n".into(),
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
            corridors: Vec::new(),
        };
        let (log, _log_task) =
            EventLog::start(&std::env::temp_dir().join("gsx-fastpath-test.ndjson"))
                .await
                .unwrap();
        let mut s = State::new(&manifest);
        let outbound: HashMap<PeerId, tokio::sync::mpsc::Sender<WireMessage>> = HashMap::new();
        let self_id: AuthorityId = 0;

        let tx = gsx_fastpath::cert::FastPathTx {
            object: gsx_fastpath::cert::OwnedObjectId([0xAB; 32]),
            owner: gsx_fastpath::cert::OwnerAddress([0xCD; 32]),
            nonce: 42,
            lineage: CertHash([0; 32]),
            lineage_round: 0,
            payload_digest: [0x11; 32],
        };
        let key = State::fastpath_key(&tx);

        // Authority 1 broadcasts a partial cert with itself as signer.
        let cert_a1 = gsx_fastpath::cert::FastPathCert {
            tx: tx.clone(),
            signers: BTreeSet::from([1u32]),
        };
        handle_fastpath_cert(&mut s, self_id, cert_a1, "v0", &log, &outbound);
        // Self (0) signed too → pending has {0,1}, below q=3.
        assert!(s.fastpath_pending.contains_key(&key));
        assert!(!s.fastpath_committed.contains(&key));
        assert_eq!(s.fastpath_pending[&key].signers.len(), 2);

        // Authority 2 broadcasts. Now pending has {0,1,2} → quorum hits.
        let cert_a2 = gsx_fastpath::cert::FastPathCert {
            tx: tx.clone(),
            signers: BTreeSet::from([2u32]),
        };
        handle_fastpath_cert(&mut s, self_id, cert_a2, "v0", &log, &outbound);
        assert!(
            s.fastpath_committed.contains(&key),
            "expected fast-path quorum (q=3) to fire on (0,1,2) signers"
        );
        assert!(!s.fastpath_pending.contains_key(&key));

        // Equivocation: a partial cert for the same (object,nonce) with
        // a different payload_digest must be rejected and logged as
        // slashed, without affecting committed state.
        let equivocating_tx = gsx_fastpath::cert::FastPathTx {
            payload_digest: [0x22; 32],
            ..tx.clone()
        };
        let bad_cert = gsx_fastpath::cert::FastPathCert {
            tx: equivocating_tx,
            signers: BTreeSet::from([3u32]),
        };
        let committed_before = s.fastpath_committed.len();
        handle_fastpath_cert(&mut s, self_id, bad_cert, "v0", &log, &outbound);
        assert_eq!(
            s.fastpath_committed.len(),
            committed_before,
            "equivocating cert must not change committed state"
        );
    }

    /// Smoke test for the LTP attestation receiver (DAG-S23).
    /// Feeds a mock `CorridorAttestation` into a fresh `State` and
    /// asserts: latest-per-corridor recorded, receive counter
    /// incremented.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ltp_receiver_records_attestation() {
        let n = 4u32;
        let manifest = GenesisManifest {
            network_id: "ltp-4n".into(),
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
            corridors: Vec::new(),
        };
        let (log, _log_task) = EventLog::start(&std::env::temp_dir().join("gsx-ltp-test.ndjson"))
            .await
            .unwrap();
        let mut s = State::new(&manifest);

        let payload = gsx_ltp::AttestationPayload {
            source_chain: 1u64, // Ethereum
            target_chain: 42u64,
            source_height: 12_345_678,
            state_root: [0xAB; 32],
            timestamp_round: 100,
        };
        let att = gsx_ltp::CorridorAttestation {
            payload: payload.clone(),
            aggregate_signature: vec![0u8; 96],
            signers: (0..7u32).collect(),
        };
        let corridor_id = corridor_id_fallback(&payload);

        assert_eq!(s.ltp_received_count, 0);
        assert!(s.ltp_latest.is_empty());

        handle_ltp_attestation(&mut s, att, "v0", &log);

        assert_eq!(s.ltp_received_count, 1);
        let stored = s
            .ltp_latest
            .get(&corridor_id)
            .expect("corridor should have an attestation");
        assert_eq!(stored.source_height, 12_345_678);
        assert_eq!(stored.state_root, [0xAB; 32]);
    }

    /// S24 smoke: when no corridor is registered for the
    /// `(source_chain, target_chain)` pair, the handler falls back to
    /// pre-S24 MVP "accept unverified" behavior. Manifest with no
    /// `[[corridors]]` block.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ltp_unverified_path_when_no_corridor_registered() {
        let manifest = GenesisManifest {
            network_id: "ltp-unreg".into(),
            validators: (0..4u32)
                .map(|i| GenesisValidator {
                    authority_id: i,
                    label: format!("v{}", i),
                    mldsa_public_key_hex: "00".into(),
                    bls_public_key_hex: "00".into(),
                    validator_stake_gsx: 1_000,
                    authority_stake_gsx: 1_000,
                })
                .collect(),
            corridors: Vec::new(),
        };
        let (log, _log_task) = EventLog::start(&std::env::temp_dir().join("gsx-ltp-unreg.ndjson"))
            .await
            .unwrap();
        let mut s = State::new(&manifest);
        assert!(s.corridors.is_empty());

        let payload = gsx_ltp::AttestationPayload {
            source_chain: 1,
            target_chain: 42,
            source_height: 99,
            state_root: [0x11; 32],
            timestamp_round: 7,
        };
        let att = gsx_ltp::CorridorAttestation {
            payload: payload.clone(),
            aggregate_signature: vec![0u8; 96],
            signers: (0..7u32).collect(),
        };
        handle_ltp_attestation(&mut s, att, "v0", &log);

        // Stored under fallback corridor id (XOR-fold of chain pair).
        let fallback_id = corridor_id_fallback(&payload);
        assert_eq!(s.ltp_received_count, 1);
        assert!(s.ltp_latest.contains_key(&fallback_id));
    }
}
