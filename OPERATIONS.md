# OPERATIONS

Runbooks for running the gsx-devnet. Every procedure here is written
to be executable cold — a fresh operator who has never deployed the
devnet should be able to follow any section without asking
questions.

For the architecture-level "what is the devnet" overview, see
[`terraform/devnet/README.md`](terraform/devnet/README.md). For
release procedures, see [`RELEASING.md`](RELEASING.md).

## Identity prerequisites

Every procedure assumes:

- `AWS_PROFILE=gsn` resolves to account `492042618949`. Verify with
  `aws sts get-caller-identity` before any action.
- GitHub identity is `tomagsx`. `gh auth status` should show that
  account active. If the active token is for a different account
  (e.g. `0xSoftBoi`), switch with
  `env -u GH_TOKEN -u GITHUB_TOKEN gh auth switch --user tomagsx`.

---

## 1. Bootstrap a fresh devnet

Run once when standing up the devnet for the first time.

1. **Build a release `gsx-node` + `gsx-faucet` + `gsx-indexer`** for
   `aarch64-unknown-linux-musl` (matches the `t4g.medium` validator
   AMI). Easiest: push a `gsx-dag-v0.1.0` tag and let
   `.github/workflows/release.yml` produce the binaries; then
   download.

   ```sh
   git tag -a gsx-dag-v0.1.0 -m "gsx-dag 0.1.0"
   git push origin gsx-dag-v0.1.0
   gh run watch
   gh release download gsx-dag-v0.1.0 \
     --pattern '*aarch64-unknown-linux-musl*'
   tar -xzf gsx-dag-0.1.0-aarch64-unknown-linux-musl.tar.gz
   ```

2. **Generate genesis + per-region keys + faucet keypair.**

   ```sh
   cargo build --release -p gsx-crypto --bin gsx-keygen
   ./scripts/devnet/gen-genesis.py --out-dir ./target/devnet/keys
   ```

   Verify the genesis manifest declares 5 validators (4 region
   validators + 1 faucet authority at `authority_id = 4`) and the
   prebalances.toml lists the faucet's address with 1 billion GSX.

3. **Apply the terraform stack.** First apply creates the S3
   artifact bucket; subsequent steps upload bin + keys + configs
   into it.

   ```sh
   BILLING_ALARM_EMAIL=ops@globalsettlement.com \
     ./scripts/devnet/deploy.sh apply
   ```

   Confirm the SNS subscription email; the billing-cap alarm is
   muted until subscription is confirmed.

4. **Upload binary + keys + faucet public key** to S3:

   ```sh
   BUCKET=gsx-dag-devnet-artifacts
   # Validator binary (from step 1).
   aws s3 cp ./gsx-dag-0.1.0-aarch64-unknown-linux-musl/gsx-node \
       s3://$BUCKET/bin/gsx-node --profile gsn

   # Faucet binary.
   aws s3 cp ./gsx-dag-0.1.0-aarch64-unknown-linux-musl/gsx-faucet \
       s3://$BUCKET/bin/gsx-faucet --profile gsn

   # Per-region validator keys + genesis (NOT the faucet secret —
   # that goes to Secrets Manager in step 5).
   aws s3 sync ./target/devnet/keys/ s3://$BUCKET/keys/ \
       --exclude "faucet/mldsa.sk" --profile gsn
   aws s3 cp ./target/devnet/keys/genesis.toml \
       s3://$BUCKET/genesis/genesis.toml --profile gsn
   aws s3 cp ./target/devnet/keys/prebalances.toml \
       s3://$BUCKET/genesis/prebalances.toml --profile gsn
   ```

