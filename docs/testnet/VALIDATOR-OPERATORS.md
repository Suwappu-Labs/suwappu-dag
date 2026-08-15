# suwappu-testnet validator operators

> ## ⚠️ STATUS: the testnet is NOT live yet (as of 2026-08-15)
>
> This document describes the **intended** operator program. Read this
> box before acting on anything below.
>
> **Not yet real:**
> - **There is no network.** Zero seed validators are running; every
>   `testnet.suwappu.bot` URL in this guide (genesis, peers, apply,
>   status) does not resolve yet.
> - **No release has been cut** — `git tag` is empty, so there is
>   nothing to `gh release download`.
> - **The apply form, the Discord channel and the leaderboard do not
>   exist.**
> - **The points→token conversion is an open decision, not a
>   commitment.** `POINTS.md` and the deferred-token architecture have
>   not been reconciled (`docs/testnet/LAUNCH-STATUS.md`, human-action
>   item 6). Treat every number in "Points" below as a draft proposal.
>   **Nothing here is an offer, allocation, or promise of any token.**
> - The seven-region seed topology below assumed AWS, which the project
>   no longer has. The live plan is now
>   [`NON-AWS-DEPLOY.md`](NON-AWS-DEPLOY.md), which starts from a much
>   smaller footprint.
>
> **Already real** (landed and CI-verified): the post-genesis join path
> — `allow_post_genesis_join`, wire sync (`GetTip`/`GetCertsByRound`/
> `GetBlock`), dynamic inbound peers, and the two-distinct-authority
> `AdmitAuthority` governance rule. The mechanism described in
> "Onboarding flow" is implemented; the *program* around it is not.
>
> Track the remaining blockers with `/goal`, or in
> [`LAUNCH-STATUS.md`](LAUNCH-STATUS.md).

This guide is for external operators who want to run a suwappu-dag
testnet validator.

For the foundation-internal infrastructure side, see
[`OPERATIONS.md`](../../OPERATIONS.md) (the seed cluster) and
[`NON-AWS-DEPLOY.md`](NON-AWS-DEPLOY.md) (the current, non-AWS standup
plan). `terraform/` is retained only as a record of the retired AWS
design.

## TL;DR

- You run **one suwappu-node process** on your hardware that peers
  with the foundation's seed validators across the public internet.
  (Planned topology was 7 regions on AWS; the actual initial footprint
  will be smaller — see the status box.)
- You get **points per epoch** for: uptime, certs observed,
  intents committed, and bugs reported via the disclosure
  process in [`SECURITY.md`](../../SECURITY.md).
- **Proposed, not committed:** points may convert to mainnet token at
  TGE per the formula in [`POINTS.md`](POINTS.md), under a testnet
  allocation discussed at 5–8% of mainnet supply. This has **not** been
  decided — see the status box.

## Eligibility

- Foundation reviews each application via a public form at
  `https://testnet.suwappu.bot/apply`.
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
| RAM | 64 GB — **and this is the resource that actually binds; see below** |
| NVMe SSD | 2 TB |
| Network | 1 Gbps symmetric, ≤ 50 ms RTT to ≥ 3 seed regions |
| OS | Linux x86_64 or arm64; tested on Ubuntu 24.04 LTS. **Use 24.04 or newer** — release binaries are glibc-linked and built on GitHub's `ubuntu-latest`, so an older distro fails at startup with `GLIBC_x.yz not found`. |

> **Correction — the node has no persistence.** An earlier version of
> this table said "state grows ~10 GB/month" on disk. That is wrong and
> the error mattered, so it is called out rather than quietly edited:
> the DAG store is **in-memory and never prunes**. Consequences you must
> plan for:
> - **RAM grows without bound** for as long as a node stays up. The 64 GB
>   figure is a starting point, not a steady state — watch RSS.
> - **A restart loses all history.** There is no on-disk state to reload;
>   a restarted node re-syncs from peers, and can only go back as far as
>   its peers have held *in their own memory* since *their* last restart.
> - Operations therefore depend on **periodic regenesis** until snapshot
>   persistence lands (`/goal` A6/A7). Expect scheduled restarts of the
>   whole network, not just your node.
>
> Disk is used for the event log (`event_log_path`) and little else, so
> the 2 TB figure is generous — but do not size RAM as if the 2 TB were
> absorbing chain growth.

