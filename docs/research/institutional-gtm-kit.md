# Institutional design-partner outreach kit (GTM-1)

**Date:** 2026-07-03
**Status:** Internal enablement asset — foundation for the human GTM/BD team.
NOT a public marketing page, NOT outreach that has happened.
**Owner:** GTM / BD (human-run). Engineering supplies and updates the proof-point status.
**Source of record:** [`competitive-gap-analysis.md`](competitive-gap-analysis.md)
(§4 where we lead, §5 positioning, §6 workstreams) and
[`briefs/landscape.md`](briefs/landscape.md) (§1 institutional context, §3 PQ
adoption, §6 distribution).

---

## 1. Purpose and audience

This is the enablement asset for the workstream **GTM-1** in the competitive gap
analysis (§6): "Institutional design-partner outreach kit built on the P0/P1
proof points + CNSA 2.0 timeline." It exists so a BD person can walk into a
conversation with a regulated institution and know — precisely — what we can
truthfully claim today, what we cannot, and which objection is coming. It is a
briefing pack, not a pitch deck and not a script.

**Audience:** internal GTM / BD / partnerships. Secondary: founders and
engineering leads who need to know which proof points BD is leaning on so they
stay accurate as the code moves.

> **The one rule that governs everything below: every claim must stay honest.**
> The 2025–26 field taught this the hard way. The criticism cycles that burned
> the loudest launches — Tempo's "neutrality" being questioned, Arc's
> reversibility/dispute design read as centralization, Robinhood Chain's single
> sequencer — were all *overclaim-versus-reality* gaps that critics found in a
> day. With institutions specifically, overclaiming is the fastest way to burn
> trust: the audience employs the analysts who check. When in doubt, concede.
> A conceded weakness is credibility; a discovered exaggeration is disqualifying.

---

## 2. Positioning

**One-liner:**

> **The post-quantum settlement layer for regulated value.**

Not "another payments chain." Retail/merchant distribution has already
consolidated around Tempo, Arc, Solana, and the Tether chains, and we have none
of it. Our defensible lane is the institutional / regulated-settlement segment —
the Canton / Kinexys / Fnality conversation, not the Plasma one (gap analysis §5).

**Three-sentence pitch:**

> suwappu-dag is a settlement layer whose consensus, signing, and cross-chain
> attestation surfaces already run on NIST-standardized post-quantum
> cryptography (ML-DSA-65 / ML-KEM-768) — the only chain in the
> payments/settlement category that ships PQ on its *primary* surfaces rather
> than as an opt-in wallet feature or a roadmap item. Its safety rests on a
> dual-ring joint-quorum design: forking the chain requires Byzantine corruption
> of *both* an Authority Ring and a Validator Ring, a structurally stronger and
> more auditable guarantee than "our validators are reputable institutions." It
> is built for institutions whose crypto-agility and CNSA 2.0 timelines make
> post-quantum a procurement requirement, not a nice-to-have.

**Why now (the hook):** NSA CNSA 2.0 requires post-quantum for new
national-security-adjacent acquisitions from **2027-01-01**, and NIST migration
pressure hits every regulated institution's crypto-agility program on a
similar horizon (landscape §3). No competitor in the category has PQ on primary
surfaces. The window to own this narrative is real but narrowing — Ethereum and
Arc have both signalled intent.

---

## 3. Target segments (ranked)

Ranked by fit with what we can defensibly claim *today*. For each: why they care,
the specific proof point that lands, and the honest objection they will raise
(and which you must not talk past).

### (a) Institutions with CNSA 2.0 / crypto-agility mandates — **primary**

Defense-adjacent primes, national-security-adjacent financial infrastructure,
and any regulated institution running a formal crypto-agility / PQ-migration
program against the 2027-01-01 CNSA 2.0 acquisition deadline.

- **Why they care:** They have a dated procurement requirement and a live
  migration budget. PQ is not a differentiator to them — it is a checkbox they
  are being forced to fill, and almost nothing in the settlement-chain category
  fills it.
- **Proof point that lands:** PQ-on-primary-surfaces (ML-DSA-65 intent signing,
  ML-KEM-768 confidential-transfer encryption, ML-DSA-65 bridge header
  attestation) — invariant 2, and the only category chain that has it in code
  rather than on a roadmap (landscape §3).
- **Honest objection:** *"Your LTP cross-chain aggregate is still classical
  BLS12-381 — so you are not actually PQ end-to-end."* True. Concede it, and
  show the migration plan (IQ-009 / PQ-1). See §4.

### (b) Regulated-settlement / tokenized-deposit venues — the Canton/Kinexys adjacency

Wholesale settlement networks and tokenized-deposit / tokenized-Treasury
programs (Canton, Kinexys, Fnality-class), and institutions building on or
adjacent to them who care about long-lived integrity of settlement records.

