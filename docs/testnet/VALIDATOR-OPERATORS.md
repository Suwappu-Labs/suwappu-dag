# suwappu-testnet validator operators

This guide is for external operators who want to run a suwappu-dag
testnet validator and earn points that convert to mainnet token
at launch.

For the foundation-internal infrastructure side, see
[`OPERATIONS.md`](../../OPERATIONS.md) (the seed cluster) and
[`terraform/testnet/`](../../terraform/testnet/) (the IaC).

## TL;DR

- You run **one suwappu-node process** on your hardware that peers
  with the foundation's 7 seed validators across the public
  internet.
- You get **points per epoch** for: uptime, certs observed,
  intents committed, and bugs reported via the disclosure
  process in [`SECURITY.md`](../../SECURITY.md).
- Points convert to mainnet token at TGE per the formula in
  [`POINTS.md`](POINTS.md). Total testnet allocation is capped
  at **5–8% of mainnet supply**.

## Eligibility

- Foundation reviews each application via a public form at
  `https://apply.testnet.suwappu.globalsettlement.com`.
- Must run hardware meeting the spec below.
- Must complete KYC (jurisdictional restrictions per the
  foundation's token-distribution policy).
- One operator = one validator. Multiple validators per
  operator are not credited.

## Hardware spec

Minimum:

| Component | Spec |
|---|---|
| vCPU | 16 |
| RAM | 64 GB |
| NVMe SSD | 2 TB (state grows ~10 GB/month) |
| Network | 1 Gbps symmetric, ≤ 50 ms RTT to ≥ 3 of the 7 seed regions |
| OS | Linux x86_64 or arm64; tested on Ubuntu 24.04 LTS |

If you can't hit the network RTT requirement (e.g. you're on
mobile-tier home internet), you'll see more dropped certs and
lower point accrual. Foundation may dequeue persistently
under-performing validators after 30 days; you can re-apply.

## Onboarding flow

1. **Apply** at `https://apply.testnet.suwappu.globalsettlement.com`.
   Foundation runs basic identity + jurisdictional checks.
2. **Foundation submits an `AdmitAuthority` governance Intent**
   with the ML-DSA-65 pubkey you mint locally (next step). Your
   `authority_id` is assigned at this step; it's `≥ 8` (ids 0–6
   are seed validators, 7 is the faucet).
3. **You receive your IAM credentials** (out-of-band, via Signal
   or a 1Password secure share). These let you upload your
   event log to the foundation's S3 bucket; no other AWS access.
4. **Wait one epoch boundary** for the admit Intent to land on
   chain. Once your `authority_id` shows up in
   `suwappu_getAuthorityRegistry`, you're live.

## Local setup

```sh
# 1. Mint your ML-DSA-65 + BLS12-381 keypairs OFF-HOST (e.g. on a
#    fresh laptop you'll later wipe) — these will be your
#    validator's signing keys.
cargo build --release -p suwappu-crypto --bin suwappu-keygen
./target/release/suwappu-keygen --algo mldsa --sk ./mldsa.sk --pk ./mldsa.pk
./target/release/suwappu-keygen --algo bls --sk ./bls.sk --pk ./bls.pk

# 2. Send only the public keys to the foundation (the ML-DSA
#    pubkey goes into the admit Intent). The secret keys NEVER
#    leave your machine.
cat ./mldsa.pk | base64
cat ./bls.pk | base64

# 3. Get your authority_id back from the foundation once admit
#    lands. Then on your validator hardware:

# 3a. Pull the testnet binary release. Tagged releases publish two
#     Linux targets; pick the one that matches your hardware. The
#     seed cluster runs aarch64-linux-gnu on c7g.xlarge — the arm64
#     build is the reference target. The amd64 build uses musl
#     (static-linked) so it boots on Alpine and older-glibc distros
#     without a runtime install. Build matrix:
#     `.github/workflows/release.yml`.
TARGET=aarch64-unknown-linux-gnu   # or x86_64-unknown-linux-musl
gh release download suwappu-dag-v0.X.Y --pattern "*${TARGET}*"
tar -xzf suwappu-dag-0.X.Y-${TARGET}.tar.gz

# 3b. Pull the public testnet genesis. While the wildcard
#     ALB serves 503 (see DEVNET.md § "Public testnet" for the
#     fronting story), pull genesis directly from the artifact
#     bucket via its public-read prefix once foundation
#     republishes it, OR via the URL the foundation sends in
#     your onboarding packet.
curl -fsSL https://testnet.suwappu.globalsettlement.com/genesis.toml \
    -o /etc/suwappu/genesis.toml

# 3c. Write your own node.toml. The seed peer list is in your
#     onboarding packet (foundation hands out the per-region
#     EIPs at admit time; the wildcard endpoint that publishes
#     peers.txt is parked behind the same fronting follow-up).
cat > /etc/suwappu/node.toml <<EOF
self_id = "<your-operator-label>"
authority_id = <your-assigned-id>
listen = "0.0.0.0:9090"
client_listen = "0.0.0.0:9091"
rpc_listen = "127.0.0.1:9092"
metrics_listen = "127.0.0.1:9093"
round_ms = 250
checkpoint_cadence_rounds = 1
mldsa_secret_key_path = "/var/lib/suwappu/mldsa.sk"
bls_secret_key_path = "/var/lib/suwappu/bls.sk"
genesis_manifest_path = "/etc/suwappu/genesis.toml"
event_log_path = "/var/log/suwappu/events.ndjson"

# Pull the current peer list as a starting point. You can prune
# to your 3 closest geographically once latency telemetry lands.
$(curl -fsSL https://testnet.suwappu.globalsettlement.com/peers.txt)
EOF

# 3d. Move your keys into place.
sudo install -m 600 ./mldsa.sk /var/lib/suwappu/mldsa.sk
sudo install -m 600 ./bls.sk   /var/lib/suwappu/bls.sk
sudo chown -R suwappu:suwappu /var/lib/suwappu

# 3e. systemd unit. Adapt the cloud-init from
#     terraform/devnet/modules/validator/cloud-init.yaml.
sudo systemctl enable suwappu-node
sudo systemctl start suwappu-node
journalctl -u suwappu-node -f
```

