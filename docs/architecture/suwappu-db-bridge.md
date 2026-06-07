# suwappu-db bridge — workspace dependency boundary

**Paper §**: 7 (substrate handoff) + suwappu-db paper-addition §7.4
**Code**: `crates/suwappu-execution/` (consumer) · `Cargo.toml` workspace deps
**IQs**: see suwappu-db's `docs/iq/IQ-2-mock-vms-vs-real-vms.md`, `IQ-3-move-vm-choice.md`, `IQ-6-verkle-commitment.md` for substrate-side decisions
**Visuals**: [`docs/visuals/suwappu-db.html`](../visuals/suwappu-db.html) · [`docs/visuals/mermaid/suwappu-db.md`](../visuals/mermaid/suwappu-db.md)
**Sprint**: DAG-S10 ✅ Closed (executor adapter)

## What it does

`suwappu-db` is the canonical state substrate for the chain — polymorphic
balance map, dual-VM projectors, OCC scheduler, state tree, anchor
pipeline, recovery replay. `suwappu-dag` consumes it as a workspace
dependency rather than embedding it, so the substrate can be developed,
audited, and benchmarked independently while keeping its invariants
intact across the integration boundary.

The integration surface is narrow:

- `suwappu-execution` calls `gsxdb_bridge::Bridge::submit` for intent
  application and `BlockExecutor::execute_block` for committed-block
  application.
- `suwappu-precompiles` reads state via the dual-VM projectors (EVM
  `balanceOf`, Move `Coin::value`).
- `suwappu-ltp::anchor` hands LTP attestations off to suwappu-db's
  `AnchorDispatcher`.

## Key invariants

The substrate enforces (and suwappu-dag inherits):

1. **Lane separation** — `gsxdb-lane` never imports into `gsxdb-state`.
2. **Dual-VM projection equality** — EVM `balanceOf` and Move
   `Coin::value` agree bit-for-bit at every commit.
3. **Schedule determinism** — same intent order → same state tree root.
4. **Bundle atomicity** — a bundle either fully commits or fully rolls
   back; no partial application.
5. **Tree determinism** — state-tree root is a pure function of state.
6. **Cross-chain parity** — anchor outputs match Solidity registry's
   view bit-for-bit.
7. **Replay equivalence** — recovery replay produces the same state as
   live commit.

SUWAPPUHELPER.md tracks these as "Invariant 4 — substrate invariants inherited
from suwappu-db". Any change in the executor adapter that would weaken one
of these must be rejected at review.

## Cross-references

- **Engineering:** `Cargo.toml` workspace deps (`gsxdb-bridge`,
  `gsxdb-state`, `gsxdb-lane`); `crates/suwappu-execution/src/lib.rs`
  exercises the integration.
- **Spec:** suwappu-db paper-additions
  [`dag-l1-section-7-4.md`](https://github.com/suwappu/suwappu-db/blob/main/docs/paper-additions/dag-l1-section-7-4.md)
  is the canonical writeup of the integration surface.
- **Substrate docs:** the full suwappu-db doc tree lives in
  [`suwappu-db/docs/`](https://github.com/suwappu/suwappu-db/tree/main/docs)
  — start at `suwappu-db/docs/README.md`.
- **Visual:** [suwappu-db.md](../visuals/mermaid/suwappu-db.md) inline-renders
  the substrate's lane → bridge → state pipeline.
