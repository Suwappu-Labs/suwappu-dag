# AWS WAF in front of the public RPC + faucet ALBs.
# Same posture as terraform/devnet/waf.tf; bumped rate-limit
# slightly because the testnet supports higher TPS targets.

resource "aws_wafv2_web_acl" "testnet" {
  provider = aws.us_east_1
  name     = "suwappu-testnet-waf"
  scope    = "REGIONAL"

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
      metric_name                = "suwappu-testnet-rate-limit-per-ip"
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
      metric_name                = "suwappu-testnet-common-rules"
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
      metric_name                = "suwappu-testnet-ip-reputation"
      sampled_requests_enabled   = true
    }
  }

  visibility_config {
    cloudwatch_metrics_enabled = true
    metric_name                = "suwappu-testnet-waf"
    sampled_requests_enabled   = true
  }

  tags = { Name = "suwappu-testnet-waf" }
}

resource "aws_wafv2_web_acl_association" "rpc" {
  provider     = aws.us_east_1
  resource_arn = aws_lb.rpc.arn
  web_acl_arn  = aws_wafv2_web_acl.testnet.arn
}

resource "aws_wafv2_web_acl_association" "faucet" {
  provider     = aws.us_east_1
  resource_arn = aws_lb.faucet.arn
  web_acl_arn  = aws_wafv2_web_acl.testnet.arn
}