## Upload events.ndjson for points

The points accumulator daemon (foundation-operated) reads from
`s3://suwappu-dag-testnet-validator-uploads/uploads/<your-authority-id>/`
every 5 minutes. You need to upload your rotated event log
hourly. Use a sidecar cron + the AWS CLI:

```sh
# Pre-req: install awscli with the IAM creds the foundation sent
# you. DO NOT use your foundation creds for anything else.
sudo apt install -y awscli logrotate

# Rotate suwappu-node's events.ndjson hourly via logrotate:
sudo tee /etc/logrotate.d/suwappu-node-events <<'EOF'
/var/log/suwappu/events.ndjson {
    hourly
    rotate 24
    compress
    delaycompress
    missingok
    notifempty
    create 0644 suwappu suwappu
    postrotate
        # Upload the just-rotated file to S3.
        AUTHORITY_ID="$(grep '^authority_id' /etc/suwappu/node.toml | awk '{print $3}')"
        TS=$(date -u +%Y-%m-%dT%H)
        aws s3 cp /var/log/suwappu/events.ndjson.1 \
            "s3://suwappu-dag-testnet-validator-uploads/uploads/$AUTHORITY_ID/$TS.ndjson" \
            --region us-east-1
    endscript
}
EOF
```

The foundation's accumulator reads the latest object per
authority every 5 minutes and updates the leaderboard at
`https://testnet.suwappu.globalsettlement.com/leaderboard`.

## Points formula (summary)

See [`POINTS.md`](POINTS.md) for the full formula + the daemon's
implementation. Headline:

| Activity | Points per epoch |
|---|---|
| Uptime ≥ 99% | 100 |
| Uptime ≥ 95% (no payout below) | 50 |
| Certs observed within 2× median epoch latency | 1 per 1k certs |
| Bugs reported via `SECURITY.md` and confirmed | 5,000–50,000 per bug (severity-dependent) |
| Hackathon submissions accepted | 1,000–10,000 per submission |

Soft caps prevent any single operator from accumulating > 2%
of the total testnet allocation.

## What gets you slashed

- **Equivocation** (signing two conflicting certs at the same
  round). 100% slash of testnet stake, immediate dequeue from
  the program. Per Paper §6.4.
- **Sustained downtime** (uptime < 80% over 14 days). Soft slash:
  loss of all accumulated points for the affected window. Re-
  apply allowed after 30 days.
- **Multiple validators per operator** discovered out-of-band
  (KYC review). Dequeue + ban.

## Communications

- Operator-only Discord channel `#validators` (invite at
  onboarding).
- Foundation publishes weekly Tuesday "validator update" with
  upcoming protocol changes, scheduled maintenance windows, and
  leaderboard highlights.
- Incidents page lives at
  `https://status.testnet.suwappu.globalsettlement.com`.

## Tear-down

If you want to stop operating:

1. Submit an exit ticket in `#validators`.
2. Foundation submits an `ExitAuthority` governance Intent.
3. At the next epoch boundary, your `authority_id` is removed
   from the registry. Stop your `suwappu-node` service.
4. Your accumulated points are preserved through TGE; you
   continue to receive future airdrops per the published
   formula. Wallets used for points payout are KYC-bound.

## See also

- [`POINTS.md`](POINTS.md) — full formula + daemon spec.
- [`../../SECURITY.md`](../../SECURITY.md) — bug-disclosure
  process (the only way to earn the bug-bounty points tier).
- [`../../OPERATIONS.md`](../../OPERATIONS.md) — foundation-side
  runbooks; the validator-restart procedure on the seed cluster
  is also a reasonable template for your own runbook.
- [`../../DEVNET.md`](../../DEVNET.md) — companion devnet
  (smaller, no points, useful for dApp prototyping before
  testnet integration).
