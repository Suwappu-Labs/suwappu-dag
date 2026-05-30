# `terraform/testnet/` — incentivized public testnet

7-region foundation-operated seed cluster for the gsx-testnet
public-L1 program (Track B of the M18–M24 mainnet plan).
External validators connect from their own hardware and earn
points convertible to mainnet token at TGE.

This stack is the **L1 testnet only**. The L2 sequencer + prover
land in a separate follow-up under `terraform/testnet/l2.tf` (Track
G). The points-accumulator daemon lands in
`crates/gsx-validator-program/` (Track B follow-up).

## Status (2026-05-18)

| | |
|---|---|
| Network id | `gsx-testnet-v1` |
| Genesis | minted by `scripts/testnet/gen-genesis.py` with a real ML-DSA-65 faucet key via `gsx-keygen`. |
| Seed validators | **7/7 running**, committing rounds (cross-region; ~500 ms/round measured). |
| Public RPC endpoint | `rpc`/`ws.testnet.*` front via **Global Accelerator → per-region ALB** (`ga.tf`, `regional_alb.tf`, `rpc_certs.tf`) — anycast, <1 min cross-region failover, handles POST (unlike CloudFront, #237). Faucet still via **CloudFront** (`cf_faucet.tf`). See [§ Known limitations](#known-limitations). |
| Faucet binary | live on the faucet EC2, dispensing 100 GSX per drip. |
| Points-accumulator daemon | not yet deployed (crate forthcoming). |

Direct-EIP endpoints (port 9092, JSON-RPC):

```
us-east-1      52.5.240.86
us-west-2      16.148.234.2
eu-west-1      54.73.42.237
eu-central-1   63.185.0.111
ap-southeast-1 18.139.179.124
ap-northeast-1 3.114.228.57
sa-east-1      54.233.81.124
```

Live values are also in `./scripts/testnet/deploy.sh output -json validators`.

## What this stack provisions

- **7 seed validators** (us-east-1, us-west-2, eu-west-1,
  eu-central-1, ap-southeast-1, ap-northeast-1, sa-east-1).
  `c7g.xlarge` arm64. 200 GB persistent EBS each. Instantiates the
  `terraform/devnet/modules/validator/` module — same module,
  more regions, larger volumes. Passes
  `name_prefix = "gsx-devnet-"`, which is historical: the testnet
  went live before the module took a prefix variable, so all
  key_pair / SG / IAM-role / EIP / instance Name fields under
  this stack read `gsx-devnet-<region>` rather than the more
  obvious `gsx-testnet-<region>`. Resources are still scoped to
  this stack via the `Component = testnet` tag from
  `providers.tf`. The rename is deferred to the next testnet
  rebuild (most likely the mainnet cutover) because key_pair / SG
  names are immutable — flipping the prefix would destroy and
  recreate every seed. **Consequence for devnet:** the devnet
  stack uses `gsx-dev-*` instead of `gsx-devnet-*` to avoid
  name collisions in the same AWS account.
- **Artifact bucket** `gsx-dag-testnet-artifacts` (private; same
  S3 lifecycle as devnet's). Stores the gsx-node binary, genesis,
  per-region node.toml, and per-region validator keys.
- **External uploads bucket** `gsx-dag-testnet-validator-uploads`
  (public-WRITE per scoped IAM, public-READ blocked). External
  operators upload rotated `events.ndjson` here every hour;
  the points-accumulator daemon reads + scores.
- **CodeBuild project** `gsx-testnet-build`. Native-compiles
  `gsx-node` / `gsx-loadgen` / `gsx-metrics` to
  `aarch64-unknown-linux-gnu` (the testnet's c7g target) on
  `aws/codebuild/amazonlinux2-aarch64-standard:3.0`. Reuses the
  perf stack's `/gsx-perf/gsx-db-deploy-key` SSM SecureString.
- **Points-accumulator infra**: `t4g.medium` EC2 in its own VPC
  + a `db.t4g.small` Postgres RDS in private subnets. Idle for
  now; daemon binary deploys via SSM once `crates/gsx-validator-
  program/` lands.
- **DNS + ACM + WAF + ALBs** for `*.testnet.gsx.globalsettlement.com`
  (RPC ALB + faucet ALB; HTTPS listener uses the wildcard ACM
  cert validated via Route53). Target groups exist but carry no
  attachments today — see [§ Known limitations](#known-limitations).
- **CloudWatch dashboard + halt alarm + 7 silent-peer alarms**.
  Halt alarm uses `DIFF(MAX(gsx_last_committed_round))` as a
  TimeSeries; fires when MAX did not advance for two consecutive
  1-min windows.
- **Billing alarm**: $2000/mo cap (4× devnet's $500 because the
  cluster is bigger + the accumulator infra adds RDS cost).

## Known limitations

### RPC fronting (resolved): Global Accelerator + per-region ALB

AWS ALB with `target_type = "ip"` **rejects public-IP targets
that are not in the ALB's own VPC, RFC1918, or RFC6598** — even
within the same region. (The devnet header comment in
`terraform/devnet/alb.tf` claimed otherwise; that statement is
wrong, and devnet's apply never reached the attachment step so
nobody noticed.) That left the old single-region RPC ALB target
group empty → 503, with CloudFront as a Phase-1 stopgap.

**Current design** (`regional_alb.tf` + `ga.tf` + `rpc_certs.tf`):
`rpc.testnet.gsx.globalsettlement.com` and `ws.testnet.*` are
served by **AWS Global Accelerator** fronting **one ALB per seed
region**. Each ALB lives *in its validator's own VPC* and targets
the instance in-region (`target_type = "instance"`), which sidesteps
the cross-VPC/public-IP restriction entirely — no VPC peering, so
the repeated `10.43.0.0/16` validator CIDR is a non-issue. The ALB
terminates TLS with a regional ACM cert and carries a REGIONAL WAF
(per-IP rate limit + AWS managed baseline). GA gives one anycast
endpoint, routes each client to the lowest-latency healthy region,
and fails over in < 1 min.

**Why GA, not CloudFront, for RPC:** JSON-RPC is POST, and
CloudFront origin failover only covers GET/HEAD — a regional
outage would break writes with no automatic failover (issue #237).
GA is L4 and reroutes POST too. (The faucet still fronts via
CloudFront — `cf_faucet.tf` — a separate, still-parked decision.)

**Rollback:** `cf_rpc.tf` (the old RPC CloudFront distro) and the
single-region skeleton in `alb.tf` are left in place, unreferenced,
for one release — flip the `rpc`/`ws` aliases in `dns.tf` back to
CloudFront if GA misbehaves. Both are slated for deletion in a
follow-up once GA is verified live.

Original Phase-1/Phase-2 sketch:
`~/.claude/plans/validated-prancing-curry.md`.

### Cloud-init's `awscli` apt path fails on Ubuntu 24.04 noble

`terraform/devnet/modules/validator/cloud-init.yaml` lists
`awscli` under `packages:`. On Ubuntu 24.04 noble the
`awscli` apt package is no longer published; cloud-init logs
`Package awscli is not available`. Consequence: `gsx-bootstrap.sh`
fails on first boot with `aws: command not found`, which leaves
both `gsx-bootstrap.service` and `gsx-node.service` inactive.

Workaround applied to all 7 seeds during the 2026-05-18
bootstrap (and documented in `OPERATIONS.md § 10.1`): SSM-run
a script that installs the official
`awscli-exe-linux-aarch64.zip`, then restarts the two units.

The module needs a fix to pull the official zip in cloud-init
instead of relying on `apt`; otherwise every instance replacement
needs the same one-shot SSM remediation.

## Apply procedure (greenfield)

This is the documented happy-path order. The current testnet was
applied iteratively (the codebuild stack was added after the
first apply); these steps are the consolidated version for the
next greenfield bring-up.

### Pre-reqs

```sh
# 1. AWS profile `gsn` resolves to account 492042618949 with
#    IAMFullAccess attached. The deploy wrapper refuses to act
#    on any other account.
aws sts get-caller-identity --profile gsn

# 2. The gsx-db deploy key must already live in SSM:
aws ssm put-parameter --name /gsx-perf/gsx-db-deploy-key \
    --type SecureString \
    --value "$(cat ~/.ssh/gsx-db-deploy)" \
    --profile gsn --region us-east-1
# (Both terraform/perf and terraform/testnet codebuild jobs read
#  this same parameter — same gsx-db repo, same key.)

# 3. Mint genesis + per-region validator keys + the real ML-DSA-65
#    faucet authority key. gsx-keygen must be on PATH.
cargo build --release -p gsx-crypto --bin gsx-keygen
export PATH="$PWD/target/release:$PATH"
python3 ./scripts/testnet/gen-genesis.py --out-dir ./target/testnet/keys
```

### Apply

```sh
# 4. terraform apply. Creates artifact bucket, VPCs, 7 validator
#    EC2s (each pulls binary + config from S3 at first boot — see
#    next step), CodeBuild project, ALBs, RDS, dashboards, alarms.
BILLING_ALARM_EMAIL=toma@globalsettlement.com \
  ./scripts/testnet/deploy.sh apply
```

Notable resources you can verify after step 4:

| Resource | Verify |
|---|---|
| Artifact bucket | `aws s3 ls s3://gsx-dag-testnet-artifacts/` |
| CodeBuild project | `aws codebuild list-projects ‖ grep gsx-testnet-build` |
| Validator EC2s | `terraform output validators` |
| Halt alarm | `aws cloudwatch describe-alarms --alarm-names gsx-testnet-halt` |

### Upload artifacts to S3

```sh
BUCKET=gsx-dag-testnet-artifacts

# 5. Push genesis + prebalances + per-region validator keys.
aws s3 cp ./target/testnet/keys/genesis.toml \
    s3://$BUCKET/genesis/genesis.toml --profile gsn --sse AES256
aws s3 cp ./target/testnet/keys/prebalances.toml \
    s3://$BUCKET/genesis/prebalances.toml --profile gsn --sse AES256
for r in us-east-1 us-west-2 eu-west-1 eu-central-1 \
         ap-southeast-1 ap-northeast-1 sa-east-1; do
  aws s3 cp ./target/testnet/keys/$r/mldsa.sk \
      s3://$BUCKET/keys/$r/mldsa.sk --profile gsn --sse AES256
  aws s3 cp ./target/testnet/keys/$r/bls.sk \
      s3://$BUCKET/keys/$r/bls.sk --profile gsn --sse AES256
done

# 6. Land the faucet secret key in Secrets Manager (NOT the bucket).
aws secretsmanager put-secret-value \
    --secret-id gsx-testnet/faucet/mldsa-secret-key \
    --secret-binary fileb://./target/testnet/keys/faucet/mldsa.sk \
    --profile gsn --region us-east-1
```

### Build the validator binary (arm64) and push it

```sh
# 7. Package source + run the CodeBuild job. ~10–15 min on a cold
#    Rust cache; subsequent runs ~3–5 min.
./scripts/testnet/build.sh
# Drops aarch64-unknown-linux-gnu binaries at
#   s3://$BUCKET/bin/{gsx-node,gsx-loadgen,gsx-metrics}
# and pulls a local copy to ./target/testnet/.
```

### Render per-region configs + start the seeds

```sh
# 8. Render + upload per-region node.toml (peer IPs known only
#    after step 4 allocated the EIPs).
./scripts/testnet/render-configs.sh

# 9. SSM-bootstrap each seed. The script also patches the
#    cloud-init awscli gap on first run.
./scripts/testnet/ssm-bootstrap.sh    # (helper TBD; see § "ssm-bootstrap.sh" below)
```

If `ssm-bootstrap.sh` doesn't exist yet, the inline command for
each region is:

```sh
PARAM=$(jq -Rs '{commands:[.]}' <<'EOF'
set -ex
if ! command -v aws >/dev/null 2>&1; then
  apt-get update -qq
  apt-get install -y unzip || true
  curl -fsSL https://awscli.amazonaws.com/awscli-exe-linux-aarch64.zip -o /tmp/awscli.zip
  unzip -q -o /tmp/awscli.zip -d /tmp/
  /tmp/aws/install --update
fi
systemctl reset-failed gsx-bootstrap.service gsx-node.service || true
systemctl restart gsx-bootstrap.service
sleep 2
systemctl restart gsx-node.service
EOF
)

# Per region (note: SSM is region-scoped, so the API call goes to
# the validator's region, not us-east-1):
for pair in $(terraform output -json validators \
              | jq -r 'to_entries[]|"\(.key)=\(.value.instance_id)"'); do
  region=${pair%=*}; iid=${pair#*=}
  aws ssm send-command --region "$region" --instance-ids "$iid" \
      --document-name AWS-RunShellScript \
      --parameters "$PARAM" \
      --profile gsn
done
```

### Verify

```sh
# 10. Probe each EIP. Each should advance latest_committed_round
#     within 2 minutes of step 9; cross-region jitter is ≤ 5
#     rounds at steady state.
for ip in $(terraform output -json validators | jq -r '.[].public_ip'); do
  echo "$ip $(curl -fsS --max-time 5 -X POST -H 'Content-Type: application/json' \
       -d '{"jsonrpc":"2.0","id":1,"method":"gsx_getEpoch"}' \
       "http://$ip:9092/" | jq -r '.result.latest_committed_round')"
done
```

### Post-apply one-time DNS step

The testnet zone `testnet.gsx.globalsettlement.com` is in the gsn
account. If `globalsettlement.com`'s apex zone lives in a
different account, paste the `testnet_nameservers` output as NS
records under the apex zone. See `terraform/testnet/dns.tf`.

## Onboarding external operators

```sh
# Once an operator is admitted via governance (see
# OPERATIONS.md § 10.2), grant them upload credentials:
./scripts/testnet/onboard-operator.sh 8 acme-validator-co
```

This creates a scoped IAM user with `s3:PutObject` rights to
exactly `s3://gsx-dag-testnet-validator-uploads/uploads/8/*` —
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

- [`OPERATIONS.md § 10`](../../OPERATIONS.md) — runbooks for the
  live testnet (bootstrap, binary rolling-update, key rotation,
  external-operator onboarding, points daemon deploy).
- [`DEVNET.md`](../../DEVNET.md) — companion devnet quickstart
  and the public testnet endpoint table.
- [`docs/testnet/VALIDATOR-OPERATORS.md`](../../docs/testnet/VALIDATOR-OPERATORS.md)
  — onboarding guide for external operators.
- [`docs/testnet/POINTS.md`](../../docs/testnet/POINTS.md) —
  public formula for points → mainnet-token conversion.
- [`terraform/devnet/README.md`](../devnet/README.md) — the
  4-region devnet this stack forked from.
- [`scripts/testnet/buildspec.yml`](../../scripts/testnet/buildspec.yml) —
  CodeBuild spec for the arm64 validator binary.
