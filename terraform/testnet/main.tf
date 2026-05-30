# 7-region GSX testnet seed cluster.
#
# Foundation operates these 7 nodes; external validators (Track B
# points program) bring their own hardware and connect via the
# public RPC + the publicly-discoverable peer addresses below.
#
# Region → authority_id mapping is stable. The 7-of-9 LTP corridor
# (paper §10.2) uses authority_ids 0–6 here; future external
# validators that join via governance get ids ≥ 8 (id 7 reserved
# for the faucet authority per devnet convention).

resource "aws_s3_bucket" "artifacts" {
  provider      = aws.us_east_1
  bucket        = var.artifact_bucket
  force_destroy = false
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

# Billing cap. Higher than devnet's $500/mo because of the larger
# seed set + the validator-program EC2 + RDS.
resource "aws_sns_topic" "billing_alarm" {
  provider = aws.us_east_1
  name     = "gsx-testnet-billing-alarm"
}

resource "aws_sns_topic_subscription" "billing_alarm_email" {
  provider  = aws.us_east_1
  topic_arn = aws_sns_topic.billing_alarm.arn
  protocol  = "email"
  endpoint  = var.billing_alarm_email
}

resource "aws_cloudwatch_metric_alarm" "monthly_billing_cap" {
  provider            = aws.us_east_1
  alarm_name          = "gsx-testnet-monthly-billing-cap"
  alarm_description   = "Projected/actual monthly spend on the testnet exceeded ${var.monthly_billing_cap_usd} USD. Investigate via Cost Explorer + the validator-program traffic charts."
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "EstimatedCharges"
  namespace           = "AWS/Billing"
  period              = 21600
  statistic           = "Maximum"
  threshold           = var.monthly_billing_cap_usd
  dimensions = {
    Currency = "USD"
  }
  alarm_actions      = [aws_sns_topic.billing_alarm.arn]
  ok_actions         = [aws_sns_topic.billing_alarm.arn]
  treat_missing_data = "notBreaching"
}

# Seed validators. The module shape is reused from
# terraform/devnet/modules/validator (no fork) — wider cluster +
# different chain_id is the only diff. Region → authority_id
# mapping is locked.
module "us_east_1" {
  source            = "../devnet/modules/validator"
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
  with_alb_subnets  = true # in-region RPC ALB (regional_alb.tf + ga.tf)
  # Historical: the testnet went live with `gsx-devnet-*` Name tags
  # because the validator module hardcoded that prefix. Those names
  # are baked into AWS state (key_pair, IAM role, SG names — all
  # immutable), so flipping to `gsx-testnet-*` would force
  # destroy+recreate of every seed. The rename is deferred to the
  # next clean window. Until then, devnet (when applied) must use a
  # non-colliding prefix — see terraform/devnet/main.tf.
  name_prefix = "gsx-devnet-"
}

module "us_west_2" {
  source            = "../devnet/modules/validator"
  providers         = { aws = aws.us_west_2 }
  region_label      = "us-west-2"
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
  with_alb_subnets  = true # in-region RPC ALB (regional_alb.tf + ga.tf)
  # Historical: the testnet went live with `gsx-devnet-*` Name tags
  # because the validator module hardcoded that prefix. Those names
  # are baked into AWS state (key_pair, IAM role, SG names — all
  # immutable), so flipping to `gsx-testnet-*` would force
  # destroy+recreate of every seed. The rename is deferred to the
  # next clean window. Until then, devnet (when applied) must use a
  # non-colliding prefix — see terraform/devnet/main.tf.
  name_prefix = "gsx-devnet-"
}

module "eu_west_1" {
  source            = "../devnet/modules/validator"
  providers         = { aws = aws.eu_west_1 }
  region_label      = "eu-west-1"
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
  with_alb_subnets  = true # in-region RPC ALB (regional_alb.tf + ga.tf)
  # Historical: the testnet went live with `gsx-devnet-*` Name tags
  # because the validator module hardcoded that prefix. Those names
  # are baked into AWS state (key_pair, IAM role, SG names — all
  # immutable), so flipping to `gsx-testnet-*` would force
  # destroy+recreate of every seed. The rename is deferred to the
  # next clean window. Until then, devnet (when applied) must use a
  # non-colliding prefix — see terraform/devnet/main.tf.
  name_prefix = "gsx-devnet-"
}

module "eu_central_1" {
  source            = "../devnet/modules/validator"
  providers         = { aws = aws.eu_central_1 }
  region_label      = "eu-central-1"
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
  with_alb_subnets  = true # in-region RPC ALB (regional_alb.tf + ga.tf)
  # Historical: the testnet went live with `gsx-devnet-*` Name tags
  # because the validator module hardcoded that prefix. Those names
  # are baked into AWS state (key_pair, IAM role, SG names — all
  # immutable), so flipping to `gsx-testnet-*` would force
  # destroy+recreate of every seed. The rename is deferred to the
  # next clean window. Until then, devnet (when applied) must use a
  # non-colliding prefix — see terraform/devnet/main.tf.
  name_prefix = "gsx-devnet-"
}

module "ap_southeast_1" {
  source            = "../devnet/modules/validator"
  providers         = { aws = aws.ap_southeast_1 }
  region_label      = "ap-southeast-1"
  authority_id      = 4
  instance_type     = var.instance_type
  ssh_public_key    = var.ssh_public_key
  operator_ip_cidrs = var.operator_ip_cidrs
  consensus_port    = var.consensus_port
  client_port       = var.client_port
  rpc_port          = var.rpc_port
  metrics_port      = var.metrics_port
  artifact_bucket   = aws_s3_bucket.artifacts.id
  state_volume_gb   = var.state_volume_gb
  with_alb_subnets  = true # in-region RPC ALB (regional_alb.tf + ga.tf)
  # Historical: the testnet went live with `gsx-devnet-*` Name tags
  # because the validator module hardcoded that prefix. Those names
  # are baked into AWS state (key_pair, IAM role, SG names — all
  # immutable), so flipping to `gsx-testnet-*` would force
  # destroy+recreate of every seed. The rename is deferred to the
  # next clean window. Until then, devnet (when applied) must use a
  # non-colliding prefix — see terraform/devnet/main.tf.
  name_prefix = "gsx-devnet-"
}

module "ap_northeast_1" {
  source            = "../devnet/modules/validator"
  providers         = { aws = aws.ap_northeast_1 }
  region_label      = "ap-northeast-1"
  authority_id      = 5
  instance_type     = var.instance_type
  ssh_public_key    = var.ssh_public_key
  operator_ip_cidrs = var.operator_ip_cidrs
  consensus_port    = var.consensus_port
  client_port       = var.client_port
  rpc_port          = var.rpc_port
  metrics_port      = var.metrics_port
  artifact_bucket   = aws_s3_bucket.artifacts.id
  state_volume_gb   = var.state_volume_gb
  with_alb_subnets  = true # in-region RPC ALB (regional_alb.tf + ga.tf)
  # Historical: the testnet went live with `gsx-devnet-*` Name tags
  # because the validator module hardcoded that prefix. Those names
  # are baked into AWS state (key_pair, IAM role, SG names — all
  # immutable), so flipping to `gsx-testnet-*` would force
  # destroy+recreate of every seed. The rename is deferred to the
  # next clean window. Until then, devnet (when applied) must use a
  # non-colliding prefix — see terraform/devnet/main.tf.
  name_prefix = "gsx-devnet-"
}

module "sa_east_1" {
  source            = "../devnet/modules/validator"
  providers         = { aws = aws.sa_east_1 }
  region_label      = "sa-east-1"
  authority_id      = 6
  instance_type     = var.instance_type
  ssh_public_key    = var.ssh_public_key
  operator_ip_cidrs = var.operator_ip_cidrs
  consensus_port    = var.consensus_port
  client_port       = var.client_port
  rpc_port          = var.rpc_port
  metrics_port      = var.metrics_port
  artifact_bucket   = aws_s3_bucket.artifacts.id
  state_volume_gb   = var.state_volume_gb
  with_alb_subnets  = true # in-region RPC ALB (regional_alb.tf + ga.tf)
  # Historical: the testnet went live with `gsx-devnet-*` Name tags
  # because the validator module hardcoded that prefix. Those names
  # are baked into AWS state (key_pair, IAM role, SG names — all
  # immutable), so flipping to `gsx-testnet-*` would force
  # destroy+recreate of every seed. The rename is deferred to the
  # next clean window. Until then, devnet (when applied) must use a
  # non-colliding prefix — see terraform/devnet/main.tf.
  name_prefix = "gsx-devnet-"
}
