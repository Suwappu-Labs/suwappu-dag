# Faucet EC2 service.
#
# Single instance in us-east-1 — the faucet doesn't need HA for a
# devnet (worst case: the faucet goes down for an hour and devs
# briefly can't acquire fresh tokens; existing tokens still work).
# Sits alongside the 4 validator VPCs but in its own VPC for
# blast-radius isolation. ALB + DNS record + ACM SAN land in G2.

resource "aws_vpc" "faucet" {
  provider             = aws.us_east_1
  cidr_block           = "10.43.10.0/24"
  enable_dns_hostnames = true
  tags                 = { Name = "suwappu-devnet-faucet-vpc" }
}

resource "aws_subnet" "faucet_public" {
  provider                = aws.us_east_1
  vpc_id                  = aws_vpc.faucet.id
  cidr_block              = "10.43.10.0/26"
  map_public_ip_on_launch = true
  availability_zone       = "us-east-1a"
  tags                    = { Name = "suwappu-devnet-faucet-subnet" }
}

resource "aws_internet_gateway" "faucet" {
  provider = aws.us_east_1
  vpc_id   = aws_vpc.faucet.id
  tags     = { Name = "suwappu-devnet-faucet-igw" }
}

resource "aws_route_table" "faucet_public" {
  provider = aws.us_east_1
  vpc_id   = aws_vpc.faucet.id
  route {
    cidr_block = "0.0.0.0/0"
    gateway_id = aws_internet_gateway.faucet.id
  }
  tags = { Name = "suwappu-devnet-faucet-rt" }
}

resource "aws_route_table_association" "faucet_public" {
  provider       = aws.us_east_1
  subnet_id      = aws_subnet.faucet_public.id
  route_table_id = aws_route_table.faucet_public.id
}

# Security group:
# - SSH from operator IPs only (fallback; SSM is primary).
# - HTTP port 8080 open to 0.0.0.0/0 (ALB target). The faucet's own
#   per-IP token bucket gates abuse at the app layer.
resource "aws_security_group" "faucet" {
  provider    = aws.us_east_1
  name        = "suwappu-devnet-faucet-sg"
  description = "SUWAPPU devnet - faucet ingress"
  vpc_id      = aws_vpc.faucet.id

  ingress {
    description = "SSH from operator"
    from_port   = 22
    to_port     = 22
    protocol    = "tcp"
    cidr_blocks = var.operator_ip_cidrs
  }
  ingress {
    description = "Faucet HTTP"
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

  tags = { Name = "suwappu-devnet-faucet-sg" }
}

resource "aws_key_pair" "faucet_operator" {
  provider   = aws.us_east_1
  key_name   = "suwappu-devnet-faucet"
  public_key = var.ssh_public_key
}

# Secrets Manager secret for the faucet's ML-DSA-65 secret key. The
# *value* is uploaded out-of-band via:
#
#   aws secretsmanager put-secret-value \
#     --secret-id suwappu-devnet/faucet/mldsa-secret-key \
#     --secret-binary fileb://target/devnet/keys/faucet/mldsa.sk \
#     --profile gsn --region us-east-1
#
# terraform only manages the secret *resource* (its name + IAM
# policy); rotating the value is a SECOPS task per
# docs/devnet/faucet-key-ceremony.md.
resource "aws_secretsmanager_secret" "faucet_sk" {
  provider                = aws.us_east_1
  name                    = "suwappu-devnet/faucet/mldsa-secret-key"
  description             = "ML-DSA-65 secret key for the suwappu-devnet faucet authority. Rotate via faucet-key-ceremony.md."
  recovery_window_in_days = 30
  tags                    = { Name = "suwappu-devnet-faucet-mldsa-sk" }
}

resource "aws_iam_role" "faucet" {
  provider = aws.us_east_1
  name     = "suwappu-devnet-faucet-ec2"

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
        # The faucet binary + its public-key file ship via the same
        # artifact bucket as the validators.
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
  name     = "suwappu-devnet-faucet"
  role     = aws_iam_role.faucet.name
}

# Pick an arm64 AMI to match the t4g.small default. Could be made
# variable but a faucet doesn't need configurability.
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
    # G2: faucet talks to the validator mesh via the public RPC
    # DNS name (behind the ALB). The faucet's own per-IP rate
    # limit is OK to count itself among the IPs the validator
    # sees — the bucket capacity is generous enough that a few
    # drips/minute don't saturate it.
    rpc_url = "https://rpc.${var.devnet_subdomain}.${var.apex_domain}"
  })

  tags = {
    Name            = "suwappu-devnet-faucet"
    "devnet:role"   = "faucet"
    "devnet:region" = "us-east-1"
  }
}

resource "aws_eip" "faucet" {
  provider = aws.us_east_1
  instance = aws_instance.faucet.id
  domain   = "vpc"
  tags     = { Name = "suwappu-devnet-faucet-eip" }
}
