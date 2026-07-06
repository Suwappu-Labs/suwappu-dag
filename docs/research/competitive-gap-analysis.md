# Competitive gap analysis — payments & settlement chains

**Date:** 2026-07-03
**Status:** Research complete; recommendations pending prioritization
**Full source briefs:** [`briefs/tempo.md`](briefs/tempo.md) ·
[`briefs/arc.md`](briefs/arc.md) ·
[`briefs/robinhood-chain.md`](briefs/robinhood-chain.md) ·
[`briefs/landscape.md`](briefs/landscape.md)

Prepared ahead of taking suwappu-dag public: where Tempo (Stripe/Paradigm),
Arc (Circle), Robinhood Chain, and the broader payments/settlement-chain
category stand as of July 2026, where suwappu-dag stands against them, and
what it would take to close the gaps that matter. All external claims are
cited in the briefs; internal claims are grounded in this repo's
`ROADMAP.md`, `README.md`, and perf-run reports.

---

## TL;DR

1. **The field has shipped.** Tempo mainnet went live 2026-03-18 (Visa,
   Stripe, MoneyGram validators; OUSD consortium stablecoin announced
   2026-06-30). Robinhood Chain mainnet went live 2026-07-01. Arc targets
   mainnet beta this summer after ~250M testnet transactions and a $222M
   ARC presale. suwappu-dag is in Phase 4 (public devnet), targeting a
   mainnet candidate in Q4 2026.
2. **Our headline differentiator is still unclaimed by anyone else.** No
   payments/settlement chain ships NIST-standardized PQ signatures
   (ML-DSA/ML-KEM) as its *primary* signing surface. Arc's April 2026 PQ
   announcement is opt-in wallet-level only; Ethereum's PQ plan is
   roadmap-stage. CNSA 2.0's 2027-01-01 acquisition deadline gives PQ a
   concrete institutional hook — but the window to own this narrative is
   narrowing.
3. **Three category table stakes are missing or unproven here:**
   stablecoin-denominated / sponsored fees, demonstrated sub-second
   finality at meaningful committed TPS, and an EVM-standard developer
   surface. Every funded competitor has all three.
4. **Distribution beats technology** — the clearest lesson of 2025–26
   (Plasma's collapse, Hyperliquid's USDH sunset, Solana winning via
   Visa/Worldpay). A public launch without named partners, wallet
   integrations, and an issuer story lands flat regardless of consensus
   quality.
5. **Our defensible lane is the institutional/regulated-settlement
   segment** (the Canton/Kinexys audience), where PQ compliance,
   dual-ring safety, constant-size attestation, and auditability decide
   wins — not retail payments volume, where Tempo/Arc/Solana have already
   consolidated distribution.

---

## 1. Where the field is (July 2026)

### Head-to-head snapshot

| | **Tempo** | **Arc** | **Robinhood Chain** | **suwappu-dag** |
|---|---|---|---|---|
| Backer | Stripe + Paradigm ($5B val.) | Circle ($222M ARC presale, $3B FDV) | Robinhood (public co.) | Suwappu Labs |
| Status | **Mainnet live** (2026-03-18) | Public testnet; mainnet summer 2026 | **Mainnet live** (2026-07-01) | Public devnet (Phase 4) |
| Layer | L1 | L1 | L2 (Arbitrum Orbit) | L1 (+ own zk L2 track) |
| Consensus | Simplex BFT (Commonware) | Malachite BFT (Tendermint-family) | Centralized sequencer → Ethereum | Mysticeti-C cert-DAG, dual-ring joint quorum |
| Finality | ~0.5–0.6 s deterministic | ~350 ms testnet, deterministic | ~100 ms blocks (soft), L1 settle later | Fast-path sub-second (design goal); ~5 s round cadence measured on devnet |
| Throughput | ~20k TPS benchmarked; 100k+ marketed | ~3k TPS demonstrated (20 validators) | Not disclosed | 100 TPS submission demonstrated; committed-TPS run invalid (see §2) |
| Execution | EVM (Reth, no native token) | EVM | EVM (Nitro) | Custom Intent surface + dual-VM (EVM/Move) projection via suwappu-db |
| Gas | Stablecoins (fee AMM, sponsorship, no native token) | USDC (EWMA fees, Paymaster) | ETH (90-day gas subsidy) | SUWAPPU (devnet); no fee-abstraction surface yet |
| Stablecoin story | TIP-20 native issuance; pathUSD; **OUSD consortium (140+ partners)** | Native USDC/EURC/USYC; StableFX; CCTP v2 + Gateway | USDG lending (Morpho); stock tokens as flagship asset | Registered-issuer + reserve-coverage precompiles; no issuer partner |
| Privacy | Opt-in confidential transfers | TEE confidential transfers + Arc Privacy | None notable | Confidential-balance zk L2 (Track H, in flight) |
| Compliance hooks | TIP-403 policy registry, memo fields | Selective disclosure, dispute/reversibility explorations | TRM/Chainalysis integrations | DID resolver, policy-vocabulary, registered-issuer, reserve-coverage precompiles |
| PQ crypto | None | **Opt-in PQ wallets at mainnet** | None | **ML-DSA-65/ML-KEM-768 on primary surfaces** |
| Validators | Permissioned (Visa, Stripe, Zodia, MoneyGram) | Permissioned PoA → PoS | Single sequencer (AWS, multisig) | 40 Authority + 200 Validator slots, dual-bonded, slashing live in code |
| Distribution | Stripe merchant graph, MPP 100+ services | Circle Mint/CPN (136 FIs), 100+ institutions | 26M+ funded accounts, 120+ countries | None yet |