- **Why they care:** For regulated-asset rails, anchor institutions and
  auditability are the moat, not TVL (landscape §6). Settlement records are
  long-lived — a "harvest now, decrypt later" exposure and a 20-year-integrity
  argument resonate here in a way they don't in retail payments.
- **Proof point that lands:** Dual-ring joint-quorum safety (Theorem 2,
  invariant 1) — a single compromised ring cannot fork the chain — plus the
  constant-size (~1,600 B) LTP attestation as a bounded cross-chain settlement
  cost argument, plus the 10k-case property-test verification culture that this
  audience's auditors actually value.
- **Honest objection:** *"Canton already has DTCC, HSBC, and $8T/month of repo
  through Broadridge DLR. You have no anchor institution and no live asset."*
  Also true. We are not competing on installed distribution; we are offering a
  PQ-and-dual-ring settlement substrate they cannot get from an incumbent. Do
  not pretend we have their pipeline.

### (c) PQ-forward custodians / infrastructure providers

Custodians, key-management / HSM vendors, and settlement-infra providers who
sell "quantum-ready" as part of their own roadmap and need a chain that backs
the claim.

- **Why they care:** They want to *resell* a credible PQ story to their own
  regulated customers, and they need a settlement surface that doesn't undercut
  it with classical-only signing.
- **Proof point that lands:** PQ on primary surfaces (invariant 2) plus the
  fast-path slashing economics (100% bond forfeiture for equivocation, in code
  and property-tested) as an accountability story competitors answer with
  governance promises.
- **Honest objection:** *"This is pre-mainnet devnet with no published
  performance and no partners — we can't build a customer promise on it yet."*
  Correct. This segment is a design-partner / co-development conversation, not a
  production-integration one. Frame it that way.

---

## 4. Proof-point status table

The load-bearing table for BD. Every claim is tied to its **current** repo/CI
status so nobody overclaims. If a row's "Current status" says something isn't
done, the "What NOT to say" column is a hard boundary, not a suggestion.

| Claim | Evidence | Current status | What NOT to say |
|---|---|---|---|
| **PQ on primary signing/confidentiality surfaces** | Invariant 2; ML-DSA-65 intent signing + ML-KEM-768 confidential-transfer encryption + ML-DSA-65 bridge header attestation; `suwappu-crypto`, DAG-S1 gated on 7 properties × 10k cases | **Real and shipped in code.** The strongest true differentiator. | Don't say "fully post-quantum" or "PQ end-to-end" — the LTP aggregate is classical (see below). Say "PQ on primary surfaces." |
| **Dual-ring joint-quorum AND-gate safety** | Invariant 1 / Theorem 2; 40-slot Authority + 200-slot Validator rings; `suwappu-consensus`, DAG-S5 gated at 10k cases | **Real and property-tested.** Structurally stronger than any single permissioned BFT set in the field. | Don't imply it's been externally audited or battle-tested at scale — it's proof-gated in code, not adversarially tested in production. |
| **Constant-size (~1,600 B) LTP attestation** | Invariant 3; ML-KEM-768 ct (1,568 B) + BLS12-381 aggregate (96 B) + SHA3-256 root (32 B); `suwappu-ltp`, DAG-S15/S16 | **Real** — but note the aggregate is **classical BLS12-381**, a documented PQ-exception zone. Migration tracked as PQ-1 / **IQ-009**. | Don't present the ~1,600 B commitment as post-quantum. The KEM ciphertext is PQ; the BLS aggregate is not. Lead with "constant-size," disclose the classical aggregate. |
| **Fast-path equivocation = 100% slashing** | Invariant 5 / paper §6.4; K=4 binding; `suwappu-fastpath` + `suwappu-validator`, DAG-S8/S9 at 10k cases | **Real in code and property-tested** — but currently **observability-only** per **issue #16**; the enforcement/reporting path is not wired to live economic execution. | Don't claim slashing is "live" or "enforced on mainnet." It exists and is tested in code; economic enforcement is observability-stage. |
| **Performance / finality** | Round-driver stall fixed (**PERF-1**); fast-path latency harness built (**PERF-2**); perf-run-2026-05-13 | **Honest / incomplete.** The stall that caused 0 committed TPS is fixed and a latency harness exists, but there is **no published multi-region committed-TPS number yet** — the AWS re-run is pending. | Do **not** cite any TPS figure, and do **not** cite competitor-style numbers ("100k TPS," "sub-second finality") as measured. Sub-second is a design goal. Say "published perf run pending." |
| **PQ cross-chain bridge** | ML-DSA-65 header attestation path (`suwappu-consensus` bridge_header, `suwappu_getHeaderAttestation` RPC); README honest-framing note; **BRIDGE-1** pending | **Attestation path is real; end-to-end mint is NOT wired.** `submitHeader` → quorum oracle → mint against a production destination is not live. | Don't demo or describe this as an end-to-end working bridge. Validators can *produce and serve* PQ attestations; the mint finalization path is not connected. |

