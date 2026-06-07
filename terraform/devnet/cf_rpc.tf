# CloudFront distribution fronting `rpc.devnet.suwappu.globalsettlement.com`
# (and the `wss://ws.devnet.*` path for event subscriptions).
#
# Ported from terraform/testnet/cf_rpc.tf, adapted to the 4-region
# devnet. Why CloudFront and not the ALB: ALB `target_type = "ip"`
# rejects public-IP targets outside the ALB's own VPC, so the
# `alb.tf` RPC target group can't attach the validators' regional
# EIPs — that's the root of today's `rpc.devnet.*` 503. CloudFront
# dials any publicly routable origin from the edge, terminates TLS
# with the wildcard ACM cert, and fails over between origins on 5xx.
#
# Cache TTLs pinned to zero: JSON-RPC responses are stateful and must
# not be cached. The value here is TLS termination + origin failover,
# not cache hit-rate. (The testnet front-ended RPC the same way before
# moving to Global Accelerator + per-region ALB; for an ephemeral
# devnet, CloudFront is the cheaper, good-enough choice — the
# POST-failover gap, #237, is acceptable on a throwaway network.)
#
# # Origin DNS: AWS-provided EC2 public hostnames, not the devnet subzone
#
# CloudFront's `custom_origin_config.domain_name` requires a publicly
# resolvable DNS name at distribution-create time, and a raw IP is
# rejected. We deliberately do NOT use names in the devnet subzone
# (e.g. `origin-us-east-1.devnet.suwappu.globalsettlement.com`) — that
# subzone is created in this same `terraform apply`, and its NS
# records are not yet published into the apex zone on first bring-up.
# CloudFront's distribution-creation validator therefore can't resolve
# those names on the first apply and rejects the distro with
# `InvalidOrigin`, hard-blocking bootstrap.
#
# Instead we synthesise the AWS-provided EC2 public hostname for each
# validator EIP. The `amazonaws.com` zone is globally delegated, so
# these names resolve from CloudFront's edge regardless of whether
# the apex zone has the devnet NS records yet. The synthesised name
# tracks the EIP, not the EC2 instance — instance replacement keeps
# the EIP and the hostname stays stable. us-east-1 uses the legacy
# `compute-1.amazonaws.com` suffix; every other region uses
# `<region>.compute.amazonaws.com` (verified via `dig SOA`; the
# unified `us-east-1.compute.amazonaws.com` does not exist). Closes
# the Codex P1 + ambarish review on PR #265's first push: "subzone
# not publicly delegated until after apply" → "use already-public
# EC2/EIP DNS as origins".
#
# **EIP stability assumption.** The synthesised hostname is tied to
# the EIP; if the EIP is released or re-allocated to a different
# instance, the hostname stops resolving and CloudFront origin
# health checks fail. Operators rotating EIPs (e.g. moving a
# validator to a new account or region) must `terraform apply`
# after the EIP rotates so the local computes from the new value;
# CloudFront re-validates the origin domain on update. This is
# acceptable for a long-lived devnet whose EIPs are persistent
# resources.
#
# # Per-region failover, not multi-region load balance
#
# This is **failover**, not load balancing: CloudFront's origin group
# supports exactly one primary + one secondary. us-east-1 is primary,
# eu-west-1 is the warm secondary; CloudFront only retries against
# eu-west-1 when us-east-1 returns one of the configured 5xx status
# codes (and only on idempotent methods — see below). The
# ap-southeast-1 / sa-east-1 origins are declared so the primary or
# secondary can be re-pointed without re-declaring origins, but they
# are **not active** in the failover group as configured. A true
# 4-region load-balanced surface would need Route53 latency / weighted
# records + per-region ALBs (the testnet path, via Global
# Accelerator); the devnet keeps the simpler CloudFront layout
# because devnet uptime is best-effort.
#
# # Client IP visibility through CloudFront
#
# The CloudFront → origin TCP connection appears, from the origin's
# perspective, to come from a CloudFront edge IP — NOT the viewer's.
# suwappu-rpc's per-IP token bucket therefore can't read the viewer's
# real IP from the socket peer address; if it does, the bucket key
# collapses to a handful of edge IPs and a single viewer can drain
# the limit for everyone.
#
# **Trust posture, current.** The validator security group in
# `modules/validator/main.tf` opens the RPC port to `0.0.0.0/0`
# (see comment in that file). That means any internet client can
# also reach `<validator-eip>:<rpc_port>` directly and set any
# `X-Forwarded-For` it likes, so the origin **cannot trust XFF** as
# the client identity — every request must be keyed by the TCP
# source IP, even though that collapses CloudFront-routed requests
# to edge IPs. The per-IP bucket effectively only rate-limits direct
# attackers in this configuration. (Acceptable for a devnet; if the
# bucket needs to bind to real client IPs through CloudFront, the
# follow-on is to (a) restrict the validator security group RPC
# port to the `com.amazonaws.global.cloudfront.origin-facing` prefix
# list + operator CIDRs, and (b) configure suwappu-rpc to read XFF /
# `CloudFront-Viewer-Address`. Both are out of scope for this PR
# — the immediate fix here is the bootstrap-blocking origin DNS
# above.)
#
# The AllViewer origin request policy below already forwards XFF +
# the CloudFront-Viewer-* headers, so the application-layer change
# is the only piece missing once the SG side lands.

