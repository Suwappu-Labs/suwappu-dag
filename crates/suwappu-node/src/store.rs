//! IQ-008 D4: durable commit log + state snapshots.
//!
//! Following Mysticeti §VI ("integrate a Write-Ahead Log ... intentionally
//! avoided key-value stores like RocksDB"), persistence is two plain
//! files under `NodeConfig::data_dir` and nothing else:
//!
//! - **`commit.log`** — append-only. One [`LogRecord`] per event, each
//!   `bincode`-encoded with the workspace's legacy config, length-
//!   prefixed, and chained by `prev_hash = blake3(previous record)`. Two
//!   record kinds:
//!   - `Authored { round, cert_hash }` — written and synced *before* the
//!     node broadcasts its own certificate, so a crash-restart can never
//!     re-sign a round it already signed (equivocation is 100% slashing
//!     under Invariant 5). This is the one place the WAL is on the
//!     critical path.
//!   - `Committed { sequence, leader_round, cert, block }` — one per
//!     committed certificate in finalize order, written before the block
//!     is applied to the substrate. Sui's rule ("blocks and commits must
//!     be flushed together") at the granularity we have: a record *is*
//!     the cert plus its block.
//!
//!   On open the chain is verified record by record; the first
//!   undecodable or unchained record ends the log and everything after it
//!   (a torn tail from a crash mid-write) is discarded by truncation.
//!
//! - **`snapshot-<round>.bin`** — a full [`StateSnapshot`] written
//!   atomically (temp file + rename) at every checkpoint boundary
//!   (`checkpoint_cadence_rounds` in the genesis manifest) and on clean
//!   shutdown. Verified on load by
//!   recomputing the substrate root against the stored `state_root`; on
//!   mismatch the loader falls back to the previous snapshot, then to
//!   genesis, mirroring Sui's formal-snapshot "revert on failed
//!   verification". The two newest snapshots are kept.
//!
//! Recovery (`Daemon::start`): load the newest valid snapshot, replay
//! every `Committed` record with `sequence > snapshot.log_sequence`
//! through the same commit path the live daemon uses, restore
//! `last_authored_round` from the snapshot and any later `Authored`
//! records, prune to the recovered gc round, then join the wire and
//! backfill from the recovered leader frontier. Replay equivalence — the
//! recovered `state_root` equals the live one — is the S34.3 exit gate
//! (`tests/proptest_persistence.rs`).

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use suwappu_authority::AuthorityRegistry;
use suwappu_consensus::{CertHash, Certificate, StakeTable};
use suwappu_execution::{Checkpoint, CoSignedCheckpoint, InMemorySubstrate, Intent, Substrate};
use suwappu_validator::ValidatorRegistry;

use crate::{client::GovAuth, wire::BlockPayload};

/// On-disk format version of both files. Bump on any layout change.
pub const STORE_VERSION: u32 = 1;

const LOG_FILE: &str = "commit.log";
const SNAPSHOT_PREFIX: &str = "snapshot-";
const SNAPSHOT_SUFFIX: &str = ".bin";
const SNAPSHOTS_TO_KEEP: usize = 2;
/// Sanity cap on a single log record (a block with the maximum intent
/// count is well under this). A larger length prefix is treated as a
/// torn/corrupt tail.
const MAX_RECORD_BYTES: u32 = 64 * 1024 * 1024;

/// One durable event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LogEvent {
    /// This node authored (signed) a certificate at `round`. Written
    /// before broadcast.
    Authored {
        /// Authored round.
        round: u64,
        /// Hash of the signed certificate.
        cert_hash: CertHash,
    },
    /// A certificate committed in finalize order.
    Committed {
        /// Position in the finalize order, from 0. Snapshots record the
        /// sequence they cover so replay knows where to resume.
        sequence: u64,
        /// Round of the leader whose sweep committed this certificate.
        /// Restores `last_committed_leader_round`, hence the gc round.
        leader_round: u64,
        /// The committed certificate.
        cert: Certificate,
        /// Its authentic block (digest-matched to the cert at commit).
        block: BlockPayload,
    },
    /// A checkpoint reached an Authority Ring quorum of signatures
    /// (IQ-008 D5). Replayed to restore the served chain.
    Checkpointed(Box<CheckpointBundle>),
}

