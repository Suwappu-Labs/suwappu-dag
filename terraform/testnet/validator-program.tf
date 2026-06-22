# Track B — points accumulator infrastructure.
#
# The points-accumulator daemon (lands in a follow-up PR as
# `crates/suwappu-validator-program/`) runs on this EC2, reads from
# the external-uploads S3 bucket on a 5-minute schedule, computes
# the weekly leaderboard, writes results into the RDS instance,
# and exposes a public read API at
# `program.testnet.suwappu.suwappu.bot/leaderboard`.
#
# For this terraform PR we provision the INFRASTRUCTURE only —
# the daemon binary itself lands separately so a fresh foundation
# operator can run `./scripts/testnet/deploy.sh apply` today and
# have the EC2 + RDS sitting idle, ready to receive the binary.

# RDS for storing leaderboard + per-validator uptime history.
# Small (db.t4g.small) — the workload is single-digit writes per
# minute + ~50 read QPS from the public API.
resource "aws_db_subnet_group" "program" {
  provider   = aws.us_east_1
  name       = "suwappu-testnet-program-subnets"
  subnet_ids = aws_subnet.program_private.*.id
  tags       = { Name = "suwappu-testnet-program-subnets" }
}

resource "aws_security_group" "program_db" {
  provider    = aws.us_east_1
  name        = "suwappu-testnet-program-db-sg"
  description = "RDS access — only the program EC2 can reach the DB."
  vpc_id      = aws_vpc.program.id

  ingress {
    description     = "PostgreSQL from program EC2"
    from_port       = 5432
    to_port         = 5432
    protocol        = "tcp"
    security_groups = [aws_security_group.program_ec2.id]
  }
  egress {
    description = "All egress"
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = { Name = "suwappu-testnet-program-db-sg" }
}

resource "random_password" "program_db" {
  length  = 32
  special = false # keep RDS-safe
}

resource "aws_secretsmanager_secret" "program_db_password" {
  provider                = aws.us_east_1
  name                    = "suwappu-testnet/program/db-password"
  recovery_window_in_days = 7
}

resource "aws_secretsmanager_secret_version" "program_db_password" {
  provider      = aws.us_east_1
  secret_id     = aws_secretsmanager_secret.program_db_password.id
  secret_string = random_password.program_db.result
}

resource "aws_db_instance" "program" {
  provider                  = aws.us_east_1
  identifier                = "suwappu-testnet-program"
  engine                    = "postgres"
  engine_version            = "16.4"
  instance_class            = "db.t4g.small"
  allocated_storage         = 50
  max_allocated_storage     = 200
  storage_type              = "gp3"
  storage_encrypted         = true
  db_name                   = "validator_program"
  username                  = "validator_program"
  password                  = random_password.program_db.result
  db_subnet_group_name      = aws_db_subnet_group.program.name
  vpc_security_group_ids    = [aws_security_group.program_db.id]
  publicly_accessible       = false
  skip_final_snapshot       = false
  final_snapshot_identifier = "suwappu-testnet-program-final-snapshot"
  backup_retention_period   = 7
  backup_window             = "03:00-04:00"
  maintenance_window        = "sun:04:00-sun:05:00"
  deletion_protection       = true

  tags = { Name = "suwappu-testnet-program-db" }
}

# Program EC2 — runs the points-accumulator daemon.
# t4g.medium because the workload is light (read S3, compute
# leaderboard, write RDS, serve HTTP). Can scale up later if the
# leaderboard read API gets traffic.
resource "aws_security_group" "program_ec2" {
  provider    = aws.us_east_1
  name        = "suwappu-testnet-program-ec2-sg"
  description = "Points-accumulator EC2 ingress."
  vpc_id      = aws_vpc.program.id

  ingress {
    description = "Leaderboard HTTP (behind ALB later)"
    from_port   = 8090
    to_port     = 8090
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

  tags = { Name = "suwappu-testnet-program-ec2-sg" }
}

resource "aws_iam_role" "program_ec2" {
  provider = aws.us_east_1
  name     = "suwappu-testnet-program-ec2"
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "ec2.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })
}

resource "aws_iam_role_policy" "program_s3_read" {
  provider = aws.us_east_1
  name     = "external-uploads-read"
  role     = aws_iam_role.program_ec2.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Action = ["s3:GetObject", "s3:ListBucket"]
      Resource = [
        aws_s3_bucket.external_uploads.arn,
        "${aws_s3_bucket.external_uploads.arn}/*",
        aws_s3_bucket.artifacts.arn,
        "${aws_s3_bucket.artifacts.arn}/*",
      ]
    }]
  })
}

