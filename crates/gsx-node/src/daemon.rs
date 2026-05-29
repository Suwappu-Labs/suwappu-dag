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
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use gsx_authority::{AuthorityMember, AuthorityRegistry};
use gsx_consensus::{
    cert::{CertHash, Certificate, Round},
    commit::{cert_at, quorum_threshold},
    dag::DagStore,
    decide_slot,
    equivocation::EquivocationProof,
    joint::{StakeTable, Vote},
    validator_quorum_met, AuthorityId, ConsensusError, LeaderStatus,
};
use gsx_execution::{execute_block, Block, InMemorySubstrate, Intent, Substrate};
use gsx_fastpath::{
    binding::{is_main_lane_consistent, MainLaneTx, FAST_PATH_CONFIRMATION_K},
    cert::{FastPathCert, FastPathTx, OwnedObjectId},
    quorum::fast_path_quorum_size,
};
use gsx_ltp::{AttestationPayload, ChainId, Corridor, CorridorAttestation, CorridorId, SuperNode};
use gsx_validator::{ValidatorMember, ValidatorRegistry};
use tracing::debug;

use crate::{
    config::{GenesisManifest, NodeConfig},
    events::{Event, EventLog, Lane},
    wire::{BlockPayload, PeerId, Wire, WireConfig, WireEvent, WireMessage, WireSplit},
};

/// DAG-S31.2 shared validator state with per-field locking.
///
/// Pre-S31 every code path took one `Arc<State>` lock,
/// serialising run_inbox + run_round_driver + try_commit at the same
/// point. S30 perf-testnet measurement on ap-northeast-1 showed the
/// round driver couldn't acquire the lock often enough to author
/// rounds; commits collapsed cluster-wide.
///
/// S31.2 splits the hot fields into individual locks. Lock
/// acquisition order to avoid deadlocks:
///   `inner → dag → stake_table → authority_registry →
///    validator_registry → votes → blocks → committed`.
///
/// Daemon holds `Arc<State>` directly (no outer mutex).
pub(crate) struct State {
    pub(crate) dag: tokio::sync::RwLock<DagStore>,
    // DAG-S31.3: parking_lot::Mutex for short critical sections — these
    // three maps see only sub-microsecond HashMap/HashSet operations
    // (insert/contains/remove/clone) and were previously paying the
    // async-yield overhead of tokio::sync::Mutex for no benefit. Holding
    // these guards across .await is forbidden; every call site uses
    // them in-statement and drops the guard before the next .await.
    pub(crate) votes: parking_lot::Mutex<HashMap<CertHash, Vec<Vote>>>,
    /// Block intents indexed by cert hash. Only the `intents` vec is
    /// retained — the dead fields (`payload_digest`, `author`, `round`)
    /// are discarded at insertion time to avoid carrying wire-only
    /// metadata in long-lived memory. The `BlockPayload` struct remains
    /// intact for bincode wire serialization.
    pub(crate) blocks: parking_lot::Mutex<HashMap<CertHash, Vec<Intent>>>,
    pub(crate) committed: parking_lot::Mutex<HashSet<CertHash>>,
    pub(crate) stake_table: tokio::sync::RwLock<StakeTable>,
    pub(crate) authority_registry: tokio::sync::RwLock<AuthorityRegistry>,
    pub(crate) validator_registry: tokio::sync::RwLock<ValidatorRegistry>,
    pub(crate) inner: tokio::sync::Mutex<StateInner>,
    /// DAG-S31.4 / A3 mempool integration: priority + rate-limited
    /// queue replacing the FIFO `intent_tx` mpsc. Both client wire
    /// (`crates/gsx-node/src/client.rs::handle_conn`) and JSON-RPC
    /// (`crates/gsx-rpc/src/methods.rs::submit_intent` via the
    /// `rpc_adapter`) `Mempool::submit` after `verify_signed_intent`;
    /// the round driver pops via `drain_for_block` at block-build
    /// time. The mempool enforces per-peer leaky-bucket rate limits,
    /// content dedup, capacity floor with priority-ordered eviction,
    /// and TTL expiry. See `crates/gsx-mempool/src/lib.rs`.
    pub(crate) mempool: std::sync::Arc<gsx_mempool::Mempool>,
}

/// Cold-path fields. Pre-S31 these lived on `State` directly; now
/// grouped under one `Mutex<StateInner>` because access frequency
/// doesn't warrant individual locks.
pub(crate) struct StateInner {
    /// Execution backend, held as a trait object so the concrete
    /// substrate (in-memory today; gsx-db once its seeding/credit
    /// surface lands) is a construction-time choice, not a type baked
    /// into every consumer.
    pub(crate) substrate: Box<dyn Substrate + Send + Sync>,
    pub(crate) last_authored_round: Option<u64>,
    /// Highest round observed across own + peer certs. Used by the
    /// synchronizer (S21.3) to detect catch-up gaps.
    pub(crate) max_observed_round: u64,
    pub(crate) n_authorities: u32,
    /// Certs received whose parents aren't yet in the local DAG.
    pub(crate) orphans: HashMap<CertHash, Vec<Certificate>>,
    /// Cert hashes for which a `GetCert` request is outstanding.
    pub(crate) inflight_fetches: HashSet<CertHash>,
    /// DAG-S32: per-orphan (last_attempt_unix_ms, attempt_count).
    /// Set on first request and on every sweeper-driven retry. Removed
    /// when the orphan is finally inserted into the DAG (alongside the
    /// inflight_fetches entry, in `ingest_cert`).
    pub(crate) inflight_fetch_history: HashMap<CertHash, (u64, u32)>,
    /// Fast-path lane (DAG-S22) — accumulating partial certs.
    pub(crate) fastpath_pending: HashMap<FastPathKey, FastPathCert>,
    pub(crate) fastpath_committed: HashSet<FastPathKey>,
    pub(crate) fastpath_first_payload: HashMap<FastPathKey, [u8; 32]>,
    /// IQ-003: main-lane index for fast-path K-binding cross-check.
    /// Every `Intent::Transfer` committed on the main lane is translated
    /// into a `MainLaneTx` and appended here; the receiver consults this
    /// when a fast-path cert reaches quorum to enforce paper §6.4
    /// Invariant 5 (100% slashing for fast-path equivocation observable
    /// via main-lane ordering within the K=4 binding window).
    pub(crate) main_lane_index: Vec<MainLaneTx>,
    /// LTP corridor attestations (DAG-S23). Latest attested payload
    /// per corridor id.
    pub(crate) ltp_latest: HashMap<CorridorId, AttestationPayload>,
    pub(crate) ltp_received_count: u64,
    pub(crate) corridors: HashMap<(ChainId, ChainId), Corridor>,
    pub(crate) epoch: EpochState,
    /// Stake parked for newly-admitted authorities (DAG-S27.7);
    /// promoted into `state.stake_table` on first-cert insertion.
    pub(crate) pending_stake: BTreeMap<AuthorityId, gsx_consensus::Stake>,
    /// Issue #18: governance intents queued for application at the
    /// next epoch boundary. Applying `AdmitAuthority` / `ExitAuthority`
    /// / `EjectAuthority` at commit time caused transitional
    /// quorum-threshold asymmetry across daemons (each daemon commits
    /// at a slightly different round under jitter, so the `n=5→n=4`
    /// eject path briefly disagrees on what constitutes a valid
    /// round-completion). Draining at the epoch boundary makes the
    /// transition atomic across the mesh.
    pub(crate) pending_governance: Vec<Intent>,
    /// `(author, round) → first cert hash observed` (DAG-S30.1).
    /// Equivocation detection O(1) per insert instead of O(dag)
    /// per try_commit.
    pub(crate) seen_at: BTreeMap<(AuthorityId, Round), CertHash>,
    /// Equivocations detected at insertion time. `try_commit`
    /// drains this queue instead of re-scanning the DAG.
    pub(crate) detected_equivocations: Vec<EquivocationProof>,
    /// `round → committed cert hash` index. Populated from `try_commit`
    /// alongside the existing `state.blocks` insert so `gsx_getBlock(round)`
    /// is O(log n) instead of O(blocks) scan. `BTreeMap` (rather than
    /// `HashMap`) so range scans for explorer "next N blocks" stay cheap.
    pub(crate) blocks_by_round: BTreeMap<Round, CertHash>,
    /// `intent_hash → (round, cert_hash, index_within_block)` index for
    /// `gsx_getTransaction(hash)`. Populated alongside `blocks_by_round`
    /// from `try_commit`. `usize` is the position in `block.intents` so
    /// the explorer can resolve to a single intent cheaply.
    pub(crate) tx_to_block: HashMap<[u8; 32], (Round, CertHash, usize)>,
}

/// Round-based epoch counter for validator-set governance (DAG-S25).
#[derive(Debug, Clone, Copy)]
pub(crate) struct EpochState {
    /// Current epoch number. Starts at 0; increments at each boundary.
    pub(crate) current: u64,
    /// How many rounds per epoch — copied from `GenesisManifest::rounds_per_epoch`.
    pub(crate) rounds_per_epoch: u64,
    /// Round number at which the current epoch began. The next boundary
    /// fires when a commit lands at `last_boundary_round + rounds_per_epoch`
    /// or beyond.
    pub(crate) last_boundary_round: u64,
}

impl EpochState {
    /// Return the epoch a given round belongs to.
    pub(crate) fn epoch_for(&self, round: u64) -> u64 {
        if self.rounds_per_epoch == 0 {
            return self.current;
        }
        round / self.rounds_per_epoch
    }

    /// Should a commit at `round` trigger an epoch boundary transition?
    pub(crate) fn boundary_crossed_by(&self, round: u64) -> bool {
        self.epoch_for(round) > self.current
    }
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

/// DAG-S32: orphan-pull exponential backoff floor. Sweeper will not
/// re-issue `GetCert` for an orphan unless at least this many ms have
/// elapsed since the last attempt. Combined with the attempt-count
/// shift below this caps the retry storm a slow node receives when its
/// `inflight_fetches` grows faster than it can ingest.
const ORPHAN_PULL_BASE_BACKOFF_MS: u64 = 500;
const ORPHAN_PULL_MAX_BACKOFF_MS: u64 = 5_000;

/// Backoff for the Nth attempt (1-indexed): base * 2^(attempt-1), capped.
/// 500, 1_000, 2_000, 4_000, 5_000, 5_000, …
fn orphan_pull_backoff_ms(attempt: u32) -> u64 {
    let shift = attempt.saturating_sub(1).min(6);
    (ORPHAN_PULL_BASE_BACKOFF_MS << shift).min(ORPHAN_PULL_MAX_BACKOFF_MS)
}

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
        let mut authority_registry = AuthorityRegistry::new();
        let mut validator_registry = ValidatorRegistry::new();
        for v in &manifest.validators {
            stake_table.insert(v.authority_id, v.validator_stake_gsx as u128);
            let mldsa_bytes = hex::decode(&v.mldsa_public_key_hex).unwrap_or_default();
            let _bls_bytes = hex::decode(&v.bls_public_key_hex).unwrap_or_default();
            if let Err(e) = authority_registry.admit(AuthorityMember {
                id: v.authority_id,
                stake_gsx: v.authority_stake_gsx,
                public_key_bytes: mldsa_bytes.clone(),
            }) {
                tracing::warn!(auth = v.authority_id, err = %e, "genesis: skipping malformed authority");
            }
            if let Err(e) = validator_registry.admit(ValidatorMember {
                id: v.authority_id,
                stake_gsx: v.validator_stake_gsx as u128,
                public_key_bytes: mldsa_bytes,
            }) {
                tracing::warn!(val = v.authority_id, err = %e, "genesis: skipping malformed validator");
            }
        }
        // Apply pre-genesis balances to the substrate before round 0.
        let mut substrate = InMemorySubstrate::new();
        if !manifest.prebalances.is_empty() {
            let allocations: Vec<(gsx_execution::Address, gsx_execution::Balance)> = manifest
                .prebalances
                .iter()
                .filter_map(|b| {
                    let trimmed = b
                        .address
                        .strip_prefix("0x")
                        .or_else(|| b.address.strip_prefix("0X"))
                        .unwrap_or(&b.address);
                    let bytes = match hex::decode(trimmed) {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::warn!(address = %b.address, err = %e, "genesis: skipping malformed prebalance address");
                            return None;
                        }
                    };
                    let addr: [u8; 20] = match bytes.as_slice().try_into() {
                        Ok(a) => a,
                        Err(_) => {
                            tracing::warn!(address = %b.address, len = bytes.len(), "genesis: skipping prebalance address — expected 20 bytes");
                            return None;
                        }
                    };
                    Some((addr, b.balance_gsx as u128))
                })
                .collect();
            if let Err(e) = substrate.apply_intent(&Intent::GenesisAllocation {
                allocations: allocations.clone(),
            }) {
                tracing::error!(err = %e, "genesis: failed to apply prebalances");
            } else {
                tracing::info!(entries = allocations.len(), "genesis: applied prebalances");
            }
        }

        let n = manifest.validators.len() as u32;
        Self {
            dag: tokio::sync::RwLock::new(DagStore::new()),
            votes: parking_lot::Mutex::new(HashMap::new()),
            blocks: parking_lot::Mutex::new(HashMap::new()),
            committed: parking_lot::Mutex::new(HashSet::new()),
            stake_table: tokio::sync::RwLock::new(stake_table),
            authority_registry: tokio::sync::RwLock::new(authority_registry),
            validator_registry: tokio::sync::RwLock::new(validator_registry),
            inner: tokio::sync::Mutex::new(StateInner {
                // Box the genesis-prebalance-applied local substrate
                // (#267) into the `Box<dyn Substrate>` field (main). Using
                // a fresh `InMemorySubstrate::new()` here would discard the
                // applied genesis allocations.
                substrate: Box::new(substrate),
                last_authored_round: None,
                max_observed_round: 0,
                n_authorities: n,
                orphans: HashMap::new(),
                inflight_fetches: HashSet::new(),
                inflight_fetch_history: HashMap::new(),
                fastpath_pending: HashMap::new(),
                fastpath_committed: HashSet::new(),
                fastpath_first_payload: HashMap::new(),
                main_lane_index: Vec::new(),
                ltp_latest: HashMap::new(),
                ltp_received_count: 0,
                corridors: load_corridors(manifest),
                epoch: EpochState {
                    current: 0,
                    rounds_per_epoch: manifest.rounds_per_epoch,
                    last_boundary_round: 0,
                },
                pending_stake: BTreeMap::new(),
                pending_governance: Vec::new(),
                seen_at: BTreeMap::new(),
                detected_equivocations: Vec::new(),
                blocks_by_round: BTreeMap::new(),
                tx_to_block: HashMap::new(),
            }),
            mempool: std::sync::Arc::new(gsx_mempool::Mempool::new(
                gsx_mempool::MempoolConfig::default(),
            )),
        }
    }

    /// Compute the `(object, nonce)` key for a fast-path tx.
    pub(crate) fn fastpath_key(tx: &FastPathTx) -> FastPathKey {
        (tx.object, tx.nonce)
    }
}

