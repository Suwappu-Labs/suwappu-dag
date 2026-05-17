# Delegated Proof-of-Stake (post-TGE permissionless entry)

**Status:** Spec ratified from `gsx-strategy/docs/mainnet-plan.md`
Track E §4.4 + Tokenomics v2 §4.1. **Implementation lands at
v1.1 (M+3 post-mainnet)** — this PR specs the design so the
governance + tokenomics whitepaper can reference a frozen
surface; engineering work happens after mainnet stabilizes
under the 240-node TGE shape.

**Audience:** governance / tokenomics whitepaper authors
(Track C C.7), validator-operator BD (Track C C.6), Standard
Validator (post-TGE permissionless cohort), foundation board.

**Companion docs:**
- `docs/architecture/authority-ring-resilience.md` (E.5) — Ring
  growth trajectory 200 → 500 validators by Y5
- `docs/validator-sla-slashing.md` (E.3) — slashing matrix
  applies identically to delegated stake
- `docs/validator-custody-requirements.md` (E.2) — custody
  bar for Standard Validators matches Tier B

---

## 1. Context

### 1.1 Why permissionless entry post-TGE

The sale-cohort active set at mainnet TGE is **240 nodes**
(40 Authority + 200 Genesis Validators per Track C). By Y5
the Validator Ring grows to **500** per Tokenomics §4.3 cap.
That 300-validator headroom (200 → 500) is filled by
**Standard Validators** — operators who join post-TGE without
a pre-mainnet sale slot.

Standard Validators don't get a slot allocation; they qualify
by **attracting delegated stake from token holders** to reach
the active-set threshold. The delegated-PoS mechanism below
is the on-ramp.

### 1.2 Why this is v1.1

The 240-node TGE shape needs to **stabilize first** under real
load before opening permissionless entry. Premature opening
risks:

- Validator-set churn during the M18–M24 stabilization window
- Slashing-distribution edge cases (per `docs/validator-sla-slashing.md`)
  manifesting on under-tested code paths
- Governance-cap enforcement (0.05% voting per validator,
  Tokenomics §2.4) tested only on the sale-cohort distribution
- Reward-distribution math regressions affecting more parties
  than the foundation can support in incident response

v1.1 timing (M+3 post-mainnet, ≈ M21) gives 3 months of TGE
operation to validate everything end-to-end before
permissionless entry opens.

---

## 2. State surface (Intent variants)

All three new Intents are governance-routine — they go through
the standard `apply_intent` dispatch and land at the next
epoch boundary (matching the existing Phase-G governance
pattern at `crates/gsx-execution/src/substrate.rs:Intent::
AdmitAuthority`).

### 2.1 `Intent::DelegateStake`

Token holder delegates GSX to a Validator-Ring member.

```rust
Intent::DelegateStake {
    delegator: Address,       // address holding the stake
    validator: AuthorityId,   // validator-ring id receiving delegation
    amount: Balance,          // GSX to delegate
}
```

**Substrate effect** (lands in v1.1 engineering):
- Deduct `amount` from `delegator`'s balance (standard
  `credit_unchecked` debit pattern matching C.8)
- Credit a reserved per-validator delegation account
  `delegation_pool(validator)` — derived as
  `BLAKE3("gsx-delegation-pool-v1" || validator_id_le)[..20]`
- Record the delegator→validator→amount triple in the
  delegation registry (state-tree leaf parallel to balances)
- At the next epoch boundary, update the validator's
  effective stake (self_stake + delegated_stake) used for
  quorum-threshold + reward computation

**Reservation invariant**: the per-validator delegation-pool
addresses are added to `crates/gsx-execution/src/reserved.rs`
when this lands, with `is_reserved` extended to recognize
them programmatically (BLAKE3 prefix match) rather than as
fixed constants. This avoids a 500-entry hardcoded list at the
Y5 active-set cap.

### 2.2 `Intent::UndelegateStake`

Token holder withdraws delegation. Subject to a **28-day
unbonding period** (matches Cosmos / Polkadot prior art) to
prevent slash-evasion attacks (withdraw → equivocate → escape).

```rust
Intent::UndelegateStake {
    delegator: Address,
    validator: AuthorityId,
    amount: Balance,
}
```

**Substrate effect**:
- Move `amount` from the active delegation registry to a
  per-delegator unbonding-queue entry
  `(delegator, validator, available_at_epoch)` where
  `available_at_epoch = current_epoch + 28 * (epochs_per_day)`
- Effective stake (used by quorum + rewards) updates at the
  next epoch boundary; reward accrual stops then
- After `28 days`, a separate `Intent::ClaimUnbonded` (or
  automatic substrate sweep at epoch boundary) moves the
  amount back to `delegator`'s balance
- **Slashing during unbonding still applies** — undelegating
  doesn't escape liability for offenses observed before the
  undelegate landed. The 28-day window covers consensus +
  fast-path + LTP fraud-detection latency.

### 2.3 `Intent::SetValidatorCommission`

Validator sets the commission rate on delegated rewards.

