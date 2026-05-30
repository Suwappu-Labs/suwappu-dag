# Execution — co-resident dual VM over polymorphic balance map

**Paper §**: 7 — Execution substrate ([`gsx-papers/papers/dag-l1`](https://github.com/GlobalSettlementNetwork/gsx-papers))
**Code**: `crates/gsx-execution/src/` (block executor, checkpoint) · workspace dep `gsx-db` for the substrate primitives
**IQs**: — *(execution-substrate IQs live in the `gsx-db` repo; see [gsx-db-bridge.md](gsx-db-bridge.md))*
**Visuals**: [`docs/visuals/mermaid/dual-vm.md`](../visuals/mermaid/dual-vm.md) *(coming with PR-2)*
**Sprint**: DAG-S10 (executor adapter) ✅ Closed · DAG-S11 (checkpoint co-signature) ✅ Closed

## What it does

`gsx-execution` is the seam between the consensus layer (DAG of certs) and
the state substrate (`gsx-db`). The block executor adapter consumes a
committed `Block` payload, threads each `Intent` through the gsx-db
`BlockExecutor`/`BundleExecutor`, and updates the polymorphic balance map
(`BalanceSlot`) atomically — same code path serves both VMs (EVM and Move).
A read-only "projector" surface lets EVM `balanceOf` and Move `Coin::value`
read the same canonical state without going through either VM's mutation
path.

## Key invariants

- **Block executor adapter parity (S10 exit gate):** `proptest_block_execution.rs`
  × 10,000 cases — a sequence of committed `Block`s produces the same
  resulting `BalanceStore` as the gsx-db `BlockExecutor`'s own test harness.
- **Checkpoint joint co-signature (S11 exit gate):** `proptest_checkpoint.rs`
  × 10,000 cases — a checkpoint is finalized iff both rings co-sign;
  drift between rings produces no valid checkpoint.
- **Substrate invariants inherited bit-for-bit (Invariant 4 in GSXHELPER.md):**
  lane separation, dual-VM projection equality, schedule determinism,
  bundle atomicity, tree determinism, cross-chain parity, replay
  equivalence — all enforced inside `gsx-db`, threaded through unchanged by
  the executor adapter.

## Cross-references

- **Engineering:** `crates/gsx-execution/src/lib.rs::execute_block` and the
  `crates/gsx-node/src/daemon.rs::try_commit` site that calls into it
  (`execute_block(&mut inner.substrate, &block)`).
- **Spec:** Paper §7, plus the gsx-db paper-additions §7.4 (in
  `gsx-db/docs/paper-additions/`).
- **Substrate boundary:** see [gsx-db-bridge.md](gsx-db-bridge.md) for the
  workspace-dependency seam.
- **Design decisions:** the dual-VM-projector decision binding lives in
  gsx-db's `IQ-2` (mock vs real VMs) and `IQ-3` (Move VM choice → Aptos).
- **Visual:** [dual-vm](../visuals/mermaid/dual-vm.md) *(coming with PR-2)*.