/// Count distinct authors with a cert at `round` in the local DAG.
/// Mysticeti-C admits round R+1 once `quorum_threshold(n)` distinct
/// authors are observed at round R. Free function (was on `&State`
/// pre-S31.2) because the caller now passes the DAG read guard.
fn distinct_authors_at(dag: &DagStore, round: u64, n_authorities: u32) -> u32 {
    (0..n_authorities)
        .filter(|a| cert_at(dag, round, *a).is_some())
        .count() as u32
}

/// DAG-S30.1: check `seen_at` for an equivocation at `(author, round)`.
///
/// If no cert was previously recorded at this key, inserts `cert_hash`
/// and returns `None`. If a *different* cert was already seen, returns
/// `Some(prev_hash)` — the caller must construct the full
/// `EquivocationProof` outside the inner lock (via `record_equivocation`).
fn check_seen_at(
    seen_at: &mut BTreeMap<(AuthorityId, Round), CertHash>,
    author: AuthorityId,
    round: Round,
    cert_hash: CertHash,
) -> Option<CertHash> {
    let key = (author, round);
    match seen_at.get(&key).copied() {
        None => {
            seen_at.insert(key, cert_hash);
            None
        }
        Some(prev) if prev != cert_hash => Some(prev),
        _ => None,
    }
}

/// DAG-S30.1: construct and record a full `EquivocationProof`.
///
/// Called *outside* the inner lock to avoid holding `inner` across the
/// async `state.dag.read()` call. Looks up the previously-seen cert
/// from the DAG, pairs it with `new_cert`, and pushes the proof into
/// `detected_equivocations`.
async fn record_equivocation(
    state: &State,
    author: AuthorityId,
    round: Round,
    prev_hash: CertHash,
    new_hash: CertHash,
    new_cert: &Certificate,
) {
    if let Some(prev_cert) = state.dag.read().await.get(&prev_hash).cloned() {
        state
            .inner
            .lock()
            .await
            .detected_equivocations
            .push(EquivocationProof {
                author,
                round,
                cert_a: prev_hash,
                cert_b: new_hash,
                cert_a_signed: prev_cert,
                cert_b_signed: new_cert.clone(),
            });
    }
}

