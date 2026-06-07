# CloudFront distribution fronting `rpc.testnet.suwappu.globalsettlement.com`
# (and the `wss://ws.testnet.*` path for event subscriptions).
#
# Phase 1 fronting per
# /Users/mongolraider/.claude/plans/validated-prancing-curry.md.
#
# Design notes:
#
#   * AWS ALB's `target_type = "ip"` rejects public IPs that are not
#     in the ALB's own VPC, RFC1918, or RFC6598. Every validator has
#     a public EIP in its own regional VPC, so the ALB target groups
#     in `alb.tf` cannot attach them — that's the root of today's
#     `rpc.testnet.*` 503. CloudFront does not have that restriction:
#     it dials any publicly routable origin over the internet from
#     the edge.
#
#   * Cache TTLs are pinned to zero. JSON-RPC responses are
#     stateful (every `suwappu_getBalance` call returns the address's
#     current balance) and MUST NOT be cached at the edge. We still
#     send through CloudFront because the value here is TLS
#     termination + WAF + DDoS shielding + automatic origin
#     failover, not cache hit-rate.
#
#   * CloudFront origin groups support a primary + a single
#     secondary origin (AWS limit). Phase 1 ships us-east-1 primary
#     with eu-west-1 secondary — failover on 5xx from the primary.
#     The other 5 origins are declared so a future Phase 2 can
#     reference them (or so we can manually flip the origin-group
#     primary without re-declaring origins). Operators reaching
#     directly via the regional EIP table continue to work
#     unchanged.
#
#   * WebSocket support requires the dedicated `/ws` cache behavior
#     below (CF supports WebSocket on custom-origin distributions
#     since 2018 but the cache behavior must allow GET/HEAD/OPTIONS
#     with header pass-through).
#
#   * CloudFront resources are in us-east-1 by convention but are
#     globally scoped at the API surface. ACM cert MUST be in
#     us-east-1 for CF (it is — see `acm.tf`).

