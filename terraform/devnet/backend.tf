# Remote state in S3, locked by DynamoDB.
#
# Shares the same `gsx-dag-tf-state` bucket + `gsx-dag-tf-locks` table
# provisioned by `terraform/bootstrap/`. Separate state key from perf
# so the two environments can be applied independently.

terraform {
  backend "s3" {
    bucket         = "gsx-dag-tf-state"
    key            = "gsx-dag/devnet/terraform.tfstate"
    region         = "us-east-1"
    profile        = "gsn"
    dynamodb_table = "gsx-dag-tf-locks"
    encrypt        = true
  }
}