### Category table stakes (2025–26 consensus, per landscape brief)

1. Stablecoin-as-gas / gasless UX — holding a volatile native token to
   move dollars is now disqualifying.
2. Sub-second deterministic finality — universally marketed; ~350–600 ms
   is the demonstrated band for BFT L1 competitors.
3. Confidential-but-auditable transfers — amounts shielded from the
   public, visible to issuer/auditor/regulator.
4. Token-layer compliance hooks — allowlists, freeze, policy registries;
   GENIUS Act rules (proposed Apr 2026) and MiCA/TFR make issuer-level
   controls mandatory plumbing.
5. Native issuer mint/burn + fiat ramps at launch.
6. Named distribution partners at launch — "a launch without named
   payment-industry partners is read as a launch without distribution."

### What makes these chains succeed or stall

- **Distribution beats technology.** Plasma peaked at ~$6B TVL and lost
  ~90% of DAU when incentives ended; it recovered only when Tether's
  wallet pointed at it. Solana wins payments via Visa/Worldpay/Western
  Union, not TPS.
- **Issuer alignment is decisive.** Hyperliquid — with massive organic
  volume — sunset its own USDH within months; USDC's distribution won.
- **For regulated-asset chains, anchor institutions are the moat.**
  Canton's DTCC/Broadridge/HSBC pipeline ($8T+/month repo via DLR) is
  worth more than any public-chain TVL figure.

---

## 2. Where suwappu-dag stands (honest internal state)

What we can truthfully claim today, from this repo:

**Real and differentiated:**

- PQ-conservative crypto surface on *primary* signing/confidentiality
  paths: ML-DSA-65 intent signing, ML-KEM-768 confidential-transfer
  encryption, ML-DSA-65 bridge header attestations (the only
  trust-minimized + PQ bridge path in the category).
- Joint-quorum AND-gate safety (Theorem 2): forking requires Byzantine
  corruption of both rings — a structurally stronger claim than any
  single permissioned BFT set in the field.
- Constant-size (~1,600 B) LTP cross-chain attestation regardless of
  payload.
- Fast-path lane with K=4 equivocation binding and 100% bond slashing —
  economics competitors don't have (Tempo/Arc slashing models are
  unpublished or absent).
- Engineering rigor: 20 sprints closed, every load-bearing claim gated on
  10,000-case property tests; slashing waterfall, delegation, inflation
  implemented and tested at the substrate level.

**Not yet demonstrated or missing:**

- **Performance credibility.** The 2026-05-13 perf run: 100 TPS
  *submission* sustained, **0 committed TPS** in that campaign (loadgen
  ordering issue), ~5.75 s p50 round cadence, and two of four regions
  stalled after round 3–4. Competitors publish 350–600 ms deterministic
  finality and 3k–20k TPS with third-party observers. Our sub-second
  fast-path is a design goal, not a measurement.
- **No fee abstraction.** Gas is devnet SUWAPPU; there is no
  stablecoin-denominated fee path, no fee-payer/sender separation, no
  paymaster surface.
- **No EVM-standard developer surface.** The Intent API + Rust/TS SDKs
  are clean but proprietary; the dual-VM (EVM/Move) work is a projection
  layer over the balance map, not a MetaMask/Foundry-compatible endpoint.
  Every competitor is EVM tool-compatible on day one.
- **No stablecoin on the chain.** The registered-issuer and
  reserve-coverage precompiles are strong primitives with no issuer
  using them.
