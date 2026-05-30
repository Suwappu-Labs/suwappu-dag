# G2 — ALB layer.
#
# Two internet-facing ALBs:
#   * `aws_lb.rpc`     — fronts the 4 validators on port 9092 (JSON-RPC + WS).
#   * `aws_lb.faucet`  — fronts the single faucet on port 8080.
#
# Both terminate TLS on the wildcard ACM cert. The ALBs live in
# us-east-1 (where the artifact bucket + faucet + ACM cert all
# already are).
#
# IMPORTANT — ALB target_type = "ip" rejects public IPs that are not
# in the ALB's own VPC subnet, RFC1918, or RFC6598, even within the
# same region. The previous header comment here claimed otherwise;
# that was wrong, and it cost the testnet a full apply round when
# the same pattern was inherited (see
# terraform/testnet/alb.tf for the matching note). The 4 validators
# each live in their own regional VPC and have public EIPs, so none
# of them qualify as ALB targets.
#
# Two viable fronting shapes for the follow-up:
#   1. Per-region NLB + AWS Global Accelerator. Validators stay
#      reachable by EIP; GA anycast IPs become the public surface.
#   2. VPC peering from this ALB VPC to each validator VPC, then
#      target by private IP. Requires non-overlapping CIDR planning
#      across the regional VPCs.
#
# Until one of those lands, this stack ships the ALBs + cert + DNS
# skeleton with NO target attachments (the wildcard endpoint will
# return 503 until fronting is layered in). Clients reach validators
# by direct EIP from `terraform output validators`.

# ------------- RPC ALB -------------

resource "aws_lb" "rpc" {
  provider           = aws.us_east_1
  name               = "gsx-devnet-rpc"
  internal           = false
  load_balancer_type = "application"

  # Use a small dedicated VPC for the ALB. Sharing with the
  # us-east-1 validator's VPC would entangle their lifecycles.
  subnets         = aws_subnet.alb_public.*.id
  security_groups = [aws_security_group.alb_rpc.id]

  enable_http2 = true # WebSocket support is on HTTP/1.1 but the ALB also serves HTTP/2 JSON-RPC

  drop_invalid_header_fields = true

  tags = { Name = "gsx-devnet-rpc-alb" }
}

resource "aws_lb_target_group" "rpc" {
  provider    = aws.us_east_1
  name        = "gsx-devnet-rpc-tg"
  port        = var.rpc_port
  protocol    = "HTTP"
  target_type = "ip"
  vpc_id      = aws_vpc.alb.id

  health_check {
    enabled             = true
    protocol            = "HTTP"
    port                = "traffic-port"
    path                = "/"       # POST-only endpoint; HEAD returns 405 which we accept as "alive"
    matcher             = "200-499" # Any well-formed HTTP response means the validator is up
    healthy_threshold   = 2
    unhealthy_threshold = 3
    interval            = 30
    timeout             = 10
  }

  # WebSocket sessions can stay open for minutes; default ALB idle
  # timeout of 60s would cut them. Stickiness OFF — every JSON-RPC
  # request is stateless and should round-robin across validators.
  stickiness {
    type    = "lb_cookie"
    enabled = false
  }

  tags = { Name = "gsx-devnet-rpc-tg" }
}

# NOTE: ALB target_type = "ip" rejects public IPs that are not in the
# ALB's own VPC subnet, RFC1918, or RFC6598 — see the header comment
# above. The 4 cross-VPC `aws_lb_target_group_attachment.rpc_*` blocks
# that used to live here were removed for the same reason as in
# terraform/testnet/alb.tf. Until per-region NLB + Global Accelerator
# or VPC peering with private-IP targets lands, this target group
# stays empty and the wildcard endpoint returns 503.

resource "aws_lb_listener" "rpc_https" {
  provider          = aws.us_east_1
  load_balancer_arn = aws_lb.rpc.arn
  port              = 443
  protocol          = "HTTPS"
  ssl_policy        = "ELBSecurityPolicy-TLS13-1-2-2021-06"
  certificate_arn   = aws_acm_certificate_validation.wildcard.certificate_arn

  default_action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.rpc.arn
  }
}

