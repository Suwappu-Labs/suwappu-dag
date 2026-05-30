# AWS WAF in front of the public RPC + faucet CloudFront distributions.
#
# Scope is CLOUDFRONT (not REGIONAL) because the fronting layer moved
# from ALB → CloudFront in Phase 1 — see
# /Users/mongolraider/.claude/plans/validated-prancing-curry.md.
#
# CloudFront-scoped web ACLs MUST be created in us-east-1 regardless
# of the distribution's edge footprint (AWS constraint, not ours).
# Same provider alias as the REGIONAL ACL had before.
#
# Association is via `web_acl_id` ON each
# `aws_cloudfront_distribution.*` resource (see cf_rpc.tf, cf_faucet.tf)
# — CloudFront-scoped ACLs do NOT use `aws_wafv2_web_acl_association`.

resource "aws_wafv2_web_acl" "testnet_cf" {
  provider = aws.us_east_1
  name     = "gsx-testnet-cf-waf"
  scope    = "CLOUDFRONT"

  default_action {
    allow {}
  }

  rule {
    name     = "rate-limit-per-ip"
    priority = 0

    action {
      block {}
    }

    statement {
      rate_based_statement {
        limit              = 10000 # 5× devnet — testnet supports higher dApp TPS
        aggregate_key_type = "IP"
      }
    }

    visibility_config {
      cloudwatch_metrics_enabled = true
      metric_name                = "gsx-testnet-rate-limit-per-ip"
      sampled_requests_enabled   = true
    }
  }

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
      metric_name                = "gsx-testnet-common-rules"
      sampled_requests_enabled   = true
    }
  }

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
      metric_name                = "gsx-testnet-ip-reputation"
      sampled_requests_enabled   = true
    }
  }

  visibility_config {
    cloudwatch_metrics_enabled = true
    metric_name                = "gsx-testnet-cf-waf"
    sampled_requests_enabled   = true
  }

  tags = { Name = "gsx-testnet-cf-waf" }
}
