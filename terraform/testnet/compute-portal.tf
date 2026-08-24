# Testnet compute-provider portal — static-site deployment.
#
# Same shape as testnet/status.tf (single-page vanilla client, no SPA
# fallback rules): private S3 bucket behind CloudFront with OAC, alias
# `compute.testnet.suwappu.bot` on this stack's wildcard ACM.
# Serves clients/provider-portal — the become-a-provider surface
# (roles, earnings calculator, points-program join flow).

resource "aws_s3_bucket" "compute_portal" {
  provider      = aws.us_east_1
  bucket        = "suwappu-dag-testnet-compute"
  force_destroy = false

  tags = { Name = "suwappu-testnet-compute" }
}

resource "aws_s3_bucket_public_access_block" "compute_portal" {
  provider                = aws.us_east_1
  bucket                  = aws_s3_bucket.compute_portal.id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_cloudfront_origin_access_control" "compute_portal" {
  provider                          = aws.us_east_1
  name                              = "suwappu-testnet-compute-oac"
  origin_access_control_origin_type = "s3"
  signing_behavior                  = "always"
  signing_protocol                  = "sigv4"
}

resource "aws_cloudfront_distribution" "compute_portal" {
  provider            = aws.us_east_1
  enabled             = true
  is_ipv6_enabled     = true
  comment             = "suwappu-testnet compute-provider portal"
  default_root_object = "index.html"
  aliases             = ["compute.${var.testnet_subdomain}.${var.apex_domain}"]
  price_class         = "PriceClass_100"

  origin {
    domain_name              = aws_s3_bucket.compute_portal.bucket_regional_domain_name
    origin_id                = "compute-portal-s3"
    origin_access_control_id = aws_cloudfront_origin_access_control.compute_portal.id
  }

  default_cache_behavior {
    target_origin_id       = "compute-portal-s3"
    viewer_protocol_policy = "redirect-to-https"
    allowed_methods        = ["GET", "HEAD"]
    cached_methods         = ["GET", "HEAD"]
    compress               = true
    cache_policy_id        = "658327ea-f89d-4fab-a63d-7e88639e58f6" # AWS-managed CachingOptimized
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

  tags = { Name = "suwappu-testnet-compute" }
}

resource "aws_s3_bucket_policy" "compute_portal" {
  provider = aws.us_east_1
  bucket   = aws_s3_bucket.compute_portal.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Sid       = "AllowCloudFrontServicePrincipalReadOnly"
      Effect    = "Allow"
      Principal = { Service = "cloudfront.amazonaws.com" }
      Action    = ["s3:GetObject"]
      Resource  = ["${aws_s3_bucket.compute_portal.arn}/*"]
      Condition = {
        StringEquals = {
          "AWS:SourceArn" = aws_cloudfront_distribution.compute_portal.arn
        }
      }
    }]
  })
}

resource "aws_route53_record" "compute_portal" {
  provider = aws.us_east_1
  zone_id  = aws_route53_zone.testnet.zone_id
  name     = "compute.${var.testnet_subdomain}.${var.apex_domain}"
  type     = "A"

  alias {
    name                   = aws_cloudfront_distribution.compute_portal.domain_name
    zone_id                = aws_cloudfront_distribution.compute_portal.hosted_zone_id
    evaluate_target_health = false
  }
}

output "compute_portal" {
  description = "Compute-provider portal handle. URL is the public alias; bucket + distribution ids are consumed by the GHA deploy workflow."
  value = {
    url             = "https://compute.${var.testnet_subdomain}.${var.apex_domain}"
    bucket          = aws_s3_bucket.compute_portal.id
    distribution_id = aws_cloudfront_distribution.compute_portal.id
  }
}