/// The committee state a checkpoint commits to via `registry_root`
/// (IQ-008 D5). Everything a joiner needs to verify the *next*
/// checkpoint's signatures and to seat itself in the right epoch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegistrySet {
    /// Authority Ring.
    pub authority_registry: AuthorityRegistry,
    /// Validator Ring.
    pub validator_registry: ValidatorRegistry,
    /// Validator stake table.
    pub stake_table: StakeTable,
    /// `(current, rounds_per_epoch, last_boundary_round)`.
    pub epoch: (u64, u64, u64),
    /// Committee size used by the commit rule.
    pub n_authorities: u32,
}

impl RegistrySet {
    /// Canonical commitment: `blake3("SUWAPPU-REGISTRY-ROOT-V1" ||
    /// bincode(self))`. The registries are `BTreeMap`-backed, so the
    /// encoding is deterministic.
    pub fn root(&self) -> [u8; 32] {
        let bytes = crate::codec::encode(self).expect("RegistrySet is serialisable");
        let mut h = blake3::Hasher::new();
        h.update(b"SUWAPPU-REGISTRY-ROOT-V1");
        h.update(&bytes);
        *h.finalize().as_bytes()
    }
}

/// A co-signed checkpoint together with the committee it establishes —
/// one link of the chain served to joiners (IQ-008 D5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckpointBundle {
    /// Quorum-signed checkpoint.
    pub cosigned: CoSignedCheckpoint,
    /// Registry set whose `root()` equals `cosigned.checkpoint.registry_root`.
    pub registries: RegistrySet,
}

/// A log record: an event plus the hash chain link.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogRecord {
    /// Store format version at write time.
    pub version: u32,
    /// `blake3` of the previous record's encoding; `[0; 32]` first.
    pub prev_hash: [u8; 32],
    /// The event.
    pub event: LogEvent,
}

impl LogRecord {
    /// Chain hash of this record's canonical encoding.
    pub fn hash(&self) -> [u8; 32] {
        let bytes = crate::codec::encode(self).expect("LogRecord is serialisable");
        let mut h = blake3::Hasher::new();
        h.update(b"SUWAPPU-COMMIT-LOG-V1");
        h.update(&bytes);
        *h.finalize().as_bytes()
    }
}

/// Append-only, hash-chained commit log.
pub struct CommitLog {
    file: File,
    path: PathBuf,
    fsync: bool,
    last_hash: [u8; 32],
    next_sequence: u64,
    last_authored_round: Option<u64>,
    records: u64,
}

impl std::fmt::Debug for CommitLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommitLog")
            .field("path", &self.path)
            .field("records", &self.records)
            .field("next_sequence", &self.next_sequence)
            .finish()
    }
}

