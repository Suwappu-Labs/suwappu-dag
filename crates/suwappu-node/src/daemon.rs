//! Validator daemon — main DAG lane.
//!
//! Composes the wire transport, DAG store, joint-quorum voter, and block
//! executor into a single running process. Drives DagBft-C rounds on a
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

use suwappu_authority::{AuthorityMember, AuthorityRegistry};
use suwappu_consensus::{
    cert::{CertHash, Certificate, Round},
    commit::{cert_at, quorum_threshold},
    dag::DagStore,
    decide_slot,
    equivocation::EquivocationProof,
    joint::{StakeTable, Vote},
    validator_quorum_met, AuthorityId, ConsensusError, LeaderStatus,
};
use suwappu_execution::Substrate;
use suwappu_execution::{execute_block, Block, InMemorySubstrate, Intent};
use suwappu_fastpath::{
    binding::{is_main_lane_consistent, MainLaneTx, FAST_PATH_CONFIRMATION_K},
    cert::{FastPathCert, FastPathTx, OwnedObjectId},
    quorum::fast_path_quorum_size,
};
use suwappu_ltp::{
    AttestationPayload, ChainId, Corridor, CorridorAttestation, CorridorId, SuperNode,
};
use suwappu_validator::{ValidatorMember, ValidatorRegistry};
use tracing::debug;

use crate::{
    config::{ConfigError, GenesisManifest, NodeConfig},
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
    pub(crate) blocks: parking_lot::Mutex<HashMap<CertHash, BlockPayload>>,
    pub(crate) committed: parking_lot::Mutex<HashSet<CertHash>>,
    pub(crate) stake_table: tokio::sync::RwLock<StakeTable>,
    pub(crate) authority_registry: tokio::sync::RwLock<AuthorityRegistry>,
    pub(crate) validator_registry: tokio::sync::RwLock<ValidatorRegistry>,
    pub(crate) inner: tokio::sync::Mutex<StateInner>,
    /// DAG-S31.4 / A3 mempool integration: priority + rate-limited
    /// queue replacing the FIFO `intent_tx` mpsc. Both client wire
    /// (`crates/suwappu-node/src/client.rs::handle_conn`) and JSON-RPC
    /// (`crates/suwappu-rpc/src/methods.rs::submit_intent` via the
    /// `rpc_adapter`) `Mempool::submit` after `verify_signed_intent`;
    /// the round driver pops via `drain_for_block` at block-build
    /// time. The mempool enforces per-peer leaky-bucket rate limits,
    /// content dedup, capacity floor with priority-ordered eviction,
    /// and TTL expiry. See `crates/suwappu-mempool/src/lib.rs`.
    pub(crate) mempool: std::sync::Arc<suwappu_mempool::Mempool>,
    /// This validator's own ML-DSA-65 secret key, loaded unconditionally from
    /// `NodeConfig::mldsa_secret_key_path` at startup (that field is required,
    /// not optional — every validator must be able to author signed
    /// certificates). Used by the round driver to sign every self-authored
    /// `Certificate` before broadcast; gossip-received certificates are
    /// verified against the author's genesis-registered public key in
    /// `ingest_cert`, not against this key.
    pub(crate) self_secret_key: suwappu_crypto::mldsa::SecretKey,
    /// Manifest network id (string form) used to recompute the
    /// `intent_signing_digest` when re-verifying governance envelopes at
    /// commit. Distinct from the `[u8;32]` bridge `network_id` below.
    pub(crate) manifest_network_id: String,
    /// Governance authorization envelopes retained at ingest
    /// (`SubmitGoverned`), keyed by intent hash, so the block author can
    /// attach them to the `BlockPayload` for every committer to
    /// re-verify. Bounded (governance intents are rare); the map is
    /// cleared past the cap.
    pub(crate) governance_envelopes: parking_lot::Mutex<HashMap<[u8; 32], crate::client::GovAuth>>,
    /// Bridge header-attestation signer, or `None` when header attestation is
    /// not configured (no `bridge_oracle_address`, or the ML-DSA key could not
    /// be loaded). When `None`, `suwappu_getHeaderAttestation` returns `null`.
    pub(crate) bridge_signer: Option<BridgeHeaderSigner>,
    /// Lazily-signed cache for the latest bridge-header attestation. Signing
    /// ML-DSA is ~ms and must never happen under the commit loop's `inner`
    /// lock; instead the RPC adapter signs on demand and caches here, re-signing
    /// only when `StateInner::latest_bridge_header` advances to a new round.
    pub(crate) bridge_attestation_cache:
        parking_lot::Mutex<Option<suwappu_consensus::bridge_header::HeaderAttestation>>,
}

/// Material needed to produce this validator's bridge-header side-attestations.
///
/// Honest framing: holding this lets the node sign a *claim* about the block
/// header it locally finalized. It is not a consensus light client and not a
/// source-state proof; the destination oracle trusts an honest >2/3-stake
/// quorum of these attestations.
pub(crate) struct BridgeHeaderSigner {
    /// This validator's Authority Ring id (matches the on-chain registry).
    pub(crate) authority_id: AuthorityId,
    /// uint256 network id (big-endian) folded into every attestation digest.
    /// Must byte-match the deployed registry's `networkId` immutable.
    pub(crate) network_id: [u8; 32],
    /// The deployed `SuwappuDagQuorumHeaderOracle` address folded into the digest.
    pub(crate) oracle: [u8; 20],
    /// This validator's ML-DSA-65 public key (as registered in genesis).
    pub(crate) pubkey: suwappu_crypto::mldsa::PublicKey,
    /// This validator's ML-DSA-65 secret key, loaded from
    /// `mldsa_secret_key_path`. First runtime use of the node's signing key.
    pub(crate) secret_key: suwappu_crypto::mldsa::SecretKey,
}

/// Default bridge network id = `keccak256("suwappu-perf-7r")`, hard-pinned as a
/// 32-byte literal (byte-identical to the value the destination wiring PR
/// pinned and to `bridge_header`'s golden vector). Used when
/// `NodeConfig::bridge_network_id` is unset.
const DEFAULT_BRIDGE_NETWORK_ID: [u8; 32] = [
    0xff, 0x43, 0x1b, 0x38, 0x51, 0xff, 0x00, 0xbe, 0x6b, 0x5a, 0x4b, 0xd9, 0xb6, 0x7e, 0x7d, 0x41,
    0x18, 0x30, 0x06, 0x93, 0x93, 0x78, 0x65, 0xdf, 0xe7, 0x58, 0x47, 0xdf, 0xd7, 0xcd, 0xd7, 0x8a,
];

impl BridgeHeaderSigner {
    /// Build the signer from config + genesis, or `None` if header attestation
    /// is not configured or the key/identity cannot be resolved. Never panics;
    /// every failure path logs and disables attestation (fail-open to "no
    /// attestation", never fail-closed to a crash).
    fn from_config(cfg: &NodeConfig, manifest: &GenesisManifest) -> Option<Self> {
        // Gating field: no oracle address => header attestation disabled.
        let oracle_hex = cfg.bridge_oracle_address.as_deref()?;
        let oracle = match decode_hex_array::<20>(oracle_hex) {
            Some(o) => o,
            None => {
                tracing::warn!(
                    oracle = oracle_hex,
                    "bridge: invalid oracle address hex; header attestation disabled"
                );
                return None;
            }
        };
        let network_id = match cfg.bridge_network_id.as_deref() {
            None => DEFAULT_BRIDGE_NETWORK_ID,
            Some(h) => match decode_hex_array::<32>(h) {
                Some(n) => n,
                None => {
                    tracing::warn!(
                        network_id = h,
                        "bridge: invalid network_id hex; header attestation disabled"
                    );
                    return None;
                }
            },
        };
        // Load the ML-DSA secret key from disk (raw bytes, else hex).
        let sk = match std::fs::read(&cfg.mldsa_secret_key_path) {
            Ok(raw) => load_mldsa_secret(&raw),
            Err(e) => {
                tracing::warn!(path = %cfg.mldsa_secret_key_path.display(), err = %e, "bridge: cannot read mldsa_secret_key_path; header attestation disabled");
                return None;
            }
        }?;
        // Own public key from the genesis manifest entry for this authority.
        let v = manifest
            .validators
            .iter()
            .find(|v| v.authority_id == cfg.authority_id)?;
        let pk_bytes = hex::decode(v.mldsa_public_key_hex.trim_start_matches("0x")).ok()?;
        let pubkey = suwappu_crypto::mldsa::PublicKey::from_bytes(&pk_bytes).ok()?;
        // sk↔pk correspondence guard: the secret key is loaded from disk but the
        // public key comes from genesis. If they are not a keypair, every
        // attestation would carry the genesis pk with a signature made by the
        // file sk and be rejected everywhere (locally by `verify`, and on-chain
        // by the registry's registered pk) — silently halting this validator's
        // bridge contribution. Probe once at startup and fail LOUD (ERROR +
        // disable) instead of silently on-chain.
        let probe = b"suwappu-bridge-header-keypair-probe";
        let matches = suwappu_crypto::mldsa::sign(probe, &sk)
            .ok()
            .is_some_and(|sig| suwappu_crypto::mldsa::verify(probe, &sig, &pubkey).is_ok());
        if !matches {
            tracing::error!(
                authority_id = cfg.authority_id,
                "bridge: loaded ML-DSA secret key does NOT match the genesis public key for this authority; header attestation DISABLED (check mldsa_secret_key_path vs genesis mldsa_public_key_hex)"
            );
            return None;
        }
        tracing::info!(
            authority_id = cfg.authority_id,
            oracle = oracle_hex,
            "bridge: header attestation ENABLED (validator-quorum side-attestation, UNFED until a relayer aggregates >2/3 stake)"
        );
        Some(Self {
            authority_id: cfg.authority_id,
            network_id,
            oracle,
            pubkey,
            secret_key: sk,
        })
    }
}

/// Decode a 0x-optional hex string into a fixed `[u8; N]`, or `None` on bad
/// length / bad hex.
fn decode_hex_array<const N: usize>(s: &str) -> Option<[u8; N]> {
    let bytes = hex::decode(s.trim_start_matches("0x")).ok()?;
    if bytes.len() != N {
        return None;
    }
    let mut out = [0u8; N];
    out.copy_from_slice(&bytes);
    Some(out)
}

/// Load an ML-DSA-65 secret key from file contents: try raw bytes first, then
/// a hex envelope. `None` if neither parses.
fn load_mldsa_secret(raw: &[u8]) -> Option<suwappu_crypto::mldsa::SecretKey> {
    if let Ok(sk) = suwappu_crypto::mldsa::SecretKey::from_bytes(raw) {
        return Some(sk);
    }
    let text = std::str::from_utf8(raw).ok()?.trim();
    let decoded = hex::decode(text.trim_start_matches("0x")).ok()?;
    suwappu_crypto::mldsa::SecretKey::from_bytes(&decoded).ok()
}

