# GSX DB — Mermaid

```mermaid
flowchart LR
  Lane[gsxdb-lane<br/>untrusted ingest] --> Bridge[gsxdb-bridge<br/>validation + OCC]
  Bridge --> State[gsxdb-state<br/>canonical state]
  Lane -. cannot import .- State
  State --> Tree[State tree<br/>root + proofs]
  State --> Anchor[AnchorDispatcher<br/>MAC / registry]
  State --> Replay[Replay / recovery]
  Read1[EVM projector<br/>balanceOf] --> State
  Read2[Move projector<br/>Coin::value] --> State
```