impl CommitLog {
    /// Open (or create) `dir/commit.log`, verify the chain, truncate any
    /// torn tail, and return the log positioned for appends together
    /// with every valid record already on disk.
    pub fn open(dir: &Path, fsync: bool) -> io::Result<(Self, Vec<LogRecord>)> {
        fs::create_dir_all(dir)?;
        let path = dir.join(LOG_FILE);
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(&path)?;

        let mut bytes = Vec::new();
        file.seek(SeekFrom::Start(0))?;
        file.read_to_end(&mut bytes)?;

        let mut records = Vec::new();
        let mut last_hash = [0u8; 32];
        let mut next_sequence = 0u64;
        let mut last_authored_round = None;
        let mut good_len = 0usize;
        let mut cursor = 0usize;
        while cursor + 4 <= bytes.len() {
            let len = u32::from_be_bytes([
                bytes[cursor],
                bytes[cursor + 1],
                bytes[cursor + 2],
                bytes[cursor + 3],
            ]);
            if len == 0 || len > MAX_RECORD_BYTES {
                break;
            }
            let start = cursor + 4;
            let end = match start.checked_add(len as usize) {
                Some(e) if e <= bytes.len() => e,
                _ => break,
            };
            let record: LogRecord = match crate::codec::decode(&bytes[start..end]) {
                Ok(r) => r,
                Err(_) => break,
            };
            if record.version != STORE_VERSION || record.prev_hash != last_hash {
                break;
            }
            match &record.event {
                LogEvent::Committed { sequence, .. } => {
                    if *sequence != next_sequence {
                        break;
                    }
                    next_sequence += 1;
                }
                LogEvent::Authored { round, .. } => {
                    last_authored_round =
                        Some(last_authored_round.map_or(*round, |r: u64| r.max(*round)));
                }
                LogEvent::Checkpointed(_) => {}
            }
            last_hash = record.hash();
            records.push(record);
            good_len = end;
            cursor = end;
        }
        if good_len < bytes.len() {
            tracing::warn!(
                path = %path.display(),
                discarded = bytes.len() - good_len,
                "commit log: discarding torn or unchained tail"
            );
            file.set_len(good_len as u64)?;
            file.sync_all()?;
        }
        file.seek(SeekFrom::End(0))?;
        let records_len = records.len() as u64;
        Ok((
            Self {
                file,
                path,
                fsync,
                last_hash,
                next_sequence,
                last_authored_round,
                records: records_len,
            },
            records,
        ))
    }

    /// Path of the underlying file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Sequence number the next `Committed` record will carry.
    pub fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    /// Highest round in any `Authored` record.
    pub fn last_authored_round(&self) -> Option<u64> {
        self.last_authored_round
    }

    /// Total records on disk.
    pub fn len(&self) -> u64 {
        self.records
    }

    /// `true` iff no records are on disk.
    pub fn is_empty(&self) -> bool {
        self.records == 0
    }

    fn append(&mut self, event: LogEvent) -> io::Result<LogRecord> {
        let record = LogRecord {
            version: STORE_VERSION,
            prev_hash: self.last_hash,
            event,
        };
        let body = crate::codec::encode(&record)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        let len = body.len() as u32;
        if len > MAX_RECORD_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "commit log record exceeds MAX_RECORD_BYTES",
            ));
        }
        let mut frame = Vec::with_capacity(4 + body.len());
        frame.extend_from_slice(&len.to_be_bytes());
        frame.extend_from_slice(&body);
        self.file.write_all(&frame)?;
        if self.fsync {
            self.file.sync_data()?;
        }
        self.last_hash = record.hash();
        self.records += 1;
        Ok(record)
    }

    /// Record that this node signed `cert_hash` at `round`. Durable
    /// (synced when `fsync` is on) before returning.
    pub fn append_authored(&mut self, round: u64, cert_hash: CertHash) -> io::Result<()> {
        self.append(LogEvent::Authored { round, cert_hash })?;
        self.last_authored_round = Some(self.last_authored_round.map_or(round, |r| r.max(round)));
        Ok(())
    }

    /// Record a quorum-co-signed checkpoint (IQ-008 D5).
    pub fn append_checkpointed(&mut self, bundle: CheckpointBundle) -> io::Result<()> {
        self.append(LogEvent::Checkpointed(Box::new(bundle)))?;
        Ok(())
    }

    /// Record a committed certificate. Returns its sequence number.
    pub fn append_committed(
        &mut self,
        leader_round: u64,
        cert: Certificate,
        block: BlockPayload,
    ) -> io::Result<u64> {
        let sequence = self.next_sequence;
        self.append(LogEvent::Committed {
            sequence,
            leader_round,
            cert,
            block,
        })?;
        self.next_sequence += 1;
        Ok(sequence)
    }
}

