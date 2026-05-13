# Per-region validator instance.
#
# Provisions: VPC + public subnet + IGW + route table + security group +
# EC2 instance + EIP + IAM role. Cloud-init pulls the gsx-node musl binary
# and genesis manifest from the shared artifact bucket and starts a systemd
# unit. Each region's instance is independent — they only know about each
# other through their public IPs (rendered into the per-instance node.toml
# by scripts/perf/render-configs.sh after `terraform apply`).

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

# Use the canonical Ubuntu 24.04 LTS AMI. Region-specific lookup avoids
# hard-coding AMI ids that drift over time.
data "aws_ami" "ubuntu" {
  most_recent = true
  owners      = ["099720109477"] # Canonical
  filter {
    name   = "name"
    values = ["ubuntu/images/hvm-ssd-gp3/ubuntu-noble-24.04-amd64-server-*"]
  }
  filter {
    name   = "virtualization-type"
    values = ["hvm"]
  }
}

# Minimal VPC: one public subnet, one IGW. The perf testnet doesn't need NAT
# — every validator is on a public EIP because peers across regions need to
# reach each other without VPC peering complexity.
resource "aws_vpc" "this" {
  cidr_block           = "10.42.0.0/16"
  enable_dns_hostnames = true
  tags                 = { Name = "gsx-perf-${var.region_label}-vpc" }
}

resource "aws_subnet" "public" {
  vpc_id                  = aws_vpc.this.id
  cidr_block              = "10.42.1.0/24"
  map_public_ip_on_launch = true
  tags                    = { Name = "gsx-perf-${var.region_label}-subnet" }
}

resource "aws_internet_gateway" "this" {
  vpc_id = aws_vpc.this.id
  tags   = { Name = "gsx-perf-${var.region_label}-igw" }
}

resource "aws_route_table" "public" {
  vpc_id = aws_vpc.this.id
  route {
    cidr_block = "0.0.0.0/0"
    gateway_id = aws_internet_gateway.this.id
  }
  tags = { Name = "gsx-perf-${var.region_label}-rt" }
}

resource "aws_route_table_association" "public" {
  subnet_id      = aws_subnet.public.id
  route_table_id = aws_route_table.public.id
}

# Security group:
# - SSH from the operator IP only.
# - Consensus + client ports open to the world. This is a closed testnet
#   identified by the network_id in genesis; auth happens at the message
#   layer (ML-DSA signatures inside Cert/Vote/FastPath/LTP), not at the
#   socket layer. Open ports keep the geographic-latency measurement clean.
resource "aws_security_group" "this" {
  name        = "gsx-perf-${var.region_label}-sg"
  description = "GSX perf testnet — validator ingress"
  vpc_id      = aws_vpc.this.id

  ingress {
    description = "SSH from operator"
    from_port   = 22
    to_port     = 22
    protocol    = "tcp"
    cidr_blocks = [var.operator_ip_cidr]
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
  egress {
    description = "All egress"
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = { Name = "gsx-perf-${var.region_label}-sg" }
}

resource "aws_key_pair" "operator" {
  key_name   = "gsx-perf-${var.region_label}"
  public_key = var.ssh_public_key
}

# Instance profile — read-only S3 access to the artifact bucket so cloud-init
# can pull the binary + genesis manifest.
resource "aws_iam_role" "ec2" {
  name = "gsx-perf-${var.region_label}-ec2"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "ec2.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })
}

resource "aws_iam_role_policy" "s3_read" {
  name = "artifact-bucket-read"
  role = aws_iam_role.ec2.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Action = ["s3:GetObject", "s3:ListBucket"]
      Resource = [
        "arn:aws:s3:::${var.artifact_bucket}",
        "arn:aws:s3:::${var.artifact_bucket}/*",
      ]
    }]
  })
}

resource "aws_iam_instance_profile" "this" {
  name = "gsx-perf-${var.region_label}"
  role = aws_iam_role.ec2.name
}

# Cloud-init: bootstrap the validator. The actual node.toml is rendered
# *after* this apply by scripts/perf/render-configs.sh (which knows all the
# EIPs once they're allocated) and uploaded to s3://<bucket>/configs/.
resource "aws_instance" "validator" {
  ami                    = data.aws_ami.ubuntu.id
  instance_type          = var.instance_type
  subnet_id              = aws_subnet.public.id
  vpc_security_group_ids = [aws_security_group.this.id]
  key_name               = aws_key_pair.operator.key_name
  iam_instance_profile   = aws_iam_instance_profile.this.name

  root_block_device {
    volume_size = 20
    volume_type = "gp3"
  }

  user_data = templatefile("${path.module}/cloud-init.yaml", {
    artifact_bucket = var.artifact_bucket
    region_label    = var.region_label
    authority_id    = var.authority_id
  })

  tags = {
    Name        = "gsx-perf-${var.region_label}"
    AuthorityId = tostring(var.authority_id)
  }
}

resource "aws_eip" "this" {
  instance = aws_instance.validator.id
  domain   = "vpc"
  tags     = { Name = "gsx-perf-${var.region_label}-eip" }
}
