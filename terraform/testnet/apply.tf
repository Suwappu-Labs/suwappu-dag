# `apply.testnet.gsx.globalsettlement.com` — public-facing operator
# application page. Phase B per
# `~/.claude/plans/validated-prancing-curry.md`.
#
# Shape: CloudFront distribution → S3 static bucket. The bucket holds
# exactly one file (`index.html`) that iframe-embeds a Typeform (form
# URL is a TF var; foundation populates the form out-of-band). The
# Typeform itself embeds the Persona inquiry widget for KYC — see
# `kyc.tf` for the webhook handler that drains Persona's events into
# DynamoDB.
#
# Why CloudFront + S3 instead of just an off-AWS Typeform link in
# Discord:
#   - Foundation owns the apply URL → can rev the form without
#     telling every operator a new URL.
#   - Wildcard ACM cert covers it for free (already issued in us-east-1).
#   - WAF + CloudFront edge gets us DDoS + bot protection at the
#     iframe layer; Typeform handles its own anti-abuse downstream.
#   - One day we'll replace the Typeform with a fully coded form;
#     keeping the public URL stable from day one saves a migration.

variable "apply_form_typeform_url" {
  description = "Typeform embed URL for the operator-application form. Set to the actual form URL after creating it; defaults to a placeholder so the stack stays applyable even before the Typeform exists."
  type        = string
  default     = "https://example.typeform.com/to/PLACEHOLDER"
}

resource "aws_s3_bucket" "apply" {
  provider      = aws.us_east_1
  bucket        = "gsx-dag-testnet-apply"
  force_destroy = true # SPA-style content; safe to wipe on stack rebuild
  tags          = { Name = "gsx-dag-testnet-apply" }
}

resource "aws_s3_bucket_public_access_block" "apply" {
  provider                = aws.us_east_1
  bucket                  = aws_s3_bucket.apply.id
  block_public_acls       = true
  block_public_policy     = false # CF OAC needs a public bucket policy
  ignore_public_acls      = true
  restrict_public_buckets = false
}

resource "aws_cloudfront_origin_access_control" "apply" {
  provider                          = aws.us_east_1
  name                              = "gsx-testnet-apply-oac"
  origin_access_control_origin_type = "s3"
  signing_behavior                  = "always"
  signing_protocol                  = "sigv4"
}

resource "aws_cloudfront_distribution" "apply" {
  provider            = aws.us_east_1
  enabled             = true
  is_ipv6_enabled     = true
  comment             = "gsx-testnet operator-application landing page (Phase B)"
  default_root_object = "index.html"
  aliases             = ["apply.${var.testnet_subdomain}.${var.apex_domain}"]
  price_class         = "PriceClass_100"
  web_acl_id          = aws_wafv2_web_acl.testnet_cf.arn

  origin {
    domain_name              = aws_s3_bucket.apply.bucket_regional_domain_name
    origin_id                = "apply-s3"
    origin_access_control_id = aws_cloudfront_origin_access_control.apply.id
  }

  default_cache_behavior {
    target_origin_id       = "apply-s3"
    viewer_protocol_policy = "redirect-to-https"
    allowed_methods        = ["GET", "HEAD", "OPTIONS"]
    cached_methods         = ["GET", "HEAD"]
    compress               = true
    # AWS-managed CachingOptimized — index.html is static; CF can cache
    # for ~24h. Form-URL changes require a CloudFront invalidation
    # (cheap, instant) OR a TF re-apply (the s3 object resource emits
    # a new ETag → CF picks it up at next cache miss).
    cache_policy_id = "658327ea-f89d-4fab-a63d-7e88639e58f6"
  }

  custom_error_response {
    error_code         = 404
    response_code      = 200
    response_page_path = "/index.html"
  }

  restrictions {
    geo_restriction { restriction_type = "none" }
  }

  viewer_certificate {
    acm_certificate_arn      = aws_acm_certificate_validation.wildcard.certificate_arn
    ssl_support_method       = "sni-only"
    minimum_protocol_version = "TLSv1.2_2021"
  }

  tags = { Name = "gsx-testnet-apply-cf" }
}

resource "aws_s3_bucket_policy" "apply" {
  provider = aws.us_east_1
  bucket   = aws_s3_bucket.apply.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Sid       = "AllowCloudFrontServicePrincipalReadOnly"
      Effect    = "Allow"
      Principal = { Service = "cloudfront.amazonaws.com" }
      Action    = ["s3:GetObject"]
      Resource  = ["${aws_s3_bucket.apply.arn}/*"]
      Condition = {
        StringEquals = {
          "AWS:SourceArn" = aws_cloudfront_distribution.apply.arn
        }
      }
    }]
  })
}

# Render the apply page from the templatefile so the Typeform URL is
# baked in at TF-apply time. Changing the URL just requires a new
# `terraform apply` (TF re-uploads with a new ETag → CF picks it up).
resource "aws_s3_object" "apply_index" {
  provider     = aws.us_east_1
  bucket       = aws_s3_bucket.apply.id
  key          = "index.html"
  content      = templatefile("${path.module}/apply-index.html.tpl", { typeform_url = var.apply_form_typeform_url })
  content_type = "text/html; charset=utf-8"
  etag         = md5(templatefile("${path.module}/apply-index.html.tpl", { typeform_url = var.apply_form_typeform_url }))
}

resource "aws_route53_record" "apply" {
  provider = aws.us_east_1
  zone_id  = aws_route53_zone.testnet.zone_id
  name     = "apply.${var.testnet_subdomain}.${var.apex_domain}"
  type     = "A"

  alias {
    name                   = aws_cloudfront_distribution.apply.domain_name
    zone_id                = aws_cloudfront_distribution.apply.hosted_zone_id
    evaluate_target_health = false
  }
}

output "cf_apply_domain_name" {
  description = "CloudFront domain (d*.cloudfront.net) for the apply page. DNS alias in this file points at it."
  value       = aws_cloudfront_distribution.apply.domain_name
}
