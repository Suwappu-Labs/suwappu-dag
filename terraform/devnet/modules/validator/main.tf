# Per-region devnet validator.
#
# Diff vs. terraform/perf/modules/region/main.tf:
#   * Persistent gp3 EBS attached at /dev/sdf and mounted at /var/lib/gsx
#     so consensus state + events.ndjson survive an instance replacement.
#   * RPC port (9092) open to 0.0.0.0/0 in the security group.
#   * Metrics port (9093) NOT exposed externally — security group does
#     not open it; CloudWatch agent on localhost scrapes it.
#   * Architecture-aware AMI lookup (arm64 default; auto-falls back to
#     amd64 if instance_type starts with non-t4g/m6g/c7g family).
#   * Tags include "devnet:role=validator" + "devnet:region=<region>"
#     so an ALB target group + dashboards can discover the fleet by tag.

terraform {
  required_providers {
    aws = {
      source                = "hashicorp/aws"
      configuration_aliases = [aws]
    }
  }
}

data "aws_region" "this" {}
data "aws_caller_identity" "this" {}

data "aws_ec2_instance_type_offerings" "supported" {
  filter {
    name   = "instance-type"
    values = [var.instance_type]
  }
  location_type = "availability-zone"
}

# Detect arm64 vs amd64 from the instance type family. t4g/m6g/c7g/r7g
# are arm64 (Graviton); everything else assumed amd64.
locals {
  is_arm64    = length(regexall("^(t4g|m6g|c7g|r7g|m7g|c6g|r6g)", var.instance_type)) > 0
  ami_arch    = local.is_arm64 ? "arm64" : "amd64"
  ami_pattern = "ubuntu/images/hvm-ssd-gp3/ubuntu-noble-24.04-${local.ami_arch}-server-*"
}

data "aws_ami" "ubuntu" {
  most_recent = true
  owners      = ["099720109477"] # Canonical
  filter {
    name   = "name"
    values = [local.ami_pattern]
  }
  filter {
    name   = "virtualization-type"
    values = ["hvm"]
  }
}

resource "aws_vpc" "this" {
  cidr_block           = "10.43.0.0/16" # /16 picked to avoid colliding with perf testnet's 10.42.0.0/16
  enable_dns_hostnames = true
  tags                 = { Name = "gsx-devnet-${var.region_label}-vpc" }
}

resource "aws_subnet" "public" {
  vpc_id                  = aws_vpc.this.id
  cidr_block              = "10.43.1.0/24"
  map_public_ip_on_launch = true
  availability_zone       = sort(data.aws_ec2_instance_type_offerings.supported.locations)[0]
  tags                    = { Name = "gsx-devnet-${var.region_label}-subnet" }
}

resource "aws_internet_gateway" "this" {
  vpc_id = aws_vpc.this.id
  tags   = { Name = "gsx-devnet-${var.region_label}-igw" }
}

resource "aws_route_table" "public" {
  vpc_id = aws_vpc.this.id
  route {
    cidr_block = "0.0.0.0/0"
    gateway_id = aws_internet_gateway.this.id
  }
  tags = { Name = "gsx-devnet-${var.region_label}-rt" }
}

resource "aws_route_table_association" "public" {
  subnet_id      = aws_subnet.public.id
  route_table_id = aws_route_table.public.id
}

