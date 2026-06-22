# `terraform/testnet/` — incentivized public testnet

7-region foundation-operated seed cluster for the suwappu-testnet
public-L1 program (Track B of the M18–M24 mainnet plan).
External validators connect from their own hardware and earn
points convertible to mainnet token at TGE.

This stack is the **L1 testnet only**. The L2 sequencer + prover
land in a separate follow-up under `terraform/testnet/l2.tf` (Track
G). The points-accumulator daemon lands in
`crates/suwappu-validator-program/` (Track B follow-up).

## What this stack provisions

- **7 seed validators** (us-east-1, us-west-2, eu-west-1,
  eu-central-1, ap-southeast-1, ap-northeast-1, sa-east-1).
  `c7g.xlarge` arm64. 200 GB persistent EBS each. Forks the
  `terraform/devnet/modules/validator/` module directly — no
  fork-and-modify; same module, more regions + bigger instances
  + larger volumes.
- **Artifact bucket** `suwappu-dag-testnet-artifacts` (private; same
  S3 lifecycle as devnet's).
- **External uploads bucket** `suwappu-dag-testnet-validator-uploads`
  (public-WRITE per scoped IAM, public-READ blocked). External
  operators upload rotated `events.ndjson` here every hour;
  the points-accumulator daemon reads + scores.
- **Points-accumulator infra**: `t4g.medium` EC2 in its own VPC
  + a `db.t4g.small` Postgres RDS in private subnets. Idle for
  now; daemon binary deploys via SSM once `crates/suwappu-validator-
  program/` lands.
- **Billing alarm**: $2000/mo cap (4× devnet's $500 because the
  cluster is bigger + the accumulator infra adds RDS cost).

## What this stack does NOT provision (yet)

- **L2 sequencer + prover** → `terraform/testnet/l2.tf` (Track G).
- **DNS + ALB + ACM + WAF** for `*.testnet.suwappu.bot`
  → fork from `terraform/devnet/{dns,acm,alb,waf}.tf` in a
  follow-up PR.
- **CloudWatch dashboard + halt alarm** specific to the testnet
  → fork from `terraform/devnet/cloudwatch.tf` once the testnet
  is live and we know which alarms need re-tuning.
- **Faucet** (testnet faucet is identical to devnet's; defer
  forking until the testnet is actually serving traffic).

These are non-blocking for an initial apply — they layer onto
the running cluster without re-doing the validator infra.

## Apply procedure

Pre-reqs:

```sh
# 1. Mint genesis (7 validators + 1 real faucet authority).
cargo build --release -p suwappu-crypto --bin suwappu-keygen
./scripts/testnet/gen-genesis.py --out-dir ./target/testnet/keys

# 2. Upload to S3 after the first apply creates the bucket.
#    (Same chicken-and-egg as devnet — see DEVNET deploy notes.)
```

Apply:

```sh
BILLING_ALARM_EMAIL=ops@suwappu.bot \
  ./scripts/testnet/deploy.sh apply
```

Post-apply:

```sh
# Render + upload per-region node.toml (peer IPs known only
# after EIPs allocate).
./scripts/testnet/render-configs.sh

# Verify the seed mesh is up.
for ip in $(./scripts/testnet/deploy.sh output -json validators | jq -r '.[].public_ip'); do
  curl -fsS -X POST -H 'Content-Type: application/json' \
       -d '{"jsonrpc":"2.0","id":1,"method":"suwappu_getEpoch"}' \
       "http://$ip:9092/" | jq -r '.result.latest_committed_round'
done
```

Every seed should return a non-zero, monotonically advancing
`latest_committed_round` within 10 minutes.

## Onboarding external operators

```sh
# Once an operator is admitted via governance (see
# OPERATIONS.md § 3), grant them upload credentials:
./scripts/testnet/onboard-operator.sh 8 acme-validator-co
```

This creates a scoped IAM user with `s3:PutObject` rights to
exactly `s3://suwappu-dag-testnet-validator-uploads/uploads/8/*` —
no other AWS access. The script prints the access key + secret
once; send them to the operator out-of-band.

## Destroy is BLOCKED

`./scripts/testnet/deploy.sh destroy` exits non-zero. EBS state
volumes carry `prevent_destroy = true`. The Postgres RDS has
`deletion_protection = true`. To intentionally wipe (e.g. at
mainnet cutover), follow `OPERATIONS.md § "Testnet tear-down"`.

This is intentionally painful. The testnet's chain history
accrues external developers' transactions + the points data
that converts to mainnet token at TGE; losing it would
invalidate months of operator work.

## See also

- [`OPERATIONS.md`](../../OPERATIONS.md) — runbooks. Adapt the
  devnet sections to the 7-region testnet shape.
- [`docs/testnet/VALIDATOR-OPERATORS.md`](../../docs/testnet/VALIDATOR-OPERATORS.md)
  — onboarding guide for external operators.
- [`docs/testnet/POINTS.md`](../../docs/testnet/POINTS.md) —
  public formula for points → mainnet-token conversion.
- [`terraform/devnet/README.md`](../devnet/README.md) — the
  4-region devnet this stack forked from.
