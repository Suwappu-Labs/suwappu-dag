# Compliance-regime mapping — GENIUS / MiCA / EU TFR ↔ suwappu-dag precompiles

**Date:** 2026-07-03
**Workstream:** COMP-1 (from the [competitive gap analysis](competitive-gap-analysis.md), gap G-7)
**Status:** Mapping complete; travel-rule gap confirmed against the code; IQ recommended
**Source briefs for regulatory facts:** [`briefs/landscape.md`](briefs/landscape.md) §2, §5 ·
[`briefs/tempo.md`](briefs/tempo.md) (TIP-403 policy registry) ·
[`briefs/arc.md`](briefs/arc.md) (selective disclosure / opt-in confidential + compliance)

> **Disclaimer.** This is an engineering-to-regulation mapping of what the
> code in `crates/suwappu-precompiles/` does today against three regulatory
> regimes. It is written for an auditor- or regulator-facing engineer. It is
> **not legal advice**, not a compliance opinion, and not a certification.
> Every "Covered" verdict below describes a *primitive that could support*
> the requirement under an operating issuer, not a discharged legal
> obligation.

---

## 1. Purpose and scope

The landscape brief (§2 item 4, §5) establishes that token-layer compliance
hooks — issuer freeze/seize, sanctions screening, reserve backing, and
travel-rule messaging — have become mandatory plumbing for any chain carrying
regulated value:

- **GENIUS Act** (US, enacted 2025-07-18) is at its 2026-07-18 statutory
  rulemaking deadline; FinCEN/OFAC proposed AML + sanctions rules on
  2026-04-08 treating permitted payment stablecoin issuers as BSA financial
  institutions, pushing issuer-level **freeze/seize**, **sanctions
  screening**, and **SAR-capable monitoring** toward mandatory (brief §5).
- **MiCA** (EU) transitional period ended 2026-07-01; only MiCA-authorized
  EMTs remain in EEA regulated markets, EMT issuers need credit-institution
  or e-money authorization, and from March 2026 EMT custody/transfer may need
  both MiCA and PSD2 licensing (brief §5).
- **EU TFR** applies a **zero-value threshold** for CASP-to-CASP travel-rule
  data — originator/beneficiary information must accompany every transfer
  between crypto-asset service providers, no de minimis (brief §5, §2).

This document scopes the mapping to the four application-layer primitives the
gap analysis named (DID, policy-vocabulary, registered-issuer,
reserve-coverage) and grounds each verdict in the actual module source, not in
the paper's aspirational language.

**Read the code, not the spec:** two of the four "primitives" the gap
analysis treats as peers do not exist as enforcement surfaces (see §5). The
verdicts below reflect that.

---

## 2. What each precompile actually does today

Grounded in `crates/suwappu-precompiles/src/`:

- **DID resolver** (`did.rs`, `did_resolver.rs`; DAG-S12). A phase-1 subset of
  W3C DID Core. `did:suwappu:<32-byte id>` → a singleton `DidDocument` holding
  ML-DSA-65 verification methods, W3C verification-relationship arrays, and
  free-form `Service` endpoints (`service_type` + `endpoint` string). The
  resolver is an in-memory `BTreeMap`; `create` is singleton-enforced,
  `update` requires an ML-DSA-65 signature under a prior-document
  `CapabilityInvocation` key. Validation is **structural only** — method-id
  uniqueness, controller = self, no dangling relationships. There is **no
  verifiable-credential verification, no revocation registry, and no
  attestation of real-world identity**; a `Service` may *name* a
  `CredentialRegistry` endpoint but nothing on-chain checks it.

- **Registered-issuer precompile** (`issuer.rs`; DAG-S13). An `IssuerRegistry`
  mapping `IssuerId → Issuer { principal_did, delegation_cap,
  reserve_schema_version, policy_vocabulary_version }`, plus supply
  book-keeping and a two-phase burn (`initiate_burn` → `finalize_burn` with a
  `PaymentReceiptAttestation`, or `reverse_burn` after an SLA deadline).
  `mint` enforces the per-issuer `delegation_cap` across all the issuer's
  assets. **What it does not have:** any operation that touches a *third
  party's* balance. Burn only retires supply the issuer already holds as
  circulating (`initiate_burn` requires `circulating() >= amount`). There is
  **no freeze, no seize, no clawback, no allowlist/denylist** of holder
  addresses.