resource "aws_cloudfront_distribution" "rpc" {
  provider        = aws.us_east_1
  enabled         = true
  is_ipv6_enabled = true
  comment         = "suwappu-testnet JSON-RPC + WebSocket fronting (Phase 1)"
  aliases = [
    "rpc.${var.testnet_subdomain}.${var.apex_domain}",
    "ws.${var.testnet_subdomain}.${var.apex_domain}",
  ]
  # PriceClass_100 = North America + Europe edge locations only. Cheap
  # ($0.085/GB egress vs $0.110/GB+ for All). Bump to PriceClass_200
  # (adds APAC + South America edges) once the testnet has enough
  # international developer traffic to make the extra cost pay back.
  price_class = "PriceClass_100"
  web_acl_id  = aws_wafv2_web_acl.testnet_cf.arn

  # ----- Origins (one per validator) -----

  origin {
    domain_name = aws_route53_record.origin_us_east_1.fqdn
    origin_id   = "validator-us-east-1"
    custom_origin_config {
      http_port              = var.rpc_port
      https_port             = 443
      origin_protocol_policy = "http-only"
      origin_ssl_protocols   = ["TLSv1.2"]
    }
  }
  origin {
    domain_name = aws_route53_record.origin_us_west_2.fqdn
    origin_id   = "validator-us-west-2"
    custom_origin_config {
      http_port              = var.rpc_port
      https_port             = 443
      origin_protocol_policy = "http-only"
      origin_ssl_protocols   = ["TLSv1.2"]
    }
  }
  origin {
    domain_name = aws_route53_record.origin_eu_west_1.fqdn
    origin_id   = "validator-eu-west-1"
    custom_origin_config {
      http_port              = var.rpc_port
      https_port             = 443
      origin_protocol_policy = "http-only"
      origin_ssl_protocols   = ["TLSv1.2"]
    }
  }
  origin {
    domain_name = aws_route53_record.origin_eu_central_1.fqdn
    origin_id   = "validator-eu-central-1"
    custom_origin_config {
      http_port              = var.rpc_port
      https_port             = 443
      origin_protocol_policy = "http-only"
      origin_ssl_protocols   = ["TLSv1.2"]
    }
  }
  origin {
    domain_name = aws_route53_record.origin_ap_southeast_1.fqdn
    origin_id   = "validator-ap-southeast-1"
    custom_origin_config {
      http_port              = var.rpc_port
      https_port             = 443
      origin_protocol_policy = "http-only"
      origin_ssl_protocols   = ["TLSv1.2"]
    }
  }
  origin {
    domain_name = aws_route53_record.origin_ap_northeast_1.fqdn
    origin_id   = "validator-ap-northeast-1"
    custom_origin_config {
      http_port              = var.rpc_port
      https_port             = 443
      origin_protocol_policy = "http-only"
      origin_ssl_protocols   = ["TLSv1.2"]
    }
  }
  origin {
    domain_name = aws_route53_record.origin_sa_east_1.fqdn
    origin_id   = "validator-sa-east-1"
    custom_origin_config {
      http_port              = var.rpc_port
      https_port             = 443
      origin_protocol_policy = "http-only"
      origin_ssl_protocols   = ["TLSv1.2"]
    }
  }

  # ----- Origin group: failover on 5xx -----

  origin_group {
    origin_id = "validator-failover"

    failover_criteria {
      status_codes = [500, 502, 503, 504]
    }

    member { origin_id = "validator-us-east-1" }
    member { origin_id = "validator-eu-west-1" }
  }

  # ----- Default cache behavior (JSON-RPC over POST) -----

  default_cache_behavior {
    target_origin_id       = "validator-failover"
    viewer_protocol_policy = "redirect-to-https"
    allowed_methods = [
      "GET", "HEAD", "OPTIONS",
      "PUT", "POST", "PATCH", "DELETE",
    ]
    cached_methods = ["GET", "HEAD"]
    compress       = true

    # CachingDisabled (AWS-managed) — TTL = 0 on all methods. JSON-RPC
    # responses change on every block; caching them at the edge would
    # serve stale balances + cert state and silently break dApps.
    cache_policy_id = "4135ea2d-6df8-44a3-9df3-4b5a84be39ad"

    # AllViewer (AWS-managed) — forwards every header, cookie, and
    # query string to the origin. Lets the validators see the original
    # Content-Type / Accept / User-Agent and (for future bot mitigation)
    # the client's CF-Connecting-IP equivalent.
    origin_request_policy_id = "216adef6-5c7f-47e4-b989-5492eafa07d3"
  }

  # ----- WebSocket cache behavior for /ws -----
  #
  # Subscribe path is `ws://<host>/ws` (per
  # `crates/suwappu-node/src/rpc/server.rs`); the same TCP port (9092)
  # serves both HTTP JSON-RPC and the WebSocket upgrade. CloudFront
  # handles WebSocket transparently when the cache behavior allows
  # GET/HEAD/OPTIONS and forwards the Upgrade/Connection headers
  # (covered by the AllViewer origin-request policy above).
  ordered_cache_behavior {
    path_pattern           = "/ws*"
    target_origin_id       = "validator-failover"
    viewer_protocol_policy = "redirect-to-https"
    allowed_methods        = ["GET", "HEAD", "OPTIONS"]
    cached_methods         = ["GET", "HEAD"]
    compress               = false

    cache_policy_id          = "4135ea2d-6df8-44a3-9df3-4b5a84be39ad"
    origin_request_policy_id = "216adef6-5c7f-47e4-b989-5492eafa07d3"
  }

  restrictions {
    geo_restriction { restriction_type = "none" }
  }

  viewer_certificate {
    acm_certificate_arn      = aws_acm_certificate_validation.wildcard.certificate_arn
    ssl_support_method       = "sni-only"
    minimum_protocol_version = "TLSv1.2_2021"
  }

  tags = { Name = "suwappu-testnet-rpc-cf" }
}

output "cf_rpc_distribution_id" {
  description = "CloudFront distribution ID for the RPC + WS endpoint. Used by CloudWatch alarms scoped to AWS/CloudFront's DistributionId dimension."
  value       = aws_cloudfront_distribution.rpc.id
}

output "cf_rpc_domain_name" {
  description = "Distribution domain (d*.cloudfront.net). DNS aliases in dns.tf point at this."
  value       = aws_cloudfront_distribution.rpc.domain_name
}