- **Bridge mint path not live.** Validators can serve ML-DSA-65 header
  attestations via RPC, but `submitHeader` is not wired to a production
  destination contract (documented in `README.md` § Bridge attestation).
- **LTP aggregate is classical.** The BLS12-381 aggregate inside the LTP
  commitment is a documented PQ-exception zone — a soft spot in our own
  headline claim that competitors' analysts would find quickly.
- **Public-launch surface incomplete.** ROADMAP tracks G2 (public RPC
  hardening), G3 (faucet service), G6 (metrics/alarms), G7 (explorer),
  G8 (status page) are still open.
- **No distribution.** No named partners, no wallet integrations, no
  fiat ramps, no exchange or custody relationships.

---

## 3. Gap register

Ranked by how much each gap would hurt a public launch. "Closing" states
the minimum credible bar, not parity with the leader.

| # | Gap | Severity | Evidence | What closing looks like |
|---|---|---|---|---|
| G-1 | **Committed-path performance unproven** — 0 committed TPS in last campaign; ~5 s round cadence; region stalls | **Critical** | perf-run-2026-05-13 vs Arc ~350 ms / Tempo ~0.5 s | A published multi-region perf run showing sustained committed TPS (target ≥1k) and measured fast-path confirmation <1 s, with the round-advance stall fixed. Publish honest numbers; the field's inflated claims (100k TPS "targets") are a credibility opening, not a bar. |
| G-2 | **No stablecoin-fee / gasless UX** | **Critical** | Table stake #1; Tempo fee AMM + sponsorship, Arc USDC gas, Plasma paymaster | Fee-payer ≠ sender separation in the Intent surface + a fee-sponsorship (paymaster) path, and a design decision (IQ) on stablecoin-denominated fees. |
| G-3 | **No EVM-standard developer endpoint** | **High** | All competitors EVM-compatible day one | Either an `eth_*` JSON-RPC compatibility layer over the EVM projection (MetaMask/Foundry can point at it) or an explicit, documented decision to compete on the Intent SDK only — with the trade-off argued publicly. |
| G-4 | **No issuer / asset story** | **High** | Table stake #5; OUSD consortium formed 2026-06-30; USDH lesson: rent distribution, don't build it | One named issuer (regional EMT, tokenized-deposit pilot, or consortium-adjacent) live on the registered-issuer precompile with reserve-coverage attestation — even on testnet. |
| G-5 | **Bridge mint path not wired end-to-end** | **High** | README honest-framing note | `submitHeader` → quorum oracle → mint path live against a public EVM testnet, demonstrating the PQ bridge claim end-to-end. |
| G-6 | **Confidential transfers not shipped** | **Medium** | Table stake #3; Arc TEE transfers, Solana confidential balances live | Track H confidential-balance L2 demo with auditor-key disclosure — our zk approach is architecturally stronger than Arc's TEE; shipping any working demo converts that into a claim. |
| G-7 | **Compliance primitives unmapped to regimes** | **Medium** | GENIUS rules (Apr 2026 NPRMs), MiCA/TFR zero-threshold | A short doc mapping DID / policy-vocabulary / registered-issuer / reserve-coverage onto GENIUS §-level and MiCA/TFR requirements; identify the one missing hook (likely travel-rule messaging) and file an IQ. |
| G-8 | **LTP BLS12-381 classical exception** | **Medium** | Our own invariant docs; Ethereum moving validator keys off BLS for PQ | A published migration target + timeline for the LTP aggregate (e.g., ML-DSA multi-sig or hash-based aggregate), so the exception zone reads as a plan, not a hole. |
| G-9 | **Interop is proprietary (LTP only)** | **Medium** | CCTP v2 + LayerZero OFT won stablecoin interop | Position LTP as the settlement-attestation layer and add an OFT-class adapter path for asset mobility; don't fight the interop standards war. |
| G-10 | **Public-launch surface incomplete** | **Medium** | ROADMAP G2/G3/G6/G7/G8 open | Close the five tracks; explorer + status page are the minimum for "public" to be taken seriously. |
| G-11 | **No distribution / partners** | **Critical (business)** | Landscape §6: distribution beats technology | Not an engineering deliverable, but engineering unblocks it: the CNSA 2.0 (2027-01-01) procurement hook + a live PQ bridge demo is the pitch asset for institutional design partners. |

---

## 4. Where we lead (moats to defend)

1. **PQ on primary surfaces.** Unique in the category. Arc's opt-in PQ
   wallets (Apr 2026) and Ethereum's leanXMSS roadmap validate the thesis
   while shipping nothing comparable. Sharpen the message: *PQ-by-default
   on consensus-critical and bridge surfaces* vs. their *opt-in,
   wallet-level, future-tense*. Defend it by closing G-8, or the
   BLS exception becomes the rebuttal.