/// Round R parents = every cert at round R-1 the local DAG has observed.
fn parents_for_round(dag: &DagStore, round: u64, n_authorities: u32) -> Vec<CertHash> {
    if round == 0 {
        return Vec::new();
    }
    (0..n_authorities)
        .filter_map(|a| cert_at(dag, round - 1, a))
        .collect()
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
    pub(crate) state: Arc<State>,
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
            inboxes,
            outbound,
            tasks: mut wire_tasks,
        } = wire.split();
        let outbound = Arc::new(outbound);
        let state = Arc::new(State::new(&manifest));

        // DAG-S31.4 / A3: client + JSON-RPC intent submissions flow
        // through `state.mempool` directly. The pre-A3 `intent_tx` /
        // `intent_rx` mpsc has been retired — the mempool itself is
        // the queue (with priority ordering, per-peer leaky-bucket
        // rate limit, dedup, TTL expiry, capacity floor). Round
        // driver drains via `state.mempool.drain_for_block` at block-
        // build time.
        let mut tasks = Vec::new();

        // DAG-S31.1: per-peer inbox tasks. Pre-S31 one run_inbox task
        // multiplexed every peer's stream; on the 4-region perf testnet
        // that single tokio task saturated under inbound bursts. One
        // task per peer lets the tokio runtime spread inbox processing
        // across worker threads.
        // Load the ML-DSA-65 signing key for certificate authentication.
        // /dev/null is the explicit dev-mode sentinel (used in tests):
        // generates an ephemeral keypair. Any other path must contain a
        // valid ML-DSA-65 secret key — misconfiguration is a hard error
        // so a production node never silently runs with a wrong key.
        let self_mldsa_sk = {
            let path = &cfg.mldsa_secret_key_path;
            if path.as_os_str() == "/dev/null" {
                tracing::info!(
                    "mldsa_secret_key_path is /dev/null; using ephemeral key (dev mode)"
                );
                let (_pk, sk) = gsx_crypto::mldsa::keypair();
                Arc::new(sk)
            } else {
                let key_bytes = tokio::fs::read(path)
                    .await
                    .map_err(|e| anyhow::anyhow!("cannot read ML-DSA-65 key at {path:?}: {e}"))?;
                let sk = gsx_crypto::mldsa::SecretKey::from_bytes(&key_bytes)
                    .map_err(|e| anyhow::anyhow!("invalid ML-DSA-65 key at {path:?}: {e}"))?;
                Arc::new(sk)
            }
        };

        // DAG-S31.1: per-peer inbox tasks. Pre-S31 one run_inbox task
        // multiplexed every peer's stream; on the 4-region perf testnet
        // that single tokio task saturated under inbound bursts. One
        // task per peer lets the tokio runtime spread inbox processing
        // across worker threads.
        for (peer_id, peer_inbox) in inboxes.into_iter() {
            let state = state.clone();
            let outbound = outbound.clone();
            let log = log.clone();
            let self_label = self_label.clone();
            let peer_label = peer_id.0.clone();
            let sk = self_mldsa_sk.clone();
            let net_id = manifest.network_id.clone();
            tasks.push(tokio::spawn(async move {
                tracing::debug!(peer = %peer_label, "inbox task: starting");
                run_inbox(
                    self_label, self_id, state, outbound, log, peer_inbox, sk, net_id,
                )
                .await;
                tracing::debug!(peer = %peer_label, "inbox task: exiting");
            }));
        }

        // Round driver — drains the shared mempool at block-build time
        // (`state.mempool.drain_for_block`). No mpsc receiver to own.
        {
            let state = state.clone();
            let outbound = outbound.clone();
            let log = log.clone();
            let self_label = self_label.clone();
            let sk = self_mldsa_sk.clone();
            let net_id = manifest.network_id.clone();
            tasks.push(tokio::spawn(async move {
                run_round_driver(
                    self_label, self_id, round_ms, state, outbound, log, sk, net_id,
                )
                .await;
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

        // Client listener: load generator submits intents over this
        // socket. After ML-DSA-65 verification (#28), the intent goes
        // to `state.mempool.submit` keyed by the connection's remote
        // address — the per-peer leaky-bucket rate-limits + dedup +
        // priority queue all live inside the mempool. The round
        // driver drains at block-build time.
        {
            let limits = crate::client::ClientListenLimits::from_config(&cfg);
            let client_task = crate::client::run(
                cfg.client_listen,
                self_label.clone(),
                log.clone(),
                state.clone(),
                manifest.network_id.clone(),
                limits,
            )
            .await?;
            tasks.push(client_task);
        }

        // Issue #27 / T2: optional JSON-RPC API. Not bound unless the
        // operator configures `rpc_listen` in NodeConfig — perf testnet
        // leaves it off so peer-to-peer latency measurements aren't
        // perturbed by an external read API. T2 added the write path
        // (`gsx_submitIntent`), so the adapter now also gets the
        // cloned intent sender + network_id to drive the same
        // verify+enqueue gate the TCP wire uses.
        if let Some(rpc_addr) = cfg.rpc_listen {
            tracing::info!(addr = %rpc_addr, "gsx-rpc server starting");
            // T6: the adapter also needs an EventLog handle to spawn
            // the Event → EventView bridge. The log is already cloneable
            // (Clone for EventLog is cheap — it's just an mpsc sender +
            // broadcast sender).
            let view = crate::rpc_adapter::NodeStateView::new(
                state.clone(),
                manifest.network_id.clone(),
                &log,
            );
            let ctx = std::sync::Arc::new(gsx_rpc::RpcContext::new(std::sync::Arc::new(view)));
            let limits = gsx_rpc::RouterLimits {
                max_concurrent_requests: cfg.rpc_max_concurrent_requests,
                max_request_body_bytes: cfg.rpc_max_request_body_bytes,
                per_ip_capacity: cfg.rpc_per_ip_capacity,
                per_ip_refill_per_sec: cfg.rpc_per_ip_refill_per_sec,
                request_timeout_ms: cfg.rpc_request_timeout_ms,
            };
            let rpc_task = gsx_rpc::start_with_limits(rpc_addr, ctx, limits).await?;
            tasks.push(rpc_task);
        } else {
            tracing::info!("rpc_listen not set — JSON-RPC API disabled. Set rpc_listen in node.toml to enable.");
        }

        // G6: Prometheus text-format metrics endpoint. Off by default
        // (perf testnet doesn't run it); devnet sets metrics_listen to
        // 127.0.0.1:9093 so the local CloudWatch agent can scrape it.
        // Security group never opens 9093 — the endpoint is loopback-only.
        if let Some(metrics_addr) = cfg.metrics_listen {
            let identity = crate::metrics_http::NodeIdentity {
                region: cfg.self_id.clone(),
                authority_id: cfg.authority_id,
            };
            if let Some(handle) = crate::metrics_http::start_if_configured(
                Some(metrics_addr),
                state.clone(),
                identity,
            )
            .await?
            {
                tasks.push(handle);
            }
        }

        Ok(Self {
            _log_task: log_task,
            tasks,
            state,
        })
    }
}

// Core consensus inbox task: many independent collaborators (ids, state,
// network out, log) — a params struct would just obscure the call site.
#[allow(clippy::too_many_arguments)]
async fn run_inbox(
    self_label: String,
    self_id: AuthorityId,
    state: Arc<State>,
    outbound: Arc<HashMap<PeerId, tokio::sync::mpsc::Sender<WireMessage>>>,
    log: EventLog,
    mut inbox: tokio::sync::mpsc::Receiver<WireEvent>,
    self_mldsa_sk: Arc<gsx_crypto::mldsa::SecretKey>,
    network_id: String,
) {
    while let Some(ev) = inbox.recv().await {
        let WireEvent { from, msg } = ev;
        match msg {
            WireMessage::Cert(cert) => {
                let h = cert.hash(&network_id);
                let round = cert.round;
                log.emit(
                    Event::now(&self_label, Lane::Main, "received")
                        .with_round(round)
                        .with_cert_hash(h.as_bytes())
                        .with_peer(from.0.clone()),
                );
                let inserted = ingest_cert(&state, cert, &from, &outbound, &network_id).await;
                for ic in inserted {
                    let mut vote = Vote {
                        validator: self_id,
                        candidate: ic.hash,
                        signature: vec![],
                    };
                    crate::validator::sign_vote(&mut vote, &self_mldsa_sk, &network_id);
                    state
                        .votes
                        .lock()
                        .entry(ic.hash)
                        .or_default()
                        .push(vote.clone());
                    log.emit(
                        Event::now(&self_label, Lane::Main, "voted")
                            .with_round(ic.round)
                            .with_cert_hash(ic.hash.as_bytes()),
                    );
                    broadcast_traced(&outbound, WireMessage::Vote(vote), &self_label, &log);
                }
                try_commit(&state, &self_label, &log).await;
            }
            WireMessage::Block(block) => {
                state.blocks.lock().insert(block.cert_hash, block.intents);
            }
            WireMessage::Vote(vote) => {
                // C5 fix: verify the vote's ML-DSA-65 signature against
                // the voter's public key in the Validator Registry before
                // accepting it into the quorum set.
                let valid = {
                    let vreg = state.validator_registry.read().await;
                    crate::validator::verify_vote_signature(&vote, &vreg, &network_id)
                };
                match valid {
                    Ok(()) => {
                        state
                            .votes
                            .lock()
                            .entry(vote.candidate)
                            .or_default()
                            .push(vote);
                        try_commit(&state, &self_label, &log).await;
                    }
                    Err(e) => {
                        tracing::warn!(
                            validator = vote.validator,
                            peer = %from.0,
                            err = %e,
                            "rejecting vote: signature verification failed",
                        );
                    }
                }
            }
            WireMessage::GetCert(hash) => {
                let cert_opt = state.dag.read().await.get(&hash).cloned();
                if let Some(cert) = cert_opt {
                    if let Some(tx) = outbound.get(&from) {
                        let _ = tx.try_send(WireMessage::Cert(cert));
                    }
                }
            }
            WireMessage::FastPath(cert) => {
                handle_fastpath_cert(&state, self_id, cert, &self_label, &log, &outbound).await;
            }
            WireMessage::Ltp(att) => {
                handle_ltp_attestation(&state, att, &self_label, &log).await;
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
async fn ingest_cert(
    state: &State,
    cert: Certificate,
    from: &PeerId,
    outbound: &HashMap<PeerId, tokio::sync::mpsc::Sender<WireMessage>>,
    network_id: &str,
) -> Vec<IngestedCert> {
    let mut inserted = Vec::new();
    let mut work: Vec<Certificate> = vec![cert];
    while let Some(c) = work.pop() {
        let h = c.hash(network_id);
        let round = c.round;
        // Verify the certificate's ML-DSA-65 signature before DAG
        // insertion. Reject unsigned or mis-signed certs from peers.
        {
            let registry = state.authority_registry.read().await;
            if let Err(e) = crate::validator::verify_cert_signature(&c, &registry, network_id) {
                tracing::warn!(
                    author = c.author, round, from = %from.0, err = %e,
                    "rejecting cert: invalid signature"
                );
                continue;
            }
        }
        // Acquire dag write lock briefly for the insert.
        let insert_result = state.dag.write().await.insert(c.clone(), network_id);
        match insert_result {
            Ok(_) => {
                // Update cold-path inner state.
                let promote_stake: Option<(AuthorityId, gsx_consensus::Stake)>;
                let unblocked: Option<Vec<Certificate>>;
                let equivocation_prev: Option<CertHash>;
                {
                    let mut inner = state.inner.lock().await;
                    if round > inner.max_observed_round {
                        inner.max_observed_round = round;
                    }
                    inner.inflight_fetches.remove(&h);
                    inner.inflight_fetch_history.remove(&h);
                    // DAG-S27.7: promote pending stake on first cert.
                    promote_stake = inner.pending_stake.remove(&c.author).map(|s| (c.author, s));
                    // Issue #18 (deferred activation): if this is the
                    // first cert from a newly-admitted authority, bump
                    // `n_authorities` now. Until this moment, the new
                    // authority was in the registry (so its certs are
                    // recognized) but didn't count toward the quorum
                    // denominator — `quorum_threshold` and round-robin
                    // `leader` rotation continued using the pre-admit
                    // `n`. Pairing this bump with the pending_stake
                    // promotion guarantees the bump happens iff the
                    // authority has actually shown up on the wire.
                    if promote_stake.is_some() {
                        inner.n_authorities = inner.n_authorities.saturating_add(1);
                    }
                    // DAG-S30.1: incremental equivocation detection.
                    // check_seen_at is pure; the DAG read + proof push
                    // happens outside the inner lock via record_equivocation.
                    equivocation_prev = check_seen_at(&mut inner.seen_at, c.author, round, h);
                    unblocked = inner.orphans.remove(&h);
                }
                // Construct the full equivocation proof outside the inner
                // lock so the async DAG read doesn't hold inner.
                if let Some(prev) = equivocation_prev {
                    record_equivocation(state, c.author, round, prev, h, &c).await;
                }
                if let Some((id, stake)) = promote_stake {
                    state.stake_table.write().await.insert(id, stake);
                }
                inserted.push(IngestedCert { hash: h, round });
                if let Some(unblocked) = unblocked {
                    work.extend(unblocked);
                }
            }
            Err(ConsensusError::UnknownParent(missing)) => {
                let send_fetch;
                {
                    let mut inner = state.inner.lock().await;
                    if inner.orphans.values().map(|v| v.len()).sum::<usize>() >= MAX_ORPHAN_CERTS {
                        debug!(peer = %from.0, "inbox: orphan buffer full, dropping cert");
                        continue;
                    }
                    inner.orphans.entry(missing).or_default().push(c);
                    send_fetch = inner.inflight_fetches.insert(missing);
                    if send_fetch {
                        // DAG-S32: track first-attempt time + count so the
                        // sweeper can back off retries instead of spamming.
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0);
                        inner.inflight_fetch_history.insert(missing, (now_ms, 1));
                    }
                }
                if send_fetch {
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
pub(crate) async fn propose_fastpath_tx(
    state: &State,
    self_id: AuthorityId,
    tx: FastPathTx,
    self_label: &str,
    log: &EventLog,
    outbound: &HashMap<PeerId, tokio::sync::mpsc::Sender<WireMessage>>,
) {
    let key = State::fastpath_key(&tx);
    let mut inner = state.inner.lock().await;
    if inner.fastpath_committed.contains(&key) {
        return;
    }
    inner
        .fastpath_first_payload
        .entry(key)
        .or_insert(tx.payload_digest);
    let mut signers = BTreeSet::new();
    signers.insert(self_id);
    let cert = FastPathCert {
        tx: tx.clone(),
        signers: signers.clone(),
    };
    inner.fastpath_pending.insert(key, cert.clone());
    log.emit(
        Event::now(self_label, Lane::FastPath, "proposed")
            .with_cert_hash(&tx.payload_digest)
            .with_round(tx.lineage_round),
    );
    for out in outbound.values() {
        let _ = out.try_send(WireMessage::FastPath(cert.clone()));
    }
    let q = fast_path_quorum_size(inner.n_authorities);
    if signers.len() as u32 >= q {
        inner.fastpath_pending.remove(&key);
        inner.fastpath_committed.insert(key);
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
async fn handle_fastpath_cert(
    state: &State,
    self_id: AuthorityId,
    cert: FastPathCert,
    self_label: &str,
    log: &EventLog,
    outbound: &HashMap<PeerId, tokio::sync::mpsc::Sender<WireMessage>>,
) {
    let key = State::fastpath_key(&cert.tx);
    let mut inner = state.inner.lock().await;

    // Reject if any signer is outside committee bounds — Byzantine input.
    for &s_id in &cert.signers {
        if s_id >= inner.n_authorities {
            debug!(signer = s_id, "fastpath: signer outside committee");
            return;
        }
    }

    // Equivocation check (internal): two fast-path certs for the same
    // (object, nonce) key with different payload digests.
    if let Some(first_payload) = inner.fastpath_first_payload.get(&key).copied() {
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
            return;
        }
    } else {
        inner
            .fastpath_first_payload
            .insert(key, cert.tx.payload_digest);
    }

    // IQ-003: K-binding cross-check against the main lane (paper §6.4).
    // If a main-lane tx in the window (lineage_round, lineage_round+K]
    // touches the same object with a different payload digest, the
    // fast-path signers equivocated against the main-lane order. Emit
    // a slashing event and refuse to accumulate. The receiver re-runs
    // this check at every fast-path cert arrival; if the main-lane
    // confirmation hasn't yet caught up to lineage_round+K, the window
    // simply has fewer txs to compare against, and a later main-lane
    // commit may surface a conflict on the next arrival of this key.
    let window_snapshot: Vec<MainLaneTx> = inner
        .main_lane_index
        .iter()
        .copied()
        .filter(|ml| {
            ml.round > cert.tx.lineage_round
                && ml.round <= cert.tx.lineage_round + FAST_PATH_CONFIRMATION_K as Round
        })
        .collect();
    if !is_main_lane_consistent(&cert, &window_snapshot) {
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
            "fastpath: K-binding violation — main-lane conflict in window \
             ({}, {}] — 100% slashing",
            cert.tx.lineage_round,
            cert.tx.lineage_round + FAST_PATH_CONFIRMATION_K as Round,
        );
        return;
    }

    if inner.fastpath_committed.contains(&key) {
        return;
    }

    let entry = inner
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

    if pre_count == 0 {
        log.emit(
            Event::now(self_label, Lane::FastPath, "received")
                .with_cert_hash(&cert.tx.payload_digest)
                .with_round(cert.tx.lineage_round),
        );
    }

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

    let signers_len = entry.signers.len() as u32;
    let q = fast_path_quorum_size(inner.n_authorities);
    if signers_len >= q {
        let signers_count = signers_len;
        inner.fastpath_pending.remove(&key);
        inner.fastpath_committed.insert(key);
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
async fn handle_ltp_attestation(
    state: &State,
    att: CorridorAttestation,
    self_label: &str,
    log: &EventLog,
) {
    let payload = att.payload.clone();
    let key = (payload.source_chain, payload.target_chain);
    let mut inner = state.inner.lock().await;
    inner.ltp_received_count += 1;

    match inner.corridors.get(&key).cloned() {
        Some(corridor) => {
            let corridor_id: CorridorId = corridor.id;
            match gsx_ltp::verify_attestation(&corridor, &att) {
                Ok(()) => {
                    inner.ltp_latest.insert(corridor_id, payload.clone());
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
            let corridor_id = corridor_id_fallback(&payload);
            inner.ltp_latest.insert(corridor_id, payload.clone());
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
/// `GetCert` for orphans still in `inflight_fetches`. A peer that
/// dropped our first request (full channel, restart, …) gets a fresh
/// chance to answer. Without this, a node that lost a single `GetCert`
/// stays stuck on that orphan forever.
///
/// DAG-S32: rate-limited per-orphan via exponential backoff
/// (`orphan_pull_backoff_ms`). Pre-S32 the sweeper re-issued GetCert
/// for every entry every tick, which produced a feedback loop on slow
/// nodes — the S31 perf-testnet logged 528k `received` events on
/// ap-northeast-1 with only 81 successful ingestions because its
/// peers were re-sending the same orphans every second. With backoff
/// (500ms, 1s, 2s, 4s, 5s cap), a chronically slow consumer's retry
/// volume bounds independent of how many orphans accumulate.
///
/// Re-issue is multi-peer (`fetch_cert_from_peers` with no preference)
/// so we rotate naturally rather than re-asking the same dropped peer.
async fn run_sync_sweeper(
    state: Arc<State>,
    outbound: Arc<HashMap<PeerId, tokio::sync::mpsc::Sender<WireMessage>>>,
) {
    let mut tick = tokio::time::interval(Duration::from_millis(SYNC_SWEEPER_INTERVAL_MS));
    // Skip the first immediate tick.
    tick.tick().await;
    loop {
        tick.tick().await;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        // DAG-S32: only retry orphans whose last attempt is older than
        // their per-orphan backoff. Bump the attempt counter as we go.
        let due: Vec<CertHash> = {
            let mut inner = state.inner.lock().await;
            let mut due_now = Vec::new();
            for h in inner.inflight_fetches.iter().copied().collect::<Vec<_>>() {
                let entry = inner.inflight_fetch_history.entry(h).or_insert((0, 0));
                let (last_ms, attempts) = *entry;
                let elapsed = now_ms.saturating_sub(last_ms);
                if elapsed >= orphan_pull_backoff_ms(attempts.max(1)) {
                    *entry = (now_ms, attempts.saturating_add(1));
                    due_now.push(h);
                }
            }
            due_now
        };
        for h in due {
            fetch_cert_from_peers(h, None, &outbound);
        }
    }
}

/// DAG-S31.4: drop-aware best-effort broadcast.
///
/// Pre-S31 the silent `broadcast` used `tx.send(...).await` which
/// queues if the per-peer outbound mpsc is full; a single slow peer
/// could back-pressure the round driver to a halt. The S30 fix made
/// it `try_send` (drop on full), but drops were silent. S31.4 emits a
/// `wire_drop` event per (peer, msg_kind) when the per-peer outbound
/// channel is full or closed. The orphan-pull machinery (S21.3)
/// already handles natural replay: a peer that missed a cert this
/// way will request it back via `GetCert` once they see a child cert.
/// The event log surface is what compliance trace + the gsx-metrics
/// pair table need to attribute "missing on receive" to "dropped on
/// send" rather than "lost in transit".
fn broadcast_traced(
    outbound: &HashMap<PeerId, tokio::sync::mpsc::Sender<WireMessage>>,
    msg: WireMessage,
    self_label: &str,
    log: &EventLog,
) {
    let kind = wire_msg_kind(&msg);
    for (peer, tx) in outbound {
        if tx.try_send(msg.clone()).is_err() {
            log.emit(
                Event::now(self_label, Lane::Main, "wire_drop")
                    .with_peer(peer.0.clone())
                    .with_kind(kind),
            );
        }
    }
}

fn wire_msg_kind(msg: &WireMessage) -> &'static str {
    match msg {
        WireMessage::Cert(_) => "cert",
        WireMessage::Block(_) => "block",
        WireMessage::Vote(_) => "vote",
        WireMessage::GetCert(_) => "get_cert",
        WireMessage::FastPath(_) => "fast_path",
        WireMessage::Ltp(_) => "ltp",
        WireMessage::Ping(_) => "ping",
        WireMessage::Pong(_) => "pong",
    }
}

async fn try_commit(state: &State, self_label: &str, log: &EventLog) {
    // Snapshot votes + n_authorities + candidate_rounds in brief locks
    // up-front so the rest of the function operates on owned data.
    let votes_flat: Vec<Vote> = state.votes.lock().values().flatten().cloned().collect();
    let n = state.inner.lock().await.n_authorities;
    let candidate_rounds: BTreeSet<u64> = {
        let dag = state.dag.read().await;
        let mut rounds = BTreeSet::new();
        for h in dag.linearize() {
            if let Some(c) = dag.get(&h) {
                rounds.insert(c.round);
            }
        }
        rounds
    };

    for round in candidate_rounds {
        let status = {
            let dag = state.dag.read().await;
            decide_slot(&dag, round, n)
        };
        let leader_hash = match status {
            LeaderStatus::Direct(h) => h,
            LeaderStatus::Skip | LeaderStatus::Undecided => continue,
        };

        if state.committed.lock().contains(&leader_hash) {
            continue;
        }

        // Joint-quorum AND-gate: validator-ring stake side.
        let stake_ok = {
            let st = state.stake_table.read().await;
            validator_quorum_met(&st, leader_hash, &votes_flat)
        };
        if !stake_ok {
            continue;
        }

        let history = {
            let dag = state.dag.read().await;
            gsx_consensus::causal_history(&dag, leader_hash)
        };
        for h in history {
            if !state.committed.lock().insert(h) {
                continue;
            }
            let cert_round = match state.dag.read().await.get(&h) {
                Some(c) => c.round,
                None => continue,
            };
            let intents = state.blocks.lock().get(&h).cloned().unwrap_or_default();
            // DAG-S26.1: capture intent hashes for compliance trace.
            // Computed once and reused for the `tx_to_block` index below
            // so we don't pay blake3 twice per intent.
            let intent_hash_bytes: Vec<[u8; 32]> = intents
                .iter()
                .map(|i| {
                    let bytes = crate::codec::encode(i).expect("intent serialize");
                    *blake3::hash(&bytes).as_bytes()
                })
                .collect();
            let intent_hashes: Vec<String> = intent_hash_bytes.iter().map(hex::encode).collect();
            let block = Block {
                round: cert_round,
                intents: intents.clone(),
            };
            // Substrate execution under inner lock.
            {
                let mut inner = state.inner.lock().await;
                let _ = execute_block(&mut inner.substrate, &block);
                // IQ-003: index single-owner-equivalent main-lane txs so
                // the fast-path receiver can K-binding cross-check.
                // Skip governance/admin intents — only state-touching
                // transfers can conflict with a fast-path cert.
                for intent in &intents {
                    if let Some(ml_tx) = intent_to_main_lane_tx(intent, cert_round, h) {
                        inner.main_lane_index.push(ml_tx);
                    }
                }
                // Secondary indices for `gsx_getBlock(round)` and
                // `gsx_getTransaction(hash)`. Populated here (and only
                // here) so the indices are tight-coupled to the canonical
                // commit path — no second `try_commit` writer.
                inner.blocks_by_round.insert(cert_round, h);
                for (idx, tx_hash) in intent_hash_bytes.iter().enumerate() {
                    inner.tx_to_block.insert(*tx_hash, (cert_round, h, idx));
                }
            }

            // Issue #18: queue Phase G governance intents for
            // epoch-boundary application. Applying at commit time made
            // n_authorities update at different rounds across daemons
            // (jitter), causing transitional quorum-threshold asymmetry
            // that stalled the eject path (n=5→n=4: threshold changes
            // from 4 to 3 mid-flight). Draining at the epoch boundary
            // (below) makes governance transitions atomic across the
            // mesh. Non-governance intents (Transfer) already executed
            // via execute_block above — they are unchanged.
            {
                let mut inner = state.inner.lock().await;
                for intent in &intents {
                    if matches!(
                        intent,
                        Intent::AdmitAuthority { .. }
                            | Intent::ExitAuthority { .. }
                            | Intent::EjectAuthority { .. }
                    ) {
                        inner.pending_governance.push(intent.clone());
                    }
                }
            }

            log.emit(
                Event::now(self_label, Lane::Main, "committed")
                    .with_round(cert_round)
                    .with_cert_hash(h.as_bytes())
                    .with_intent_hashes(intent_hashes),
            );
            state.votes.lock().remove(&h);

            // Epoch boundary detection (DAG-S25 Phase G).
            // Issue #18: drains queued governance intents here so that
            // registry mutations land atomically at the boundary round.
            let boundary_crossed = {
                let mut inner = state.inner.lock().await;
                if inner.epoch.boundary_crossed_by(cert_round) {
                    let new_epoch = inner.epoch.epoch_for(cert_round);
                    inner.epoch.current = new_epoch;
                    inner.epoch.last_boundary_round = cert_round;
                    true
                } else {
                    false
                }
            };
            if boundary_crossed {
                let queued: Vec<Intent> = {
                    let mut inner = state.inner.lock().await;
                    inner.pending_governance.drain(..).collect()
                };
                for intent in &queued {
                    apply_governance_intent(state, intent, cert_round, self_label, log).await;
                }
                log.emit(
                    Event::now(self_label, Lane::Main, "epoch_boundary").with_round(cert_round),
                );
                let new_epoch = state.inner.lock().await.epoch.current;
                tracing::info!(
                    epoch = new_epoch,
                    round = cert_round,
                    drained = queued.len(),
                    "epoch boundary crossed; governance applied"
                );
            }
        }
    }

    // DAG-S30.1: drain the equivocation queue.
    let proofs: Vec<EquivocationProof> = state
        .inner
        .lock()
        .await
        .detected_equivocations
        .drain(..)
        .collect();
    for proof in proofs {
        let id = proof.author;
        if state.authority_registry.read().await.contains(id) {
            state.authority_registry.write().await.remove(id);
            state.validator_registry.write().await.remove(id);
            state.stake_table.write().await.remove(&id);
            let new_n = state.authority_registry.read().await.len() as u32;
            {
                let mut inner = state.inner.lock().await;
                inner.pending_stake.remove(&id);
                inner.n_authorities = new_n;
            }
            log.emit(Event::now(self_label, Lane::Main, "slashing_evidence").with_authority_id(id));
            log.emit(Event::now(self_label, Lane::Main, "authority_ejected").with_authority_id(id));
            tracing::warn!(
                authority = id,
                "auto-ejected on detected authority equivocation"
            );
        }
    }
}

/// IQ-003: translate a main-lane `Intent` to a `MainLaneTx` for the
/// fast-path binding window cross-check. Returns `None` for intents
/// that can't conflict with a fast-path cert (governance, etc.).
///
/// The translation models `Transfer { from, to, amount }` as a
/// single-owner state change on object `from`: the `OwnedObjectId` is
/// the sender's 20-byte address zero-padded to 32 bytes, the
/// `payload_digest` is `blake3(bincode((to, amount)))`. The fast-path
/// proposer must compute the same digest for the matching transaction
/// so the cross-check is symmetric.
fn intent_to_main_lane_tx(intent: &Intent, round: Round, lineage: CertHash) -> Option<MainLaneTx> {
    match intent {
        Intent::Transfer { from, to, amount } => {
            let mut object_bytes = [0u8; 32];
            object_bytes[..from.len()].copy_from_slice(from);
            let payload_bytes = crate::codec::encode(&(to, amount)).ok()?;
            let payload_digest: [u8; 32] = blake3::hash(&payload_bytes).into();
            Some(MainLaneTx {
                round,
                object: OwnedObjectId(object_bytes),
                payload_digest,
                lineage,
            })
        }
        // Governance / admission / ejection intents don't touch a
        // single-owner object; they can't conflict with a fast-path cert.
        // Catch-all required because `Intent` is `#[non_exhaustive]`
        // (C4) — future variants default to "no fast-path mapping".
        Intent::AdmitAuthority { .. }
        | Intent::ExitAuthority { .. }
        | Intent::EjectAuthority { .. } => None,
        _ => None,
    }
}

/// Apply a single governance Intent to State (DAG-S27.3).
/// Extracted from try_commit's body so the S31.2 per-field-lock
/// pattern stays readable. Lock acquisition order respects the
/// canonical: stake_table → authority_registry → validator_registry → inner.
async fn apply_governance_intent(
    state: &State,
    intent: &Intent,
    cert_round: u64,
    self_label: &str,
    log: &EventLog,
) {
    match intent {
        Intent::AdmitAuthority {
            authority_id,
            stake_gsx,
            mldsa_public_key,
            bls_public_key: _bls,
        } => {
            let admit_result = state
                .authority_registry
                .write()
                .await
                .admit(AuthorityMember {
                    id: *authority_id,
                    stake_gsx: *stake_gsx,
                    public_key_bytes: mldsa_public_key.clone(),
                });
            match admit_result {
                Ok(()) => {
                    // DAG-S27.7: park stake; activated on first cert.
                    state
                        .inner
                        .lock()
                        .await
                        .pending_stake
                        .insert(*authority_id, *stake_gsx as u128);
                    let _ = state
                        .validator_registry
                        .write()
                        .await
                        .admit(ValidatorMember {
                            id: *authority_id,
                            stake_gsx: *stake_gsx as u128,
                            public_key_bytes: mldsa_public_key.clone(),
                        });
                    // Issue #18 (deferred activation): the registries are
                    // grown to the new size so the new authority's certs
                    // are recognized when they arrive, but
                    // `inner.n_authorities` is INTENTIONALLY NOT bumped
                    // here. Bumping it now would (a) collapse the
                    // `quorum_threshold(n)` jump that the post-admit
                    // cluster can't meet under jitter, and (b) shift the
                    // round-robin `leader(round, n)` rotation onto an
                    // authority that hasn't yet produced any cert. We
                    // defer the bump to the first-cert ingest site
                    // (`ingest_cert`, next to the existing pending_stake
                    // promotion), where we have proof the new authority
                    // is actually participating. See the
                    // `bft-stake-denominator-deadlock-on-admit` skill for
                    // the full class of bug this avoids.
                    log.emit(
                        Event::now(self_label, Lane::Main, "authority_admitted")
                            .with_round(cert_round)
                            .with_authority_id(*authority_id),
                    );
                }
                Err(e) => {
                    tracing::warn!(auth = authority_id, err = %e, "admit rejected");
                }
            }
        }
        Intent::ExitAuthority { authority_id } => {
            let removed = state.authority_registry.write().await.remove(*authority_id);
            if removed.is_some() {
                state.validator_registry.write().await.remove(*authority_id);
                state.stake_table.write().await.remove(authority_id);
                let new_n = state.authority_registry.read().await.len() as u32;
                {
                    let mut inner = state.inner.lock().await;
                    inner.pending_stake.remove(authority_id);
                    inner.n_authorities = new_n;
                }
                log.emit(
                    Event::now(self_label, Lane::Main, "authority_exited")
                        .with_round(cert_round)
                        .with_authority_id(*authority_id),
                );
            }
        }
        Intent::EjectAuthority {
            authority_id,
            proof_ref: _proof,
        } => {
            let removed = state.authority_registry.write().await.remove(*authority_id);
            if removed.is_some() {
                state.validator_registry.write().await.remove(*authority_id);
                state.stake_table.write().await.remove(authority_id);
                let new_n = state.authority_registry.read().await.len() as u32;
                {
                    let mut inner = state.inner.lock().await;
                    inner.pending_stake.remove(authority_id);
                    inner.n_authorities = new_n;
                }
                log.emit(
                    Event::now(self_label, Lane::Main, "authority_ejected")
                        .with_round(cert_round)
                        .with_authority_id(*authority_id),
                );
            }
        }
        Intent::Transfer { .. } => {}
        // `Intent` is `#[non_exhaustive]` (C4). Future variants
        // default to no governance-side effect; substrate or
        // execution-layer code is where any new variant's logic
        // lands.
        _ => {}
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

/// Max intents drained from the mempool per block proposal.
/// Honors mempool's priority ordering (higher priority drained first).
/// Conservative — same order of magnitude as the pre-A3 try_recv loop
/// which drained whatever the channel had, capped only by the channel
/// fill rate.
const MAX_INTENTS_PER_BLOCK: usize = 4096;

// Core consensus round driver: same many-collaborator shape as run_inbox.
#[allow(clippy::too_many_arguments)]
async fn run_round_driver(
    self_label: String,
    self_id: AuthorityId,
    round_ms: u64,
    state: Arc<State>,
    outbound: Arc<HashMap<PeerId, tokio::sync::mpsc::Sender<WireMessage>>>,
    log: EventLog,
    self_mldsa_sk: Arc<gsx_crypto::mldsa::SecretKey>,
    network_id: String,
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

        // DAG-S30.2: split the propose path into 3 short lock-windows
        // with the expensive bincode/blake3 work outside the lock.
        // Pre-S30 the driver held `state.lock().await` for the entire
        // path including serialising ~1100 intents (~70 KB) and a
        // blake3 hash, ~30-50 ms wall-clock under load. run_inbox
        // couldn't acquire the lock fast enough to drain votes, so
        // the round driver's *next* tick also stalled waiting for
        // quorum-of-parents — feedback loop that capped commits at
        // 0.6/sec instead of 4/sec at ROUND_MS=250.
        //
        // Phase 1 (locked): read quorum metadata + parents.
        let target_round;
        let prev_round;
        let parents;
        {
            let inner = state.inner.lock().await;
            let n = inner.n_authorities;
            let dag = state.dag.read().await;
            target_round = inner.last_authored_round.map(|r| r + 1).unwrap_or(0);
            prev_round = target_round.saturating_sub(1);
            if inner.last_authored_round.is_some() {
                let parents_count = distinct_authors_at(&dag, prev_round, n);
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
            parents = parents_for_round(&dag, target_round, n);
        }
        round_started_at = tokio::time::Instant::now();

        // Phase 2 (unlocked): drain intents from the shared mempool
        // honoring priority ordering, then bincode + blake3 + cert
        // hash. Heaviest CPU on the whole path; run_inbox can drain
        // votes during this window. Pre-A3 this was an mpsc try_recv
        // loop; A3 routes both ingress wires (TCP + JSON-RPC) through
        // `state.mempool.submit` so per-peer rate limits + dedup +
        // capacity-floor eviction live at admission, and the round
        // driver simply pops the top-priority intents at propose time.
        let intents: Vec<gsx_execution::Intent> =
            state.mempool.drain_for_block(MAX_INTENTS_PER_BLOCK);
        let payload_digest: [u8; 32] =
            blake3::hash(&crate::codec::encode(&intents).expect("intents serialize")).into();
        let mut cert = Certificate {
            author: self_id,
            round: target_round,
            parents,
            payload_digest,
            signature: vec![],
        };
        crate::validator::sign_cert(&mut cert, &self_mldsa_sk, &network_id);
        let cert_hash = cert.hash(&network_id);

        // Per-intent hashes for the `tx_to_block` secondary index.
        // Computed BEFORE moving `intents` into the `BlockPayload`.
        // `try_commit` populates the same indices on the peer-receive
        // path; this site mirrors it for the self-propose path so
        // single-node and multi-node clusters both expose
        // `blocks_by_round` and `tx_to_block`.
        let intent_hash_bytes: Vec<[u8; 32]> = intents
            .iter()
            .map(|i| *blake3::hash(&crate::codec::encode(i).expect("intent serialize")).as_bytes())
            .collect();

        let block = BlockPayload {
            payload_digest,
            author: self_id,
            round: target_round,
            cert_hash,
            intents,
        };

        // Phase 3 (locked): brief — insert cert + block + update
        // markers + record own (author, round) for S30.1 incremental
        // equivocation detection.
        let equivocation_prev;
        {
            let mut inner = state.inner.lock().await;
            inner.last_authored_round = Some(target_round);
            if target_round > inner.max_observed_round {
                inner.max_observed_round = target_round;
            }
            inner.blocks_by_round.insert(target_round, cert_hash);
            for (idx, tx_hash) in intent_hash_bytes.iter().enumerate() {
                inner
                    .tx_to_block
                    .insert(*tx_hash, (target_round, cert_hash, idx));
            }
            equivocation_prev = check_seen_at(&mut inner.seen_at, self_id, target_round, cert_hash);
        }
        // Construct equivocation proof outside the inner lock.
        if let Some(prev) = equivocation_prev {
            record_equivocation(&state, self_id, target_round, prev, cert_hash, &cert).await;
        }
        let _ = state.dag.write().await.insert(cert.clone(), &network_id);
        state.blocks.lock().insert(cert_hash, block.intents.clone());

        // Phase 4 (unlocked): event log emit + cluster broadcast. No
        // state access, pure I/O.
        log.emit(
            Event::now(&self_label, Lane::Main, "proposed")
                .with_round(target_round)
                .with_cert_hash(cert_hash.as_bytes()),
        );
        broadcast_traced(&outbound, WireMessage::Block(block), &self_label, &log);
        broadcast_traced(&outbound, WireMessage::Cert(cert), &self_label, &log);
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use super::*;
    use crate::config::{GenesisValidator, Peer};

    /// Generate `n` ML-DSA-65 keypairs and write each secret key to a
    /// temp file. Returns `(pk_hex_vec, sk_path_vec, sk_vec)` — the pk
    /// hexes go into the `GenesisManifest`, the sk paths into
    /// `NodeConfig`, and the sk vec is available for loadgen clients.
    fn test_key_files(
        n: u32,
    ) -> (
        Vec<String>,
        Vec<std::path::PathBuf>,
        Vec<gsx_crypto::mldsa::SecretKey>,
        Vec<gsx_crypto::mldsa::PublicKey>,
    ) {
        let mut pk_hexes = Vec::with_capacity(n as usize);
        let mut sk_paths = Vec::with_capacity(n as usize);
        let mut sks = Vec::with_capacity(n as usize);
        let mut pks = Vec::with_capacity(n as usize);
        let pid = std::process::id();
        for i in 0..n {
            let (pk, sk) = gsx_crypto::mldsa::keypair();
            pk_hexes.push(hex::encode(pk.as_bytes()));
            let path = std::env::temp_dir().join(format!("gsx-test-sk-{}-{}.bin", i, pid));
            std::fs::write(&path, sk.as_bytes()).unwrap();
            sk_paths.push(path);
            pks.push(pk);
            sks.push(sk);
        }
        (pk_hexes, sk_paths, sks, pks)
    }

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

    /// Submit one transfer intent over the client listener, give the round
    /// driver a tick to drain the mpsc queue and produce a block, and verify
    /// the intent landed in a block. Pre-S27.2 this test poked
    /// `state.pending_intents` directly; that field no longer exists since
    /// intents flow over a lock-free mpsc to the round driver.
    ///
    /// Issue #28 (Phase 2.6): the listener now enforces ML-DSA-65
    /// signatures, so the test seeds a real keypair into the genesis
    /// manifest and uses the matching secret key on the client side.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn client_listener_accepts_intent() {
        let n = 1u32;
        let base_port: u16 = 19_500;
        let network_id = "client-1n".to_string();
        let (pk, sk) = gsx_crypto::mldsa::keypair();
        let pk_hex = hex::encode(pk.as_bytes());
        let manifest = GenesisManifest {
            network_id: network_id.clone(),
            validators: (0..n)
                .map(|i| GenesisValidator {
                    authority_id: i,
                    label: format!("v{}", i),
                    mldsa_public_key_hex: pk_hex.clone(),
                    bls_public_key_hex: "00".into(),
                    // Issue #28: stakes must clear AUTHORITY_STAKE_THRESHOLD_GSX
                    // (100_000) and VALIDATOR_STAKE_THRESHOLD_GSX (25_000) so
                    // AuthorityRegistry::admit succeeds — otherwise the registry
                    // stays empty and the new signature-verify path rejects
                    // every submit with `unknown signer`.
                    validator_stake_gsx: 150_000,
                    authority_stake_gsx: 150_000,
                })
                .collect(),
            corridors: Vec::new(),
            rounds_per_epoch: 1024,
            prebalances: vec![],
        };
        let cfg = NodeConfig {
            self_id: "v0".into(),
            authority_id: 0,
            listen: format!("127.0.0.1:{}", base_port).parse().unwrap(),
            client_listen: format!("127.0.0.1:{}", base_port + 100).parse().unwrap(),
            rpc_listen: None,
            peers: vec![],
            round_ms: 500,
            checkpoint_cadence_rounds: 1,
            mldsa_secret_key_path: "/dev/null".into(),
            bls_secret_key_path: "/dev/null".into(),
            genesis_manifest_path: "/dev/null".into(),
            event_log_path: std::env::temp_dir().join("gsx-client-test.ndjson"),

            max_client_connections: 256,
            client_idle_timeout_ms: 30_000,
            client_per_ip_limit: 8,
            rpc_per_ip_capacity: 60,
            rpc_per_ip_refill_per_sec: 10,
            rpc_request_timeout_ms: 30_000,
            rpc_max_request_body_bytes: 1024 * 1024,
            rpc_max_concurrent_requests: 64,
            metrics_listen: None,
        };
        let d = Daemon::start(cfg.clone(), manifest).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut client =
            crate::client::LoadGenClient::connect(cfg.client_listen, sk, pk, network_id)
                .await
                .unwrap();
        let intent = gsx_execution::Intent::Transfer {
            from: [1u8; 20],
            to: [2u8; 20],
            amount: 42,
        };
        let _hash = client.submit(intent).await.unwrap();
        // Give the round driver a full tick to drain the mpsc + propose.
        tokio::time::sleep(Duration::from_millis(700)).await;

        // Post-S27.2: intents are visible once they land in a proposed block.
        // Since we run a single-node cluster, that block is guaranteed within
        // one round_ms tick of the submit landing on the mpsc.
        let blocks = d.state.blocks.lock();
        let in_block = blocks.values().any(|b| {
            b.iter()
                .any(|i| matches!(i, gsx_execution::Intent::Transfer { amount: 42, .. }))
        });
        assert!(in_block, "intent was not carried into any block");
    }

    /// DAG-S29.2: submit a batch of N intents in one wire roundtrip
    /// and verify the daemon returns N hashes in order, and that the
    /// intents land in proposed blocks. Issue #28 (Phase 2.6): the
    /// batch path now signs every intent and verifies all signatures
    /// before pushing any onto the mpsc.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn client_listener_accepts_intent_batch() {
        let n = 1u32;
        // Stay out of every other daemon test's range:
        //   four_node_main_lane_commits   19_000-19_103
        //   client_listener_accepts_intent 19_500 + 19_600
        //   phase_g_admit_and_eject       19_700-19_803
        // 20_000 base is well clear.
        let base_port: u16 = 20_000;
        let network_id = "client-batch-1n".to_string();
        let (pk, sk) = gsx_crypto::mldsa::keypair();
        let pk_hex = hex::encode(pk.as_bytes());
        let manifest = GenesisManifest {
            network_id: network_id.clone(),
            validators: (0..n)
                .map(|i| GenesisValidator {
                    authority_id: i,
                    label: format!("v{}", i),
                    mldsa_public_key_hex: pk_hex.clone(),
                    bls_public_key_hex: "00".into(),
                    // See #28 note on the first client test — stakes must
                    // clear both ring thresholds for the registry to populate.
                    validator_stake_gsx: 150_000,
                    authority_stake_gsx: 150_000,
                })
                .collect(),
            corridors: Vec::new(),
            rounds_per_epoch: 1024,
            prebalances: vec![],
        };
        let cfg = NodeConfig {
            self_id: "v0".into(),
            authority_id: 0,
            listen: format!("127.0.0.1:{}", base_port).parse().unwrap(),
            client_listen: format!("127.0.0.1:{}", base_port + 100).parse().unwrap(),
            rpc_listen: None,
            peers: vec![],
            round_ms: 500,
            checkpoint_cadence_rounds: 1,
            mldsa_secret_key_path: "/dev/null".into(),
            bls_secret_key_path: "/dev/null".into(),
            genesis_manifest_path: "/dev/null".into(),
            event_log_path: std::env::temp_dir().join("gsx-client-batch-test.ndjson"),

            max_client_connections: 256,
            client_idle_timeout_ms: 30_000,
            client_per_ip_limit: 8,
            rpc_per_ip_capacity: 60,
            rpc_per_ip_refill_per_sec: 10,
            rpc_request_timeout_ms: 30_000,
            rpc_max_request_body_bytes: 1024 * 1024,
            rpc_max_concurrent_requests: 64,
            metrics_listen: None,
        };
        let d = Daemon::start(cfg.clone(), manifest).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut client =
            crate::client::LoadGenClient::connect(cfg.client_listen, sk, pk, network_id)
                .await
                .unwrap();
        let batch: Vec<gsx_execution::Intent> = (0..50u8)
            .map(|i| gsx_execution::Intent::Transfer {
                from: [i; 20],
                to: [i.wrapping_add(1); 20],
                amount: 99,
            })
            .collect();
        let hashes = client.submit_batch(batch).await.unwrap();
        assert_eq!(hashes.len(), 50, "ack hash count must match batch size");

        // Give the round driver a tick to drain + propose.
        tokio::time::sleep(Duration::from_millis(700)).await;

        let blocks = d.state.blocks.lock();
        let total_intents_in_blocks: usize = blocks
            .values()
            .map(|b| {
                b.iter()
                    .filter(|i| matches!(i, gsx_execution::Intent::Transfer { amount: 99, .. }))
                    .count()
            })
            .sum();
        assert!(
            total_intents_in_blocks >= 50,
            "expected ≥50 batch intents in blocks, got {}",
            total_intents_in_blocks
        );
    }

    /// 4 daemons on loopback, all dialing each other. Within 3 seconds at
    /// 100 ms round cadence, every validator should have committed at least
    /// one round-0 cert, and all 4 substrates should agree on the post-commit
    /// state root.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn four_node_main_lane_commits() {
        let n = 4u32;
        let base_port: u16 = 19_000;
        let (pk_hexes, sk_paths, _sks, _pks) = test_key_files(n);

        let manifest = GenesisManifest {
            network_id: "test-4n".into(),
            validators: (0..n)
                .map(|i| GenesisValidator {
                    authority_id: i,
                    label: format!("v{}", i),
                    mldsa_public_key_hex: pk_hexes[i as usize].clone(),
                    bls_public_key_hex: "00".into(),
                    validator_stake_gsx: 150_000,
                    authority_stake_gsx: 150_000,
                })
                .collect(),
            corridors: Vec::new(),
            rounds_per_epoch: 1024,
            prebalances: vec![],
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
                rpc_listen: None,
                peers,
                round_ms: 100,
                checkpoint_cadence_rounds: 1,
                mldsa_secret_key_path: sk_paths[i as usize].clone(),
                bls_secret_key_path: "/dev/null".into(),
                genesis_manifest_path: "/dev/null".into(),
                event_log_path: std::env::temp_dir().join(format!("gsx-daemon-test-v{}.ndjson", i)),

                max_client_connections: 256,
                client_idle_timeout_ms: 30_000,
                client_per_ip_limit: 8,
                rpc_per_ip_capacity: 60,
                rpc_per_ip_refill_per_sec: 10,
                rpc_request_timeout_ms: 30_000,
                rpc_max_request_body_bytes: 1024 * 1024,
                rpc_max_concurrent_requests: 64,
                metrics_listen: None,
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
            let committed_empty = d.state.committed.lock().is_empty();
            let inner = d.state.inner.lock().await;
            assert!(
                !committed_empty,
                "daemon {:?} did not commit any cert",
                inner.last_authored_round
            );
            state_roots.push(inner.substrate.state_root());
        }
        let first = state_roots[0];
        for r in &state_roots[1..] {
            assert_eq!(*r, first, "state roots disagree across daemons");
        }
    }

    /// Phase G integration test (DAG-S27.5).
    ///
    /// Spins up a 4-node loopback cluster with above-threshold stakes so
    /// the genesis admission populates both registries on every node.
    /// Submits an `AdmitAuthority` intent for a new id=4, waits for it
    /// to commit across the mesh, then submits an `EjectAuthority` for
    /// the same id. Asserts the registries converge to size 5 → size 4
    /// on every validator, which is the end-to-end guarantee Phase G
    /// claims (paper §4.1 + Invariant 5 for the eject path).
    ///
    /// Issue #18 fix (2026-05-14): governance intents
    /// (`AdmitAuthority` / `ExitAuthority` / `EjectAuthority`) are now
    /// queued at commit time and drained at the next epoch boundary,
    /// making the registry mutation atomic across the mesh. This
    /// eliminates the transitional quorum-threshold asymmetry where
    /// daemons disagreed on `quorum_threshold(5)=4` vs
    /// `quorum_threshold(4)=3` during the n=5→n=4 window. The test
    /// uses a deliberately short `rounds_per_epoch = 16` (≈1.6s at
    /// `round_ms=100ms`) so each governance op only waits ~one epoch
    /// boundary, not multi-round consensus convergence.
    ///
    /// Un-`#[ignore]`'d in #35: the eject path was failing with the
    /// bare `registry sizes = [5,5,5,5]` panic, which doesn't say
    /// whether the eject Intent ever reached a block, ever committed,
    /// or whether `pending_governance` is draining. The eject failure
    /// branch below now mirrors the admit branch's diagnostic so the
    /// CI log actually identifies which step is wedged.
    ///
    /// **Re-`#[ignore]`'d 2026-05-16** under tracking issue #171: the
    /// test is still flaky on shared GHA runners (~60s admit timeout
    /// fires under load even with the diagnostic instrumentation from
    /// #35). The un-ignore in #35 was deliberate — those eject-path
    /// regressions still need coverage.
    ///
    /// **Un-`#[ignore]`'d 2026-05-28** under #171: the flake's root
    /// cause was thread oversubscription, not the consensus pipeline.
    /// The runner spun `worker_threads = 8` for a 4-node mesh on
    /// 2-core GHA boxes — 8 Tokio workers fighting over 2 cores
    /// produced the scheduler jitter that starved round progress and
    /// fired the admit timeout. Dropping to `worker_threads = 4` (one
    /// per simulated node, matching the physical reality the runners
    /// can actually deliver) removes the oversubscription. The large
    /// 60s/30s/180s deadlines are kept as failure ceilings, not
    /// expected wall-clock: a healthy run converges in a few seconds
    /// and only a genuine regression burns the budget. Verified stable
    /// with `for i in $(seq 1 20); do cargo test -p gsx-node \
    /// phase_g_admit_and_eject -- --exact || break; done`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn phase_g_admit_and_eject() {
        let n = 4u32;
        let base_port: u16 = 19_700;
        let network_id = "phase-g-4n".to_string();

        // Generate real ML-DSA-65 keypairs for all validators so cert
        // signatures verify across the cluster. v0's key is also used
        // by the loadgen client to sign governance intents.
        let (pk_hexes, sk_paths, sks, pks) = test_key_files(n);
        let client_sk = sks.into_iter().next().unwrap();
        let client_pk = pks.into_iter().next().unwrap();

        let manifest = GenesisManifest {
            network_id: network_id.clone(),
            validators: (0..n)
                .map(|i| GenesisValidator {
                    authority_id: i,
                    label: format!("v{}", i),
                    mldsa_public_key_hex: pk_hexes[i as usize].clone(),
                    bls_public_key_hex: "00".into(),
                    validator_stake_gsx: 30_000, // >= VALIDATOR_STAKE_THRESHOLD_GSX
                    authority_stake_gsx: 150_000, // >= AUTHORITY_STAKE_THRESHOLD_GSX
                })
                .collect(),
            corridors: Vec::new(),
            // Issue #18: short epochs so governance application
            // (which now lands at the next boundary) is exercised on
            // CI-sane timescales. 16 rounds * 100ms = 1.6s/boundary.
            rounds_per_epoch: 16,
            prebalances: vec![],
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
                rpc_listen: None,
                peers,
                round_ms: 100,
                checkpoint_cadence_rounds: 1,
                mldsa_secret_key_path: sk_paths[i as usize].clone(),
                bls_secret_key_path: "/dev/null".into(),
                genesis_manifest_path: "/dev/null".into(),
                event_log_path: std::env::temp_dir().join(format!("gsx-phaseg-test-v{}.ndjson", i)),

                max_client_connections: 256,
                client_idle_timeout_ms: 30_000,
                client_per_ip_limit: 8,
                rpc_per_ip_capacity: 60,
                rpc_per_ip_refill_per_sec: 10,
                rpc_request_timeout_ms: 30_000,
                rpc_max_request_body_bytes: 1024 * 1024,
                rpc_max_concurrent_requests: 64,
                metrics_listen: None,
            };
            let d = Daemon::start(cfg, manifest.clone()).await.unwrap();
            daemons.push(d);
        }

        // Wait for the mesh to come up and produce a few rounds (matches
        // the warm-up cadence of `four_node_main_lane_commits`).
        tokio::time::sleep(Duration::from_secs(3)).await;

        // Sanity: genesis admission populated all 4 registries on every node.
        for (i, d) in daemons.iter().enumerate() {
            let reg = d.state.authority_registry.read().await;
            assert_eq!(reg.len(), 4, "node v{} genesis admission size", i);
        }

        // Submit AdmitAuthority for a new id=4 via v0's client port.
        let admit_addr = format!("127.0.0.1:{}", base_port + 100)
            .parse::<SocketAddr>()
            .unwrap();
        let mut client = crate::client::LoadGenClient::connect(
            admit_addr,
            client_sk,
            client_pk,
            network_id.clone(),
        )
        .await
        .unwrap();
        let admit = gsx_execution::Intent::AdmitAuthority {
            authority_id: 4,
            stake_gsx: 150_000,
            mldsa_public_key: vec![0u8; 32],
            bls_public_key: vec![0u8; 48],
        };
        client.submit(admit).await.unwrap();

        // Poll for convergence rather than sleeping a fixed wall-clock
        // window. CI runners (2-core, many parallel daemon tests) can
        // starve commit progress for several seconds; a fixed sleep
        // misses the deadline whereas a poll passes as soon as the
        // registry reflects the new admission on every node.
        let admit_deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            let all_at_5 = {
                let mut ok = true;
                for d in &daemons {
                    let reg = d.state.authority_registry.read().await;
                    if reg.len() != 5 || !reg.contains(4) {
                        ok = false;
                        break;
                    }
                }
                ok
            };
            if all_at_5 {
                break;
            }
            if std::time::Instant::now() >= admit_deadline {
                // Capture per-node state for a diagnostic panic message.
                // parking_lot guards must NOT cross .await — snapshot
                // out of them into owned scalars first, then acquire
                // the tokio async guards.
                let mut diag = Vec::new();
                for (i, d) in daemons.iter().enumerate() {
                    let (committed_n, blocks_n, intent_in_block, votes_total, votes_keys) = {
                        let committed = d.state.committed.lock();
                        let blocks = d.state.blocks.lock();
                        let votes = d.state.votes.lock();
                        let intent_in_block = blocks.values().any(|b| {
                            b.iter().any(|x| {
                                matches!(
                                    x,
                                    Intent::AdmitAuthority {
                                        authority_id: 4,
                                        ..
                                    }
                                )
                            })
                        });
                        let votes_total: usize = votes.values().map(|v| v.len()).sum();
                        (
                            committed.len(),
                            blocks.len(),
                            intent_in_block,
                            votes_total,
                            votes.len(),
                        )
                    };
                    let inner = d.state.inner.lock().await;
                    let reg = d.state.authority_registry.read().await;
                    let stake_table = d.state.stake_table.read().await;
                    let dag = d.state.dag.read().await;
                    let last_authored = inner.last_authored_round.unwrap_or(u64::MAX);
                    let reg_size = reg.len();
                    let has_id4 = reg.contains(4);
                    let n_auth = inner.n_authorities;
                    let stake_total = stake_table.total();
                    let stake_thresh =
                        gsx_consensus::joint::validator_quorum_threshold(&stake_table);
                    let auth_equiv = gsx_consensus::detect_authority_equivocation(&dag).len();
                    diag.push(format!(
                        "v{}: reg={} has4={} n={} last_authored={} committed={} blocks={} admit_in_block={} votes(k={},tot={}) stake(tot={},thr={}) equiv={}",
                        i, reg_size, has_id4, n_auth, last_authored, committed_n, blocks_n,
                        intent_in_block, votes_keys, votes_total, stake_total, stake_thresh,
                        auth_equiv
                    ));
                }
                panic!("phase G admit timed out (60s):\n  {}", diag.join("\n  "));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        // Eject the new authority.
        let eject = gsx_execution::Intent::EjectAuthority {
            authority_id: 4,
            proof_ref: [0u8; 32],
        };
        client.submit(eject).await.unwrap();

        // F3 stage-A probe: before entering the long convergence
        // loop below, give every daemon a bounded window to RECEIVE
        // the eject cert (i.e., observe a block carrying the
        // EjectAuthority{4} intent in its local `state.blocks`).
        // Decoupling "did the cert propagate?" from "did the
        // boundary process the pending governance op?" turns a
        // 180s mystery timeout into a 30s "node vN never observed
        // the eject cert" panic when the bug is in propagation,
        // and keeps the rest of the budget for the
        // boundary-application stage that's the actual bottleneck.
        let propagate_deadline = std::time::Instant::now() + Duration::from_secs(30);
        let mut propagate_last_resubmit = std::time::Instant::now();
        loop {
            let observers = {
                let mut who = Vec::with_capacity(daemons.len());
                for d in &daemons {
                    let blocks = d.state.blocks.lock();
                    let observed = blocks.values().any(|b| {
                        b.iter().any(|x| {
                            matches!(
                                x,
                                Intent::EjectAuthority {
                                    authority_id: 4,
                                    ..
                                }
                            )
                        })
                    });
                    who.push(observed);
                }
                who
            };
            if observers.iter().all(|b| *b) {
                break;
            }
            if propagate_last_resubmit.elapsed() >= Duration::from_secs(5) {
                let resubmit = gsx_execution::Intent::EjectAuthority {
                    authority_id: 4,
                    proof_ref: [0u8; 32],
                };
                let _ = client.submit(resubmit).await;
                propagate_last_resubmit = std::time::Instant::now();
            }
            if std::time::Instant::now() >= propagate_deadline {
                let missing: Vec<String> = observers
                    .iter()
                    .enumerate()
                    .filter(|(_, ok)| !**ok)
                    .map(|(i, _)| format!("v{i}"))
                    .collect();
                panic!(
                    "phase G eject-cert propagation timed out (30s): \
                     node(s) [{}] never observed a block carrying the \
                     EjectAuthority{{id=4}} intent. Bug is in cert \
                     broadcast / orphan-pull, NOT in pending-governance \
                     drain.",
                    missing.join(",")
                );
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        // Issue #18 + #32 deferred-activation: post-admit the
        // authority_registry has size 5, but `inner.n_authorities`
        // stays at 4 (v4 never authors a cert, so the pending_stake
        // promotion site in `ingest_cert` never bumps it). Leader
        // rotation continues over v0..v3 at full speed and the
        // joint-quorum stake threshold is unchanged, so commits
        // flow at line rate. The eject Intent only has to make it
        // into one block + commit + cross the next epoch boundary
        // (~1.6s for rounds_per_epoch=16). 60s is the failure
        // ceiling, not the expected wall-clock.
        // Issue #35 (2026-05-15): two-stage CI failure mode.
        //
        // Stage 1 — single-cert orphaning. The original test
        // submitted eject once. Under scheduler jitter the
        // containing cert sometimes landed in a wave where peers
        // hadn't picked its hash into their round-R+1 parent set
        // (orphan-cert skip path in `decide_slot`), so the slot
        // stayed Undecided forever and the intent vanished from
        // the commit pipeline. Fix: resubmit every 5s — a fresh
        // cert gets a fresh chance at the next anchor's
        // causal_history.
        //
        // Stage 2 — lagging-node convergence. With resubmits in
        // place, ≥3 of 4 daemons converge to reg=4 quickly, but
        // the 4th can lag 20-30 rounds under heavy CI load. It
        // has the eject cert locally but hasn't committed it,
        // so its registry stays at 5 and `all_at_4` is false.
        // The lagging daemon does recover via orphan-pull, just
        // slower than wall-clock allows. Fix: budget 180s so the
        // tail-latency daemon has room to catch up + cross one
        // more epoch boundary (16 rounds × 100ms each).
        let eject_deadline = std::time::Instant::now() + Duration::from_secs(180);
        let mut last_resubmit = std::time::Instant::now();
        loop {
            let all_at_4 = {
                let mut ok = true;
                for d in &daemons {
                    let reg = d.state.authority_registry.read().await;
                    if reg.len() != 4 || reg.contains(4) {
                        ok = false;
                        break;
                    }
                }
                ok
            };
            if all_at_4 {
                break;
            }
            if last_resubmit.elapsed() >= Duration::from_secs(5) {
                let resubmit = gsx_execution::Intent::EjectAuthority {
                    authority_id: 4,
                    proof_ref: [0u8; 32],
                };
                let _ = client.submit(resubmit).await;
                last_resubmit = std::time::Instant::now();
            }
            if std::time::Instant::now() >= eject_deadline {
                // Mirror the admit-phase diagnostic so a CI failure
                // identifies WHERE the eject pipeline is stuck:
                //   * `eject_in_block` — did v0 propose a block with it?
                //   * `committed` / `blocks` — is the cluster still committing?
                //   * `n_auth` / `stake(tot,thr)` — quorum reachable?
                //   * `pending_gov` — is the intent queued waiting for boundary?
                //   * `epoch(cur,last_bd)` / `max_round` — has a new epoch
                //     fired since admit applied? If not, the queued
                //     eject never drains.
                let mut diag = Vec::new();
                for (i, d) in daemons.iter().enumerate() {
                    let (
                        committed_n,
                        blocks_n,
                        eject_in_block,
                        votes_total,
                        votes_keys,
                        eject_cert_hash,
                        eject_cert_committed,
                    ) = {
                        let committed = d.state.committed.lock();
                        let blocks = d.state.blocks.lock();
                        let votes = d.state.votes.lock();
                        // Locate the block carrying the eject intent so we
                        // can answer the binary question: was that cert
                        // actually committed?
                        let mut eject_in_block = false;
                        let mut eject_cert_hash: Option<CertHash> = None;
                        for (h, b) in blocks.iter() {
                            if b.iter().any(|x| {
                                matches!(
                                    x,
                                    Intent::EjectAuthority {
                                        authority_id: 4,
                                        ..
                                    }
                                )
                            }) {
                                eject_in_block = true;
                                eject_cert_hash = Some(*h);
                                break;
                            }
                        }
                        let eject_cert_committed = eject_cert_hash.map(|h| committed.contains(&h));
                        let votes_total: usize = votes.values().map(|v| v.len()).sum();
                        (
                            committed.len(),
                            blocks.len(),
                            eject_in_block,
                            votes_total,
                            votes.len(),
                            eject_cert_hash,
                            eject_cert_committed,
                        )
                    };
                    let inner = d.state.inner.lock().await;
                    // Resolve the round from the blocks_by_round index
                    // (the round field was dropped from in-memory storage).
                    let eject_block_round: Option<u64> = eject_cert_hash.and_then(|ch| {
                        inner
                            .blocks_by_round
                            .iter()
                            .find(|(_, v)| **v == ch)
                            .map(|(r, _)| *r)
                    });
                    let reg = d.state.authority_registry.read().await;
                    let stake_table = d.state.stake_table.read().await;
                    let last_authored = inner.last_authored_round.unwrap_or(u64::MAX);
                    let reg_size = reg.len();
                    let has_id4 = reg.contains(4);
                    let n_auth = inner.n_authorities;
                    let stake_total = stake_table.total();
                    let stake_thresh =
                        gsx_consensus::joint::validator_quorum_threshold(&stake_table);
                    let pending_gov = inner.pending_governance.len();
                    let pending_gov_has_eject = inner.pending_governance.iter().any(|x| {
                        matches!(
                            x,
                            Intent::EjectAuthority {
                                authority_id: 4,
                                ..
                            }
                        )
                    });
                    let epoch_cur = inner.epoch.current;
                    let epoch_last_bd = inner.epoch.last_boundary_round;
                    let max_round = inner.max_observed_round;
                    let eject_round_str = eject_block_round
                        .map(|r| r.to_string())
                        .unwrap_or_else(|| "-".into());
                    let eject_committed_str = eject_cert_committed
                        .map(|b| b.to_string())
                        .unwrap_or_else(|| "-".into());
                    let _ = eject_cert_hash; // hash itself is too noisy
                    diag.push(format!(
                        "v{}: reg={} has4={} n={} last_authored={} max_round={} committed={} blocks={} eject_in_block={} eject_block_round={} eject_cert_committed={} votes(k={},tot={}) stake(tot={},thr={}) pending_gov(n={},eject={}) epoch(cur={},last_bd={})",
                        i, reg_size, has_id4, n_auth, last_authored, max_round, committed_n,
                        blocks_n, eject_in_block, eject_round_str, eject_committed_str,
                        votes_keys, votes_total, stake_total, stake_thresh, pending_gov,
                        pending_gov_has_eject, epoch_cur, epoch_last_bd
                    ));
                }
                panic!("phase G eject timed out (180s):\n  {}", diag.join("\n  "));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// Issue #28 (Phase 2.6): end-to-end signature-gate enforcement.
    ///
    /// Stand up a single-validator daemon whose Authority Ring seats
    /// a known ML-DSA-65 pubkey, then send four submission attempts
    /// over raw TCP:
    ///   1. Properly-signed intent → expect `Ack`, intent lands in a block.
    ///   2. Bogus signer_pubkey_hash → expect `Err("unknown signer ...")`.
    ///   3. Valid hash but garbage signature → expect `Err("bad ML-DSA-65 ...")`.
    ///   4. Properly-formed message but signature for a DIFFERENT intent
    ///      (replay-class attack) → expect `Err("bad ML-DSA-65 ...")`.
    ///
    /// After all submissions, assert exactly one intent landed in any
    /// proposed block — proving the bad cases were rejected before
    /// reaching the round driver's mpsc.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn client_listener_enforces_mldsa_signature() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        use crate::client::{
            intent_signing_digest, signer_pubkey_hash, ClientMessage, ClientResponse,
        };

        let n = 1u32;
        let base_port: u16 = 20_500;
        let network_id = "auth-1n".to_string();
        let (pk, sk) = gsx_crypto::mldsa::keypair();
        let pk_hex = hex::encode(pk.as_bytes());
        let manifest = GenesisManifest {
            network_id: network_id.clone(),
            validators: (0..n)
                .map(|i| GenesisValidator {
                    authority_id: i,
                    label: format!("v{}", i),
                    mldsa_public_key_hex: pk_hex.clone(),
                    bls_public_key_hex: "00".into(),
                    // See #28 note on the first client test — stakes must
                    // clear both ring thresholds for the registry to populate.
                    validator_stake_gsx: 150_000,
                    authority_stake_gsx: 150_000,
                })
                .collect(),
            corridors: Vec::new(),
            rounds_per_epoch: 1024,
            prebalances: vec![],
        };
        let cfg = NodeConfig {
            self_id: "v0".into(),
            authority_id: 0,
            listen: format!("127.0.0.1:{}", base_port).parse().unwrap(),
            client_listen: format!("127.0.0.1:{}", base_port + 100).parse().unwrap(),
            rpc_listen: None,
            peers: vec![],
            round_ms: 500,
            checkpoint_cadence_rounds: 1,
            mldsa_secret_key_path: "/dev/null".into(),
            bls_secret_key_path: "/dev/null".into(),
            genesis_manifest_path: "/dev/null".into(),
            event_log_path: std::env::temp_dir().join("gsx-client-auth-test.ndjson"),

            max_client_connections: 256,
            client_idle_timeout_ms: 30_000,
            client_per_ip_limit: 8,
            rpc_per_ip_capacity: 60,
            rpc_per_ip_refill_per_sec: 10,
            rpc_request_timeout_ms: 30_000,
            rpc_max_request_body_bytes: 1024 * 1024,
            rpc_max_concurrent_requests: 64,
            metrics_listen: None,
        };
        let d = Daemon::start(cfg.clone(), manifest).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Helper: send one framed `ClientMessage`, read one framed
        // `ClientResponse`. Returns the response.
        async fn round_trip(addr: SocketAddr, msg: &ClientMessage) -> ClientResponse {
            let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
            let _ = s.set_nodelay(true);
            let bytes = crate::codec::encode_frame(msg).unwrap();
            let len = (bytes.len() as u32).to_be_bytes();
            s.write_all(&len).await.unwrap();
            s.write_all(&bytes).await.unwrap();
            s.flush().await.unwrap();
            let mut len_buf = [0u8; 4];
            s.read_exact(&mut len_buf).await.unwrap();
            let n = u32::from_be_bytes(len_buf) as usize;
            let mut buf = vec![0u8; n];
            s.read_exact(&mut buf).await.unwrap();
            crate::codec::decode_frame(&buf).unwrap()
        }

        // ----- Case 1: properly-signed intent → Ack + lands in block.
        let good_intent = gsx_execution::Intent::Transfer {
            from: [1u8; 20],
            to: [2u8; 20],
            amount: 42,
        };
        let good_digest = intent_signing_digest(&network_id, &good_intent);
        let good_sig = gsx_crypto::mldsa::sign(&good_digest, &sk).unwrap();
        let pkh = signer_pubkey_hash(pk.as_bytes());
        let good_msg = ClientMessage::Submit {
            intent: good_intent.clone(),
            signature: good_sig.as_bytes().to_vec(),
            signer_pubkey_hash: pkh,
            signer_pubkey: None,
        };
        match round_trip(cfg.client_listen, &good_msg).await {
            ClientResponse::Ack { .. } => {}
            other => panic!("good submit should Ack, got {:?}", other),
        }

        // ----- Case 2: bogus signer_pubkey_hash → reject.
        let bogus_pkh = [0xAAu8; 32];
        let bogus_msg = ClientMessage::Submit {
            intent: gsx_execution::Intent::Transfer {
                from: [9u8; 20],
                to: [9u8; 20],
                amount: 99,
            },
            signature: good_sig.as_bytes().to_vec(),
            signer_pubkey_hash: bogus_pkh,
            signer_pubkey: None,
        };
        match round_trip(cfg.client_listen, &bogus_msg).await {
            ClientResponse::Err(e) => assert!(
                e.contains("unknown signer"),
                "expected unknown-signer error, got {}",
                e
            ),
            other => panic!("bogus pkh should reject, got {:?}", other),
        }

        // ----- Case 3: valid pkh but garbage signature → reject.
        let garbage_msg = ClientMessage::Submit {
            intent: gsx_execution::Intent::Transfer {
                from: [7u8; 20],
                to: [8u8; 20],
                amount: 77,
            },
            signature: vec![0u8; 3309], // structured-shape garbage
            signer_pubkey_hash: pkh,
            signer_pubkey: None,
        };
        match round_trip(cfg.client_listen, &garbage_msg).await {
            ClientResponse::Err(e) => assert!(
                e.contains("bad ML-DSA-65 signature"),
                "expected bad-sig error, got {}",
                e
            ),
            other => panic!("garbage sig should reject, got {:?}", other),
        }

        // ----- Case 4: signature is genuine but for a DIFFERENT intent.
        // Submitter signed intent_A, then swaps the body to intent_B
        // hoping the verifier doesn't bind the signature to the body.
        let intent_b = gsx_execution::Intent::Transfer {
            from: [1u8; 20],
            to: [2u8; 20],
            amount: 999, // different amount
        };
        let replay_msg = ClientMessage::Submit {
            intent: intent_b,
            signature: good_sig.as_bytes().to_vec(), // signs good_intent, not intent_b
            signer_pubkey_hash: pkh,
            signer_pubkey: None,
        };
        match round_trip(cfg.client_listen, &replay_msg).await {
            ClientResponse::Err(e) => assert!(
                e.contains("bad ML-DSA-65 signature"),
                "expected bad-sig (body-swap) error, got {}",
                e
            ),
            other => panic!("body-swap should reject, got {:?}", other),
        }

        // Give the round driver time to drain whatever made it through.
        tokio::time::sleep(Duration::from_millis(700)).await;

        // Exactly one intent (case 1) should have landed.
        let blocks = d.state.blocks.lock();
        let landed: Vec<&Intent> = blocks
            .values()
            .flat_map(|b| b.iter())
            .filter(|i| matches!(i, Intent::Transfer { .. }))
            .collect();
        assert_eq!(
            landed.len(),
            1,
            "exactly one signed intent should land; got {:?}",
            landed
        );
        assert!(
            matches!(landed[0], Intent::Transfer { amount: 42, .. }),
            "the landed intent should be the genuinely-signed amount=42, got {:?}",
            landed[0]
        );
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
            rounds_per_epoch: 1024,
            prebalances: vec![],
        };
        let (log, _log_task) =
            EventLog::start(&std::env::temp_dir().join("gsx-fastpath-test.ndjson"))
                .await
                .unwrap();
        let state = Arc::new(State::new(&manifest));
        let outbound: HashMap<PeerId, tokio::sync::mpsc::Sender<WireMessage>> = HashMap::new();
        let self_id: AuthorityId = 0;

        let tx = gsx_fastpath::cert::FastPathTx {
            object: gsx_fastpath::cert::OwnedObjectId([0xAB; 32]),
            owner: gsx_fastpath::cert::OwnerAddress([0xCD; 32]),
            nonce: 42,
            lineage: CertHash::from([0; 32]),
            lineage_round: 0,
            payload_digest: [0x11; 32],
        };
        let key = State::fastpath_key(&tx);

        // Authority 1 broadcasts a partial cert with itself as signer.
        let cert_a1 = gsx_fastpath::cert::FastPathCert {
            tx: tx.clone(),
            signers: BTreeSet::from([1u32]),
        };
        handle_fastpath_cert(&state, self_id, cert_a1, "v0", &log, &outbound).await;
        // Self (0) signed too → pending has {0,1}, below q=3.
        {
            let inner = state.inner.lock().await;
            assert!(inner.fastpath_pending.contains_key(&key));
            assert!(!inner.fastpath_committed.contains(&key));
            assert_eq!(inner.fastpath_pending[&key].signers.len(), 2);
        }

        // Authority 2 broadcasts. Now pending has {0,1,2} → quorum hits.
        let cert_a2 = gsx_fastpath::cert::FastPathCert {
            tx: tx.clone(),
            signers: BTreeSet::from([2u32]),
        };
        handle_fastpath_cert(&state, self_id, cert_a2, "v0", &log, &outbound).await;
        {
            let inner = state.inner.lock().await;
            assert!(
                inner.fastpath_committed.contains(&key),
                "expected fast-path quorum (q=3) to fire on (0,1,2) signers"
            );
            assert!(!inner.fastpath_pending.contains_key(&key));
        }

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
        let committed_before = state.inner.lock().await.fastpath_committed.len();
        handle_fastpath_cert(&state, self_id, bad_cert, "v0", &log, &outbound).await;
        assert_eq!(
            state.inner.lock().await.fastpath_committed.len(),
            committed_before,
            "equivocating cert must not change committed state"
        );
    }

    /// IQ-003 — K-binding cross-check (paper §6.4 / Invariant 5).
    ///
    /// Drives `handle_fastpath_cert` against a pre-seeded `main_lane_index`
    /// to exercise both branches of the new check at the unit level (no
    /// multi-daemon spin-up — that variant lives in the perf testbed):
    ///
    /// 1. **Slash path:** a fast-path cert whose `(object, payload_digest)`
    ///    conflicts with a main-lane tx in the binding window
    ///    `(lineage_round, lineage_round + K]` is rejected and produces no
    ///    pending or committed state. This is the equivocation signal the
    ///    paper requires the receiver to surface.
    /// 2. **Consistent path:** a fast-path cert for a different object
    ///    (no main-lane conflict) accumulates normally. Confirms the
    ///    K-binding check is not a blanket reject.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fastpath_k_binding_slashes_on_main_lane_conflict() {
        let n = 4u32;
        let manifest = GenesisManifest {
            network_id: "fp-k-binding-4n".into(),
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
            rounds_per_epoch: 1024,
            prebalances: vec![],
        };
        let (log, _log_task) =
            EventLog::start(&std::env::temp_dir().join("gsx-fp-k-binding-test.ndjson"))
                .await
                .unwrap();
        let state = Arc::new(State::new(&manifest));
        let outbound: HashMap<PeerId, tokio::sync::mpsc::Sender<WireMessage>> = HashMap::new();
        let self_id: AuthorityId = 0;

        // ── Pre-seed the main-lane index with a tx for OBJECT_A at
        // round 5 with payload P_main. The fast-path cert below will
        // carry the same object but payload P_fp != P_main, within the
        // K=4 binding window.
        let object_a = OwnedObjectId([0xAA; 32]);
        let main_payload: [u8; 32] = [0x11; 32];
        {
            let mut inner = state.inner.lock().await;
            inner.main_lane_index.push(MainLaneTx {
                round: 5,
                object: object_a,
                payload_digest: main_payload,
                lineage: CertHash::from([0xDE; 32]),
            });
        }

        // ── Conflicting cert: same object, different payload, lineage at
        // round 3 (window = (3, 7], which includes the seeded round 5).
        let conflicting_tx = gsx_fastpath::cert::FastPathTx {
            object: object_a,
            owner: gsx_fastpath::cert::OwnerAddress([0xCD; 32]),
            nonce: 1,
            lineage: CertHash::from([0; 32]),
            lineage_round: 3,
            payload_digest: [0x22; 32], // != main_payload
        };
        let conflicting_key = State::fastpath_key(&conflicting_tx);
        let conflicting_cert = gsx_fastpath::cert::FastPathCert {
            tx: conflicting_tx,
            signers: BTreeSet::from([1u32]),
        };

        handle_fastpath_cert(&state, self_id, conflicting_cert, "v0", &log, &outbound).await;

        {
            let inner = state.inner.lock().await;
            assert!(
                !inner.fastpath_pending.contains_key(&conflicting_key),
                "K-binding violator must not accumulate into fastpath_pending"
            );
            assert!(
                !inner.fastpath_committed.contains(&conflicting_key),
                "K-binding violator must not reach fastpath_committed"
            );
        }

        // ── Positive control: a cert for a DIFFERENT object should
        // accumulate normally (the K-binding check is per-object, not a
        // blanket reject).
        let object_b = OwnedObjectId([0xBB; 32]);
        let consistent_tx = gsx_fastpath::cert::FastPathTx {
            object: object_b,
            owner: gsx_fastpath::cert::OwnerAddress([0xCD; 32]),
            nonce: 1,
            lineage: CertHash::from([0; 32]),
            lineage_round: 3,
            payload_digest: [0x33; 32],
        };
        let consistent_key = State::fastpath_key(&consistent_tx);
        let consistent_cert = gsx_fastpath::cert::FastPathCert {
            tx: consistent_tx,
            signers: BTreeSet::from([1u32]),
        };

        handle_fastpath_cert(&state, self_id, consistent_cert, "v0", &log, &outbound).await;

        {
            let inner = state.inner.lock().await;
            assert!(
                inner.fastpath_pending.contains_key(&consistent_key)
                    || inner.fastpath_committed.contains(&consistent_key),
                "consistent cert must accumulate or commit (no spurious K-binding reject)"
            );
        }
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
            rounds_per_epoch: 1024,
            prebalances: vec![],
        };
        let (log, _log_task) = EventLog::start(&std::env::temp_dir().join("gsx-ltp-test.ndjson"))
            .await
            .unwrap();
        let state = Arc::new(State::new(&manifest));

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

        {
            let inner = state.inner.lock().await;
            assert_eq!(inner.ltp_received_count, 0);
            assert!(inner.ltp_latest.is_empty());
        }

        handle_ltp_attestation(&state, att, "v0", &log).await;

        let inner = state.inner.lock().await;
        assert_eq!(inner.ltp_received_count, 1);
        let stored = inner
            .ltp_latest
            .get(&corridor_id)
            .expect("corridor should have an attestation");
        assert_eq!(stored.source_height, 12_345_678);
        assert_eq!(stored.state_root, [0xAB; 32]);
    }

    /// S25.1 smoke: `EpochState` boundary detection at the right round.
    /// `rounds_per_epoch = 10` → boundary at round 10, 20, 30, ...
    #[test]
    fn epoch_boundary_detected_at_period_rounds() {
        let mut e = EpochState {
            current: 0,
            rounds_per_epoch: 10,
            last_boundary_round: 0,
        };
        // Rounds 0-9 are epoch 0.
        for r in 0u64..10 {
            assert_eq!(e.epoch_for(r), 0, "round {r} should be epoch 0");
            assert!(!e.boundary_crossed_by(r), "round {r} should not cross");
        }
        // Round 10 crosses into epoch 1.
        assert_eq!(e.epoch_for(10), 1);
        assert!(e.boundary_crossed_by(10));
        // Simulate the daemon's update.
        e.current = e.epoch_for(10);
        e.last_boundary_round = 10;
        // Subsequent rounds within epoch 1 don't re-trigger.
        for r in 10u64..20 {
            assert!(!e.boundary_crossed_by(r), "round {r} should not re-cross");
        }
        // Round 20 crosses to epoch 2.
        assert!(e.boundary_crossed_by(20));
        assert_eq!(e.epoch_for(20), 2);
    }

    /// `rounds_per_epoch = 0` is a degenerate config — should not
    /// trigger boundaries (avoids div-by-zero in production).
    #[test]
    fn epoch_zero_rounds_per_epoch_is_safe() {
        let e = EpochState {
            current: 5,
            rounds_per_epoch: 0,
            last_boundary_round: 0,
        };
        assert_eq!(e.epoch_for(1_000_000), 5);
        assert!(!e.boundary_crossed_by(1_000_000));
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
            rounds_per_epoch: 1024,
            prebalances: vec![],
        };
        let (log, _log_task) = EventLog::start(&std::env::temp_dir().join("gsx-ltp-unreg.ndjson"))
            .await
            .unwrap();
        let state = Arc::new(State::new(&manifest));
        assert!(state.inner.lock().await.corridors.is_empty());

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
        handle_ltp_attestation(&state, att, "v0", &log).await;

        // Stored under fallback corridor id (XOR-fold of chain pair).
        let fallback_id = corridor_id_fallback(&payload);
        let inner = state.inner.lock().await;
        assert_eq!(inner.ltp_received_count, 1);
        assert!(inner.ltp_latest.contains_key(&fallback_id));
    }

    /// Issue #27: end-to-end JSON-RPC binding. When `NodeConfig.rpc_listen`
    /// is set, `Daemon::start` spawns `gsx_rpc::start` and the four
    /// read-only methods become reachable over HTTP. This test boots a
    /// single-validator daemon, opens a TCP socket to the RPC port,
    /// sends a `gsx_getEpoch` request as raw HTTP, and verifies the
    /// JSON-RPC response envelope. Stays at the wire level (no `reqwest`)
    /// so the test crate doesn't pull in another HTTP client.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rpc_binding_returns_epoch_over_http() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let base_port: u16 = 21_000;
        let manifest = GenesisManifest {
            network_id: "rpc-bind-1n".into(),
            validators: vec![GenesisValidator {
                authority_id: 0,
                label: "v0".into(),
                mldsa_public_key_hex: "00".into(),
                bls_public_key_hex: "00".into(),
                validator_stake_gsx: 30_000,
                authority_stake_gsx: 150_000,
            }],
            corridors: Vec::new(),
            rounds_per_epoch: 1024,
            prebalances: vec![],
        };
        let cfg = NodeConfig {
            self_id: "v0".into(),
            authority_id: 0,
            listen: format!("127.0.0.1:{}", base_port).parse().unwrap(),
            client_listen: format!("127.0.0.1:{}", base_port + 100).parse().unwrap(),
            rpc_listen: Some(format!("127.0.0.1:{}", base_port + 200).parse().unwrap()),
            peers: vec![],
            round_ms: 500,
            checkpoint_cadence_rounds: 1,
            mldsa_secret_key_path: "/dev/null".into(),
            bls_secret_key_path: "/dev/null".into(),
            genesis_manifest_path: "/dev/null".into(),
            event_log_path: std::env::temp_dir().join("gsx-rpc-bind-test.ndjson"),

            max_client_connections: 256,
            client_idle_timeout_ms: 30_000,
            client_per_ip_limit: 8,
            rpc_per_ip_capacity: 60,
            rpc_per_ip_refill_per_sec: 10,
            rpc_request_timeout_ms: 30_000,
            rpc_max_request_body_bytes: 1024 * 1024,
            rpc_max_concurrent_requests: 64,
            metrics_listen: None,
        };
        let _d = Daemon::start(cfg.clone(), manifest).await.unwrap();
        // Give the bound listener a tick to accept connections.
        tokio::time::sleep(Duration::from_millis(200)).await;

        let body = br#"{"jsonrpc":"2.0","id":1,"method":"gsx_getEpoch"}"#;
        let req = format!(
            "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );

        let rpc_addr = cfg.rpc_listen.unwrap();
        let mut stream = tokio::net::TcpStream::connect(rpc_addr).await.unwrap();
        let _ = stream.set_nodelay(true);
        stream.write_all(req.as_bytes()).await.unwrap();
        stream.write_all(body).await.unwrap();
        stream.flush().await.unwrap();

        let mut resp_bytes = Vec::new();
        stream.read_to_end(&mut resp_bytes).await.unwrap();
        let resp = String::from_utf8(resp_bytes).expect("response is utf-8");

        // Split off HTTP body (everything after the blank line).
        let body_start = resp
            .find("\r\n\r\n")
            .expect("HTTP response has CRLF-CRLF separator")
            + 4;
        let json_body = &resp[body_start..];
        let parsed: serde_json::Value = serde_json::from_str(json_body.trim_end())
            .unwrap_or_else(|e| panic!("failed to parse JSON-RPC body {:?}: {}", json_body, e));

        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["id"], 1);
        assert_eq!(parsed["result"]["current"], 0);
        assert_eq!(parsed["result"]["rounds_per_epoch"], 1024);
        assert!(parsed["error"].is_null());
    }

    /// `blocks_by_round` + `tx_to_block` indices populate on commit.
    ///
    /// Stand up a single-validator daemon, submit three signed transfer
    /// intents over the client wire, wait for them to land, and assert
    /// the round-keyed and tx-hash-keyed indices both resolve to the
    /// same cert + the right intent positions. Mirrors the
    /// `client_listener_accepts_intent` scaffold; the unique port range
    /// avoids collisions with other inline daemon tests.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn try_commit_populates_blocks_by_round_and_tx_to_block() {
        let n = 1u32;
        let base_port: u16 = 21_000;
        let network_id = "blocks-idx-1n".to_string();
        let (pk, sk) = gsx_crypto::mldsa::keypair();
        let pk_hex = hex::encode(pk.as_bytes());
        let manifest = GenesisManifest {
            network_id: network_id.clone(),
            validators: (0..n)
                .map(|i| GenesisValidator {
                    authority_id: i,
                    label: format!("v{}", i),
                    mldsa_public_key_hex: pk_hex.clone(),
                    bls_public_key_hex: "00".into(),
                    // Stakes must clear both ring thresholds so the
                    // signature gate accepts our submissions.
                    validator_stake_gsx: 150_000,
                    authority_stake_gsx: 150_000,
                })
                .collect(),
            corridors: Vec::new(),
            rounds_per_epoch: 1024,
            prebalances: vec![],
        };
        let cfg = NodeConfig {
            self_id: "v0".into(),
            authority_id: 0,
            listen: format!("127.0.0.1:{}", base_port).parse().unwrap(),
            client_listen: format!("127.0.0.1:{}", base_port + 100).parse().unwrap(),
            rpc_listen: None,
            peers: vec![],
            round_ms: 500,
            checkpoint_cadence_rounds: 1,
            mldsa_secret_key_path: "/dev/null".into(),
            bls_secret_key_path: "/dev/null".into(),
            genesis_manifest_path: "/dev/null".into(),
            event_log_path: std::env::temp_dir().join("gsx-blocks-idx-test.ndjson"),

            max_client_connections: 256,
            client_idle_timeout_ms: 30_000,
            client_per_ip_limit: 8,
            rpc_per_ip_capacity: 60,
            rpc_per_ip_refill_per_sec: 10,
            rpc_request_timeout_ms: 30_000,
            rpc_max_request_body_bytes: 1024 * 1024,
            rpc_max_concurrent_requests: 64,
            metrics_listen: None,
        };
        let d = Daemon::start(cfg.clone(), manifest).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut client =
            crate::client::LoadGenClient::connect(cfg.client_listen, sk, pk, network_id)
                .await
                .unwrap();
        let intents: Vec<Intent> = (0..3u8)
            .map(|i| Intent::Transfer {
                from: [i + 1; 20],
                to: [i + 2; 20],
                amount: 1_000 + i as u128,
            })
            .collect();
        let hashes = client.submit_batch(intents.clone()).await.unwrap();
        assert_eq!(hashes.len(), 3);

        // Wait for the round driver to author + commit blocks carrying
        // these intents. They may land in one block or split across
        // adjacent rounds depending on mpsc-drain vs tick scheduling;
        // either is correct.
        let intent_hashes: Vec<[u8; 32]> = intents
            .iter()
            .map(|i| *blake3::hash(&crate::codec::encode(i).unwrap()).as_bytes())
            .collect();
        let mut found = false;
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let inner = d.state.inner.lock().await;
            if intent_hashes
                .iter()
                .all(|h| inner.tx_to_block.contains_key(h))
            {
                found = true;
                break;
            }
        }
        assert!(found, "tx_to_block did not index every submitted intent");

        // For every indexed intent: the round-keyed index must agree on
        // the cert hash, and the block whose cert is committed must
        // contain the intent at the recorded position.
        let inner = d.state.inner.lock().await;
        let blocks = d.state.blocks.lock();
        for h in &intent_hashes {
            let (round, cert_hash, idx) = inner.tx_to_block.get(h).copied().unwrap();
            assert_eq!(
                inner.blocks_by_round.get(&round).copied(),
                Some(cert_hash),
                "blocks_by_round disagrees with tx_to_block at round {}",
                round,
            );
            let block = blocks
                .get(&cert_hash)
                .expect("cert hash from tx_to_block must resolve in state.blocks");
            let stored = crate::codec::encode(&block[idx]).unwrap();
            let stored_hash: [u8; 32] = *blake3::hash(&stored).as_bytes();
            assert_eq!(
                &stored_hash, h,
                "intent at block[{}] does not match tx_to_block key",
                idx,
            );
        }
    }

    /// T2: end-to-end `gsx_submitIntent`. Bind a single-validator
    /// daemon with `rpc_listen` set, sign an intent client-side using
    /// the same digest format the TCP wire uses, POST to the RPC, and
    /// assert (1) Ack response carries the intent hash, (2) the intent
    /// lands in a committed block, (3) `tx_to_block` indexes it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rpc_submit_intent_round_trips_through_consensus() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        use crate::client::{intent_signing_digest, signer_pubkey_hash};

        let base_port: u16 = 21_500;
        let network_id = "rpc-submit-1n".to_string();
        let (pk, sk) = gsx_crypto::mldsa::keypair();
        let pk_hex = hex::encode(pk.as_bytes());
        let manifest = GenesisManifest {
            network_id: network_id.clone(),
            validators: vec![GenesisValidator {
                authority_id: 0,
                label: "v0".into(),
                mldsa_public_key_hex: pk_hex,
                bls_public_key_hex: "00".into(),
                validator_stake_gsx: 30_000,
                authority_stake_gsx: 150_000,
            }],
            corridors: Vec::new(),
            rounds_per_epoch: 1024,
            prebalances: vec![],
        };
        let cfg = NodeConfig {
            self_id: "v0".into(),
            authority_id: 0,
            listen: format!("127.0.0.1:{}", base_port).parse().unwrap(),
            client_listen: format!("127.0.0.1:{}", base_port + 100).parse().unwrap(),
            rpc_listen: Some(format!("127.0.0.1:{}", base_port + 200).parse().unwrap()),
            peers: vec![],
            round_ms: 500,
            checkpoint_cadence_rounds: 1,
            mldsa_secret_key_path: "/dev/null".into(),
            bls_secret_key_path: "/dev/null".into(),
            genesis_manifest_path: "/dev/null".into(),
            event_log_path: std::env::temp_dir().join("gsx-rpc-submit-test.ndjson"),

            max_client_connections: 256,
            client_idle_timeout_ms: 30_000,
            client_per_ip_limit: 8,
            rpc_per_ip_capacity: 60,
            rpc_per_ip_refill_per_sec: 10,
            rpc_request_timeout_ms: 30_000,
            rpc_max_request_body_bytes: 1024 * 1024,
            rpc_max_concurrent_requests: 64,
            metrics_listen: None,
        };
        let d = Daemon::start(cfg.clone(), manifest).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Sign client-side using the same primitives the TCP wire uses.
        let intent = Intent::Transfer {
            from: [1u8; 20],
            to: [2u8; 20],
            amount: 42,
        };
        let intent_bincode = crate::codec::encode(&intent).unwrap();
        let digest = intent_signing_digest(&network_id, &intent);
        let signature = gsx_crypto::mldsa::sign(&digest, &sk).unwrap();
        let pkh = signer_pubkey_hash(pk.as_bytes());

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "gsx_submitIntent",
            "params": {
                "intent": format!("0x{}", hex::encode(&intent_bincode)),
                "signature": format!("0x{}", hex::encode(signature.as_bytes())),
                "signer_pubkey_hash": format!("0x{}", hex::encode(pkh)),
            },
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let req = format!(
            "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body_bytes.len()
        );

        let rpc_addr = cfg.rpc_listen.unwrap();
        let mut stream = tokio::net::TcpStream::connect(rpc_addr).await.unwrap();
        let _ = stream.set_nodelay(true);
        stream.write_all(req.as_bytes()).await.unwrap();
        stream.write_all(&body_bytes).await.unwrap();
        stream.flush().await.unwrap();

        let mut resp_bytes = Vec::new();
        stream.read_to_end(&mut resp_bytes).await.unwrap();
        let resp_text = String::from_utf8(resp_bytes).unwrap();
        let body_start = resp_text.find("\r\n\r\n").unwrap() + 4;
        let parsed: serde_json::Value =
            serde_json::from_str(resp_text[body_start..].trim_end()).unwrap();

        // Ack must carry the blake3 hash of the bincode bytes.
        let expected_hash: [u8; 32] = *blake3::hash(&intent_bincode).as_bytes();
        let tx_hash_hex = parsed["result"]["tx_hash"]
            .as_str()
            .expect("tx_hash present");
        assert_eq!(tx_hash_hex, format!("0x{}", hex::encode(expected_hash)));

        // Wait for the round driver to author + commit a block carrying
        // this intent, then check tx_to_block is populated.
        let mut found = false;
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let inner = d.state.inner.lock().await;
            if inner.tx_to_block.contains_key(&expected_hash) {
                found = true;
                break;
            }
        }
        assert!(
            found,
            "intent submitted via RPC never landed in tx_to_block"
        );
    }

    /// T2: rejection paths through the RPC ingress. UnknownSigner
    /// surfaces as -32001 in the JSON-RPC envelope.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rpc_submit_intent_unknown_signer() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let base_port: u16 = 21_600;
        let network_id = "rpc-submit-bad-1n".to_string();
        let (pk, _sk) = gsx_crypto::mldsa::keypair();
        let pk_hex = hex::encode(pk.as_bytes());
        let manifest = GenesisManifest {
            network_id,
            validators: vec![GenesisValidator {
                authority_id: 0,
                label: "v0".into(),
                mldsa_public_key_hex: pk_hex,
                bls_public_key_hex: "00".into(),
                validator_stake_gsx: 30_000,
                authority_stake_gsx: 150_000,
            }],
            corridors: Vec::new(),
            rounds_per_epoch: 1024,
            prebalances: vec![],
        };
        let cfg = NodeConfig {
            self_id: "v0".into(),
            authority_id: 0,
            listen: format!("127.0.0.1:{}", base_port).parse().unwrap(),
            client_listen: format!("127.0.0.1:{}", base_port + 100).parse().unwrap(),
            rpc_listen: Some(format!("127.0.0.1:{}", base_port + 200).parse().unwrap()),
            peers: vec![],
            round_ms: 500,
            checkpoint_cadence_rounds: 1,
            mldsa_secret_key_path: "/dev/null".into(),
            bls_secret_key_path: "/dev/null".into(),
            genesis_manifest_path: "/dev/null".into(),
            event_log_path: std::env::temp_dir().join("gsx-rpc-submit-bad-test.ndjson"),

            max_client_connections: 256,
            client_idle_timeout_ms: 30_000,
            client_per_ip_limit: 8,
            rpc_per_ip_capacity: 60,
            rpc_per_ip_refill_per_sec: 10,
            rpc_request_timeout_ms: 30_000,
            rpc_max_request_body_bytes: 1024 * 1024,
            rpc_max_concurrent_requests: 64,
            metrics_listen: None,
        };
        let _d = Daemon::start(cfg.clone(), manifest).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Submit with a pubkey hash that doesn't match anyone seated.
        let intent = Intent::Transfer {
            from: [3u8; 20],
            to: [4u8; 20],
            amount: 7,
        };
        let intent_bincode = crate::codec::encode(&intent).unwrap();
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "gsx_submitIntent",
            "params": {
                "intent": format!("0x{}", hex::encode(&intent_bincode)),
                "signature": format!("0x{}", "00".repeat(3309)),
                "signer_pubkey_hash": format!("0x{}", "ee".repeat(32)),
            },
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let req = format!(
            "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body_bytes.len()
        );

        let rpc_addr = cfg.rpc_listen.unwrap();
        let mut stream = tokio::net::TcpStream::connect(rpc_addr).await.unwrap();
        let _ = stream.set_nodelay(true);
        stream.write_all(req.as_bytes()).await.unwrap();
        stream.write_all(&body_bytes).await.unwrap();
        stream.flush().await.unwrap();

        let mut resp_bytes = Vec::new();
        stream.read_to_end(&mut resp_bytes).await.unwrap();
        let resp_text = String::from_utf8(resp_bytes).unwrap();
        let body_start = resp_text.find("\r\n\r\n").unwrap() + 4;
        let parsed: serde_json::Value =
            serde_json::from_str(resp_text[body_start..].trim_end()).unwrap();

        assert_eq!(parsed["error"]["code"], -32001);
    }

    #[test]
    fn genesis_prebalances_applied_to_substrate() {
        use crate::config::GenesisBalance;

        let faucet_addr = "0x0102030405060708091011121314151617181920";
        let manifest = GenesisManifest {
            network_id: "prebal-test".into(),
            validators: vec![GenesisValidator {
                authority_id: 0,
                label: "v0".into(),
                mldsa_public_key_hex: "00".into(),
                bls_public_key_hex: "00".into(),
                validator_stake_gsx: 150_000,
                authority_stake_gsx: 150_000,
            }],
            corridors: Vec::new(),
            rounds_per_epoch: 1024,
            prebalances: vec![GenesisBalance {
                address: faucet_addr.into(),
                balance_gsx: 1_000_000,
                role: Some("faucet".into()),
            }],
        };

        let state = State::new(&manifest);
        let inner = state.inner.blocking_lock();
        let addr: [u8; 20] = [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x10, 0x11, 0x12, 0x13, 0x14,
            0x15, 0x16, 0x17, 0x18, 0x19, 0x20,
        ];
        let bal = inner.substrate.balance(&addr);
        assert_eq!(bal, 1_000_000, "prebalance should be applied");
    }
}
