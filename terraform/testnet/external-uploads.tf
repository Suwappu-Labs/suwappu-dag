# External validator event-log uploads.
#
# The points program (Track B) ranks operators by uptime + commits-
# observed + bugs-found. Foundation can compute uptime via probes,
# but commits-observed requires reading each validator's NDJSON
# event log — and external validators don't run in our AWS account.
#
# Solution: a public-WRITE-only S3 bucket where every validator
# uploads its rotated events.ndjson once per hour. Each operator
# gets a scoped IAM role tied to their validator's authority_id so
# they can only write under their own prefix.
#
# The points accumulator daemon (Track B follow-up — see
# crates/suwappu-validator-program/) reads from this bucket on a
# schedule and writes the leaderboard to validator-program.tf's
# RDS instance.

resource "aws_s3_bucket" "external_uploads" {
  provider      = aws.us_east_1
  bucket        = var.external_uploads_bucket
  force_destroy = false

  tags = { Name = "suwappu-testnet-validator-uploads" }
}

# Public READ is blocked — only the points-accumulator daemon's
# IAM role can list/get from here. WRITE is per-operator scoped.
resource "aws_s3_bucket_public_access_block" "external_uploads" {
  provider                = aws.us_east_1
  bucket                  = aws_s3_bucket.external_uploads.id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_versioning" "external_uploads" {
  provider = aws.us_east_1
  bucket   = aws_s3_bucket.external_uploads.id
  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_lifecycle_configuration" "external_uploads" {
  provider = aws.us_east_1
  bucket   = aws_s3_bucket.external_uploads.id

  rule {
    id     = "abort-incomplete-multipart"
    status = "Enabled"
    filter {}
    abort_incomplete_multipart_upload {
      days_after_initiation = 1
    }
  }

  # External uploads age out aggressively — points-accumulator
  # reads them within minutes; we don't archive operator logs
  # beyond the points-computation window.
  rule {
    id     = "expire-uploads"
    status = "Enabled"
    filter {
      prefix = "uploads/"
    }
    expiration {
      days = 14
    }
    noncurrent_version_expiration {
      noncurrent_days = 7
    }
  }
}

# A SINGLE IAM user is created per onboarded external operator
# via a separate process (out-of-band; OPERATIONS.md § 11 covers
# the workflow). Each user's policy is scoped to their own prefix
# `uploads/<authority_id>/`. terraform just creates the BASE
# policy template that the onboarding workflow specializes.
resource "aws_iam_policy" "external_operator_upload_template" {
  provider    = aws.us_east_1
  name        = "suwappu-testnet-external-operator-upload-template"
  description = "Template policy for external validator operators — substitute AUTHORITY_ID_PLACEHOLDER at onboarding time. The onboarding workflow creates a per-operator IAM user + attaches a specialized copy of this policy."
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = ["s3:PutObject", "s3:PutObjectAcl"]
        # The placeholder is substituted by scripts/testnet/onboard-operator.sh
        # at the time a new operator is added — terraform doesn't manage
        # per-operator users, just the template.
        Resource = ["arn:aws:s3:::${var.external_uploads_bucket}/uploads/AUTHORITY_ID_PLACEHOLDER/*"]
      },
    ]
  })
}
