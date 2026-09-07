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
| Votes | `State::votes` | removed only on commit (`daemon.rs:1962`) | Votes for never-committed certs stay forever. (S34.4: retained until the certificate is pruned at the gc round, so they can be relayed to catching-up peers; still bounded by the live window.) |
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
- As implemented (S34.4), three details the sketch above glossed over:
  1. **Votes are relayed with certificates.** Validator-Ring votes are
     unsigned gossip that seeds used to drop at commit. A joiner (or a
     restarted seed) catching up over rounds its peers already committed
     therefore held every certificate but no vote, and the AND-gate
     correctly halted it at the first un-ratified leader forever. Votes
     now live until their certificate is pruned at the gc round (still
     bounded by the live window), and every `GetCert` /
     `GetCertsByRound` reply is followed by a `Votes` frame for that
     certificate. The receiver applies the live-`Vote` rule (configured
     peers only), so this adds no trust the joiner did not already
     place in the peers it dials. No checkpoint-based shortcut around the
     Validator Ring was added: a co-signed checkpoint is Authority-Ring
     evidence only and using it to ratify leaders would collapse one ring
     into the other for the catch-up window (Invariant 1).
  2. **Backfill resumes below the DAG tip after an install or recovery.**
     Certificates above the restored leader frontier travel inside the
     snapshot / commit log without their votes or blocks; forward backfill
     keys on the DAG tip and would never re-ask for them. A one-shot
     `backfill_resume` re-pulls the tail from `leader_round + 1`.
  3. **Seeds push to dynamic peers.** Certificates, blocks, votes and
     checkpoint signatures are fanned out to every connected dynamic
     peer (not only configured ones) so a joiner that is nobody's
     configured peer still receives the live stream; dynamic peers remain
     unable to *inject* votes, tips, blocks or checkpoint signatures.
  4. **Checkpoint identity and content are anchored on agreed state.** A
     checkpoint's `round` is the boundary round, its `height` /
     `prev_checkpoint` come from the latest co-signed checkpoint, and its
     `state_root` covers exactly the leaders at or below the boundary: a
     boundary strictly below the next leader is crossed *before* that
     leader's sweep, and a leader at the boundary itself crosses it after
     its own sweep, so nodes that cross at different leaders sign the
     same value (consensus-review finding on S34.4). The checkpoint also
     carries `snapshot_root`, a commitment to the commit-derived body of
     the served snapshot (leader frontier, gc round, sorted commit marks,
     queued governance), and the node prunes to the frontier before
     capturing so that body is a function of the leader sequence. A node whose view wobbles at one boundary (IQ-004)
     produces one divergent hash and rejoins at the next boundary rather
     than poisoning every later checkpoint through its own prev-hash
     chain. `checkpoint_cadence_rounds` (genesis manifest, default 32)
     must satisfy `2 × cadence ≤ gc_depth_rounds` so the served snapshot
     window edge outlives the joiner's catch-up; `Daemon::start` rejects
     manifests that violate it.
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
  established by its predecessor, and consecutive heights chain by
  `prev_checkpoint`. Exit gate: `proptest_checkpoint_chain.rs`.

These are candidates for CLAUDE.md §Load-bearing invariants pending the
human sign-off this IQ requires; they are not added there by this
branch.

## Residuals and interactions

1. **IQ-004 / #45 late `Skip → Direct` flips.** The current rule allows a
   leader slot to flip to `Direct` after later leaders have committed.
   Under GC, a flip for a slot at or below a node's `gc_round` can no
   longer be acted on by that node (its certificate is pruned), while a
   slower node might still commit it. This is a *new* divergence class
   whenever a flip lands below the frontier at all: a late leader `L`
   with `L.round < last_leader` has `commit_floor(L) < gc_round`, so a
   node that has pruned sweeps `L`'s history cut at its own `gc_round`
   while a slower peer sweeps it cut at `commit_floor(L)` — a superset.
   (The consensus review of S34 corrected an earlier version of this
   paragraph that bounded the window by `GC_DEPTH`; the real condition is
   one round of lateness, and the difference is confined to certificates
   at rounds in `(commit_floor(L), gc_round]` that no earlier committed
   leader already swept.) The daemon makes the clamp explicit
   (`floor = max(commit_floor(L), gc_round)`), emits `gc_late_flip` each
   time it applies, and `proptest_gc.rs::bounded_history_below_gc_is_clamped`
   pins the clamped sweep as a property of the DAG (the clamp also
   applies when the leader has no floor at all, i.e. `commit_floor` is
   `None` while the node has pruned). The consequence is not transient:
   the certificates only the slower peer sweeps have their intents
   applied there and never on the pruned node, so the two post-roots stay
   apart until the pruned node re-bootstraps from a co-signed snapshot,
   and if at least `f + 1` nodes split the mesh can no longer co-sign a
   checkpoint at that or any later boundary. The clean fix is the
   Mysticeti-style sequential, final decision order that #45 already
   tracks; until then the fault-injection run in `/goal` B2 will show how
   often the class is reached.
