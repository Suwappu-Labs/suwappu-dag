# 4-region SUWAPPU devnet — always-on, public RPC enabled, persistent state.
#
# Region → authority_id mapping is stable. Any future region rotation MUST
# regenerate genesis (the manifest's validators[N] must match
# `authority_id = N` here).
#
# vs. terraform/perf/:
#   - 4 regions instead of 7 (cost + ops surface area).
#   - Persistent EBS for /var/lib/suwappu so daemon restarts and instance
#     replacements don't wipe consensus state.
#   - rpc_listen ENABLED on each validator (perf testnet leaves it off
#     to avoid measurement skew).
#   - Billing alarm hard cap; SNS subscription is a required variable.

# Shared artifact bucket for binary + genesis + per-region config + logs.
# Lives in us-east-1; every validator's instance profile gets read access.
resource "aws_s3_bucket" "artifacts" {
  provider      = aws.us_east_1
  bucket        = var.artifact_bucket
  force_destroy = false # devnet is long-lived; do not allow wipe via terraform
}

resource "aws_s3_bucket_public_access_block" "artifacts" {
  provider                = aws.us_east_1
  bucket                  = aws_s3_bucket.artifacts.id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_versioning" "artifacts" {
  provider = aws.us_east_1
  bucket   = aws_s3_bucket.artifacts.id
  versioning_configuration {
    status = "Enabled"
  }
}

# Lifecycle: aggressive cleanup of CodeBuild source bundles + log
# tiering. The devnet runs forever; without lifecycle the events.ndjson
# upload prefix would grow without bound.
resource "aws_s3_bucket_lifecycle_configuration" "artifacts" {
  provider = aws.us_east_1
  bucket   = aws_s3_bucket.artifacts.id

  rule {
    id     = "abort-incomplete-multipart"
    status = "Enabled"
    filter {}
    abort_incomplete_multipart_upload {
      days_after_initiation = 7
    }
  }

  rule {
    id     = "expire-sources"
    status = "Enabled"
    filter {
      prefix = "sources/"
    }
    expiration {
      days = 7
    }
    noncurrent_version_expiration {
      noncurrent_days = 7
    }
  }

  # Validator events.ndjson uploads land under logs/<region>/; tier
  # them down so a long-lived devnet doesn't accumulate STANDARD
  # storage cost forever.
  rule {
    id     = "tier-and-expire-logs"
    status = "Enabled"
    filter {
      prefix = "logs/"
    }
    transition {
      days          = 30
      storage_class = "STANDARD_IA"
    }
    transition {
      days          = 90
      storage_class = "GLACIER_IR"
    }
    expiration {
      days = 365
    }
  }

  rule {
    id     = "noncurrent-version-cleanup"
    status = "Enabled"
    filter {}
    noncurrent_version_expiration {
      noncurrent_days = 30
    }
  }
}

# Billing cap. SNS subscription must be confirmed by the operator email
# out-of-band before the alarm has somewhere to publish.
resource "aws_sns_topic" "billing_alarm" {
  provider = aws.us_east_1
  name     = "suwappu-devnet-billing-alarm"
}

resource "aws_sns_topic_subscription" "billing_alarm_email" {
  provider  = aws.us_east_1
  topic_arn = aws_sns_topic.billing_alarm.arn
  protocol  = "email"
  endpoint  = var.billing_alarm_email
}

# CloudWatch billing metrics live in us-east-1 only (AWS global service).
resource "aws_cloudwatch_metric_alarm" "monthly_billing_cap" {
  provider            = aws.us_east_1
  alarm_name          = "suwappu-devnet-monthly-billing-cap"
  alarm_description   = "Projected/actual monthly spend on the devnet exceeded ${var.monthly_billing_cap_usd} USD. Investigate via Cost Explorer."
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "EstimatedCharges"
  namespace           = "AWS/Billing"
  period              = 21600 # 6h — CloudWatch billing metrics update every 6h
  statistic           = "Maximum"
  threshold           = var.monthly_billing_cap_usd
  dimensions = {
    Currency = "USD"
  }
  alarm_actions      = [aws_sns_topic.billing_alarm.arn]
  ok_actions         = [aws_sns_topic.billing_alarm.arn]
  treat_missing_data = "notBreaching"
}

# Per-region validator instances. Authority IDs are baked into genesis;
# changing them after-the-fact is a hard fork.
module "us_east_1" {
  source            = "./modules/validator"
  providers         = { aws = aws.us_east_1 }
  region_label      = "us-east-1"
  authority_id      = 0
  instance_type     = var.instance_type
  ssh_public_key    = var.ssh_public_key
  operator_ip_cidrs = var.operator_ip_cidrs
  consensus_port    = var.consensus_port
  client_port       = var.client_port
  rpc_port          = var.rpc_port
  metrics_port      = var.metrics_port
  artifact_bucket   = aws_s3_bucket.artifacts.id
  state_volume_gb   = var.state_volume_gb
}

module "eu_west_1" {
  source            = "./modules/validator"
  providers         = { aws = aws.eu_west_1 }
  region_label      = "eu-west-1"
  authority_id      = 1
  instance_type     = var.instance_type
  ssh_public_key    = var.ssh_public_key
  operator_ip_cidrs = var.operator_ip_cidrs
  consensus_port    = var.consensus_port
  client_port       = var.client_port
  rpc_port          = var.rpc_port
  metrics_port      = var.metrics_port
  artifact_bucket   = aws_s3_bucket.artifacts.id
  state_volume_gb   = var.state_volume_gb
}

module "ap_southeast_1" {
  source            = "./modules/validator"
  providers         = { aws = aws.ap_southeast_1 }
  region_label      = "ap-southeast-1"
  authority_id      = 2
  instance_type     = var.instance_type
  ssh_public_key    = var.ssh_public_key
  operator_ip_cidrs = var.operator_ip_cidrs
  consensus_port    = var.consensus_port
  client_port       = var.client_port
  rpc_port          = var.rpc_port
  metrics_port      = var.metrics_port
  artifact_bucket   = aws_s3_bucket.artifacts.id
  state_volume_gb   = var.state_volume_gb
}

module "sa_east_1" {
  source            = "./modules/validator"
  providers         = { aws = aws.sa_east_1 }
  region_label      = "sa-east-1"
  authority_id      = 3
  instance_type     = var.instance_type
  ssh_public_key    = var.ssh_public_key
  operator_ip_cidrs = var.operator_ip_cidrs
  consensus_port    = var.consensus_port
  client_port       = var.client_port
  rpc_port          = var.rpc_port
  metrics_port      = var.metrics_port
  artifact_bucket   = aws_s3_bucket.artifacts.id
  state_volume_gb   = var.state_volume_gb
}