resource "aws_iam_role_policy" "program_secrets_read" {
  provider = aws.us_east_1
  name     = "db-secret-read"
  role     = aws_iam_role.program_ec2.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect   = "Allow"
      Action   = ["secretsmanager:GetSecretValue"]
      Resource = [aws_secretsmanager_secret.program_db_password.arn]
    }]
  })
}

resource "aws_iam_role_policy_attachment" "program_ssm" {
  provider   = aws.us_east_1
  role       = aws_iam_role.program_ec2.name
  policy_arn = "arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore"
}

resource "aws_iam_role_policy_attachment" "program_cw_agent" {
  provider   = aws.us_east_1
  role       = aws_iam_role.program_ec2.name
  policy_arn = "arn:aws:iam::aws:policy/CloudWatchAgentServerPolicy"
}

resource "aws_iam_instance_profile" "program" {
  provider = aws.us_east_1
  name     = "suwappu-testnet-program"
  role     = aws_iam_role.program_ec2.name
}

data "aws_ami" "program_ubuntu" {
  provider    = aws.us_east_1
  most_recent = true
  owners      = ["099720109477"]
  filter {
    name   = "name"
    values = ["ubuntu/images/hvm-ssd-gp3/ubuntu-noble-24.04-arm64-server-*"]
  }
}

resource "aws_instance" "program" {
  provider               = aws.us_east_1
  ami                    = data.aws_ami.program_ubuntu.id
  instance_type          = "t4g.medium"
  subnet_id              = aws_subnet.program_public.id
  vpc_security_group_ids = [aws_security_group.program_ec2.id]
  iam_instance_profile   = aws_iam_instance_profile.program.name

  root_block_device {
    volume_size = 30
    volume_type = "gp3"
  }

  # No user-data here — the daemon binary lands via a follow-up PR
  # + SSM deployment. For now the instance comes up bare; ops uses
  # SSM Session Manager to install the suwappu-validator-program
  # systemd unit when the binary is ready.
  user_data_replace_on_change = false

  tags = {
    Name           = "suwappu-testnet-program"
    "testnet:role" = "validator-program"
  }
}

resource "aws_eip" "program" {
  provider = aws.us_east_1
  instance = aws_instance.program.id
  domain   = "vpc"
  tags     = { Name = "suwappu-testnet-program-eip" }
}

# Program VPC — isolated from validator + faucet VPCs so the
# program daemon's blast radius is bounded.
resource "aws_vpc" "program" {
  provider             = aws.us_east_1
  cidr_block           = "10.44.0.0/24"
  enable_dns_hostnames = true
  tags                 = { Name = "suwappu-testnet-program-vpc" }
}

resource "aws_subnet" "program_public" {
  provider                = aws.us_east_1
  vpc_id                  = aws_vpc.program.id
  cidr_block              = "10.44.0.0/26"
  map_public_ip_on_launch = true
  availability_zone       = "us-east-1a"
  tags                    = { Name = "suwappu-testnet-program-subnet-public" }
}

# Two private subnets in different AZs — RDS requires ≥ 2.
resource "aws_subnet" "program_private" {
  provider          = aws.us_east_1
  count             = 2
  vpc_id            = aws_vpc.program.id
  cidr_block        = cidrsubnet(aws_vpc.program.cidr_block, 2, count.index + 1)
  availability_zone = ["us-east-1a", "us-east-1b"][count.index]
  tags              = { Name = "suwappu-testnet-program-subnet-private-${count.index}" }
}

resource "aws_internet_gateway" "program" {
  provider = aws.us_east_1
  vpc_id   = aws_vpc.program.id
  tags     = { Name = "suwappu-testnet-program-igw" }
}

resource "aws_route_table" "program_public" {
  provider = aws.us_east_1
  vpc_id   = aws_vpc.program.id
  route {
    cidr_block = "0.0.0.0/0"
    gateway_id = aws_internet_gateway.program.id
  }
  tags = { Name = "suwappu-testnet-program-rt" }
}

resource "aws_route_table_association" "program_public" {
  provider       = aws.us_east_1
  subnet_id      = aws_subnet.program_public.id
  route_table_id = aws_route_table.program_public.id
}
