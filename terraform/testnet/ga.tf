# AWS Global Accelerator — single anycast front for the 7 regional RPC
# ALBs (regional_alb.tf).
#
# Why GA and not CloudFront for RPC: JSON-RPC is POST, and CloudFront
# origin failover only covers GET/HEAD — a regional outage would break
# writes with no automatic failover (issue #237). GA is L4: it health-
# checks every endpoint (via the ALB's own target health) and reroutes
# ALL traffic, POST included, to a healthy region in < 1 min, over the
# AWS backbone. TLS terminates at the regional ALB (GA is passthrough),
# so clients see that region's wildcard cert for rpc.testnet.*.
#
# All GA resources use the us-east-1 provider: the Global Accelerator
# API is only available in us-east-1, even though endpoints span regions.

resource "aws_globalaccelerator_accelerator" "rpc" {
  provider        = aws.us_east_1
  name            = "suwappu-testnet-rpc"
  ip_address_type = "IPV4"
  enabled         = true

  tags = { Name = "suwappu-testnet-rpc-ga" }
}

# TCP passthrough on 443 (RPC + WS over TLS) and 80 (the ALB issues the
# 80→443 redirect). client_affinity NONE — every region serves the same
# committed chain state, so no per-client pinning is needed.
resource "aws_globalaccelerator_listener" "rpc" {
  provider        = aws.us_east_1
  accelerator_arn = aws_globalaccelerator_accelerator.rpc.id
  client_affinity = "NONE"
  protocol        = "TCP"

  port_range {
    from_port = 443
    to_port   = 443
  }
  port_range {
    from_port = 80
    to_port   = 80
  }
}

# One endpoint group per region → that region's ALB. Equal weights:
# GA's default routing sends each client to the lowest-latency healthy
# region, and fails over to the next-closest healthy one automatically.
# For ALB endpoints GA derives health from the ALB's target health, so
# no separate health-check block is needed here.

resource "aws_globalaccelerator_endpoint_group" "us_east_1" {
  provider              = aws.us_east_1
  listener_arn          = aws_globalaccelerator_listener.rpc.id
  endpoint_group_region = "us-east-1"
  endpoint_configuration {
    endpoint_id                    = module.rpc_alb_us_east_1.alb_arn
    weight                         = 128
    client_ip_preservation_enabled = true
  }
}

resource "aws_globalaccelerator_endpoint_group" "us_west_2" {
  provider              = aws.us_east_1
  listener_arn          = aws_globalaccelerator_listener.rpc.id
  endpoint_group_region = "us-west-2"
  endpoint_configuration {
    endpoint_id                    = module.rpc_alb_us_west_2.alb_arn
    weight                         = 128
    client_ip_preservation_enabled = true
  }
}

resource "aws_globalaccelerator_endpoint_group" "eu_west_1" {
  provider              = aws.us_east_1
  listener_arn          = aws_globalaccelerator_listener.rpc.id
  endpoint_group_region = "eu-west-1"
  endpoint_configuration {
    endpoint_id                    = module.rpc_alb_eu_west_1.alb_arn
    weight                         = 128
    client_ip_preservation_enabled = true
  }
}

resource "aws_globalaccelerator_endpoint_group" "eu_central_1" {
  provider              = aws.us_east_1
  listener_arn          = aws_globalaccelerator_listener.rpc.id
  endpoint_group_region = "eu-central-1"
  endpoint_configuration {
    endpoint_id                    = module.rpc_alb_eu_central_1.alb_arn
    weight                         = 128
    client_ip_preservation_enabled = true
  }
}

resource "aws_globalaccelerator_endpoint_group" "ap_southeast_1" {
  provider              = aws.us_east_1
  listener_arn          = aws_globalaccelerator_listener.rpc.id
  endpoint_group_region = "ap-southeast-1"
  endpoint_configuration {
    endpoint_id                    = module.rpc_alb_ap_southeast_1.alb_arn
    weight                         = 128
    client_ip_preservation_enabled = true
  }
}

resource "aws_globalaccelerator_endpoint_group" "ap_northeast_1" {
  provider              = aws.us_east_1
  listener_arn          = aws_globalaccelerator_listener.rpc.id
  endpoint_group_region = "ap-northeast-1"
  endpoint_configuration {
    endpoint_id                    = module.rpc_alb_ap_northeast_1.alb_arn
    weight                         = 128
    client_ip_preservation_enabled = true
  }
}

resource "aws_globalaccelerator_endpoint_group" "sa_east_1" {
  provider              = aws.us_east_1
  listener_arn          = aws_globalaccelerator_listener.rpc.id
  endpoint_group_region = "sa-east-1"
  endpoint_configuration {
    endpoint_id                    = module.rpc_alb_sa_east_1.alb_arn
    weight                         = 128
    client_ip_preservation_enabled = true
  }
}

output "rpc_ga_dns_name" {
  description = "Global Accelerator DNS name fronting the RPC ALBs. rpc/ws.testnet.* alias to this (dns.tf)."
  value       = aws_globalaccelerator_accelerator.rpc.dns_name
}

output "rpc_ga_static_ips" {
  description = "The accelerator's two static anycast IPs."
  value       = aws_globalaccelerator_accelerator.rpc.ip_sets[0].ip_addresses
}