locals {
  # AWS-provided EC2 public DNS, computed from each validator's EIP.
  # Globally resolvable; sidesteps the devnet-subzone-not-yet-delegated
  # bootstrap problem. See the header comment for the full rationale.
  validator_origin_dns = {
    "us-east-1"      = "ec2-${replace(module.us_east_1.public_ip, ".", "-")}.compute-1.amazonaws.com"
    "eu-west-1"      = "ec2-${replace(module.eu_west_1.public_ip, ".", "-")}.eu-west-1.compute.amazonaws.com"
    "ap-southeast-1" = "ec2-${replace(module.ap_southeast_1.public_ip, ".", "-")}.ap-southeast-1.compute.amazonaws.com"
    "sa-east-1"      = "ec2-${replace(module.sa_east_1.public_ip, ".", "-")}.sa-east-1.compute.amazonaws.com"
  }
}

resource "aws_cloudfront_distribution" "rpc" {
  provider        = aws.us_east_1
  enabled         = true
  is_ipv6_enabled = true
  comment         = "suwappu-devnet JSON-RPC + WebSocket fronting"
  aliases = [
    "rpc.${var.devnet_subdomain}.${var.apex_domain}",
    "ws.${var.devnet_subdomain}.${var.apex_domain}",
  ]
  price_class = "PriceClass_100" # NA + EU edges; cheap. Bump to _200 for APAC/SA.

  origin {
    domain_name = local.validator_origin_dns["us-east-1"]
    origin_id   = "validator-us-east-1"
    custom_origin_config {
      http_port              = var.rpc_port
      https_port             = 443
      origin_protocol_policy = "http-only"
      origin_ssl_protocols   = ["TLSv1.2"]
    }
  }
  origin {
    domain_name = local.validator_origin_dns["eu-west-1"]
    origin_id   = "validator-eu-west-1"
    custom_origin_config {
      http_port              = var.rpc_port
      https_port             = 443
      origin_protocol_policy = "http-only"
      origin_ssl_protocols   = ["TLSv1.2"]
    }
  }
  # ap-southeast-1 + sa-east-1 declared but not active in the failover
  # group below — see the header comment ("Per-region failover, not
  # multi-region load balance"). Kept declared so the failover group
  # can be re-pointed without re-declaring origins.
  origin {
    domain_name = local.validator_origin_dns["ap-southeast-1"]
    origin_id   = "validator-ap-southeast-1"
    custom_origin_config {
      http_port              = var.rpc_port
      https_port             = 443
      origin_protocol_policy = "http-only"
      origin_ssl_protocols   = ["TLSv1.2"]
    }
  }
  origin {
    domain_name = local.validator_origin_dns["sa-east-1"]
    origin_id   = "validator-sa-east-1"
    custom_origin_config {
      http_port              = var.rpc_port
      https_port             = 443
      origin_protocol_policy = "http-only"
      origin_ssl_protocols   = ["TLSv1.2"]
    }
  }

  # Origin group: us-east-1 primary, eu-west-1 secondary. CloudFront
  # only retries against the secondary on the configured 5xx status
  # codes AND only on idempotent HTTP methods (GET, HEAD, OPTIONS).
  # POST/PUT/PATCH/DELETE — i.e. every JSON-RPC call — does NOT fail
  # over. Tracked as #237 (acceptable on a devnet; the testnet uses
  # Global Accelerator to get full-method failover).
  origin_group {
    origin_id = "validator-failover"
    failover_criteria {
      status_codes = [500, 502, 503, 504]
    }
    member { origin_id = "validator-us-east-1" }
    member { origin_id = "validator-eu-west-1" }
  }

  # JSON-RPC over POST. CachingDisabled + AllViewer (AWS-managed
  # policies) — TTL 0, forward all headers/methods to the origin so
  # X-Forwarded-For / CloudFront-Viewer-Address reach suwappu-rpc for
  # the per-IP bucket (see the header comment).
  default_cache_behavior {
    target_origin_id       = "validator-failover"
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

  # WebSocket subscribe path `/ws` (same TCP port 9092). CF handles the
  # upgrade when the behavior allows GET/HEAD/OPTIONS and forwards the
  # Upgrade/Connection headers (covered by AllViewer).
  ordered_cache_behavior {
    path_pattern             = "/ws*"
    target_origin_id         = "validator-failover"
    viewer_protocol_policy   = "redirect-to-https"
    allowed_methods          = ["GET", "HEAD", "OPTIONS"]
    cached_methods           = ["GET", "HEAD"]
    compress                 = false
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

  tags = { Name = "suwappu-devnet-rpc-cf" }
}

output "cf_rpc_distribution_id" {
  description = "CloudFront distribution ID for the devnet RPC + WS endpoint."
  value       = aws_cloudfront_distribution.rpc.id
}

output "cf_rpc_domain_name" {
  description = "Distribution domain (d*.cloudfront.net). DNS aliases in dns.tf point at this."
  value       = aws_cloudfront_distribution.rpc.domain_name
}
