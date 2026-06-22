# Testnet live monitoring — dashboard + alarms.
#
# Fork of terraform/devnet/cloudwatch.tf. Diffs:
#   * 7 silent-peer alarms (one per seed validator) instead of 4.
#   * Metric namespace `suwappu-testnet` (vs `suwappu-devnet`).
#   * Halt alarm + faucet alarm target the testnet's Route53 health
#     check on `faucet.testnet.suwappu.suwappu.bot`.
#   * SNS topic name `suwappu-testnet-ops-pages`.

resource "aws_sns_topic" "ops_pages" {
  provider = aws.us_east_1
  name     = "suwappu-testnet-ops-pages"
}

resource "aws_sns_topic_subscription" "ops_pages_email" {
  provider  = aws.us_east_1
  topic_arn = aws_sns_topic.ops_pages.arn
  protocol  = "email"
  endpoint  = var.billing_alarm_email # reuse same subscriber; can split later
}

# Halt alarm — MAX(last_committed_round) across all 7 validators
# flat for 2 consecutive 1-min windows.
resource "aws_cloudwatch_metric_alarm" "halt" {
  provider            = aws.us_east_1
  alarm_name          = "suwappu-testnet-halt"
  alarm_description   = "Testnet has stopped progressing: `suwappu_last_committed_round` MAX across all validators didn't advance in the last 5 minutes. Investigate via OPERATIONS.md § 'Diagnose stuck commits'."
  comparison_operator = "LessThanOrEqualToThreshold"
  evaluation_periods  = 2
  threshold           = 0
  treat_missing_data  = "breaching"

  metric_query {
    id          = "current_max"
    return_data = false
    metric {
      metric_name = "suwappu_last_committed_round"
      namespace   = "suwappu-testnet"
      period      = 60
      stat        = "Maximum"
    }
  }

  # Per-period rate of change in committed-round MAX. `RATE` returns
  # a TimeSeries (one data-point per period). A halt is detectable as
  # `rate ≤ 0` for two consecutive windows (counter flat or
  # regressed). The previous shape (`MAX(current_max) - MAX(lagged_max)`)
  # collapsed to a scalar, which CloudWatch rejects for an
  # alarm-returning expression.
  metric_query {
    id          = "delta"
    return_data = true
    expression  = "RATE(current_max)"
    label       = "round advance rate"
  }

  alarm_actions = [aws_sns_topic.ops_pages.arn]
  ok_actions    = [aws_sns_topic.ops_pages.arn]
}

# Silent-peer alarms — one per seed validator. The 7-region list
# mirrors main.tf module instantiations.
locals {
  validator_regions = [
    "us-east-1",
    "us-west-2",
    "eu-west-1",
    "eu-central-1",
    "ap-southeast-1",
    "ap-northeast-1",
    "sa-east-1",
  ]
}

resource "aws_cloudwatch_metric_alarm" "silent_peer" {
  for_each            = toset(local.validator_regions)
  provider            = aws.us_east_1
  alarm_name          = "suwappu-testnet-silent-peer-${each.key}"
  alarm_description   = "Validator ${each.key} stopped serving /metrics scrapes — likely crashed, locked up, or unreachable from CloudWatch agent. Cluster may still commit if joint quorum survives with the remaining 6."
  comparison_operator = "LessThanThreshold"
  evaluation_periods  = 2
  metric_name         = "suwappu_metrics_scrapes_total"
  namespace           = "suwappu-testnet"
  period              = 60
  statistic           = "Sum"
  threshold           = 1
  treat_missing_data  = "breaching"
  dimensions = {
    region = each.key
  }
  alarm_actions = [aws_sns_topic.ops_pages.arn]
  ok_actions    = [aws_sns_topic.ops_pages.arn]
}

# Faucet liveness via Route53 HTTPS health check on the public DNS.
resource "aws_route53_health_check" "faucet" {
  provider          = aws.us_east_1
  fqdn              = "faucet.${var.testnet_subdomain}.${var.apex_domain}"
  port              = 443
  type              = "HTTPS"
  resource_path     = "/health"
  request_interval  = 30
  failure_threshold = 3
  tags = {
    Name = "suwappu-testnet-faucet-health"
  }
}

resource "aws_cloudwatch_metric_alarm" "faucet_down" {
  provider            = aws.us_east_1
  alarm_name          = "suwappu-testnet-faucet-down"
  alarm_description   = "Faucet /health is failing — devs can't acquire test tokens. Investigate via OPERATIONS.md § 'Restart the faucet service'."
  comparison_operator = "LessThanThreshold"
  evaluation_periods  = 2
  metric_name         = "HealthCheckStatus"
  namespace           = "AWS/Route53"
  period              = 60
  statistic           = "Minimum"
  threshold           = 1
  dimensions = {
    HealthCheckId = aws_route53_health_check.faucet.id
  }
  alarm_actions = [aws_sns_topic.ops_pages.arn]
  ok_actions    = [aws_sns_topic.ops_pages.arn]
}

