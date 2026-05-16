# Testnet block explorer — static-site deployment.
#
# Fork of terraform/devnet/explorer.tf. Differences:
#   * Bucket name `gsx-dag-testnet-explorer`.
#   * Alias `explorer.testnet.gsx.globalsettlement.com`.
#   * Reuses this stack's wildcard ACM from acm.tf.

resource "aws_s3_bucket" "explorer" {
  provider      = aws.us_east_1
  bucket        = "gsx-dag-testnet-explorer"
  force_destroy = false

  tags = { Name = "gsx-testnet-explorer" }
}

resource "aws_s3_bucket_public_access_block" "explorer" {
  provider                = aws.us_east_1
  bucket                  = aws_s3_bucket.explorer.id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_cloudfront_origin_access_control" "explorer" {
  provider                          = aws.us_east_1
  name                              = "gsx-testnet-explorer-oac"
  origin_access_control_origin_type = "s3"
  signing_behavior                  = "always"
  signing_protocol                  = "sigv4"
}

resource "aws_cloudfront_distribution" "explorer" {
  provider            = aws.us_east_1
  enabled             = true
  is_ipv6_enabled     = true
  comment             = "gsx-testnet explorer SPA"
  default_root_object = "index.html"
  aliases             = ["explorer.${var.testnet_subdomain}.${var.apex_domain}"]
  price_class         = "PriceClass_100"

  origin {
    domain_name              = aws_s3_bucket.explorer.bucket_regional_domain_name
    origin_id                = "explorer-s3"
    origin_access_control_id = aws_cloudfront_origin_access_control.explorer.id
  }

  default_cache_behavior {
    target_origin_id       = "explorer-s3"
    viewer_protocol_policy = "redirect-to-https"
    allowed_methods        = ["GET", "HEAD"]
    cached_methods         = ["GET", "HEAD"]
    compress               = true
    cache_policy_id        = "658327ea-f89d-4fab-a63d-7e88639e58f6" # AWS-managed CachingOptimized
  }

  custom_error_response {
    error_code         = 404
    response_code      = 200
    response_page_path = "/index.html"
  }
  custom_error_response {
    error_code         = 403
    response_code      = 200
    response_page_path = "/index.html"
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

  tags = { Name = "gsx-testnet-explorer" }
}

resource "aws_s3_bucket_policy" "explorer" {
  provider = aws.us_east_1
  bucket   = aws_s3_bucket.explorer.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Sid       = "AllowCloudFrontServicePrincipalReadOnly"
      Effect    = "Allow"
      Principal = { Service = "cloudfront.amazonaws.com" }
      Action    = ["s3:GetObject"]
      Resource  = ["${aws_s3_bucket.explorer.arn}/*"]
      Condition = {
        StringEquals = {
          "AWS:SourceArn" = aws_cloudfront_distribution.explorer.arn
        }
      }
    }]
  })
}

resource "aws_route53_record" "explorer" {
  provider = aws.us_east_1
  zone_id  = aws_route53_zone.testnet.zone_id
  name     = "explorer.${var.testnet_subdomain}.${var.apex_domain}"
  type     = "A"

  alias {
    name                   = aws_cloudfront_distribution.explorer.domain_name
    zone_id                = aws_cloudfront_distribution.explorer.hosted_zone_id
    evaluate_target_health = false
  }
}

output "explorer" {
  description = "Block-explorer handle. URL is the public alias; bucket + distribution ids are consumed by the GHA deploy workflow."
  value = {
    url             = "https://explorer.${var.testnet_subdomain}.${var.apex_domain}"
    bucket          = aws_s3_bucket.explorer.id
    distribution_id = aws_cloudfront_distribution.explorer.id
  }
}
