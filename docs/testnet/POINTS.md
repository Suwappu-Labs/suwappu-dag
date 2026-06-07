# Testnet points formula

The points-accumulator daemon (foundation-operated; lands in a
follow-up PR as `crates/suwappu-validator-program/`) computes
per-validator weekly points from event logs uploaded to S3 + the
foundation's own uptime probes.

This document is the **public contract** — what activities earn
points, how points convert to mainnet token at TGE.

## Formula (v1)

Per epoch (≈ 17 minutes at the testnet's 4096 rounds × 250 ms
cadence):

```
points_epoch = uptime_points + cert_points + commit_points
points_week  = sum(points_epoch) + bug_bounty_points + hackathon_points
```

Where:

| Component | Definition | Weight |
|---|---|---|
| `uptime_points` | Foundation probes the validator's RPC every 60 s. ≥ 99% success in the epoch → 100 points. 95–99% → 50. < 95% → 0. | 100 |
| `cert_points` | Count of distinct cert hashes appearing in this validator's `events.ndjson` for the epoch (uploaded to S3 hourly). Divided by 1000. | up to 50/epoch |
| `commit_points` | Count of intents committed where this validator was the leader (per the round-robin schedule). | up to 30/epoch |
| `bug_bounty_points` | Foundation triages each `SECURITY.md` submission. Confirmed bugs award **5,000** (low) / **15,000** (medium) / **50,000** (high) per the published severity matrix. | none/cap |
| `hackathon_points` | Foundation-judged submissions to quarterly hackathons. **1,000–10,000** per accepted submission. | per-event cap |

### Soft caps

- **Per-operator cap**: no single operator may accumulate more
  than **2% of the total testnet allocation** (excluding bug
  bounty). Hard ceiling.
- **Sybil prevention**: KYC at onboarding (one operator = one
  validator). Detected sybils dequeued + banned.

### Conversion to mainnet token

At TGE:

```
mainnet_tokens = (your_total_points / sum(all_operators_points)) × testnet_allocation
testnet_allocation ∈ [5%, 8%] of mainnet supply
```

The exact `testnet_allocation` percentage is set by the
foundation board ≥ 90 days pre-TGE, weighting toward 5% if the
program runs lean (~50 operators) and toward 8% if the program
ends up large (~200+ operators). Published in the token
whitepaper (see `docs/whitepaper/`).

### Audit

- Weekly leaderboard published at
  `https://testnet.suwappu.globalsettlement.com/leaderboard`.
- Per-operator detail page: `/leaderboard/<authority_id>`.
  Includes the raw counts that fed the formula so any operator
  can sanity-check their own.
- Discrepancies: file via the operator Discord `#points` channel
  within 7 days of the weekly publication. Foundation reviews
  + adjusts on a rolling basis.

## v1 scope limits (deliberate)

- **No per-app TVL bonuses.** Encouraging operators to do
  application-side stuff (deploy contracts, lock TVL) creates
  perverse incentives — the points are for VALIDATING, not for
  inflating activity metrics.
- **No referral / multi-account combos.** One operator = one
  validator = one wallet.
- **No off-chain content (Twitter, blog posts, etc.).** Those
  are marketing program rewards — separate from the points
  program.

## v2 considerations (post-mainnet validator-program v2)

- **Stake-weighted points** once real mainnet stake exists.
- **Slashing replay** — points lost for past slashable offences
  surface here too.
- **Decentralized scoring** — currently the daemon is a single
  foundation-operated trust point. Future: scoring runs as a
  multi-party computation across the seed validators.

## Reference implementation

`crates/suwappu-validator-program/` (forthcoming) is the daemon. It
runs on the EC2 in `terraform/testnet/validator-program.tf`,
reads from `s3://suwappu-dag-testnet-validator-uploads/`, writes to
the program RDS. The leaderboard HTTP server reads from the
RDS.

This document is the spec. The implementation MUST match it; any
divergence is a bug, not a "the formula was unclear" defence.

## See also

- [`VALIDATOR-OPERATORS.md`](VALIDATOR-OPERATORS.md) — operator
  onboarding flow.
- [`../../SECURITY.md`](../../SECURITY.md) — disclosure path
  (the only way to earn the bug-bounty tier).
- [`../whitepaper/`](../whitepaper/) — token economic design
  (forthcoming; testnet allocation lives here).