- **Reserve-coverage circuit breaker** (`reserve.rs`; DAG-S14). A
  `CoverageRule` enum — `OneToOnePar` (documented as GENIUS payment-stablecoin
  and MiCA EMT backing), `NavStrike`, `Jurisdiction` — plus a
  `ReserveCoverageChecker` state machine (`set_rule`, `submit_attestation`,
  `can_mint` with a TTL). The math predicate is real and property-tested. Two
  honest caveats from the code: (a) the `proof` field is an **opaque
  placeholder** — phase-1 verifies directly against the public
  `total_reserves` input, the Plonky3/SP1 circuit is a follow-up; (b) the
  breaker is **not wired into `mint`** — `IssuerRegistry::mint` checks only the
  delegation cap, and the module header states mint integration "is a
  follow-up." So today the breaker is a standalone, unenforced state machine.

- **Policy-vocabulary.** **Not a precompile.** It exists solely as
  `policy_vocabulary_version: u32`, an inert field on the `Issuer` struct
  (mirroring `reserve_schema_version`). There is no vocabulary, no rule set, no
  evaluator, and nothing reads the field. The gap analysis (and the
  competitive snapshot's "compliance hooks" row) list "policy-vocabulary"
  alongside the DID/issuer/reserve precompiles as if it were a peer
  enforcement surface; in the code it is a version number. This is the first
  thing an auditor comparing us to Tempo's TIP-403 *policy registry* (brief:
  tempo) would find.

- **Transfer surface.** The transfer path is `Intent::Transfer { from, to,
  amount }` in `crates/suwappu-execution/src/substrate.rs`. It carries **no
  memo, note, attachment, or reference field** — three fields, all
  value-movement, no messaging channel. (Confirmed by reading the full
  `Intent` enum; the only variants with byte payloads are consensus/L2
  operations — `proof_ref`, `public_inputs`, pubkey material — none
  transfer-attached.)

---

## 3. Requirement ↔ primitive ↔ coverage

Verdicts: **Covered** (a primitive plausibly discharges the technical
requirement under an operating issuer) / **Partial** (a primitive exists but is
incomplete, unenforced, or a placeholder) / **Missing** (no primitive
addresses it).

| Regime | Requirement | suwappu-dag primitive | Verdict | Grounding |
|---|---|---|---|---|
| **GENIUS** | Issuer-level **freeze / seize** of holder balances | registered-issuer precompile | **Missing** | `issuer.rs` has mint + two-phase burn only; burn requires the issuer's *own* circulating supply — no operation reaches a third-party holder's balance. No freeze/seize/clawback. |
| **GENIUS** | **Sanctions screening** (OFAC) at transfer/mint | — | **Missing** | No address blocklist, no screening hook, no OFAC list anywhere in the precompiles or the transfer path. DID gives per-account identity but nothing screens it. |
| **GENIUS** | **SAR-capable monitoring** (BSA financial-institution obligations) | DID resolver (identity binding) + off-chain indexer | **Partial** | DID binds accounts to keys/identity, a prerequisite for monitoring; but there is no on-chain risk-scoring, thresholding, or reporting surface. Monitoring would be entirely off-chain against the indexer. |
| **GENIUS** | **1:1 reserve backing** for payment stablecoins | reserve-coverage `OneToOnePar` rule | **Partial** | Predicate is real and property-tested and explicitly maps to GENIUS par; but `proof` is a placeholder and the breaker is **not wired into `mint`** (`reserve.rs` header). It gates nothing today. |
| **MiCA** | **EMT issuer authorization** gating who may mint | registered-issuer registry (`principal_did`, `delegation_cap`) | **Partial** | The registry structurally gates minting to a registered principal DID under a cap — a plausible authorization anchor. But registration is an admin insert with no binding to a real license/authorization, and `policy_vocabulary_version` (the field that would carry jurisdictional rules) is inert. |
| **MiCA** | **EMT 1:1 backing** | reserve-coverage `OneToOnePar` rule | **Partial** | Same predicate, same caveats as the GENIUS backing row. |
| **MiCA** | **Custody / transfer controls** (restrict, pause, condition transfers) | — | **Missing** | No transfer-restriction hook. `Intent::Transfer` is unconditional value movement; no per-asset freeze, allowlist, or policy gate sits in the transfer path. |
| **EU TFR** | **Travel-rule data** (originator + beneficiary VASP info) on CASP-to-CASP transfers, zero-value threshold | — (DID resolver is an adjacent building block only) | **Missing** | `Intent::Transfer` has no memo/attachment; there is no originator/beneficiary field and no messaging channel. DID `Service` endpoints could *address* a VASP but carry no travel-rule payload. **This is the prime missing hook (§4).** |

---

## 4. The missing hook: travel-rule messaging

The clearest, highest-severity gap, and the one the gap analysis predicted, is
**EU TFR / FATF travel-rule messaging**. Confirmed against the code:

- `Intent::Transfer { from, to, amount }` has no field to carry originator or
  beneficiary VASP data, and there is no separate travel-rule message type.