/// Everything needed to resume without the peers' help, captured at one
/// consistent point of the commit sequence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateSnapshot {
    /// Store format version.
    pub version: u32,
    /// Manifest network id; a snapshot from another network is refused.
    pub network_id: String,
    /// `last_committed_leader_round` at capture (0 if none yet).
    pub leader_round: u64,
    /// Whether any leader had committed at capture.
    pub has_committed: bool,
    /// gc round at capture.
    pub gc_round: Option<u64>,
    /// `Committed` records with `sequence < log_sequence` are covered by
    /// this snapshot; replay resumes at `log_sequence`.
    pub log_sequence: u64,
    /// Highest round this node had authored at capture.
    pub last_authored_round: Option<u64>,
    /// Execution substrate.
    pub substrate: InMemorySubstrate,
    /// `substrate.state_root()` at capture — the self-check on load.
    pub state_root: [u8; 32],
    /// Authority Ring.
    pub authority_registry: AuthorityRegistry,
    /// Validator Ring.
    pub validator_registry: ValidatorRegistry,
    /// Validator stake table.
    pub stake_table: StakeTable,
    /// Epoch counter `(current, rounds_per_epoch, last_boundary_round)`.
    pub epoch: (u64, u64, u64),
    /// Governance intents queued for the next boundary, with envelopes.
    pub pending_governance: Vec<(Intent, Option<GovAuth>)>,
    /// Stake parked for admitted-but-not-yet-active authorities.
    pub pending_stake: BTreeMap<u32, u128>,
    /// Committee size used by the commit rule.
    pub n_authorities: u32,
    /// Live DAG window (rounds above `gc_round`), topologically ordered.
    pub dag_certs: Vec<Certificate>,
    /// Tombstone window.
    pub tombstones: Vec<(CertHash, u64)>,
    /// Commit marks for the live window.
    pub committed: Vec<CertHash>,
    /// Block payloads for the live window.
    pub blocks: Vec<BlockPayload>,
    /// The checkpoint this snapshot was captured at (IQ-008 D5): its
    /// `state_root` equals `state_root` here and its `registry_root`
    /// equals the root of the registries above. `None` for a shutdown
    /// snapshot taken between checkpoint rounds.
    #[serde(default)]
    pub checkpoint: Option<Checkpoint>,
    /// Committee-transition checkpoints plus the latest co-signed one,
    /// oldest first — the chain served to joiners.
    #[serde(default)]
    pub checkpoint_chain: Vec<CheckpointBundle>,
    /// Checkpoint sequencing state: next boundary round, next height,
    /// hash of the last emitted checkpoint.
    #[serde(default)]
    pub checkpoint_cursor: (u64, u64, [u8; 32]),
}

/// The commit-derived body of a snapshot that a checkpoint's
/// `snapshot_root` commits to (IQ-008 D5). Only fields that are a pure
/// function of the commit sequence up to `leader_round` belong here:
/// every node that co-signs the checkpoint must compute the same root.
/// Certificates, blocks and tombstones are receipt-timing dependent and
/// are validated structurally on install instead (signatures against the
/// bound Authority Ring, block digests against the certificates they
/// back, tombstone rounds against the bound gc round).
#[derive(Serialize)]
struct SnapshotBody<'a> {
    leader_round: u64,
    has_committed: bool,
    gc_round: Option<u64>,
    /// Sorted.
    committed: Vec<CertHash>,
    pending_governance: &'a [(Intent, Option<GovAuth>)],
}

impl StateSnapshot {
    /// Canonical commitment to the commit-derived body:
    /// `blake3("SUWAPPU-SNAPSHOT-ROOT-V1" || bincode(body))` with the commit
    /// marks sorted, so the value is independent of the capturing node's
    /// hash-set iteration order.
    pub fn commit_root(&self) -> [u8; 32] {
        let mut committed = self.committed.clone();
        committed.sort();
        committed.dedup();
        let body = SnapshotBody {
            leader_round: self.leader_round,
            has_committed: self.has_committed,
            gc_round: self.gc_round,
            committed,
            pending_governance: &self.pending_governance,
        };
        let bytes = crate::codec::encode(&body).expect("snapshot body is serialisable");
        let mut h = blake3::Hasher::new();
        h.update(b"SUWAPPU-SNAPSHOT-ROOT-V1");
        h.update(&bytes);
        *h.finalize().as_bytes()
    }

