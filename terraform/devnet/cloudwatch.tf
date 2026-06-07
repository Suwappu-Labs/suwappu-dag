# G6: live monitoring — dashboard + alarms.
#
# The cloud-init scripts in modules/validator/cloud-init.yaml +
# faucet-cloud-init.yaml install amazon-cloudwatch-agent and point
# it at the local Prometheus exporter (suwappu-node:9093, suwappu-faucet has
# no exporter v0.1 — its /health endpoint is the liveness signal
# the alarm scrapes via HTTP). Metrics land in the `suwappu-devnet`
# CloudWatch namespace under the `region` + `authority_id` dimensions
# the agent's emf_processor declares.
#
# Alarms:
#
#  - **suwappu-devnet-halt** (cluster-wide).
#    Fires when `suwappu_last_committed_round` is FLAT across all 4
#    validators for >5 min. CloudWatch math expression compares the
#    current value to a 5-min-old sample (delta). Pages ops via SNS.
#
#  - **suwappu-devnet-silent-peer-<region>** (one per validator).
#    Fires when this validator's `suwappu_metrics_scrapes_total` stops
#    incrementing for >2 min — proxy for "this validator stopped
#    serving" without us needing per-peer inbound counters.
#    Emails ops (not paging — a single silent validator with the
#    cluster still committing is degraded, not halted).
#
#  - **suwappu-devnet-faucet-down** (faucet).
#    Fires when the faucet's HTTP /health returns non-200 for >5 min.
#    Implementation: CloudWatch Synthetics canary OR a simple
#    Route53 health check probing the faucet ALB. For v0.1 we use
#    a Route53 health check that scrapes /health every 30s, which
#    is then surfaced as a CloudWatch metric (HealthCheckStatus).
#    G2 adds the actual canary endpoint (`faucet.devnet.suwappu.*`);
#    pre-G2 the canary URL points at the faucet's EIP.

resource "aws_sns_topic" "ops_pages" {
  provider = aws.us_east_1
  name     = "suwappu-devnet-ops-pages"
}

resource "aws_sns_topic_subscription" "ops_pages_email" {
  provider  = aws.us_east_1
  topic_arn = aws_sns_topic.ops_pages.arn
  protocol  = "email"
  endpoint  = var.billing_alarm_email # reuse same subscriber; can split later
}

# Halt alarm — composite math expression across all 4 validators'
# last_committed_round. We can't pick a single SUM/AVG aggregator
# because the regions have different absolute values (region with
# the highest committed round wins). Instead, compute the MAX across
# regions and compare against a lagged sample.
resource "aws_cloudwatch_metric_alarm" "halt" {
  provider            = aws.us_east_1
  alarm_name          = "suwappu-devnet-halt"
  alarm_description   = "Devnet has stopped progressing: `suwappu_last_committed_round` MAX across all validators didn't advance in the last 5 minutes. Investigate via OPERATIONS.md § 'Diagnose stuck commits'."
  comparison_operator = "LessThanOrEqualToThreshold"
  evaluation_periods  = 2 # 2 consecutive 1-min windows of no progress
  threshold           = 0
  treat_missing_data  = "breaching" # missing metrics = alarm; better noisy than silent

  metric_query {
    id          = "current_max"
    return_data = false
    metric {
      metric_name = "suwappu_last_committed_round"
      namespace   = "suwappu-devnet"
      period      = 60
      stat        = "Maximum"
    }
  }

  # Per-period rate of change in committed-round MAX. `RATE` returns
  # a TimeSeries (one data-point per period). A halt is detectable as
  # `rate ≤ 0` for two consecutive windows (counter flat or regressed).
  # The previous shape (`MAX(current_max) - MAX(lagged_max)`) collapsed
  # to a scalar, which CloudWatch rejects for an alarm-returning
  # expression. Mirrors the testnet fix in
  # terraform/testnet/cloudwatch.tf (PR #220).
  metric_query {
    id          = "delta"
    return_data = true
    expression  = "RATE(current_max)"
    label       = "round advance rate"
  }

  alarm_actions = [aws_sns_topic.ops_pages.arn]
  ok_actions    = [aws_sns_topic.ops_pages.arn]
}

# Silent-peer alarm: one per validator. The per-region region
# label is picked from `suwappu_node_info`. We don't have a way to
# loop over the 4 regions in pure HCL without a `for_each` and
# a static list, so the list is mirrored from main.tf.
locals {
  validator_regions = [
    "us-east-1",
    "eu-west-1",
    "ap-southeast-1",
    "sa-east-1",
  ]
}

resource "aws_cloudwatch_metric_alarm" "silent_peer" {
  for_each            = toset(local.validator_regions)
  provider            = aws.us_east_1
  alarm_name          = "suwappu-devnet-silent-peer-${each.key}"
  alarm_description   = "Validator ${each.key} stopped serving /metrics scrapes — likely crashed, locked up, or unreachable from CloudWatch agent. Cluster may still commit if joint quorum survives with the remaining 3."
  comparison_operator = "LessThanThreshold"
  evaluation_periods  = 2
  metric_name         = "suwappu_metrics_scrapes_total"
  namespace           = "suwappu-devnet"
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

# G2 — faucet liveness via Route53 HTTPS health check on the DNS
# name. Uses the wildcard ACM cert + the faucet ALB. Pre-G2 this
# pointed at the raw EIP:8080; post-G2 it follows the same DNS
# path external devs use.
resource "aws_route53_health_check" "faucet" {
  provider          = aws.us_east_1
  fqdn              = "faucet.${var.devnet_subdomain}.${var.apex_domain}"
  port              = 443
  type              = "HTTPS"
  resource_path     = "/health"
  request_interval  = 30
  failure_threshold = 3
  tags = {
    Name = "suwappu-devnet-faucet-health"
  }
}

resource "aws_cloudwatch_metric_alarm" "faucet_down" {
  provider            = aws.us_east_1
  alarm_name          = "suwappu-devnet-faucet-down"
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

# Dashboard. Single page with three rows: tip-round chart per
# region, mempool size per region, faucet health.
resource "aws_cloudwatch_dashboard" "devnet" {
  provider       = aws.us_east_1
  dashboard_name = "suwappu-devnet"
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
            ["suwappu-devnet", "suwappu_last_committed_round", "region", "us-east-1"],
            ["...", "eu-west-1"],
            ["...", "ap-southeast-1"],
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
            ["suwappu-devnet", "suwappu_mempool_size", "region", "us-east-1"],
            ["...", "eu-west-1"],
            ["...", "ap-southeast-1"],
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
          title  = "Faucet /health (Route53 check)"
          region = "us-east-1"
          metrics = [
            ["AWS/Route53", "HealthCheckStatus", "HealthCheckId", aws_route53_health_check.faucet.id],
          ]
          view   = "timeSeries"
          stat   = "Minimum"
          period = 60
        }
      },
    ]
  })
}