# HTTP → HTTPS redirect so legacy clients fall through cleanly.
resource "aws_lb_listener" "rpc_http_redirect" {
  provider          = aws.us_east_1
  load_balancer_arn = aws_lb.rpc.arn
  port              = 80
  protocol          = "HTTP"

  default_action {
    type = "redirect"
    redirect {
      protocol    = "HTTPS"
      port        = "443"
      status_code = "HTTP_301"
    }
  }
}

# ------------- Faucet ALB -------------

resource "aws_lb" "faucet" {
  provider           = aws.us_east_1
  name               = "gsx-devnet-faucet"
  internal           = false
  load_balancer_type = "application"
  subnets            = aws_subnet.alb_public.*.id
  security_groups    = [aws_security_group.alb_faucet.id]

  drop_invalid_header_fields = true

  tags = { Name = "gsx-devnet-faucet-alb" }
}

resource "aws_lb_target_group" "faucet" {
  provider    = aws.us_east_1
  name        = "gsx-devnet-faucet-tg"
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

  tags = { Name = "gsx-devnet-faucet-tg" }
}

# NOTE: same ALB-public-IP limitation as the RPC target group above.
# The faucet EIP is publicly routable but lives outside this ALB's
# VPC, so ALB rejects the attachment. Park behind the same fronting
# follow-up; until then, the faucet is reachable directly by EIP on
# port 8080.

resource "aws_lb_listener" "faucet_https" {
  provider          = aws.us_east_1
  load_balancer_arn = aws_lb.faucet.arn
  port              = 443
  protocol          = "HTTPS"
  ssl_policy        = "ELBSecurityPolicy-TLS13-1-2-2021-06"
  certificate_arn   = aws_acm_certificate_validation.wildcard.certificate_arn

  default_action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.faucet.arn
  }
}

resource "aws_lb_listener" "faucet_http_redirect" {
  provider          = aws.us_east_1
  load_balancer_arn = aws_lb.faucet.arn
  port              = 80
  protocol          = "HTTP"

  default_action {
    type = "redirect"
    redirect {
      protocol    = "HTTPS"
      port        = "443"
      status_code = "HTTP_301"
    }
  }
}

# ------------- Shared ALB networking -------------

resource "aws_vpc" "alb" {
  provider             = aws.us_east_1
  cidr_block           = "10.43.20.0/24"
  enable_dns_hostnames = true
  tags                 = { Name = "gsx-devnet-alb-vpc" }
}

# ALBs require subnets in ≥ 2 AZs.
resource "aws_subnet" "alb_public" {
  provider                = aws.us_east_1
  count                   = 2
  vpc_id                  = aws_vpc.alb.id
  cidr_block              = cidrsubnet(aws_vpc.alb.cidr_block, 2, count.index)
  availability_zone       = ["us-east-1a", "us-east-1b"][count.index]
  map_public_ip_on_launch = true
  tags                    = { Name = "gsx-devnet-alb-subnet-${count.index}" }
}

resource "aws_internet_gateway" "alb" {
  provider = aws.us_east_1
  vpc_id   = aws_vpc.alb.id
  tags     = { Name = "gsx-devnet-alb-igw" }
}

resource "aws_route_table" "alb_public" {
  provider = aws.us_east_1
  vpc_id   = aws_vpc.alb.id
  route {
    cidr_block = "0.0.0.0/0"
    gateway_id = aws_internet_gateway.alb.id
  }
  tags = { Name = "gsx-devnet-alb-rt" }
}

resource "aws_route_table_association" "alb_public" {
  provider       = aws.us_east_1
  count          = length(aws_subnet.alb_public)
  subnet_id      = aws_subnet.alb_public[count.index].id
  route_table_id = aws_route_table.alb_public.id
}

resource "aws_security_group" "alb_rpc" {
  provider    = aws.us_east_1
  name        = "gsx-devnet-alb-rpc-sg"
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

  tags = { Name = "gsx-devnet-alb-rpc-sg" }
}

resource "aws_security_group" "alb_faucet" {
  provider    = aws.us_east_1
  name        = "gsx-devnet-alb-faucet-sg"
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

  tags = { Name = "gsx-devnet-alb-faucet-sg" }
}