# Security group:
# - SSH from operator IPs only (fallback path; SSM is primary).
# - Consensus + client + RPC ports open to 0.0.0.0/0. Auth is at the
#   message layer; the per-IP rate limit in gsx-rpc keeps abuse bounded.
# - Metrics port (9093) intentionally NOT in this list — only the local
#   CloudWatch agent reads it.
#
# SECURITY NOTE (audit M-RPC-1): exposing 9092 to 0.0.0.0/0 is the
# devnet posture. Defence-in-depth layers at the application level:
#   1. tower::timeout::TimeoutLayer (B2.2) — 30 s per-request wall-clock cap
#   2. tower::limit::ConcurrencyLimitLayer (B2) — 64 concurrent requests
#   3. tower_http::limit::RequestBodyLimitLayer (B2) — 1 MiB body cap
#   4. per_ip::PerIpRateLimiter (B2.1) — 60-burst / 10 req/s token bucket
# For production, replace this rule with the ALB SG (alb.tf) and bind
# rpc_listen to 127.0.0.1 so only the ALB can reach the validator.
resource "aws_security_group" "this" {
  name        = "gsx-devnet-${var.region_label}-sg"
  description = "GSX devnet - validator ingress"
  vpc_id      = aws_vpc.this.id

  ingress {
    description = "SSH from operator"
    from_port   = 22
    to_port     = 22
    protocol    = "tcp"
    cidr_blocks = var.operator_ip_cidrs
  }
  ingress {
    description = "Consensus peer traffic"
    from_port   = var.consensus_port
    to_port     = var.consensus_port
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }
  ingress {
    description = "Client intent submission"
    from_port   = var.client_port
    to_port     = var.client_port
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }
  ingress {
    description = "Public JSON-RPC (HTTP + WebSocket on same port)"
    from_port   = var.rpc_port
    to_port     = var.rpc_port
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

  tags = { Name = "gsx-devnet-${var.region_label}-sg" }
}

resource "aws_key_pair" "operator" {
  key_name   = "gsx-devnet-${var.region_label}"
  public_key = var.ssh_public_key
}

# Instance profile — read access to artifact bucket + ability to write
# event logs back. CloudWatchAgentServerPolicy lets the local agent
# forward metrics to CloudWatch.
resource "aws_iam_role" "ec2" {
  name = "gsx-devnet-${var.region_label}-ec2"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "ec2.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })
}

resource "aws_iam_role_policy" "s3_read_write_logs" {
  name = "artifact-bucket-rw"
  role = aws_iam_role.ec2.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = ["s3:GetObject", "s3:ListBucket"]
        Resource = [
          "arn:aws:s3:::${var.artifact_bucket}",
          "arn:aws:s3:::${var.artifact_bucket}/*",
        ]
      },
      {
        # Logs prefix only — validators upload their events.ndjson
        # rotation here so the operator can pull complete files.
        Effect   = "Allow"
        Action   = ["s3:PutObject"]
        Resource = ["arn:aws:s3:::${var.artifact_bucket}/logs/*"]
      },
    ]
  })
}

resource "aws_iam_role_policy_attachment" "ssm" {
  role       = aws_iam_role.ec2.name
  policy_arn = "arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore"
}

resource "aws_iam_role_policy_attachment" "cloudwatch_agent" {
  role       = aws_iam_role.ec2.name
  policy_arn = "arn:aws:iam::aws:policy/CloudWatchAgentServerPolicy"
}

resource "aws_iam_instance_profile" "this" {
  name = "gsx-devnet-${var.region_label}"
  role = aws_iam_role.ec2.name
}

# Persistent state volume — separate gp3 EBS attached at /dev/sdf,
# mounted at /var/lib/gsx by cloud-init. Surviving instance replacement
# is the whole point of a long-lived devnet; without this, every AMI
# refresh or instance type change would wipe consensus state.
resource "aws_ebs_volume" "state" {
  availability_zone = aws_subnet.public.availability_zone
  size              = var.state_volume_gb
  type              = "gp3"
  encrypted         = true

  tags = {
    Name                  = "gsx-devnet-${var.region_label}-state"
    "devnet:role"         = "state-volume"
    "devnet:region"       = var.region_label
    "devnet:authority_id" = tostring(var.authority_id)
  }

  lifecycle {
    # Prevent terraform from destroying the EBS volume on a stack
    # destroy. A stray `terraform destroy` (denied by deploy-aws.sh
    # for devnet, but defense in depth) cannot wipe months of state.
    prevent_destroy = true
  }
}

resource "aws_volume_attachment" "state" {
  device_name = "/dev/sdf"
  volume_id   = aws_ebs_volume.state.id
  instance_id = aws_instance.validator.id
}

resource "aws_instance" "validator" {
  ami                    = data.aws_ami.ubuntu.id
  instance_type          = var.instance_type
  subnet_id              = aws_subnet.public.id
  vpc_security_group_ids = [aws_security_group.this.id]
  key_name               = aws_key_pair.operator.key_name
  iam_instance_profile   = aws_iam_instance_profile.this.name

  root_block_device {
    volume_size = 20 # OS + binary only; state lives on the separate EBS.
    volume_type = "gp3"
  }

  user_data = templatefile("${path.module}/cloud-init.yaml", {
    artifact_bucket = var.artifact_bucket
    region_label    = var.region_label
    authority_id    = var.authority_id
    state_device    = "/dev/nvme1n1" # First non-root NVMe maps to /dev/sdf on Nitro instances
    metrics_port    = var.metrics_port
  })

  tags = {
    Name                  = "gsx-devnet-${var.region_label}"
    "devnet:role"         = "validator"
    "devnet:region"       = var.region_label
    "devnet:authority_id" = tostring(var.authority_id)
  }
}

resource "aws_eip" "this" {
  instance = aws_instance.validator.id
  domain   = "vpc"
  tags     = { Name = "gsx-devnet-${var.region_label}-eip" }
}