    /// Stake parked for admitted-but-not-yet-active authorities, derived
    /// from the bound registries rather than taken from the wire: an
    /// authority in the Validator Ring with no stake-table row is exactly
    /// one whose first certificate has not been seen yet (DAG-S27.7
    /// deferred activation).
    pub fn derived_pending_stake(&self) -> BTreeMap<u32, u128> {
        self.validator_registry
            .members()
            .filter(|m| self.stake_table.weight(m.id) == 0)
            .map(|m| (m.id, m.stake_suwappu))
            .collect()
    }

    /// The registry set this snapshot carries, for `RegistrySet::root`.
    pub fn registries(&self) -> RegistrySet {
        RegistrySet {
            authority_registry: self.authority_registry.clone(),
            validator_registry: self.validator_registry.clone(),
            stake_table: self.stake_table.clone(),
            epoch: self.epoch,
            n_authorities: self.n_authorities,
        }
    }

    /// Recompute the substrate root and compare with the recorded one.
    pub fn verify(&self) -> Result<(), SnapshotError> {
        if self.version != STORE_VERSION {
            return Err(SnapshotError::Version(self.version));
        }
        let root = self.substrate.state_root();
        if root != self.state_root {
            return Err(SnapshotError::RootMismatch {
                recorded: self.state_root,
                recomputed: root,
            });
        }
        Ok(())
    }
}

/// Why a snapshot file was rejected.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    /// Unknown store version.
    #[error("snapshot store version {0} is not supported")]
    Version(u32),
    /// The substrate does not hash to the recorded root.
    #[error("snapshot state root mismatch: recorded {recorded:?}, recomputed {recomputed:?}")]
    RootMismatch {
        /// Root stored in the file.
        recorded: [u8; 32],
        /// Root recomputed from the decoded substrate.
        recomputed: [u8; 32],
    },
    /// Snapshot was taken on a different network.
    #[error("snapshot is for network {found:?}, this node is {expected:?}")]
    Network {
        /// Network id in the file.
        found: String,
        /// This node's network id.
        expected: String,
    },
    /// File could not be decoded.
    #[error("snapshot decode: {0}")]
    Decode(String),
    /// I/O.
    #[error("snapshot io: {0}")]
    Io(#[from] io::Error),
}

fn snapshot_path(dir: &Path, leader_round: u64) -> PathBuf {
    dir.join(format!(
        "{SNAPSHOT_PREFIX}{leader_round:020}{SNAPSHOT_SUFFIX}"
    ))
}

fn snapshot_round_from_name(name: &str) -> Option<u64> {
    name.strip_prefix(SNAPSHOT_PREFIX)?
        .strip_suffix(SNAPSHOT_SUFFIX)?
        .parse()
        .ok()
}

/// Write `snap` atomically to `dir` and drop all but the newest
/// `SNAPSHOTS_TO_KEEP` snapshot files. Returns the final path.
pub fn write_snapshot(dir: &Path, snap: &StateSnapshot) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let bytes = crate::codec::encode(snap)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    let final_path = snapshot_path(dir, snap.leader_round);
    let tmp_path = final_path.with_extension("tmp");
    {
        let mut f = File::create(&tmp_path)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp_path, &final_path)?;
    // Directory entry durability for the rename.
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }

    // Retention.
    let mut rounds: Vec<u64> = list_snapshot_rounds(dir)?;
    rounds.sort_unstable();
    while rounds.len() > SNAPSHOTS_TO_KEEP {
        let old = rounds.remove(0);
        let _ = fs::remove_file(snapshot_path(dir, old));
    }
    Ok(final_path)
}

fn list_snapshot_rounds(dir: &Path) -> io::Result<Vec<u64>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if let Some(r) = entry
            .file_name()
            .to_str()
            .and_then(snapshot_round_from_name)
        {
            out.push(r);
        }
    }
    Ok(out)
}

