#!/usr/bin/env bash
# Native-compile gsx-node / gsx-loadgen / gsx-metrics to
# aarch64-unknown-linux-gnu via AWS CodeBuild (ARM image).
#
# Why CodeBuild and not `cross`? Repo convention is to avoid Docker Desktop
# for cross-platform builds; CodeBuild runs in AWS (image pulled from the
# AWS-managed ARM registry, not Docker Hub) and the resulting binaries land
# directly in s3://<artifact_bucket>/bin/.
#
# Prereqs (one-time):
#   1. terraform/testnet applied (creates the artifact bucket + codebuild project)
#   2. The gsx-db deploy key already lives in /gsx-perf/gsx-db-deploy-key
#      (testnet reuses it — same gsx-db repo, same key).
#
# Output: target/testnet/{gsx-node,gsx-loadgen,gsx-metrics}, pulled from S3.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TF="$ROOT/terraform/testnet"
OUT="$ROOT/target/testnet"

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
  echo "error: terraform/testnet has not been applied yet." >&2
  echo "  scripts/testnet/deploy.sh apply" >&2
  exit 1
fi
PROJECT="$(cd "$TF" && terraform output -raw codebuild_project)"

echo "[build] artifact bucket: $BUCKET"
echo "[build] codebuild project: $PROJECT"

mkdir -p "$OUT"
SOURCE_ZIP="$OUT/gsx-dag.zip"
echo "[build] packaging source -> $SOURCE_ZIP"
cd "$ROOT"
rm -f "$SOURCE_ZIP"
git archive --format=zip --prefix= -o "$SOURCE_ZIP" HEAD

echo "[build] uploading source to s3://$BUCKET/sources/gsx-dag.zip"
aws s3 cp "$SOURCE_ZIP" "s3://$BUCKET/sources/gsx-dag.zip" --region us-east-1

echo "[build] starting CodeBuild project $PROJECT"
BUILD_ID="$(aws codebuild start-build \
  --project-name "$PROJECT" \
  --region us-east-1 \
  --query 'build.id' --output text)"
echo "[build] build id: $BUILD_ID"
echo "[build] cloudwatch logs: aws logs tail /aws/codebuild/$PROJECT --follow --region us-east-1"

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
    IN_PROGRESS) echo "[build] in progress..."; sleep 15 ;;
    *) echo "[build] unknown status: $STATUS"; sleep 15 ;;
  esac
done

echo "[build] pulling binaries from s3://$BUCKET/bin/"
for bin in gsx-node gsx-loadgen gsx-metrics; do
  aws s3 cp "s3://$BUCKET/bin/$bin" "$OUT/$bin" --region us-east-1
  chmod +x "$OUT/$bin"
done
ls -lh "$OUT"
echo "[build] done"
