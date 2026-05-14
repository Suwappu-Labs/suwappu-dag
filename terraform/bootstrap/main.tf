# Terraform-state bootstrap.
#
# Creates the S3 bucket + DynamoDB table that every other terraform config in
# this repo uses as its remote backend. This config itself uses LOCAL state on
# purpose — it's a chicken-and-egg solver, run once.
#
# Apply once:
#
#     cd terraform/bootstrap
#     AWS_PROFILE=gsn terraform init
#     AWS_PROFILE=gsn terraform apply
#
# After apply, the bucket+table exist and all sibling configs
# (`terraform/`, `terraform/perf/`) can run `terraform init -migrate-state`
# to push their state into S3.
#
# The bucket has `prevent_destroy = true` and the table has PITR enabled —
# losing this state would orphan every resource in the gsn account.

resource "aws_s3_bucket" "tf_state" {
  bucket = "gsx-dag-tf-state"

  lifecycle {
    prevent_destroy = true
  }
}

resource "aws_s3_bucket_versioning" "tf_state" {
  bucket = aws_s3_bucket.tf_state.id
  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "tf_state" {
  bucket = aws_s3_bucket.tf_state.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

resource "aws_s3_bucket_public_access_block" "tf_state" {
  bucket                  = aws_s3_bucket.tf_state.id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_dynamodb_table" "tf_locks" {
  name         = "gsx-dag-tf-locks"
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "LockID"

  attribute {
    name = "LockID"
    type = "S"
  }

  point_in_time_recovery {
    enabled = true
  }

  lifecycle {
    prevent_destroy = true
  }
}
