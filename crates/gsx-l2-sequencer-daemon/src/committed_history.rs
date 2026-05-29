//! Durable committed-batch-tx-hash history (#256).
//!
//! ## Why this exists
//!
//! `gsx_l2_sequencer::force_include::evaluate` decides, per L1
//! tick, whether each `Pending` force-include obligation should
//! be **honored** (its tx was committed on time) or **slashed**
//! (the deadline passed un-honored). Pass 1 emits `MarkHonored`
//! for every Pending obligation whose `tx_hash` is in the
//! `committed_batch_tx_hashes` set the daemon supplies.
//!
//! That set was historically the daemon's **in-memory** record
//! of "tx hashes committed since this process last booted." A
//! restart or outage wiped it. After the wipe, an obligation
//! whose tx was *already committed on time* no longer appears in
//! the set, so pass 1 skips it — and once `current_l1_height`
//! crosses its deadline, pass 2 emits `SlashMissedForceInclude`
//! against an obligation the sequencer **already honored**. That
//! is a false slashing decision driven purely by lost RAM.
//!
//! This module gives the daemon a **crash-safe, on-disk**
//! record of committed tx hashes (each tagged with the L1 height
//! observed when it was committed). It is loaded + replayed at
//! startup so the honor evidence survives a restart, and
//! rewritten after each commit. Old entries are pruned past the
//! maximum obligation lifetime so the file does not grow without
//! bound.
//!
//! ## What this is NOT (scope)
//!
//! This is the storage layer only. Wiring it into a live
//! force-include watcher — polling L1 obligations, calling
//! `evaluate`, submitting the resulting `MarkHonored` /
//! `SlashMissedForceInclude` Intents — is Phase 2.2-c work and
//! depends on the SP1 prover + real L1 client, neither of which
//! the daemon has yet. The daemon today only *builds* batches
//! (see `batch_builder_task::run_one_tick`); it does not yet
//! submit `CommitL2StateRoot`. So `record_commit` is not yet
//! called on a real commit path — it is exercised by tests and
//! ready for the watcher to drive.
//!
//! ## Crash-safety
//!
//! `persist` writes to a temp file in the *same directory* as
//! the target (so the final `rename` stays within one
//! filesystem and is atomic) and renames it over the target. A
//! crash mid-write leaves either the old complete file or the
//! new complete file, never a torn one.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use gsx_l2_sequencer::force_include::TxHash;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

/// Retention horizon, in L1 blocks, for committed-hash entries.
///
/// An entry recorded at L1 height `h` is safe to prune only once
/// no still-open obligation could reference it. The pure
/// evaluator's worst-case obligation lifetime is
/// `deadline_l1_height + EJECTION_WINDOW_L1_BLOCKS` (10,000
/// blocks per `force_include.rs`). We retain comfortably beyond
/// that so pruning can never reintroduce the exact false-slash
/// bug this module fixes: an entry is dropped only when it is
/// older than `current_l1_height - RETENTION_L1_BLOCKS`.
///
/// 50,000 blocks ≈ 5x the ejection window — generous headroom
/// for clock skew between the recorded height and an obligation
/// deadline registered later. The file holds 32 bytes + a small
/// height per entry, so over-retaining is cheap.
pub const RETENTION_L1_BLOCKS: u64 = 50_000;

/// On-disk schema version. Bumped if the serialized shape
/// changes so a future daemon can detect + migrate (or reject)
/// an incompatible file rather than silently misreading it.
const SCHEMA_VERSION: u32 = 1;

/// Default filename for the history checkpoint within the data
/// dir.
pub const HISTORY_FILE_NAME: &str = "committed_history.json";

/// Errors from loading or persisting the history store.
#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    /// Filesystem I/O failed (read, write, rename, mkdir).
    #[error("committed-history I/O at {path}: {source}")]
    Io {
        /// Path involved.
        path: String,
        /// Underlying OS error.
        #[source]
        source: io::Error,
    },

    /// The on-disk file is not valid JSON or does not match the
    /// expected schema.
    #[error("parsing committed-history file {path}: {source}")]
    Parse {
        /// Path attempted.
        path: String,
        /// Underlying serde error.
        #[source]
        source: serde_json::Error,
    },

    /// The file declares a schema version this binary does not
    /// understand. Surfaced rather than silently misread.
    #[error("committed-history file {path} has unsupported schema version {found} (expected {expected})")]
    UnsupportedSchema {
        /// Path attempted.
        path: String,
        /// Version found in the file.
        found: u32,
        /// Version this binary writes/reads.
        expected: u32,
    },
}

