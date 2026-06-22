# G2 — AWS WAF in front of the public RPC + faucet ALBs.
#
# Defense in depth on top of the application-layer per-IP rate
# limiter (F1, `crates/suwappu-rpc/src/per_ip.rs`). WAF gates:
#   * Rate-limit per IP at the edge (separate from the app limiter
#     so a flood doesn't even reach the validator).
#   * Block obvious abuse — oversize request, malformed
#     Content-Type, AWS managed common ruleset.
#
# WAF is intentionally light. The validator's own
# `RouterLimits::max_concurrent_requests` (64) + per-IP bucket
# (60 burst / 10 req/s) are the load-bearing protections; WAF is
# the airbag.

resource "aws_wafv2_web_acl" "devnet" {
  provider = aws.us_east_1
  name     = "suwappu-devnet-waf"
  scope    = "REGIONAL"

  default_action {
    allow {}
  }

  # Edge per-IP rate limit. Generous — the app-layer limiter is
  # the precise control; this is for crude floods.
  rule {
    name     = "rate-limit-per-ip"
    priority = 0

    action {
      block {}
    }

    statement {
      rate_based_statement {
        limit              = 2000 # requests per 5-min window per IP
        aggregate_key_type = "IP"
      }
    }

    visibility_config {
      cloudwatch_metrics_enabled = true
      metric_name                = "suwappu-devnet-rate-limit-per-ip"
      sampled_requests_enabled   = true
    }
  }

  # AWS managed: known-bad inputs (SQLi, XSS, oversized request
  # body, malformed UTF-8, etc.). Blocks before the request hits
  # the validator. Standard AWS-curated ruleset.
  rule {
    name     = "aws-common-ruleset"
    priority = 1

    override_action {
      none {}
    }

    statement {
      managed_rule_group_statement {
        name        = "AWSManagedRulesCommonRuleSet"
        vendor_name = "AWS"
      }
    }

    visibility_config {
      cloudwatch_metrics_enabled = true
      metric_name                = "suwappu-devnet-common-rules"
      sampled_requests_enabled   = true
    }
  }

  # AWS managed: known-bad IP addresses (Amazon IP-reputation list).
  # Blocks Tor exit nodes + bulletproof hosters known for abuse.
  rule {
    name     = "aws-ip-reputation"
    priority = 2

    override_action {
      none {}
    }

    statement {
      managed_rule_group_statement {
        name        = "AWSManagedRulesAmazonIpReputationList"
        vendor_name = "AWS"
      }
    }

    visibility_config {
      cloudwatch_metrics_enabled = true
      metric_name                = "suwappu-devnet-ip-reputation"
      sampled_requests_enabled   = true
    }
  }

  visibility_config {
    cloudwatch_metrics_enabled = true
    metric_name                = "suwappu-devnet-waf"
    sampled_requests_enabled   = true
  }

  tags = { Name = "suwappu-devnet-waf" }
}

resource "aws_wafv2_web_acl_association" "rpc" {
  provider     = aws.us_east_1
  resource_arn = aws_lb.rpc.arn
  web_acl_arn  = aws_wafv2_web_acl.devnet.arn
}

resource "aws_wafv2_web_acl_association" "faucet" {
  provider     = aws.us_east_1
  resource_arn = aws_lb.faucet.arn
  web_acl_arn  = aws_wafv2_web_acl.devnet.arn
}
