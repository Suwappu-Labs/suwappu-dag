# Fast path + equivocation slashing

Covers `crates/gsx-fastpath/` and the daemon-side handler at
`crates/gsx-node/src/daemon.rs::handle_fastpath_cert` (~lines 499-820).
Paper §6.4. Sprint exit gates: DAG-S8 (lane), DAG-S9 (slashing).

```mermaid
flowchart LR
    Client[Client submits<br/>single-owner intent] --> Lane[Fast-path lane<br/>K-of-N owner-binding cert]
    Lane -->|K=4 confirmations| Ack[Client receives ack<br/>1-RTT commit confirmation]
    Lane --> Cross{K-binding cross-check<br/>vs main_lane_index<br/>(populated by try_commit)}
    Cross -->|consistent| Final[Fast-path cert<br/>safe to spend]
    Cross -->|conflict<br/>in K-window| Equiv[EquivocationProof<br/>two fast-path certs<br/>same owner, same nonce]
    Equiv --> Slash[100% bonded stake<br/>+ Authority Ring expulsion]
    subgraph MainLane[Main-lane fallback]
      MLane[Same intent flows<br/>through Mysticeti-C DAG]
      MLane --> Commit[Block commit<br/>main_lane_index updated]
    end
    Cross -.- MLane
    Slash -. paper §6.4<br/>Invariant 5 .- Inv[100% slashing<br/>+ expulsion]
```

## Notes

- Single-owner restriction (S8): an intent touching more than one owned
  object cannot ride the fast-path; the lane state machine rejects it
  at admission.
- K-binding cross-check (IQ-003): the fast-path cert is matched against
  the `main_lane_index` `try_commit` populates on every committed
  block. A mismatch within the K-window window — i.e., the main-lane
  ordering disagrees with the fast-path commit — produces an
  `EquivocationProof`.
- Slashing (S9): the proof is non-interactive and publishable; any
  honest validator can submit it. Slashing is total (100% bonded
  stake) plus immediate Authority Ring expulsion. Paper Invariant 5,
  CLAUDE.md.
- The IQ-003 implementation status: handler + proposer wired,
  K-binding cross-check defined in `binding.rs` but not yet exercised
  outside unit tests at the daemon level. See
  [IQ-003](../../iq/IQ-003-fast-path-architecture.md).