/// Serializable on-disk envelope: schema version + the entries.
///
/// `entries` maps each committed `TxHash` to the L1 height at
/// which it was observed committed. A map (rather than a list)
/// makes re-recording the same hash idempotent and keeps the
/// file deterministic.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct HistoryFile {
    version: u32,
    /// Hex-encoded `TxHash` → L1 height. Hex keys keep the JSON
    /// human-inspectable and avoid array-key serialization
    /// quirks.
    entries: BTreeMap<String, u64>,
}

/// In-memory, durably-backed record of committed batch tx
/// hashes keyed by the L1 height at which each was committed.
///
/// Construct via [`CommittedHistory::load`] (replays any
/// existing checkpoint) and mutate via [`CommittedHistory::record_commit`].
/// Call [`CommittedHistory::persist`] after each commit tick to
/// make the new state durable.
#[derive(Debug, Clone, Default)]
pub struct CommittedHistory {
    /// `TxHash` → L1 height observed at commit time.
    entries: BTreeMap<TxHash, u64>,
    /// Where [`Self::persist`] writes. `None` for an in-memory-only
    /// store (tests that don't exercise the disk path).
    path: Option<PathBuf>,
}

impl CommittedHistory {
    /// An empty, in-memory-only store with no backing file.
    pub fn in_memory() -> Self {
        Self {
            entries: BTreeMap::new(),
            path: None,
        }
    }

    /// Load (replay) the checkpoint at `path`, or start empty if
    /// the file does not exist yet. The returned store remembers
    /// `path` and will [`Self::persist`] back to it.
    ///
    /// A missing file is the normal first-boot case and is NOT an
    /// error. A present-but-corrupt or wrong-version file IS an
    /// error: silently discarding it would drop honor evidence
    /// and reintroduce the false-slash bug, so the operator must
    /// see it.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, HistoryError> {
        let path = path.as_ref().to_path_buf();
        let entries = match fs::read(&path) {
            Ok(bytes) => {
                let file: HistoryFile =
                    serde_json::from_slice(&bytes).map_err(|source| HistoryError::Parse {
                        path: path.display().to_string(),
                        source,
                    })?;
                if file.version != SCHEMA_VERSION {
                    return Err(HistoryError::UnsupportedSchema {
                        path: path.display().to_string(),
                        found: file.version,
                        expected: SCHEMA_VERSION,
                    });
                }
                let mut decoded = BTreeMap::new();
                for (hex_key, height) in file.entries {
                    match decode_hash(&hex_key) {
                        Some(hash) => {
                            decoded.insert(hash, height);
                        }
                        None => {
                            warn!(
                                key = %hex_key,
                                "committed-history: skipping malformed tx-hash key"
                            );
                        }
                    }
                }
                debug!(
                    count = decoded.len(),
                    path = %path.display(),
                    "committed-history: replayed checkpoint"
                );
                decoded
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                debug!(path = %path.display(), "committed-history: no checkpoint, starting empty");
                BTreeMap::new()
            }
            Err(source) => {
                return Err(HistoryError::Io {
                    path: path.display().to_string(),
                    source,
                });
            }
        };
        Ok(Self {
            entries,
            path: Some(path),
        })
    }

    /// Record that `tx_hash` was committed at L1 height
    /// `l1_height`. Idempotent: re-recording the same hash keeps
    /// the *earliest* observed height (the on-time evidence), so
    /// a later re-record can never make an entry look younger and
    /// get it pruned prematurely.
    pub fn record_commit(&mut self, tx_hash: TxHash, l1_height: u64) {
        self.entries
            .entry(tx_hash)
            .and_modify(|h| *h = (*h).min(l1_height))
            .or_insert(l1_height);
    }

    /// Drop entries older than the retention horizon relative to
    /// `current_l1_height`. An entry recorded at height `h` is
    /// removed only when `h < current_l1_height - RETENTION_L1_BLOCKS`,
    /// i.e. once no still-open obligation could reference it.
    /// Saturating subtraction means early in chain life (low
    /// heights) nothing is pruned.
    pub fn prune(&mut self, current_l1_height: u64) {
        let floor = current_l1_height.saturating_sub(RETENTION_L1_BLOCKS);
        let before = self.entries.len();
        self.entries.retain(|_, &mut h| h >= floor);
        let removed = before - self.entries.len();
        if removed > 0 {
            debug!(
                removed,
                floor, "committed-history: pruned aged entries below retention floor"
            );
        }
    }

    /// The set of committed tx hashes, ready to hand to
    /// `EvaluateInput::committed_batch_tx_hashes`.
    pub fn tx_hashes(&self) -> std::collections::BTreeSet<TxHash> {
        self.entries.keys().copied().collect()
    }

    /// Whether `tx_hash` is recorded as committed.
    pub fn contains(&self, tx_hash: &TxHash) -> bool {
        self.entries.contains_key(tx_hash)
    }

    /// Number of recorded entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the store holds no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Atomically write the current state to the backing file.
    ///
    /// Writes a temp file in the **same directory** as the target
    /// (keeping the rename within one filesystem so it is atomic)
    /// then renames it over the target. A crash mid-write leaves
    /// the old complete file intact; the next boot replays it.
    ///
    /// A no-op for an in-memory-only store ([`Self::in_memory`]).
    pub fn persist(&self) -> Result<(), HistoryError> {
        let Some(path) = &self.path else {
            return Ok(());
        };

        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|source| HistoryError::Io {
                    path: parent.display().to_string(),
                    source,
                })?;
            }
        }

        let file = HistoryFile {
            version: SCHEMA_VERSION,
            entries: self
                .entries
                .iter()
                .map(|(hash, height)| (encode_hash(hash), *height))
                .collect(),
        };
        let bytes = serde_json::to_vec_pretty(&file).map_err(|source| HistoryError::Parse {
            path: path.display().to_string(),
            source,
        })?;

        // Temp file in the same directory as the target.
        let tmp_path = temp_path_for(path);
        fs::write(&tmp_path, &bytes).map_err(|source| HistoryError::Io {
            path: tmp_path.display().to_string(),
            source,
        })?;
        fs::rename(&tmp_path, path).map_err(|source| HistoryError::Io {
            path: path.display().to_string(),
            source,
        })?;
        debug!(
            count = self.entries.len(),
            path = %path.display(),
            "committed-history: persisted checkpoint"
        );
        Ok(())
    }
}

