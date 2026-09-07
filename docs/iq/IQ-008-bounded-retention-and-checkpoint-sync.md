# IQ-008 — Bounded DAG retention, durable commit log, checkpoint-anchored sync

**Status:** Recommendation, implemented on branch `claude/kasper-research-n9i4ct`
as sprint DAG-S34. Consensus-touching (commit sweep, DAG store, ingest
floor): requires `consensus-reviewer` and **human consensus-team sign-off**
before it is treated as production-ready. CI green is necessary, not
sufficient, for this class (same posture as IQ-007).
**Owner:** consensus
**Date:** 2026-09-07
**Tracking:** `/goal` items A6 (no persistence, DAG never prunes) and A7
(snapshot/checkpoint sync). Interacts with IQ-004 / #45 (see Residuals).

## Question

How does a validator keep bounded memory, survive a restart without
losing history, and let a late joiner catch up when its peers no longer
hold the rounds it missed — without changing any commit decision that
the current DagBft-C rule (IQ-001, IQ-002, IQ-007) would make?

## Background: what grows today, and why it matters for launch

Every structure below is append-only for the life of the process. A seed
that runs for weeks accumulates all of it in RAM, and a restart loses all
of it. `/goal` A6 calls this "the single biggest 'it will fall over'
risk"; the operator guide currently tells outsiders to expect periodic
regenesis.

| Surface | Where | Growth | Notes |
|---|---|---|---|
| Certificate DAG | `crates/suwappu-consensus/src/dag.rs:33-40` | every cert ever ingested | Module doc: "append-only; certificates cannot be modified or removed". `max_round` carries an explicit warning (`dag.rs:115-121`) that pruning would break the commit-path substitution proven in `commit.rs::mod equivalence`. |
| Block payloads | `State::blocks` (`daemon.rs`) | every block ever received | Served back to peers by `GetBlock` / `GetCertsByRound`. |
| Votes | `State::votes` | removed only on commit (`daemon.rs:1962`) | Votes for never-committed certs stay forever. |
| Committed set | `State::committed` | every committed cert hash | Consulted on every sweep. |
| Equivocation index | `StateInner::seen_at` | one entry per (author, round) | Grows with rounds. |
| Fast-path binding index | `StateInner::main_lane_index` | one entry per committed transfer | IQ-003 only needs the K=4 window. |
| RPC indices | `blocks_by_round`, `tx_to_block` | per block / per intent | Explorer surface; can move to the durable log. |
| Commit sweep cost | `try_commit` (`daemon.rs:1772-1784`) | O(rounds in DAG) per pass | `candidate_rounds` is *every* round in the store; `decide_slot` then scans anchors up to `max_round` (`commit.rs:230`). With an unbounded DAG this is quadratic in chain age and is a plausible contributor to the ~0.125 TPS recorded in `.sprint-state.md` (A10). |
| Persistence | none | — | `main.rs:39-52` builds state from genesis on every start. The DAG-S11 `Checkpointer` (`crates/suwappu-execution/src/checkpoint.rs:169`) is never constructed by the daemon; `checkpoint_cadence_rounds` appears only in test configs. |
| Catch-up reach | `run_backfill` (`daemon.rs:1081`) | limited to peers' RAM | A joiner "can only sync back to what its peers have held since their own boot" (`LAUNCH-STATUS.md` gap 1). |

## What the literature says

The design below is not novel. It is the standard answer from the
round-based DAG BFT line that DagBft-C descends from, plus the checkpoint
machinery Sui built around Mysticeti. Sources are quoted from the papers
and from public code, with the text extracted from the PDFs (the
scratch copies are not committed).

