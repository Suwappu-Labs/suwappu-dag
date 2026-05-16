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

resource "aws_route53_record" "rpc" {
  provider = aws.us_east_1
  zone_id  = aws_route53_zone.testnet.zone_id
  name     = "rpc.${var.testnet_subdomain}.${var.apex_domain}"
  type     = "A"

  alias {
    name                   = aws_lb.rpc.dns_name
    zone_id                = aws_lb.rpc.zone_id
    evaluate_target_health = true
  }
}

resource "aws_route53_record" "ws" {
  provider = aws.us_east_1
  zone_id  = aws_route53_zone.testnet.zone_id
  name     = "ws.${var.testnet_subdomain}.${var.apex_domain}"
  type     = "A"

  alias {
    name                   = aws_lb.rpc.dns_name
    zone_id                = aws_lb.rpc.zone_id
    evaluate_target_health = true
  }
}

resource "aws_route53_record" "faucet" {
  provider = aws.us_east_1
  zone_id  = aws_route53_zone.testnet.zone_id
  name     = "faucet.${var.testnet_subdomain}.${var.apex_domain}"
  type     = "A"

  alias {
    name                   = aws_lb.faucet.dns_name
    zone_id                = aws_lb.faucet.zone_id
    evaluate_target_health = true
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

# explorer.testnet.gsx + status.testnet.gsx land in follow-up PRs
# (forks of terraform/devnet/{explorer,status}.tf).

output "testnet_nameservers" {
  description = "Authoritative nameservers for the testnet subdomain. Publish these as NS records under the apex zone for ${var.testnet_subdomain}.${var.apex_domain}."
  value       = aws_route53_zone.testnet.name_servers
}
