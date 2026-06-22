# Remote state in S3, locked by DynamoDB.
#
# Shares the same `suwappu-dag-tf-state` bucket + `suwappu-dag-tf-locks` table
# provisioned by `terraform/bootstrap/`. Separate state key from perf
# so the two environments can be applied independently.

terraform {
  backend "s3" {
    bucket         = "suwappu-dag-tf-state"
    key            = "suwappu-dag/devnet/terraform.tfstate"
    region         = "us-east-1"
    profile        = "gsn"
    dynamodb_table = "suwappu-dag-tf-locks"
    encrypt        = true
  }
}
