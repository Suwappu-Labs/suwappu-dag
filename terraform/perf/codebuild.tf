# AWS CodeBuild project for the musl cross-compile.
#
# Avoids using Docker Desktop locally for cross-platform builds (per repo
# convention). Source is uploaded to s3://artifact_bucket/sources/gsx-dag.zip
# before each invocation by scripts/perf/build.sh; CodeBuild pulls it, runs
# `cargo build --target x86_64-unknown-linux-musl`, drops the binaries to
# s3://artifact_bucket/bin/.
#
# Image is pulled from public.ecr.aws (Amazon's ECR public registry), not
# Docker Hub — avoids the Docker Hub rate-limit class of failure entirely.

resource "aws_iam_role" "codebuild" {
  provider = aws.us_east_1
  name     = "gsx-perf-codebuild"

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
  name     = "perf-codebuild"
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

resource "aws_codebuild_project" "musl" {
  provider     = aws.us_east_1
  name         = "gsx-perf-musl-build"
  description  = "Cross-compile gsx-node binaries to x86_64-unknown-linux-musl for the perf testnet"
  service_role = aws_iam_role.codebuild.arn

  build_timeout = 30

  artifacts {
    type      = "S3"
    location  = aws_s3_bucket.artifacts.id
    path      = "bin"
    packaging = "NONE"
    name      = "."
    # Important: we want files written directly under bin/ (not bin/<build-id>/).
    namespace_type         = "NONE"
    override_artifact_name = true
  }

  environment {
    compute_type                = "BUILD_GENERAL1_LARGE"
    # Ubuntu-based standard image: Amazon Linux 2 lacks musl-tools in its
    # default repos and adding EPEL adds complexity. Standard:7.0 has
    # musl-tools, build-essential, etc. via apt and is also pulled from
    # public.ecr.aws (not Docker Hub).
    image = "public.ecr.aws/codebuild/standard:7.0"
    type                        = "LINUX_CONTAINER"
    image_pull_credentials_type = "CODEBUILD"
    privileged_mode             = false
  }

  source {
    type      = "S3"
    location  = "${aws_s3_bucket.artifacts.id}/sources/gsx-dag.zip"
    buildspec = "scripts/perf/buildspec.yml"
  }

  cache {
    type     = "S3"
    location = "${aws_s3_bucket.artifacts.id}/cache/codebuild"
  }

  logs_config {
    cloudwatch_logs {
      group_name  = "/aws/codebuild/gsx-perf-musl-build"
      stream_name = "main"
    }
  }
}