```rust
Intent::SetValidatorCommission {
    validator: AuthorityId,
    new_rate_bps: u16,    // basis points, 0..=3000 (0–30%)
}
```

**Substrate effect**:
- Validates `new_rate_bps <= MAX_COMMISSION_BPS = 3000`
- Stores the new rate in the validator's registry record
- Effective at the next epoch boundary
- **Frequency-limited**: at most 1 commission change per
  validator per 7 days (prevents commission front-running of
  reward distribution)

---

## 3. Active-set qualification

A Standard Validator qualifies for the active set when **all
three** of the following hold:

| Requirement | Threshold |
|---|---|
| Self-stake | ≥ **100,000 GSX** (matches Paper §5.1 AUTHORITY_STAKE_THRESHOLD_GSX) |
| Total delegated stake | ≥ **1,000,000 GSX** |
| Compliance attestation | Passed per `docs/validator-custody-requirements.md` (Tier B equivalent) |

The thresholds give the foundation room to admit ~300
Standard Validators between TGE and Y5 (300 × 1.1 M GSX
delegated = ~330 M GSX delegated, well within the
30% Ecosystem allocation's ability to seed-delegate early
validators if needed).

**Tie-breaking** for the 500-slot cap (per Tokenomics §4.3):
when total qualified validators exceed 500, the top 500 by
effective stake (self_stake + delegated_stake) take the
active slots; the rest go on the standby queue and rotate in
as active validators voluntarily exit, get ejected for cause,
or have their stake drop below threshold.

---

## 4. Reward distribution

### 4.1 Per-epoch flow

At each epoch boundary (24h per Tokenomics §4.2):

1. Compute the per-validator emission share:
   `validator_share = emission_pool × (validator_effective_stake / total_active_stake)`
   where `total_active_stake = sum(effective_stake[i] for i in active_set)`.
2. Subtract the validator's **base operating compensation**
   (per Tokenomics §5.4 — Authority nodes get base + perf,
   Standard Validators get pro-rata only).
3. Split the remainder:
   - Validator commission: `validator_share × (commission_bps / 10000)`
   - Delegators (pro-rata by delegated amount):
     `validator_share × (1 - commission_bps / 10000)`
4. Credit each delegator's balance directly (auto-compound is
   v1.2; v1.1 ships uncompounded rewards landing at delegator
   addresses each epoch).

### 4.2 Performance multiplier (Authority Ring only)

Per `docs/validator-sla-slashing.md` §2 Low tier, Authority
Nodes earn a performance multiplier on top of the base
emission share:

- Uptime < 99% → **0.95×** multiplier (loses 5%)
- Uptime < 95% → **0.80×** multiplier (loses 20%)
- Single missed page-response SLA → **0.99×** for the
  affected epoch

Standard Validators do not get a performance multiplier
beyond uptime — the simpler model is appropriate for the
post-TGE permissionless cohort.

---

## 5. Governance voting cap

Per Tokenomics §2.4: **voting power capped at 0.05% per
validator**. The cap applies to the validator's effective
stake when computing governance vote weight.

### 5.1 On-chain enforcement

The voting-weight computation in the governance Intent
handlers (which land in v1.1+ separate from this spec)
applies the cap as:

```
vote_weight = min(
    validator_effective_stake,
    0.0005 × total_active_stake,
)
```

A validator whose effective stake exceeds 0.05% of total
active stake has its vote weight clipped at that cap. The
**excess** is NOT redistributed — it's simply not counted.

### 5.2 Effect on delegated-stake incentives

The cap creates an incentive against extreme stake
concentration:

- A delegator stacking onto a single validator past the cap
  contributes effective stake (for reward purposes) but
  not governance weight (capped)
- Rational delegators spread across multiple validators
  to maximize their governance influence
- The cap is denominated relative to `total_active_stake`,
  so it scales with the network — early-stage 240-node
  network has higher per-validator cap (0.05% of 240
  effective-stake-sum) than the Y5 550-node network

### 5.3 Comparison with Tier A self-stake

A Tier A buyer's 15 M GSX self-stake represents (at 10B total
supply): 0.15% of total supply. Of that, 0.05% is the
governance cap — so Tier A buyers have effective governance
representation but cannot dominate. This is
deliberate: governance is broader than the Authority Ring.

---

## 6. Slashing of delegated stake

Per `docs/validator-sla-slashing.md` §2, slashing is
**proportional** across self-stake and delegated stake:

- Validator equivocates → 100% of self_stake forfeit + 100%
  of delegated_stake forfeit (delegators bear the loss)
- Validator missed force-include → 5–10% of self_stake +
  5–10% of delegated_stake
- Authority compliance violation → up to 100% of self_stake
  + up to 100% of delegated_stake (severity-scaled)

**Delegator risk disclosure**: the slashing-exposure
relationship is explicit. Token-holder UIs (block explorers
+ staking dashboards) MUST surface:

- Which validator a delegator is staking with
- That validator's slashing history
- The current slashing exposure of the delegator's stake

The standard delegated-PoS social contract holds: delegators
share the risk + reward of their chosen validator.

