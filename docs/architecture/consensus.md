# Consensus

Paper §6. Implemented in [`suwappu-consensus`](../../crates/suwappu-consensus).
**This doc is the canonical "how it works" writeup.** Design-decision
detail (paper-vs-production formulas, indirect commit, orphan-window
liveness gap) lives in the IQs linked inline.

## Topology

Throughput and ordering are decoupled. Validators produce certificates into
a directed-acyclic-graph gossip layer in parallel; a Mysticeti-style BFT
linearization protocol orders the DAG into a canonical linear history
[Babel et al., 2023].

Target mainnet parameters:

| Metric | Target |
|---|---|
| Certificate production | sub-second |
| Deterministic finality (p95, honest quorum) | ≤ 3 s |
| Transaction-throughput headroom (general availability) | > 10,000 TPS |
| Transaction-throughput peak (reduced TX size) | 50,000 TPS |

p95 finality budget breakdown:

- ≈500 ms certificate production
- ≈500 ms reliable broadcast
- ≈500 ms DAG advancement to the subsequent round
- ≈500 ms leader commit
- ≈1 s network-delay variance

## Mysticeti-C selection

Mysticeti-C is the consensus base. Five reasons (paper §6.2):

1. **Apache 2.0 license** — removes the IP-contamination risk of earlier
   Monad-derived paths.
2. **Production-validated at Sui-mainnet scale** [Sui Consensus, 2024].
3. **Deterministic finality through an uncertified DAG with a novel commit
   rule** — eliminates probabilistic-reorganization concerns of weakly-
   finalizing DAG protocols.
4. **Consensus path is hash-based** — natively post-quantum on the safety
   surface.
5. **Mysticeti v2 [Sui Mysticeti V2, 2025]** is the upstream evolution target.

## Parent-set selection

When the round driver proposes at round R, it takes the parent set from
its *current local view* of round R-1 certs:

```rust
parents = parents_for_round(&dag, target_round, n);
```

This is the standard Mysticeti propose-with-what-you-have rule. It admits
a known orphan window: if the round-R leader cert hasn't arrived at peer A
by the time A proposes at R+1, A ships round R+1 without that hash in its
parents field. Orphan-pull recovery delivers the cert into the DAG later,
but A's round-R+1 cert is already committed and the leader cert can no
longer be a parent of any R+1 cert.

