# `terraform/devnet/` — public devnet infra

4-region always-on devnet that external developers point their SDKs at.

This stack is the **operational** layer; the **DNS + ALB + WAF** layer
that fronts these validators lives in [G2 — to be added in a follow-up
PR for `terraform/devnet/{dns,alb,acm,waf}.tf`].

## What this stack provisions

- 4 EC2 instances (`t4g.medium`, arm64) one per region:
  - `us-east-1` (authority_id = 0)
  - `eu-west-1` (authority_id = 1)
  - `ap-southeast-1` (authority_id = 2)
  - `sa-east-1` (authority_id = 3)
- Per-instance:
  - Public EIP (validators reach each other by EIP).
  - 50 GB gp3 EBS attached at `/dev/sdf`, mounted at `/var/lib/suwappu`.
    Carries consensus state + `events.ndjson` + the per-region
    ML-DSA/BLS secret keys. `prevent_destroy = true` — accidental
    `terraform destroy` cannot wipe it.
  - Security group opens ports 9090 (peer), 9091 (client), 9092
    (RPC) to `0.0.0.0/0`. Port 9093 (Prometheus metrics) is
    LOCALHOST-ONLY.
  - IAM role with S3 read access to `suwappu-dag-devnet-artifacts`,
    `s3:PutObject` to the `logs/` prefix, SSM Session Manager,
    and CloudWatch agent push.
- Shared `suwappu-dag-devnet-artifacts` S3 bucket (us-east-1) with
  lifecycle rules for CodeBuild sources + log tiering.
- CloudWatch billing alarm at `$500/mo` published to an SNS topic;
  ops email is the only subscriber.

## What this stack does NOT provision (G2+)

- Route53 records / ACM cert / ALB fronting the 4 validators →
  pending in `terraform/devnet/{dns,acm,alb,waf}.tf`.
- Faucet EC2 instance + ALB + DNS record → G3.
- Block explorer S3 + CloudFront → G7.
- Status page S3 + CloudFront + API Gateway → G8.
- CloudWatch dashboard + halt/silent-peer/faucet alarms → G6
  (the agent + Prometheus exporter are already wired in via
  cloud-init.yaml; G6 only adds the dashboard + alarm resources).

## Apply procedure

Prerequisites:

```sh
# 1. Real ML-DSA-65 faucet keypair — only the faucet authority uses a
#    real key. Validator-side keys stay as placeholders for now.
cargo build --release -p suwappu-crypto --bin suwappu-keygen  # if not yet built
./scripts/devnet/gen-genesis.py --out-dir ./target/devnet/keys

# 2. Upload genesis + keys to S3. After `terraform apply` runs at least
#    once, the bucket exists. First apply:
#       a. Comment out the cloud-init bootstrap.sh's aws s3 cp lines.
#       b. Apply terraform.
#       c. Upload binary + genesis + keys + per-region configs.
#       d. Uncomment cloud-init and re-apply (forces user-data update +
#          instance replacement; persistent EBS survives).
#    OR run the bootstrap once manually:
aws s3 cp ./target/<arch>/release/suwappu-node \
    s3://suwappu-dag-devnet-artifacts/bin/suwappu-node \
    --profile gsn
aws s3 sync ./target/devnet/keys/ s3://suwappu-dag-devnet-artifacts/keys/ \
    --profile gsn
aws s3 cp ./target/devnet/keys/genesis.toml \
    s3://suwappu-dag-devnet-artifacts/genesis/genesis.toml --profile gsn
```

Apply:

```sh
BILLING_ALARM_EMAIL=ops@suwappu.bot \
  ./scripts/devnet/deploy.sh apply
```

Post-apply:

```sh
# Render + upload per-region node.toml (peer IPs known only after apply).
./scripts/devnet/render-configs.sh

# Confirm the SNS subscription email; the billing-cap alarm is muted
# until subscription is confirmed.

# Verify mesh is up.
for ip in $(./scripts/devnet/deploy.sh output -json validators | jq -r '.[].public_ip'); do
  curl -fsS -X POST -H 'Content-Type: application/json' \
       -d '{"jsonrpc":"2.0","id":1,"method":"suwappu_getEpoch"}' \
       "http://$ip:9092/" | jq -r '.result.latest_committed_round'
done
```

Every node should return a non-zero, monotonically advancing
`latest_committed_round` within 10 minutes.

## Destroy is BLOCKED

`./scripts/devnet/deploy.sh destroy` exits non-zero. EBS state
volumes carry `prevent_destroy = true`. To intentionally wipe:

1. Snapshot every state volume:
   `aws ec2 create-snapshot --volume-id <id> --description 'pre-wipe'`.
2. Edit `terraform/devnet/modules/validator/main.tf`: remove
   `prevent_destroy = true` on `aws_ebs_volume.state`.
3. `terraform apply` to update the lifecycle setting.
4. `scripts/deploy-aws.sh destroy devnet`.

This is intentionally painful. The devnet's chain history accrues
external developers' transactions; losing it surfaces as 404s in their
explorer queries and broken SDK examples.

## See also

- `OPERATIONS.md` (repo root) — runbooks for restart, key roll,
  binary update, snapshot+restore, emergency stop.
- `docs/devnet/genesis.toml` — checked-in canonical genesis (the
  `gen-genesis.py` script generates this; it's committed so devs
  can compare against what the validators actually loaded).
- `docs/devnet/faucet-key-ceremony.md` — how the faucet's ML-DSA
  key was minted, where the matching pubkey lives in genesis, and
  how to roll it (G3 + G5).
- [Plan: G1–G8](../../).
