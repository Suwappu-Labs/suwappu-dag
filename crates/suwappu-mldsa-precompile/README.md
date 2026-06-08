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
  `cargo test -p suwappu-mldsa-precompile` → 16/16 green against real ML-DSA-65 keys:
  FIPS-204 sizes (pk=1952, sig=3309), valid accepted, tampered msg/sig/wrong-key
  rejected, truncated/empty inputs handled with no panic.
- ✅ **Commit binding + distinct-signer quorum (`MintCommit` +
  `verify_mint_authorization` + `verify_mint_quorum`).** Canonical, domain-separated
  (`SUWAPPU-MINT-COMMIT-V1`, `u32`-BE length-prefixed) encoding of
  `(source_chain, target_chain, commit_id, amount, recipient)`, so callers can't
  drift. `verify_mint_quorum` enforces a k-of-n threshold on distinct signer
  *indices* (never signature bytes — ML-DSA is randomized, so byte-counting would
  let one key satisfy k-of-n alone). 19/19 revert-fails tests, incl. the
  single-signer-collapse attack. See `docs/iq/IQ-009-*` for the joint-ring design
  this composes into (two review rounds, all findings resolved).
- ⏭️ **EVM registration glue (CI-verified next step).** Register as a Monad
  precompile in `suwappu-revm` at a fixed address (e.g. `0x0101`), following the
  `Precompile::new(PrecompileId::custom("MLDSA65_VERIFY"), addr, run_fn)` pattern
  in `suwappu-revm/crates/suwappu-revm/src/precompiles.rs`. The `run_fn` is a thin
  adapter: gas check → `suwappu_mldsa_precompile::verify(input)` → `PrecompileOutput`.
  Gas: set ~8–12k (cf. EIP-8051's 4,500 for ML-DSA-44). This step needs a cold
  `suwappu-revm` (revm v34) build, run via CI — not on the local Mac (workspace-build
  rule in the workspace build rules).
- ⏭️ **End-to-end wiring needs an IQ — not a free local edit.** The value-movement
  authorization today is the LTP corridor attestation (`suwappu-ltp::verify_attestation`):
  a **7-of-9 BLS12-381 aggregate** (classical, Shor-breakable) consumed by the
  execution substrate on `L1Lock`/`L2BurnProven`. Gating those paths on this crate's
  PQ check is the goal — but the BLS aggregate is, by **load-bearing invariant 3**
  (constant-size ~1,600 B LTP commitment, paper §10.2), deliberately part of the
  on-chain commitment, and ML-DSA-65 (3,309 B) cannot be added to that surface.
  The PQ signature must therefore be carried as a **transient execution-time
  authorization witness on the Intent** (verified by `verify_mint_authorization`
  then discarded, *not* persisted into the commitment). That changes the substrate
  authorization model, so per `SUWAPPUHELPER.md` it requires an **IQ + crypto-reviewer
  + consensus-reviewer** before it ships. This crate is the ready primitive; the
  substrate edit is gated on that decision.
- ⏭️ **EVM precompile path** (the `0x0101` registration above) is separately blocked
  on the `production-evm-executor` feature (PRs #25-31) and best done by **inlining**
  the verify into `suwappu-revm` to avoid a circular gsx-revm↔gsx-dag dependency.

## Binding to the bridged commit

Callers MUST pass `message = canonical_commit_encoding` (chainId, commitId,
amount, recipient, …) and a `pubkey` pinned to the validator/committee key in
consensus. That binding is what closes LTP-A-001 (and the C1/C2/C3 relayer-trust
criticals in suwappu-lattice-protocol): a relayer can no longer assert a commit the
signature does not actually cover.
