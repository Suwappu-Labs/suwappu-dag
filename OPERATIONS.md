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

## See also

- [`RELEASING.md`](RELEASING.md) — version bump + tag + Release
  workflow procedure.
- [`DEVNET.md`](DEVNET.md) — what external developers see; what we
  promise about devnet stability.
- [`terraform/devnet/README.md`](terraform/devnet/README.md) —
  infrastructure overview + apply prerequisites.
- [`docs/devnet/faucet-key-ceremony.md`](docs/devnet/faucet-key-ceremony.md) —
  faucet-specific key handling.
- `CONTRIBUTING.md` — how external developers report ops issues.
- `SECURITY.md` — coordinated disclosure for security incidents
  (the part of § 6 that requires careful messaging).
