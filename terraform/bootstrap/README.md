# terraform/bootstrap

Creates the S3 bucket and DynamoDB table that every other terraform config in
this repo uses as its remote backend.

This config is intentionally the only one with **local state** — it solves the
chicken-and-egg problem of "the bucket that stores tf state must itself be
created somehow." It runs once, then nothing in here should change.

## Resources

| Resource | Name | Purpose |
|---|---|---|
| `aws_s3_bucket` | `suwappu-dag-tf-state` | Remote state for all sibling configs |
| `aws_dynamodb_table` | `suwappu-dag-tf-locks` | State-lock coordination, PITR on |

Both have `prevent_destroy = true`. Losing this state would orphan every
resource in the gsn account.

## First-time apply

```bash
cd terraform/bootstrap
AWS_PROFILE=gsn terraform init
AWS_PROFILE=gsn terraform apply
```

After apply, commit the resulting `terraform.tfstate` to **encrypted offline
storage** (e.g., 1Password). The local state file is gitignored.

## Migrating a sibling config to the remote backend

Once the bucket exists, any sibling config (`terraform/`, `terraform/perf/`)
can move its state into S3:

```bash
cd terraform/perf  # or terraform/
AWS_PROFILE=gsn terraform init -migrate-state
# Confirm "yes" at the prompt to copy local state to S3
```

Then delete the local state files:

```bash
rm terraform/perf/terraform.tfstate terraform/perf/terraform.tfstate.backup
```

## State backend layout

| Config | Backend key |
|---|---|
| `terraform/` | `suwappu-dag/terraform.tfstate` (existing in `backend.tf`) |
| `terraform/perf/` | `suwappu-dag/perf/terraform.tfstate` |
| `terraform/bootstrap/` | LOCAL — never moves to S3 |
