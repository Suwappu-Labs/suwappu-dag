output "state_bucket" {
  description = "S3 bucket name used as the terraform remote backend by every other config."
  value       = aws_s3_bucket.tf_state.id
}

output "lock_table" {
  description = "DynamoDB table name used for terraform state locking."
  value       = aws_dynamodb_table.tf_locks.name
}
