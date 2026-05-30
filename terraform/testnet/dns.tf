# DNS records for the public testnet.
#
# Same shape as terraform/devnet/dns.tf — different subdomain.
# Apex zone delegation: publish the NS records output below into
# whichever account hosts globalsettlement.com. One-time step.

resource "aws_route53_zone" "testnet" {
  provider = aws.us_east_1
  name     = "${var.testnet_subdomain}.${var.apex_domain}"
  tags     = { Name = "gsx-testnet-zone" }

  lifecycle {
    prevent_destroy = true
  }
}

# rpc/ws now alias to the Global Accelerator (ga.tf), which fronts the
# 7 regional RPC ALBs (regional_alb.tf). GA — not CloudFront — because
# JSON-RPC is POST and CloudFront origin failover is GET/HEAD-only
# (#237); GA reroutes POST to a healthy region in < 1 min. The faucet
# record below still points at CloudFront (faucet is a separate fronting
# decision, out of scope here).
#
# The old `aws_cloudfront_distribution.rpc` in cf_rpc.tf is now
# unreferenced (dead). It is intentionally left in place for one
# release as a fast rollback path — flip these two aliases back to it
# if GA misbehaves — and removed in a follow-up once GA is verified
# live. evaluate_target_health is true: GA exposes endpoint health to
# Route53.

resource "aws_route53_record" "rpc" {
  provider = aws.us_east_1
  zone_id  = aws_route53_zone.testnet.zone_id
  name     = "rpc.${var.testnet_subdomain}.${var.apex_domain}"
  type     = "A"

  alias {
    name                   = aws_globalaccelerator_accelerator.rpc.dns_name
    zone_id                = aws_globalaccelerator_accelerator.rpc.hosted_zone_id
    evaluate_target_health = true
  }
}

resource "aws_route53_record" "ws" {
  provider = aws.us_east_1
  zone_id  = aws_route53_zone.testnet.zone_id
  name     = "ws.${var.testnet_subdomain}.${var.apex_domain}"
  type     = "A"

  alias {
    name                   = aws_globalaccelerator_accelerator.rpc.dns_name
    zone_id                = aws_globalaccelerator_accelerator.rpc.hosted_zone_id
    evaluate_target_health = true
  }
}

resource "aws_route53_record" "faucet" {
  provider = aws.us_east_1
  zone_id  = aws_route53_zone.testnet.zone_id
  name     = "faucet.${var.testnet_subdomain}.${var.apex_domain}"
  type     = "A"

  alias {
    name                   = aws_cloudfront_distribution.faucet.domain_name
    zone_id                = aws_cloudfront_distribution.faucet.hosted_zone_id
    evaluate_target_health = false
  }
}

# Validator-program leaderboard endpoint. The points-accumulator
# daemon (forthcoming) listens on port 8090 of the program EC2;
# this DNS record points at the EIP directly (no ALB — single
# foundation-operated host; the leaderboard endpoint is
# foundation-controlled, not a public dApp surface).
resource "aws_route53_record" "program" {
  provider = aws.us_east_1
  zone_id  = aws_route53_zone.testnet.zone_id
  name     = "program.${var.testnet_subdomain}.${var.apex_domain}"
  type     = "A"
  ttl      = 60
  records  = [aws_eip.program.public_ip]
}

# CloudFront origin records.
#
# CloudFront `custom_origin_config.domain_name` requires a publicly
# resolvable DNS name; raw EIP literals are rejected at apply time.
# We mint one Route53 A record per origin EIP and reference its `fqdn`
# from cf_rpc.tf / cf_faucet.tf. (Codex #228 P1 — `cf_rpc.tf:61`,
# `cf_faucet.tf:26`.)
#
# These names are internal-implementation (not user-facing); user
# traffic still hits `rpc.${testnet_subdomain}.${apex_domain}` etc.
# which are CloudFront aliases.

resource "aws_route53_record" "origin_faucet" {
  provider = aws.us_east_1
  zone_id  = aws_route53_zone.testnet.zone_id
  name     = "origin-faucet.${var.testnet_subdomain}.${var.apex_domain}"
  type     = "A"
  ttl      = 60
  records  = [aws_eip.faucet.public_ip]
}

resource "aws_route53_record" "origin_us_east_1" {
  provider = aws.us_east_1
  zone_id  = aws_route53_zone.testnet.zone_id
  name     = "origin-us-east-1.${var.testnet_subdomain}.${var.apex_domain}"
  type     = "A"
  ttl      = 60
  records  = [module.us_east_1.public_ip]
}

resource "aws_route53_record" "origin_us_west_2" {
  provider = aws.us_east_1
  zone_id  = aws_route53_zone.testnet.zone_id
  name     = "origin-us-west-2.${var.testnet_subdomain}.${var.apex_domain}"
  type     = "A"
  ttl      = 60
  records  = [module.us_west_2.public_ip]
}

resource "aws_route53_record" "origin_eu_west_1" {
  provider = aws.us_east_1
  zone_id  = aws_route53_zone.testnet.zone_id
  name     = "origin-eu-west-1.${var.testnet_subdomain}.${var.apex_domain}"
  type     = "A"
  ttl      = 60
  records  = [module.eu_west_1.public_ip]
}

resource "aws_route53_record" "origin_eu_central_1" {
  provider = aws.us_east_1
  zone_id  = aws_route53_zone.testnet.zone_id
  name     = "origin-eu-central-1.${var.testnet_subdomain}.${var.apex_domain}"
  type     = "A"
  ttl      = 60
  records  = [module.eu_central_1.public_ip]
}

resource "aws_route53_record" "origin_ap_southeast_1" {
  provider = aws.us_east_1
  zone_id  = aws_route53_zone.testnet.zone_id
  name     = "origin-ap-southeast-1.${var.testnet_subdomain}.${var.apex_domain}"
  type     = "A"
  ttl      = 60
  records  = [module.ap_southeast_1.public_ip]
}

resource "aws_route53_record" "origin_ap_northeast_1" {
  provider = aws.us_east_1
  zone_id  = aws_route53_zone.testnet.zone_id
  name     = "origin-ap-northeast-1.${var.testnet_subdomain}.${var.apex_domain}"
  type     = "A"
  ttl      = 60
  records  = [module.ap_northeast_1.public_ip]
}

resource "aws_route53_record" "origin_sa_east_1" {
  provider = aws.us_east_1
  zone_id  = aws_route53_zone.testnet.zone_id
  name     = "origin-sa-east-1.${var.testnet_subdomain}.${var.apex_domain}"
  type     = "A"
  ttl      = 60
  records  = [module.sa_east_1.public_ip]
}

# explorer.testnet.gsx + status.testnet.gsx land in follow-up PRs
# (forks of terraform/devnet/{explorer,status}.tf).

output "testnet_nameservers" {
  description = "Authoritative nameservers for the testnet subdomain. Publish these as NS records under the apex zone for the testnet_subdomain (variable) of the apex_domain (variable)."
  value       = aws_route53_zone.testnet.name_servers
}
