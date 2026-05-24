# SCION transport — path-authenticated routing + RaptorQ shred/reconstruct

Covers `crates/gsx-transport/` end-to-end: RaptorQ in-memory
(DAG-S2 exit gate), SCION path auth (DAG-S18), SCION-IP-Gateway fallback
(DAG-S19). Paper §6.3.

```mermaid
flowchart LR
    Author["Validator authors cert"] --> Shred["RaptorQ encode<br/>n shreds, any k reconstruct"]
    Shred --> Path{"SCION path<br/>available?"}
    Path -->|"yes"| SCION["Path-authenticated<br/>SCION transport"]
    Path -->|"no"| Gateway["SCION-IP-Gateway<br/>fallback shim"]
    SCION --> Wire[("Wire — cluster mesh")]
    Gateway --> Wire
    Wire --> Recv["Receiving validator<br/>collects ≥ k shreds"]
    Recv --> Reco["RaptorQ reconstruct"]
    Reco --> Ingest["ingest_cert<br/>insert into DAG"]
    Ingest -->|"UnknownParent"| Orphan["orphan-pull buffer<br/>fetch missing parents"]
    Ingest -->|"OK"| Vote["cast Vote<br/>broadcast"]
    Orphan -.->|"per-orphan backoff<br/>500ms → 1s → 2s → 4s → 5s cap<br/>DAG-S32"| Wire
```

## Notes

- **RaptorQ fountain code (S2):** payload is encoded into more shreds
  than strictly needed; any `k` distinct shreds reconstruct the source
  bit-exactly. Decouples delivery success from packet loss.
  `proptest_reconstruction.rs` × 10k cases.
- **SCION path auth (S18):** when the network supports SCION, paths
  are explicitly authenticated by the originating validator's
  path-segment certificates. `proptest_scion.rs` × 10k cases.
- **IP-Gateway fallback (S19):** if no native SCION path is
  available, the gateway shim carries the same shred set over plain
  IP with an equivalent authentication shim.
  `proptest_gateway.rs` × 10k cases.
- **Per-orphan backoff (S32):** the orphan-pull recovery has
  exponential backoff per missing-cert hash, so a slow consumer
  doesn't suffer a retry storm. See the
  `dag-orphan-pull-retry-storm-without-per-orphan-backoff` operator
  skill.
