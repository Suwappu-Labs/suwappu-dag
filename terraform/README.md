# Terraform — suwappu-dag AWS infrastructure

Targets the **gsn** AWS profile (account `492042618949`, region `us-east-1`).

## Layout

```text
terraform/
├── backend.tf            # S3 + DynamoDB remote state
├── providers.tf          # AWS provider pinned to gsn profile
├── main.tf               # Composition of validator-node module
├── variables.tf
├── outputs.tf
└── modules/
    └── validator-node/   # EC2 validator (c6i.4xlarge target)
        ├── main.tf
        ├── variables.tf
        └── outputs.tf
```

## Bootstrap (one-time, manual)

The S3 state bucket and DynamoDB lock table are created out-of-band before
`terraform init`:

```bash
AWS_PROFILE=gsn aws s3api create-bucket \
    --bucket suwappu-dag-tf-state \
    --region us-east-1

AWS_PROFILE=gsn aws s3api put-bucket-versioning \
    --bucket suwappu-dag-tf-state \
    --versioning-configuration Status=Enabled

AWS_PROFILE=gsn aws dynamodb create-table \
    --table-name suwappu-dag-tf-locks \
    --attribute-definitions AttributeName=LockID,AttributeType=S \
    --key-schema AttributeName=LockID,KeyType=HASH \
    --billing-mode PAY_PER_REQUEST \
    --region us-east-1
```

## Apply

Never run `terraform apply` directly. Use the wrapper:

```bash
scripts/deploy-aws.sh plan      # always
scripts/deploy-aws.sh apply     # requires confirmation
```

`terraform destroy` is denied by the Claude Code denylist
(`claude-code/settings.json`).

## Status

**Skeleton only.** No resources defined yet. Phase A (DAG-S1, S2) does not
need AWS. Real validator-node provisioning lands in DAG-S20 (full node E2E).
