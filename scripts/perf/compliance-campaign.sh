#!/usr/bin/env bash
# DAG-S26.6 — end-to-end bank-compliance campaign orchestrator.
#
# Assumes:
#  - suwappu-loadgen.service is already running on one host (DAG-S26.3)
#    so the cluster has sustained load. If not, the campaign will
#    still complete but TPS and e2e metrics will be empty.
#  - Validators in all configured regions are reachable via SSM.
#  - AWS_PROFILE=gsn or equivalent is set in the environment.
#
# Steps:
#   1. Record start_ms.
#   2. Wait $DURATION seconds.
#   3. Record end_ms.
#   4. Collect events.ndjson from every validator via SSM (parallel).
#   5. Collect loadgen.csv from the loadgen host.
#   6. Run suwappu-metrics in each mode (cert/e2e/pair/tps/recovery).
#   7. Run report.py to produce report.json + report.html.
#   8. Upload the campaign artifacts to S3.
#
# Usage:
#   AWS_PROFILE=gsn ./scripts/perf/compliance-campaign.sh --duration 600
#
# Output:
#   /tmp/campaign-<id>/{events/<region>.ndjson, loadgen.csv,
#                        cert.csv, e2e.csv, pair.csv, tps.csv,
#                        recovery.csv, meta.json, report.json,
#                        report.html}
#   s3://suwappu-dag-perf-artifacts/reports/<id>/

set -euo pipefail

DURATION_S=600
OUTPUT_BASE="${TMPDIR:-/tmp}"
LOADGEN_HOST_REGION="us-east-1"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --duration) DURATION_S="$2"; shift 2 ;;
    --output-base) OUTPUT_BASE="$2"; shift 2 ;;
    --loadgen-host-region) LOADGEN_HOST_REGION="$2"; shift 2 ;;
    *) echo "unknown arg: $1" >&2; exit 1 ;;
  esac
done

CAMPAIGN_ID="campaign-$(date -u +%Y%m%dT%H%M%SZ)"
CAMPAIGN_DIR="$OUTPUT_BASE/$CAMPAIGN_ID"
mkdir -p "$CAMPAIGN_DIR/events"

# Validator inventory — same shape as ssm-redeploy targets.
# (region:instance_id pairs.) Edit if the testnet topology changes.
declare -a VALIDATORS=(
  "us-east-1:i-09ec6b896639cadac"
  "us-west-2:i-06c0063e7fd26a14e"
  "eu-west-1:i-08d7f82fd9b70bf48"
  "ap-northeast-1:i-046500481231aa287"
)

REGIONS=()
for v in "${VALIDATORS[@]}"; do REGIONS+=("${v%%:*}"); done

# Use python3 for millisecond epoch — BSD `date` on macOS doesn't
# support `%3N` (GNU coreutils only). python3 is always present.
START_MS=$(python3 -c 'import time; print(int(time.time() * 1000))')
echo "[$(date -u +%FT%TZ)] campaign $CAMPAIGN_ID — observing $DURATION_S s on ${#VALIDATORS[@]} validators"

sleep "$DURATION_S"

END_MS=$(python3 -c 'import time; print(int(time.time() * 1000))')
echo "[$(date -u +%FT%TZ)] observation window closed"

# Step 4–5: collect logs from each validator via S3 push (DAG-S28.1).
# SSM `StandardOutputContent` truncates at 24,576 bytes per command —
# pre-S28 this silently dropped multi-MB event logs to ~130 lines per
# region. The validator EC2 IAM role already has `s3:PutObject` on the
# `logs/` prefix of the artifact bucket (terraform/perf/modules/region/
# main.tf:165-172), so we instruct each validator to gzip its events
# file and `aws s3 cp` it into `s3://<bucket>/logs/<campaign>/<region>.
# ndjson.gz`, then download locally from S3 with no truncation.
BUCKET="${ARTIFACT_BUCKET:-suwappu-dag-perf-artifacts}"
S3_PREFIX="logs/$CAMPAIGN_ID"
echo "[$(date -u +%FT%TZ)] collecting events.ndjson via S3 push -> s3://$BUCKET/$S3_PREFIX/"

