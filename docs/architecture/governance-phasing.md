# Governance phasing — Phase G2 → G3 → G4

**Paper §**: 14 — Governance ([`gsx-papers/papers/dag-l1`](https://github.com/GlobalSettlementNetwork/gsx-papers))
**Code**: `crates/gsx-node/src/daemon.rs::apply_governance_intent` (Phase G `Intent::AdmitAuthority` / `ExitAuthority` / `EjectAuthority`)
**IQs**: [IQ-004 decide_slot orphan window](../iq/IQ-004-decide-slot-orphan-window.md) (interaction with epoch-boundary application)
**Visuals**: [`docs/visuals/mermaid/governance-flow.md`](../visuals/mermaid/governance-flow.md) *(coming with PR-2)*
**Sprint**: DAG-S25 (validator governance) ✅ Closed · post-S25 hot-fixes #18 (epoch-boundary), #28 (signed intents), #32 (deferred activation)

## What it does

Governance evolves through three published phases:

- **G2 (current):** static authority-ring + manual operator coordination
  via `AdmitAuthority` / `ExitAuthority` / `EjectAuthority` intents. Intents
  are signed by an authority operator's ML-DSA-65 key (#28), queued in
  `pending_governance` at commit time (#18), and drained atomically at the
  next epoch boundary so registry mutations land cluster-wide in one
  transition. The newly admitted authority's stake table contribution is
  parked in `pending_stake` and only promoted on the authority's first
  authored cert (#32 deferred-activation) — a permanent fix for the
  stake-table denominator inflation bug captured in the
  `bft-stake-denominator-deadlock-on-admit` skill.
- **G3 (post-mainnet):** on-chain stake-weighted voting on
  proposer-introduced governance bundles; G2 intents become a strict
  subset.
- **G4 (long-run):** progressive decentralization of the Authority Ring
  via on-chain ratification of operator-set changes; super-node
  designations rotate through ring members.

## Key invariants

- **Atomic epoch-boundary application (#18):** governance intents only
  apply to the registries at the boundary round; intermediate rounds
  agree on the pre-mutation registry.
- **ML-DSA-65 signed intents (#28):** the client-side wire enforces a
  detached ML-DSA-65 signature on every governance intent; unsigned or
  forged-signature submissions are rejected before the consensus mpsc.
- **Deferred activation (#32):** `inner.n_authorities` is bumped only
  when the new authority's first cert ingests, preventing quorum
  threshold inflation during the no-cert-yet window.

## Cross-references

- **Engineering:** `crates/gsx-node/src/daemon.rs::apply_governance_intent`
  (~line 1353), `pending_governance: Vec<Intent>` (~line 131), the
  epoch-boundary drain block in `try_commit` (~line 1252), and the
  pending_stake promotion site in `ingest_cert` (~line 619).
- **Spec:** Paper §14 (G2 / G3 / G4 phases).
- **Design decisions:** IQ-004 documents the orphan-window interaction
  with governance application; a one-shot governance intent that lands
  in a single cert is exposed to the `decide_slot` skip path. Current
  test-side mitigation: client resubmit every 5s — see
  [PR #44](https://github.com/GlobalSettlementNetwork/gsx-dag/pull/44).
- **Visual:** [governance-flow](../visuals/mermaid/governance-flow.md)
  *(coming with PR-2)* — diagrams the submit → commit → queue → boundary
  drain → activate pipeline.
- **Test coverage:** `phase_g_admit_and_eject` exercises the G2 happy path
  end-to-end across 4 daemons.
