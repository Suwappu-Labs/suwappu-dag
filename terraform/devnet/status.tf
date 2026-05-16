# G8 — status page static-site deployment.
#
# Same shape as explorer.tf — S3 + CloudFront + Route53 + wildcard
# ACM. The only differences:
#   * Bucket name `gsx-dag-devnet-status` (separate from explorer).
#   * DNS alias `status.devnet.gsx.globalsettlement.com`.
#   * No SPA fallback — status page is a single index.html + app.js,
#     no client-side routing.

resource "aws_s3_bucket" "status" {
  provider      = aws.us_east_1
  bucket        = "gsx-dag-devnet-status"
  force_destroy = false

  tags = { Name = "gsx-devnet-status" }
}

resource "aws_s3_bucket_public_access_block" "status" {
  provider                = aws.us_east_1
  bucket                  = aws_s3_bucket.status.id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_cloudfront_origin_access_control" "status" {
  provider                          = aws.us_east_1
  name                              = "gsx-devnet-status-oac"
  origin_access_control_origin_type = "s3"
  signing_behavior                  = "always"
  signing_protocol                  = "sigv4"
}

resource "aws_cloudfront_distribution" "status" {
  provider            = aws.us_east_1
  enabled             = true
  is_ipv6_enabled     = true
  comment             = "gsx-devnet status page"
  default_root_object = "index.html"
  aliases             = ["status.${var.devnet_subdomain}.${var.apex_domain}"]
  price_class         = "PriceClass_100"

  origin {
    domain_name              = aws_s3_bucket.status.bucket_regional_domain_name
    origin_id                = "status-s3"
    origin_access_control_id = aws_cloudfront_origin_access_control.status.id
  }

  default_cache_behavior {
    target_origin_id       = "status-s3"
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

  tags = { Name = "gsx-devnet-status" }
}

resource "aws_s3_bucket_policy" "status" {
  provider = aws.us_east_1
  bucket   = aws_s3_bucket.status.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Sid       = "AllowCloudFrontServicePrincipalReadOnly"
      Effect    = "Allow"
      Principal = { Service = "cloudfront.amazonaws.com" }
      Action    = ["s3:GetObject"]
      Resource  = ["${aws_s3_bucket.status.arn}/*"]
      Condition = {
        StringEquals = {
          "AWS:SourceArn" = aws_cloudfront_distribution.status.arn
        }
      }
    }]
  })
}

resource "aws_route53_record" "status" {
  provider = aws.us_east_1
  zone_id  = aws_route53_zone.devnet.zone_id
  name     = "status.${var.devnet_subdomain}.${var.apex_domain}"
  type     = "A"

  alias {
    name                   = aws_cloudfront_distribution.status.domain_name
    zone_id                = aws_cloudfront_distribution.status.hosted_zone_id
    evaluate_target_health = false
  }
}

output "status_page" {
  description = "Status-page handle."
  value = {
    url             = "https://status.${var.devnet_subdomain}.${var.apex_domain}"
    bucket          = aws_s3_bucket.status.id
    distribution_id = aws_cloudfront_distribution.status.id
  }
}
