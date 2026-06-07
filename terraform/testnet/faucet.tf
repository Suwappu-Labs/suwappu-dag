# Testnet faucet EC2.
#
# Forked from terraform/devnet/faucet.tf with these diffs:
#   * VPC + subnet on distinct CIDR (10.45.10.0/24 vs devnet's
#     10.43.10.0/24) so the two faucets can coexist.
#   * Separate Secrets Manager secret + IAM role + S3 bucket
#     references.
#   * cloud-init env vars point at the testnet RPC DNS (rpc.
#     testnet.suwappu.globalsettlement.com).

resource "aws_vpc" "faucet" {
  provider             = aws.us_east_1
  cidr_block           = "10.45.10.0/24"
  enable_dns_hostnames = true
  tags                 = { Name = "suwappu-testnet-faucet-vpc" }
}

resource "aws_subnet" "faucet_public" {
  provider                = aws.us_east_1
  vpc_id                  = aws_vpc.faucet.id
  cidr_block              = "10.45.10.0/26"
  map_public_ip_on_launch = true
  availability_zone       = "us-east-1a"
  tags                    = { Name = "suwappu-testnet-faucet-subnet" }
}

resource "aws_internet_gateway" "faucet" {
  provider = aws.us_east_1
  vpc_id   = aws_vpc.faucet.id
  tags     = { Name = "suwappu-testnet-faucet-igw" }
}

resource "aws_route_table" "faucet_public" {
  provider = aws.us_east_1
  vpc_id   = aws_vpc.faucet.id
  route {
    cidr_block = "0.0.0.0/0"
    gateway_id = aws_internet_gateway.faucet.id
  }
  tags = { Name = "suwappu-testnet-faucet-rt" }
}

resource "aws_route_table_association" "faucet_public" {
  provider       = aws.us_east_1
  subnet_id      = aws_subnet.faucet_public.id
  route_table_id = aws_route_table.faucet_public.id
}

resource "aws_security_group" "faucet" {
  provider    = aws.us_east_1
  name        = "suwappu-testnet-faucet-sg"
  description = "SUWAPPU testnet - faucet ingress"
  vpc_id      = aws_vpc.faucet.id

  ingress {
    description = "SSH from operator"
    from_port   = 22
    to_port     = 22
    protocol    = "tcp"
    cidr_blocks = var.operator_ip_cidrs
  }
  ingress {
    description = "Faucet HTTP (ALB target)"
    from_port   = 8080
    to_port     = 8080
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

  tags = { Name = "suwappu-testnet-faucet-sg" }
}

resource "aws_key_pair" "faucet_operator" {
  provider   = aws.us_east_1
  key_name   = "suwappu-testnet-faucet"
  public_key = var.ssh_public_key
}

# Secrets Manager secret for the testnet faucet's ML-DSA-65 secret
# key. SEPARATE from the devnet faucet's secret — they have
# different keypairs and rotate independently.
resource "aws_secretsmanager_secret" "faucet_sk" {
  provider                = aws.us_east_1
  name                    = "suwappu-testnet/faucet/mldsa-secret-key"
  description             = "ML-DSA-65 secret key for the suwappu-testnet faucet authority. Rotate via docs/devnet/faucet-key-ceremony.md."
  recovery_window_in_days = 30
  tags                    = { Name = "suwappu-testnet-faucet-mldsa-sk" }
}

resource "aws_iam_role" "faucet" {
  provider = aws.us_east_1
  name     = "suwappu-testnet-faucet-ec2"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "ec2.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })
}

resource "aws_iam_role_policy" "faucet_secret_read" {
  provider = aws.us_east_1
  name     = "faucet-sk-read"
  role     = aws_iam_role.faucet.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect   = "Allow"
      Action   = ["secretsmanager:GetSecretValue"]
      Resource = [aws_secretsmanager_secret.faucet_sk.arn]
    }]
  })
}

resource "aws_iam_role_policy" "faucet_s3_read" {
  provider = aws.us_east_1
  name     = "faucet-bin-read"
  role     = aws_iam_role.faucet.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Action = ["s3:GetObject"]
      Resource = [
        "arn:aws:s3:::${aws_s3_bucket.artifacts.id}/bin/suwappu-faucet",
        "arn:aws:s3:::${aws_s3_bucket.artifacts.id}/keys/faucet/mldsa.pk",
      ]
    }]
  })
}

resource "aws_iam_role_policy_attachment" "faucet_ssm" {
  provider   = aws.us_east_1
  role       = aws_iam_role.faucet.name
  policy_arn = "arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore"
}

resource "aws_iam_role_policy_attachment" "faucet_cw_agent" {
  provider   = aws.us_east_1
  role       = aws_iam_role.faucet.name
  policy_arn = "arn:aws:iam::aws:policy/CloudWatchAgentServerPolicy"
}

resource "aws_iam_instance_profile" "faucet" {
  provider = aws.us_east_1
  name     = "suwappu-testnet-faucet"
  role     = aws_iam_role.faucet.name
}

data "aws_ami" "faucet_ubuntu" {
  provider    = aws.us_east_1
  most_recent = true
  owners      = ["099720109477"]
  filter {
    name   = "name"
    values = ["ubuntu/images/hvm-ssd-gp3/ubuntu-noble-24.04-arm64-server-*"]
  }
  filter {
    name   = "virtualization-type"
    values = ["hvm"]
  }
}

resource "aws_instance" "faucet" {
  provider               = aws.us_east_1
  ami                    = data.aws_ami.faucet_ubuntu.id
  instance_type          = "t4g.small"
  subnet_id              = aws_subnet.faucet_public.id
  vpc_security_group_ids = [aws_security_group.faucet.id]
  key_name               = aws_key_pair.faucet_operator.key_name
  iam_instance_profile   = aws_iam_instance_profile.faucet.name

  root_block_device {
    volume_size = 20
    volume_type = "gp3"
  }

  user_data = templatefile("${path.module}/faucet-cloud-init.yaml", {
    artifact_bucket = aws_s3_bucket.artifacts.id
    secret_arn      = aws_secretsmanager_secret.faucet_sk.arn
    network_id      = var.network_id
    rpc_url         = "https://rpc.${var.testnet_subdomain}.${var.apex_domain}"
  })

  tags = {
    Name             = "suwappu-testnet-faucet"
    "testnet:role"   = "faucet"
    "testnet:region" = "us-east-1"
  }
}

resource "aws_eip" "faucet" {
  provider = aws.us_east_1
  instance = aws_instance.faucet.id
  domain   = "vpc"
  tags     = { Name = "suwappu-testnet-faucet-eip" }
}
