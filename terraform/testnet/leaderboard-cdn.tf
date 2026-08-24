# HTTPS front for the validator-program leaderboard API.
#
# The points-accumulator daemon serves plain HTTP on port 8090 of the
# program EC2 (`program.testnet.suwappu.bot:8090`, see
# validator-program.tf + dns.tf). Browser frontends — the
# compute-provider portal's earnings lookup — cannot call an http://
# origin from an https:// page (mixed content), so this distribution
# terminates TLS at CloudFront and proxies to the existing origin:
#
#   https://leaderboard.testnet.suwappu.bot/leaderboard
#     -> http://program.testnet.suwappu.bot:8090/leaderboard
#
# Deliberately additive: the `program.` A record is untouched, so any
# operator tooling hitting port 8090 directly keeps working. Caching
# is disabled (the API stamps `computed_at` and stays fresh); CORS
# headers come from the daemon itself (leaderboard.rs
# `add_public_cors`), not from CloudFront.

resource "aws_cloudfront_distribution" "leaderboard" {
  provider        = aws.us_east_1
  enabled         = true
  is_ipv6_enabled = true
  comment         = "suwappu-testnet leaderboard API (TLS front over program EC2)"
  aliases         = ["leaderboard.${var.testnet_subdomain}.${var.apex_domain}"]
  price_class     = "PriceClass_100"

  origin {
    domain_name = "program.${var.testnet_subdomain}.${var.apex_domain}"
    origin_id   = "program-http-8090"

    custom_origin_config {
      http_port              = 8090
      https_port             = 443 # unused; origin_protocol_policy is http-only
      origin_protocol_policy = "http-only"
      origin_ssl_protocols   = ["TLSv1.2"]
    }
  }

  default_cache_behavior {
    target_origin_id       = "program-http-8090"
    viewer_protocol_policy = "redirect-to-https"
    allowed_methods        = ["GET", "HEAD"]
    cached_methods         = ["GET", "HEAD"]
    compress               = true
    # AWS-managed CachingDisabled — the leaderboard must be live.
    cache_policy_id = "4135ea2d-6df8-44a3-9df3-4b5a84be39ad"
    # AWS-managed AllViewerExceptHostHeader — forward query/headers,
    # let CloudFront set Host to the origin domain.
    origin_request_policy_id = "b689b0a8-53d0-40ab-baf2-68738e2966ac"
  }

  restrictions {
    geo_restriction {
      restriction_type = "none"
    }
  }

  viewer_certificate {
    acm_certificate_arn      = aws_acm_certificate_validation.wildcard.certificate_arn
    ssl_support_method       = "sni-only"
    minimum_protocol_version = "TLSv1.2_2021"
  }

  tags = { Name = "suwappu-testnet-leaderboard" }
}

resource "aws_route53_record" "leaderboard" {
  provider = aws.us_east_1
  zone_id  = aws_route53_zone.testnet.zone_id
  name     = "leaderboard.${var.testnet_subdomain}.${var.apex_domain}"
  type     = "A"

  alias {
    name                   = aws_cloudfront_distribution.leaderboard.domain_name
    zone_id                = aws_cloudfront_distribution.leaderboard.hosted_zone_id
    evaluate_target_health = false
  }
}

output "leaderboard_api" {
  description = "TLS-fronted leaderboard API consumed by the compute-provider portal."
  value = {
    url             = "https://leaderboard.${var.testnet_subdomain}.${var.apex_domain}/leaderboard"
    distribution_id = aws_cloudfront_distribution.leaderboard.id
  }
}