The implication for `decide_slot` is captured in
[IQ-004](../iq/IQ-004-decide-slot-orphan-window.md): the slot stays
`Undecided` or eventually `Skip` cluster-wide and any one-shot intent
the leader cert carried vanishes from the commit pipeline. Current
mitigation: client-side resubmit on timeout (see PR #44 + the
`dag-decide-slot-single-cert-orphan-after-parent-set-frozen` operator
skill). Real fix tracked in
[#45](https://github.com/suwappu/suwappu-dag/issues/45).

## Quorum formula

`crates/suwappu-consensus/src/commit.rs::quorum_threshold` ships:

```rust
pub fn quorum_threshold(n: CommitteeSize) -> u32 {
    if n == 0 { return 1; }
    n - (n - 1) / 3
}
```

This is `2f+1` for `n = 3f + 1` — the canonical BFT supermajority and the
formula every production DAG-BFT implementation (Sui, Aptos, Bullshark)
ships. It diverges from paper §6.4's literal `⌈2n/3⌉ + 1`, which
collapses to unanimity for `n ∈ {1, 4, 7, …}`.

**Why this divergence is safe.** Paper Definition 2's
"strict majority of 2/3" inequality is satisfied by `2f+1` at every
`n = 3f+1`, so Theorem 2's safety proof is unchanged. The integer
encoding is the only thing the production formula adjusts. Full
analysis: [IQ-001](../iq/IQ-001-quorum-formula.md) — ratified
2026-05-14 via [suwappu-papers#1](https://github.com/suwappu/suwappu-papers/pull/1).

## Commit rule

`decide_slot` runs the Mysticeti-C direct + indirect rule:

```mermaid
flowchart TB
    R[Round R<br/>leader cert authored<br/>by leader(R, n)]
    R1[Round R+1<br/>peer authors propose<br/>parent set = local DAG R-certs]
    Q{Distinct authors at R+1<br/>citing leader_hash as parent<br/>≥ quorum_threshold(n)?}
    Direct[LeaderStatus::Direct<br/>commit + drain block]
    Anchor[Search anchor at R' ≥ R+2<br/>directly decided]
    InHist{leader cert in<br/>causal_history(anchor)?}
    Skip[LeaderStatus::Skip<br/>permanently dropped<br/>one-shot intent lost]
    Late[Late-arriving leader cert<br/>via orphan-pull]
    IQ4[IQ-004 fix candidates:<br/>A late-arrival re-decide<br/>B wait-for-leader timer<br/>C author-buffered duplicates]

    R --> R1
    R1 --> Q
    Q -->|yes| Direct
    Q -->|no| Anchor
    Anchor --> InHist
    InHist -->|yes| Direct
    InHist -->|no| Skip
    Late -. arrives AFTER R+1<br/>parent-set frozen .- Q
    Skip -. mitigated by .- IQ4
```

- **Direct rule** (`try_direct_decide`): a leader cert at round R is
  directly committed if ≥ `quorum_threshold(n)` distinct authors at
  R+1 list `leader_hash` in their parents field.
- **Indirect rule** ([IQ-002](../iq/IQ-002-indirect-commit.md), ratified
  2026-05-14): if direct returns `Undecided`, scan for an anchor at
  R' ≥ R+2 that is directly decided; walk `causal_history(anchor)`.
  If the leader cert is reachable, the slot inherits `Direct`;
  otherwise `Skip`.
- **Orphan window** (IQ-004, pending): the paper-vs-production
  liveness gap that surfaced via issue #35. See the
  [Parent-set selection](#parent-set-selection) section above.

Wire-level walkthrough lives in
[`docs/visuals/mermaid/commit-rule.md`](../visuals/mermaid/commit-rule.md)
or [presentation](../visuals/commit-rule.html).

## Joint-quorum AND-gate (Theorem 2)

`crates/suwappu-consensus/src/joint.rs::validator_quorum_met` runs the
Validator-Ring stake side. A candidate cert finalizes iff:

1. The Authority Ring's `decide_slot` returns `Direct(leader_hash)`, AND
2. `voting_stake(stake_table, leader_hash, votes) ≥
   validator_quorum_threshold(stake_table)` (strictly >2/3 of total stake).

This is paper Theorem 2: a safety violation requires Byzantine corruption
of *both* rings simultaneously. SUWAPPUHELPER.md tracks this as Invariant 1.
Full safety + liveness writeup: [safety-liveness.md](safety-liveness.md).

## Inter-validator transport

Inter-validator transport runs on SCION [SCION Book, 2017] with a
SCION-IP-Gateway fallback for external clients. SCION's path-authenticated
routing eliminates the BGP-class attack vector that has produced multiple
production blockchain incidents on flat IP infrastructure [Birgi et al., 2022].
Trust Root Configuration governance over the validator mesh's Isolation
Domain provides cryptographically anchored route-authority rotation. Block
propagation uses RaptorQ erasure coding (RFC 6330) [RFC 6330, 2011].

Detailed transport spec: [transport.md](transport.md). Implementation:
[`suwappu-transport`](../../crates/suwappu-transport).

## Fast-path lane

A fast-path lane runs in parallel with main-lane consensus for single-
owner-object operations [FastPay, 2020]. Eligibility is restricted to
transactions whose read-write footprint is a single owned Move object
with the owner as sole signer and lineage grounded in a main-lane path.
A fast-path certificate is binding subject to main-lane confirmation
within K rounds (target K=4, ≈2 s); equivocation is slashable at 100%
of the offending Authority Node's bonded stake plus expulsion.

Detailed spec: [fast-path.md](fast-path.md). Architecture decision in
[IQ-003](../iq/IQ-003-fast-path-architecture.md). Implementation:
[`suwappu-fastpath`](../../crates/suwappu-fastpath).

## Sprint exit gates

| Sprint | Exit gate |
|---|---|
| DAG-S3 | `dag_topological_order_unique` @ 10k |
| DAG-S4 | `mysticeti_c_finality` @ 10k |
| DAG-S5 | `joint_quorum_safety` @ 10k (Theorem 2) |
| DAG-S21.1 | IQ-001 ratification ([suwappu-papers#1](https://github.com/suwappu/suwappu-papers/pull/1)) |
| DAG-S21.2 | IQ-002 ratification ([suwappu-papers#1](https://github.com/suwappu/suwappu-papers/pull/1)) |

## Cross-references

- **Design decisions:** [IQ-001](../iq/IQ-001-quorum-formula.md) (quorum
  integer encoding), [IQ-002](../iq/IQ-002-indirect-commit.md) (indirect
  commit), [IQ-004](../iq/IQ-004-decide-slot-orphan-window.md) (orphan
  window).
- **Engineering:** `crates/suwappu-consensus/src/commit.rs`,
  `crates/suwappu-consensus/src/joint.rs`, `crates/suwappu-node/src/daemon.rs::run_round_driver`.
- **Visuals:** [commit-rule](../visuals/mermaid/commit-rule.md),
  [README](../visuals/README.md), [index.html](../visuals/index.html).
- **Safety + liveness narrative:** [safety-liveness.md](safety-liveness.md).