/// Build the temp-file path next to `target` (same directory so
/// the rename is atomic). Suffixes the filename with `.tmp`.
fn temp_path_for(target: &Path) -> PathBuf {
    let mut name = target
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".tmp");
    match target.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(name),
        _ => PathBuf::from(name),
    }
}

/// Lowercase-hex encode a 32-byte hash for the JSON key.
fn encode_hash(hash: &TxHash) -> String {
    let mut s = String::with_capacity(64);
    for b in hash {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Decode a 64-char lowercase-hex string back to a `TxHash`.
/// Returns `None` on wrong length or non-hex input.
fn decode_hash(s: &str) -> Option<TxHash> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        let hi = (s.as_bytes()[i * 2] as char).to_digit(16)?;
        let lo = (s.as_bytes()[i * 2 + 1] as char).to_digit(16)?;
        *byte = (hi * 16 + lo) as u8;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use gsx_l2_sequencer::force_include::{
        evaluate, DaemonAction, EvaluateInput, ObligationSnapshot, ObligationStatus,
    };

    use super::*;

    fn hash(b: u8) -> TxHash {
        [b; 32]
    }

    /// A scratch path under the OS temp dir, unique per test, so
    /// parallel test runs don't collide.
    fn scratch_path(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "gsx-committed-history-test-{}-{}-{}",
            tag,
            std::process::id(),
            // nanos for uniqueness within the same process.
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        p.push(HISTORY_FILE_NAME);
        p
    }

    #[test]
    fn hex_round_trip() {
        for b in [0u8, 1, 15, 16, 255, 128] {
            let h = hash(b);
            assert_eq!(decode_hash(&encode_hash(&h)), Some(h));
        }
        assert_eq!(decode_hash("zz"), None);
        assert_eq!(decode_hash(""), None);
        assert_eq!(decode_hash(&"a".repeat(63)), None);
    }

    #[test]
    fn record_is_idempotent_and_keeps_earliest_height() {
        let mut h = CommittedHistory::in_memory();
        h.record_commit(hash(1), 100);
        h.record_commit(hash(1), 200);
        h.record_commit(hash(1), 50);
        assert_eq!(h.len(), 1);
        // earliest height wins so prune can't drop it early.
        h.prune(50 + RETENTION_L1_BLOCKS);
        assert!(h.contains(&hash(1)));
    }

    #[test]
    fn prune_drops_only_aged_entries() {
        let mut h = CommittedHistory::in_memory();
        h.record_commit(hash(1), 10);
        h.record_commit(hash(2), 60_000);
        // current = 70_000, floor = 70_000 - 50_000 = 20_000.
        h.prune(70_000);
        assert!(!h.contains(&hash(1)), "old entry pruned");
        assert!(h.contains(&hash(2)), "recent entry retained");
    }

    #[test]
    fn prune_is_noop_below_retention_horizon() {
        let mut h = CommittedHistory::in_memory();
        h.record_commit(hash(1), 5);
        h.prune(100); // saturating floor = 0
        assert!(h.contains(&hash(1)));
    }

    #[test]
    fn missing_file_loads_empty() {
        let p = scratch_path("missing");
        let h = CommittedHistory::load(&p).unwrap();
        assert!(h.is_empty());
    }

    #[test]
    fn persist_then_reload_round_trips() {
        let p = scratch_path("roundtrip");
        let mut h = CommittedHistory::load(&p).unwrap();
        h.record_commit(hash(7), 1234);
        h.record_commit(hash(9), 5678);
        h.persist().unwrap();

        let reloaded = CommittedHistory::load(&p).unwrap();
        assert_eq!(reloaded.len(), 2);
        assert!(reloaded.contains(&hash(7)));
        assert!(reloaded.contains(&hash(9)));

        let _ = fs::remove_file(&p);
    }

    #[test]
    fn unsupported_schema_is_an_error() {
        let p = scratch_path("badschema");
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, br#"{"version":999,"entries":{}}"#).unwrap();
        let err = CommittedHistory::load(&p).unwrap_err();
        assert!(matches!(
            err,
            HistoryError::UnsupportedSchema { found: 999, .. }
        ));
        let _ = fs::remove_file(&p);
    }

    /// The core #256 regression, verified *through the evaluator*.
    ///
    /// Scenario: the sequencer commits a force-include tx on time,
    /// then the daemon restarts (state dropped + reloaded from
    /// disk). With the durable history, a later tick — now past
    /// the obligation's deadline — must still emit `MarkHonored`
    /// and must NOT emit `SlashMissedForceInclude`.
    #[test]
    fn restart_preserves_honor_and_prevents_false_slash() {
        let p = scratch_path("restart");

        // The honored tx + its obligation.
        let tx = hash(42);
        let obligation_id = [7u8; 32];
        let deadline = 1_000u64;

        // --- Before restart: commit the tx on time, persist. ---
        {
            let mut history = CommittedHistory::load(&p).unwrap();
            // committed at height 900, comfortably before deadline 1000.
            history.record_commit(tx, 900);
            history.persist().unwrap();
        }

        // --- Restart: original in-memory state is gone; reload. ---
        let history = CommittedHistory::load(&p).unwrap();

        // A later tick, now PAST the deadline (height 1500). Without
        // durable history this is exactly when the false slash fires.
        let mut obligations = BTreeMap::new();
        obligations.insert(
            obligation_id,
            ObligationSnapshot {
                tx_hash: tx,
                deadline_l1_height: deadline,
                status: ObligationStatus::Pending,
            },
        );
        let committed: BTreeSet<TxHash> = history.tx_hashes();
        let ejected: BTreeSet<[u8; 32]> = BTreeSet::new();

        let actions = evaluate(EvaluateInput {
            obligations: &obligations,
            committed_batch_tx_hashes: &committed,
            current_l1_height: 1_500,
            ejected_obligations: &ejected,
            ejector: [0u8; 20],
        });

        assert!(
            actions.contains(&DaemonAction::MarkHonored { obligation_id }),
            "reloaded history must still honor the on-time commit"
        );
        assert!(
            !actions
                .iter()
                .any(|a| matches!(a, DaemonAction::SlashMissedForceInclude { .. })),
            "durable history must prevent the post-restart false slash"
        );

        let _ = fs::remove_file(&p);
    }

    /// Control: without the durable history (empty set), the same
    /// past-deadline tick *does* false-slash — proving the test
    /// above is checking the real defect, not a tautology.
    #[test]
    fn without_history_false_slash_occurs() {
        let tx = hash(42);
        let obligation_id = [7u8; 32];

        let mut obligations = BTreeMap::new();
        obligations.insert(
            obligation_id,
            ObligationSnapshot {
                tx_hash: tx,
                deadline_l1_height: 1_000,
                status: ObligationStatus::Pending,
            },
        );
        let committed: BTreeSet<TxHash> = BTreeSet::new(); // lost on restart
        let ejected: BTreeSet<[u8; 32]> = BTreeSet::new();

        let actions = evaluate(EvaluateInput {
            obligations: &obligations,
            committed_batch_tx_hashes: &committed,
            current_l1_height: 1_500,
            ejected_obligations: &ejected,
            ejector: [0u8; 20],
        });

        assert!(
            actions.contains(&DaemonAction::SlashMissedForceInclude { obligation_id }),
            "control: lost history => false slash (the bug #256 fixes)"
        );
    }
}