# Use parallel indexed arrays (region:cmd_id pairs) instead of an
# associative array — Mac default bash is 3.2 and lacks `declare -A`.
SSM_CMDS_PAIRS=()
for v in "${VALIDATORS[@]}"; do
  region="${v%%:*}"
  iid="${v##*:}"
  cmd_id=$(aws ssm send-command \
    --region "$region" \
    --instance-ids "$iid" \
    --document-name AWS-RunShellScript \
    --parameters "commands=[\"set -e\",\"sudo cp /var/log/suwappu/events.ndjson /tmp/e-$CAMPAIGN_ID.ndjson\",\"sudo chmod 644 /tmp/e-$CAMPAIGN_ID.ndjson\",\"gzip -f /tmp/e-$CAMPAIGN_ID.ndjson\",\"aws s3 cp /tmp/e-$CAMPAIGN_ID.ndjson.gz s3://$BUCKET/$S3_PREFIX/$region.ndjson.gz --region us-east-1\",\"rm -f /tmp/e-$CAMPAIGN_ID.ndjson.gz\",\"echo uploaded $region\"]" \
    --query Command.CommandId --output text)
  SSM_CMDS_PAIRS+=("$region:$cmd_id")
done

for v in "${VALIDATORS[@]}"; do
  region="${v%%:*}"
  iid="${v##*:}"
  cmd_id=""
  for pair in "${SSM_CMDS_PAIRS[@]}"; do
    if [[ "${pair%%:*}" == "$region" ]]; then cmd_id="${pair#*:}"; break; fi
  done
  until aws ssm get-command-invocation \
          --region "$region" --command-id "$cmd_id" --instance-id "$iid" \
          --query Status --output text 2>/dev/null \
          | grep -qE "Success|Failed|Cancelled|TimedOut"; do
    sleep 2
  done
  status=$(aws ssm get-command-invocation \
      --region "$region" --command-id "$cmd_id" --instance-id "$iid" \
      --query Status --output text)
  if [[ "$status" != "Success" ]]; then
    echo "  $region: upload FAILED ($status)"
    aws ssm get-command-invocation \
        --region "$region" --command-id "$cmd_id" --instance-id "$iid" \
        --query StandardErrorContent --output text >&2
    continue
  fi
  # Pull the gzipped log from S3 to the local campaign dir.
  aws s3 cp "s3://$BUCKET/$S3_PREFIX/$region.ndjson.gz" \
      "$CAMPAIGN_DIR/events/$region.ndjson.gz" --region us-east-1 --quiet
  gunzip -f "$CAMPAIGN_DIR/events/$region.ndjson.gz"
  echo "  $region: $(wc -l < "$CAMPAIGN_DIR/events/$region.ndjson") events"
done

# loadgen.csv from the designated loadgen host (us-east-1 by default).
LOADGEN_IID=""
for v in "${VALIDATORS[@]}"; do
  if [[ "${v%%:*}" == "$LOADGEN_HOST_REGION" ]]; then
    LOADGEN_IID="${v##*:}"
  fi
done
if [[ -n "$LOADGEN_IID" ]]; then
  cmd_id=$(aws ssm send-command \
    --region "$LOADGEN_HOST_REGION" \
    --instance-ids "$LOADGEN_IID" \
    --document-name AWS-RunShellScript \
    --parameters "commands=[\"set -e\",\"if [ -f /var/log/suwappu/loadgen.csv ]; then sudo cp /var/log/suwappu/loadgen.csv /tmp/lg-$CAMPAIGN_ID.csv; sudo chmod 644 /tmp/lg-$CAMPAIGN_ID.csv; gzip -f /tmp/lg-$CAMPAIGN_ID.csv; aws s3 cp /tmp/lg-$CAMPAIGN_ID.csv.gz s3://$BUCKET/$S3_PREFIX/loadgen.csv.gz --region us-east-1; rm -f /tmp/lg-$CAMPAIGN_ID.csv.gz; echo uploaded loadgen; else echo no loadgen.csv; fi\"]" \
    --query Command.CommandId --output text)
  until aws ssm get-command-invocation \
          --region "$LOADGEN_HOST_REGION" --command-id "$cmd_id" --instance-id "$LOADGEN_IID" \
          --query Status --output text 2>/dev/null \
          | grep -qE "Success|Failed|Cancelled|TimedOut"; do
    sleep 2
  done
  if aws s3 ls "s3://$BUCKET/$S3_PREFIX/loadgen.csv.gz" --region us-east-1 >/dev/null 2>&1; then
    aws s3 cp "s3://$BUCKET/$S3_PREFIX/loadgen.csv.gz" \
        "$CAMPAIGN_DIR/loadgen.csv.gz" --region us-east-1 --quiet
    gunzip -f "$CAMPAIGN_DIR/loadgen.csv.gz"
    echo "  loadgen.csv: $(wc -l < "$CAMPAIGN_DIR/loadgen.csv") rows"
  else
    echo "  loadgen.csv: (not found on loadgen host)"
    echo "client_submitted_ms,tx_hash,target_idx" > "$CAMPAIGN_DIR/loadgen.csv"
  fi
fi

