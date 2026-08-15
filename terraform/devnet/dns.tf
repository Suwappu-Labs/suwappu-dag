# G2 — DNS records for the public devnet.
#
# Subdomain layout (all under devnet.suwappu.bot):
#
#   rpc.devnet.suwappu.bot        → ALB fronting the 4 validators (JSON-RPC + WS)
#   faucet.devnet.suwappu.bot     → ALB fronting the faucet EC2
#   explorer.devnet.suwappu.bot   → CloudFront distribution serving the SPA   (G7)
#   status.devnet.suwappu.bot     → CloudFront distribution serving the status page (G8)
#
# All DNS records use ALIAS to the underlying AWS endpoints
# (cheaper than A records + faster + no TTL gotchas).
#
# OPEN ITEM: this stack assumes the apex zone for
# `suwappu.bot` lives in account 492042618949 (the same
# gsn account). If the apex zone lives in a different account,
# the operator must publish the NS records of this devnet zone
# into that apex zone manually after the first `terraform apply`.

variable "apex_domain" {
  description = "The apex domain that hosts the devnet subdomain. The team must already own this and the Route53 zone for it must be reachable from the gsn account (either same-account or via a delegated subdomain NS record)."
  type        = string
  default     = "suwappu.bot"
}

variable "devnet_subdomain" {
  description = "Subdomain under which all devnet records live."
  type        = string
  default     = "devnet"
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

# ALIAS records pointing at the load balancers + CloudFront distros.
# The ALB + CloudFront resources are defined in alb.tf + (future)
# explorer.tf / status.tf.

resource "aws_route53_record" "rpc" {
  provider = aws.us_east_1
  zone_id  = aws_route53_zone.devnet.zone_id
  name     = "rpc.${var.devnet_subdomain}.${var.apex_domain}"
  type     = "A"

  alias {
    name                   = aws_lb.rpc.dns_name
    zone_id                = aws_lb.rpc.zone_id
    evaluate_target_health = true
  }
}

# WebSocket subscription path also lands on the same ALB — the ALB
# upgrades the GET /ws to a WebSocket connection. Separate DNS name
# so SDK consumers can configure RPC + WS endpoints distinctly
# (some clients won't accept https:// → wss:// transparent upgrade).
resource "aws_route53_record" "ws" {
  provider = aws.us_east_1
  zone_id  = aws_route53_zone.devnet.zone_id
  name     = "ws.${var.devnet_subdomain}.${var.apex_domain}"
  type     = "A"

  alias {
    name                   = aws_lb.rpc.dns_name
    zone_id                = aws_lb.rpc.zone_id
    evaluate_target_health = true
  }
}

resource "aws_route53_record" "faucet" {
  provider = aws.us_east_1
  zone_id  = aws_route53_zone.devnet.zone_id
  name     = "faucet.${var.devnet_subdomain}.${var.apex_domain}"
  type     = "A"

  alias {
    name                   = aws_lb.faucet.dns_name
    zone_id                = aws_lb.faucet.zone_id
    evaluate_target_health = true
  }
}

# explorer.devnet.suwappu.bot + status.devnet.suwappu.bot record stubs live
# with their respective CloudFront distributions (G7 + G8 add them).

# Output the NS records so an operator can paste them into the apex
# zone for delegation.
output "devnet_nameservers" {
  description = "Authoritative nameservers for the devnet subdomain. Publish these as NS records under the apex zone for the devnet_subdomain (variable) of the apex_domain (variable)."
  value       = aws_route53_zone.devnet.name_servers
}
