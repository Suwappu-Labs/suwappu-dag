# Testnet re-genesis runbook

**Status:** Spec ratified from `suwappu-strategy/docs/mainnet-plan.md`
Track B §"Re-genesis cadence" + the existing devnet wipe+regenesis
procedure in `OPERATIONS.md`. Companion to `docs/testnet/POINTS.md`
(points-program continuity across re-genesis events).

**Audience:** foundation ops + foundation board (re-genesis
authorization), Tier B Validator-Operators (cluster restart),
external testnet developers (announcement window).

**Authoritative inputs:**
- `suwappu-strategy/docs/mainnet-plan.md` Track B §"Re-genesis cadence"
- `OPERATIONS.md` § "Devnet wipe+regenesis"
- `docs/architecture/authority-ring-resilience.md` §6.4 — the
  catastrophic-recovery option for mainnet (this doc covers
  the planned testnet equivalent)
- `docs/testnet/POINTS.md` — points-program reset rules

---

## 1. When re-genesis is appropriate

Re-genesis is a **scheduled** procedure that wipes testnet
state and bootstraps a fresh genesis. It's the right tool for:

| Scenario | Trigger |
|---|---|
| Major protocol upgrade requiring a hard fork | Foundation board ratifies the upgrade via IQ |
| Validator-set redesign (e.g., expanding the sale-cohort allocation) | Track C governance decision |
| State-tree corruption surfaced by audit (Track A) | Audit-firm critical finding |
| L2 STM circuit upgrade (Track G) | Phase G2 / G4 production ratification |
| End-of-quarter cleanup if accumulated state degrades operator experience | Foundation-board ops review |

Re-genesis is **NOT** appropriate for:
- Routine bug fixes (use hot rollouts via the existing
  RELEASING.md process)
- Single-validator issues (use `Intent::EjectAuthority` per
  E.5 standby program)
- Minor governance changes (use normal governance Intents)
- Mainnet emergency recovery — see
  `docs/architecture/authority-ring-resilience.md` §6.4 for
  the mainnet catastrophic-recovery procedure, which is a
  separate workflow with stricter constraints

## 2. Cadence + announcement window

Per Track B §"Re-genesis cadence":

- **L1 testnet: at most once per quarter** (3 months between
  re-genesis events)
- **L2 testnet: more frequent during sequencer maturation**
  (Track G v1 may re-genesis monthly until 20k TPS sustained
  30d milestone hits)
- **Announcement window: ≥ 14 days** advance notice required
  on the public communication channels (Discord, X, testnet
  status page)
- **Reserved exception**: critical-severity audit findings
  may compress the window to 72 hours with foundation-board
  vote

The announcement window is enforced by the
`Intent::ReGenesis` substrate gate (§4 below): an Intent
landing < 14 days before `target_genesis_at` is rejected
unless the embedded foundation-board signature carries the
critical-severity emergency flag.

## 3. Pre-flight checklist (T-14 days through T-0)

### T-14 days: announcement + freeze prep

- [ ] Foundation board ratifies the re-genesis via IQ
- [ ] Foundation publishes the announcement on:
  - `https://status.testnet.suwappu.bot`
  - `https://blog.suwappu.bot/announcements/`
  - Discord `#announcements` channel
  - X account (@suwappubot)
  - Email to all registered Tier B Validator-Operators
- [ ] Announcement includes: rationale, target re-genesis
  time, expected downtime, state-snapshot URL, point-program
  conversion treatment

### T-7 days: state snapshot capture

- [ ] CloudWatch metric snapshot: full per-region latency,
  throughput, validator-uptime histograms at the moment of
  snapshot
- [ ] Validator-program leaderboard snapshot: full points
  table per operator, archived to
  `s3://suwappu-dag-testnet-archive/leaderboard/<re-genesis-id>/`
- [ ] L1 state snapshot: last committed checkpoint's full
  state-tree (balances + delegation registry once v1.1 ships
  + auth/validator registries) published as a downloadable
  `.bincode` artifact under
  `https://testnet.suwappu.bot/snapshots/`
- [ ] L2 state snapshot: same shape for the L2 MPT root +
  cert registry + nullifier set (once Track G lands)
- [ ] Per-operator events.ndjson archive cutoff (operators
  notified to perform their final upload before T-0)

### T-72 hours: faucet + RPC throttle

- [ ] Faucet drip rate halved (signals approaching wind-down)
- [ ] Public RPC layer flips to read-only mode for new
  sessions (existing sessions allowed to finish; new
  `suwappu_submitIntent` calls rejected with 503 + the
  re-genesis announcement URL in the error body)
- [ ] Status page updates to "RE-GENESIS WINDOW"

### T-24 hours: validator-cluster prep

- [ ] All 7 seed validators receive the new genesis manifest
  via the existing
  `https://testnet.suwappu.bot/genesis.toml`
  endpoint (rotated to a new path so old endpoint stays
  serving the current genesis for last-minute readers)
