# AWS CodeBuild project for the testnet validator binaries.
#
# Targets aarch64-unknown-linux-gnu — the testnet's c7g.xlarge seed
# instances are arm64, so the perf stack's x86_64 build doesn't apply.
# Image is pulled from public.ecr.aws (Amazon's ARM CodeBuild image),
# not Docker Hub — avoids the Docker Hub rate-limit class of failure.
#
# Reuses the perf stack's `/gsx-perf/gsx-db-deploy-key` SSM parameter
# (same gsx-db repo, same deploy key) — kept under one path to avoid
# rotating two secrets in lockstep.

resource "aws_iam_role" "codebuild" {
  provider = aws.us_east_1
  name     = "gsx-testnet-codebuild"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "codebuild.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })
}

resource "aws_iam_role_policy" "codebuild" {
  provider = aws.us_east_1
  name     = "testnet-codebuild"
  role     = aws_iam_role.codebuild.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "logs:CreateLogGroup",
          "logs:CreateLogStream",
          "logs:PutLogEvents",
        ]
        Resource = ["arn:aws:logs:*:*:*"]
      },
      {
        Effect = "Allow"
        Action = [
          "s3:GetObject",
          "s3:ListBucket",
          "s3:PutObject",
          "s3:DeleteObject",
        ]
        Resource = [
          "arn:aws:s3:::${aws_s3_bucket.artifacts.id}",
          "arn:aws:s3:::${aws_s3_bucket.artifacts.id}/*",
        ]
      },
      {
        Effect = "Allow"
        Action = [
          "ssm:GetParameter",
          "ssm:GetParameters",
        ]
        Resource = [
          "arn:aws:ssm:us-east-1:*:parameter/gsx-perf/*",
        ]
      },
      {
        Effect   = "Allow"
        Action   = ["kms:Decrypt"]
        Resource = ["*"]
      },
    ]
  })
}

resource "aws_codebuild_project" "build" {
  provider     = aws.us_east_1
  name         = "gsx-testnet-build"
  description  = "Native-compile gsx-node binaries to aarch64-unknown-linux-gnu for the testnet seed cluster"
  service_role = aws_iam_role.codebuild.arn

  build_timeout = 45

  artifacts {
    type      = "S3"
    location  = aws_s3_bucket.artifacts.id
    path      = "bin"
    packaging = "NONE"
    # CodeBuild treats `name = "."` as a literal folder name, so
    # outputs land under `bin/./<file>` (a `.` directory). The
    # download path in scripts/testnet/build.sh fetches
    # `s3://$BUCKET/bin/$bin` and misses those objects. `"/"` is
    # the documented sentinel meaning "no extra folder" — outputs
    # land directly at `bin/<file>`. (Codex #228 P1.)
    name                   = "/"
    namespace_type         = "NONE"
    override_artifact_name = true
  }

  environment {
    compute_type                = "BUILD_GENERAL1_LARGE"
    image                       = "aws/codebuild/amazonlinux2-aarch64-standard:3.0"
    type                        = "ARM_CONTAINER"
    image_pull_credentials_type = "CODEBUILD"
    privileged_mode             = false
  }

  source {
    type      = "S3"
    location  = "${aws_s3_bucket.artifacts.id}/sources/gsx-dag.zip"
    buildspec = "scripts/testnet/buildspec.yml"
  }

  cache {
    type     = "S3"
    location = "${aws_s3_bucket.artifacts.id}/cache/codebuild"
  }

  logs_config {
    cloudwatch_logs {
      group_name  = "/aws/codebuild/gsx-testnet-build"
      stream_name = "main"
    }
  }
}

output "codebuild_project" {
  description = "Name of the CodeBuild project that compiles the testnet validator binaries."
  value       = aws_codebuild_project.build.name
}
