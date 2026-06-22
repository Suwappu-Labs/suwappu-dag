# Authority Ring resilience + standby program

**Status:** Architectural spec ratified from code audit + the
`suwappu-strategy/docs/mainnet-plan.md` Track E + the sale-model
Risk Factor §20 ("40 super nodes is a concentrated set; loss of
multiple participants could impair authority quorum").

**Audience:** consensus + ops engineers; foundation board; Tier A
Authority Super Node operators.

**Authoritative inputs:**
- `crates/suwappu-authority/src/lib.rs:35-38` — `AUTHORITY_RING_MIN = 30`,
  `AUTHORITY_RING_MAX = 50`
- `crates/suwappu-consensus/src/commit.rs:61-66` — `quorum_threshold(n)`
- `crates/suwappu-ltp/src/lib.rs:41-43` — `LTP_ATTESTATION_QUORUM_THRESHOLD = 7`,
  `LTP_ATTESTATION_QUORUM_SIZE = 9`
- `crates/suwappu-ltp/src/attestation.rs:61-69` — `Corridor` struct
  requires exactly 9 members
- `docs/iq/IQ-001-quorum-formula.md` — ratified quorum formula

---

## 1. Executive summary

The Authority Ring's safety + liveness depends on a **canonical
2f+1 BFT quorum** computed by
`quorum_threshold(n) = n - (n - 1) / 3` (line 65 of
`crates/suwappu-consensus/src/commit.rs`).

For the production active-set sizes:

| Active n | quorum_threshold(n) | Tolerated offline | Notes |
|---|---:|---:|---|
| 30 (Y0 floor) | **21** | 9 | Minimum Ring size per `AUTHORITY_RING_MIN` |
| 40 (Y1 TGE) | **27** | 13 | Sale-model committed set |
| 42 (Y2 baseline) | **29** | 13 | Per sale-model row 80 |
| 50 (Y5 cap) | **34** | 16 | `AUTHORITY_RING_MAX` |

Below the quorum threshold the DAG halts: no certificates finalize,
no L2 state roots commit, no LTP corridor attestations succeed.
Recovery requires **manual reseating** by the foundation, documented
in §6.

The **LTP cross-chain corridor** is a **separate 9-member subset**
of the Authority Ring (`Corridor` struct at
`crates/suwappu-ltp/src/attestation.rs:61-69`), with its own 7-of-9
threshold. Loss of 3+ corridor members halts that specific corridor
but does **not** halt the L1 chain. Multiple corridors run in
parallel; each is its own 9-of-N selection from the Ring.

---

## 2. Code-confirmed invariants

The strategic plan's distinction "Authority Ring ≠ LTP corridor" is
enforced in code:

| Invariant | Source | Status |
|---|---|---|
| Ring size 30 ≤ n ≤ 50 | `crates/suwappu-authority/src/lib.rs:35-38` | Hardcoded `const` |
| Quorum formula = 2f+1 (`n - (n-1)/3`) | `crates/suwappu-consensus/src/commit.rs:65` | Ratified per IQ-001 |
| Corridor size = exactly 9 | `crates/suwappu-ltp/src/lib.rs:43` + `attestation.rs:142-147` | `BadCorridorSize` error rejects ≠ 9 |
| Corridor threshold = 7 | `crates/suwappu-ltp/src/lib.rs:41` + `attestation.rs:150-156` | `BelowQuorum` error rejects < 7 signers |
| Corridor is `Vec<SuperNode>`, `SuperNode.authority: AuthorityId` | `crates/suwappu-ltp/src/attestation.rs:48-69` | Corridor is structurally a subset of the Ring |
| Authority Ring stake threshold = 100,000 SUWAPPU | `crates/suwappu-authority/src/lib.rs:32` | Per Paper §5.1 |
| Tier A self-stake = 15,000,000 SUWAPPU (sale model) | `suwappu-strategy/docs/sources/SUWAPPU-Node-Validator-Sale-Model-v2.0.xlsx` Sale Structure | 150× the Paper §5.1 minimum; commitments to "skin in the game" |

**No code change is required to support n=40 at TGE** — the
production code paths already operate over `n` as a parameter; the
quorum formula scales accordingly.

---

## 3. Quorum-tolerance math

`quorum_threshold(n) = n - (n - 1) / 3` gives the strict 2/3-Byzantine
threshold. Tolerated faults = `n - quorum_threshold(n) = (n - 1) / 3`
(integer floor).

Sale-model active-set trajectory (per row 80 of the sale model):

| Year | Active Authority | Quorum | Offline budget | LTP corridors (9-member) | Per-corridor offline budget |
|---|---:|---:|---:|---|---:|
| Y1 (TGE 2027) | 40 | 27 | **13** | up to ⌊40/9⌋ = 4 simultaneous corridors | 2 |
| Y2 | 42 | 29 | **13** | up to 4 corridors (with 6 floaters) | 2 |
| Y3 | 45 | 31 | **14** | up to 5 corridors | 2 |
| Y4 | 48 | 33 | **15** | up to 5 corridors (with 3 floaters) | 2 |
| Y5 | 50 | 34 | **16** | up to 5 corridors (with 5 floaters) | 2 |

Per-corridor offline budget = `9 - 7 = 2` (`LTP_ATTESTATION_QUORUM_SIZE -
LTP_ATTESTATION_QUORUM_THRESHOLD`). The corridor stays live as long as
**7 of its 9 sworn super-nodes** can sign; loss of 3+ corridor members
forces re-corridor (re-membership selection by Authority Ring governance
vote).

---

## 4. Concentration risk + diversification policy

Sale-model Risk Factor §20: "40 super nodes is a concentrated set; loss
of multiple participants could impair authority quorum."

To stay strictly within the offline budget, the foundation enforces two
diversification constraints **at slot allocation**:

### 4.1 Geographic diversification

- **Tier A: no > 25% in any single region** (i.e., max 10 of 40 in
  any one of AWS regions us-east-1 / us-west-2 / eu-west-1 /
  eu-central-1 / ap-southeast-1 / ap-northeast-1 / sa-east-1 or
  equivalent non-AWS region groupings).
- **Tier B: no > 15% in any single region** (max 30 of 200).

Rationale: if a region-wide outage takes out 25% of Authority Ring (10
of 40), Ring drops to 30 active — at the `AUTHORITY_RING_MIN` floor with
quorum = 21. Within the 13-offline tolerance with zero margin. Hence the
25% cap is tight but workable.

A larger single-region concentration (e.g., 30%) would push the floor
below the offline budget under regional outage and **halt the chain**.

### 4.2 Sector diversification

Tier A buyer mix is structurally diverse per the sale-model target
profile:

| Sector | Target count (of 40) | Cap |
|---|---:|---:|
| Global custody banks | 2-3 | ≤ 30% (12) |
| Settlement infra (Fnality/Onyx/Citi class) | 2-3 | ≤ 30% (12) |
| Stablecoin issuers (Circle/Paxos/Agora/Ripple) | 2-3 | ≤ 30% (12) |
| Exchanges (CME/Cboe/Nasdaq class) | 2-3 | ≤ 30% (12) |
| RWA tokenization platforms | 2-3 | ≤ 30% (12) |
| Payment networks | 2-3 | ≤ 30% (12) |
| Strategic + foundation-anchor | ~22 | (residual) |

**Rationale**: regulatory action against a single sector (e.g., a
sweeping FATF or OFAC ruling on stablecoin issuers) shouldn't take out
more than the offline budget. The 30%-per-sector cap leaves 28 of 40
operational — quorum still met with offline budget = 1.

---

## 5. Standby Authority Node program

Per the sale-model risk-factor mitigation: "**Geographic + sector
diversification; standby authority node program**".

### 5.1 Sizing + scope

- Foundation operates **5-10 standby Authority Nodes** outside the
  sale-cohort allocation
- Standby nodes are **hot-spare ready** to be activated via
  `Intent::AdmitAuthority` within 1 hour of a foundation-board vote
- Standby is **purely backup**: does NOT count toward the
  50-node Y5 cap (`AUTHORITY_RING_MAX`) — standbys cycle in to replace
  ejected sale-cohort slots, not as additive seats
- Foundation operates own self-stake out of the **20% Treasury
  allocation** (separate from the 14% Institutional & Strategic
  Partners allocation that backs sale-cohort buyers)

### 5.2 Activation flow

1. **Trigger event**: any of:
   - Sale-cohort buyer ejected for cause (compliance, equivocation,
     etc. — per `docs/validator-sla-slashing.md` §2)
   - Sale-cohort buyer voluntarily exits (`Intent::ExitAuthority`)
   - Regional / sector outage drops active count toward
     offline budget
2. **Foundation board vote** (M+1 hour): ≥ 3-of-5 board signatures
   on the activation request
3. **Quorum vote on `Intent::AdmitAuthority`** for the chosen standby
   (≥ 27-of-40 of the active Ring sign per IQ-001)
4. **Standby brings up its node** within 1 hour: bootstrap genesis +
   peer mesh + faucet credentials + KYC-rolled self-stake to the
   slashable bond contract
5. **Activated standby joins active set**; if replacing an ejected
   slot, the ejected slot's slashed self-stake follows the
   distribution waterfall in `docs/validator-sla-slashing.md` §4

### 5.3 Standby operator selection

Foundation board selects standby operators with:
- Foundation-controlled HSM custody (no third-party reliance)
- Geographic diversity (standbys placed in regions
  underrepresented in the active set)
- No conflict with sale-cohort sectors (standbys are neutral
  infrastructure providers, not sectoral concentrations)

### 5.4 Standby cost

Standby cost is borne by the foundation (Treasury allocation, not
the Ecosystem allocation). Modeled at ~$200k/yr per standby (similar
to Tier A opex of $250k/yr; standbys run reduced compliance overhead
since they're foundation-internal until activated):

- 5 standbys: ~$1M/yr
- 10 standbys: ~$2M/yr

Annual budget allocated from Treasury (20% of 10B SUWAPPU = 2B SUWAPPU at
$0.10 TGE = $200M reserve, so $1-2M/yr is < 1% of the Treasury and
sustainable for the full M18-M24+ runway).

---

## 6. Recovery from quorum loss

If the active Authority Ring drops below `quorum_threshold(n)` due
to unplanned outage, the chain halts. Recovery requires manual
operator intervention.

### 6.1 Detection

CloudWatch alarm fires when the foundation probe observes:
- < `quorum_threshold(n)` distinct authorities producing certs in
  the last 10 rounds, OR
- L2 sequencer reports no `CommitL2StateRoot` Intent accepted in
  the last 5 batches (downstream symptom)

### 6.2 Triage

1. **Identify which authorities are offline**: foundation probe
   correlates with each operator's reported uptime + AWS region
   status pages
2. **Assess scope**: regional outage vs. simultaneous independent
   failures vs. coordinated equivocation (cryptographic attack)
3. **Activate standby program** (§5.2) if scope justifies

### 6.3 Manual reseating procedure (when standby insufficient)

If standby capacity is exhausted (e.g., regional outage takes out
15 Authority Nodes simultaneously and we have 10 standbys), the
foundation must execute a manual reseating:

1. Foundation board vote (≥ 4-of-5) authorizing emergency reseating
2. Reduce active-set requirement temporarily: `Intent::EjectAuthority`
   for offline operators ⇒ active count drops to e.g. 25 (above
   `AUTHORITY_RING_MIN` of 30 is preferred but emergency may go
   lower)
3. Wait for chain liveness to resume at reduced n
4. As offline operators come back, `Intent::AdmitAuthority` them
   back to the active set
5. Post-incident review: are diversification constraints (§4) tight
   enough? Should standby program scale to >10?

### 6.4 Catastrophic-recovery option

If quorum cannot be restored even with manual reseating (i.e., > 33
of 40 are unreachable simultaneously — a black-swan event), the
foundation may invoke **chain-replay-from-snapshot** per the
existing devnet wipe+regenesis procedure in OPERATIONS.md § "Devnet
wipe+regenesis", adapted for mainnet:

- Pause new tx ingest at the L1 RPC layer
- Publish a state snapshot (last finalized state root from the
  reachable subset)
- Coordinate genesis-style re-bootstrap among reachable operators
- 14-day re-genesis announcement (per Track B re-genesis procedure
  in OPERATIONS.md § "Testnet re-genesis runbook" once Track B.3
  (#120) ratifies it)

This is a **last-resort** option and should never be necessary if
diversification constraints (§4) hold. Document the trigger criteria
in the foundation board's emergency-procedure handbook.

---

## 7. What changes if Authority Ring grows past 50

The `AUTHORITY_RING_MAX = 50` constant at
`crates/suwappu-authority/src/lib.rs:38` is a Paper §5.1 ceiling, NOT
a Mysticeti-C protocol constraint. The consensus paths handle
arbitrary `n`.

To raise the ceiling:
1. Update the `AUTHORITY_RING_MAX` constant (single-line code change)
2. Foundation governance vote (separate IQ document)
3. Coordinate with paper revision (suwappu-papers)
4. Sale-model update + investor disclosure (Track C C.7)

**Recommendation**: do not raise the ceiling in v1. The 30-50 range
captures the institutional-infra positioning; raising it to e.g. 100
shifts suwappu-dag toward Cosmos-validator-set shape and weakens the
"Canton-grade institutional participation" pitch. Revisit in v1.1+
based on demand signal.

---

## 8. Open follow-ups

- [ ] **Integration test**: simulate 14 Authority offline simultaneously
  in the existing 4-node devnet harness (using a scaled-up 40-node
  test, sized down to the test-fixture scale). Verify halt + manual
  reseating recovery procedure works end-to-end. Tracked separately
  if not addressed by Track A.2 (Trail of Bits consensus audit
  surfaces it).
- [ ] **Operations runbook entry**: add this doc's §6 (quorum loss
  recovery) to OPERATIONS.md § "Mainnet emergency procedures"
  (currently §10 is testnet only; mainnet section needed pre-launch).
- [ ] **Validator dashboard** (E.1, #142): expose the offline-budget
  countdown metric per epoch (the "how many more can we lose"
  number).

---

## 9. Cross-references

- **Quorum formula ratification**: `docs/iq/IQ-001-quorum-formula.md`
- **Indirect commit rule**: `docs/iq/IQ-002-indirect-commit.md`
- **Validator SLA + slashing**: `docs/validator-sla-slashing.md`
  (this doc + slashing doc together cover the full operator-facing
  surface)
- **Sale model concentration risk**: `suwappu-strategy/docs/sources/
  SUWAPPU-Node-Validator-Sale-Model-v2.0.xlsx` Risk Factor §20
- **Authority Ring code**: `crates/suwappu-authority/`
- **Consensus code**: `crates/suwappu-consensus/{commit,joint}.rs`
- **LTP corridor code**: `crates/suwappu-ltp/src/attestation.rs`
- **Track A audit scope**: Trail of Bits consensus audit (A.2, #114)
  is the primary owner of this surface

## 10. Change log

| Date | Change | Source |
|---|---|---|
| 2026-05-16 | Initial draft + code audit | E.5 (issue #146); audit confirmed code matches sale-model + plan spec |