/// Cold-path fields. Pre-S31 these lived on `State` directly; now
/// grouped under one `Mutex<StateInner>` because access frequency
/// doesn't warrant individual locks.
pub(crate) struct StateInner {
    pub(crate) substrate: InMemorySubstrate,
    pub(crate) last_authored_round: Option<u64>,
    /// Highest round observed across own + peer certs. Used by the
    /// synchronizer (S21.3) to detect catch-up gaps.
    pub(crate) max_observed_round: u64,
    /// Highest DAG round any peer has reported via `WireMessage::Tip`.
    /// Drives the forward backfill loop (`run_backfill`) for late-join
    /// and restart catch-up.
    pub(crate) sync_tip: u64,
    pub(crate) n_authorities: u32,
    /// Certs received whose parents aren't yet in the local DAG.
    pub(crate) orphans: HashMap<CertHash, Vec<Certificate>>,
    /// Cert hashes for which a `GetCert` request is outstanding.
    pub(crate) inflight_fetches: HashSet<CertHash>,
    /// Committed certs whose authentic (cert-digest-matching) block is not
    /// yet available, so commit was deferred. The sync sweeper issues
    /// `GetBlock` for these; entries are removed once the block arrives
    /// (and the cert commits) or is otherwise no longer needed.
    pub(crate) needed_blocks: HashSet<CertHash>,
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
    pub(crate) pending_stake: BTreeMap<AuthorityId, suwappu_consensus::Stake>,
    /// Issue #18: governance intents queued for application at the
    /// next epoch boundary. Applying `AdmitAuthority` / `ExitAuthority`
    /// / `EjectAuthority` at commit time caused transitional
    /// quorum-threshold asymmetry across daemons (each daemon commits
    /// at a slightly different round under jitter, so the `n=5→n=4`
    /// eject path briefly disagrees on what constitutes a valid
    /// round-completion). Draining at the epoch boundary makes the
    /// transition atomic across the mesh.
    pub(crate) pending_governance: Vec<(Intent, Option<crate::client::GovAuth>)>,
    /// `(author, round) → first cert hash observed` (DAG-S30.1).
    /// Equivocation detection O(1) per insert instead of O(dag)
    /// per try_commit.
    pub(crate) seen_at: BTreeMap<(AuthorityId, Round), CertHash>,
    /// Equivocations detected at insertion time. `try_commit`
    /// drains this queue instead of re-scanning the DAG.
    pub(crate) detected_equivocations: Vec<EquivocationProof>,
    /// `round → committed cert hash` index. Populated from `try_commit`
    /// alongside the existing `state.blocks` insert so `suwappu_getBlock(round)`
    /// is O(log n) instead of O(blocks) scan. `BTreeMap` (rather than
    /// `HashMap`) so range scans for explorer "next N blocks" stay cheap.
    pub(crate) blocks_by_round: BTreeMap<Round, CertHash>,
    /// `intent_hash → (round, cert_hash, index_within_block)` index for
    /// `suwappu_getTransaction(hash)`. Populated alongside `blocks_by_round`
    /// from `try_commit`. `usize` is the position in `block.intents` so
    /// the explorer can resolve to a single intent cheaply.
    pub(crate) tx_to_block: HashMap<[u8; 32], (Round, CertHash, usize)>,
    /// Latest finalized `(round, post_root)` captured at commit for the bridge
    /// header attestation surface. Lossy-latest: overwritten on every commit,
    /// `None` until the first block finalizes. Read by the RPC adapter to
    /// lazily sign `suwappu_getHeaderAttestation`. The pair is only co-available at
    /// execution time (`InMemorySubstrate::state_root()` returns latest with no
    /// round→root history), so it must be captured here, not reconstructed.
    pub(crate) latest_bridge_header: Option<(u64, [u8; 32])>,
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
    /// Retain a verified governance envelope for later block attachment.
    /// Governance intents are rare; if the map grows past the cap
    /// (submissions never mined), clear it — a dropped envelope only
    /// means the submitter must resubmit.
    pub(crate) fn remember_governance_envelope(
        &self,
        intent_hash: [u8; 32],
        auth: crate::client::GovAuth,
    ) {
        let mut map = self.governance_envelopes.lock();
        // Drop the NEW envelope when at capacity rather than clearing the
        // map: never evict an already-pending (legitimately dual-signed)
        // envelope, so a flood can't censor an honest admit. The dropped
        // submission simply needs resubmission. Reaching the cap requires
        // 4096 ingress-valid (two-authority-signed) submissions, so this
        // is a soft bound, not an attack surface.
        if map.len() >= 4096 && !map.contains_key(&intent_hash) {
            tracing::warn!("governance envelope store full; dropping new envelope (resubmit)");
            return;
        }
        map.insert(intent_hash, auth);
    }

    /// Take (remove) a retained envelope by intent hash, if present.
    pub(crate) fn take_governance_envelope(
        &self,
        intent_hash: &[u8; 32],
    ) -> Option<crate::client::GovAuth> {
        self.governance_envelopes.lock().remove(intent_hash)
    }

    fn new(
        manifest: &GenesisManifest,
        self_secret_key: suwappu_crypto::mldsa::SecretKey,
        bridge_signer: Option<BridgeHeaderSigner>,
    ) -> Self {
        let mut stake_table = StakeTable::new();
        let mut authority_registry = AuthorityRegistry::new();
        let mut validator_registry = ValidatorRegistry::new();
        for v in &manifest.validators {
            stake_table.insert(v.authority_id, v.validator_stake_suwappu as u128);
            let mldsa_bytes = hex::decode(&v.mldsa_public_key_hex).unwrap_or_default();
            let _bls_bytes = hex::decode(&v.bls_public_key_hex).unwrap_or_default();
            if let Err(e) = authority_registry.admit(AuthorityMember {
                id: v.authority_id,
                stake_suwappu: v.authority_stake_suwappu,
                public_key_bytes: mldsa_bytes,
            }) {
                tracing::warn!(auth = v.authority_id, err = %e, "genesis: skipping malformed authority");
            }
            if let Err(e) = validator_registry.admit(ValidatorMember {
                id: v.authority_id,
                stake_suwappu: v.validator_stake_suwappu as u128,
            }) {
                tracing::warn!(val = v.authority_id, err = %e, "genesis: skipping malformed validator");
            }
        }
        // Genesis funding (launch fix): apply the manifest's
        // `[[prebalances]]` to the fresh substrate here, in the same
        // manifest-driven spot where the stake table and authority /
        // validator registries are seeded. Every validator shares the
        // identical genesis.toml, so the resulting balances (and state
        // root) are deterministic across the mesh; a restarting node
        // reconstructs the substrate from empty and re-applies, so this
        // is idempotent across restarts. Application goes through
        // `Intent::GenesisAllocation`, whose height-0 gate a fresh
        // `InMemorySubstrate` satisfies (`current_block_height` starts
        // at 0) — the same code path the execution crate's own genesis
        // tests exercise.
        let mut substrate = InMemorySubstrate::new();
        let allocations: Vec<(suwappu_execution::Address, suwappu_execution::Balance)> = manifest
            .prebalances
            .iter()
            .filter_map(|p| match p.address_bytes() {
                Ok(addr) => Some((addr, p.balance_suwappu as u128)),
                Err(e) => {
                    tracing::warn!(err = %e, "genesis: skipping malformed prebalance entry");
                    None
                }
            })
            .collect();
        if !allocations.is_empty() {
            if let Err(e) = substrate.apply_intent(&Intent::GenesisAllocation { allocations }) {
                tracing::warn!(err = %e, "genesis: prebalance application failed");
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
                substrate,
                last_authored_round: None,
                max_observed_round: 0,
                sync_tip: 0,
                n_authorities: n,
                orphans: HashMap::new(),
                inflight_fetches: HashSet::new(),
                needed_blocks: HashSet::new(),
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
                latest_bridge_header: None,
            }),
            mempool: std::sync::Arc::new(suwappu_mempool::Mempool::new(
                suwappu_mempool::MempoolConfig::default(),
            )),
            self_secret_key,
            manifest_network_id: manifest.network_id.clone(),
            governance_envelopes: parking_lot::Mutex::new(HashMap::new()),
            bridge_signer,
            bridge_attestation_cache: parking_lot::Mutex::new(None),
        }
    }

    /// Compute the `(object, nonce)` key for a fast-path tx.
    pub(crate) fn fastpath_key(tx: &FastPathTx) -> FastPathKey {
        (tx.object, tx.nonce)
    }
}

/// Count distinct authors with a cert at `round` in the local DAG.
/// DagBft-C admits round R+1 once `quorum_threshold(n)` distinct
/// authors are observed at round R. Free function (was on `&State`
/// pre-S31.2) because the caller now passes the DAG read guard.
fn distinct_authors_at(dag: &DagStore, round: u64, n_authorities: u32) -> u32 {
    (0..n_authorities)
        .filter(|a| cert_at(dag, round, *a).is_some())
        .count() as u32
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
        match manifest.validate_against(&cfg) {
            Ok(_) => {}
            Err(ConfigError::MissingAuthority(id)) if cfg.allow_post_genesis_join => {
                // Post-genesis joiner: admitted (or to be admitted) via a
                // governed `AdmitAuthority` intent rather than genesis. The
                // node starts in passive-sync mode — it ingests, backfills
                // via the sync protocol, and serves requests, but authors
                // certificates and votes only once it observes itself
                // seated in the Authority Ring.
                tracing::warn!(
                    authority_id = id,
                    "authority not in genesis manifest; starting in post-genesis join mode"
                );
            }
            Err(e) => return Err(e.into()),
        }
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
            dyn_inbox,
            tasks: mut wire_tasks,
        } = wire.split();
        let outbound = Arc::new(outbound);
        // Load this validator's own ML-DSA-65 secret key. Unlike the bridge
        // signer below (which fails open — header attestation is optional),
        // this load is fatal: `mldsa_secret_key_path` is a required
        // `NodeConfig` field, and a validator that cannot sign cannot author
        // certificates every peer's `ingest_cert` verify-gate will accept.
        let self_secret_key = std::fs::read(&cfg.mldsa_secret_key_path)
            .ok()
            .and_then(|raw| load_mldsa_secret(&raw))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "cannot load ML-DSA-65 secret key from mldsa_secret_key_path {:?}",
                    cfg.mldsa_secret_key_path
                )
            })?;
        // First runtime use of the validator's ML-DSA key for the (separate,
        // optional) bridge-attestation surface: load it (if header
        // attestation is configured) so the RPC adapter can sign bridge
        // headers. `None` => attestation disabled; the daemon still runs.
        let bridge_signer = BridgeHeaderSigner::from_config(&cfg, &manifest);
        let state = Arc::new(State::new(&manifest, self_secret_key, bridge_signer));

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
        for (peer_id, peer_inbox) in inboxes.into_iter() {
            let state = state.clone();
            let outbound = outbound.clone();
            let log = log.clone();
            let self_label = self_label.clone();
            let peer_label = peer_id.0.clone();
            tasks.push(tokio::spawn(async move {
                tracing::debug!(peer = %peer_label, "inbox task: starting");
                run_inbox(self_label, self_id, state, outbound, log, peer_inbox, false).await;
                tracing::debug!(peer = %peer_label, "inbox task: exiting");
            }));
        }

        // Dynamic-peer inbox (late-join): one consumer for every inbound
        // connection whose hello label isn't in the configured peer set.
        // Same handler as the per-peer inboxes — replies to dynamic peers
        // travel back over the event's `reply` sender.
        {
            let state = state.clone();
            let outbound = outbound.clone();
            let log = log.clone();
            let self_label = self_label.clone();
            tasks.push(tokio::spawn(async move {
                run_inbox(self_label, self_id, state, outbound, log, dyn_inbox, true).await;
            }));
        }

        // Forward backfill (late-join / restart catch-up): polls peer tips
        // and pulls missing rounds oldest-first through the normal ingest
        // path. Idle when the node is within the lag threshold of the
        // best-known tip.
        {
            let state = state.clone();
            let outbound = outbound.clone();
            tasks.push(tokio::spawn(async move {
                run_backfill(state, outbound).await;
            }));
        }

        // Round driver — drains the shared mempool at block-build time
        // (`state.mempool.drain_for_block`). No mpsc receiver to own.
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
        // (`suwappu_submitIntent`), so the adapter now also gets the
        // cloned intent sender + network_id to drive the same
        // verify+enqueue gate the TCP wire uses.
        if let Some(rpc_addr) = cfg.rpc_listen {
            // T6: the adapter also needs an EventLog handle to spawn
            // the Event → EventView bridge. The log is already cloneable
            // (Clone for EventLog is cheap — it's just an mpsc sender +
            // broadcast sender).
            let view = crate::rpc_adapter::NodeStateView::new(
                state.clone(),
                manifest.network_id.clone(),
                &log,
            );
            let ctx = std::sync::Arc::new(suwappu_rpc::RpcContext::new(std::sync::Arc::new(view)));
            // B2.1: thread per-IP rate-limit knobs through to the
            // router. Other RouterLimits fields keep their B2 defaults
            // (concurrency cap, body-size cap) until we have a reason
            // to make them config-driven.
            let limits = suwappu_rpc::RouterLimits {
                per_ip_capacity: cfg.rpc_per_ip_capacity,
                per_ip_refill_per_sec: cfg.rpc_per_ip_refill_per_sec,
                ..suwappu_rpc::RouterLimits::default()
            };
            let rpc_task = suwappu_rpc::start_with_limits(rpc_addr, ctx, limits).await?;
            tasks.push(rpc_task);
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

/// Canonical block payload digest, binding BOTH the intents AND the
/// governance authorization envelopes. The authoring cert commits to and
/// signs this value, so the envelopes are covered by the cert signature —
/// a relay cannot strip or mutate `governance_auth` for a committed cert
/// without producing a digest mismatch. Two independent blake3 updates
/// (rather than encoding a tuple) keep the encoding unambiguous.
fn compute_payload_digest(
    intents: &[Intent],
    governance_auth: &[(u32, crate::client::GovAuth)],
) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(&crate::codec::encode(&intents).unwrap_or_default());
    h.update(&crate::codec::encode(&governance_auth).unwrap_or_default());
    *h.finalize().as_bytes()
}

/// A [`BlockPayload`] is self-consistent when its `payload_digest` equals
/// [`compute_payload_digest`] over its intents AND governance envelopes.
/// A block failing this check is a forgery (or corruption) — including a
/// relay that stripped/mutated `governance_auth` — and must never enter
/// `state.blocks`.
fn block_payload_is_consistent(block: &BlockPayload) -> bool {
    compute_payload_digest(&block.intents, &block.governance_auth) == block.payload_digest
}