### 6.1 Slashing-distribution waterfall

The Tokenomics §8.3 waterfall (per `docs/validator-sla-
slashing.md` §4) applies identically to slashed delegated
stake:

1. **Reimbursement of affected counterparties** — slashed
   stake first pays out to anyone harmed by the offense (e.g.,
   LTP attestation fraud victims)
2. **Allocation to insurance pool** — `insurance_pool_address`
   (per PR #177)
3. **Allocation to protocol treasury** — `treasury_address`
   (per PR #177)

No portion of slashed delegated stake is returned to the
slashed validator's other delegators (no "social slashing
insurance"). This is intentional: rebating to other
delegators creates moral hazard on validator selection.

---

## 7. Reserved address derivations

Per `crates/gsx-execution/src/reserved.rs` (PR #177) the
slashing-pool + treasury addresses are pinned BLAKE3 derivations.
The delegated-PoS variants extend the pattern with per-
validator pool addresses:

```
delegation_pool_address(validator_id: u32) =
    BLAKE3("gsx-delegation-pool-v1" || validator_id.to_be_bytes())[..20]
```

`reserved::is_reserved` extends to a programmatic check:

```rust
pub fn is_reserved(addr: &Address) -> bool {
    addr == &l2_registry_address()
        || addr == &insurance_pool_address()
        || addr == &treasury_address()
        || is_delegation_pool_address(addr)
}
```

Where `is_delegation_pool_address` validates the address is
the prefix-match of any `delegation_pool_address(i)` for
`i in 0..=max_active_validator_id`. The substrate's
`Transfer` gate rejects any user attempt to mutate
delegation pools via the transfer path.

---

## 8. Migration path from sale-cohort to delegated-PoS

The Tier B Genesis Validators (sale cohort) are NOT
automatically converted to Standard Validators. Tier B keeps
its slot subscription terms (4yr vesting + 6mo cliff). What
changes at v1.1 is:

- **Existing 200 Tier B slots stay locked** to sale buyers
- New Validator-Ring slots open above the 200 cap via the
  Standard Validator qualification path
- Tier B operators MAY accept delegated stake on top of their
  self-stake (subject to the same commission + cap rules)
- Tier B opex assumption ($30k/yr per sale-model) does not
  change

**Authority Ring (Tier A) is NOT subject to delegated
entry** — the 40-slot Tier A allocation is permissioned-only
per Tokenomics §4.3 (PoA tier). Authority Ring growth from 40
→ 50 by Y5 happens via foundation-board admission, not by
delegated qualification.

---

## 9. v1.1 implementation scope (engineering punch list)

The doc above is the spec. The implementation is a series of
follow-up PRs (at M+3 post-mainnet, ≈ M21):

1. **New Intent variants** in `crates/gsx-execution/src/substrate.rs`:
   `DelegateStake`, `UndelegateStake`, `ClaimUnbonded`,
   `SetValidatorCommission` — same `#[non_exhaustive]` pattern
   as G2.1 / G3.1 / C.8
2. **Delegation registry** in `crates/gsx-execution` as a
   parallel state-tree structure (similar to balance map)
3. **Per-validator delegation-pool reserved addresses** in
   `crates/gsx-execution/src/reserved.rs` with the prefix-
   match `is_delegation_pool_address`
4. **Unbonding queue** with epoch-boundary sweep
5. **Voting-cap math** in the governance Intent handlers
6. **Reward distribution** with per-validator commission +
   delegator pro-rata math
7. **Slashing extension** so the §6 waterfall reaches
   delegated stake
8. **Validator dashboard extension** (E.1, #142) to show
   per-validator delegation totals + slashing history +
   commission rate
9. **proptest** coverage for the unbonding queue ordering,
   commission frequency limit, and voting-cap edge cases
10. **Documentation** — update `docs/architecture/validator-rings.md`
    to reference this doc

---

## 10. Cross-references

- **Authority Ring resilience**: `docs/architecture/authority-ring-resilience.md`
  (E.5, PR #174) — Y5 active-set trajectory + standby program
- **SLA + slashing**: `docs/validator-sla-slashing.md` (E.3,
  PR #173) — slashing matrix applies to delegated stake
  identically
- **Custody requirements**: `docs/validator-custody-requirements.md`
  (E.2, PR #181) — Standard Validators meet Tier B custody bar
- **Reserved addresses**: `crates/gsx-execution/src/reserved.rs`
  (C.8 / PR #177) — pattern this doc extends with delegation
  pools
- **Tokenomics whitepaper** (Track C C.7): references this
  doc for the §2.4 voting-cap mechanism + §4.1 DPoS
  description
- **Sale model**: `gsx-strategy/docs/sources/GSX-Node-Validator-Sale-Model-v2.0.xlsx`
  — Tier A / Tier B slot economics this doc preserves

---

## 11. Change log

| Date | Change | Source |
|---|---|---|
| 2026-05-17 | Initial draft | E.4 (issue #145); spec only, implementation v1.1 (M+3 post-mainnet ≈ M21) |
