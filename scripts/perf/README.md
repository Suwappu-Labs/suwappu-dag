# GSX perf campaign scripts

Drives the 7-region perf testnet from local laptop to publishable CDF.

## End-to-end run

```sh
# 1. Set up AWS profile, SSH key, and operator IP.
export AWS_PROFILE=gsn
export SSH_PUB=~/.ssh/gsx-perf.pub
ssh-keygen -t ed25519 -f ~/.ssh/gsx-perf -N "" -C "gsx-perf"

# 2. Provision (builds binaries, generates keys, terraform apply, uploads
#    artifacts). Prompts before spending money — type 'y' to continue.
scripts/perf/provision.sh

# 3. Wait ~2 min for cloud-init + systemd to start the daemons. Sanity check
#    one instance:
ssh -i ~/.ssh/gsx-perf ubuntu@$(cd terraform/perf && terraform output -json validators | jq -r '."us-east-1".public_ip') \
  sudo journalctl -u gsx-node -n 50

# 4. Run the load generator from your laptop.
scripts/perf/run.sh                 # default: 100 tps for 60s
# or: RATE=500 DURATION=300 scripts/perf/run.sh

# 5. Collect logs.
scripts/perf/collect.sh

# 6. Plot.
scripts/perf/analyze.sh
open target/perf/run/cdf_main_lane.png

# 7. Stop the EC2 meter.
scripts/perf/teardown.sh
```

## Script index

| Script | Purpose |
|---|---|
| `build.sh` | Cross-compile `gsx-node` / `gsx-loadgen` / `gsx-metrics` for `x86_64-unknown-linux-musl` via `cross`. |
| `gen-genesis.py` | Generate placeholder ML-DSA/BLS keypairs and the genesis manifest. **Placeholder keys are only valid for the closed perf testnet.** |
| `render-configs.sh` | Read EIPs from `terraform output` and write one `node.toml` per region. |
| `provision.sh` | Build → keygen → terraform apply → render configs → upload to S3. Prompts before spending. |
| `run.sh` | Run `gsx-loadgen` against one region's client port. |
| `collect.sh` | SCP `events.ndjson` back from every validator. |
| `analyze.sh` | Run `gsx-metrics` to join logs into a CSV, then plot. |
| `plot.py` | Matplotlib CDF + p50/p95/p99 summary. |
| `teardown.sh` | `terraform destroy`. |

## Requirements (local)

- AWS CLI configured for the `gsn` profile (account `492042618949`).
- `terraform >= 1.5`.
- `cross` (https://github.com/cross-rs/cross) for the musl build.
- `jq`, `python3` with `matplotlib`.

## Cost

7 × t3.small + EIPs ≈ $0.15/hr ≈ $3.50/day. A complete capture run is
under an hour.
