# IQ-004 — `decide_slot` single-cert orphaning under parent-set freeze

**Status:** Draft — awaiting ratification
**Owner:** consensus
**Date:** 2026-05-15
**Tracking:** [#35](https://github.com/GlobalSettlementNetwork/gsx-dag/issues/35) (closed by [#44](https://github.com/GlobalSettlementNetwork/gsx-dag/pull/44), test-side mitigation only) + [#45](https://github.com/GlobalSettlementNetwork/gsx-dag/issues/45) (real consensus fix)

## Question

When a round-R leader cert is delivered to a peer *after* that peer has
already proposed at round R+1, every R+1 proposer ships without the
leader cert hash in its `parents` set. The cert eventually reaches the
DAG via orphan-pull (`crates/gsx-node/src/daemon.rs:659-686`), but by
then `try_direct_decide(R)` and every subsequent anchor's
`causal_history` walk both miss it. The slot stays `Undecided` →
eventually `Skip`. Any one-shot intent that lived in only that single
cert is silently dropped from the commit pipeline.

Should `gsx-consensus` close this liveness gap, and if so, how?

## Background

Three layers interact:

1. **Parent-set selection at propose time**
   (`crates/gsx-node/src/daemon.rs::run_round_driver`, ~line 1544):
   ```rust
   parents = parents_for_round(&dag, target_round, n);
   ```
   This takes a snapshot of every round-(R-1) cert currently in the
   local DAG and uses it as the parent set for the cert being proposed
   at round R. There is no wait-for-leader timer.

2. **Direct commit rule**
   (`crates/gsx-consensus/src/commit.rs::try_direct_decide`, line 126):
   ```rust
   let support = supporters(dag, leader_hash, round + 1);
   if support.len() as u32 >= quorum_threshold(n) {
       LeaderStatus::Direct(leader_hash)
   } else {
       LeaderStatus::Undecided
   }
   ```
   `supporters` counts the distinct authors of round-R+1 certs whose
   `parents` field contains `leader_hash`. Permanent — a peer can't
   retroactively edit a cert's parent set.

3. **Indirect commit rule**
   (`crates/gsx-consensus/src/commit.rs::try_indirect_decide`, line 149):
   ```rust
   if causal_history(dag, anchor).contains(&target_hash) { Direct } else { Skip }
   ```
   `causal_history` walks parents transitively. A cert that isn't a
   parent of *any* round-R+1+ cert is unreachable from every anchor.

The orphan-pull buffer (#21) ensures the late cert *reaches the DAG*.
It doesn't (and can't) edit the round-R+1 certs already in flight.

## Evidence

- **#35 reproduction** (4-daemon n=4 on GHA ubuntu-latest, `phase_g_admit_and_eject`):
  ~28% per-attempt failure rate at 60s deadline. Every failure shows
  the same shape:
  ```
  v0..v3: eject_in_block=true eject_block_round=53
          eject_cert_committed=false pending_gov(n=0,eject=false)
          committed=167 epoch(cur=3,last_bd=48)
  ```
  The intent reaches every daemon's `state.blocks` (orphan-pull
  worked), but the containing cert never lands in `state.committed`
  on any daemon. The cluster commits 167+ other certs and crosses
  3 epoch boundaries — proving the commit pipeline is otherwise
  healthy and only this one slot is wedged.

- **Differential diagnosis ruled out:**
  - `bft-stake-denominator-deadlock-on-admit`: `stake(tot)>thr`,
    cluster keeps committing — not the stake-table inflation bug.
  - `mysticeti-leader-rotation-needs-active-manifest`: leader cert
    IS authored on time — the slot is not "permanently missing
    leader", the leader's cert is in the DAG, just orphaned from
    every R+1 cert.
  - `dag-consensus-orphan-cert-silent-split`: orphan-pull DOES
    deliver. This is a different layer.

- **Sui Lutris** (`consensus/core/src/round.rs::propose_block`): ships
  a "wait for the round-R leader before proposing R+1" timer with a
  fallback timeout, plus a parents-quorum gate. The wait-for-leader
  rule closes the orphan window in practice; the indirect-decide
  path is still theoretically vulnerable, but is rarely hit because
  honest proposers wait.

- **#44 mitigation**: client-side resubmit every 5s + 180s deadline
  takes the test from 28% failure rate to 0% in a 6-rerun sample.
  Mitigation works because each resubmission lands in a *fresh*
  leader cert at a later round R'. As long as one resubmission's R'
  falls in a wave where every daemon has every R'-1 cert at parent
  selection time, the slot direct-commits and `causal_history` picks
  it up. The mitigation does not address the root cause.

## Options considered

### Option A — Late-arrival re-decide (consensus-side)

When orphan-pull delivers cert C at round R, re-run `decide_slot(R)`
if `leader(R, n) == C.author`. Also re-run `decide_slot` for every
round < R that was previously `Skip` and is now reachable from any
directly-decided anchor's updated causal history.

