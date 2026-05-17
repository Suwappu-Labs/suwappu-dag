# Validator self-stake custody requirements

**Status:** Spec ratified from `gsx-strategy/docs/mainnet-plan.md`
Track E §custody + sale-model Risk Factor §26 ("Custody of
self-stake — key compromise = total stake loss"). Referenced
verbatim by Tier A + Tier B subscription agreements (Track C
issues #128 + #129).

**Audience:** Tier A Authority Super Node operators, Tier B
Genesis Validator operators, Standard Validators (post-TGE
permissionless cohort), foundation compliance team.

**Companion docs:**
- `docs/validator-sla-slashing.md` — SLA + slashing matrix
  (E.3, PR #173)
- `docs/architecture/authority-ring-resilience.md` — Ring
  quorum tolerance + standby program (E.5, PR #174)

---

## 1. Why this matters

Self-stake is **slashable**. A compromised signing key can be
used to:

- Produce equivocating certificates → **100% safety bond
  forfeiture** (per `docs/validator-sla-slashing.md` §2
  Critical tier).
- Sign LTP attestation fraud → **100% safety bond forfeiture**.
- Produce certificate fraud → **100% safety bond forfeiture**.

At Tier A self-stake of 15 M GSX (≈ $1.5 M at $0.10 TGE / $3.3 M
at $0.22 Y3 base) per slot, a single key compromise costs the
operator the full bond. The custody requirements below are
designed so that "key compromise = total stake loss" is a
**negligible probability event**, not a single-machine
failure mode.

The requirements are graduated by tier:

- **Tier A**: institutional-grade custody. Mandatory.
- **Tier B**: institutional-recommended custody. HSM strongly
  preferred; certified custody partner optional.
- **Standard Validator**: aligned with Tier B (no preferential
  treatment vs sale cohort).

---

## 2. Per-tier custody matrix

### 2.1 Tier A — Authority Super Node

| Requirement | Mandate | Rationale |
|---|---|---|
| Multi-sig | **3-of-5 minimum**, geographically distributed | No single operator (compromised or coerced) can produce a signing event |
| HSM | **FIPS 140-3 Level 3 or better** | Hardware tamper-resistance; key never leaves the device |
| Certified custody partner | **Required** — one of: Fireblocks, BitGo, Anchorage Digital | SOC 2 Type II + insurance + 24/7 SOC + reg-compliant; Track D D.7 certifies these partners pre-mainnet |
| Key rotation | **Quarterly** | Bounds the compromise window |
| Geographic distribution | Signing quorum members ≥ 3 distinct regions | Defense against regional outage + jurisdictional coercion |
| Air-gapped key generation | **Required** | No keypair generated on a network-connected machine |
| Secret backup | **3-of-5 Shamir's Secret Sharing**, geographically distributed across the buyer's compliance custodians | Survives any single backup site failure |
| Incident response | Documented runbook + foundation-board-approved escalation chain | Reduces MTTR on suspected compromise |
| Compliance attestation | **Quarterly review** by foundation + buyer's internal compliance | Ongoing verification, not point-in-time |

**Total self-stake at risk: 15 M GSX per slot.** Custody
hardening is proportional to the stake size; FIPS 140-3 L3 +
multi-sig 3-of-5 + certified partner gets the probability of
loss low enough that the residual risk is acceptable.

### 2.2 Tier B — Genesis Validator

| Requirement | Mandate | Rationale |
|---|---|---|
| Multi-sig | **2-of-3 minimum** | Defense in depth without the Tier-A overhead |
| HSM | **FIPS 140-3 Level 2+ strongly recommended** | Not mandated to keep the bar reachable for solo-operator buyers, but operators without HSM accept proportionally higher residual risk |
| Certified custody partner | Recommended; optional | Smaller stake → smaller absolute loss; some Tier B buyers run their own ops |
| Key rotation | **Annual** | Lower stake → wider rotation window acceptable |
| Geographic distribution | Signing quorum members ≥ 2 distinct regions | Same defense as Tier A, lower bar |
| Air-gapped key generation | **Strongly recommended** | Cloud key generation explicitly permitted but operator accepts residual risk |
| Secret backup | **2-of-3 SSS** or equivalent | At least one backup geographically distinct from primary |
| Incident response | Documented runbook | Foundation provides template |
| Compliance attestation | **Annual** | Less frequent than Tier A |

**Total self-stake at risk: 3 M GSX per slot** (≈ $150 k TGE /
$660 k Y3 base).

### 2.3 Standard Validator (post-TGE permissionless)

Standard Validators are the permissionless on-ramp post-TGE per
Track E `docs/architecture/authority-ring-resilience.md` §1. No
pre-mainnet slot subscription, but the same slashing exposure
on the self-stake portion of their stake.

Custody requirements are **identical to Tier B** — the
foundation does not differentiate sale-cohort vs permissionless
validators on custody.

---

## 3. Key-class taxonomy

Each operator holds **multiple distinct keypairs** for distinct
purposes; mixing them across roles weakens the security posture.

| Key class | Purpose | Slash exposure | Required hot/cold |
|---|---|---|---|
| **Consensus signing key** (ML-DSA-65) | Sign certificates, fast-path attestations, governance votes | 100% safety bond on equivocation | Hot (multi-sig + HSM); must respond within `round_ms` |
| **LTP attestation key** (BLS12-381) | Sign LTP corridor attestations (Tier A super-nodes only) | 100% safety bond on fraud | Hot (multi-sig + HSM) |
| **Self-stake custody key** | Deposit / withdraw / claim self-stake | Indirect (controls the at-risk funds) | Cold (3-of-5 SSS recommended, no signing-machine exposure) |
| **Sequencer signing key** (Track G post-mainnet) | Sign L2 batches | 5–10% liveness bond per occurrence; 100% safety bond on equivocation | Hot |
| **Foundation-admin key** (if granted) | Operate `/admin/*` endpoints on validator-program / faucet / etc. | None (operational role; no on-chain stake) | Warm (HSM but not consensus-tier hot) |

Operators **MUST NOT** reuse the same keypair across classes.
The substrate's reserved-address gate (per `crates/gsx-execution/src/reserved.rs`)
+ the slashing dispatch's per-key-class adjudication assumes
distinct keys.

---

## 4. Audit + attestation cadence

### 4.1 Onboarding attestation (pre-slot-activation)

Before the foundation activates an operator via
`Intent::AdmitAuthority`, the operator submits:

- Multi-sig setup proof: cryptographic proof of N-of-M quorum
  + addresses of all signers + their HSM device serial numbers
- Custody-partner SLA certificate (Tier A only) — issued by
  Fireblocks / BitGo / Anchorage with the operator named
- Air-gapped key generation attestation: signed statement by
  the buyer's compliance officer that all consensus + LTP +
  self-stake keys were generated on an air-gapped machine
- Incident-response runbook + named escalation contacts

Foundation reviews + signs off; rejection or remediation
requests halt the activation.

### 4.2 Ongoing attestation

- **Tier A: quarterly**. Foundation receives:
  - Re-attestation that custody setup is unchanged or
    declaring any material changes
  - HSM firmware version + last-update date
  - Last key-rotation date + reason
  - Incident log (zero-knowledge: "no incidents" or
    "N incidents, redacted detail per buyer's compliance")
- **Tier B / Standard: annual**, same fields.

Failure to attest by deadline triggers the SLA matrix's
Medium-severity penalty (per `docs/validator-sla-slashing.md`
§2: "Failed compliance re-attestation — 10% per quarter
missed").

### 4.3 Compromise disclosure

Per `docs/validator-sla-slashing.md` §2 High tier (key
compromise + delayed disclosure → 100% if disclosure > 24h
after detection):

- Operator MUST notify foundation security@ within **24 hours**
  of suspected or confirmed compromise
- Foundation triggers emergency rotation procedure (see §6)
- Slashing of the safety bond is **suspended pending
  investigation** if disclosure was within 24h
- Slashing of the safety bond is **automatic** if disclosure
  was after 24h (regardless of what the compromise actually
  did or didn't enable)

---

## 5. Certified custody-partner integration paths

Each Tier A buyer must integrate with at least one of the
three certified partners (per Track D D.7 certification work).

### 5.1 Fireblocks

- Add gsx-dag as a supported network in Fireblocks' Network
  Connector (foundation engagement, M+0 to M+3)
- Consensus signing key + LTP attestation key + self-stake
  custody key all custodied via Fireblocks vault accounts
- MPC-based signing replaces HSM for some buyer profiles
  (Fireblocks MPC + GP DLPolicy)
- SOC 2 Type II shared at engagement time

### 5.2 BitGo

- BitGo Connect partnership; gsx-dag as supported network
- Cold-storage HSM for self-stake custody
- Multi-user signing policies match Tier A 3-of-5 quorum
- Insurance coverage available (rider-pricing per stake size)

### 5.3 Anchorage Digital

- Regulated US custodian (OCC trust charter)
- Preferred for US institutional buyers + regulated entities
- SOC 2 Type II
- Insurance coverage included

**Secondary partners** (recommended but not yet certified, M+6+ work):

- Copper (EU + APAC coverage)
- Hex Trust (APAC)
- Coinbase Custody Trust Company

---

## 6. Emergency key-rotation procedure

If an operator suspects key compromise, the procedure is:

1. **Immediate (T+0):** operator notifies foundation
   security@globalsettlement.com + the operator's compliance
   officer + the buyer's custody partner.
2. **T+15 min (Tier A) / T+30 min (Tier B):** operator
   freezes consensus participation via a signed `Intent::
   ExitAuthority` from the multi-sig (voluntary withdrawal,
   no slashing trigger).
3. **T+1 hour:** foundation activates a standby Authority
   Node (per `docs/architecture/authority-ring-resilience.md`
   §5) to backfill the active set; quorum remains intact.
4. **T+24 hours:** operator publishes preliminary incident
   report; foundation begins compromise-disclosure clock.
5. **T+7 days:** operator submits new keys via the onboarding
   attestation flow (§4.1); foundation re-admits the operator
   via `Intent::AdmitAuthority` if attestation passes.
6. **T+30 days:** post-incident review + foundation-board
   ratification of the operator's continued slot ownership.

Each step has documented evidence requirements; the operator's
incident-response runbook (per §2.1 / §2.2) must reference this
procedure verbatim or supersede it with stricter measures.

---

## 7. Insurance recommendations

The foundation does not directly underwrite operator key-
compromise risk, but the following providers offer relevant
coverage:

- **Coincover** — key-management coverage for institutional
  custodians + HSM-based operators
- **Aon** — cyber insurance with crypto-custody riders
- **Munich Re** — institutional crypto coverage
- **Marsh** — broker-of-record for foundation-board-approved
  underwriters

**Tier A buyers SHOULD carry insurance** covering at minimum
50% of self-stake notional value. Tier B insurance is
optional. The foundation may at its discretion require
insurance evidence as part of onboarding attestation (§4.1).

---

## 8. Cross-references

- **Slashing matrix**: `docs/validator-sla-slashing.md` (E.3,
  PR #173) — what compromise costs
- **Authority Ring resilience**: `docs/architecture/authority-ring-resilience.md`
  (E.5, PR #174) — quorum tolerance during compromise +
  standby program
- **Sale subscription agreements**: Track C #128 (Tier A
  data-room outreach) + #129 (Tier B sale execution) reference
  this doc verbatim
- **Custody-partner certification**: Track D D.7 (#138) —
  Fireblocks / BitGo / Anchorage certification work
- **SOC 2 attestation**: Track D D.8 (#139) — foundation-level
  SOC 2 Type II covering this doc's compliance attestations
- **Reserved-address gate**: `crates/gsx-execution/src/reserved.rs`
  (PR #177) — substrate-level enforcement of key-class
  separation

---

## 9. Change log

| Date | Change | Source |
|---|---|---|
| 2026-05-17 | Initial draft | E.2 (issue #143); drafted from `gsx-strategy/docs/mainnet-plan.md` Track E + sale-model Risk Factor §26 |