---

## 5. Competitive contrast cheatsheet

One true differentiator we hold over each rival, and the one thing each does
better that we concede up front. Conceding is the point — see §1.

| Rival | Our one true differentiator | What they do better (concede it) |
|---|---|---|
| **Tempo** (Stripe/Paradigm) | PQ on primary surfaces + dual-ring joint-quorum safety; they run classical crypto on a single permissioned BFT set. | **Mainnet is live** (2026-03-18) with real distribution — Visa/Stripe/MoneyGram validators, OUSD consortium, benchmarked throughput. We are pre-mainnet devnet with no distribution. |
| **Arc** (Circle) | PQ is *default on our consensus/bridge surfaces*; Arc's PQ (Apr 2026) is **opt-in, wallet-level, future-tense**. | Native USDC/EURC issuance, EVM tooling day one, TEE confidential transfers, and a real issuer story. We have precompiles with no issuer using them and no EVM-standard endpoint. |
| **Robinhood Chain** | Decentralized dual-ring settlement with slashing economics; theirs is a **single centralized sequencer**. | Live mainnet, 26M+ funded accounts, 120+ countries, real distribution and a flagship asset (tokenized stocks). We have none of that reach. |

---

## 6. What we must ship before serious outreach

Outreach at scale is **gated** on these. Each is a workstream in the gap
register (see [`competitive-gap-analysis.md`](competitive-gap-analysis.md) §3
gap register and §6 workstreams). Until they land, conversations should be
framed as early design-partner exploration, not "come integrate."

1. **A published, honest multi-region perf run** (PERF-1 → PERF-2 → campaign;
   closes gap **G-1**). The round-advance stall is fixed and a latency harness
   exists; what's missing is the AWS re-run producing a sustained committed-TPS
   number and a measured fast-path confirmation figure, published honestly.
   Until this exists BD cites *no* performance numbers.
2. **The end-to-end PQ bridge demo** (**BRIDGE-1**; closes gap **G-5**).
   Wire `submitHeader` → quorum oracle → mint against a public EVM testnet so
   the PQ-bridge claim is a *demo*, not a description. This is the single
   highest-leverage proof point for segment (a).
3. **The confidential-transfer demo** (**CONF-1**; closes gap **G-6**).
   Track H confidential-balance L2 with auditor-key selective disclosure —
   converts our architecturally-stronger-than-TEE zk approach from a claim into
   a working demo, which segment (b) specifically asks about.

Supporting, not strictly gating, but sharpens the story: **PQ-1 / IQ-009**
(published migration target + timeline for the classical LTP BLS12-381
aggregate — closes gap **G-8** and turns the §4 objection into a plan) and
**COMP-1** (compliance mapping onto GENIUS / MiCA / TFR — gap **G-7**).

---

## 7. Do NOT claim

Hard boundaries. Saying any of these is an overclaim that an institutional
analyst will catch, and per §1 that is the fastest way to lose the room.

- **NOT on mainnet.** Current line is `0.x`; mainnet GA targets v1.0 in the
  M18–M24 window. Do not imply production availability.
- **No live token.** Devnet SUWAPPU is fungible test currency with no value.
- **No TPS or finality numbers we haven't measured.** No committed-TPS figure is
  published; sub-second finality is a design goal, not a measurement. Do not
  borrow competitor-style throughput/latency numbers.
- **Not EVM-compatible.** No `eth_*` JSON-RPC / MetaMask / Foundry endpoint
  exists (EVM-1 is undecided). The Intent SDK is proprietary. Don't imply drop-in
  EVM tooling.
- **No issuer / no stablecoin on the chain.** The registered-issuer and
  reserve-coverage precompiles are real primitives with **no issuer using them**.
  Do not name or imply an issuer, asset, or reserve partner we do not have.
- **Not PQ end-to-end.** The LTP aggregate is classical BLS12-381. Say "PQ on
  primary surfaces," never "fully quantum-safe."
- **Slashing is not economically enforced/live** — it is in-code and
  property-tested, observability-only per issue #16.
- **No named distribution partners, wallet integrations, or fiat ramps.** Don't
  imply any.

---

## 8. Close

This document is the **asset**, not the outreach. The actual go-to-market
execution — identifying and contacting design partners, booking meetings,
running pilots, negotiating terms — is **⛔ human-run** and out of scope for
automated action. Engineering keeps §4 accurate as code moves; BD decides who to
talk to and when. Serious outreach should wait on the §6 gating list; until then,
any conversation is early design-partner exploration framed honestly, or it
risks the exact overclaim failure mode §1 warns against.

Cross-references: gap register and workstreams in
[`competitive-gap-analysis.md`](competitive-gap-analysis.md); institutional
context and PQ-adoption evidence in [`briefs/landscape.md`](briefs/landscape.md);
the "what this is NOT" framing this doc preserves is in the repo
[`README.md`](../../README.md).
