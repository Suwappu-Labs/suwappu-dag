# CloudFront distribution fronting `faucet.devnet.gsx.globalsettlement.com`.
#
# Ported from terraform/testnet/cf_faucet.tf. Single origin (the faucet
# EC2's EIP) since the faucet is a singleton — no origin group; if the
# faucet is down, drips fail (there's nothing to fail over to). TTL
# pinned to zero: every `/faucet` POST mutates substrate state. No WAF,
# matching the devnet's other CloudFront distros (see cf_rpc.tf note);
# the faucet's own per-IP token bucket bounds abuse.
#
# Origin DNS strategy: same as cf_rpc.tf — synthesise the AWS-provided
# EC2 public hostname from the faucet EIP rather than reference a name
# in the devnet subzone. The subzone isn't publicly delegated on first
# apply (its NS records land in the apex zone only after the operator
# pastes them in), so a CloudFront distribution whose origin lives in
# that subzone fails creation with `InvalidOrigin`. The
# `amazonaws.com` zone is globally delegated, which sidesteps the
# bootstrap problem. Faucet EIP is in us-east-1, so the legacy
# `compute-1.amazonaws.com` suffix applies. See cf_rpc.tf header.

resource "aws_cloudfront_distribution" "faucet" {
  provider        = aws.us_east_1
  enabled         = true
  is_ipv6_enabled = true
  comment         = "gsx-devnet faucet fronting"
  aliases         = ["faucet.${var.devnet_subdomain}.${var.apex_domain}"]
  price_class     = "PriceClass_100"

  origin {
    domain_name = "ec2-${replace(aws_eip.faucet.public_ip, ".", "-")}.compute-1.amazonaws.com"
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
    cached_methods           = ["GET", "HEAD"]
    compress                 = true
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

  tags = { Name = "gsx-devnet-faucet-cf" }
}

output "cf_faucet_distribution_id" {
  description = "CloudFront distribution ID for the devnet faucet endpoint."
  value       = aws_cloudfront_distribution.faucet.id
}

output "cf_faucet_domain_name" {
  description = "Distribution domain (d*.cloudfront.net). DNS alias in dns.tf points at this."
  value       = aws_cloudfront_distribution.faucet.domain_name
}