**Narwhal & Tusk (Danezis, Kokoris-Kogias, Sonnino, Spiegelman; EuroSys
'22), §"Garbage Collection".** The round structure is what makes GC
possible: a validator "decide[s] on the validity of a block only from
information about the current round", so "validators in Narwhal are not
required to examine the entire history to verify new blocks". The
agreement problem is stated exactly: "if two validators garbage collect
different rounds then when a new block b is committed, validators might
disagree on b's causal history and thus totally order different
histories. To this end, Narwhal leverages the properties of a consensus
protocol ... to agree on the garbage collection round. Blocks from
earlier rounds can be safely be stored off the main validator and all
later messages from previous rounds can be safely ignored." The commit
rule "orders b's causal history up to the garbage collection point."
Censorship is addressed by re-injection: "an honest node that garbage
collect an old round that didn't make it into the DAG re-inject
transactions to a later round." The footnote is the operational warning
for us: "A bug in our garbage collection led to exhausting 120GB of RAM
in minutes compared to 700MB memory footprint of Narwhal."

**Bullshark (Spiegelman, Giridharan, Sonnino, Kokoris-Kogias; CCS '22),
Appendix "Garbage collection in Bullshark".** States the impossibility
that bounds our ambitions: "providing the BAB's validity (fairness)
property with bounded memory in fully asynchronous executions is
impossible since blocks of honest parties can be arbitrarily delayed".
Their resolution is fairness only after GST, with the GC round derived
from the *ordered leader*: Algorithm 6 sets `GCround` while walking the
leader's causal history, and "once parties agree which leaders to order
they also agree what rounds to garbage collect. Therefore, the garbage
collection mechanism preserves the safety and liveness properties". We
adopt the leader-derived floor but use round depth instead of their
timestamp median, because our rounds are already clocked (`round_ms`).

**Mysticeti (Babel, Chursin, Danezis, Kokoris-Kogias, Sonnino; 2023),
§VI Implementation.** On persistence: "To ensure data persistence and
crash recovery, integrate a Write-Ahead Log (WAL) ... We have
intentionally avoided key-value stores like RocksDB [35] to eliminate
associated overhead and periodic compaction penalties." §VII notes the
Sui integration added "crash recovery mechanisms, and bulk
synchronization". §I motivates the joiner problem: a "crash-recovered
validator ... typically needs to verify thousands of signatures when
trying to catch up".

**Sui `consensus/core` (MystenLabs, public), `dag_state.rs` and
`commit_syncer.rs`.** The production reference for the paper above.
`gc_round` is "the highest round that blocks of equal or lower round are
considered obsolete and no longer possible to be committed", computed as
`commit_round.saturating_sub(gc_depth)` from the last committed leader.
Persistence buffers blocks and commits and flushes them together: "when
buffering a block, all of its ancestors and the latest commit ... must
also be buffered" and "all of the buffered blocks and commits must be
flushed together to ensure consistency". Catch-up trusts certification
rather than re-verifying: blocks in commits certified by ≥2f+1 stake
"have already passed verification on honest validators", and "commits
have a simple dependency graph (linear), so it is easy to fetch ranges
of commits in parallel". Sync is "not on the critical path" and tuned
for throughput over reaction time.

**Sui checkpoint verification (docs.sui.io, "Checkpoint verification").**
Checkpoints carry the previous checkpoint's hash and are certified by
"more than 2/3" of the committee. A syncing client bootstraps the
committee from genesis and walks forward: verify each checkpoint with the
current committee, "at epoch boundaries, extract the next committee from
the final checkpoint and continue verification". This resolves the
circularity that "the client needs to know the committee to verify
checkpoints, but also learns the committee through checkpoint
validation." Formal snapshots contain "only the end of epoch live object
set" and are "checked against" a "commitment to the live object state at
end of epoch" at restore time; on failure "the node reverts to its
pre-restoration state".

**Kaspa (kaspanet docs, "Finality and Pruning").** The PoW-DAG analogue:
a pruning point below which "any data that belongs to a block ... can be
deleted"; a pruned node keeps the UTXO set at the pruning point plus
headers, and a syncing node "trusts a blue work declaration of the
selected tip" with a proof "planned but not yet implemented". Useful as
a contrast: without a finality quorum, Kaspa's sync trust is weaker than
what a BFT checkpoint gives us for free.

## Options considered

**A. Status quo plus scheduled regenesis.** Zero code. Rejected: it is
exactly what `/goal` A6 says is "not credible for a testnet others
join", and every regenesis throws away the history an incentivized
program must keep.

**B. Persist everything, never prune (RocksDB / sled behind `DagStore`).**
Bounds RAM but not disk, keeps the O(rounds) commit sweep, adds a heavy
dependency to a long-running validator, and does nothing for the
joiner: a peer can serve every round since genesis, so a joiner must
replay the whole chain and verify every signature (Mysticeti's
"thousands of signatures" problem). Rejected as the primary mechanism;
compatible as a later backend for the commit log.

**C. Consensus-anchored GC + append-only commit log + co-signed
checkpoint snapshots (chosen).** Bounded RAM by construction, O(GC_DEPTH)
commit sweep, restart from local disk, and joiners bootstrap from a
quorum-certified state at a verified checkpoint. This is the Narwhal /
Bullshark / Sui design transposed onto the structures we already have:
the DAG-S11 `Checkpoint` and `ratify_checkpoint`, the wire sync
primitives from A1, and the substrate's `state_root`.

## Decision

### D1. GC round and commit floor are functions of the commit sequence

- `GC_DEPTH` (rounds) is a protocol constant, `suwappu_consensus::gc::GC_DEPTH`.
  Initial value 256 rounds ≈ 64 s at `round_ms = 250`. Larger than Sui's
  default depth because our indirect rule (IQ-002) scans anchors up to
  `max_round` and IQ-004 permits late `Skip → Direct` flips; the depth
  must dwarf any observed late-arrival lag (see Residuals).
- `gc_round = last_committed_leader_round.saturating_sub(GC_DEPTH)`.
  Certificates with `round ≤ gc_round` are obsolete: never inserted,
  never served, never committed (Sui `dag_state.rs` semantics).
- The committed sub-DAG of leader `L` is
  `causal_history(L) ∩ { c : c.round > commit_floor(L) }` with
  `commit_floor(L) = L.round.saturating_sub(GC_DEPTH)`. Because the
  floor is a pure function of the leader, every honest node computes the
  same set from the same leader regardless of how far its own pruning has
  progressed. This is the Bullshark Algorithm 6 property restated in
  rounds.
- Consistency argument. A node prunes round `x` only when it has
  committed a leader at round ≥ `x + GC_DEPTH`. Any later leader `L`
  has `L.round > x + GC_DEPTH`, so `x ≤ commit_floor(L)` and the pruned
  certificate is excluded from `L`'s sweep on every node, pruned or not.
  Hence pruning can never change a committed set. This is stated as
  invariant candidate I-GC1 below and is the S34.1 exit-gate property.

### D2. Ingest tolerates pruned parents through a bounded tombstone window

`DagStore::insert` requires every parent to be present (`dag.rs:76-89`).
After pruning, a certificate at `gc_round + 1` names parents at
`gc_round` that no longer exist, and would be misclassified as an orphan,
triggering `GetCert` for a hash no peer can serve. Sui carries the
ancestor's round inside the block reference; our `parents` are bare
hashes, and changing the signed certificate layout is out of scope.

Instead `DagStore` keeps tombstones — `(hash → round)` for every pruned
certificate with `round ∈ (gc_round − GC_DEPTH, gc_round]`. A parent
that is tombstoned satisfies validation (its round is known, so the
monotonicity check still runs); a parent that is neither present nor
tombstoned is a genuine `UnknownParent` exactly as today. Memory is
bounded by `n × GC_DEPTH` hashes. Round-driver parents are always the
previous round (`daemon.rs:597`), so in practice only the first round
above the floor ever consults the window.

### D3. Every growing structure is pruned at the same floor

After each commit that advances `gc_round`, the daemon prunes, under
the canonical lock order: `dag` (D2), `blocks`, `votes`, `committed`
(hashes for rounds ≤ gc_round can be forgotten because such certs are
rejected at ingest and excluded from every sweep), `seen_at`,
`orphans` + `inflight_fetches` + `inflight_fetch_history` (an orphan
whose missing parent is at or below the floor can never be inserted),
`main_lane_index` (IQ-003 needs only rounds > gc_round), and
`blocks_by_round` / `tx_to_block` (the durable commit log answers
`suwappu_getBlock` / `suwappu_getTransaction` for older rounds). The
commit sweep iterates `candidate_rounds` from `gc_round + 1`, making
`try_commit` O(GC_DEPTH) per pass.

### D4. Durability: append-only commit log plus periodic state snapshot

Following Mysticeti §VI, no key-value store. A new
`crates/suwappu-node/src/store.rs`:

- `CommitLog`: one record per committed certificate in finalize order,
  `bincode`-encoded and length-prefixed, carrying
  `(sequence, round, cert, block payload, governance envelopes,
  prev_record_hash)`. The `prev_record_hash` chain makes truncation and
  tampering of the local file detectable; a torn tail is discarded on
  open. Records are appended before the substrate mutation is
  considered durable (write-ahead), matching Sui's "flush blocks and
  commits together" rule at the granularity we have (one record = one
  cert + its block).
- `StateSnapshot`: written at every checkpoint boundary (D5) and on
  clean shutdown: `{ checkpoint, substrate, authority_registry,
  validator_registry, stake_table, epoch, pending_governance,
  pending_stake, n_authorities, gc_round, last_committed_leader_round,
  log_sequence }`. Verified on load by recomputing `substrate.state_root()`
  against `checkpoint.state_root`; a mismatch falls back to the previous
  snapshot, then to genesis (Sui formal-snapshot "revert" semantics).
- Startup: load the newest valid snapshot, replay log records with
  `sequence > snapshot.log_sequence` through the same `execute_block`
  path, then join the wire and backfill from
  `last_committed_leader_round + 1`. Replay equivalence — the Invariant-4
  substrate property — is the S34.3 exit gate: for any committed
  sequence, `snapshot + replay(log)` and the live daemon produce the
  same `state_root` and the same registries.
- `NodeConfig::data_dir` is optional. Unset keeps today's ephemeral
  behaviour (tests, perf cluster); the operator template sets it.

### D5. Checkpoints are co-signed on the wire and anchor joiner sync

- The daemon constructs the DAG-S11 `Checkpointer` with
  `checkpoint_cadence_rounds` and calls `maybe_emit` at commit.
  `Checkpoint` gains `registry_root` (BLAKE3 of the canonical encoding of
  the Authority Ring, Validator Ring, stake table and epoch state) so a
  checkpoint commits to the committee as well as the substrate; the hash
  domain tag bumps to `SUWAPPU-CHECKPOINT-V2`. Nothing is deployed, so
  no migration is needed; the DAG-S11 proptest is updated in place.
- Each seated authority signs the checkpoint (`sign_checkpoint`) and
  broadcasts `WireMessage::CheckpointSig`. Nodes aggregate signatures per
  checkpoint hash and call `ratify_checkpoint` once the count reaches
  `quorum_threshold`; the resulting `CoSignedCheckpoint` is persisted with
  the snapshot. Only `CheckpointSig` frames from configured or seated
  peers are aggregated (same posture as `Tip`, `daemon.rs:1014-1019`).
- Joiner protocol, following Sui's checkpoint-verification walk:
  1. `Tip` becomes `TipInfo { max_round, gc_round, latest_checkpoint_height }`
     (appended variant; the legacy `Tip(u64)` index is preserved because
     bincode variant indexes are load-bearing, `wire.rs:106-107`).
  2. If every configured peer's `gc_round` exceeds the joiner's local
     round, forward backfill cannot succeed; the joiner requests
     `GetCheckpoints(from_height)` and receives the chain of
     `CoSignedCheckpoint`s.
  3. The joiner verifies the chain from the genesis Authority Ring: each
     checkpoint's signatures are checked against the committee it
     currently trusts; the `prev_checkpoint` link is checked; after a
     checkpoint verifies, the committee is replaced by the registry bound
     by that checkpoint's `registry_root`, obtained with the snapshot.
     This is `verify_checkpoint_chain` in `suwappu-execution::checkpoint`,
     a pure function with a 10k-case exit gate: any forged signature,
     broken link, or registry not matching `registry_root` is rejected;
     any honestly co-signed chain across committee changes is accepted.
  4. `GetSnapshot(height)` returns the `StateSnapshot`; the joiner
     verifies `state_root` and `registry_root` against the verified
     checkpoint, installs it, and resumes ordinary backfill from
     `checkpoint.round + 1`.
- The trust obtained is the same as for any committed block: an honest
  ≥ quorum of the Authority Ring at that checkpoint. This preserves
  Invariant 1's framing — a joiner can be fed a false state only by a
  Byzantine Authority quorum, which is already a safety violation.

### D6. Observability

`suwappu_getSyncStatus` (this branch) gains `gc_round`,
`latest_checkpoint_height` and `needs_snapshot`; `/metrics` gains
`suwappu_gc_round`, `suwappu_dag_certs` and `suwappu_checkpoint_height`.
The daemon emits `gc_pruned` (with counts) and `gc_late_flip` events.

## Invariant candidates

- **I-GC1 (commit determinism under GC).** For any leader `L` and any two
  DAG stores that agree on all certificates with round in
  `(commit_floor(L), L.round]`, the committed sub-DAG of `L` is identical.
  Exit gate: `proptest_gc.rs::committed_set_is_prune_invariant`.
- **I-GC2 (decision stability under pruning).** For any round
  `r > gc_round`, `decide_slot(dag, r, n)` is unchanged by
  `prune_below(gc_round)`. Exit gate: `decide_slot_is_prune_invariant`.
- **I-GC3 (bounded memory).** After pruning, the store holds at most
  `n × (GC_DEPTH + 1)` certificates plus `n × GC_DEPTH` tombstones per
  honest schedule. Exit gate: `store_size_is_bounded`.
- **I-P1 (replay equivalence).** Snapshot + log replay reproduces the
  live `state_root` and registries. Exit gate:
  `proptest_persistence.rs::replay_equivalence`.
- **I-CK1 (checkpoint chain soundness).** `verify_checkpoint_chain`
  accepts iff every checkpoint is co-signed by a quorum of the committee
  established by its predecessor. Exit gate:
  `proptest_checkpoint_chain.rs`.

These are candidates for CLAUDE.md §Load-bearing invariants pending the
human sign-off this IQ requires; they are not added there by this
branch.

## Residuals and interactions

1. **IQ-004 / #45 late `Skip → Direct` flips.** The current rule allows a
   leader slot to flip to `Direct` after later leaders have committed.
   Under GC, a flip for a slot at or below a node's `gc_round` can no
   longer be acted on by that node (its certificate is pruned), while a
   slower node might still commit it. This is a *new* divergence class
   only if a flip arrives more than `GC_DEPTH` rounds late; the
   pre-existing class (same certs committed at different sequence
   positions on different nodes) is unchanged. The clean fix is the
   Mysticeti-style sequential, final decision order that #45 already
   tracks; until then `GC_DEPTH = 256` is chosen to dwarf the
   single-round lag IQ-004 documents, and the daemon emits `gc_late_flip`
   whenever a slot ≤ `gc_round` becomes `Direct`, so the fault-injection
   run in `/goal` B2 will show whether the class is ever reached.
2. **Intents in certificates that fall below the floor are not
   committed.** Narwhal's stance; re-injection from the mempool is the
   remedy. The mempool is explicitly not persistent ("on restart, peers
   re-submit", `suwappu-mempool/src/lib.rs`), so this sprint documents the
   behaviour for SDK users (a submitted intent not observed in
   `suwappu_getTransaction` within `GC_DEPTH` rounds should be
   resubmitted) rather than adding re-injection.
3. **Checkpoint cadence trade-off.** Snapshots are written at checkpoint
   boundaries; a low cadence means large replay on restart, a high
   cadence means many co-signature rounds. The default stays at the
   epoch length (1024 rounds) for the testnet; `checkpoint_cadence_rounds`
   remains operator-tunable but must be identical across the mesh
   because it changes the co-signed message sequence.
4. **Consensus-reviewer + human sign-off.** Required before this branch
   is merged (CLAUDE.md §Specialist subagents; `/goal` Rules). The
   reviewer's verdict is recorded in the PR body.

## Implementation sketch (DAG-S34)

| Step | Crate | Exit gate |
|---|---|---|
| S34.1 | `suwappu-consensus` (`dag.rs`, new `gc.rs`, `commit.rs`) | `tests/proptest_gc.rs`: I-GC1, I-GC2, I-GC3, tombstone-tolerant insert × 10k |
| S34.2 | `suwappu-node` (`daemon.rs`, `wire.rs`, `rpc_adapter.rs`, `metrics_http.rs`) | daemon loopback test: 4-node cluster runs > `2 × GC_DEPTH` rounds with bounded store size and identical post-roots |
| S34.3 | `suwappu-node` (`store.rs`, `config.rs`, `main.rs`) | `tests/proptest_persistence.rs`: I-P1 × 10k; restart test |
| S34.4 | `suwappu-execution` (`checkpoint.rs`), `suwappu-node` (wire + daemon) | `tests/proptest_checkpoint_chain.rs`: I-CK1 × 10k; joiner-from-snapshot loopback test |
| S34.5 | docs, `/goal`, CLAUDE.md | consensus-reviewer verdict recorded |
