#!/usr/bin/env bash
# Cross-compile gsx-node / gsx-loadgen / gsx-metrics to
# x86_64-unknown-linux-musl via AWS CodeBuild.
#
# Why CodeBuild and not `cross`? Repo convention is to avoid Docker Desktop
# for cross-platform builds; CodeBuild runs in AWS (image pulled from
# public.ecr.aws, not Docker Hub) and the resulting binaries land directly
# in s3://<artifact_bucket>/bin/.
#
# Prereqs (one-time):
#   1. terraform/perf has been applied (creates the artifact bucket + project)
#   2. The gsx-db deploy key is uploaded to SSM:
#        aws ssm put-parameter --name /gsx-perf/gsx-db-deploy-key \
#          --type SecureString --value "$(cat ~/.ssh/gsx-db-deploy)" \
#          --profile gsn --region us-east-1
#
# Output: target/perf/{gsx-node,gsx-loadgen,gsx-metrics}, pulled from S3.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TF="$ROOT/terraform/perf"
OUT="$ROOT/target/perf"

export AWS_PROFILE=gsn

if ! command -v aws >/dev/null 2>&1; then
  echo "error: aws CLI required" >&2
  exit 1
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "error: jq required" >&2
  exit 1
fi

if ! BUCKET="$(cd "$TF" && terraform output -raw artifact_bucket 2>/dev/null)"; then
  echo "error: terraform/perf has not been applied yet." >&2
  echo "  scripts/deploy-aws.sh apply perf" >&2
  exit 1
fi
PROJECT="$(cd "$TF" && terraform output -raw codebuild_project)"

echo "[build] artifact bucket: $BUCKET"
echo "[build] codebuild project: $PROJECT"

# 1. Package the workspace (excluding target/, .git/, large irrelevant trees).
mkdir -p "$OUT"
SOURCE_ZIP="$OUT/gsx-dag.zip"
echo "[build] packaging source -> $SOURCE_ZIP"
cd "$ROOT"
rm -f "$SOURCE_ZIP"
# `git archive` produces a clean zip of HEAD (respects .gitignore, no target/).
git archive --format=zip --prefix= -o "$SOURCE_ZIP" HEAD

# 2. Upload to S3.
echo "[build] uploading source to s3://$BUCKET/sources/gsx-dag.zip"
aws s3 cp "$SOURCE_ZIP" "s3://$BUCKET/sources/gsx-dag.zip" --region us-east-1

# 3. Start the CodeBuild build and poll for completion.
echo "[build] starting CodeBuild project $PROJECT"
BUILD_ID="$(aws codebuild start-build \
  --project-name "$PROJECT" \
  --region us-east-1 \
  --query 'build.id' --output text)"
echo "[build] build id: $BUILD_ID"
echo "[build] cloudwatch logs: aws logs tail /aws/codebuild/$PROJECT --follow --region us-east-1"

# Poll every 10 seconds.
while true; do
  STATUS="$(aws codebuild batch-get-builds \
    --ids "$BUILD_ID" --region us-east-1 \
    --query 'builds[0].buildStatus' --output text)"
  case "$STATUS" in
    SUCCEEDED) echo "[build] succeeded"; break ;;
    FAILED|FAULT|TIMED_OUT|STOPPED)
      echo "[build] FAILED: $STATUS" >&2
      aws codebuild batch-get-builds --ids "$BUILD_ID" --region us-east-1 \
        --query 'builds[0].phases[?phaseStatus!=`SUCCEEDED`]' \
        --output json >&2
      exit 1
      ;;
    IN_PROGRESS) echo "[build] in progress..."; sleep 10 ;;
    *) echo "[build] unknown status: $STATUS"; sleep 10 ;;
  esac
done

# 4. Pull artifacts back to target/perf/.
echo "[build] pulling binaries from s3://$BUCKET/bin/"
for bin in gsx-node gsx-loadgen gsx-metrics; do
  aws s3 cp "s3://$BUCKET/bin/$bin" "$OUT/$bin" --region us-east-1
  chmod +x "$OUT/$bin"
done
ls -lh "$OUT"
echo "[build] done"
