# One region's RPC front: an internet-facing ALB in the validator's VPC
# that terminates TLS (regional ACM cert), is rate-limited by a REGIONAL
# WAF, and forwards HTTP(S)+WebSocket to the validator instance on the
# RPC port. Global Accelerator (ga.tf) fronts the 7 of these behind one
# anycast endpoint and fails over between them in <1 min — including for
# POST, which the old CloudFront path could not do (issue #237).

terraform {
  required_providers {
    aws = {
      source                = "hashicorp/aws"
      configuration_aliases = [aws]
    }
  }
}

# --- ALB security group: public 443/80 in, all egress (reaches the
#     in-VPC validator on the RPC port, which its SG already allows). ---
resource "aws_security_group" "alb" {
  name        = "${var.name_prefix}${var.region_label}-rpc-alb-sg"
  description = "RPC ALB ingress (HTTPS/HTTP)"
  vpc_id      = var.vpc_id

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
    description = "All egress (to the in-VPC validator RPC port)"
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = { Name = "${var.name_prefix}${var.region_label}-rpc-alb-sg" }
}

resource "aws_lb" "rpc" {
  name               = "${var.name_prefix}${var.region_label}-rpc"
  internal           = false
  load_balancer_type = "application"
  subnets            = var.subnet_ids
  security_groups    = [aws_security_group.alb.id]

  enable_http2               = true
  drop_invalid_header_fields = true

  tags = { Name = "${var.name_prefix}${var.region_label}-rpc-alb" }
}

resource "aws_lb_target_group" "rpc" {
  name        = "${var.name_prefix}${var.region_label}-rpc-tg"
  port        = var.target_port
  protocol    = "HTTP"
  target_type = "instance"
  vpc_id      = var.vpc_id

  # gsx-rpc answers GET / with 405 (JSON-RPC is POST-only), so accept
  # the 2xx-4xx band as "process is up" — same matcher as the old
  # single-region skeleton in alb.tf.
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

  # WebSocket upgrade and JSON-RPC POST are both plain request/response
  # over one connection; no stickiness needed (all 7 regions serve the
  # same committed chain state).
  stickiness {
    type    = "lb_cookie"
    enabled = false
  }

  tags = { Name = "${var.name_prefix}${var.region_label}-rpc-tg" }
}

resource "aws_lb_target_group_attachment" "rpc" {
  target_group_arn = aws_lb_target_group.rpc.arn
  target_id        = var.target_instance_id
  port             = var.target_port
}

# HTTPS:443 terminates TLS with the regional cert and forwards to the
# validator. WebSocket rides this same listener (HTTP/1.1 Upgrade).
resource "aws_lb_listener" "https" {
  load_balancer_arn = aws_lb.rpc.arn
  port              = 443
  protocol          = "HTTPS"
  ssl_policy        = "ELBSecurityPolicy-TLS13-1-2-2021-06"
  certificate_arn   = var.certificate_arn

  default_action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.rpc.arn
  }
}

# HTTP:80 → HTTPS:443 redirect (so http:// clients are upgraded).
resource "aws_lb_listener" "http_redirect" {
  load_balancer_arn = aws_lb.rpc.arn
  port              = 80
  protocol          = "HTTP"

  default_action {
    type = "redirect"
    redirect {
      port        = "443"
      protocol    = "HTTPS"
      status_code = "HTTP_301"
    }
  }
}

# --- REGIONAL WAF: per-IP rate limit + AWS managed baseline rules.
#     (The old CloudFront WAF in waf.tf is scope=CLOUDFRONT and can't
#     attach to a regional ALB.) ---
resource "aws_wafv2_web_acl" "rpc" {
  name        = "${var.name_prefix}${var.region_label}-rpc-waf"
  description = "Regional WAF for the ${var.region_label} RPC ALB"
  scope       = "REGIONAL"

  default_action {
    allow {}
  }

  rule {
    name     = "rate-limit-per-ip"
    priority = 0
    action {
      block {}
    }
    statement {
      rate_based_statement {
        limit              = var.waf_rate_limit
        aggregate_key_type = "IP"
      }
    }
    visibility_config {
      cloudwatch_metrics_enabled = true
      metric_name                = "${var.name_prefix}${var.region_label}-rpc-rate"
      sampled_requests_enabled   = true
    }
  }

  rule {
    name     = "common-rule-set"
    priority = 1
    override_action {
      none {}
    }
    statement {
      managed_rule_group_statement {
        vendor_name = "AWS"
        name        = "AWSManagedRulesCommonRuleSet"
      }
    }
    visibility_config {
      cloudwatch_metrics_enabled = true
      metric_name                = "${var.name_prefix}${var.region_label}-rpc-common"
      sampled_requests_enabled   = true
    }
  }

  rule {
    name     = "ip-reputation"
    priority = 2
    override_action {
      none {}
    }
    statement {
      managed_rule_group_statement {
        vendor_name = "AWS"
        name        = "AWSManagedRulesAmazonIpReputationList"
      }
    }
    visibility_config {
      cloudwatch_metrics_enabled = true
      metric_name                = "${var.name_prefix}${var.region_label}-rpc-iprep"
      sampled_requests_enabled   = true
    }
  }

  visibility_config {
    cloudwatch_metrics_enabled = true
    metric_name                = "${var.name_prefix}${var.region_label}-rpc-waf"
    sampled_requests_enabled   = true
  }

  tags = { Name = "${var.name_prefix}${var.region_label}-rpc-waf" }
}

resource "aws_wafv2_web_acl_association" "rpc" {
  resource_arn = aws_lb.rpc.arn
  web_acl_arn  = aws_wafv2_web_acl.rpc.arn
}
