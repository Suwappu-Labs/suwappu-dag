# L2 proof system — selection rationale

**Status**: Soft-locked. SP1 Groth16 BN254 is the chosen proof
system for the v1 L2; revisit only on a strong-case event (see
"Re-evaluation triggers" below).

**Phase**: Track G G2.2 phase 2 (issue #97) — the verifier
precompile now invokes the real `sp1-verifier` BN254 pairing
check; the upcoming `gsx-l2-stm` crate (Phase 1.1 per
`~/.claude/plans/validated-prancing-curry.md`) is an SP1 guest
program.

**Authors**: This doc captures the half-day sanity-check the L2
plan called for before kicking off Phase 1.1. Skim it before
proposing any change to the proof-system surface.

---

## Context

Across four crates and IQ-006, the codebase has been assuming
**SP1 Groth16 BN254** as the L2 proof system:

- `crates/gsx-l2-verifier-precompile/src/lib.rs` — fixed 260 B
  proof + 240 B fixed-offset public-input layout (Filecoin /
  op-succinct shape)
- `crates/gsx-l2-bridge/src/lib.rs` — L1Lock / L2BurnProven
  payload validation aligned to the same public-input layout
- `crates/gsx-l2-sequencer/src/lib.rs` — `BatchHeader`'s
  240-byte public-input format gates on the verifier's offsets
- `crates/gsx-l2-confidential/src/lib.rs` — Track H nullifier
  commitments designed to fold into the same proof's
  `confidential_root` field
- `docs/iq/IQ-006-l2-state-root-commitment-surface.md` —
  multi-L2 VK keying assumes the SP1 verifying-key shape

The four open candidates worth comparing before committing:
**SP1**, **Risc0**, **Plonky3**, **Nexus**.

---

## Evaluation matrix

| | SP1 (Succinct) | Risc0 | Plonky3 | Nexus |
|---|---|---|---|---|
| **Guest language** | Rust → RISC-V | Rust → RISC-V | Pure Rust circuit | Rust → RISC-V |
| **Native proof system** | STARK + Groth16 wrapper | STARK + Groth16 wrapper | STARK / Plonky3 | Nova IVC |
| **Groth16 BN254 wrap** | Yes (260 B, EVM-precompile-friendly) | Yes | No (would need bespoke wrap) | Roadmap |
| **On-chain verifier size** | ~5 KB (sp1-verifier crate) | ~5 KB | ~50 KB (Plonky3 STARK verifier) | N/A (no production EVM verifier) |
| **Proving network** | Succinct Prover Network (managed) | Bonsai (managed) | Self-hosted only | Self-hosted only |
| **Prover cost @ batch size 500 tx** | ~$1–3 / batch (Succinct pricing 2025) | ~$3–6 / batch (Bonsai) | Compute-only, ~$0.50 (self-hosted GPU) | N/A |
| **Verifier latency on L1** | 2–5 ms (single-core BN254 pairing) | 2–5 ms (similar) | 50–200 ms (STARK verify) | N/A |
| **Audit footprint** | Audited by Trail of Bits + Cantina (2024-25) | Audited (multiple) | Audited (Polygon zkEVM uses Plonky2/3) | Pre-audit |
| **Production users** | OP-Succinct, Mantle, Polygon AggLayer, several appchains | Bonfida, EigenLayer AVS, several appchains | Polygon zkEVM type 1 prover | None at L1 scale yet |
| **Rust toolchain ergonomics** | `cargo prove` CLI; smooth no-std support | `cargo risczero` CLI; smooth | Library-style API; circuit DSL learning curve | CLI is research-grade |
| **Time-to-first-proof for a new STM** | ~2 weeks (Track G G1) | ~2 weeks (similar) | ~4–6 weeks (DSL ramp-up) | ~6–8 weeks (research-grade) |
| **Network maturity** | Public Prover Network, free tier + paid | Bonsai is alpha-stable | N/A (no managed service) | Research, no managed service |
| **License** | Apache-2.0 + MIT | Apache-2.0 | MIT | Apache-2.0 |

---

## Why SP1 wins for v1

1. **Verifier already wired.** `gsx-l2-verifier-precompile`
   landed in G2.2 phase 1 with SP1's public-input layout baked
   in. Switching costs a complete rewrite of that crate + the
   sequencer's batch header + the IQ-006 decision record.

2. **Groth16 BN254 wrap is a first-class output.** The 260 B
   proof + 240 B public inputs format is what every EVM-side
   L2 verifier in production uses (op-succinct, Mantle,
   AggLayer). Verifier latency stays in the 2–5 ms range,
   which means an L2 batch commit doesn't blow the L1 round
   budget.

3. **Managed prover network exists.** Succinct's Prover
   Network is GA and priced at ~$1–3 per batch at 500 tx, with
   a free tier for testnet. We can defer GPU procurement
   indefinitely; Phase 2.1's prover daemon talks to the network
   over HTTP. Risc0's Bonsai is similar but more expensive;
   Plonky3 and Nexus have no equivalent.

4. **Confidential payload integration path is clear.** Track H
   commitments (`gsx-l2-confidential`) bind into the proof's
   `confidential_root` public-input field. SP1's Rust guest
   model supports this trivially — the STM program imports
   `gsx-l2-confidential::commit` and the resulting bytes flow
   into the proof's public inputs. Plonky3's circuit DSL would
   require a dedicated commit gadget.

5. **Time-to-mainnet matters.** A 2-week STM circuit (SP1) vs
   4–6 weeks (Plonky3) compresses the critical path. Plonky3
   is the right call if (and only if) we want to drop the
   Groth16 wrapper for direct STARK verification on L1 — a
   ~10× verifier-cost win at the price of L1 verifier-code
   complexity. That's a v2 conversation.

---

## What we give up by picking SP1

- **Single-vendor risk.** Succinct's Prover Network is the
  only managed service. If they raise prices or change SLA
  unilaterally, our cost model moves. Mitigation: keep the
  prover daemon's "submit to network OR self-host" toggle
  (Phase 2.1) so we can fall back to in-house GPU if needed.

- **Proof cost at scale.** ~$1–3 per batch × ~150 batches/day
  (250 ms × 500 tx batches running 24/7 nets us ~150 commits
  if we batch every ~10 min) = ~$5k/month at sustained
  testnet load. Acceptable for testnet; mainnet may justify
  self-hosted GPU at higher TPS.

- **No on-L1 STARK verification.** We're committed to the
  Groth16 wrapper. If a future security finding compromises
  BN254 (e.g., parameter-collision attack), we'd need to
  rotate the proof system entirely — not just rotate VK pairs.
  Counter: BN254 is widely deployed (every EVM-side L2) and
  the cryptanalysis landscape is well-understood.

---

## Re-evaluation triggers

Soft-lock means we revisit SP1 only on one of these events:

1. **Succinct Prover Network outage or pricing change** that
   makes the cost model untenable. Mitigation already in
   plan: self-hosted GPU path.

2. **BN254 cryptographic finding** (e.g. a meaningful
   subgroup-attack discount). Mainnet audit (Phase 5.1)
   would surface this; if it lands during Phase 1–3, we
   pause and re-evaluate.

3. **Plonky3-style direct-STARK-verify becomes table stakes**
   in the L2 ecosystem (e.g., every major L2 ships with
   STARK-on-L1). v2 conversation; doesn't change v1.

4. **Risc0 / Nexus ship a production-grade managed prover**
   with materially better price/performance or audit posture
   AND we have headroom to migrate.

Otherwise, SP1 is the chosen system; rebuild discussions
should cite this doc and the trigger above.

---

## Implementation references

- Verifier: `crates/gsx-l2-verifier-precompile/src/lib.rs` —
  `verify_l2_batch` calls `sp1_verifier::Groth16Verifier::verify`
  with the chain-state-pinned `aggregation_vk_hash`.
- VK pinning: `crates/gsx-execution/src/l2_state.rs` —
  multi-chain `ChainVks` map keyed by `l2_chain_id_hash`.
- STM circuit: `crates/gsx-l2-stm/` (Phase 1.1, forthcoming) —
  SP1 guest program proving `(prev_l2_state_root,
  batch_da_commitment, prev_l1_state_root, l2_chain_id_hash) →
  (new_l2_state_root, range_vk_commitment, confidential_root)`.
- Prover daemon: `crates/gsx-l2-prover/` (Phase 2.1,
  forthcoming) — talks to Succinct Prover Network by default,
  with `--prover=local` fallback.

---

## See also

- `docs/iq/IQ-006-l2-state-root-commitment-surface.md` — the
  registry-encoding decision record this proof system feeds.
- `~/.claude/plans/validated-prancing-curry.md` — the
  mainnet-ready L2 plan that sets the timeline this doc
  unblocks.
- `crates/gsx-l2-verifier-precompile/src/lib.rs` lines 1–40 —
  the in-tree explainer that points the reader here.