2. **Dual-ring joint-quorum safety.** Every competitor is a single
   permissioned validator set (or a single sequencer). "Corrupting one
   ring — even fully — cannot fork the chain" is a stronger and more
   auditable claim than "our validators are reputable institutions."
3. **Constant-size cross-chain commitment.** ~1,600 B regardless of
   payload is a real cost-scaling argument against per-payload
   attestation schemes, and pairs naturally with the institutional
   settlement pitch.
4. **Slashing economics that exist in code.** 100% fast-path
   equivocation slashing, dual bonds, waterfall distribution — all
   property-tested. Competitors' accountability stories are governance
   promises.
5. **Verification culture.** 10k-case property gates per sprint and
   honest perf reporting are assets with exactly the audience (auditors,
   regulated institutions) we should target.

## 5. Positioning recommendation

Do **not** launch as "another payments chain" — Tempo, Arc, Solana, and
the Tether chains have consolidated retail/merchant distribution, and we
have none. Launch as **the post-quantum settlement layer for regulated
value** — the Canton/Kinexys/Fnality conversation, not the Plasma one:

- **Hook:** CNSA 2.0 requires PQ for new US national-security-adjacent
  acquisitions from 2027-01-01; NIST migration pressure hits every bank's
  crypto-agility program. We are the only chain in the category whose
  primary surfaces already comply.
- **Proof points to have ready:** live PQ bridge demo (G-5), honest
  multi-region perf run (G-1), confidential-transfer demo with auditor
  disclosure (G-6), and the compliance mapping doc (G-7).
- **Concede explicitly:** we are not EVM-first, not retail, not
  liquidity-driven. An honest "what this is NOT" section already exists
  in the README — keep that voice in public messaging; the field's
  criticism cycle (Tempo neutrality, Arc reversibility, Robinhood
  centralization) shows overclaiming is the fastest way to burn trust.

## 6. Proposed workstreams (for sprint planning)

Priority order; each candidate needs an IQ and sprint spec before work
starts, per the normal workflow.

**P0 — credibility (block public launch on these):**

- **PERF-1:** Fix round-advance stall (eu-west-1/ap-northeast-1 regions
  stopped after round 3–4 in the 05-13 run); re-run the multi-region
  campaign with committed-TPS as the headline metric; publish.
- **PERF-2:** Fast-path latency measurement — demonstrate the sub-second
  claim on the public devnet with a reproducible harness.
- **LAUNCH-1:** Close ROADMAP G2/G3/G6/G7/G8 (RPC hardening, faucet,
  metrics, explorer, status page).

**P1 — table stakes (target: before incentivized testnet, Phase 5):**

- **FEE-1 (IQ first):** Fee-payer/sender separation + fee-sponsorship in
  the Intent surface; decide stablecoin-denominated fees.
- **BRIDGE-1:** Wire `submitHeader` → quorum oracle → mint end-to-end on
  a public EVM testnet (converts the PQ bridge from claim to demo).
- **EVM-1 (IQ first):** Decide the developer-surface strategy — `eth_*`
  RPC compatibility layer over the EVM projection vs. Intent-SDK-only —
  and document the decision either way.
- **CONF-1:** Track H confidential-balance demo with auditor-key
  selective disclosure.

**P2 — differentiation defense & positioning:**

- **PQ-1 (IQ first):** Migration target + published timeline for the LTP
  BLS12-381 aggregate (closes the exception-zone rebuttal).
- **COMP-1:** Compliance mapping doc (GENIUS / MiCA / TFR ↔ existing
  precompiles); file an IQ for the travel-rule gap if confirmed.
- **INTEROP-1 (IQ first):** OFT-class adapter feasibility, positioning
  LTP as attestation, not asset-mobility, layer.
- **GTM-1 (non-engineering):** Institutional design-partner outreach kit
  built on the P0/P1 proof points + CNSA 2.0 timeline.

---

## Method note

Competitor facts were gathered 2026-07-03 by four parallel research
passes over public sources (announcements, docs, funding coverage,
critical commentary); every claim in the [briefs](briefs/) carries an
inline citation, and rumored/unconfirmed items are flagged. Internal
facts come from `README.md`, `ROADMAP.md`, `CHANGELOG.md`, and
`docs/perf-run-2026-05-13/`. Competitor performance figures are largely
self-reported; treat marketing "targets" (e.g., Tempo's 100k TPS) as
distinct from demonstrated numbers throughout.
