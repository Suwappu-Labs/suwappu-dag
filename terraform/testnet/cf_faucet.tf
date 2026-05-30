# CloudFront distribution fronting `faucet.testnet.gsx.globalsettlement.com`.
#
# Phase 1 fronting per
# /Users/mongolraider/.claude/plans/validated-prancing-curry.md.
#
# Same shape as cf_rpc.tf but with a single origin (the faucet EC2's
# EIP) since the faucet is a singleton. No origin group needed — if
# the faucet is down, drip requests fail; there's no other faucet to
# fail over to.
#
# TTL still pinned to zero: every `/faucet` POST mutates substrate
# state and must hit the origin. `/health` is fine to cache but we
# don't bother — health-check freshness is more valuable than a
# 1-byte savings per probe.

resource "aws_cloudfront_distribution" "faucet" {
  provider        = aws.us_east_1
  enabled         = true
  is_ipv6_enabled = true
  comment         = "gsx-testnet faucet fronting (Phase 1)"
  aliases         = ["faucet.${var.testnet_subdomain}.${var.apex_domain}"]
  price_class     = "PriceClass_100"
  web_acl_id      = aws_wafv2_web_acl.testnet_cf.arn

  origin {
    # CloudFront requires a resolvable DNS name; the Route53 A record
    # in dns.tf resolves to `aws_eip.faucet.public_ip`. Apply fails on
    # a raw IP literal here. (Codex #228 P1 — `cf_faucet.tf:26`.)
    domain_name = aws_route53_record.origin_faucet.fqdn
    origin_id   = "faucet-ec2"
    custom_origin_config {
      http_port              = 8080
      https_port             = 443
      origin_protocol_policy = "http-only"
      origin_ssl_protocols   = ["TLSv1.2"]
    }
  }

  default_cache_behavior {
    target_origin_id       = "faucet-ec2"
    viewer_protocol_policy = "redirect-to-https"
    allowed_methods = [
      "GET", "HEAD", "OPTIONS",
      "PUT", "POST", "PATCH", "DELETE",
    ]
    cached_methods = ["GET", "HEAD"]
    compress       = true

    cache_policy_id          = "4135ea2d-6df8-44a3-9df3-4b5a84be39ad" # CachingDisabled
    origin_request_policy_id = "216adef6-5c7f-47e4-b989-5492eafa07d3" # AllViewer
  }

  restrictions {
    geo_restriction { restriction_type = "none" }
  }

  viewer_certificate {
    acm_certificate_arn      = aws_acm_certificate_validation.wildcard.certificate_arn
    ssl_support_method       = "sni-only"
    minimum_protocol_version = "TLSv1.2_2021"
  }

  tags = { Name = "gsx-testnet-faucet-cf" }
}

output "cf_faucet_distribution_id" {
  description = "CloudFront distribution ID for the faucet endpoint."
  value       = aws_cloudfront_distribution.faucet.id
}

output "cf_faucet_domain_name" {
  description = "Distribution domain (d*.cloudfront.net). DNS alias in dns.tf points at this."
  value       = aws_cloudfront_distribution.faucet.domain_name
}
