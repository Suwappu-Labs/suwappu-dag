# Transport — SCION path-authenticated routing + RaptorQ shred/reconstruct

**Paper §**: 6.3 — Inter-validator transport ([`gsx-papers/papers/dag-l1`](https://github.com/GlobalSettlementNetwork/gsx-papers))
**Code**: `crates/gsx-transport/src/` (entry: `lib.rs`)
**IQs**: —
**Visuals**: [`docs/visuals/mermaid/scion-transport.md`](../visuals/mermaid/scion-transport.md) *(coming with PR-2)*
**Sprint**: DAG-S2 (RaptorQ) ✅ Closed · DAG-S18 (SCION) ✅ Closed · DAG-S19 (SCION-IP-Gateway) ✅ Closed

## What it does

`gsx-transport` carries certificates, votes, fast-path messages, and LTP
attestations between validators. Cert payloads are sharded with RaptorQ
fountain codes so any `k`-of-`n` shards reconstruct the original, decoupling
delivery success from packet loss. Path-authenticated routing rides SCION when
the network supports it and falls back to a SCION-IP-Gateway shim otherwise —
the path choice is opaque to the round driver.

## Key invariants

- **In-memory determinism (S2 exit gate):** `proptest_reconstruction.rs` ×
  10,000 cases — any `k` distinct shards reconstruct the source payload
  bit-exactly.
- **SCION path authentication (S18 exit gate):** `proptest_scion.rs` × 10,000
  cases — only well-formed authenticated paths advance the path-selection
  state machine.
- **Gateway fallback (S19 exit gate):** `proptest_gateway.rs` × 10,000 cases
  — when no native SCION path is available, the IP-Gateway shim delivers the
  same shred set without compromising authentication.

## Cross-references

- **Engineering:** `crates/gsx-transport/src/lib.rs`, the inbox + outbound
  channels at `crates/gsx-node/src/daemon.rs::run_inbox` consume the wire
  events transport produces.
- **Spec:** Paper §6.3, §6.4 (transport plus fast-path overlay).
- **Design decisions:** none ratified — transport choices were spec'd in the
  paper, not deferred to an IQ.
- **Visual:** [scion-transport](../visuals/mermaid/scion-transport.md)
  *(coming with PR-2)*; the broader stack picture is in
  [`docs/visuals/README.md`](../visuals/README.md).
- **Related operator concern:** orphan-pull retries that ride on this
  transport are bounded by per-orphan backoff added in DAG-S32 — see the
  `dag-orphan-pull-retry-storm-without-per-orphan-backoff` operator skill
  notes and `crates/gsx-node/src/daemon.rs::run_sync_sweeper`.
