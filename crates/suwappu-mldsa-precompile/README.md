# suwappu-mldsa-precompile

On-chain **ML-DSA-65 (FIPS 204)** signature-verification precompile core for the
Suwappu bridge — the load-bearing piece of the on-chain post-quantum verification
program (audit P5b, Phase 1).

`verify(pubkey || signature || message) -> 32-byte EVM word (1 = valid, 0 = not)`.

It wraps the **same NIST PQC reference verifier** (`pqcrypto-mldsa`, ML-DSA-65)
that `suwappu-crypto` already uses for validator/consensus signatures, so it is
genuinely FIPS-204 post-quantum sound: **no SNARK wrapper, no scheme
substitution.** (Contrast the SP1→Groth16/BN254 path, which is Shor-broken — see
`suwappu-lattice-protocol/docs/security/audits/suwappu/P5b_ONCHAIN_PQ.md`.)

## Status (2026-06-07)

- ✅ **Core verifier + I/O encoding: implemented and LOCALLY VERIFIED.**
  `cargo test -p suwappu-mldsa-precompile` → 8/8 green against real ML-DSA-65 keys:
  FIPS-204 sizes (pk=1952, sig=3309), valid accepted, tampered msg/sig/wrong-key
  rejected, truncated/empty inputs handled with no panic.
- ⏭️ **EVM registration glue (CI-verified next step).** Register as a Monad
  precompile in `suwappu-revm` at a fixed address (e.g. `0x0101`), following the
  `Precompile::new(PrecompileId::custom("MLDSA65_VERIFY"), addr, run_fn)` pattern
  in `suwappu-revm/crates/suwappu-revm/src/precompiles.rs`. The `run_fn` is a thin
  adapter: gas check → `suwappu_mldsa_precompile::verify(input)` → `PrecompileOutput`.
  Gas: set ~8–12k (cf. EIP-8051's 4,500 for ML-DSA-44). This step needs a cold
  `suwappu-revm` (revm v34) build, run via CI — not on the local Mac (workspace-build
  rule in the workspace build rules).
- ⏭️ **End-to-end wiring blocked on EVM integration.** suwappu-dag main does not yet
  enable the `production-evm-executor` feature (behind PRs #25-31). Until then,
  this crate also serves the **intent-handler path**: call `verify()` directly
  from the execution substrate so mint/unlock/finalize require an on-chain
  ML-DSA-65 check without waiting for the EVM precompile surface.

## Binding to the bridged commit

Callers MUST pass `message = canonical_commit_encoding` (chainId, commitId,
amount, recipient, …) and a `pubkey` pinned to the validator/committee key in
consensus. That binding is what closes LTP-A-001 (and the C1/C2/C3 relayer-trust
criticals in suwappu-lattice-protocol): a relayer can no longer assert a commit the
signature does not actually cover.
