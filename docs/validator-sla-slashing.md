# Validator SLA + slashing terms

**Status:** Spec ratified by `suwappu-strategy/docs/mainnet-plan.md` Track E + Tokenomics
v2 §8.1–8.3. Referenced verbatim by Tier A and Tier B subscription
agreements (Track C, suwappu-dag issues #128 + #129).

**Audience:** Authority Super Node operators (Tier A) and
Genesis/Standard Validator operators (Tier B) of the suwappu-dag L1.

**Authoritative inputs:**
- `suwappu-strategy/docs/sources/SUWAPPU-Tokenomics-v2.docx` §8 — Slashing
- `suwappu-strategy/docs/sources/SUWAPPU-Node-Validator-Sale-Model-v2.0.xlsx`
  — Risk Factors §17, §20, §24, §26
- Paper §6.4 — fast-path equivocation = 100% slash
- `crates/suwappu-fastpath/` — production equivocation-proof
  implementation (DAG-S9 exit gate proptest at 10k cases)

---

## 1. Per-tier SLA matrix

The SLA fields below are part of the operator's slot subscription
agreement. The foundation monitors compliance; sustained breach is
grounds for slot suspension or full ejection by Authority Ring
quorum.

### Tier A — Authority Super Node (40 slots @ \$3M)

| Field | Requirement | Slot bond at risk |
|---|---|---|
| Uptime (rolling 30-day) | **≥ 99.9 %** | Liveness slash (medium) below 99.5 %; full review below 95 % |
| Response-to-page (P1/critical) | **≤ 15 min** | Manual escalation to foundation ops |
| Response-to-page (P2/high) | **≤ 1 hr** | Performance-reward reduction |
| Geographic distribution | **No > 25 % of Tier A in any one region** | Foundation BD enforces at slot allocation |
| Sector diversification | **No > 30 % of Tier A in any one sector** (custody / settlement / stablecoin / exchange / RWA / payments) | Same |
| Compliance attestation | **Quarterly**: KYC re-verification, sanctions screening, jurisdictional license proof | Slot suspension; potential re-allocation |
| Custody for self-stake | **Multi-sig 3-of-5 + HSM (FIPS 140-3 Level 3+) + certified custody partner** (Fireblocks / BitGo / Anchorage) | See E.2 (#143); key compromise = total stake loss |
| Self-stake | **15 M SUWAPPU** (≈ \$1.5 M at \$0.10 TGE, \$3.3 M at \$0.22 Y3 base) | Slashable per matrix in §3 |
| Hardware (min) | 32 vCPU / 128 GB / 4 TB NVMe / 10 Gbps + dedicated ops + compliance team | Operator-funded; opex modeled \$250 k/yr per slot in sale model |

### Tier B — Genesis Validator (200 slots @ \$150 k)

| Field | Requirement | Slot bond at risk |
|---|---|---|
| Uptime (rolling 30-day) | **≥ 99.5 %** | Liveness slash below 99 %; full review below 90 % |
| Response-to-page (P1/critical) | **≤ 30 min** | Manual escalation |
| Response-to-page (P2/high) | **≤ 4 hr** | Reward reduction |
| Geographic distribution | **No > 15 % of Tier B in any one region** | Foundation BD enforces at slot allocation |
| Compliance attestation | **Annual** | Slot suspension; potential re-allocation |
| Custody for self-stake | **Multi-sig 2-of-3 + HSM strongly recommended** (FIPS 140-3 Level 2+); certified custody partner optional | See E.2 (#143) |
| Self-stake | **3 M SUWAPPU** (≈ \$150 k at \$0.05 TGE, \$660 k at \$0.22 Y3 base) | Slashable per matrix in §3 |
| Hardware (min) | 16 vCPU / 64 GB / 2 TB NVMe / 10 Gbps | Operator-funded; opex modeled \$30 k/yr per slot in sale model |

### Standard Validator (post-TGE, no sale slot)

Standard Validators are the permissionless on-ramp post-TGE. They
attract delegated stake to reach the minimum self-stake threshold;
the Validator Ring grows from 200 → 500 by Y5.

| Field | Requirement |
|---|---|
| Self-stake (minimum) | **100 k SUWAPPU** + **1 M SUWAPPU** total delegated stake to qualify for active set |
| Commission rate | 0 % – 30 % (set per validator) |
| Voting cap | **0.05 %** of total voting power per validator (Tokenomics §2.4); on-chain enforced |
| SLA targets | Identical to Tier B |
| Slashing | Identical to Tier B (no preferential treatment vs sale-cohort validators) |

---

## 2. Slashing offense matrix

Severity tiers map onto the slashing distribution in §4. All offenses
below are evaluated by Authority Ring quorum (≥ 27-of-40, per
IQ-001 quorum formula).

### Critical (100 % stake forfeiture)

| Offense | Surface | Detection |
|---|---|---|
| Double-vote / equivocation | DAG (validator) and fast-path (Authority) lanes | `crates/suwappu-consensus/src/` + `crates/suwappu-fastpath/` |
| Fast-path equivocation | Authority-only; Paper §6.4 | `crates/suwappu-fastpath/` |
| LTP attestation fraud | Authority-only; cross-chain LTP corridor surface | `crates/suwappu-ltp/` |
| Certificate fraud | Authority-only; certificate-production surface | `crates/suwappu-authority/` |
| Genesis ceremony key reuse | Authority-only; M18 launch surface | Operational |

A confirmed critical-offense proof results in **100 % self-stake
forfeiture** AND **slot ejection by Intent::EjectAuthority** (for
Tier A) or **Intent::EjectValidator** (for Tier B / Standard).

### High (up to 100 %, severity-scaled)

| Offense | Surface | Penalty range |
|---|---|---|
| Compliance violation | Tier A regulatory framework | 25 %–100 % depending on severity + jurisdiction |
| Sanctions screening failure (OFAC, EU, HMT) | Tier A | 50 %–100 % + immediate ejection |
| FATF Travel Rule non-compliance (post-TGE) | All tiers | 25 %–50 % |
| Key compromise + delayed disclosure | All tiers | 100 % if disclosure > 24h after detection per E.2 (#143) |

### Medium (5–10 % per occurrence; caps at 50 % drained before ejection)

| Offense | Surface | Per-occurrence penalty |
|---|---|---|
| Prolonged downtime (> 4 hrs in 30-day window) | All tiers | 5 % of self-stake |
| Missed force-include deadline | L2 sequencer only (Track G #103) | 5–10 %; caps at 50 %, then full ejection |
| Failed compliance re-attestation | Tier A | 10 % per quarter missed |
| Failed geographic / sector diversification | Tier A | 10 % per detected breach (foundation review) |

### Low (warning + reward reduction; no stake slash)

| Offense | Surface | Penalty |
|---|---|---|
| Single missed page-response SLA | All tiers | Foundation warning + performance-reward reduction next epoch |
| Hardware-spec underprovisioning detected | All tiers | Foundation warning + 30-day remediation window |

---

## 3. L2 sequencer slashing (Track G addition)

The L2 sequencer is a special-case role with its own bond structure
(per Track G issue #103). It runs in parallel with the L1 validator
program; sequencer operators are recruited from the Validator Ring
pool, so the bond sizes are aligned with Tier A / Tier B self-stake.

| Bond | Size | At TGE (\$0.10) | At Y3 base (\$0.22) | Slashing |
|---|---|---|---|---|
| Liveness bond (refundable) | 3 M SUWAPPU | \$300 k | \$660 k | 5–10 % per missed force-include deadline; caps at 50 % drained before full ejection |
| Safety bond (forfeit) | 15 M SUWAPPU | \$1.5 M | \$3.3 M | **100 %** on equivocation (signing two conflicting L2 batches) or invalid batch |

Force-include deadline mechanics — see Track G issue #103. Replay
defense via three layers: L1 dedup hash, L2 nonce, deadline expiry.

---

## 4. Slashing distribution waterfall (Tokenomics §8.3)

Slashed funds are distributed in this strict order:

1. **Reimbursement of affected counterparties.** Identified by the
   slash trigger (e.g., LTP attestation fraud's affected corridor
   pair; double-spend victim addresses). For surfaces without
   direct counterparty (equivocation, downtime), this step is
   skipped.
2. **Allocation to the insurance pool.** Reserved L1 registry
   account `suwappu_insurance_pool` (per Track C issue #131 wiring).
3. **Allocation to the protocol treasury.** Reserved L1 registry
   account `suwappu_treasury`.

**NOT** in the distribution waterfall:
- **No burn.** Slashed funds always land in one of the three
  destinations above; never destroyed.
- **No direct snitch / whistleblower reward from the slashed bond.**
  Snitch rewards for force-include violations (and similar
  whistleblower paths) are paid from a separate **treasury bounty**
  (typically 5–10 % of the slashed amount, capped at 1 M SUWAPPU per
  event). This is structurally cleaner than the burn-as-incentive
  pattern from the prior draft of the strategic plan.

---

## 5. Detection and adjudication

| Surface | Detector | Adjudicator | Reference impl |
|---|---|---|---|
| Equivocation (validator) | Any node observing two certs from the same authority at the same round | Authority Ring quorum vote | `crates/suwappu-consensus/` |
| Equivocation (fast-path / Authority) | Any node observing two fast-path certs for conflicting transactions | Authority Ring quorum vote | `crates/suwappu-fastpath/` (DAG-S9 exit gate) |
| LTP attestation fraud | Any chain operating an LTP corridor verifier (Ethereum + Solana per Track I) | Authority Ring quorum vote | `crates/suwappu-ltp/` |
| Downtime | Foundation probe + operator-uploaded NDJSON event log | Foundation ops, escalated to quorum vote on second occurrence | `crates/suwappu-validator-program/` |
| Compliance violation | Quarterly compliance attestation review | Foundation board, escalated to quorum vote | Operational (not on-chain) |
| Force-include miss | Any L1 watcher; snitch posts `Intent::SlashSequencer` | Substrate `apply_intent` arm (no quorum vote required; deterministic L1 rule) | Track G issue #103 |

Quorum-vote adjudication uses the existing `Intent::EjectAuthority`
+ `Intent::EjectValidator` (post v1.1 expansion) primitives. The
quorum threshold for slashing is identical to the consensus quorum
(⌈2n/3⌉+1 per IQ-001).

---

## 6. Appeal process

An operator who believes a slashing decision was incorrect has a
**14-day appeal window** post-quorum-vote to:

1. Submit evidence to the foundation board contesting the slash.
2. Request an independent review by a non-conflicted third party
   (e.g., one of the Track A audit firms).
3. If the appeal succeeds, the slashed funds are restored from the
   insurance pool (not from new emissions).

Appeals are **NOT** valid for critical-offense slashing where the
cryptographic proof is on-chain (equivocation, LTP fraud,
certificate fraud, force-include miss). For those, the slash is
automatic and irreversible.

---

## 7. Cross-references

- **Sale subscription agreements** (Track C #128 + #129) — reference
  this doc verbatim.
- **Custody requirements** (Track E #143) — companion doc on
  self-stake key management.
- **Authority Ring resilience** (Track E #146) — quorum-tolerance
  analysis; ties to the standby Authority Node program.
- **L2 sequencer bond** (Track G #103) — full force-include
  mechanics.
- **Slashing distribution wiring** (Track C #131) — engineering
  implementation of the §4 waterfall.
- **Track A audits** — the slashing matrix is in-scope for Trail
  of Bits (consensus, A.2 #114), OtterSec/Halborn (app + economic,
  A.4 #116), and Zellic/Veridise-ZK (L2 sequencer slashing, A.5
  #117).

## 8. Change log

| Date | Change | Source |
|---|---|---|
| 2026-05-16 | Initial draft | E.3 (issue #144), drafted from `suwappu-strategy/docs/mainnet-plan.md` Track E + Tokenomics v2 §8 |