- There is no on-chain commitment surface (hash/root) that a transfer could
  bind an off-chain IVMS101 payload to.
- The EU TFR zero-value threshold (brief §5) means this applies to *every*
  CASP-to-CASP transfer, not a subset — so an omission here is not an edge
  case, it is the common path for any regulated-CASP deployment.

Two secondary GENIUS/MiCA gaps sit alongside it and should be recorded even
though travel-rule is the headline: **issuer-level freeze/seize** and
**sanctions screening** are both fully Missing, and the reserve-coverage
breaker is **Partial-but-unenforced** until it is wired into `mint`.

What we do have that is adjacent: the DID resolver already models `Service`
endpoints with a `service_type` + `endpoint` string. A CASP DID could
advertise a travel-rule messaging endpoint (Notabene/TRISA/Veriscope-style)
via a `Service` entry — the identity/addressing layer for travel-rule exists;
the *payload and its on-chain binding* do not.

---

## 5. Recommendation

**Build a travel-rule surface, and decide its shape via an IQ** because the
leading option touches the load-bearing Intent surface.

Two candidate designs, to be argued in the IQ:

1. **On-chain travel-rule attachment on the transfer Intent.** Add an optional
   commitment (e.g. a 32-byte hash of an off-chain IVMS101 payload) to the
   transfer path, so the chain carries a tamper-evident binding while the PII
   stays off-chain. This **touches `Intent::Transfer`**, which is
   `#[non_exhaustive]` and a substrate-invariant surface (lane separation,
   determinism, bundle atomicity per CLAUDE.md invariant 4). That is exactly
   the kind of change the workflow requires an **IQ** for before spec — the
   Intent enum is consumed across `suwappu-execution`, `suwappu-node`,
   `suwappu-rpc`, `suwappu-fastpath`, `suwappu-mempool`, and both SDKs.

2. **Off-chain messaging integration** (Notabene / TRISA / Veriscope), keyed by
   a DID `Service` endpoint per CASP, with only a minimal on-chain reference
   (or nothing on-chain). This leaves the Intent surface untouched and is
   lower-risk, but concedes that travel-rule compliance lives entirely
   off-chain — a positioning choice worth stating explicitly given the
   institutional-settlement audience.

**Filing:** open **COMP-1a** as an IQ (`/iq-decision travel-rule-surface`)
covering the attachment-vs-off-chain decision. Separately, two follow-ups do
*not* need an IQ and can be sprint-specced directly, since they extend existing
precompiles without touching the Intent surface:

- Wire `ReserveCoverageChecker::can_mint` into `IssuerRegistry::mint` (closes
  the Partial→enforced gap on reserve backing; already flagged as a follow-up
  in `reserve.rs`).
- Add an issuer-level freeze/seize + address-screening surface to the
  registered-issuer precompile (GENIUS/MiCA custody controls). This likely
  *does* warrant its own IQ if freeze reaches into third-party balances, as it
  interacts with the substrate's balance-map invariants — flag when specced.

---

## 6. Honest limitations

- **Issuer-level compliance rides on an issuer we don't have.** Every
  "Covered/Partial" verdict above assumes a registered issuer is operating the
  mint/burn/reserve machinery. suwappu-dag has no issuer today — this is gap
  **G-4** (no issuer / asset story) and **FEE-1**-adjacent in the competitive
  analysis. Freeze, seize, sanctions screening, SAR monitoring, and reserve
  attestation are all obligations of the *issuer as a regulated entity*; the
  chain provides primitives, the issuer provides the program. No issuer, no
  discharged obligation.

- **These are primitives, not a certified compliance program.** Nothing here
  has been assessed by counsel or a compliance auditor. The reserve predicate's
  ZK `proof` is a placeholder; the coverage breaker is unenforced; the DID
  layer verifies structure, not real-world identity; "policy-vocabulary" is a
  version integer. A regulator would read this as building blocks, not a
  control framework.

- **Regulatory facts are as of 2026-07-03** and cited to the briefs; NPRMs
  (GENIUS AML/sanctions, effective ~2027) are proposed, not final, and the
  mapping should be revisited when rules finalize.

---

## Method note

Regulatory facts are drawn from [`briefs/landscape.md`](briefs/landscape.md)
§2 and §5 (each claim carries an inline citation there). Internal facts are
grounded in a direct read of `crates/suwappu-precompiles/src/{did,did_resolver,
issuer,reserve,lib}.rs` and the `Intent` enum in
`crates/suwappu-execution/src/substrate.rs` on 2026-07-03. Where the paper's
language and the code diverge (policy-vocabulary, the reserve `proof` field,
mint-integration of the breaker), this document follows the code.