/// Load the newest snapshot in `dir` that decodes, verifies, and belongs
/// to `network_id`. Returns `Ok(None)` when there is none. A snapshot
/// that fails verification is logged and skipped, never used.
pub fn load_latest_snapshot(
    dir: &Path,
    network_id: &str,
) -> Result<Option<StateSnapshot>, SnapshotError> {
    if !dir.exists() {
        return Ok(None);
    }
    let mut rounds = list_snapshot_rounds(dir)?;
    rounds.sort_unstable_by(|a, b| b.cmp(a));
    for r in rounds {
        let path = snapshot_path(dir, r);
        let attempt = (|| -> Result<StateSnapshot, SnapshotError> {
            let bytes = fs::read(&path)?;
            let snap: StateSnapshot =
                crate::codec::decode(&bytes).map_err(|e| SnapshotError::Decode(e.to_string()))?;
            snap.verify()?;
            if snap.network_id != network_id {
                return Err(SnapshotError::Network {
                    found: snap.network_id.clone(),
                    expected: network_id.to_string(),
                });
            }
            Ok(snap)
        })();
        match attempt {
            Ok(snap) => return Ok(Some(snap)),
            Err(e) => {
                tracing::warn!(path = %path.display(), err = %e, "snapshot rejected; trying older");
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use suwappu_consensus::Certificate;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "suwappu-store-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn cert(round: u64, tag: u8) -> Certificate {
        Certificate {
            author: 0,
            round,
            parents: Vec::new(),
            payload_digest: [tag; 32],
            signature: Vec::new(),
        }
    }

    fn block(c: &Certificate) -> BlockPayload {
        BlockPayload {
            payload_digest: c.payload_digest,
            author: c.author,
            round: c.round,
            cert_hash: c.hash(),
            intents: vec![Intent::Transfer {
                from: [1; 20],
                to: [2; 20],
                amount: c.round as u128,
            }],
            governance_auth: Vec::new(),
        }
    }

    #[test]
    fn log_round_trips_and_chains() {
        let dir = tmp_dir("chain");
        {
            let (mut log, existing) = CommitLog::open(&dir, false).unwrap();
            assert!(existing.is_empty());
            log.append_authored(0, cert(0, 1).hash()).unwrap();
            let c1 = cert(1, 2);
            assert_eq!(log.append_committed(1, c1.clone(), block(&c1)).unwrap(), 0);
            let c2 = cert(2, 3);
            assert_eq!(log.append_committed(2, c2.clone(), block(&c2)).unwrap(), 1);
            log.append_authored(3, cert(3, 4).hash()).unwrap();
        }
        let (log, records) = CommitLog::open(&dir, false).unwrap();
        assert_eq!(records.len(), 4);
        assert_eq!(log.next_sequence(), 2);
        assert_eq!(log.last_authored_round(), Some(3));
        // Chain links.
        assert_eq!(records[0].prev_hash, [0u8; 32]);
        for w in records.windows(2) {
            assert_eq!(w[1].prev_hash, w[0].hash());
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn torn_tail_is_truncated() {
        let dir = tmp_dir("torn");
        let good_len;
        {
            let (mut log, _) = CommitLog::open(&dir, false).unwrap();
            let c1 = cert(1, 2);
            log.append_committed(1, c1.clone(), block(&c1)).unwrap();
            good_len = fs::metadata(log.path()).unwrap().len();
            let c2 = cert(2, 3);
            log.append_committed(2, c2.clone(), block(&c2)).unwrap();
        }
        // Chop the second record in half.
        let path = dir.join(LOG_FILE);
        let full = fs::metadata(&path).unwrap().len();
        let f = OpenOptions::new().write(true).open(&path).unwrap();
        f.set_len(good_len + (full - good_len) / 2).unwrap();
        drop(f);

        let (log, records) = CommitLog::open(&dir, false).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(log.next_sequence(), 1);
        assert_eq!(fs::metadata(&path).unwrap().len(), good_len);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tampered_record_ends_the_chain() {
        let dir = tmp_dir("tamper");
        let first_len;
        {
            let (mut log, _) = CommitLog::open(&dir, false).unwrap();
            let c1 = cert(1, 2);
            log.append_committed(1, c1.clone(), block(&c1)).unwrap();
            first_len = fs::metadata(log.path()).unwrap().len() as usize;
            let c2 = cert(2, 3);
            log.append_committed(2, c2.clone(), block(&c2)).unwrap();
            let c3 = cert(3, 4);
            log.append_committed(3, c3.clone(), block(&c3)).unwrap();
        }
        let path = dir.join(LOG_FILE);
        let mut bytes = fs::read(&path).unwrap();
        // Flip a byte inside the FIRST record's body (past its length
        // prefix): its hash changes, so record 2's prev_hash no longer
        // matches and the chain must end after record 1... unless the
        // flipped byte makes record 1 itself undecodable, in which case
        // the chain is empty. Either way nothing after the tamper is
        // trusted.
        bytes[first_len - 1] ^= 0xFF;
        fs::write(&path, &bytes).unwrap();
        let (_, records) = CommitLog::open(&dir, false).unwrap();
        assert!(
            records.len() <= 1,
            "records after a tampered one were trusted"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    fn sample_snapshot(leader_round: u64, network: &str) -> StateSnapshot {
        let substrate = InMemorySubstrate::from_balances([([7u8; 20], 1000u128), ([8u8; 20], 5)]);
        StateSnapshot {
            version: STORE_VERSION,
            network_id: network.into(),
            leader_round,
            has_committed: true,
            gc_round: leader_round.checked_sub(8),
            log_sequence: 42,
            last_authored_round: Some(leader_round + 1),
            state_root: substrate.state_root(),
            substrate,
            authority_registry: AuthorityRegistry::new(),
            validator_registry: ValidatorRegistry::new(),
            stake_table: StakeTable::new(),
            epoch: (0, 1024, 0),
            pending_governance: Vec::new(),
            pending_stake: BTreeMap::new(),
            n_authorities: 4,
            dag_certs: vec![cert(leader_round, 9)],
            tombstones: vec![(cert(1, 1).hash(), 1)],
            committed: vec![cert(leader_round, 9).hash()],
            blocks: Vec::new(),
            checkpoint: None,
            checkpoint_chain: Vec::new(),
            checkpoint_cursor: (0, 0, [0u8; 32]),
        }
    }

    #[test]
    fn snapshot_round_trips_keeps_two_and_loads_newest() {
        let dir = tmp_dir("snap");
        for r in [10u64, 20, 30] {
            write_snapshot(&dir, &sample_snapshot(r, "net")).unwrap();
        }
        let mut rounds = list_snapshot_rounds(&dir).unwrap();
        rounds.sort_unstable();
        assert_eq!(rounds, vec![20, 30], "only the newest two are kept");
        let snap = load_latest_snapshot(&dir, "net").unwrap().unwrap();
        assert_eq!(snap.leader_round, 30);
        assert_eq!(snap, sample_snapshot(30, "net"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_newest_snapshot_falls_back_to_older() {
        let dir = tmp_dir("fallback");
        write_snapshot(&dir, &sample_snapshot(10, "net")).unwrap();
        // Newest snapshot claims a root its substrate does not hash to.
        let mut bad = sample_snapshot(20, "net");
        bad.state_root[0] ^= 1;
        write_snapshot(&dir, &bad).unwrap();
        let snap = load_latest_snapshot(&dir, "net").unwrap().unwrap();
        assert_eq!(snap.leader_round, 10);
        // Wrong network is refused outright.
        assert!(load_latest_snapshot(&dir, "other").unwrap().is_none());
        // Missing dir is "no snapshot", not an error.
        assert!(load_latest_snapshot(&dir.join("nope"), "net")
            .unwrap()
            .is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
