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

# Lifecycle rules for the shared artifact bucket. Without these, every
# perf campaign drops ~70 MB of compressed NDJSON + reports per region
# into `logs/` and `reports/`, and CodeBuild source bundles accumulate
# in `sources/`. As of 2026-05-14 the bucket holds 2.9 GB across 105
# objects — the campaign-paused steady state. Bin/configs/genesis/keys
# are intentionally not lifecycled (current versions are load-bearing
# and tiny).
resource "aws_s3_bucket_lifecycle_configuration" "artifacts" {
  provider = aws.us_east_1
  bucket   = aws_s3_bucket.artifacts.id

  # Standard hygiene: clean up failed/abandoned multipart uploads.
  rule {
    id     = "abort-incomplete-multipart"
    status = "Enabled"

    filter {}

    abort_incomplete_multipart_upload {
      days_after_initiation = 7
    }
  }

  # CodeBuild source bundles are inputs only — `scripts/perf/build.sh`
  # uploads a fresh `sources/gsx-dag.zip` on every build invocation, so
  # any historical zip is dead weight.
  rule {
    id     = "expire-sources"
    status = "Enabled"

    filter {
      prefix = "sources/"
    }

    expiration {
      days = 7
    }

    noncurrent_version_expiration {
      noncurrent_days = 7
    }
  }

  # Campaign artifacts (logs + reports) tier down and expire. 30d in
  # STANDARD covers active investigation; STANDARD_IA after 30 days
  # halves storage cost with a small retrieval fee; GLACIER_IR after
  # 90 days drops cost ~6x; 365d expiration prevents unbounded growth.
  rule {
    id     = "tier-and-expire-logs"
    status = "Enabled"

    filter {
      prefix = "logs/"
    }

    transition {
      days          = 30
      storage_class = "STANDARD_IA"
    }

    transition {
      days          = 90
      storage_class = "GLACIER_IR"
    }

    expiration {
      days = 365
    }
  }

  rule {
    id     = "tier-and-expire-reports"
    status = "Enabled"

    filter {
      prefix = "reports/"
    }

    transition {
      days          = 30
      storage_class = "STANDARD_IA"
    }

    transition {
      days          = 90
      storage_class = "GLACIER_IR"
    }

    expiration {
      days = 365
    }
  }

  # Bucket-wide cleanup of obsolete versions left by versioned overwrites
  # (e.g. `bin/gsx-node` is overwritten on every CodeBuild release —
  # versioning preserves the prior copy as noncurrent, which we don't
  # need beyond 30 days). Per-prefix rules above already cover their
  # own noncurrent cleanup; this is the catch-all for the rest.
  rule {
    id     = "noncurrent-version-cleanup"
    status = "Enabled"

    filter {}

    noncurrent_version_expiration {
      noncurrent_days = 30
    }
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

# ap-southeast-2 (Sydney) and sa-east-1 (São Paulo) intentionally NOT
# instantiated for this campaign. Both showed >2s peer-to-peer RTT under
# real load and stalled the round driver waiting for them as leaders.
# When we add a "max observed round" snap-up to the round driver (so slow
# nodes can skip ahead by referencing parents from a later round), we can
# put them back. Tracked as a follow-up.
#
# module "ap_southeast_2" { ... }
# module "sa_east_1" { ... }

# af-south-1 is intentionally NOT instantiated. The region requires a manual
# one-time opt-in via the AWS console / `aws account enable-region`, and the
# gsn account is not currently opted in. The current campaign runs with the
# 6 always-on regions (us-east-1, us-west-2, eu-west-1, ap-northeast-1,
# ap-southeast-2, sa-east-1) which still provide ~300 ms cross-region RTT
# worst case.
#
# To re-enable: opt the region in once, then uncomment this block and bump
# the genesis manifest validator count from 6 to 7 in scripts/perf/gen-genesis.py.
#
# module "af_south_1" {
#   source           = "./modules/region"
#   providers        = { aws = aws.af_south_1 }
#   region_label     = "af-south-1"
#   authority_id     = 6
#   instance_type    = var.instance_type
#   ssh_public_key   = var.ssh_public_key
#   operator_ip_cidr = var.operator_ip_cidr
#   consensus_port   = var.consensus_port
#   client_port      = var.client_port
#   artifact_bucket  = aws_s3_bucket.artifacts.id
# }