5. **Upload the faucet secret key to Secrets Manager** (the
   faucet's IAM role grants `GetSecretValue` for exactly this ARN):

   ```sh
   aws secretsmanager put-secret-value \
       --secret-id gsx-devnet/faucet/mldsa-secret-key \
       --secret-binary fileb://./target/devnet/keys/faucet/mldsa.sk \
       --profile gsn --region us-east-1
   ```

6. **Render + upload per-region `node.toml` files** (peer IPs are
   only known after the apply allocated EIPs):

   ```sh
   ./scripts/devnet/render-configs.sh
   ```

7. **Restart bootstrap on each validator** to pick up the
   freshly-uploaded artifacts:

   ```sh
   ./scripts/devnet/deploy.sh output -json validators \
     | jq -r '.[].instance_id' \
     | xargs -I {} aws ssm send-command --instance-ids {} \
         --document-name AWS-RunShellScript \
         --parameters 'commands=["systemctl restart gsx-bootstrap gsx-node"]' \
         --profile gsn --region us-east-1
   ```

8. **Verify the mesh is up.** Within 60 seconds, every validator's
   `gsx_getEpoch.latest_committed_round` should be non-zero and
   monotonically advancing:

   ```sh
   for region in us-east-1 eu-west-1 ap-southeast-1 sa-east-1; do
     ip=$(./scripts/devnet/deploy.sh output -json validators \
            | jq -r ".[\"$region\"].public_ip")
     echo -n "$region ($ip): "
     curl -fsS -X POST -H 'Content-Type: application/json' \
          -d '{"jsonrpc":"2.0","id":1,"method":"gsx_getEpoch"}' \
          "http://$ip:9092/" | jq -r '.result.latest_committed_round'
   done
   ```

9. **Verify the faucet.** The faucet may take an extra ~30 seconds
   to bootstrap (it pulls the binary from S3 + the secret from
   Secrets Manager):

   ```sh
   faucet_ip=$(./scripts/devnet/deploy.sh output -json faucet \
                | jq -r '.public_ip')
   curl -fsS "http://$faucet_ip:8080/health" | jq .
   curl -fsS -X POST -H 'Content-Type: application/json' \
        -d '{"address":"0x0000000000000000000000000000000000000001"}' \
        "http://$faucet_ip:8080/faucet" | jq .
   ```

After step 9 returns a tx_hash, the devnet is operational.

---

## 2. Restart a stuck validator

Symptoms: a single region's `gsx_last_committed_round` lags >30
behind the others, or the `gsx-devnet-silent-peer-<region>` alarm
fires.

1. **Identify the instance:**

   ```sh
   ./scripts/devnet/deploy.sh output -json validators \
     | jq -r ".[\"<region>\"].instance_id"
   ```

2. **Open an SSM session** (preferred over SSH; doesn't require an
   unlocked private key locally):

   ```sh
   aws ssm start-session --target i-... --profile gsn \
       --region <region>
   ```

3. **Inside the session, inspect logs + restart:**

   ```sh
   sudo journalctl -u gsx-node -n 200 --no-pager
   sudo systemctl restart gsx-node
   sudo systemctl status gsx-node
   ```

4. **Verify reconnect.** From off-host, watch
   `gsx_subscribeEvents` for the next "committed" event from this
   region's authority:

   ```sh
   region_ip=$(...)
   curl -fsS "http://$region_ip:9092/ws"   # WS upgrade; abort after first frame
   ```

   The CloudWatch dashboard should show this region's tip-round
   resume within 60 seconds.

If the validator fails to start (binary missing, key file missing,
config file invalid), follow § 5 to re-bootstrap the validator
from S3 instead of just restarting.

---

## 3. Roll a validator's ML-DSA-65 key

When: scheduled rotation (quarterly), suspected compromise, or
operator handoff.

1. **Mint a fresh keypair OFF-HOST** (never on the validator EC2;
   keep the new secret out of any shared filesystem):

   ```sh
   cargo run --release -p gsx-crypto --bin gsx-keygen -- \
     --algo mldsa --sk ./new/<region>/mldsa.sk \
                  --pk ./new/<region>/mldsa.pk
   ```

2. **Submit the eject governance Intent** for the validator's
   current `authority_id`. The eject takes effect at the NEXT
   epoch boundary (1024 rounds × 250 ms ≈ 4 min 16 s by default).

   ```sh
   # Hand-built via the SDK; see examples/rust/admit_authority.rs
   # for the pattern.
   ```

3. **Submit the admit governance Intent** for a NEW `authority_id`
   (e.g. one past the current max) with the new pubkey hex. Use a
   new authority_id rather than reusing the old one — keeps the
   audit trail clean.

4. **Wait for the next epoch boundary** to apply both. Verify on
   every validator:

   ```sh
   for region in ...; do
     curl -fsS -X POST -d '{"jsonrpc":"2.0","id":1,"method":"gsx_getAuthorityRegistry"}' \
          "http://<region-ip>:9092/" | jq '.result | length'
   done
   # Expected: 5 (4 validators + 1 faucet) — count is unchanged.
   ```

5. **Update the local secret key on the validator's persistent
   EBS:**

   ```sh
   aws s3 cp ./new/<region>/mldsa.sk \
       s3://gsx-dag-devnet-artifacts/keys/<region>/mldsa.sk --profile gsn
   aws s3 cp ./new/<region>/mldsa.pk \
       s3://gsx-dag-devnet-artifacts/keys/<region>/mldsa.pk --profile gsn
   # SSM into the instance and force a re-bootstrap so it pulls
   # the new key + restarts:
   aws ssm send-command --instance-ids <i-...> \
       --document-name AWS-RunShellScript \
       --parameters 'commands=["
         rm -f /var/lib/gsx/mldsa.sk /var/lib/gsx/bls.sk
         systemctl restart gsx-bootstrap gsx-node
       "]' --profile gsn --region <region>
   ```

6. **Verify** the new key is in use: the validator's
   `gsx_getAuthorityRegistry` entry shows the new `authority_id`
   + new pubkey hex.

---

## 4. Roll the faucet key

When: scheduled rotation (quarterly), suspected exfiltration, or
governance change.

1. **Mint the new keypair** off-host (same as § 3 step 1).

2. **Drain the OLD faucet wallet** to a burn address using the OLD
   key, BEFORE the eject step lands. This window must be strictly
   ordered: drain → eject. If they race, the drain can fail because
   the old authority is no longer seated. To drain:

   ```sh
   # The drain is a single Transfer using the old faucet's
   # signing key. Submit via examples/rust/submit_transfer.rs or
   # equivalent.
   ```

3. **Submit the eject Intent** for the old faucet authority
   (`authority_id = 4` by default).

4. **Submit the admit Intent** for the new faucet authority with
   the new ML-DSA pubkey.

5. **Wait for the next epoch boundary.**

6. **Pre-balance the new faucet address.** Genesis pre-balance is
   one-shot; subsequent re-funding is a normal Transfer from a
   reserve wallet (the team holds one off-chain). Source the
   reserve via:

   ```sh
   # ad-hoc Transfer of N GSX from reserve to new faucet address
   ```

7. **Update Secrets Manager:**

   ```sh
   aws secretsmanager put-secret-value \
       --secret-id gsx-devnet/faucet/mldsa-secret-key \
       --secret-binary fileb://./new/faucet/mldsa.sk \
       --profile gsn --region us-east-1
   ```

8. **Restart the faucet service** so it loads the new key:

   ```sh
   faucet_id=$(./scripts/devnet/deploy.sh output -json faucet \
                | jq -r '.instance_id')
   aws ssm send-command --instance-ids $faucet_id \
       --document-name AWS-RunShellScript \
       --parameters 'commands=["systemctl restart gsx-faucet-bootstrap gsx-faucet"]' \
       --profile gsn --region us-east-1
   ```

9. **Verify** via `curl .../health` + a test `curl .../faucet`.

See [`docs/devnet/faucet-key-ceremony.md`](docs/devnet/faucet-key-ceremony.md)
for the threat model and the rationale behind each step's ordering.

---

## 5. Update validator binary (rolling restart)

When: a new tagged release lands and the devnet should pick it up.

1. **Download the release binaries.**

   ```sh
   gh release download gsx-dag-v0.X.Y \
     --pattern '*aarch64-unknown-linux-musl*'
   tar -xzf gsx-dag-0.X.Y-aarch64-unknown-linux-musl.tar.gz
   ```

2. **Upload to S3** (versioned bucket — overwrite is safe; the
   previous version stays as a noncurrent version for 30 days
   per the lifecycle policy):

   ```sh
   BUCKET=gsx-dag-devnet-artifacts
   aws s3 cp ./gsx-dag-0.X.Y-aarch64-unknown-linux-musl/gsx-node \
       s3://$BUCKET/bin/gsx-node --profile gsn
   ```

3. **Rolling restart, one region at a time.** Wait for each region
   to reconverge before moving to the next.

   ```sh
   for region in us-east-1 eu-west-1 ap-southeast-1 sa-east-1; do
     id=$(./scripts/devnet/deploy.sh output -json validators \
            | jq -r ".[\"$region\"].instance_id")
     echo "Restarting $region ($id)..."
     aws ssm send-command --instance-ids $id \
         --document-name AWS-RunShellScript \
         --parameters 'commands=["systemctl restart gsx-bootstrap gsx-node"]' \
         --profile gsn --region $region
     # Wait for tip-round to advance again before moving on.
     sleep 60
     ip=$(./scripts/devnet/deploy.sh output -json validators \
            | jq -r ".[\"$region\"].public_ip")
     curl -fsS -X POST \
          -d '{"jsonrpc":"2.0","id":1,"method":"gsx_getEpoch"}' \
          "http://$ip:9092/" | jq .result.latest_committed_round
   done
   ```

4. **Verify the new version** is running on every region via
   `journalctl` log lines after the restart — the daemon logs its
   version at startup.

If the new binary panics on startup, follow § 9 to roll back to
the previous version stored as the noncurrent S3 version.

---

## 6. Apply a security patch

Same procedure as § 5, but:

- The release should be a **patch** version (e.g. `0.1.1` instead
  of `0.2.0`) per RELEASING.md.
- The commit message + CHANGELOG entry should describe the patch
  without revealing exploitation details, until coordinated
  disclosure per `SECURITY.md`.
- Rolling restart should be **as fast as possible** — don't wait
  60s between regions if the patch closes an actively-exploited
  vulnerability. The cluster tolerates the round-driver-skip path
  while individual validators restart; commits resume within a
  few rounds.

---

## 7. Diagnose stuck commits

When: `gsx-devnet-halt` alarm fires.

1. **Check the CloudWatch dashboard** at
   `https://us-east-1.console.aws.amazon.com/cloudwatch/home?region=us-east-1#dashboards/dashboard/gsx-devnet`.
   Identify whether tip-round is flat on ALL 4 regions (cluster
   halt) or just a subset (partial — see § 2 for single-region
   recovery).

2. **Inspect the events.ndjson** on each region. The "committed"
   event stream stopping cold is the canonical signal:

   ```sh
   for region in ...; do
     id=...
     aws ssm send-command --instance-ids $id \
         --document-name AWS-RunShellScript \
         --parameters 'commands=["tail -n 50 /var/log/gsx/events.ndjson | grep committed | tail -5"]' \
         --profile gsn --region $region
   done
   ```

3. **Per-validator state snapshot via `gsx_getEpoch`:** if some
   regions return a higher `latest_committed_round` than others,
   the lagging regions are still catching up — wait 5 minutes
   before deeper investigation.

4. **Common causes:**
   - **Network partition**: one region's EIP became unreachable.
     Check `aws ec2 describe-instances --instance-ids ...` for
     state; check security group; check Route53 health.
   - **Single-cert orphan** (rare post-IQ-004): see the skill
     `dag-decide-slot-single-cert-orphan-after-parent-set-frozen`.
   - **Lagging-node convergence** (Phase G governance window):
     see the skill `dag-phase-g-eject-stage-lagging-node-flake`.
   - **Stake-denominator deadlock** (only during admit
     governance): see `bft-stake-denominator-deadlock-on-admit`.

5. **If unfixable in ≤30 min:** § 9 "Emergency stop" + § 1
   "Bootstrap" with a new genesis if the chain state is
   irrecoverable.

---

## 8. Snapshot + restore

Per-validator EBS snapshot is the recovery point for forensics or
catastrophic state corruption. The persistent volume already
survives instance replacement (lifecycle `prevent_destroy`).

### Take a snapshot

```sh
for region in us-east-1 eu-west-1 ap-southeast-1 sa-east-1; do
  volume=$(./scripts/devnet/deploy.sh output -json validators \
             | jq -r ".[\"$region\"].state_volume_id")
  aws ec2 create-snapshot --volume-id $volume \
      --description "gsx-devnet pre-patch $(date -u +%Y-%m-%dT%H:%M:%SZ)" \
      --profile gsn --region $region
done
```

### Restore from a snapshot

```sh
# 1. Stop the validator service (don't delete the instance).
# 2. Detach the existing state volume.
# 3. Create a new volume from the snapshot in the same AZ.
# 4. Attach the new volume at /dev/sdf.
# 5. SSM in; remount /var/lib/gsx; restart gsx-node.
```

Restore is intentionally manual — the procedure is rare and
mistakes are easy to make. Don't script it.

---

## 9. Emergency stop

When: the cluster is actively misbehaving in a way that's hurting
external developers (e.g. forking, double-committing, security
incident). The goal: stop the validators FAST, investigate, then
resume cleanly.

1. **Stop all 4 validators in parallel:**

   ```sh
   for region in us-east-1 eu-west-1 ap-southeast-1 sa-east-1; do
     id=$(./scripts/devnet/deploy.sh output -json validators \
            | jq -r ".[\"$region\"].instance_id")
     aws ec2 stop-instances --instance-ids $id \
         --profile gsn --region $region &
   done
   wait
   ```

2. **Take EBS snapshots** of all 4 state volumes (see § 8) so the
   stopped state is recoverable.

3. **Pause the faucet** (a stopped cluster can't process drips
   anyway, but stop the faucet HTTP server explicitly so devs see
   a clean 503 instead of a confused timeout):

   ```sh
   faucet_id=$(./scripts/devnet/deploy.sh output -json faucet \
                | jq -r '.instance_id')
   aws ec2 stop-instances --instance-ids $faucet_id \
       --profile gsn --region us-east-1
   ```

4. **Post a status update** to the status page (G8) + Discord +
   the team Slack `#incidents` channel.

5. **Investigate** via § 7. The snapshots from step 2 are the
   forensic record.

### Resume from emergency stop

1. Start each validator: `aws ec2 start-instances --instance-ids
   <id>`. The persistent EBS reattaches at boot; cloud-init
   restarts `gsx-node`.
2. Start the faucet: same `start-instances`.
3. Verify mesh per § 1 step 8.

If the underlying cause WAS a bug in the daemon, do NOT resume
without first deploying the patched binary per § 6.

---

## 10. Testnet operations

Procedures §§ 1–9 above apply to the **devnet**. The **testnet**
is structurally identical (same gsx-node binary, same systemd
unit shape, same SSM access pattern), with these scale-up
diffs:

- **7 seed regions** instead of 4 (us-east-1, us-west-2,
  eu-west-1, eu-central-1, ap-southeast-1, ap-northeast-1,
  sa-east-1).
- **Bigger instances** (`c7g.xlarge` not `t4g.medium`) +
  **larger state volumes** (200 GB vs 50 GB).
- **External validators** join via the points program — see
  § 10.2 below.
- **Validator-program EC2 + RDS** runs the points accumulator
  daemon (forthcoming) — see § 10.3.
- **DNS surface** is `*.testnet.gsx.globalsettlement.com`
  (devnet uses `*.devnet.gsx.*`).
- **Chain id 20251**, network_id `gsx-testnet-v1`.

When a procedure from §§ 1–9 also applies to testnet, swap
`devnet` → `testnet` in the paths + bucket names + the
`scripts/devnet/` → `scripts/testnet/` references; everything
else is identical.

### 10.1 Bootstrap the testnet

Same shape as § 1 with the per-region count + bucket names
bumped up. Use `./scripts/testnet/deploy.sh apply` (not
`./scripts/devnet/deploy.sh`); the wrapper enforces
`BILLING_ALARM_EMAIL` + blocks destroy.

The procedure below is the **consolidated greenfield order**
captured after the first live bootstrap on 2026-05-18. Two
non-obvious gotchas surfaced that need explicit steps:

1. The validator binary is **arm64** (`c7g.xlarge`). The perf
   stack's existing CodeBuild only emits `x86_64`, so the
   testnet ships its own CodeBuild project +
   `scripts/testnet/buildspec.yml` targeting
   `aarch64-unknown-linux-gnu`. The build step is mandatory
   before SSM-restart; otherwise `gsx-bootstrap.service` fails
   pulling `s3://$BUCKET/bin/gsx-node` (NoSuchKey).
2. The validator AMI is **Ubuntu 24.04 noble**, which no longer
   ships an `awscli` apt package — cloud-init's `packages:`
   directive logs `Package awscli is not available` and
   `bootstrap.sh` fails on first boot with
   `aws: command not found`. Step 8 installs the official
   `awscli-exe-linux-aarch64.zip` via SSM as part of restarting
   the units; do this for every region. The cloud-init template
   in `terraform/devnet/modules/validator/cloud-init.yaml`
   needs a follow-up fix so future instance replacements
   bootstrap cleanly without SSM intervention.

```sh
# 1. Mint genesis. gsx-keygen MUST be on PATH so the script
#    mints a real ML-DSA-65 faucet keypair (otherwise the
#    script falls back to a placeholder + the faucet binary
#    rejects every drip).
cargo build --release -p gsx-crypto --bin gsx-keygen
export PATH="$PWD/target/release:$PATH"
./scripts/testnet/gen-genesis.py --out-dir ./target/testnet/keys

# 2. SSM gsx-db deploy key (one-time per account; testnet
#    reuses /gsx-perf/gsx-db-deploy-key — same gsx-db repo,
#    same key).
aws ssm put-parameter --name /gsx-perf/gsx-db-deploy-key \
    --type SecureString \
    --value "$(cat ~/.ssh/gsx-db-deploy)" \
    --profile gsn --region us-east-1 || true   # already there from perf

# 3. First apply. Creates artifact bucket, VPCs, 7 validator
#    EC2s, CodeBuild project, ALBs, RDS, dashboards, alarms.
#    ~10–15 min wall time on a cold apply (ACM DNS validation
#    + RDS initial backup are the long tails).
BILLING_ALARM_EMAIL=ops@globalsettlement.com \
  ./scripts/testnet/deploy.sh apply

# 4. Upload genesis + per-region validator keys to the
#    artifact bucket. The faucet secret key goes to Secrets
#    Manager, NOT the bucket.
BUCKET=gsx-dag-testnet-artifacts
aws s3 cp ./target/testnet/keys/genesis.toml \
    s3://$BUCKET/genesis/genesis.toml      --profile gsn --sse AES256
aws s3 cp ./target/testnet/keys/prebalances.toml \
    s3://$BUCKET/genesis/prebalances.toml  --profile gsn --sse AES256
for r in us-east-1 us-west-2 eu-west-1 eu-central-1 \
         ap-southeast-1 ap-northeast-1 sa-east-1; do
  aws s3 cp ./target/testnet/keys/$r/mldsa.sk \
      s3://$BUCKET/keys/$r/mldsa.sk --profile gsn --sse AES256
  aws s3 cp ./target/testnet/keys/$r/bls.sk \
      s3://$BUCKET/keys/$r/bls.sk --profile gsn --sse AES256
done
aws secretsmanager put-secret-value \
    --secret-id gsx-testnet/faucet/mldsa-secret-key \
    --secret-binary fileb://./target/testnet/keys/faucet/mldsa.sk \
    --profile gsn --region us-east-1

# 5. Build the validator binary on CodeBuild (arm64). This
#    script packages HEAD, uploads to the artifact bucket's
#    sources/ prefix, runs `aws/codebuild/amazonlinux2-aarch64-
#    standard:3.0`, and pulls the resulting binaries back to
#    ./target/testnet/. Cold cache ~10–15 min, warm ~3–5 min.
./scripts/testnet/build.sh

# 6. Render + upload per-region node.toml (peer IPs known only
#    after step 3 allocated the EIPs).
./scripts/testnet/render-configs.sh

# 7. Pull the validator instance IDs.
./scripts/testnet/deploy.sh output -json validators \
  | jq -r 'to_entries[]|"\(.key)=\(.value.instance_id)"' \
  > /tmp/validators.kv

# 8. SSM-bootstrap each seed. Installs the official awscli zip
#    (cloud-init's apt path fails on Ubuntu noble) and restarts
#    the units. SSM is region-scoped — call the API in each
#    validator's region.
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
while IFS='=' read -r region iid; do
  echo "[$region] ssm-bootstrap $iid"
  aws ssm send-command --region "$region" --instance-ids "$iid" \
      --document-name AWS-RunShellScript \
      --parameters "$PARAM" \
      --profile gsn --timeout-seconds 600 \
      --query 'Command.CommandId' --output text
done < /tmp/validators.kv

# 9. Verify each EIP advances rounds. The wildcard ALB
#    `rpc.testnet.gsx.globalsettlement.com` returns 503 today
#    (see § 10.4 "ALB has no target attachments"), so probe
#    each validator directly until that's fixed.
for ip in $(./scripts/testnet/deploy.sh output -json validators \
            | jq -r '.[].public_ip'); do
  echo "$ip $(curl -fsS --max-time 5 -X POST \
       -H 'Content-Type: application/json' \
       -d '{"jsonrpc":"2.0","id":1,"method":"gsx_getEpoch"}' \
       "http://$ip:9092/" | jq -r '.result.latest_committed_round')"
done
# Expect: every IP returns a non-zero `latest_committed_round`,
# and the cross-region spread is ≤ 5 rounds within a 2-min
# sample (round_ms=250; cross-region propagation adds ~250 ms).
```

After step 9 shows all 7 seeds advancing in lockstep, the
chain is producing certs. The faucet binary upload + first
drip is a separate procedure (the faucet binary isn't built
yet today; the faucet EC2 sits idle).

**Post-bootstrap one-time DNS step.** The testnet zone is in
the gsn account; if `globalsettlement.com`'s apex zone lives
in a different account, an operator must paste the
`testnet_nameservers` output as NS records under the apex.
See `terraform/testnet/dns.tf` for the output.

### 10.1.1 Phase 1 fronting: CloudFront over the public EIPs

The RPC + faucet ALBs are kept in TF as a no-op skeleton
(`alb.tf` ships them with NO listeners and NO target attachments)
because AWS ALB rejects `target_type = "ip"` with a public IP that
isn't inside the ALB's own VPC subnet, RFC1918, or RFC6598 — even
within the same region. The 7 seed validators each live in their
own regional VPC and have public EIPs, so none qualify as ALB
targets.

**Phase 1, in production today:** two CloudFront distributions
(`cf_rpc.tf`, `cf_faucet.tf`) dial the EIPs directly from the
edge:

- `rpc.testnet.gsx.globalsettlement.com` → CloudFront → 7
  validator EIPs declared as origins, with an origin group
  (`validator-failover`) wired for primary = us-east-1,
  secondary = eu-west-1, failover on 5xx (500/502/503/504).
- `ws.testnet.gsx.globalsettlement.com` → same distribution, with
  a dedicated `/ws*` cache behavior that allows GET/HEAD/OPTIONS
  and forwards Upgrade/Connection headers (CF has native
  WebSocket support on custom-origin distributions since 2018).
- `faucet.testnet.gsx.globalsettlement.com` → second CloudFront
  distribution → single origin (the faucet EIP, port 8080). No
  origin group: the faucet is a singleton, there's nothing to
  fail over to.

Cache TTL is pinned to 0 across both distributions (cache policy
`CachingDisabled` / id `4135ea2d-6df8-44a3-9df3-4b5a84be39ad`)
because JSON-RPC + faucet responses mutate substrate state on
every call; caching at the edge would silently serve stale
balances and break dApps. CloudFront's value here is TLS
termination + WAF + DDoS protection + automatic primary→secondary
origin failover, not cache hit-rate.

WAF (`waf.tf`) ships with scope `CLOUDFRONT` — the same
ruleset as the prior REGIONAL ACL (10k req/IP rate limit + AWS
common rules + IP reputation list). Both distributions reference
it via `web_acl_id` on the distribution resource directly.

**Phase 2 (deferred, est. M21+ when external operators land):**
swap to per-region NLB + AWS Global Accelerator. True global
anycast + per-region failure isolation + the right shape for
"operator brings their own hardware in their region's NLB target
group." Phase 2 is also the right point to re-CIDR the
validator VPCs from the currently-repeated `10.43.0.0/16` to
non-overlapping ranges (required for NLB peering; currently
blocked by `prevent_destroy = true` on the EBS state volumes;
will be bundled with a fleet rotation).

Design decision + Phase 2 sketch live in
`~/.claude/plans/validated-prancing-curry.md`.

For direct-EIP probes against individual validators (still useful
for debugging single-region health), use the public-IP table in
§ 10.1 step 9.

### 10.2 Onboard an external validator operator

The points program (Track B) accepts external operators after
they apply + KYC. The flow:

```sh
# After foundation submits an AdmitAuthority governance Intent
# for the new operator (admit assigns them an authority_id ≥ 8),
# generate their scoped IAM credentials:
./scripts/testnet/onboard-operator.sh <authority_id> <operator-label>

# Example for authority_id=8, label=acme-validator-co:
./scripts/testnet/onboard-operator.sh 8 acme-validator-co
```

The script:
1. Creates IAM user `gsx-testnet-operator-acme-validator-co`.
2. Attaches a policy scoped to `s3:PutObject` on exactly
   `s3://gsx-dag-testnet-validator-uploads/uploads/8/*`.
3. Generates an access-key + secret pair.
4. Prints the credentials in a block to forward to the
   operator out-of-band (Signal / 1Password secure share).

Send the operator both the credentials AND the URL of
[`docs/testnet/VALIDATOR-OPERATORS.md`](docs/testnet/VALIDATOR-OPERATORS.md)
for their setup procedure.

### 10.3 Deploy the points-accumulator daemon

The daemon binary (`crates/gsx-validator-program/`) lives in
`s3://gsx-dag-testnet-artifacts/bin/gsx-validator-program`
(arm64, ~11 MB; built by the CodeBuild project in
`terraform/testnet/codebuild.tf` via `scripts/testnet/build.sh`).
The program EC2 (`aws_instance.program` in
`terraform/testnet/validator-program.tf`, `t4g.medium`) and its
RDS Postgres (`aws_db_instance.program`, `db.t4g.small`) are
already provisioned and idle — this section turns them on.

#### 10.3.0 Prereqs (one-time, before first deploy)

```sh
# 1. Mint an admin bearer token for the /admin/* endpoints and
#    park it in Secrets Manager. admit-operator.sh reads from here
#    when registering new operators in the points table.
ADMIN_TOKEN=$(openssl rand -hex 32)
AWS_PROFILE=gsn aws secretsmanager create-secret --region us-east-1 \
    --name gsx-testnet/program/admin-token \
    --secret-string "$ADMIN_TOKEN" >/dev/null
echo "admin token (save in 1Password): $ADMIN_TOKEN"

# 2. Note the RDS endpoint + the random-generated DB password
#    that terraform created.
PROGRAM_DB_ENDPOINT=$(./scripts/testnet/deploy.sh output -raw validator_program | jq -r '.db_endpoint')
PROGRAM_DB_PASSWORD=$(AWS_PROFILE=gsn aws secretsmanager get-secret-value --region us-east-1 \
    --secret-id gsx-testnet/program/db-password --query SecretString --output text)
# Sanity:
echo "DB: postgres://gsx_program:****@${PROGRAM_DB_ENDPOINT}:5432/validator_program"
```

#### 10.3.1 SSM-deploy the binary + systemd unit

SSM `AWS-RunShellScript` runs under `/bin/sh` (dash on Ubuntu),
not bash — keep the deploy script POSIX-shell-clean.

```sh
PROGRAM_ID=$(./scripts/testnet/deploy.sh output -raw validator_program | jq -r '.ec2_instance_id')

# Stage the deploy script locally so we can pass it via --parameters.
cat > /tmp/program-deploy.sh <<'EOF'
set -ex

# Install awscli zip (cloud-init's apt path is broken on Ubuntu 24.04 noble —
# same gotcha as the validator cloud-init; see § 10.1 step 8).
if ! command -v aws >/dev/null 2>&1; then
  apt-get update -qq
  apt-get install -y unzip jq postgresql-client || true
  curl -fsSL "https://awscli.amazonaws.com/awscli-exe-linux-$(uname -m).zip" -o /tmp/awscli.zip
  unzip -q -o /tmp/awscli.zip -d /tmp/
  /tmp/aws/install --update
fi

# Pull binary + admin token + DB password.
mkdir -p /opt/gsx /var/log/gsx-program
aws s3 cp s3://gsx-dag-testnet-artifacts/bin/gsx-validator-program /opt/gsx/gsx-validator-program
chmod +x /opt/gsx/gsx-validator-program

ADMIN_TOKEN=$(aws secretsmanager get-secret-value --region us-east-1 \
    --secret-id gsx-testnet/program/admin-token --query SecretString --output text)
DB_PASSWORD=$(aws secretsmanager get-secret-value --region us-east-1 \
    --secret-id gsx-testnet/program/db-password --query SecretString --output text)

# Resolve the RDS endpoint at deploy time so the systemd unit
# doesn't carry a stale value if RDS gets resized later.
DB_HOST=$(aws rds describe-db-instances --region us-east-1 \
    --db-instance-identifier gsx-testnet-program \
    --query 'DBInstances[0].Endpoint.Address' --output text)

# systemd unit. Sources env from /etc/gsx-program/env, which we
# rewrite from secrets-manager every time this script runs (so
# admin-token rotation is a re-run of this exact deploy).
mkdir -p /etc/gsx-program
cat > /etc/gsx-program/env <<ENV
GSX_PROGRAM_DATABASE_URL=postgres://gsx_program:${DB_PASSWORD}@${DB_HOST}:5432/validator_program
GSX_PROGRAM_RPC_URL=https://rpc.testnet.gsx.globalsettlement.com
GSX_PROGRAM_BIND=0.0.0.0:8090
GSX_PROGRAM_ADMIN_TOKEN=${ADMIN_TOKEN}
RUST_LOG=gsx_validator_program=info,sqlx=warn,axum=warn
ENV
chmod 600 /etc/gsx-program/env

cat > /etc/systemd/system/gsx-validator-program.service <<UNIT
[Unit]
Description=gsx-testnet points accumulator daemon
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
EnvironmentFile=/etc/gsx-program/env
ExecStart=/opt/gsx/gsx-validator-program
Restart=on-failure
RestartSec=3
StandardOutput=append:/var/log/gsx-program/stdout.log
StandardError=append:/var/log/gsx-program/stderr.log
LimitNOFILE=65535

[Install]
WantedBy=multi-user.target
UNIT

systemctl daemon-reload
systemctl reset-failed gsx-validator-program.service || true
systemctl enable gsx-validator-program.service
systemctl restart gsx-validator-program.service
sleep 4
echo "--- status ---"
systemctl is-active gsx-validator-program.service
echo "--- listening ---"
ss -tlnp | grep ':8090' || true
echo "--- stdout tail ---"
tail -20 /var/log/gsx-program/stdout.log 2>&1 || true
EOF

# Send to the program EC2.
PARAM=$(jq -Rs '{commands:[.]}' </tmp/program-deploy.sh)
AWS_PROFILE=gsn aws ssm send-command --region us-east-1 \
    --instance-ids "$PROGRAM_ID" \
    --document-name AWS-RunShellScript \
    --parameters "$PARAM" \
    --timeout-seconds 600 \
    --query 'Command.CommandId' --output text
```

#### 10.3.2 Seed the 7 foundation operators

Until external operators land, the leaderboard shows only the
foundation seeds (authority_id 0..6). Register them after the
daemon's HTTP API is live:

```sh
ADMIN_TOKEN=$(AWS_PROFILE=gsn aws secretsmanager get-secret-value --region us-east-1 \
    --secret-id gsx-testnet/program/admin-token --query SecretString --output text)
for entry in "0:us-east-1" "1:us-west-2" "2:eu-west-1" "3:eu-central-1" \
             "4:ap-southeast-1" "5:ap-northeast-1" "6:sa-east-1"; do
  aid="${entry%%:*}"; label="${entry##*:}"
  curl -fsS -X POST -H "Authorization: Bearer ${ADMIN_TOKEN}" \
       -H 'Content-Type: application/json' \
       -d "{\"authority_id\":${aid},\"label\":\"${label}\",\"is_seed\":true}" \
       https://program.testnet.gsx.globalsettlement.com/admin/operators
done
```

#### 10.3.3 Verify

```sh
# Health (no auth):
curl -fsS https://program.testnet.gsx.globalsettlement.com/health

# Leaderboard (no auth):
curl -fsS https://program.testnet.gsx.globalsettlement.com/leaderboard | jq .
```

The `gsx-testnet-program-down` CloudWatch alarm (Route53 health
check on `program.testnet.gsx.*:8090/leaderboard`) switches from
`INSUFFICIENT_DATA` to `OK` within 5 minutes of the daemon
responding. After the first 1-hour uptime sample, the leaderboard
starts showing per-seed uptime points (100/epoch per validator at
≥99% uptime).

#### 10.3.4 Rotate the admin bearer token

```sh
NEW_TOKEN=$(openssl rand -hex 32)
AWS_PROFILE=gsn aws secretsmanager put-secret-value --region us-east-1 \
    --secret-id gsx-testnet/program/admin-token --secret-string "$NEW_TOKEN"
# Re-run § 10.3.1 to push the new value into /etc/gsx-program/env
# and restart the unit.
```

### 10.4 Quarterly testnet maintenance window

The testnet allows scheduled re-genesis at most once per
quarter, announced ≥ 14 days in advance via the status page +
Discord. The maintenance procedure:

1. **T−14 days**: post the maintenance window on the status
   page + Discord `#announcements`. Include the expected
   downtime window + the rationale + the rollback plan.
2. **T−24 hours**: snapshot every state volume (§ 8) so the
   pre-maintenance state is recoverable if the window goes bad.
3. **T+0**: stop all 7 seed validators (`§ 9 emergency stop`).
4. **T+0..T+1h**: re-generate genesis, upload the new
   `genesis.toml` to S3, replace per-region node.toml files
   (render-configs.sh).
5. **T+1h**: restart the seeds in a coordinated wave; verify
   the new chain head via the explorer.
6. **T+2h**: status page returns to green; post the
   "maintenance complete" notice.

If the window slips past T+4h, escalate to a CEO/founder
decision: extend the window vs. roll back to the pre-snapshot
state. Default: roll back; missed deadlines hurt operator
trust.

### 10.5 Testnet tear-down (mainnet cutover)

When mainnet launches, the testnet either (a) keeps running
indefinitely as a parallel testing environment, or (b) is
formally torn down. The team picks at TGE.

If (b) — formal tear-down:

1. Announce the tear-down date ≥ 90 days in advance. Operators
   need time to wind down their dApps + extract any data they
   want to keep.
2. Export the points-accumulator RDS via `pg_dump` to S3 —
   this is the canonical record of operator points that
   converts to mainnet token.
3. Snapshot every state volume one final time.
4. Edit `terraform/testnet/modules/validator/main.tf` —
   remove `prevent_destroy = true` on `aws_ebs_volume.state`.
5. Edit `terraform/testnet/validator-program.tf` — set
   `deletion_protection = false` on the RDS instance.
6. `terraform apply` to update the lifecycle settings.
7. `scripts/deploy-aws.sh destroy testnet`. (The deploy.sh
   testnet-wrapper blocks this; bypass via the underlying
   `scripts/deploy-aws.sh` directly.)

This is deliberately painful. The testnet's chain history +
points data are load-bearing for the TGE conversion; don't
destroy them by accident.

---

## See also

- [`RELEASING.md`](RELEASING.md) — version bump + tag + Release
  workflow procedure.
- [`DEVNET.md`](DEVNET.md) — what external developers see; what we
  promise about devnet + testnet stability.
- [`terraform/devnet/README.md`](terraform/devnet/README.md) —
  devnet infrastructure overview + apply prerequisites.
- [`terraform/testnet/README.md`](terraform/testnet/README.md) —
  testnet infrastructure overview.
- [`docs/devnet/faucet-key-ceremony.md`](docs/devnet/faucet-key-ceremony.md) —
  faucet-specific key handling (applies to both devnet + testnet
  faucets; they share the ceremony shape with distinct secrets).
- [`docs/testnet/VALIDATOR-OPERATORS.md`](docs/testnet/VALIDATOR-OPERATORS.md) —
  external operator onboarding flow.
- [`docs/testnet/POINTS.md`](docs/testnet/POINTS.md) — points
  formula contract.
- `CONTRIBUTING.md` — how external developers report ops issues.
- `SECURITY.md` — coordinated disclosure for security incidents
  (the part of § 6 that requires careful messaging).
