# Feature-parity matrix — suwappu-dag vs. the payments/stablecoin-chain field

**Date:** 2026-07-03
**Companion to:** [`competitive-gap-analysis.md`](competitive-gap-analysis.md) (strategy) ·
[`briefs/`](briefs/) (sourced competitor facts) ·
[`compliance-regime-mapping.md`](compliance-regime-mapping.md) ·
IQ-007…IQ-010 (the decisions each gap needs)

This is the granular, feature-by-feature companion to the gap analysis:
for every capability dimension, where Tempo (Stripe/Paradigm), Arc
(Circle), Robinhood Chain, and the best-in-class *other* stablecoin
chain stand, where suwappu-dag stands **today** (grounded in this repo,
not aspiration), a **parity verdict**, and the **specific move** to reach
parity-or-better — pointed at the IQ / issue / workstream that carries it.

Competitor facts are cited in the [briefs](briefs/); read those for
sources. Suwappu-dag cells are grounded in the code as of this branch.

## Verdict legend

- **AHEAD** — suwappu-dag already does this better, or it is structurally
  ours to win.
- **PARITY** — equivalent capability exists (may be less mature).
- **BEHIND** — competitors ship this; we do not (yet).
- **BEHIND (by design)** — a gap we should *choose* not to close, because
  it conflicts with our thesis (e.g. classical-crypto dev surfaces) or
  our segment (retail RWA). Closing it would cost more than it returns.
- **N/A** — different product; not a fair axis.

A blunt honesty rule carried from the gap analysis: competitor
performance numbers are largely self-reported, and "mainnet-live" beats
"devnet" regardless of consensus elegance. We do not paper over that.

---

## A. Consensus, safety & performance