- [ ] All Tier B Validator-Operators receive the new
  manifest + bootstrap procedure via their secure-comms
  channel (S3 IAM credentials per
  `docs/testnet/VALIDATOR-OPERATORS.md`)
- [ ] Pagerduty primary on-call confirmed
- [ ] Foundation board signs the `Intent::ReGenesis` (see
  §4) for landing at the boundary epoch
- [ ] Last sanity check: state snapshot integrity
  (hash matches the published `.sha3` companion file)

## 4. `Intent::ReGenesis` (governance-gated)

The re-genesis trigger is an on-chain Intent that lands at the
final epoch boundary before the new genesis. Authority Ring
quorum (≥ 27-of-40 per IQ-001) must sign the Intent for it to
be accepted by the substrate.

```rust
Intent::ReGenesis {
    new_genesis_hash: [u8; 32],
    target_genesis_at: u64,           // unix seconds
    state_snapshot_uri: String,       // s3://… or https://…
    rationale_url: String,            // public-docs URL
    emergency_flag: bool,             // bypasses 14-day window
    authority_signatures: Vec<AuthoritySignature>,  // ≥ 27 of 40
}
```

**Substrate effect** (lands in v1.1 engineering alongside
the rest of the Phase G governance work):

- Validates `authority_signatures.len() >= quorum_threshold(n_active_authorities)`
- Each `AuthoritySignature` is verified as an ML-DSA-65
  signature over `blake3(new_genesis_hash || target_genesis_at
  || rationale_url)`
- Validates the 14-day window UNLESS `emergency_flag` is true
  AND the signature count is ≥ 33 (≥80% of 40, super-quorum
  threshold for emergency bypass)
- On accept, emits `Event::ReGenesisScheduled` and ALL
  subsequent block production halts at `target_genesis_at` —
  no new certificates are produced after that point

This is a **hard fork** at the chain level. Operators who
don't bootstrap the new genesis stay on the old fork (which
will continue existing as a stalled chain) but cannot
re-sync to the new chain without the new manifest.

## 5. Cutover sequence (T-0)

At `target_genesis_at`:

1. **Sequencer (Track G):** if L2 testnet has a live sequencer,
   it submits its final `Intent::CommitL2StateRoot` for the
   last batch before stopping
2. **L1 final checkpoint:** Authority Ring co-signs the final
   checkpoint at `last_round_before_genesis`. This checkpoint's
   hash is published as the chain's terminal state for
   historical-proof purposes
3. **All seed validators stop their suwappu-node systemd unit**
   (via the existing `deploy.sh stop` flow)
4. **All Tier B Validator-Operators stop their nodes** within
   the 30-minute cutover window (per the announcement)
5. **Foundation publishes the new genesis manifest** at the
   canonical URL
6. **Foundation operates the new seed mesh first** (T+0 to
   T+10 min) to ensure quorum forms cleanly
7. **Tier B operators bootstrap against the new genesis** as
   they come online (T+10 min onwards)
8. **Foundation watches the CloudWatch dashboard** for first
   `latest_committed_round` advancement past round 0 on each
   seed
9. **Faucet returns to normal drip rate** once 7-of-7 seeds
   confirm round 50+ committed (i.e., ~12s of advancing
   chain at 250ms rounds)

Estimated total cutover time: **30–60 minutes** for the
foundation-operated seed mesh; **2–4 hours** for the full
Tier B cohort to come back online.

## 6. Points-program reset rules

Per `docs/testnet/POINTS.md`, the points program survives
re-genesis events:

- **Cumulative points are preserved** — re-genesis does not
  reset the operator's lifetime points total
- **Per-epoch rollups stay archived** — the `epoch_points`
  table for the pre-genesis epoch range is retained
- **Cert observations across the genesis boundary**: certs
  observed in the OLD chain count toward the OLD chain's
  final epoch; certs observed in the NEW chain count toward
  the NEW chain's first epoch. No cross-genesis correlation
- **Points conversion at TGE** uses the cumulative total
  across all re-genesis events
- **The `suwappu-validator-program` daemon doesn't auto-reset**:
  the foundation MUST manually flush the `epoch_points` and
  `uptime_samples` tables for samples taken after the
  re-genesis boundary (a `POST /admin/regenesis` endpoint
  is a follow-up if re-genesis becomes routine)

## 7. State-snapshot retention

The last **4 re-genesis epoch snapshots** are retained
indefinitely in S3 for forensic + historical proof purposes:

- Path: `s3://suwappu-dag-testnet-archive/snapshots/<re-genesis-id>/`
- Contents: full state-tree, validator registry, leaderboard,
  CloudWatch metrics, announcement record, IQ ratification,
  `Intent::ReGenesis` payload + signatures
- Retention: indefinite for the last 4; older snapshots may
  be archived to S3 Glacier per the foundation's
  cost-management policy

The 4-snapshot floor (vs strict "last N") is per the
sale-model risk-factor disclosure that historical state must
be auditable for at least 12 months post-event.

## 8. Communication templates

