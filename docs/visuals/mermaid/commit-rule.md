# DagBft-C commit rule — direct + indirect + orphan window

Covers `crates/suwappu-consensus/src/commit.rs` end-to-end: `try_direct_decide`,
`try_indirect_decide`, `decide_slot`, `causal_history`. Highlights the
parent-set freeze window that [IQ-004](../../iq/IQ-004-decide-slot-orphan-window.md)
catches.

```mermaid
flowchart TB
    R["Round R<br/>leader cert authored<br/>by leader(R, n)"]
    R1["Round R+1<br/>peer authors propose<br/>parent set = local DAG R-certs"]
    Q{"Distinct authors at R+1<br/>citing leader_hash as parent<br/>≥ quorum_threshold(n)?"}
    Direct["LeaderStatus::Direct<br/>commit + drain block"]
    Anchor["Search anchor at R' ≥ R+2<br/>directly decided"]
    InHist{"leader cert in<br/>causal_history(anchor)?"}
    Skip["LeaderStatus::Skip<br/>permanently dropped<br/>one-shot intent lost"]
    Late["Late-arriving leader cert<br/>via orphan-pull"]
    IQ4["IQ-004 fix candidates:<br/>A) late-arrival re-decide<br/>B) wait-for-leader timer<br/>C) author-buffered duplicates"]

    R --> R1
    R1 --> Q
    Q -->|"yes"| Direct
    Q -->|"no"| Anchor
    Anchor --> InHist
    InHist -->|"yes"| Direct
    InHist -->|"no"| Skip
    Late -.->|"arrives AFTER R+1<br/>parent-set frozen"| Q
    Skip -.->|"mitigated by"| IQ4
```

## Notes

- Honest commit path: a healthy round-R cert + ≥`quorum_threshold(n)`
  distinct supporters at R+1 finalizes via the direct rule.
  `quorum_threshold(n) = n − ⌊(n−1)/3⌋` per [IQ-001](../../iq/IQ-001-quorum-formula.md).
- Indirect path ([IQ-002](../../iq/IQ-002-indirect-commit.md)): if R is
  Undecided, scan for an anchor at R' ≥ R+2 that is directly decided;
  walk its `causal_history` to determine R's outcome.
- Orphan window ([IQ-004](../../iq/IQ-004-decide-slot-orphan-window.md)):
  if the leader's R cert is delivered to a peer *after* that peer has
  proposed at R+1, the leader is not in R+1's parent set. Orphan-pull
  delivers the cert into the DAG but `causal_history` (which walks
  parents) never reaches it from any later anchor. The slot is
  permanently `Skip` and any one-shot intent it carried vanishes.
- Joint-quorum gate (not shown — handled in `joint.rs`): the
  Validator Ring also has to co-sign before the candidate cert
  finalizes. This is paper Theorem 2's AND-gate.