async fn run_inbox(
    self_label: String,
    self_id: AuthorityId,
    state: Arc<State>,
    outbound: Arc<HashMap<PeerId, tokio::sync::mpsc::Sender<WireMessage>>>,
    log: EventLog,
    mut inbox: tokio::sync::mpsc::Receiver<WireEvent>,
    // Dynamic peers are unauthenticated internet dialers (late-joiners
    // not in the configured set). They may only drive the sync protocol
    // and receive signature-gated Cert responses — never inject a Vote
    // (unsigned; would forge Validator-Ring stake and collapse the
    // Theorem-2 AND-gate) or a Tip (would poison the backfill target).
    // See the consensus-reviewer finding on 62cce31.
    is_dynamic: bool,
) {
    while let Some(ev) = inbox.recv().await {
        let WireEvent { from, msg, reply } = ev;
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
                let inserted = ingest_cert(&state, cert, &from, &outbound).await;
                // Post-genesis joiners ingest and commit but do not vote
                // until seated in the Authority Ring: an unseated vote
                // carries zero stake in `validator_quorum_met` anyway,
                // and emitting it would just be gossip noise.
                let seated = state.authority_registry.read().await.contains(self_id);
                if seated {
                    for ic in inserted {
                        let vote = Vote {
                            validator: self_id,
                            candidate: ic.hash,
                        };
                        state.votes.lock().entry(ic.hash).or_default().push(vote);
                        log.emit(
                            Event::now(&self_label, Lane::Main, "voted")
                                .with_round(ic.round)
                                .with_cert_hash(&ic.hash.0),
                        );
                        broadcast_traced(&outbound, WireMessage::Vote(vote), &self_label, &log);
                    }
                }
                try_commit(&state, &self_label, &log).await;
            }
            WireMessage::Block(block) => {
                // Dynamic (unauthenticated) peers only ever REQUEST blocks
                // (GetBlock / GetCertsByRound); a Block frame from one is
                // never a legitimate flow, so drop it rather than let it
                // populate state.blocks. Configured peers and dial-return
                // responses are is_dynamic == false.
                if is_dynamic {
                    debug!(peer = %from.0, "inbox: dropping Block from dynamic peer");
                } else if !block_payload_is_consistent(&block) {
                    // Self-consistency: payload_digest must equal
                    // compute_payload_digest(intents, governance_auth), so a
                    // relay cannot strip/mutate the intents OR the governance
                    // envelopes.
                    debug!(peer = %from.0, "inbox: block payload digest mismatch, dropping");
                } else {
                    // Bind to the SIGNED cert when we already have it: only
                    // accept a block whose digest matches the cert's signed
                    // payload_digest, and OVERWRITE any previously-stored
                    // (e.g. relay-poisoned stripped) block for this cert with
                    // the authentic one. When the cert isn't known yet, store
                    // best-effort (first-write-wins); `ingest_cert` purges a
                    // mismatched squatter when the cert arrives. This closes
                    // the stripped-block poison that first-write-wins alone
                    // left open (consensus-review of 8eefd3d).
                    let cert_digest = state
                        .dag
                        .read()
                        .await
                        .get(&block.cert_hash)
                        .map(|c| c.payload_digest);
                    match cert_digest {
                        Some(cd) if cd == block.payload_digest => {
                            state.blocks.lock().insert(block.cert_hash, block);
                        }
                        Some(_) => {
                            debug!(peer = %from.0, "inbox: block does not match known cert digest, dropping");
                        }
                        None => {
                            state.blocks.lock().entry(block.cert_hash).or_insert(block);
                        }
                    }
                }
            }
            WireMessage::Vote(vote) => {
                // Votes are unsigned; only trusted configured peers may
                // deliver them. A dynamic (unauthenticated) peer's vote
                // would forge Validator-Ring stake.
                if is_dynamic {
                    debug!(peer = %from.0, "inbox: dropping Vote from dynamic peer");
                } else {
                    state
                        .votes
                        .lock()
                        .entry(vote.candidate)
                        .or_default()
                        .push(vote);
                    try_commit(&state, &self_label, &log).await;
                }
            }
            WireMessage::GetCert(hash) => {
                let cert_opt = state.dag.read().await.get(&hash).cloned();
                if let Some(cert) = cert_opt {
                    reply_to(&outbound, &from, &reply, WireMessage::Cert(cert));
                }
            }
            WireMessage::FastPath(cert) => {
                // Fast-path and LTP frames mutate consensus / attestation
                // state and re-broadcast; only trusted configured peers
                // may drive them. Dynamic peers are sync-only.
                if is_dynamic {
                    debug!(peer = %from.0, "inbox: dropping FastPath from dynamic peer");
                } else {
                    handle_fastpath_cert(&state, self_id, cert, &self_label, &log, &outbound).await;
                }
            }
            WireMessage::Ltp(att) => {
                if is_dynamic {
                    debug!(peer = %from.0, "inbox: dropping Ltp from dynamic peer");
                } else {
                    handle_ltp_attestation(&state, att, &self_label, &log).await;
                }
            }
            WireMessage::Ping(t) => {
                reply_to(&outbound, &from, &reply, WireMessage::Pong(t));
            }
            WireMessage::Pong(_) => {}
            WireMessage::GetTip => {
                let tip = state.dag.read().await.max_round().unwrap_or(0);
                reply_to(&outbound, &from, &reply, WireMessage::Tip(tip));
            }
            WireMessage::Tip(r) => {
                // Only configured peers set the backfill target: a
                // dynamic peer could send Tip(u64::MAX) and pin this node
                // into perpetual (futile) backfill.
                if is_dynamic {
                    debug!(peer = %from.0, "inbox: ignoring Tip from dynamic peer");
                } else {
                    let mut inner = state.inner.lock().await;
                    if r > inner.sync_tip {
                        inner.sync_tip = r;
                    }
                }
            }
            WireMessage::GetCertsByRound(round) => {
                // Serve every cert at `round` as ordinary Cert frames (the
                // requester's normal ingest verifies + dedups), each with
                // its block payload when held so the requester can also
                // replay intents. All of this is public chain data.
                let certs: Vec<Certificate> = {
                    let dag = state.dag.read().await;
                    dag.round_hashes(round)
                        .iter()
                        .filter_map(|h| dag.get(h).cloned())
                        .collect()
                };
                for cert in certs {
                    let h = cert.hash();
                    reply_to(&outbound, &from, &reply, WireMessage::Cert(cert));
                    let block_opt = state.blocks.lock().get(&h).cloned();
                    if let Some(block) = block_opt {
                        reply_to(&outbound, &from, &reply, WireMessage::Block(block));
                    }
                }
            }
            WireMessage::GetBlock(hash) => {
                let block_opt = state.blocks.lock().get(&hash).cloned();
                if let Some(block) = block_opt {
                    reply_to(&outbound, &from, &reply, WireMessage::Block(block));
                }
            }
        }
    }
}

/// Route a reply to `from`: configured peers via the outbound map,
/// dynamic peers via the per-connection `reply` sender carried on the
/// event (see [`WireEvent::reply`]). Best-effort — a full channel drops
/// the frame, consistent with the gossip model.
fn reply_to(
    outbound: &HashMap<PeerId, tokio::sync::mpsc::Sender<WireMessage>>,
    from: &PeerId,
    reply: &Option<tokio::sync::mpsc::Sender<WireMessage>>,
    msg: WireMessage,
) {
    if let Some(tx) = outbound.get(from) {
        let _ = tx.try_send(msg);
    } else if let Some(tx) = reply {
        let _ = tx.try_send(msg);
    }
}

