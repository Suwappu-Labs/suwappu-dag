# Remote state in S3, locked by DynamoDB.
#
# The bucket + table are provisioned by `terraform/bootstrap/`. Run the
# bootstrap config once before `terraform init` here. To migrate an existing
# local `terraform.tfstate` into S3:
#
#     AWS_PROFILE=gsn terraform init -migrate-state
#
# Confirm "yes" when terraform asks to copy local state into the new backend.
# After migration, delete the local `terraform.tfstate*` files.

terraform {
  backend "s3" {
    bucket         = "gsx-dag-tf-state"
    key            = "gsx-dag/perf/terraform.tfstate"
    region         = "us-east-1"
    profile        = "gsn"
    dynamodb_table = "gsx-dag-tf-locks"
    encrypt        = true
  }
}