### 8.1 T-14 day announcement (public)

> **SUWAPPU Testnet Re-Genesis: <Date> <Time UTC>**
>
> The Suwappu Labs foundation will perform a
> scheduled re-genesis of the SUWAPPU testnet on **<date>** at
> **<time> UTC**. This event is governance-ratified under
> IQ-<NNN> with the rationale: **<one-paragraph rationale>**.
>
> ### What this means for you
>
> - **All testnet state will be wiped.** Account balances,
>   contract state, and pending transactions on the existing
>   testnet will not carry over.
> - **Cumulative validator-program points are preserved.**
>   See [docs/testnet/POINTS.md](https://github.com/Suwappu-Labs/suwappu-dag/blob/main/docs/testnet/POINTS.md).
> - **The faucet drip rate will halve at <T-72h>** and the
>   public RPC will enter read-only mode at <T-72h>.
> - **Validator-Operators will receive the new genesis
>   manifest via their normal secure-comms channel by
>   <T-24h>**.
>
> Tier B Validator-Operators: please ensure your event-log
> uploads are flushed before <T-24h>.
>
> Questions: support@suwappu.bot.

### 8.2 T-0 cutover notice (validator-operators)

> **SUWAPPU Testnet Re-Genesis cutover beginning now.**
>
> All foundation seed validators are stopping their nodes.
> The new genesis manifest is available at
> `https://testnet.suwappu.bot/genesis-<re-genesis-id>.toml`.
>
> ### Operator action
>
> 1. Stop your suwappu-node systemd unit:
>    `sudo systemctl stop suwappu-node`
> 2. Backup your current state directory:
>    `mv /var/lib/suwappu/state /var/lib/suwappu/state.pre-<re-genesis-id>`
> 3. Download the new genesis manifest and peers list:
>    `curl -fsS https://testnet.suwappu.bot/genesis-<re-genesis-id>.toml > /etc/suwappu/genesis.toml`
>    `curl -fsS https://testnet.suwappu.bot/peers-<re-genesis-id>.txt > /etc/suwappu/peers.txt`
> 4. Restart: `sudo systemctl start suwappu-node`
> 5. Confirm `latest_committed_round` advances past 50
>    within 30 min: see the validator dashboard at
>    `https://explorer.testnet.suwappu.bot/validator/<your-id>`
>
> Foundation primary on-call: <pagerduty-link>.

## 9. Post-cutover review

Within 72 hours of cutover the foundation publishes:

- **Re-genesis post-mortem** documenting any operational
  issues + their resolution
- **Operator-cluster recovery time** histogram across the
  Tier B + Standard Validator cohort
- **Points-program adjustment** if any operator's points
  were materially affected by the cutover (e.g.,
  prolonged ingest gap that wasn't the operator's fault)
- **IQ-NNN closure** — the originating IQ is marked
  closed with the actual cutover-time evidence

## 10. Mainnet difference (forward reference)

**This runbook is testnet-only.** Mainnet has no equivalent
"re-genesis" workflow — mainnet state is permanent. The
mainnet equivalent for catastrophic recovery is documented
separately at `docs/architecture/authority-ring-resilience.md`
§6.4 (the catastrophic-recovery option), which has stricter
constraints:

- Requires ≥ 80% Authority Ring board vote (vs the
  testnet's 27-of-40 quorum)
- Requires reachable-validator quorum verification
- Requires a 7-day re-genesis announcement window (vs
  testnet's 14-day; emergency)
- No state-snapshot artifact published (mainnet snapshots
  are governed by the audit + foundation-board rules at
  Track A.4)

The testnet procedure exists primarily so the foundation
+ operators have practiced the mechanic before mainnet
launches — the M15 hard-fork dry run (Track B.4 #122) uses
this exact procedure as its rehearsal target.

---

## 11. Cross-references

- **Track B.4 hard-fork rehearsal**: `#122` — uses this
  procedure for the M15 dry run
- **Authority Ring resilience**: `docs/architecture/authority-ring-resilience.md`
  §6.4 — mainnet catastrophic-recovery (analogous procedure
  with stricter constraints)
- **Validator-program continuity**: `docs/testnet/POINTS.md` —
  points-program reset rules across re-genesis
- **Operations runbooks**: `OPERATIONS.md` § "Devnet
  wipe+regenesis" — the precursor pattern this runbook
  generalizes
- **Sale model**: `suwappu-strategy/docs/sources/SUWAPPU-Node-Validator-Sale-Model-v2.0.xlsx`
  Risk Factor (operational risk) — names testnet re-genesis
  as a known recoverable event class
- **Substrate Intent variant**: `Intent::ReGenesis` lands in
  the v1.1 governance Intent batch (not part of the
  current Track G / H / I engineering scope)

---

## 12. Change log

| Date | Change | Source |
|---|---|---|
| 2026-05-17 | Initial draft | B.3 (issue #121); spec only, `Intent::ReGenesis` engineering lands with v1.1 governance batch |