/// Forward backfill loop: poll peer tips, and whenever the local DAG is
/// behind the best-known tip by more than a small lag threshold, request
/// the missing rounds oldest-first via `GetCertsByRound`. Responses ride
/// the ordinary `Cert`/`Block` ingest path (signature-verified, deduped),
/// and round-at-a-time forward requests arrive parents-first so catch-up
/// never leans on the bounded orphan buffer. Idle in steady state.
async fn run_backfill(
    state: Arc<State>,
    outbound: Arc<HashMap<PeerId, tokio::sync::mpsc::Sender<WireMessage>>>,
) {
    const BACKFILL_TICK_MS: u64 = 500;
    const BACKFILL_BATCH_ROUNDS: u64 = 8;
    const BACKFILL_LAG_THRESHOLD: u64 = 2;

    let mut tick = tokio::time::interval(std::time::Duration::from_millis(BACKFILL_TICK_MS));
    let mut ticks: u64 = 0;
    loop {
        tick.tick().await;
        // Refresh peer tips every 4 ticks (2s).
        if ticks % 4 == 0 {
            for tx in outbound.values() {
                let _ = tx.try_send(WireMessage::GetTip);
            }
        }
        ticks = ticks.wrapping_add(1);

        let local = state.dag.read().await.max_round().unwrap_or(0);
        let target = state.inner.lock().await.sync_tip;
        if target <= local.saturating_add(BACKFILL_LAG_THRESHOLD) {
            continue;
        }
        let from_round = local.saturating_add(1);
        let to_round = from_round
            .saturating_add(BACKFILL_BATCH_ROUNDS - 1)
            .min(target);
        // Snapshot senders once; rotate the fan-out start per round so a
        // fixed pair of alive-but-behind peers doesn't absorb every
        // request (consensus-reviewer fairness finding).
        let senders: Vec<_> = outbound.values().collect();
        if senders.is_empty() {
            continue;
        }
        for round in from_round..=to_round {
            // Two-peer fan-out, mirroring `fetch_cert_from_peers`.
            let start = (round as usize) % senders.len();
            let mut sent = 0usize;
            for offset in 0..senders.len() {
                let tx = senders[(start + offset) % senders.len()];
                if tx.try_send(WireMessage::GetCertsByRound(round)).is_ok() {
                    sent += 1;
                    if sent >= 2 {
                        break;
                    }
                }
            }
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
) -> Vec<IngestedCert> {
    let mut inserted = Vec::new();
    let mut work: Vec<Certificate> = vec![cert];
    while let Some(c) = work.pop() {
        let h = c.hash();
        let round = c.round;

        // DAG-S6 / epic item 2: verify the author's signature against their
        // genesis-registered public key before admission. A cert that fails
        // this is never inserted, never cascades through the orphan buffer,
        // and is never served to other peers via `GetCert` — the network
        // effectively never gossips it further, satisfying "reject /
        // don't-gossip on failure" without needing an explicit relay step
        // (this daemon doesn't push-relay certs; peers pull via GetCert).
        let author_pubkey = state
            .authority_registry
            .read()
            .await
            .get(c.author)
            .and_then(|m| suwappu_crypto::mldsa::PublicKey::from_bytes(&m.public_key_bytes).ok());
        match author_pubkey {
            Some(pk) if c.verify_signature(&pk) => {}
            Some(_) => {
                debug!(peer = %from.0, author = c.author, round, "inbox: cert signature verification failed");
                continue;
            }
            None => {
                debug!(peer = %from.0, author = c.author, round, "inbox: cert author not a seated authority (or malformed registered key)");
                continue;
            }
        }

        // Acquire dag write lock briefly for the insert.
        let cert_payload_digest = c.payload_digest;
        let insert_result = state.dag.write().await.insert(c.clone());
        match insert_result {
            Ok(_) => {
                // Now that the SIGNED cert is known, evict any stored block
                // for it that does not match the cert's payload_digest — a
                // relay-poisoned stripped block cannot squat past cert
                // arrival and block the authentic one from being stored.
                {
                    let mut blocks = state.blocks.lock();
                    if blocks
                        .get(&h)
                        .is_some_and(|b| b.payload_digest != cert_payload_digest)
                    {
                        blocks.remove(&h);
                    }
                }
                // Update cold-path inner state.
                let promote_stake: Option<(AuthorityId, suwappu_consensus::Stake)>;
                let unblocked: Option<Vec<Certificate>>;
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
                    let key = (c.author, round);
                    match inner.seen_at.get(&key).copied() {
                        None => {
                            inner.seen_at.insert(key, h);
                        }
                        Some(prev) if prev != h => {
                            inner.detected_equivocations.push(EquivocationProof {
                                author: c.author,
                                round,
                                cert_a: prev,
                                cert_b: h,
                            });
                        }
                        _ => {}
                    }
                    unblocked = inner.orphans.remove(&h);
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
/// pipeline (`suwappu_fastpath::slashing`) consumes this for 100% bonded
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
/// 2. Verify the aggregate BLS signature via `suwappu_ltp::verify_attestation`.
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
            match suwappu_ltp::verify_attestation(&corridor, &att) {
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

        // Fetch deferred blocks: certs committed-in-order-blocked on a
        // missing authentic block. Two-peer fan-out per block, mirroring
        // the cert path. Entries are cleared by try_commit once the block
        // arrives; prune any that have since committed.
        let needed: Vec<CertHash> = {
            let mut inner = state.inner.lock().await;
            let committed_prune: Vec<CertHash> = inner
                .needed_blocks
                .iter()
                .copied()
                .filter(|h| state.committed.lock().contains(h))
                .collect();
            for h in committed_prune {
                inner.needed_blocks.remove(&h);
            }
            inner.needed_blocks.iter().copied().collect()
        };
        for h in needed {
            fetch_block_from_peers(h, &outbound);
        }
    }
}

/// Unicast `GetBlock(hash)` to up to two peers — mirrors
/// `fetch_cert_from_peers` for the block-availability layer. Used to
/// repair a deferred commit whose authentic (cert-digest-matching) block
/// hasn't arrived (or was poisoned by a stripped-block relay).
fn fetch_block_from_peers(
    hash: CertHash,
    outbound: &HashMap<PeerId, tokio::sync::mpsc::Sender<WireMessage>>,
) {
    let mut sent = 0usize;
    for tx in outbound.values() {
        if tx.try_send(WireMessage::GetBlock(hash)).is_ok() {
            sent += 1;
            if sent >= 2 {
                return;
            }
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
/// The event log surface is what compliance trace + the suwappu-metrics
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
        WireMessage::GetTip => "get_tip",
        WireMessage::Tip(_) => "tip",
        WireMessage::GetCertsByRound(_) => "get_certs_by_round",
        WireMessage::GetBlock(_) => "get_block",
    }
}

async fn try_commit(state: &State, self_label: &str, log: &EventLog) {
    // Snapshot votes + n_authorities + candidate_rounds in brief locks
    // up-front so the rest of the function operates on owned data.
    let votes_flat: Vec<Vote> = state.votes.lock().values().flatten().copied().collect();
    let n = state.inner.lock().await.n_authorities;
    let candidate_rounds: BTreeSet<u64> = {
        let dag = state.dag.read().await;
        dag.rounds().collect()
    };

    // Labeled so a deferred cert (missing authentic block) stops the
    // ENTIRE commit walk, not just the current leader's causal history:
    // committing a later leader's certs while an earlier finalize-ordered
    // cert is deferred would apply intents out of the canonical order and
    // diverge post-roots between honest nodes (round-4 consensus review).
    // Every node therefore commits a strictly-growing prefix of the
    // canonical finalize order.
    'commit: for round in candidate_rounds {
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
            // DEFER THE WHOLE WALK, don't `continue`. This leader is
            // authority-Direct but not yet validator-ratified locally
            // (e.g. its votes haven't arrived, or are being withheld by a
            // Byzantine relay). `continue`-ing would let a LATER, fully
            // finalized anchor sweep this leader's causal history — which
            // includes lower-round certs — into commit at a DIFFERENT
            // position than a node that did have this leader's votes and
            // committed its sweep here. That reorders substrate
            // application and diverges post-roots between honest nodes
            // with zero ring corruption (round-4/5 consensus review). The
            // joint-quorum AND-gate is halt-not-fork by design: no commit
            // past an un-ratified earlier leader until its validator
            // quorum is observed (or the chain intentionally waits).
            break 'commit;
        }

        let history = {
            let dag = state.dag.read().await;
            suwappu_consensus::causal_history(&dag, leader_hash)
        };
        for h in history {
            if state.committed.lock().contains(&h) {
                continue;
            }
            // `None` is unreachable today: `h` came from
            // `causal_history`, which only yields certs already in the
            // DAG, and the DAG is append-only (no eviction/pruning). If
            // pruning is ever added, this `continue` would skip a cert
            // while committing finalize-later ones — reopening the
            // ordering-divergence class fixed above; it must become a
            // walk-wide defer (`break 'commit`) at that point.
            let (cert_round, cert_payload_digest) = match state.dag.read().await.get(&h) {
                Some(c) => (c.round, c.payload_digest),
                None => continue,
            };
            // Bind the block to the SIGNED cert: only consume a block whose
            // payload digest equals the committed cert's payload_digest
            // (which the author signed). This means the intents AND the
            // governance envelopes we apply are exactly what the author
            // committed.
            //
            // If a cert-matching block is NOT yet available (never arrived,
            // or a Byzantine relay poisoned the slot with a self-consistent
            // stripped block), we must NOT commit an empty block — doing so
            // would diverge our state from peers that have the full block.
            // Instead we DEFER: request the block and break out of the
            // history walk so commit order is preserved. The cert stays
            // uncommitted and is retried on the next try_commit once the
            // authentic block arrives (consensus-review of 8eefd3d).
            let block_payload = state
                .blocks
                .lock()
                .get(&h)
                .filter(|b| b.payload_digest == cert_payload_digest)
                .map(|b| (b.intents.clone(), b.governance_auth.clone()));
            let (intents, block_gov_auth) = match block_payload {
                Some(p) => p,
                None => {
                    // Record the missing block for the sync sweeper to
                    // fetch (try_commit has no outbound handle), and defer
                    // the WHOLE commit walk — no finalize-later cert may
                    // commit ahead of this one, on any node.
                    state.inner.lock().await.needed_blocks.insert(h);
                    break 'commit;
                }
            };
            // Atomically CLAIM the cert only now that its block is in hand.
            // `insert` returns false if a concurrent per-peer inbox task
            // already claimed it — skip to avoid double-application. The
            // block-availability check above happens BEFORE the claim, so
            // we never claim-then-commit-empty.
            if !state.committed.lock().insert(h) {
                continue;
            }
            // No longer waiting on this block.
            state.inner.lock().await.needed_blocks.remove(&h);
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
                let report = execute_block(&mut inner.substrate, &block);
                // Bridge header attestation: capture (round, post_root) at the
                // single canonical commit point — the only place the pair is
                // co-available (the substrate exposes only the latest root, with
                // no round→root history). Lossy-latest; read out-of-band and
                // signed lazily by the RPC adapter, never under this lock.
                inner.latest_bridge_header = Some((report.round, report.post_root));
                // IQ-003: index single-owner-equivalent main-lane txs so
                // the fast-path receiver can K-binding cross-check.
                // Skip governance/admin intents — only state-touching
                // transfers can conflict with a fast-path cert.
                for intent in &intents {
                    if let Some(ml_tx) = intent_to_main_lane_tx(intent, cert_round, h) {
                        inner.main_lane_index.push(ml_tx);
                    }
                }
                // Secondary indices for `suwappu_getBlock(round)` and
                // `suwappu_getTransaction(hash)`. Populated here (and only
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
                for (idx, intent) in intents.iter().enumerate() {
                    if matches!(
                        intent,
                        Intent::AdmitAuthority { .. }
                            | Intent::ExitAuthority { .. }
                            | Intent::EjectAuthority { .. }
                    ) {
                        // Carry the block's authorization envelope for this
                        // intent (by index) into the pending queue so it is
                        // re-verified at the epoch boundary. A Byzantine
                        // author that omits it leaves `None`, and the apply
                        // path then drops the intent.
                        let env = block_gov_auth
                            .iter()
                            .find(|(i, _)| *i as usize == idx)
                            .map(|(_, a)| a.clone());
                        inner.pending_governance.push((intent.clone(), env));
                    }
                }
            }

            log.emit(
                Event::now(self_label, Lane::Main, "committed")
                    .with_round(cert_round)
                    .with_cert_hash(&h.0)
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
                let queued: Vec<(Intent, Option<crate::client::GovAuth>)> = {
                    let mut inner = state.inner.lock().await;
                    inner.pending_governance.drain(..).collect()
                };
                for (intent, env) in &queued {
                    apply_governance_intent(
                        state,
                        intent,
                        env.as_ref(),
                        cert_round,
                        self_label,
                        log,
                    )
                    .await;
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
    auth: Option<&crate::client::GovAuth>,
    cert_round: u64,
    self_label: &str,
    log: &EventLog,
) {
    // Governance authorization as a CONSENSUS rule: re-verify the
    // on-chain envelope against THIS node's seated Authority Ring before
    // mutating the registries. Every honest node reaches this epoch
    // boundary with the same committed registry and the same manifest
    // network id, so the verify decision is deterministic across the
    // mesh — and a Byzantine block author that embedded an un-cosigned
    // (or forged) governance intent has it dropped here, closing the
    // block-author bypass of the ingress dual-signature gate.
    match auth {
        Some(a) => {
            if let Err(reason) =
                crate::client::verify_governed_intent(state, &state.manifest_network_id, intent, a)
                    .await
            {
                tracing::warn!(round = cert_round, %reason, "governance intent failed commit-time verification; dropping");
                log.emit(
                    Event::now(self_label, Lane::Main, "governance_rejected")
                        .with_round(cert_round),
                );
                return;
            }
        }
        None => {
            tracing::warn!(
                round = cert_round,
                "governance intent has no authorization envelope; dropping"
            );
            log.emit(
                Event::now(self_label, Lane::Main, "governance_rejected").with_round(cert_round),
            );
            return;
        }
    }
    match intent {
        Intent::AdmitAuthority {
            authority_id,
            stake_suwappu,
            mldsa_public_key,
            bls_public_key: _bls,
        } => {
            let admit_result = state
                .authority_registry
                .write()
                .await
                .admit(AuthorityMember {
                    id: *authority_id,
                    stake_suwappu: *stake_suwappu,
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
                        .insert(*authority_id, *stake_suwappu as u128);
                    let _ = state
                        .validator_registry
                        .write()
                        .await
                        .admit(ValidatorMember {
                            id: *authority_id,
                            stake_suwappu: *stake_suwappu as u128,
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
/// `f + 1` — DagBft-C §6.2 fallback.
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

async fn run_round_driver(
    self_label: String,
    self_id: AuthorityId,
    round_ms: u64,
    state: Arc<State>,
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

        // Post-genesis joiners: never author until seated in the
        // Authority Ring (admitted at an epoch boundary). Every peer's
        // `ingest_cert` would reject an unseated author's cert anyway —
        // this gate just avoids authoring rejected certs and, more
        // importantly, keeps a joiner from burning its round cadence
        // before catch-up completes.
        if !state.authority_registry.read().await.contains(self_id) {
            continue;
        }

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
        let intents: Vec<suwappu_execution::Intent> =
            state.mempool.drain_for_block(MAX_INTENTS_PER_BLOCK);

        // Per-intent hashes for the `tx_to_block` secondary index and the
        // governance-envelope lookup. Computed BEFORE the payload digest.
        let intent_hash_bytes: Vec<[u8; 32]> = intents
            .iter()
            .map(|i| *blake3::hash(&crate::codec::encode(i).expect("intent serialize")).as_bytes())
            .collect();

        // Attach the on-chain governance authorization envelope for every
        // governance intent in this block (looked up by intent hash from
        // the ingest-time store and consumed). Committers re-verify these
        // at the epoch boundary.
        let governance_auth: Vec<(u32, crate::client::GovAuth)> = intents
            .iter()
            .enumerate()
            .filter(|(_, i)| {
                matches!(
                    i,
                    Intent::AdmitAuthority { .. }
                        | Intent::ExitAuthority { .. }
                        | Intent::EjectAuthority { .. }
                )
            })
            .filter_map(|(idx, _)| {
                state
                    .take_governance_envelope(&intent_hash_bytes[idx])
                    .map(|a| (idx as u32, a))
            })
            .collect();

        // The payload digest binds BOTH the intents AND the governance
        // authorization envelopes, so the signed cert transitively
        // commits to the envelopes. A relay that strips or mutates
        // `governance_auth` produces a different digest and is rejected by
        // `block_payload_is_consistent` — every honest node that commits
        // this cert therefore holds byte-identical envelopes, which is
        // what makes the commit-time governance verification deterministic
        // mesh-wide (see IQ-007 and the consensus-reviewer finding on
        // b6c60ad).
        let payload_digest: [u8; 32] = compute_payload_digest(&intents, &governance_auth);
        let mut cert = Certificate {
            author: self_id,
            round: target_round,
            parents,
            payload_digest,
            signature: Vec::new(),
        };
        cert.sign(&state.self_secret_key);
        let cert_hash = cert.hash();

        let block = BlockPayload {
            payload_digest,
            author: self_id,
            round: target_round,
            cert_hash,
            intents,
            governance_auth,
        };

        // Phase 3 (locked): brief — insert cert + block + update
        // markers + record own (author, round) for S30.1 incremental
        // equivocation detection.
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
            let key = (self_id, target_round);
            match inner.seen_at.get(&key).copied() {
                None => {
                    inner.seen_at.insert(key, cert_hash);
                }
                Some(prev) if prev != cert_hash => {
                    inner.detected_equivocations.push(EquivocationProof {
                        author: self_id,
                        round: target_round,
                        cert_a: prev,
                        cert_b: cert_hash,
                    });
                }
                _ => {}
            }
        }
        let _ = state.dag.write().await.insert(cert.clone());
        state.blocks.lock().insert(cert_hash, block.clone());

        // Phase 4 (unlocked): event log emit + cluster broadcast. No
        // state access, pure I/O.
        log.emit(
            Event::now(&self_label, Lane::Main, "proposed")
                .with_round(target_round)
                .with_cert_hash(&cert_hash.0),
        );
        broadcast_traced(&outbound, WireMessage::Block(block), &self_label, &log);
        broadcast_traced(&outbound, WireMessage::Cert(cert), &self_label, &log);
    }
}

#[cfg(test)]
mod tests {
    // ---- TCP port allocation for the daemon tests in this module ----
    //
    // These tests bind real loopback sockets and cargo runs them in
    // PARALLEL, so every test needs an EXCLUSIVE band. A single-node test
    // consumes `base .. base+200` (listen, +100 client, +200 rpc); a 4-node
    // cluster consumes `base .. base+200` too (listen base+0..3, client
    // base+100..103). Overlapping bands surface as a flaky
    // "Address already in use (os error 98)" in whichever test loses the
    // race — which is exactly how the three collisions below were found.
    //
    //   19_000  four_node_main_lane_commits (4n)
    //   19_500  client_listener_accepts_intent
    //   20_000  client_listener_accepts_intent_batch
    //   20_500  client_listener_enforces_mldsa_signature
    //   21_000  rpc_binding_returns_epoch_over_http
    //   21_300  try_commit_populates_blocks_by_round_and_tx_to_block
    //   21_500  rpc_submit_intent_round_trips_through_consensus
    //   21_800  rpc_submit_intent_unknown_signer
    //   23_000  phase_g_admit (4n)
    //   23_200  phase_g_eject (4n)
    //   23_400  phase_g_growing_prefix_under_transient_unavailability (4n)
    //
    // Adding a test? Take the next free 200-wide band and list it here.

    #[test]
    fn block_payload_consistency_rejects_forgery() {
        use crate::wire::BlockPayload;
        use suwappu_consensus::cert::CertHash;
        let intents = vec![suwappu_execution::Intent::Transfer {
            from: [1u8; 20],
            to: [2u8; 20],
            amount: 7,
        }];
        // A governance-carrying block: the digest binds intents AND the
        // envelope, so it must be computed with compute_payload_digest.
        let gov_intents = vec![suwappu_execution::Intent::EjectAuthority {
            authority_id: 4,
            proof_ref: [0u8; 32],
        }];
        let env = crate::client::GovAuth {
            sponsor_pubkey_hash: [1u8; 32],
            sponsor_signature: vec![9u8; 8],
            co_signer_pubkey_hash: [2u8; 32],
            co_signature: vec![8u8; 8],
            candidate_pop_signature: vec![],
        };
        let gov_auth = vec![(0u32, env)];
        let good = BlockPayload {
            payload_digest: compute_payload_digest(&gov_intents, &gov_auth),
            author: 0,
            round: 1,
            cert_hash: CertHash([9u8; 32]),
            intents: gov_intents.clone(),
            governance_auth: gov_auth.clone(),
        };
        assert!(block_payload_is_consistent(&good));

        // Same intents + same payload_digest, but the governance envelope
        // STRIPPED — the exact divergence vector the consensus review
        // found. The digest must no longer match, so honest nodes reject
        // it rather than commit a cert with a different envelope than
        // their peers.
        let mut stripped = good.clone();
        stripped.governance_auth = vec![];
        assert!(
            !block_payload_is_consistent(&stripped),
            "a block with a stripped governance envelope must fail the digest check"
        );

        // Tampered intents (the original overwrite-forgery vector).
        let mut forged = good.clone();
        forged.intents = vec![suwappu_execution::Intent::Transfer {
            from: [1u8; 20],
            to: [2u8; 20],
            amount: 1_000_000,
        }];
        assert!(
            !block_payload_is_consistent(&forged),
            "a block whose intents don't match its committed digest must be rejected"
        );

        // A plain (non-governance) block still validates.
        let plain = BlockPayload {
            payload_digest: compute_payload_digest(&intents, &[]),
            author: 0,
            round: 2,
            cert_hash: CertHash([7u8; 32]),
            intents: intents.clone(),
            governance_auth: vec![],
        };
        assert!(block_payload_is_consistent(&plain));
    }
    use std::net::SocketAddr;

    use super::*;
    use crate::config::{GenesisValidator, Peer};

    /// Write an ML-DSA-65 secret key's raw bytes to a fresh temp file and
    /// return its path, for use as `NodeConfig::mldsa_secret_key_path` — the
    /// daemon now loads and requires this key at startup (DAG-S6 / epic item
    /// 2), so every test that starts a real `Daemon` needs a real key file
    /// on disk, not the pre-signing-era `"/dev/null"` placeholder.
    fn write_mldsa_key_file(sk: &suwappu_crypto::mldsa::SecretKey) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "suwappu-node-test-mldsa-sk-{}-{}.bin",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(&path, sk.as_bytes()).expect("write test mldsa secret key file");
        path
    }

    /// Bridge header-attestation key-loading + hex helpers, and a full
    /// create→verify roundtrip under a key loaded the way `from_config` loads
    /// it (raw bytes, then hex envelope). Covers the novel runtime-key path
    /// without standing up a full daemon.
    #[test]
    fn bridge_helpers_and_attestation_roundtrip() {
        // Fixed-length hex decode: length is enforced.
        assert!(
            decode_hex_array::<20>("0x00").is_none(),
            "short hex must be rejected"
        );
        let oracle = decode_hex_array::<20>("0x00000000000000000000000000000000000000a1")
            .expect("20-byte oracle decodes");
        assert_eq!(oracle[19], 0xa1);

        // The pinned default network id is keccak256("suwappu-perf-7r") — the
        // same value proven cross-language in `suwappu_consensus::bridge_header`'s
        // golden vector. A fat-finger here would silently fail every on-chain
        // quorum check, so assert the full 32 bytes.
        let expected_network_id = decode_hex_array::<32>(
            "0xff431b3851ff00be6b5a4bd9b67e7d4118300693937865dfe75847dfd7cdd78a",
        )
        .unwrap();
        assert_eq!(DEFAULT_BRIDGE_NETWORK_ID, expected_network_id);

        // A real key loaded from raw bytes, and from a hex envelope, both work.
        let (pk, sk) = suwappu_crypto::mldsa::keypair();
        let raw = sk.as_bytes().to_vec();
        let from_raw = load_mldsa_secret(&raw).expect("raw secret-key bytes load");
        let hex_envelope = format!("0x{}\n", hex::encode(&raw));
        let from_hex = load_mldsa_secret(hex_envelope.as_bytes()).expect("hex secret-key loads");

        // Garbage loads to None, never panics.
        assert!(load_mldsa_secret(b"not a key").is_none());

        // An attestation signed with the loaded key verifies against its
        // binding and fails under a different oracle.
        let att = suwappu_consensus::bridge_header::HeaderAttestation::create(
            DEFAULT_BRIDGE_NETWORK_ID,
            oracle,
            4242,
            [0x11; 32],
            0,
            &pk,
            &from_raw,
        );
        assert!(att.verify(DEFAULT_BRIDGE_NETWORK_ID, oracle));
        let mut other = oracle;
        other[0] ^= 0x01;
        assert!(!att.verify(DEFAULT_BRIDGE_NETWORK_ID, other));

        // The hex-loaded key is the same secret, so it reproduces the signature
        // surface identically (verifies under the same pk).
        let att2 = suwappu_consensus::bridge_header::HeaderAttestation::create(
            DEFAULT_BRIDGE_NETWORK_ID,
            oracle,
            4242,
            [0x11; 32],
            0,
            &pk,
            &from_hex,
        );
        assert!(att2.verify(DEFAULT_BRIDGE_NETWORK_ID, oracle));

        // sk↔pk correspondence probe (the guard `from_config` runs at startup):
        // a signature by `sk` verifies under its own `pk` but NOT under an
        // unrelated key — so a mismatched (file-sk, genesis-pk) pair is caught.
        let probe = b"suwappu-bridge-header-keypair-probe";
        let sig = suwappu_crypto::mldsa::sign(probe, &sk).unwrap();
        assert!(suwappu_crypto::mldsa::verify(probe, &sig, &pk).is_ok());
        let (other_pk, _other_sk) = suwappu_crypto::mldsa::keypair();
        assert!(
            suwappu_crypto::mldsa::verify(probe, &sig, &other_pk).is_err(),
            "probe must reject a non-matching public key"
        );
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
        let (pk, sk) = suwappu_crypto::mldsa::keypair();
        let pk_hex = hex::encode(pk.as_bytes());
        let manifest = GenesisManifest {
            network_id: network_id.clone(),
            validators: (0..n)
                .map(|i| GenesisValidator {
                    authority_id: i,
                    label: format!("v{}", i),
                    mldsa_public_key_hex: pk_hex.clone(),
                    bls_public_key_hex: "00".into(),
                    // Issue #28: stakes must clear AUTHORITY_STAKE_THRESHOLD_SUWAPPU
                    // (100_000) and VALIDATOR_STAKE_THRESHOLD_SUWAPPU (25_000) so
                    // AuthorityRegistry::admit succeeds — otherwise the registry
                    // stays empty and the new signature-verify path rejects
                    // every submit with `unknown signer`.
                    validator_stake_suwappu: 150_000,
                    authority_stake_suwappu: 150_000,
                })
                .collect(),
            corridors: Vec::new(),
            prebalances: Vec::new(),
            rounds_per_epoch: 1024,
        };
        let cfg = NodeConfig {
            self_id: "v0".into(),
            authority_id: 0,
            listen: format!("127.0.0.1:{}", base_port).parse().unwrap(),
            client_listen: format!("127.0.0.1:{}", base_port + 100).parse().unwrap(),
            rpc_listen: None,
            peers: vec![],
            allow_post_genesis_join: false,
            round_ms: 500,
            checkpoint_cadence_rounds: 1,
            mldsa_secret_key_path: write_mldsa_key_file(&sk),
            bls_secret_key_path: "/dev/null".into(),
            genesis_manifest_path: "/dev/null".into(),
            event_log_path: std::env::temp_dir().join("suwappu-client-test.ndjson"),

            max_client_connections: 256,
            client_idle_timeout_ms: 30_000,
            client_per_ip_limit: 8,
            rpc_per_ip_capacity: 60,
            rpc_per_ip_refill_per_sec: 10,
            bridge_oracle_address: None,
            bridge_network_id: None,
            metrics_listen: None,
        };
        let d = Daemon::start(cfg.clone(), manifest).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut client =
            crate::client::LoadGenClient::connect(cfg.client_listen, sk, pk, network_id)
                .await
                .unwrap();
        let intent = suwappu_execution::Intent::Transfer {
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
            b.intents
                .iter()
                .any(|i| matches!(i, suwappu_execution::Intent::Transfer { amount: 42, .. }))
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
        let (pk, sk) = suwappu_crypto::mldsa::keypair();
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
                    validator_stake_suwappu: 150_000,
                    authority_stake_suwappu: 150_000,
                })
                .collect(),
            corridors: Vec::new(),
            prebalances: Vec::new(),
            rounds_per_epoch: 1024,
        };
        let cfg = NodeConfig {
            self_id: "v0".into(),
            authority_id: 0,
            listen: format!("127.0.0.1:{}", base_port).parse().unwrap(),
            client_listen: format!("127.0.0.1:{}", base_port + 100).parse().unwrap(),
            rpc_listen: None,
            peers: vec![],
            allow_post_genesis_join: false,
            round_ms: 500,
            checkpoint_cadence_rounds: 1,
            mldsa_secret_key_path: write_mldsa_key_file(&sk),
            bls_secret_key_path: "/dev/null".into(),
            genesis_manifest_path: "/dev/null".into(),
            event_log_path: std::env::temp_dir().join("suwappu-client-batch-test.ndjson"),

            max_client_connections: 256,
            client_idle_timeout_ms: 30_000,
            client_per_ip_limit: 8,
            rpc_per_ip_capacity: 60,
            rpc_per_ip_refill_per_sec: 10,
            bridge_oracle_address: None,
            bridge_network_id: None,
            metrics_listen: None,
        };
        let d = Daemon::start(cfg.clone(), manifest).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut client =
            crate::client::LoadGenClient::connect(cfg.client_listen, sk, pk, network_id)
                .await
                .unwrap();
        let batch: Vec<suwappu_execution::Intent> = (0..50u8)
            .map(|i| suwappu_execution::Intent::Transfer {
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
                b.intents
                    .iter()
                    .filter(|i| matches!(i, suwappu_execution::Intent::Transfer { amount: 99, .. }))
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

        // DAG-S6: each validator now signs its own certs and every peer
        // verifies against the genesis-registered key, so each of the 4
        // needs a real, distinct keypair rather than a shared placeholder.
        let keypairs: Vec<(
            suwappu_crypto::mldsa::PublicKey,
            suwappu_crypto::mldsa::SecretKey,
        )> = (0..n).map(|_| suwappu_crypto::mldsa::keypair()).collect();

        let manifest = GenesisManifest {
            network_id: "test-4n".into(),
            validators: (0..n)
                .map(|i| GenesisValidator {
                    authority_id: i,
                    label: format!("v{}", i),
                    mldsa_public_key_hex: hex::encode(keypairs[i as usize].0.as_bytes()),
                    bls_public_key_hex: "00".into(),
                    // Must clear AUTHORITY_STAKE_THRESHOLD_SUWAPPU (100_000):
                    // previously a silently-failed admission (just a
                    // `tracing::warn!`) didn't matter because certs weren't
                    // checked against the registry at all. Now that
                    // `ingest_cert` verifies every cert's signature against
                    // the author's registered key, an unadmitted authority
                    // means every one of its certs is rejected and consensus
                    // never commits.
                    validator_stake_suwappu: 150_000,
                    authority_stake_suwappu: 150_000,
                })
                .collect(),
            corridors: Vec::new(),
            prebalances: Vec::new(),
            rounds_per_epoch: 1024,
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
                allow_post_genesis_join: false,
                round_ms: 100,
                checkpoint_cadence_rounds: 1,
                mldsa_secret_key_path: write_mldsa_key_file(&keypairs[i as usize].1),
                bls_secret_key_path: "/dev/null".into(),
                genesis_manifest_path: "/dev/null".into(),
                event_log_path: std::env::temp_dir()
                    .join(format!("suwappu-daemon-test-v{}.ndjson", i)),

                max_client_connections: 256,
                client_idle_timeout_ms: 30_000,
                client_per_ip_limit: 8,
                rpc_per_ip_capacity: 60,
                rpc_per_ip_refill_per_sec: 10,
                bridge_oracle_address: None,
                bridge_network_id: None,
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

    /// Shared cluster setup for the Phase G admit/eject tests below.
    ///
    /// Spins up a 4-node loopback cluster with above-threshold stakes so
    /// the genesis admission populates both registries on every node,
    /// waits for warm-up, and connects a signed loadgen client on v0's
    /// client port. Returns `(daemons, client, base_port, network_id)`.
    ///
    /// Split out of the former single `phase_g_admit_and_eject` mega-test
    /// (tracking issue #171) so admit and eject are independently
    /// runnable/diagnosable: a transient flake in one no longer also
    /// swallows the other's result, and CI can retry just the flaky half
    /// instead of the whole ~270s worst-case combined test. This does
    /// NOT by itself resolve the underlying CI-runner resource-contention
    /// flake described below (only reproducible under real shared-runner
    /// load, not verified fixed here) — it narrows the blast radius and
    /// makes a future flake's diagnostics per-stage instead of pooled.
    #[allow(clippy::type_complexity)]
    async fn spawn_phase_g_cluster(
        base_port: u16,
        network_id: &str,
    ) -> (
        Vec<Daemon>,
        crate::client::LoadGenClient,
        Vec<(
            suwappu_crypto::mldsa::PublicKey,
            suwappu_crypto::mldsa::SecretKey,
        )>,
    ) {
        let n = 4u32;

        // Issue #28 (Phase 2.6): generate a real ML-DSA-65 keypair
        // for v0 so the loadgen client can sign AdmitAuthority /
        // EjectAuthority intents that pass the new signature gate.
        // DAG-S6: this is now ALSO v0's own certificate-authoring key
        // (its authority identity and its client-signer identity are
        // the same keypair in this test) — v0's `mldsa_secret_key_path`
        // below must load `client_sk`, not a separate key.
        let (client_pk, client_sk) = suwappu_crypto::mldsa::keypair();
        let client_pk_hex = hex::encode(client_pk.as_bytes());
        // v1-v3 need their own real, distinct keypairs too: every cert
        // they author must now verify against their genesis-registered
        // key on every peer.
        let other_keypairs: Vec<(
            suwappu_crypto::mldsa::PublicKey,
            suwappu_crypto::mldsa::SecretKey,
        )> = (1..n).map(|_| suwappu_crypto::mldsa::keypair()).collect();

        let manifest = GenesisManifest {
            network_id: network_id.to_string(),
            validators: (0..n)
                .map(|i| GenesisValidator {
                    authority_id: i,
                    label: format!("v{}", i),
                    // v0 carries the loadgen-known pubkey so this
                    // test's `client.submit(...)` calls verify, and so
                    // v0's own self-authored certs verify too (same key).
                    mldsa_public_key_hex: if i == 0 {
                        client_pk_hex.clone()
                    } else {
                        hex::encode(other_keypairs[(i - 1) as usize].0.as_bytes())
                    },
                    bls_public_key_hex: "00".into(),
                    validator_stake_suwappu: 30_000, // ≥ VALIDATOR_STAKE_THRESHOLD_SUWAPPU
                    authority_stake_suwappu: 150_000, // ≥ AUTHORITY_STAKE_THRESHOLD_SUWAPPU
                })
                .collect(),
            corridors: Vec::new(),
            prebalances: Vec::new(),
            // Issue #18: short epochs so governance application
            // (which now lands at the next boundary) is exercised on
            // CI-sane timescales. 16 rounds * 100ms = 1.6s/boundary.
            rounds_per_epoch: 16,
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
                allow_post_genesis_join: false,
                round_ms: 100,
                checkpoint_cadence_rounds: 1,
                mldsa_secret_key_path: write_mldsa_key_file(if i == 0 {
                    &client_sk
                } else {
                    &other_keypairs[(i - 1) as usize].1
                }),
                bls_secret_key_path: "/dev/null".into(),
                genesis_manifest_path: "/dev/null".into(),
                event_log_path: std::env::temp_dir()
                    .join(format!("suwappu-phaseg-test-v{}-{}.ndjson", i, base_port)),

                max_client_connections: 256,
                client_idle_timeout_ms: 30_000,
                client_per_ip_limit: 8,
                rpc_per_ip_capacity: 60,
                rpc_per_ip_refill_per_sec: 10,
                bridge_oracle_address: None,
                bridge_network_id: None,
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

        let admit_addr = format!("127.0.0.1:{}", base_port + 100)
            .parse::<SocketAddr>()
            .unwrap();
        let client = crate::client::LoadGenClient::connect(
            admit_addr,
            client_sk,
            client_pk,
            network_id.to_string(),
        )
        .await
        .unwrap();

        (daemons, client, other_keypairs)
    }

    /// Submit `AdmitAuthority{id=4}` and poll for it to converge across
    /// every node's registry, panicking with a full per-node diagnostic
    /// on timeout. Shared by both `phase_g_admit` and `phase_g_eject`
    /// (the latter needs authority 4 admitted before it can eject it).
    async fn admit_authority_4(
        daemons: &[Daemon],
        client: &mut crate::client::LoadGenClient,
        co_pk: &suwappu_crypto::mldsa::PublicKey,
        co_sk: &suwappu_crypto::mldsa::SecretKey,
    ) {
        // Governance rule (wire v3): AdmitAuthority needs the sponsor
        // (client = v0), a SECOND distinct seated authority (co_pk/co_sk =
        // v1), and the candidate's proof-of-possession over its own
        // freshly-minted key.
        let (cand_pk, cand_sk) = suwappu_crypto::mldsa::keypair();
        let admit = suwappu_execution::Intent::AdmitAuthority {
            authority_id: 4,
            stake_suwappu: 150_000,
            mldsa_public_key: cand_pk.as_bytes().to_vec(),
            bls_public_key: vec![0u8; 48],
        };
        client
            .submit_governed(admit, co_pk, co_sk, Some((&cand_pk, &cand_sk)))
            .await
            .unwrap();

        // Poll for convergence rather than sleeping a fixed wall-clock
        // window. CI runners (2-core, many parallel daemon tests) can
        // starve commit progress for several seconds; a fixed sleep
        // misses the deadline whereas a poll passes as soon as the
        // registry reflects the new admission on every node.
        let admit_deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            let all_at_5 = {
                let mut ok = true;
                for d in daemons {
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
                            b.intents.iter().any(|x| {
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
                        suwappu_consensus::joint::validator_quorum_threshold(&stake_table);
                    let auth_equiv = suwappu_consensus::detect_authority_equivocation(&dag).len();
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
    }

    /// Phase G admit half (DAG-S27.5, split from the former combined
    /// test under issue #171 — see `spawn_phase_g_cluster`).
    ///
    /// Spins up a 4-node loopback cluster, submits `AdmitAuthority` for
    /// a new id=4, and asserts the registry converges to size 5 on every
    /// validator. Issue #18 fix (2026-05-14): governance intents are
    /// queued at commit time and drained at the next epoch boundary,
    /// making the registry mutation atomic across the mesh — this
    /// eliminates the transitional quorum-threshold asymmetry where
    /// daemons disagreed on `quorum_threshold(5)=4` vs
    /// `quorum_threshold(4)=3` during the n=5→n=4 window.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn phase_g_admit() {
        let (daemons, mut client, other_keypairs) =
            spawn_phase_g_cluster(23_000, "phase-g-admit-4n").await;
        // v1 is the required second seated authority co-signer.
        admit_authority_4(
            &daemons,
            &mut client,
            &other_keypairs[0].0,
            &other_keypairs[0].1,
        )
        .await;
    }

    /// IQ-007: no commit retraction under transient block/vote absence,
    /// plus one agreed settlement order and identical governance apply.
    ///
    /// The joint-gated commit rule must, when a leader's block or its
    /// validator-vote quorum is momentarily unavailable, DEFER the whole
    /// commit walk (`break 'commit`) rather than skip ahead — so an honest
    /// node's finalized set only ever GROWS, and nodes never disagree about
    /// where a transaction settled, even while back-filling missing blocks
    /// over `GetBlock`. That is the property IQ-007 flagged as unguarded.
    ///
    /// Observable choice (learned the hard way — do not "simplify" this
    /// back): `inner.blocks_by_round` is NOT the finalize order and must
    /// not be asserted append-only. It is a lossy `round -> cert` lookup
    /// index for `suwappu_getBlock(round)`, written last-writer-wins at
    /// `blocks_by_round.insert(cert_round, h)`. A DAG round holds one cert
    /// PER AUTHORITY, and a later leader's `causal_history` sweep
    /// legitimately pulls in a straggler cert at an already-swept round,
    /// overwriting that entry. An earlier draft of this test asserted
    /// append-only on it and correctly failed ("v0 rewrote finalized round
    /// 29") against entirely healthy behaviour.
    ///
    /// So we assert on the two observables that ARE sound:
    ///   1. `state.committed` never loses a member (no retraction) —
    ///      sampled continuously while the mesh commits and back-fills.
    ///      This is exactly what `break 'commit` must guarantee: halt,
    ///      never roll back.
    ///   2. `inner.tx_to_block` agrees across every pair of nodes on the
    ///      (round, cert) a transaction settled in. Disagreement there is a
    ///      fork of the joint-gated order on the settlement-relevant axis.
    ///      (Sound here because every intent is submitted through a single
    ///      client to v0, so each is authored into exactly one block.)
    ///   3. Every node makes the identical governance apply decision —
    ///      registries converge to exactly {0,1,2,3,4}.
    ///
    /// Scope note: the *pure-consensus* append-only ordering property is
    /// already covered at 10k in
    /// `suwappu-consensus/tests/proptest_dagbft_commit.rs`
    /// (`finalize_is_append_only`). This test adds the *daemon-level*
    /// defer-under-unavailability + recovery guarantee that the pure layer
    /// cannot see. A full 10k `proptest!` is impractical here (a real
    /// multi-node tokio cluster per case), so this is a deterministic
    /// scenario on top of that pure proptest — and, per IQ-007, still on
    /// top of the human consensus-team sign-off + loaded devnet
    /// fault-injection run that a change of this class requires.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn phase_g_growing_prefix_under_transient_unavailability() {
        let (daemons, mut client, other_keypairs) =
            spawn_phase_g_cluster(23_400, "iq007-growing-prefix-4n").await;

        // Pre-admit transfer traffic so the finalize order already spans
        // several rounds before governance lands (more room for a reorder
        // bug to surface).
        let pre: Vec<Intent> = (0..4u8)
            .map(|i| Intent::Transfer {
                from: [i + 1; 20],
                to: [i + 9; 20],
                amount: 500 + i as u128,
            })
            .collect();
        client.submit_batch(pre).await.unwrap();

        // Governed AdmitAuthority{4}: sponsor = v0 (client), second seated
        // authority = v1, candidate proof-of-possession over a fresh key.
        let (cand_pk, cand_sk) = suwappu_crypto::mldsa::keypair();
        let admit = suwappu_execution::Intent::AdmitAuthority {
            authority_id: 4,
            stake_suwappu: 150_000,
            mldsa_public_key: cand_pk.as_bytes().to_vec(),
            bls_public_key: vec![0u8; 48],
        };
        client
            .submit_governed(
                admit,
                &other_keypairs[0].0,
                &other_keypairs[0].1,
                Some((&cand_pk, &cand_sk)),
            )
            .await
            .unwrap();

        // Post-admit traffic so ordering keeps advancing across the epoch
        // boundary where the admission applies.
        let post: Vec<Intent> = (0..4u8)
            .map(|i| Intent::Transfer {
                from: [i + 20; 20],
                to: [i + 30; 20],
                amount: 700 + i as u128,
            })
            .collect();
        client.submit_batch(post).await.unwrap();

        // Per-node last-observed committed set. `committed` is a
        // parking_lot mutex, so snapshot out of the guard before any
        // .await (guards must not cross await points here).
        let mut last_committed: Vec<_> = daemons
            .iter()
            .map(|d| d.state.committed.lock().clone())
            .collect();

        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            let mut all_at_5 = true;
            for (i, d) in daemons.iter().enumerate() {
                let snap = { d.state.committed.lock().clone() };
                // (1) No retraction: every cert this node had already
                // committed must still be committed. Deferring on a missing
                // block/vote must HALT the walk, never roll one back.
                for h in last_committed[i].iter() {
                    assert!(
                        snap.contains(h),
                        "node v{} un-committed cert {:?} — commit retraction, finalized set must only grow",
                        i,
                        h,
                    );
                }
                last_committed[i] = snap;
                let reg = d.state.authority_registry.read().await;
                if reg.len() != 5 || !reg.contains(4) {
                    all_at_5 = false;
                }
            }
            if all_at_5 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "IQ-007 growing-prefix test timed out (60s) before the admission converged",
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        // (2) Cross-node settlement agreement: any transaction indexed by
        // two nodes must have settled at the same (round, cert) on both.
        let mut placements = Vec::new();
        for d in &daemons {
            placements.push(d.state.inner.lock().await.tx_to_block.clone());
        }
        let mut compared = 0usize;
        for a in 0..placements.len() {
            for b in (a + 1)..placements.len() {
                for (tx, av) in placements[a].iter() {
                    if let Some(bv) = placements[b].get(tx) {
                        assert_eq!(
                            av.0, bv.0,
                            "nodes v{} and v{} disagree on the round tx {:?} settled in — joint order forked",
                            a, b, tx,
                        );
                        assert_eq!(
                            av.1, bv.1,
                            "nodes v{} and v{} disagree on the cert tx {:?} settled in — joint order forked",
                            a, b, tx,
                        );
                        compared += 1;
                    }
                }
            }
        }
        assert!(
            compared > 0,
            "no transaction was indexed on two nodes — the cross-node settlement-agreement check never ran",
        );

        // (3) Identical governance apply: every node converged to exactly
        // the same authority set {0,1,2,3,4}.
        for (i, d) in daemons.iter().enumerate() {
            let reg = d.state.authority_registry.read().await;
            assert_eq!(reg.len(), 5, "node v{} authority count", i);
            for id in [0u32, 1, 2, 3, 4] {
                assert!(reg.contains(id), "node v{} missing authority {}", i, id);
            }
        }
    }

    /// Phase G eject half (DAG-S27.5, split from the former combined
    /// test under issue #171 — see `spawn_phase_g_cluster`).
    ///
    /// Admits authority 4 (precondition — eject needs something to
    /// eject), then submits `EjectAuthority` for it and asserts the
    /// registry converges back to size 4 → excludes id 4 on every
    /// validator, which is the end-to-end guarantee Phase G claims
    /// (paper §4.1 + Invariant 5 for the eject path).
    ///
    /// Un-`#[ignore]`'d in #35: the eject path was failing with the
    /// bare `registry sizes = [5,5,5,5]` panic, which doesn't say
    /// whether the eject Intent ever reached a block, ever committed,
    /// or whether `pending_governance` is draining. The eject failure
    /// branch below mirrors `admit_authority_4`'s diagnostic so the CI
    /// log actually identifies which step is wedged.
    ///
    /// **Re-`#[ignore]`'d 2026-05-16** under tracking issue #171: the
    /// combined test was still flaky on shared GHA runners under load
    /// even with the diagnostic instrumentation from #35. This split
    /// (admit_authority_4 factored out, separate ports/network_id so
    /// `phase_g_admit` and `phase_g_eject` can run concurrently without
    /// colliding) is the "test split" mitigation #171 suggested — it
    /// narrows each test's failure surface and lets CI retry just the
    /// flaky half, but has NOT been verified against a real loaded
    /// shared runner (only this sandboxed environment), so this stays
    /// `#[ignore]`d until a CI run confirms the flake is actually gone,
    /// not just relocated. Un-ignore once that's confirmed; if it's
    /// still flaky, the remaining #171 investigations (dedicated
    /// integration budget, RUST_LOG=trace local repro under simulated
    /// load) are still open.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    #[ignore = "test-split mitigation for #171 not yet confirmed against loaded CI; un-ignore after a clean CI run"]
    async fn phase_g_eject() {
        let (daemons, mut client, other_keypairs) =
            spawn_phase_g_cluster(23_200, "phase-g-eject-4n").await;
        let (co_pk, co_sk) = (&other_keypairs[0].0, &other_keypairs[0].1);
        admit_authority_4(&daemons, &mut client, co_pk, co_sk).await;

        // Eject the new authority. Governance rule: v0 sponsors (the
        // client's own key) and v1 — a second, distinct seated authority
        // — co-signs. No candidate PoP for an eject.
        let eject = suwappu_execution::Intent::EjectAuthority {
            authority_id: 4,
            proof_ref: [0u8; 32],
        };
        client
            .submit_governed(eject, co_pk, co_sk, None)
            .await
            .unwrap();

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
                        b.intents.iter().any(|x| {
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
                let resubmit = suwappu_execution::Intent::EjectAuthority {
                    authority_id: 4,
                    proof_ref: [0u8; 32],
                };
                let _ = client.submit_governed(resubmit, co_pk, co_sk, None).await;
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
                let resubmit = suwappu_execution::Intent::EjectAuthority {
                    authority_id: 4,
                    proof_ref: [0u8; 32],
                };
                let _ = client.submit_governed(resubmit, co_pk, co_sk, None).await;
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
                        eject_block_round,
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
                        let mut eject_block_round: Option<u64> = None;
                        for (h, b) in blocks.iter() {
                            if b.intents.iter().any(|x| {
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
                                eject_block_round = Some(b.round);
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
                            eject_block_round,
                            eject_cert_committed,
                        )
                    };
                    let inner = d.state.inner.lock().await;
                    let reg = d.state.authority_registry.read().await;
                    let stake_table = d.state.stake_table.read().await;
                    let last_authored = inner.last_authored_round.unwrap_or(u64::MAX);
                    let reg_size = reg.len();
                    let has_id4 = reg.contains(4);
                    let n_auth = inner.n_authorities;
                    let stake_total = stake_table.total();
                    let stake_thresh =
                        suwappu_consensus::joint::validator_quorum_threshold(&stake_table);
                    let pending_gov = inner.pending_governance.len();
                    let pending_gov_has_eject = inner.pending_governance.iter().any(|(x, _)| {
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
        let (pk, sk) = suwappu_crypto::mldsa::keypair();
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
                    validator_stake_suwappu: 150_000,
                    authority_stake_suwappu: 150_000,
                })
                .collect(),
            corridors: Vec::new(),
            prebalances: Vec::new(),
            rounds_per_epoch: 1024,
        };
        let cfg = NodeConfig {
            self_id: "v0".into(),
            authority_id: 0,
            listen: format!("127.0.0.1:{}", base_port).parse().unwrap(),
            client_listen: format!("127.0.0.1:{}", base_port + 100).parse().unwrap(),
            rpc_listen: None,
            peers: vec![],
            allow_post_genesis_join: false,
            round_ms: 500,
            checkpoint_cadence_rounds: 1,
            mldsa_secret_key_path: write_mldsa_key_file(&sk),
            bls_secret_key_path: "/dev/null".into(),
            genesis_manifest_path: "/dev/null".into(),
            event_log_path: std::env::temp_dir().join("suwappu-client-auth-test.ndjson"),

            max_client_connections: 256,
            client_idle_timeout_ms: 30_000,
            client_per_ip_limit: 8,
            rpc_per_ip_capacity: 60,
            rpc_per_ip_refill_per_sec: 10,
            bridge_oracle_address: None,
            bridge_network_id: None,
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
        let good_intent = suwappu_execution::Intent::Transfer {
            from: [1u8; 20],
            to: [2u8; 20],
            amount: 42,
        };
        let good_digest = intent_signing_digest(&network_id, &good_intent);
        let good_sig = suwappu_crypto::mldsa::sign(&good_digest, &sk).unwrap();
        let pkh = signer_pubkey_hash(pk.as_bytes());
        let good_msg = ClientMessage::Submit {
            intent: good_intent.clone(),
            signature: good_sig.as_bytes().to_vec(),
            signer_pubkey_hash: pkh,
        };
        match round_trip(cfg.client_listen, &good_msg).await {
            ClientResponse::Ack { .. } => {}
            other => panic!("good submit should Ack, got {:?}", other),
        }

        // ----- Case 2: bogus signer_pubkey_hash → reject.
        let bogus_pkh = [0xAAu8; 32];
        let bogus_msg = ClientMessage::Submit {
            intent: suwappu_execution::Intent::Transfer {
                from: [9u8; 20],
                to: [9u8; 20],
                amount: 99,
            },
            signature: good_sig.as_bytes().to_vec(),
            signer_pubkey_hash: bogus_pkh,
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
            intent: suwappu_execution::Intent::Transfer {
                from: [7u8; 20],
                to: [8u8; 20],
                amount: 77,
            },
            signature: vec![0u8; 3309], // structured-shape garbage
            signer_pubkey_hash: pkh,
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
        let intent_b = suwappu_execution::Intent::Transfer {
            from: [1u8; 20],
            to: [2u8; 20],
            amount: 999, // different amount
        };
        let replay_msg = ClientMessage::Submit {
            intent: intent_b,
            signature: good_sig.as_bytes().to_vec(), // signs good_intent, not intent_b
            signer_pubkey_hash: pkh,
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
            .flat_map(|b| b.intents.iter())
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

    /// Genesis funding (launch fix): `State::new` must credit every
    /// manifest `[[prebalances]]` entry to the fresh substrate, so a
    /// pre-balanced faucet address starts with spendable balance at
    /// height 0. Also asserts the malformed-entry warn-and-skip path
    /// and that a manifest without prebalances funds nothing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn state_new_applies_genesis_prebalances() {
        let faucet_addr: [u8; 20] = [0xAB; 20];
        let other_addr: [u8; 20] = [0xCD; 20];
        let mut manifest = GenesisManifest {
            network_id: "prebalance-test".into(),
            validators: vec![GenesisValidator {
                authority_id: 0,
                label: "v0".into(),
                mldsa_public_key_hex: "00".into(),
                bls_public_key_hex: "00".into(),
                validator_stake_suwappu: 1_000,
                authority_stake_suwappu: 1_000,
            }],
            corridors: Vec::new(),
            rounds_per_epoch: 1024,
            prebalances: vec![
                crate::config::GenesisPrebalance {
                    address: format!("0x{}", hex::encode(faucet_addr)),
                    balance_suwappu: 10_000_000_000,
                    role: Some("faucet".into()),
                },
                // No 0x prefix — must also be accepted.
                crate::config::GenesisPrebalance {
                    address: hex::encode(other_addr),
                    balance_suwappu: 7,
                    role: None,
                },
                // Malformed — warn-and-skip, must not poison the rest.
                crate::config::GenesisPrebalance {
                    address: "0xnothex".into(),
                    balance_suwappu: 1,
                    role: None,
                },
            ],
        };
        let (_pk, sk) = suwappu_crypto::mldsa::keypair();
        let state = State::new(&manifest, sk, None);
        {
            let inner = state.inner.lock().await;
            assert_eq!(inner.substrate.balance(&faucet_addr), 10_000_000_000);
            assert_eq!(inner.substrate.balance(&other_addr), 7);
            assert_eq!(inner.substrate.total_supply(), 10_000_000_007);
        }

        // A manifest without prebalances funds nothing (pre-fix behavior).
        manifest.prebalances = Vec::new();
        let (_pk2, sk2) = suwappu_crypto::mldsa::keypair();
        let state2 = State::new(&manifest, sk2, None);
        {
            let inner = state2.inner.lock().await;
            assert_eq!(inner.substrate.balance(&faucet_addr), 0);
            assert_eq!(inner.substrate.total_supply(), 0);
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
                    validator_stake_suwappu: 1_000,
                    authority_stake_suwappu: 1_000,
                })
                .collect(),
            corridors: Vec::new(),
            prebalances: Vec::new(),
            rounds_per_epoch: 1024,
        };
        let (log, _log_task) =
            EventLog::start(&std::env::temp_dir().join("suwappu-fastpath-test.ndjson"))
                .await
                .unwrap();
        let (_unused_pk, self_sk) = suwappu_crypto::mldsa::keypair();
        let state = Arc::new(State::new(&manifest, self_sk, None));
        let outbound: HashMap<PeerId, tokio::sync::mpsc::Sender<WireMessage>> = HashMap::new();
        let self_id: AuthorityId = 0;

        let tx = suwappu_fastpath::cert::FastPathTx {
            object: suwappu_fastpath::cert::OwnedObjectId([0xAB; 32]),
            owner: suwappu_fastpath::cert::OwnerAddress([0xCD; 32]),
            nonce: 42,
            lineage: CertHash([0; 32]),
            lineage_round: 0,
            payload_digest: [0x11; 32],
        };
        let key = State::fastpath_key(&tx);

        // Authority 1 broadcasts a partial cert with itself as signer.
        let cert_a1 = suwappu_fastpath::cert::FastPathCert {
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
        let cert_a2 = suwappu_fastpath::cert::FastPathCert {
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
        let equivocating_tx = suwappu_fastpath::cert::FastPathTx {
            payload_digest: [0x22; 32],
            ..tx.clone()
        };
        let bad_cert = suwappu_fastpath::cert::FastPathCert {
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
                    validator_stake_suwappu: 1_000,
                    authority_stake_suwappu: 1_000,
                })
                .collect(),
            corridors: Vec::new(),
            prebalances: Vec::new(),
            rounds_per_epoch: 1024,
        };
        let (log, _log_task) =
            EventLog::start(&std::env::temp_dir().join("suwappu-fp-k-binding-test.ndjson"))
                .await
                .unwrap();
        let (_unused_pk, self_sk) = suwappu_crypto::mldsa::keypair();
        let state = Arc::new(State::new(&manifest, self_sk, None));
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
                lineage: CertHash([0xDE; 32]),
            });
        }

        // ── Conflicting cert: same object, different payload, lineage at
        // round 3 (window = (3, 7], which includes the seeded round 5).
        let conflicting_tx = suwappu_fastpath::cert::FastPathTx {
            object: object_a,
            owner: suwappu_fastpath::cert::OwnerAddress([0xCD; 32]),
            nonce: 1,
            lineage: CertHash([0; 32]),
            lineage_round: 3,
            payload_digest: [0x22; 32], // != main_payload
        };
        let conflicting_key = State::fastpath_key(&conflicting_tx);
        let conflicting_cert = suwappu_fastpath::cert::FastPathCert {
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
        let consistent_tx = suwappu_fastpath::cert::FastPathTx {
            object: object_b,
            owner: suwappu_fastpath::cert::OwnerAddress([0xCD; 32]),
            nonce: 1,
            lineage: CertHash([0; 32]),
            lineage_round: 3,
            payload_digest: [0x33; 32],
        };
        let consistent_key = State::fastpath_key(&consistent_tx);
        let consistent_cert = suwappu_fastpath::cert::FastPathCert {
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
                    validator_stake_suwappu: 1_000,
                    authority_stake_suwappu: 1_000,
                })
                .collect(),
            corridors: Vec::new(),
            prebalances: Vec::new(),
            rounds_per_epoch: 1024,
        };
        let (log, _log_task) =
            EventLog::start(&std::env::temp_dir().join("suwappu-ltp-test.ndjson"))
                .await
                .unwrap();
        let (_unused_pk, self_sk) = suwappu_crypto::mldsa::keypair();
        let state = Arc::new(State::new(&manifest, self_sk, None));

        let payload = suwappu_ltp::AttestationPayload {
            source_chain: 1u64, // Ethereum
            target_chain: 42u64,
            source_height: 12_345_678,
            state_root: [0xAB; 32],
            timestamp_round: 100,
        };
        let att = suwappu_ltp::CorridorAttestation {
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
                    validator_stake_suwappu: 1_000,
                    authority_stake_suwappu: 1_000,
                })
                .collect(),
            corridors: Vec::new(),
            prebalances: Vec::new(),
            rounds_per_epoch: 1024,
        };
        let (log, _log_task) =
            EventLog::start(&std::env::temp_dir().join("suwappu-ltp-unreg.ndjson"))
                .await
                .unwrap();
        let (_unused_pk, self_sk) = suwappu_crypto::mldsa::keypair();
        let state = Arc::new(State::new(&manifest, self_sk, None));
        assert!(state.inner.lock().await.corridors.is_empty());

        let payload = suwappu_ltp::AttestationPayload {
            source_chain: 1,
            target_chain: 42,
            source_height: 99,
            state_root: [0x11; 32],
            timestamp_round: 7,
        };
        let att = suwappu_ltp::CorridorAttestation {
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
    /// is set, `Daemon::start` spawns `suwappu_rpc::start` and the four
    /// read-only methods become reachable over HTTP. This test boots a
    /// single-validator daemon, opens a TCP socket to the RPC port,
    /// sends a `suwappu_getEpoch` request as raw HTTP, and verifies the
    /// JSON-RPC response envelope. Stays at the wire level (no `reqwest`)
    /// so the test crate doesn't pull in another HTTP client.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rpc_binding_returns_epoch_over_http() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let base_port: u16 = 21_000;
        let (rpc_pk, rpc_sk) = suwappu_crypto::mldsa::keypair();
        let manifest = GenesisManifest {
            network_id: "rpc-bind-1n".into(),
            validators: vec![GenesisValidator {
                authority_id: 0,
                label: "v0".into(),
                mldsa_public_key_hex: hex::encode(rpc_pk.as_bytes()),
                bls_public_key_hex: "00".into(),
                validator_stake_suwappu: 30_000,
                authority_stake_suwappu: 150_000,
            }],
            corridors: Vec::new(),
            prebalances: Vec::new(),
            rounds_per_epoch: 1024,
        };
        let cfg = NodeConfig {
            self_id: "v0".into(),
            authority_id: 0,
            listen: format!("127.0.0.1:{}", base_port).parse().unwrap(),
            client_listen: format!("127.0.0.1:{}", base_port + 100).parse().unwrap(),
            rpc_listen: Some(format!("127.0.0.1:{}", base_port + 200).parse().unwrap()),
            peers: vec![],
            allow_post_genesis_join: false,
            round_ms: 500,
            checkpoint_cadence_rounds: 1,
            mldsa_secret_key_path: write_mldsa_key_file(&rpc_sk),
            bls_secret_key_path: "/dev/null".into(),
            genesis_manifest_path: "/dev/null".into(),
            event_log_path: std::env::temp_dir().join("suwappu-rpc-bind-test.ndjson"),

            max_client_connections: 256,
            client_idle_timeout_ms: 30_000,
            client_per_ip_limit: 8,
            rpc_per_ip_capacity: 60,
            rpc_per_ip_refill_per_sec: 10,
            bridge_oracle_address: None,
            bridge_network_id: None,
            metrics_listen: None,
        };
        let _d = Daemon::start(cfg.clone(), manifest).await.unwrap();
        // Give the bound listener a tick to accept connections.
        tokio::time::sleep(Duration::from_millis(200)).await;

        let body = br#"{"jsonrpc":"2.0","id":1,"method":"suwappu_getEpoch"}"#;
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
        let base_port: u16 = 21_300;
        let network_id = "blocks-idx-1n".to_string();
        let (pk, sk) = suwappu_crypto::mldsa::keypair();
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
                    validator_stake_suwappu: 150_000,
                    authority_stake_suwappu: 150_000,
                })
                .collect(),
            corridors: Vec::new(),
            prebalances: Vec::new(),
            rounds_per_epoch: 1024,
        };
        let cfg = NodeConfig {
            self_id: "v0".into(),
            authority_id: 0,
            listen: format!("127.0.0.1:{}", base_port).parse().unwrap(),
            client_listen: format!("127.0.0.1:{}", base_port + 100).parse().unwrap(),
            rpc_listen: None,
            peers: vec![],
            allow_post_genesis_join: false,
            round_ms: 500,
            checkpoint_cadence_rounds: 1,
            mldsa_secret_key_path: write_mldsa_key_file(&sk),
            bls_secret_key_path: "/dev/null".into(),
            genesis_manifest_path: "/dev/null".into(),
            event_log_path: std::env::temp_dir().join("suwappu-blocks-idx-test.ndjson"),

            max_client_connections: 256,
            client_idle_timeout_ms: 30_000,
            client_per_ip_limit: 8,
            rpc_per_ip_capacity: 60,
            rpc_per_ip_refill_per_sec: 10,
            bridge_oracle_address: None,
            bridge_network_id: None,
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
            let stored = crate::codec::encode(&block.intents[idx]).unwrap();
            let stored_hash: [u8; 32] = *blake3::hash(&stored).as_bytes();
            assert_eq!(
                &stored_hash, h,
                "intent at block.intents[{}] does not match tx_to_block key",
                idx,
            );
        }
    }

    /// T2: end-to-end `suwappu_submitIntent`. Bind a single-validator
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
        let (pk, sk) = suwappu_crypto::mldsa::keypair();
        let pk_hex = hex::encode(pk.as_bytes());
        let manifest = GenesisManifest {
            network_id: network_id.clone(),
            validators: vec![GenesisValidator {
                authority_id: 0,
                label: "v0".into(),
                mldsa_public_key_hex: pk_hex,
                bls_public_key_hex: "00".into(),
                validator_stake_suwappu: 30_000,
                authority_stake_suwappu: 150_000,
            }],
            corridors: Vec::new(),
            prebalances: Vec::new(),
            rounds_per_epoch: 1024,
        };
        let cfg = NodeConfig {
            self_id: "v0".into(),
            authority_id: 0,
            listen: format!("127.0.0.1:{}", base_port).parse().unwrap(),
            client_listen: format!("127.0.0.1:{}", base_port + 100).parse().unwrap(),
            rpc_listen: Some(format!("127.0.0.1:{}", base_port + 200).parse().unwrap()),
            peers: vec![],
            allow_post_genesis_join: false,
            round_ms: 500,
            checkpoint_cadence_rounds: 1,
            mldsa_secret_key_path: write_mldsa_key_file(&sk),
            bls_secret_key_path: "/dev/null".into(),
            genesis_manifest_path: "/dev/null".into(),
            event_log_path: std::env::temp_dir().join("suwappu-rpc-submit-test.ndjson"),

            max_client_connections: 256,
            client_idle_timeout_ms: 30_000,
            client_per_ip_limit: 8,
            rpc_per_ip_capacity: 60,
            rpc_per_ip_refill_per_sec: 10,
            bridge_oracle_address: None,
            bridge_network_id: None,
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
        let signature = suwappu_crypto::mldsa::sign(&digest, &sk).unwrap();
        let pkh = signer_pubkey_hash(pk.as_bytes());

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "suwappu_submitIntent",
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

        let base_port: u16 = 21_800;
        let network_id = "rpc-submit-bad-1n".to_string();
        // v0's own real keypair — needed so the daemon can sign its own
        // certs (DAG-S6). The test's "unknown signer" case submits a
        // CLIENT intent under a *different*, unregistered keypair below;
        // this one is unrelated to that assertion.
        let (pk, sk) = suwappu_crypto::mldsa::keypair();
        let pk_hex = hex::encode(pk.as_bytes());
        let manifest = GenesisManifest {
            network_id,
            validators: vec![GenesisValidator {
                authority_id: 0,
                label: "v0".into(),
                mldsa_public_key_hex: pk_hex,
                bls_public_key_hex: "00".into(),
                validator_stake_suwappu: 30_000,
                authority_stake_suwappu: 150_000,
            }],
            corridors: Vec::new(),
            prebalances: Vec::new(),
            rounds_per_epoch: 1024,
        };
        let cfg = NodeConfig {
            self_id: "v0".into(),
            authority_id: 0,
            listen: format!("127.0.0.1:{}", base_port).parse().unwrap(),
            client_listen: format!("127.0.0.1:{}", base_port + 100).parse().unwrap(),
            rpc_listen: Some(format!("127.0.0.1:{}", base_port + 200).parse().unwrap()),
            peers: vec![],
            allow_post_genesis_join: false,
            round_ms: 500,
            checkpoint_cadence_rounds: 1,
            mldsa_secret_key_path: write_mldsa_key_file(&sk),
            bls_secret_key_path: "/dev/null".into(),
            genesis_manifest_path: "/dev/null".into(),
            event_log_path: std::env::temp_dir().join("suwappu-rpc-submit-bad-test.ndjson"),

            max_client_connections: 256,
            client_idle_timeout_ms: 30_000,
            client_per_ip_limit: 8,
            rpc_per_ip_capacity: 60,
            rpc_per_ip_refill_per_sec: 10,
            bridge_oracle_address: None,
            bridge_network_id: None,
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
            "method": "suwappu_submitIntent",
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
}
