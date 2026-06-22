# Application — precompiles (DID, registered-issuer, reserve-coverage)

**Paper §**: 8 — Application layer ([`suwappu-papers/papers/dag-l1`](https://github.com/Suwappu-Labs/suwappu-papers))
**Code**: `crates/suwappu-precompiles/src/`
**IQs**: —
**Visuals**: covered in [`docs/visuals/suwappu-dag.html`](../visuals/suwappu-dag.html) (right panel "Applications")
**Sprint**: DAG-S12 (DID resolver) ✅ Closed · DAG-S13 (issuer mint/burn) ✅ Closed · DAG-S14 (reserve-coverage breaker) ✅ Closed

## What it does

The application layer is a set of precompiles callable from either VM,
exposing identity (DID), regulated issuance, and a reserve-coverage circuit
breaker. Each precompile is a deterministic, post-quantum-correct gate
between user-level intents and the canonical balance map.

- **DID resolver (S12):** `did:suwappu:<id>` → ML-DSA-65 + ML-KEM-768 keys,
  with rotation proofs.
- **Registered-issuer mint/burn (S13):** an Authority-Ring-seated issuer
  can mint or burn registered assets; mint without coverage is rejected by
  the next precompile.
- **Reserve-coverage circuit breaker (S14):** a PlonK predicate enforces
  *total issued ≤ proven reserves* at every mint; below a configurable
  margin the precompile rejects further mints until coverage proofs catch up.

## Key invariants

- **DID rotation correctness (S12 exit gate):** `proptest_did.rs` × 10,000
  cases — a key rotation produces a chain of verifiable proofs that the
  resolver accepts and rejects forgeries.
- **Issuer mint/burn determinism (S13 exit gate):** `proptest_issuer.rs` ×
  10,000 cases — concurrent mint + burn produce the same final balance map
  regardless of intent ordering within a block.
- **Reserve-coverage predicate (S14 exit gate):** `proptest_reserve.rs` ×
  10,000 cases — any mint over the proven-reserve total is rejected.

## Cross-references

- **Engineering:** `crates/suwappu-precompiles/src/did.rs`, `.../issuer.rs`,
  `.../reserve.rs` (one module per S12/S13/S14).
- **Spec:** Paper §8.
- **Design decisions:** none ratified; pre-compile choices were spec'd.
- **Visual:** the right panel of [suwappu-dag.html](../visuals/suwappu-dag.html)
  lists the precompile set; a dedicated diagram is not yet drawn.
- **Issuance studio + compliance hooks:** Phase-G governance machinery
  uses these precompiles — see [governance-phasing.md](governance-phasing.md).