| # | Feature | Tempo | Arc | Robinhood Chain | Best other | suwappu-dag today | Verdict | Move to parity-or-better |
|---|---|---|---|---|---|---|---|---|
| A1 | Consensus | Simplex BFT (Commonware) | Malachite BFT (Tendermint-family) | Centralized sequencer → Ethereum | Plasma PlasmaBFT / Solana PoH+TowerBFT | Mysticeti-C certificate-DAG, **dual-ring joint quorum** | **AHEAD** (safety model) | Hold. The dual-ring AND-gate (Theorem 2) is a stronger, more auditable claim than any single permissioned BFT set or single sequencer. Keep it front-and-centre. |
| A2 | Safety model | Single permissioned validator set | Single PoA→PoS set | Single Robinhood sequencer (AWS, multisig) | Single set | Fork requires Byzantine capture of **both** a 40-slot Authority Ring **and** a 200-slot Validator Ring | **AHEAD** | Hold + prove: publish the Theorem-2 argument as an auditor-facing note; it is the moat competitors cannot copy without re-architecting. |
| A3 | Finality type | Deterministic (~0.5–0.6 s) | Deterministic (~350 ms) | ~100 ms soft, L1 settle later | Deterministic sub-second | Fast-path sub-second (design goal); DAG-commit deterministic | **PARITY (unproven)** | **Publish a measured number.** PERF-2 built the fast-path latency harness (`suwappu-metrics --mode fastpath`); run it multi-region and report p50/p95. Until then this is a claim, not a result. |
| A4 | Throughput | ~20k TPS benchmarked; 100k+ marketed | ~3k TPS demonstrated (20 val.) | Not disclosed | Solana real payments volume | 100 TPS submission demonstrated; committed-TPS run pending | **BEHIND (unproven)** | PERF-1 fixed the round-driver stall that zeroed committed TPS (CI-green). Next: re-run the 4-region campaign with **committed TPS** as the headline (⛔ needs AWS). Compete on *honest* numbers — the field's 100k-TPS "targets" are a credibility opening. |
| A5 | Liveness under region loss | BFT-standard | BFT-standard | Sequencer SPOF | varies | Frontier-snap + density-clamp round driver (PERF-1) fixes the 2-of-4-region stall from the 2026-05-13 run | **PARITY** | Land the published perf run to confirm the fix at scale; file the 10k-case liveness proptest (issue #17). |

## B. Fees & transaction UX

| # | Feature | Tempo | Arc | Robinhood Chain | Best other | suwappu-dag today | Verdict | Move to parity-or-better |
|---|---|---|---|---|---|---|---|---|
| B1 | Stablecoin / no-volatile gas | No native token; stablecoin fees + fee AMM | USDC is gas | ETH gas (90-day subsidy) | Plasma paymaster (zero-fee USDT); Stable USDT gas | **SUWAPPU only; no fee market** | **BEHIND** (table stake #1) | **IQ-007 (FEE-1)** recommends fee-payer/sender separation in the Intent envelope + a sponsorship signature (Tempo-style), phased with stablecoin-denominated fees via the registered-issuer path. Spec only — needs sign-off + build. |
| B2 | Fee sponsorship / paymaster | Native (second-signature fee payer) | Circle Paymaster | Gas subsidy (temporary) | Plasma protocol paymaster | None | **BEHIND** | Same IQ-007 — the sponsorship-signature design is the direct analog; land it before incentivized testnet (Phase 5). |
| B3 | Fee predictability | Deterministic fee schedule | EWMA base-fee + ceiling | FCFS, low MEV | Fixed/low | No fee market to be un/predictable | **N/A → BEHIND once B1 exists** | Fold a predictable-fee policy into the FEE-1 build; cite Arc's EWMA and Tempo's fixed schedule as the two proven patterns. |

## C. Stablecoins, FX & real-world assets

| # | Feature | Tempo | Arc | Robinhood Chain | Best other | suwappu-dag today | Verdict | Move to parity-or-better |
|---|---|---|---|---|---|---|---|---|
| C1 | Native stablecoin issuance | TIP-20; OUSD consortium (140+) | Native USDC/EURC/USYC | USDG lending (Morpho) | Codex native USDC mint/burn | **Registered-issuer + reserve-coverage precompiles; no live issuer** | **BEHIND** (primitives, no product) | Primitives are real and arguably better-specified (reserve-coverage circuit breaker). Gap G-4: land **one** named issuer (regional EMT / tokenized-deposit pilot) on the precompile, even on testnet. Business-led, engineering-ready. |
| C2 | FX / multi-currency PvP | Fee AMM + non-USD stablecoins | StableFX RFQ, 24/7 PvP | EUR/USD perps | — | None | **BEHIND (by design, near-term)** | Not our wedge. Defer; the registered-issuer + LTP settlement primitives could support PvP later, but this chases Arc/Tempo's retail-FX segment. Revisit post-issuer. |
| C3 | Tokenized RWA / securities | (payments focus) | Tokenized funds/collateral (BlackRock etc.) | **Stock tokens (flagship)** | Canton DTCC Treasuries | None | **N/A (different segment)** | Robinhood's whole product; not ours. Our RWA-adjacent play is *settlement* of tokenized assets (Canton/Kinexys adjacency), not issuing stock tokens. Keep out. |
| C4 | Yield / money-market | (via OUSD reserve share) | USYC native | Morpho Earn ~7% | Stable USDT yield | None | **N/A** | Out of segment. |

## D. Privacy & compliance

| # | Feature | Tempo | Arc | Robinhood Chain | Best other | suwappu-dag today | Verdict | Move to parity-or-better |
|---|---|---|---|---|---|---|---|---|
| D1 | Confidential-but-auditable transfers | Opt-in confidential balances | TEE confidential + Arc Privacy | None notable | Solana Token-2022 confidential balances; Sui privacy-by-default | **Track H confidential-balance L2 (in flight); not shipped** | **BEHIND (closable, structurally ahead)** | **CONF-1**: Track H needs Phase 2 (ML-KEM-768 viewing keys + AEAD) — unbuilt, crypto-reviewer-gated. Our zk+PQ approach is architecturally stronger than Arc's TEE (no trusted-hardware assumption) and PQ-confidential unlike Solana's classical ZK — **shipping any working demo converts that into a claim.** |
| D2 | Compliance hooks (allow/deny, freeze, policy) | TIP-403 Policy Registry | Selective disclosure | TRM/Chainalysis | Token-2022 extensions; Elliptic on Codex | DID resolver, policy-vocabulary, registered-issuer, reserve-coverage precompiles | **PARITY** | COMP-1 maps these to GENIUS/MiCA. Coverage is real; keep the precompiles honest (they're primitives, not a certified program). |
| D3 | Travel rule (originator/beneficiary VASP data) | (memo fields) | (selective disclosure) | (off-chain) | Notabene/TRISA integrations | **Missing** (per COMP-1) | **BEHIND** | COMP-1 identifies this as the prime missing hook. Add a travel-rule attachment to the transfer Intent or an off-chain TRISA-style messaging path — file as an IQ (touches the Intent surface). |
| D4 | Reversibility / dispute protocol | — | Explored (controversial) | — | — | None (deliberate) | **AHEAD (by conviction)** | Do **not** build. Arc took reputational damage for even exploring reversible settlement. Immutable settlement is a feature for our audience; say so. |

## E. Developer surface

| # | Feature | Tempo | Arc | Robinhood Chain | Best other | suwappu-dag today | Verdict | Move to parity-or-better |
|---|---|---|---|---|---|---|---|---|
| E1 | EVM tooling (MetaMask/Foundry) | Full (Reth) | Full | Full (Nitro) | ~all EVM | **Dual-VM projection is read-only; no `eth_*` RPC** | **BEHIND (by design)** | **IQ-008 (EVM-1)** recommends **Intent-SDK-only**, because a real `eth_sendRawTransaction` reintroduces ECDSA secp256k1 on the primary write path — the exact classical primitive Invariant 2 minimizes. Optionally expose read-only `eth_call`/`eth_getBalance` over the projection (Option C) for integrators, no signing surface. This is a *positioning choice*, not a deficiency — own it. |
| E2 | SDKs | EVM toolchain | EVM + thirdweb | EVM + Arbitrum | EVM everywhere | Rust + TS SDKs (`suwappu_*` RPC) | **PARITY (narrow)** | Fine for the Intent model. Keep the SDKs first-class; document the "why not EVM" trade-off publicly (IQ-008). |
| E3 | Explorer / status / faucet | explore.tempo.xyz | testnet explorer + faucet | testnet explorer + faucet | all have | **All built** (`clients/explorer`, `clients/status-page`, `suwappu-faucet`) — deploy-gated | **PARITY (code) / BEHIND (live)** | LAUNCH-1: software + terraform are apply-ready; only the AWS `scripts/deploy-aws.sh` apply remains (⛔ credential-gated). No engineering left. |

## F. Cross-chain & cryptography

| # | Feature | Tempo | Arc | Robinhood Chain | Best other | suwappu-dag today | Verdict | Move to parity-or-better |
|---|---|---|---|---|---|---|---|---|
| F1 | Cross-chain asset mobility | (stablecoin rails) | **CCTP v2 + Gateway (native)** | LayerZero + 0x | CCTP v2 (13+ chains); LayerZero OFT ($70B+) | LTP attestation only; **quorum header → mint not wired** | **BEHIND** | **IQ-010 (INTEROP-1)**: position LTP as the *settlement-attestation* layer (not a bridge) and add a thin OFT-class adapter for asset mobility. Depends on **BRIDGE-1** wiring `submitHeader` → oracle → mint end-to-end first. |
| F2 | Settlement attestation | (none comparable) | Gateway unified balances | — | Canton sub-tx privacy | **Constant-size ≈1,600 B LTP attestation regardless of payload** (Invariant 3) | **AHEAD** | Unique cost-scaling property. Keep it; it's the natural pitch to the Canton/Kinexys settlement audience. |
| F3 | **Post-quantum crypto (primary surfaces)** | None | Opt-in PQ *wallets* at mainnet (future) | None | Ethereum leanXMSS (roadmap); QRL (no payments relevance) | **ML-DSA-65 signing + ML-KEM-768 confidentiality on primary surfaces; ML-DSA bridge attestation PQ end-to-end** | **AHEAD** (only one in category) | The single biggest differentiator. **Defend it:** close the one soft spot — the classical BLS12-381 LTP aggregate — via **IQ-009 (PQ-1)** with a published migration target + timeline. CNSA 2.0 (Jan 2027) is the institutional hook. |
| F4 | Accountability / slashing | Unpublished | Unpublished/absent | Sequencer-operator model | varies | 100% fast-path equivocation slashing, dual bonds, waterfall — **in code, property-tested** | **AHEAD (with caveat)** | Real and tested, but the fast-path `slashed` path is **observability-only** today (no stake moves) — issue #16 gates promoting it to real forfeiture (needs per-signer cert signatures + post-commit signer retention). Don't overclaim live enforcement until #16 lands. |

## G. Ecosystem, distribution & status

| # | Feature | Tempo | Arc | Robinhood Chain | Best other | suwappu-dag today | Verdict | Move to parity-or-better |
|---|---|---|---|---|---|---|---|---|
| G1 | Mainnet status | **Live** (2026-03-18) | Testnet; mainnet summer 2026 | **Live** (2026-07-01) | Plasma/Stable/Codex live | Public devnet (Phase 4); mainnet candidate Q4 2026 | **BEHIND** | Sequence the P0/P1 proof points → Phase 5 incentivized testnet → mainnet candidate. Don't rush mainnet ahead of the audits (Phase 6 gate). |
| G2 | Live token | None (deliberate) | ARC ($222M presale, $3B FDV) | HOOD (equity) | XPL/STABLE/etc. | None (devnet SUWAPPU) | **N/A** | Tokenomics are specced (Phases 2–3 shipped); TGE is a mainnet-gate decision, not a feature race. |
| G3 | Named launch partners / distribution | Visa, Stripe, MoneyGram validators; OUSD | 100+ institutions; Circle stack | 26M+ accounts, 120+ countries | Tether wallet (Plasma) | **None** | **BEHIND (business, not engineering)** | The clearest "distribution beats technology" lesson. **GTM-1** built the outreach kit; engineering unblocks it via the CNSA 2.0 hook + a live PQ bridge demo (BRIDGE-1). Execution is ⛔ human-run. |
| G4 | Fiat on/off ramps | (Stripe) | Circle Mint | (Robinhood app) | Coinbase/Nium/BVNK | None | **BEHIND (by segment)** | Ramps follow an issuer/partner (G1/C1), not a chain feature. Defer to the issuer story. |

---

## Scorecard

Counting only axes where a fair comparison exists (excluding N/A):

- **AHEAD (defend):** dual-ring joint-quorum safety (A1/A2), constant-size
  LTP attestation (F2), **PQ on primary surfaces (F3)**, slashing
  economics (F4, caveated), immutable settlement (D4). These five are the
  moat. Four of them no competitor in the category has at all.
- **PARITY (mature/prove):** finality type (A3, unproven), liveness (A5),
  compliance hooks (D2), SDKs (E2), explorer/status/faucet (E3, deploy-gated).
- **BEHIND (closable, prioritized):**
  1. **Stablecoin/sponsored fees** (B1/B2) — table stake, blocks a
     credible public launch → IQ-007 (FEE-1).
  2. **Demonstrated committed throughput** (A4) — credibility → PERF-1
     landed the fix; publish the perf run.
  3. **Confidential transfers** (D1) — table stake, but we can leapfrog
     (PQ+zk vs. TEE) → CONF-1 (Track H Phase 2).
  4. **Cross-chain asset mobility** (F1) → IQ-010 depends on BRIDGE-1.
  5. **Travel-rule hook** (D3) → COMP-1 follow-up IQ.
  6. **A live issuer / stablecoin** (C1) and **named partners** (G3) —
     business-led, engineering-ready.
- **BEHIND (by design — don't chase):** EVM signing surface (E1),
  retail FX (C2), stock-token issuance (C3), reversibility (D4).

## The honest one-paragraph read

Suwappu-dag is **ahead where the category can't easily follow** (PQ-by-default,
dual-ring safety, constant-size attestation, real slashing) and **behind on
the table stakes any funded chain ships day one** (stablecoin/gasless fees,
demonstrated throughput, confidential transfers, a live issuer, named
partners). The parity-or-better path is therefore *not* to become another
EVM payments chain — Tempo/Arc/Solana own that and we'd be permanently
behind on distribution — but to **close the four closable table stakes
(fees, perf number, confidential demo, travel-rule) while widening the four
moats**, and sell into the institutional/regulated-settlement segment
(Canton/Kinexys adjacency) where PQ + auditability + safety decide wins.
Every "move" cell above is already carried by a filed IQ (007–010), issue
(#16, #17), or P0/P1 workstream — this matrix is the coverage map, not new
work to invent.