# Step 6: run suwappu-metrics in each mode (DAG-S28.4 — resolve in order:
#   1. $SUWAPPU_METRICS_BIN env var if explicitly set
#   2. ./target/release/suwappu-metrics relative to repo root
#   3. on-demand `cargo build --release -p suwappu-node --bin suwappu-metrics`
#   4. PATH fallback (CI containers / docker)
ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
if [[ -n "${SUWAPPU_METRICS_BIN:-}" ]] && [[ -x "$SUWAPPU_METRICS_BIN" ]]; then
  METRICS="$SUWAPPU_METRICS_BIN"
elif [[ -x "$ROOT_DIR/target/release/suwappu-metrics" ]]; then
  METRICS="$ROOT_DIR/target/release/suwappu-metrics"
elif command -v cargo >/dev/null 2>&1; then
  echo "[$(date -u +%FT%TZ)] building suwappu-metrics (native, one-shot)"
  ( cd "$ROOT_DIR" && cargo build --release -p suwappu-node --bin suwappu-metrics 2>&1 | tail -2 )
  METRICS="$ROOT_DIR/target/release/suwappu-metrics"
elif command -v suwappu-metrics >/dev/null 2>&1; then
  METRICS="suwappu-metrics"
else
  echo "error: suwappu-metrics not found. Set SUWAPPU_METRICS_BIN=/path/to/suwappu-metrics," >&2
  echo "       run \`cargo build --release -p suwappu-node --bin suwappu-metrics\` first," >&2
  echo "       or run this script from a container with suwappu-metrics on PATH." >&2
  exit 1
fi
echo "[$(date -u +%FT%TZ)] suwappu-metrics: $METRICS"

LOG_ARGS=()
for r in "${REGIONS[@]}"; do
  LOG_ARGS+=(--logs "$r=$CAMPAIGN_DIR/events/$r.ndjson")
done

echo "[$(date -u +%FT%TZ)] running suwappu-metrics in 5 modes"
"$METRICS" "${LOG_ARGS[@]}" --mode cert > "$CAMPAIGN_DIR/cert.csv"
"$METRICS" "${LOG_ARGS[@]}" --mode pair > "$CAMPAIGN_DIR/pair.csv"
"$METRICS" "${LOG_ARGS[@]}" --mode tps  > "$CAMPAIGN_DIR/tps.csv"
"$METRICS" "${LOG_ARGS[@]}" --mode recovery > "$CAMPAIGN_DIR/recovery.csv"

if [[ -s "$CAMPAIGN_DIR/loadgen.csv" ]]; then
  # DAG-S30.4: pass the campaign window so suwappu-metrics e2e can drop
  # stale committed events from prior runs that share intent hashes.
  "$METRICS" "${LOG_ARGS[@]}" --mode e2e \
      --loadgen-csv "$CAMPAIGN_DIR/loadgen.csv" \
      --campaign-start-ms "$START_MS" \
      --campaign-end-ms "$END_MS" \
      > "$CAMPAIGN_DIR/e2e.csv"
else
  echo "  skipping e2e — no loadgen.csv"
  echo "tx_hash,submitted_ms,region,first_committed_ms,e2e_latency_ms" > "$CAMPAIGN_DIR/e2e.csv"
fi

# Step 7: report.json + report.html.
cat > "$CAMPAIGN_DIR/meta.json" <<JSON
{
  "id": "$CAMPAIGN_ID",
  "start_ms": $START_MS,
  "end_ms": $END_MS,
  "regions": [$(printf '"%s",' "${REGIONS[@]}" | sed 's/,$//')] ,
  "binary_version": "$(date -u +%Y%m%dT%H%M%SZ)"
}
JSON

REPORT_PY="$(dirname "$0")/report.py"
echo "[$(date -u +%FT%TZ)] generating report.json + report.html"
python3 "$REPORT_PY" --input-dir "$CAMPAIGN_DIR" --output-dir "$CAMPAIGN_DIR"

# Step 8: upload to S3.
if [[ -n "${SKIP_S3_UPLOAD:-}" ]]; then
  echo "[$(date -u +%FT%TZ)] SKIP_S3_UPLOAD set — local artifacts only"
else
  echo "[$(date -u +%FT%TZ)] uploading to s3://suwappu-dag-perf-artifacts/reports/$CAMPAIGN_ID/"
  aws s3 cp --recursive "$CAMPAIGN_DIR" "s3://suwappu-dag-perf-artifacts/reports/$CAMPAIGN_ID/" \
    --region us-east-1 \
    || echo "  (s3 upload failed — local artifacts still at $CAMPAIGN_DIR)"
fi

echo "[$(date -u +%FT%TZ)] campaign $CAMPAIGN_ID complete"
echo "  local:  $CAMPAIGN_DIR"
echo "  s3:     s3://suwappu-dag-perf-artifacts/reports/$CAMPAIGN_ID/"
echo "  open:   $CAMPAIGN_DIR/report.html"
