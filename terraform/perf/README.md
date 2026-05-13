# GSX perf testnet — terraform

7-region AWS deployment for measuring main-lane consensus, fast-path, and
LTP attestation latency under real geographic spread.

## Layout

- `providers.tf` — one aliased AWS provider per region.
- `main.tf` — seven module instantiations + the shared S3 artifact bucket.
- `variables.tf` — operator-supplied inputs (SSH key, IP CIDR, instance type).
- `outputs.tf` — public IPs + bucket name; consumed by `scripts/perf/render-configs.sh`.
- `modules/region/` — per-region VPC + EC2 + EIP + SG + IAM + cloud-init.

## Regions

| Region | authority_id |
|---|---|
| us-east-1 | 0 |
| us-west-2 | 1 |
| eu-west-1 | 2 |
| ap-northeast-1 | 3 |
| ap-southeast-2 | 4 |
| sa-east-1 | 5 |
| af-south-1 | 6 |

This is also the index order in `genesis.toml`.

## Apply

```sh
cd terraform/perf
terraform init
terraform plan \
  -var operator_ip_cidr="$(curl -s ifconfig.me)/32" \
  -var ssh_public_key="$(cat ~/.ssh/gsx-perf.pub)"
terraform apply ...
```

`af-south-1` requires one-time region opt-in in the AWS console before the
first apply succeeds.

## Cost

7 × `t3.small` on-demand at us-east-1 pricing ≈ $0.0208 × 7 × 730 hrs ≈ $107/mo
running 24/7. For a 1-day campaign: ~$3.50. EIPs are free while attached.
S3 storage for logs (~1 GB) is negligible.

## Teardown

```sh
terraform destroy ...
```

Bucket has `force_destroy = true` so logs are deleted with the apply. Pull
them with `scripts/perf/collect.sh` first if you want to keep them.