- **Pros:** Correctness — every cert that the cluster eventually
  agrees on becomes committable. No latency penalty in the common case.
- **Cons:** Changes the *definition* of "finalized history" to allow
  retroactive `Skip → Direct` flips. Theorem 2's safety proof needs
  to be re-checked in this stronger setting (informally: the flip
  only happens when `causal_history` of an anchor that was already
  joint-committed includes the late cert, so the safety guarantee
  inherits from the anchor — but this needs a written argument).
- **Where:** `crates/gsx-consensus/src/commit.rs::decide_slot` +
  trigger from `crates/gsx-node/src/daemon.rs::ingest_cert` on
  successful insert of any cert whose author == `leader(round, n)`.

### Option B — Wait-for-leader timer at propose time (consensus-side)

Round driver waits up to `leader_timeout / 2` for the deterministic
leader's R cert before proposing R+1, unless f+1 fallback fires.
Matches Sui Lutris production behavior.

- **Pros:** Simple, mechanically obvious, matches a known-good
  reference implementation. No change to the commit rule.
- **Cons:** Median round latency goes up by ~half a leader_timeout
  in the common case where the leader cert is in-flight. At
  `round_ms=100ms, LEADER_TIMEOUT_ROUNDS=2` the timer adds ~100ms
  median latency per round, which compounds over a campaign.
- **Where:** `crates/gsx-node/src/daemon.rs::run_round_driver`
  Phase-1 lock window, before `parents_for_round` is called.

### Option C — Author-buffered duplicate emission (protocol-side)

Phase G governance intents are pinned to N=3 consecutive leader
cert proposals server-side. Effectively the same as the
client-side resubmit shipped in #44, but enforced by the protocol.

- **Pros:** Backwards-compatible with single-cert client wires
  (clients don't need to know to retry). Targets the specific
  intent class — Transfer and other high-volume intent types
  are unaffected.
- **Cons:** N-1 epoch boundaries of duplicate intent traffic on
  every governance op. Doesn't fix the underlying liveness gap
  for any non-governance one-shot intent (e.g., a sole high-value
  transfer in a sparse block).
- **Where:** `crates/gsx-node/src/daemon.rs::run_round_driver`
  intent batching loop + `try_commit` dedup at commit time.

## Recommendation

**Option A (late-arrival re-decide)** is the only one that closes
the gap *generally* — for governance intents, for Eject, for any
one-shot intent that happens to land in a single cert during a
jitter window. The safety argument is approachable: a retroactive
`Skip → Direct` flip only happens when an already-joint-committed
anchor's updated `causal_history` includes the late cert; the
joint-commit on the anchor is the safety witness.

The retroactive flip needs its own proptest gate:

```rust
// crates/gsx-consensus/tests/proptest_late_arrival.rs
proptest! {
    #[test]
    fn late_arriving_leader_cert_resolves_slot(
        n in 4u32..=10,
        late_round in 1u64..=20,
    ) {
        // Build a DAG where every node at late_round+1 was proposed
        // BEFORE the leader's late_round cert arrived. Insert the
        // leader cert last via the orphan-pull path. Assert:
        // decide_slot(late_round) eventually returns Direct after
        // the late-arrival re-decide trigger fires.
    }
}
```

Plus: revert #44's test-side resubmit loop and run
`phase_g_admit_and_eject` 50× back to back, all pass.

## Implementation sketch (if Option A is ratified)

1. Add `state.decide_pending: BTreeSet<Round>` — rounds whose
   `decide_slot` returned `Skip` or `Undecided` at last evaluation.

2. In `ingest_cert` (post-insert site), if the inserted cert's author
   == `leader(cert.round, n)`:
   ```rust
   state.inner.lock().await.decide_pending.insert(cert.round);
   ```

3. In `try_commit` (`crates/gsx-node/src/daemon.rs:1125`),
   before the main `candidate_rounds` loop, re-evaluate every
   `decide_pending` round. If `decide_slot` now returns `Direct`,
   thread it through the same commit/queue/boundary pipeline as
   the regular `candidate_rounds` loop.

4. Update `safety-liveness.md` with the retroactive-flip safety
   argument. Get a written review from the consensus-reviewer
   subagent.

## Decision

_Pending sign-off. Track in #45._

## See also

- `dag-decide-slot-single-cert-orphan-after-parent-set-frozen` skill
  (`~/.claude/skills/...`) — failure mode + diagnostic field table
- [IQ-002](./IQ-002-indirect-commit.md) — indirect commit rule, the
  layer this IQ extends with late-arrival semantics
- `crates/gsx-node/src/daemon.rs::run_round_driver` parent-set
  selection site
- `crates/gsx-consensus/src/commit.rs` — `decide_slot`,
  `try_direct_decide`, `try_indirect_decide`, `causal_history`