# Points-accumulator daemon liveness — HTTP probe against the
# program EC2's leaderboard endpoint. Pre-daemon-deploy (the binary
# is a follow-up) the alarm fires permanently; un-mute by deploying
# the daemon OR by setting `treat_missing_data = notBreaching`.
resource "aws_route53_health_check" "program" {
  provider          = aws.us_east_1
  ip_address        = aws_eip.program.public_ip
  port              = 8090
  type              = "HTTP"
  resource_path     = "/leaderboard"
  request_interval  = 30
  failure_threshold = 5 # softer than faucet; daemon is foundation-internal
  tags = {
    Name = "suwappu-testnet-program-health"
  }
}

resource "aws_cloudwatch_metric_alarm" "program_down" {
  provider            = aws.us_east_1
  alarm_name          = "suwappu-testnet-program-down"
  alarm_description   = "Points-accumulator leaderboard endpoint is failing — operators see stale or no leaderboard. NOT a chain-impact alarm; lower urgency than faucet/halt."
  comparison_operator = "LessThanThreshold"
  evaluation_periods  = 4
  metric_name         = "HealthCheckStatus"
  namespace           = "AWS/Route53"
  period              = 60
  statistic           = "Minimum"
  threshold           = 1
  # The daemon binary lands in a follow-up PR; until then this
  # alarm is expected to be in ALARM state. notBreaching keeps the
  # noise down until then.
  treat_missing_data = "notBreaching"
  dimensions = {
    HealthCheckId = aws_route53_health_check.program.id
  }
  alarm_actions = [aws_sns_topic.ops_pages.arn]
  ok_actions    = [aws_sns_topic.ops_pages.arn]
}

# Dashboard — testnet adds a row showing the 7-region tip vs
# expected progress + an external-uploads-bucket activity tile.
resource "aws_cloudwatch_dashboard" "testnet" {
  provider       = aws.us_east_1
  dashboard_name = "suwappu-testnet"
  dashboard_body = jsonencode({
    widgets = [
      {
        type   = "metric"
        x      = 0
        y      = 0
        width  = 24
        height = 6
        properties = {
          title  = "Last committed round (per region)"
          region = "us-east-1"
          metrics = [
            ["suwappu-testnet", "suwappu_last_committed_round", "region", "us-east-1"],
            ["...", "us-west-2"],
            ["...", "eu-west-1"],
            ["...", "eu-central-1"],
            ["...", "ap-southeast-1"],
            ["...", "ap-northeast-1"],
            ["...", "sa-east-1"],
          ]
          view   = "timeSeries"
          stat   = "Maximum"
          period = 60
        }
      },
      {
        type   = "metric"
        x      = 0
        y      = 6
        width  = 12
        height = 6
        properties = {
          title  = "Mempool size (per region)"
          region = "us-east-1"
          metrics = [
            ["suwappu-testnet", "suwappu_mempool_size", "region", "us-east-1"],
            ["...", "us-west-2"],
            ["...", "eu-west-1"],
            ["...", "eu-central-1"],
            ["...", "ap-southeast-1"],
            ["...", "ap-northeast-1"],
            ["...", "sa-east-1"],
          ]
          view   = "timeSeries"
          stat   = "Average"
          period = 60
        }
      },
      {
        type   = "metric"
        x      = 12
        y      = 6
        width  = 12
        height = 6
        properties = {
          title  = "Endpoint health"
          region = "us-east-1"
          metrics = [
            ["AWS/Route53", "HealthCheckStatus", "HealthCheckId", aws_route53_health_check.faucet.id, { "label" : "faucet" }],
            ["...", aws_route53_health_check.program.id, { "label" : "program" }],
          ]
          view   = "timeSeries"
          stat   = "Minimum"
          period = 60
        }
      },
      {
        type   = "metric"
        x      = 0
        y      = 12
        width  = 24
        height = 4
        properties = {
          title  = "External operator uploads (S3 PutObject rate)"
          region = "us-east-1"
          metrics = [
            ["AWS/S3", "NumberOfObjects", "BucketName", aws_s3_bucket.external_uploads.id, "StorageType", "AllStorageTypes"],
          ]
          view   = "timeSeries"
          stat   = "Average"
          period = 300
        }
      },
    ]
  })
}
