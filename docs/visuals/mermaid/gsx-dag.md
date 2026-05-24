# GSX DAG — Mermaid

```mermaid
flowchart LR
  Users["Users / Apps"] --> DAG["gsx-dag"]
  DAG --> C["Consensus<br/>Mysticeti-C DAG"]
  C --> E["Execution<br/>Dual VM"]
  E --> DB["gsx-db<br/>Canonical state lattice"]
  DB --> LTP["LTP<br/>Commit / Lattice / Materialize"]
  LTP --> Corridors[("Base chains / corridors")]
  subgraph Rings ["Dual-ring security (joint-quorum AND-gate)"]
    A["Authority Ring<br/>30–50 institutions"]
    V["Validator Ring<br/>100–500 stake-weighted participants"]
  end
  C --- Rings
```
