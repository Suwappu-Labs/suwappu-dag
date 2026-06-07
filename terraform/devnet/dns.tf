# G2 — DNS records for the public devnet.
#
# Subdomain layout (all under devnet.suwappu.globalsettlement.com):
#
#   rpc.devnet.suwappu        → ALB fronting the 4 validators (JSON-RPC + WS)
#   faucet.devnet.suwappu     → ALB fronting the faucet EC2
#   explorer.devnet.suwappu   → CloudFront distribution serving the SPA   (G7)
#   status.devnet.suwappu     → CloudFront distribution serving the status page (G8)
#
# All DNS records use ALIAS to the underlying AWS endpoints
# (cheaper than A records + faster + no TTL gotchas).
#
# OPEN ITEM: this stack assumes the apex zone for
# `globalsettlement.com` lives in account 492042618949 (the same
# gsn account). If the apex zone lives in a different account,
# the operator must publish the NS records of this devnet zone
# into that apex zone manually after the first `terraform apply`.

variable "apex_domain" {
  description = "The apex domain that hosts the devnet subdomain. The team must already own this and the Route53 zone for it must be reachable from the gsn account (either same-account or via a delegated subdomain NS record)."
  type        = string
  default     = "globalsettlement.com"
}

variable "devnet_subdomain" {
  description = "Subdomain under which all devnet records live."
  type        = string
  default     = "devnet.suwappu"
}

# Hosted zone for the devnet subdomain. The team has two options:
#
# 1. Create this zone in the gsn account (this stack does that), then
#    publish its NS records under the apex zone (one-time manual
#    step). All subsequent record changes happen in this zone.
#
# 2. Skip this resource and use the apex zone's existing hosted zone
#    id directly. Requires the apex zone to be in the gsn account.
#
# Default is option 1 — explicit delegation keeps blast radius
# bounded. To switch to option 2, comment out aws_route53_zone.devnet
# and use a `data "aws_route53_zone" "apex"` block instead.
resource "aws_route53_zone" "devnet" {
  provider = aws.us_east_1
  name     = "${var.devnet_subdomain}.${var.apex_domain}"
  tags = {
    Name = "suwappu-devnet-zone"
  }

  # Devnet DNS lives forever; force_destroy is a footgun.
  lifecycle {
    prevent_destroy = true
  }
}

# ALIAS records pointing at the CloudFront distributions + (explorer/
# status) CloudFront distros. rpc/ws/faucet front via CloudFront
# (cf_rpc.tf / cf_faucet.tf) — NOT the ALBs in alb.tf, whose target
# groups are empty (ALB target_type=ip can't attach the validators'
# cross-VPC public EIPs → 503). The alb.tf RPC/faucet skeleton is left
# in place, unreferenced, as a rollback anchor.
#
# evaluate_target_health is false for CloudFront aliases — the distro
# does its own per-origin health checking via the origin group's 5xx
# failover criteria (cf_rpc.tf).

resource "aws_route53_record" "rpc" {
  provider = aws.us_east_1
  zone_id  = aws_route53_zone.devnet.zone_id
  name     = "rpc.${var.devnet_subdomain}.${var.apex_domain}"
  type     = "A"

  alias {
    name                   = aws_cloudfront_distribution.rpc.domain_name
    zone_id                = aws_cloudfront_distribution.rpc.hosted_zone_id
    evaluate_target_health = false
  }
}

# WebSocket subscription path rides the same CloudFront distro (the
# `/ws*` behavior in cf_rpc.tf). Separate DNS name so SDK consumers can
# configure RPC + WS endpoints distinctly (some clients won't accept
# an https:// → wss:// transparent upgrade).
resource "aws_route53_record" "ws" {
  provider = aws.us_east_1
  zone_id  = aws_route53_zone.devnet.zone_id
  name     = "ws.${var.devnet_subdomain}.${var.apex_domain}"
  type     = "A"

  alias {
    name                   = aws_cloudfront_distribution.rpc.domain_name
    zone_id                = aws_cloudfront_distribution.rpc.hosted_zone_id
    evaluate_target_health = false
  }
}

resource "aws_route53_record" "faucet" {
  provider = aws.us_east_1
  zone_id  = aws_route53_zone.devnet.zone_id
  name     = "faucet.${var.devnet_subdomain}.${var.apex_domain}"
  type     = "A"

  alias {
    name                   = aws_cloudfront_distribution.faucet.domain_name
    zone_id                = aws_cloudfront_distribution.faucet.hosted_zone_id
    evaluate_target_health = false
  }
}

# Note: this stack previously published `origin-<region>.devnet.*` A
# records here so CloudFront could resolve them as origins. That made
# CloudFront's first-apply distribution-create lookup fail with
# `InvalidOrigin` until the devnet subzone's NS records were
# delegated under the apex — a deadlock on bootstrap. cf_rpc.tf /
# cf_faucet.tf now synthesise the AWS-provided EC2 public hostname
# (`ec2-<dashed-eip>.<region>.compute.amazonaws.com`) from each
# validator / faucet EIP directly, so no devnet-subzone A records
# are needed for the origin lookups. The user-facing rpc/ws/faucet
# aliases above still need apex delegation for end-users to reach
# the devnet, but that's a runtime concern, not an
# infrastructure-create blocker.

# explorer.devnet.suwappu + status.devnet.suwappu records live with their
# CloudFront distributions (explorer.tf / status.tf).

# Output the NS records so an operator can paste them into the apex
# zone for delegation.
output "devnet_nameservers" {
  description = "Authoritative nameservers for the devnet subdomain. Publish these as NS records under the apex zone for the devnet_subdomain (variable) of the apex_domain (variable)."
  value       = aws_route53_zone.devnet.name_servers
}
