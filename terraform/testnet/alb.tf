# ALB layer — fork of terraform/devnet/alb.tf scaled to 7 backend
# validators. Same target-group + listener pattern; same
# cross-region IP target trick.
#
# SUPERSEDED (RPC): the single-region RPC ALB below (empty target group
# → 503) is replaced by the per-region ALBs in regional_alb.tf fronted
# by Global Accelerator in ga.tf. It is kept here, unreferenced, only as
# a one-release rollback anchor (flip the rpc/ws aliases in dns.tf back
# to cf_rpc.tf's CloudFront distribution if GA misbehaves) and is slated
# for deletion in the follow-up cleanup once GA is verified live. The
# faucet ALB + shared ALB networking below are unchanged (faucet
# fronting is a separate, still-parked decision).

# ------------- RPC ALB (superseded — see note above) -------------

resource "aws_lb" "rpc" {
  provider           = aws.us_east_1
  name               = "suwappu-testnet-rpc"
  internal           = false
  load_balancer_type = "application"

  subnets         = aws_subnet.alb_public.*.id
  security_groups = [aws_security_group.alb_rpc.id]

  enable_http2 = true

  drop_invalid_header_fields = true

  tags = { Name = "suwappu-testnet-rpc-alb" }
}

resource "aws_lb_target_group" "rpc" {
  provider    = aws.us_east_1
  name        = "suwappu-testnet-rpc-tg"
  port        = var.rpc_port
  protocol    = "HTTP"
  target_type = "ip"
  vpc_id      = aws_vpc.alb.id

  health_check {
    enabled             = true
    protocol            = "HTTP"
    port                = "traffic-port"
    path                = "/"
    matcher             = "200-499"
    healthy_threshold   = 2
    unhealthy_threshold = 3
    interval            = 30
    timeout             = 10
  }

  stickiness {
    type    = "lb_cookie"
    enabled = false
  }

  tags = { Name = "suwappu-testnet-rpc-tg" }
}

# NOTE: ALB target_type = "ip" rejects public IPs that are not in the
# ALB's own VPC subnet, RFC1918, or RFC6598 — AWS does not allow
# cross-region public-IP targets despite what the devnet comment
# suggests. To front the 7-region seed cluster behind a single
# `rpc.testnet.suwappu.globalsettlement.com` endpoint we need either
#   (a) per-region NLB + Global Accelerator, or
#   (b) VPC peering from this ALB VPC to each validator VPC + target
#       by private IP.
# Both are scope-expansion follow-ups. For now the ALB + listener +
# wildcard cert are kept (cheap; gives us the public DNS name) but
# the target group has no attachments, so the ALB returns 503. Until
# the proper fronting lands, external operators reach validators
# directly by EIP (printed in `terraform output validators`).

# RPC ALB listeners (HTTPS + HTTP→HTTPS redirect) are NOT declared in
# Phase 1 — the public TLS surface lives on CloudFront (`cf_rpc.tf`).
# Re-add these listeners when Phase 2 (per-region NLB + Global
# Accelerator) lands and we have private-IP targets that ALB will
# accept. See
# /Users/mongolraider/.claude/plans/validated-prancing-curry.md.
#
# Keeping the ALB + target group + VPC + SGs in TF as a no-op skeleton
# so Phase 2 doesn't need to re-issue the cert or re-allocate subnets.
# ALBs without listeners do not incur listener-hour charges; the
# remaining cost is the LCU minimum (~$16/mo per ALB) which is the
# price of keeping the cert + WAF + DNS skeleton ready to swap in.

# ------------- Faucet ALB -------------

resource "aws_lb" "faucet" {
  provider           = aws.us_east_1
  name               = "suwappu-testnet-faucet"
  internal           = false
  load_balancer_type = "application"
  subnets            = aws_subnet.alb_public.*.id
  security_groups    = [aws_security_group.alb_faucet.id]

  drop_invalid_header_fields = true

  tags = { Name = "suwappu-testnet-faucet-alb" }
}

resource "aws_lb_target_group" "faucet" {
  provider    = aws.us_east_1
  name        = "suwappu-testnet-faucet-tg"
  port        = 8080
  protocol    = "HTTP"
  target_type = "ip"
  vpc_id      = aws_vpc.alb.id

  health_check {
    enabled             = true
    protocol            = "HTTP"
    port                = "traffic-port"
    path                = "/health"
    matcher             = "200"
    healthy_threshold   = 2
    unhealthy_threshold = 3
    interval            = 30
    timeout             = 10
  }

  tags = { Name = "suwappu-testnet-faucet-tg" }
}

# NOTE: same ALB-public-IP limitation as the RPC target group above.
# The faucet EIP is publicly routable but lives outside this ALB's
# VPC, so ALB rejects the attachment. Park behind the same fronting
# follow-up; until then, the faucet is reachable directly by EIP on
# port 8080.

# Faucet ALB listeners stripped in Phase 1 for the same reason as the
# RPC ones above — public TLS lives on CloudFront (`cf_faucet.tf`).

# ------------- Shared ALB networking -------------

resource "aws_vpc" "alb" {
  provider             = aws.us_east_1
  cidr_block           = "10.45.0.0/24" # /24 distinct from devnet's 10.43.20.0/24
  enable_dns_hostnames = true
  tags                 = { Name = "suwappu-testnet-alb-vpc" }
}

resource "aws_subnet" "alb_public" {
  provider                = aws.us_east_1
  count                   = 2
  vpc_id                  = aws_vpc.alb.id
  cidr_block              = cidrsubnet(aws_vpc.alb.cidr_block, 2, count.index)
  availability_zone       = ["us-east-1a", "us-east-1b"][count.index]
  map_public_ip_on_launch = true
  tags                    = { Name = "suwappu-testnet-alb-subnet-${count.index}" }
}

resource "aws_internet_gateway" "alb" {
  provider = aws.us_east_1
  vpc_id   = aws_vpc.alb.id
  tags     = { Name = "suwappu-testnet-alb-igw" }
}

resource "aws_route_table" "alb_public" {
  provider = aws.us_east_1
  vpc_id   = aws_vpc.alb.id
  route {
    cidr_block = "0.0.0.0/0"
    gateway_id = aws_internet_gateway.alb.id
  }
  tags = { Name = "suwappu-testnet-alb-rt" }
}

resource "aws_route_table_association" "alb_public" {
  provider       = aws.us_east_1
  count          = length(aws_subnet.alb_public)
  subnet_id      = aws_subnet.alb_public[count.index].id
  route_table_id = aws_route_table.alb_public.id
}

resource "aws_security_group" "alb_rpc" {
  provider    = aws.us_east_1
  name        = "suwappu-testnet-alb-rpc-sg"
  description = "RPC ALB ingress"
  vpc_id      = aws_vpc.alb.id

  ingress {
    description = "HTTPS public"
    from_port   = 443
    to_port     = 443
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }
  ingress {
    description = "HTTP redirect public"
    from_port   = 80
    to_port     = 80
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }
  egress {
    description = "All egress"
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = { Name = "suwappu-testnet-alb-rpc-sg" }
}

resource "aws_security_group" "alb_faucet" {
  provider    = aws.us_east_1
  name        = "suwappu-testnet-alb-faucet-sg"
  description = "Faucet ALB ingress"
  vpc_id      = aws_vpc.alb.id

  ingress {
    description = "HTTPS public"
    from_port   = 443
    to_port     = 443
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }
  ingress {
    description = "HTTP redirect public"
    from_port   = 80
    to_port     = 80
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }
  egress {
    description = "All egress"
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = { Name = "suwappu-testnet-alb-faucet-sg" }
}