2. **Intents in certificates that fall below the floor are not
   committed.** Narwhal's stance; re-injection from the mempool is the
   remedy. The mempool is explicitly not persistent ("on restart, peers
   re-submit", `suwappu-mempool/src/lib.rs`), so this sprint documents the
   behaviour for SDK users (a submitted intent not observed in
   `suwappu_getTransaction` within `GC_DEPTH` rounds should be
   resubmitted) rather than adding re-injection.
3. **Checkpoint cadence trade-off.** Snapshots are written at checkpoint
   boundaries; a low cadence means large replay on restart, a high
   cadence means many co-signature rounds. The manifest default is 32
   rounds (one eighth of `GC_DEPTH`, well inside the `2 × cadence ≤
   gc_depth` bound); `checkpoint_cadence_rounds` is a genesis-manifest
   field, identical across the mesh by construction, because it changes
   the co-signed message sequence.
4. **Consensus-reviewer + human sign-off.** Required before this branch
   is merged (CLAUDE.md §Specialist subagents; `/goal` Rules). The
   reviewer's verdict is recorded in the PR body. The first review
   (S34.5) returned NEEDS-CHANGES; the two HIGH findings (snapshot capture
   racing the commit claim; snapshot body not bound by the checkpoint) and
   the I-GC1 gate gap are fixed on this branch (`State::commit_lock`,
   `Checkpoint.snapshot_root` + `verify_served_snapshot`, the clamp
   above). The items below are what remains for the human decision.
5. **Validator-Ring votes are unsigned relay data.** DAG-S5 defined
   `Vote` without a signature; a live node accepts votes from configured
   peers only, and S34.4 relays votes to catching-up peers under the same
   rule. A single Byzantine *configured* peer can therefore vouch for
   votes it did not originate and satisfy `validator_quorum_met` for one
   leader on its own — the Validator-Ring half of Theorem 2 is only as
   strong as the configured peer set until votes are signed (ML-DSA over
   `network_id || candidate || validator`, verified against the Validator
   Ring). Votes from ids outside the Validator Ring are dropped and a
   `Votes` frame is capped, which bounds the map but not the trust.
   Signed votes are a DAG-S35 candidate; they change the vote wire size
   from 36 B to ~3.3 KB per vote.
6. **Bootstrap trusts the Authority Ring alone.** A joiner installs the
   state a quorum of the Authority Ring co-signed; the Validator Ring
   provides no independent check over that prefix. Leaders above the
   checkpoint are still ratified through the live AND-gate (no shortcut
   around `validator_quorum_met` exists), but the adopted prefix is
   Authority-only evidence — specifically the Authority Ring in force at
   each checkpoint of the verified chain; certificates in the served
   window are admitted only against the ring in force at their round, so
   a key seated in an earlier era cannot mint window certificates at
   live rounds. The same trust applies in steady state: a node whose
   own checkpointing has stalled adopts a strictly newer chain a
   configured peer serves (verified from the genesis committee, cursor
   only — no state is installed), which replaces the committee it
   verifies future co-signatures against. This is the documented
   bootstrap exception to Invariant 1 and needs the human sign-off this
   IQ requires; the
   alternative is a stake-weighted Validator-Ring co-signature over the
   checkpoint hash. Related: the served chain is sparse (transitions plus
   the latest), so `verify_checkpoint_chain` checks `prev_checkpoint`
   only between consecutive heights; across a gap every link is still
   signed by a committee that was legitimately in force, and what a gap
   can hide is only which one — the usual long-range-key exposure of any
   proof-of-stake bootstrap, mitigated operationally (fresh joiners should
   take the genesis manifest and a recent checkpoint hash from the
   published artifacts, not only from the peers they dial).
7. **Tombstones and certificates in a snapshot are validated
   structurally, not committed to.** The certificate window is
   receipt-timing dependent and cannot be part of a consensus-agreed
   root; a joiner checks every certificate's signature against the bound
   Authority Ring in force at the certificate's round — the rings
   established by verified checkpoints within `gc_depth` rounds below
   it (an honest node ingests against its live registry, which trails
   its commit frontier by up to that much, so an ejected author's
   certificates legitimately sit up to `gc_depth` above the eject), the
   last ring below it, and the next one as admit grace; a key seated
   only before that lag admits nothing —
   its round against the bound gc round, at most two certificates per
   (author, round) — the same cap ingest applies, two being all an
   equivocation proof needs — and the tombstone window's rounds and
   size. A Byzantine authority's certificate
   referencing a fabricated pruned parent can still enter a joiner's
   window through a fabricated tombstone — the same exposure a live node
   has at its own window edge, and one that affects only support counts
   at rounds the joiner will re-decide from live data. The served window
   is exactly `(gc_round, ck.round]` — certificates above the checkpoint
   round are dropped at capture, and both ends are checkpoint-bound
   (`ck.round` directly, `gc_round` through `snapshot_root`) — so the
   joiner enforces the bound `2 × |authors bound by the chain| ×
   (ck.round − gc_round)`, provable from the per-slot cap. Every
   other snapshot field is either bound (`state_root`, `registry_root`,
   `snapshot_root`) or node-local and never installed from a peer
   (`pending_stake` is derived from the bound registries,
   `last_authored_round` and `log_sequence` are cleared, the checkpoint
   cursor is derived from the trusted checkpoint). `GetSnapshot` is
   answered for any peer without a rate limit; the reply is bounded
   (`SNAPSHOT_CHUNK_BYTES` × chunks) but not free. Checkpoint
   signatures aggregate against every recently emitted checkpoint
   (bounded to eight, each holding its snapshot in memory until
   settled), buffered only for heights this node could ratify next and
   at most `MAX / n` foreign entries per signer; a
   node whose own checkpointing stalls for two boundaries asks peers for
   their chain and adopts a strictly newer verified one, re-anchoring its
   cursor without touching consensus state. Two Authority-Ring changes
   inside one cadence window leave an intermediate ring no checkpoint
   records; that member's certificates in the window are unverifiable
   by joiners until GC passes them.
8. **The authoring round is anchored on quorum, and the admissible round
   window is bounded.** A validator that falls behind jumps its next
   authoring round to one above the highest round holding a quorum of
   distinct authors — never to the raw DAG tip, which one seated
   authority can push arbitrarily high with a single valid certificate.
   Ingest drops any certificate more than `gc_depth` rounds above that
   same anchor (or the commit frontier, whichever is higher), so a single
   authority cannot ratchet the window either: the per-node DAG stays
   within one retention window of honest progress, and a joiner far
   behind catches up by backfill and snapshot, not by live pushes. Both
   were consensus-review findings on the S34.5 fix pass. Known gap: the
   quorum scan iterates authority ids `0..n`, like `parents_for_round`
   before it; a mid-ring eject that leaves ids non-contiguous is a
   pre-existing follow-up.
9. **Authors now vote for their own certificates.** Found while widening
   the restart test to an outage longer than the retention window: the
   Validator-Ring side of the AND-gate only ever collected votes from
   *other* seated validators, so a certificate had at most `n − 1` votes
   and a four-node ring with one member down could not ratify any leader
   (three survivors: 300k of a 600k table against a 400,001 threshold).
   The author's own vote is recorded and broadcast at proposal time. This
   is a pre-existing liveness hole, not an IQ-008 change; it is recorded
   here because the S34 tests are what exposed it.

## Implementation sketch (DAG-S34)

| Step | Crate | Exit gate |
|---|---|---|
| S34.1 | `suwappu-consensus` (`dag.rs`, new `gc.rs`, `commit.rs`) | `tests/proptest_gc.rs`: I-GC1, I-GC2, I-GC3, tombstone-tolerant insert × 10k |
| S34.2 | `suwappu-node` (`daemon.rs`, `wire.rs`, `rpc_adapter.rs`, `metrics_http.rs`) | daemon loopback test: 4-node cluster runs > `2 × GC_DEPTH` rounds with bounded store size and identical post-roots |
| S34.3 | `suwappu-node` (`store.rs`, `config.rs`, `main.rs`) | `tests/proptest_persistence.rs`: I-P1 × 10k; restart test |
| S34.4 | `suwappu-execution` (`checkpoint.rs`), `suwappu-node` (wire + daemon) | `tests/proptest_checkpoint_chain.rs`: I-CK1 × 10k; joiner-from-snapshot loopback test |
| S34.5 | docs, `/goal`, CLAUDE.md | consensus-reviewer verdict recorded |