If you can't hit the network RTT requirement (e.g. you're on
mobile-tier home internet), you'll see more dropped certs and
lower point accrual. Foundation may dequeue persistently
under-performing validators after 30 days; you can re-apply.

## Onboarding flow

1. **Apply** at `https://testnet.suwappu.bot/apply`.
   Foundation runs basic identity + jurisdictional checks.
2. **Foundation submits a dual-signed `AdmitAuthority` governance
   Intent** (client wire v3) with the ML-DSA-65 pubkey you mint
   locally (next step). You co-sign the intent digest with that same
   key — a proof of possession, so an admit cannot be forged for a
   key you don't hold; the foundation's tooling walks you through
   producing the co-signature. Your `authority_id` is assigned at
   this step; it's `≥ 8` (ids 0–6 are seed validators, 7 is the
   faucet).
3. **Event-log submission — mechanism TBD.** This step previously
   issued you AWS IAM credentials for an S3 bucket. **The project no
   longer has AWS**, so that path is gone and no replacement has been
   chosen yet (it needs to work on the non-AWS footprint in
   [`NON-AWS-DEPLOY.md`](NON-AWS-DEPLOY.md)). Until it is decided, keep
   your `event_log_path` file locally; the foundation will publish a
   submission method before points accrual starts.
4. **Wait one epoch boundary** for the admit Intent to land on
   chain. Once your `authority_id` shows up in
   `suwappu_getAuthorityRegistry`, you're admitted. Your node —
   started with `allow_post_genesis_join = true` (see the config
   template below) — will have been syncing passively the whole
   time via the wire sync protocol, and begins authoring
   certificates automatically once it observes itself seated. No
   seed-side config change is needed for you to sync: seeds accept
   late-joiner connections dynamically.

## Local setup

```sh
# 1. Mint your ML-DSA-65 + BLS12-381 keypairs OFF-HOST (e.g. on a
#    fresh laptop you'll later wipe) — these will be your
#    validator's signing keys.
#    `suwappu-keygen` ships in the release tarball (step 3a), so you do
#    NOT need to build from source — which matters because the
#    `suwappu-db` dependency is private and external operators cannot
#    build the workspace.
./suwappu-keygen --algo mldsa --sk ./mldsa.sk --pk ./mldsa.pk
./suwappu-keygen --algo bls --sk ./bls.sk --pk ./bls.pk

# 2. Send only the public keys to the foundation (the ML-DSA
#    pubkey goes into the admit Intent). The secret keys NEVER
#    leave your machine.
cat ./mldsa.pk | base64
cat ./bls.pk | base64

# 3. Get your authority_id back from the foundation once admit
#    lands. Then on your validator hardware:

# 3a. Pull the testnet binary release. Linux builds are glibc, not
#     musl (the musl target does not build — see the header of
#     .github/workflows/release.yml). Pick the tarball for your arch:
#       x86_64  -> x86_64-unknown-linux-gnu
#       arm64   -> aarch64-unknown-linux-gnu
gh release download suwappu-dag-v0.X.Y --pattern '*x86_64-unknown-linux-gnu*'
tar -xzf suwappu-dag-0.X.Y-x86_64-unknown-linux-gnu.tar.gz

# 3b. Pull the public testnet genesis.
curl -fsSL https://testnet.suwappu.bot/genesis.toml \
    -o /etc/suwappu/genesis.toml

# 3c. Write your own node.toml. The seed peer list is published
#     at https://testnet.suwappu.bot/peers.txt.
cat > /etc/suwappu/node.toml <<EOF
self_id = "<your-operator-label>"
authority_id = <your-assigned-id>
# Post-genesis joiners MUST set this: your id is not in the published
# genesis. The node boots in passive-sync mode and starts authoring
# only once it observes itself seated in the Authority Ring.
allow_post_genesis_join = true
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
$(curl -fsSL https://testnet.suwappu.bot/peers.txt)
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
`https://testnet.suwappu.bot/leaderboard`.

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

  > Note on what "slash" currently means: equivocation **detection** is
  > implemented and a detected equivocator is ejected, but testnet stake
  > is a **declared integer in the admit intent, not an escrowed bond**
  > (`/goal` A9). So the enforceable penalty today is expulsion and loss
  > of accrued points — there is no posted collateral to confiscate.
  > Bonding is a separate, unlanded change; this line will become
  > literal when it lands.
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
  `https://status.testnet.suwappu.bot`.

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
