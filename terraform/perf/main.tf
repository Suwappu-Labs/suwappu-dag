# 7-region GSX perf testnet.
#
# One module instantiation per AWS region. The set matches the paper's 7-of-9
# LTP corridor (paper §10.2) so the geographic-latency measurement is directly
# relevant to the LTP attestation timing claim.
#
# Region → authority_id mapping is stable and load-bearing — the genesis
# manifest's `validators[N]` must match `authority_id = N` here.

# Shared S3 bucket for binary + genesis + per-region config + logs. Lives in
# us-east-1 (the artifact provider region); every validator's instance
# profile gets read access via its per-region IAM role.
resource "aws_s3_bucket" "artifacts" {
  provider      = aws.us_east_1
  bucket        = var.artifact_bucket
  force_destroy = true
}

resource "aws_s3_bucket_public_access_block" "artifacts" {
  provider                = aws.us_east_1
  bucket                  = aws_s3_bucket.artifacts.id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_versioning" "artifacts" {
  provider = aws.us_east_1
  bucket   = aws_s3_bucket.artifacts.id
  versioning_configuration {
    status = "Enabled"
  }
}

module "us_east_1" {
  source           = "./modules/region"
  providers        = { aws = aws.us_east_1 }
  region_label     = "us-east-1"
  authority_id     = 0
  instance_type    = var.instance_type
  ssh_public_key   = var.ssh_public_key
  operator_ip_cidr = var.operator_ip_cidr
  consensus_port   = var.consensus_port
  client_port      = var.client_port
  artifact_bucket  = aws_s3_bucket.artifacts.id
}

module "us_west_2" {
  source           = "./modules/region"
  providers        = { aws = aws.us_west_2 }
  region_label     = "us-west-2"
  authority_id     = 1
  instance_type    = var.instance_type
  ssh_public_key   = var.ssh_public_key
  operator_ip_cidr = var.operator_ip_cidr
  consensus_port   = var.consensus_port
  client_port      = var.client_port
  artifact_bucket  = aws_s3_bucket.artifacts.id
}

module "eu_west_1" {
  source           = "./modules/region"
  providers        = { aws = aws.eu_west_1 }
  region_label     = "eu-west-1"
  authority_id     = 2
  instance_type    = var.instance_type
  ssh_public_key   = var.ssh_public_key
  operator_ip_cidr = var.operator_ip_cidr
  consensus_port   = var.consensus_port
  client_port      = var.client_port
  artifact_bucket  = aws_s3_bucket.artifacts.id
}

module "ap_northeast_1" {
  source           = "./modules/region"
  providers        = { aws = aws.ap_northeast_1 }
  region_label     = "ap-northeast-1"
  authority_id     = 3
  instance_type    = var.instance_type
  ssh_public_key   = var.ssh_public_key
  operator_ip_cidr = var.operator_ip_cidr
  consensus_port   = var.consensus_port
  client_port      = var.client_port
  artifact_bucket  = aws_s3_bucket.artifacts.id
}

module "ap_southeast_2" {
  source           = "./modules/region"
  providers        = { aws = aws.ap_southeast_2 }
  region_label     = "ap-southeast-2"
  authority_id     = 4
  instance_type    = var.instance_type
  ssh_public_key   = var.ssh_public_key
  operator_ip_cidr = var.operator_ip_cidr
  consensus_port   = var.consensus_port
  client_port      = var.client_port
  artifact_bucket  = aws_s3_bucket.artifacts.id
}

module "sa_east_1" {
  source           = "./modules/region"
  providers        = { aws = aws.sa_east_1 }
  region_label     = "sa-east-1"
  authority_id     = 5
  instance_type    = var.instance_type
  ssh_public_key   = var.ssh_public_key
  operator_ip_cidr = var.operator_ip_cidr
  consensus_port   = var.consensus_port
  client_port      = var.client_port
  artifact_bucket  = aws_s3_bucket.artifacts.id
}

# af-south-1 requires manual region opt-in on the AWS account before this
# applies. If `terraform apply` errors with `OptInRequired`, enable the
# region in the AWS console once; subsequent applies will work.
module "af_south_1" {
  source           = "./modules/region"
  providers        = { aws = aws.af_south_1 }
  region_label     = "af-south-1"
  authority_id     = 6
  instance_type    = var.instance_type
  ssh_public_key   = var.ssh_public_key
  operator_ip_cidr = var.operator_ip_cidr
  consensus_port   = var.consensus_port
  client_port      = var.client_port
  artifact_bucket  = aws_s3_bucket.artifacts.id
}
