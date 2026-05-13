#!/usr/bin/env bash
# DAG-S26.6 — end-to-end bank-compliance campaign orchestrator.
#
# Assumes:
#  - gsx-loadgen.service is already running on one host (DAG-S26.3)
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
#   6. Run gsx-metrics in each mode (cert/e2e/pair/tps/recovery).
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
#   s3://gsx-dag-perf-artifacts/reports/<id>/

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

START_MS=$(date +%s%3N)
echo "[$(date -u +%FT%TZ)] campaign $CAMPAIGN_ID — observing $DURATION_S s on ${#VALIDATORS[@]} validators"

sleep "$DURATION_S"

END_MS=$(date +%s%3N)
echo "[$(date -u +%FT%TZ)] observation window closed"

# Step 4–5: collect logs from each validator in parallel via SSM.
echo "[$(date -u +%FT%TZ)] collecting events.ndjson + loadgen.csv via SSM"
declare -A SSM_CMDS
for v in "${VALIDATORS[@]}"; do
  region="${v%%:*}"
  iid="${v##*:}"
  cmd_id=$(aws ssm send-command \
    --region "$region" \
    --instance-ids "$iid" \
    --document-name AWS-RunShellScript \
    --parameters 'commands=["cat /var/log/gsx/events.ndjson"]' \
    --query Command.CommandId --output text)
  SSM_CMDS["$region"]="$cmd_id"
done

for v in "${VALIDATORS[@]}"; do
  region="${v%%:*}"
  iid="${v##*:}"
  cmd_id="${SSM_CMDS[$region]}"
  # Wait for completion, then dump StandardOutputContent to events file.
  until aws ssm get-command-invocation \
          --region "$region" --command-id "$cmd_id" --instance-id "$iid" \
          --query Status --output text 2>/dev/null \
          | grep -q "Success\|Failed"; do
    sleep 2
  done
  aws ssm get-command-invocation \
      --region "$region" --command-id "$cmd_id" --instance-id "$iid" \
      --query StandardOutputContent --output text \
      > "$CAMPAIGN_DIR/events/$region.ndjson"
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
    --parameters 'commands=["cat /var/log/gsx/loadgen.csv 2>/dev/null || true"]' \
    --query Command.CommandId --output text)
  until aws ssm get-command-invocation \
          --region "$LOADGEN_HOST_REGION" --command-id "$cmd_id" --instance-id "$LOADGEN_IID" \
          --query Status --output text 2>/dev/null \
          | grep -q "Success\|Failed"; do
    sleep 2
  done
  aws ssm get-command-invocation \
      --region "$LOADGEN_HOST_REGION" --command-id "$cmd_id" --instance-id "$LOADGEN_IID" \
      --query StandardOutputContent --output text \
      > "$CAMPAIGN_DIR/loadgen.csv"
  echo "  loadgen.csv: $(wc -l < "$CAMPAIGN_DIR/loadgen.csv") rows"
fi

# Step 6: run gsx-metrics in each mode. Assumes gsx-metrics is built
# in ./target/release or available on PATH.
METRICS="${GSX_METRICS_BIN:-gsx-metrics}"

LOG_ARGS=()
for r in "${REGIONS[@]}"; do
  LOG_ARGS+=(--logs "$r=$CAMPAIGN_DIR/events/$r.ndjson")
done

echo "[$(date -u +%FT%TZ)] running gsx-metrics in 5 modes"
"$METRICS" "${LOG_ARGS[@]}" --mode cert > "$CAMPAIGN_DIR/cert.csv"
"$METRICS" "${LOG_ARGS[@]}" --mode pair > "$CAMPAIGN_DIR/pair.csv"
"$METRICS" "${LOG_ARGS[@]}" --mode tps  > "$CAMPAIGN_DIR/tps.csv"
"$METRICS" "${LOG_ARGS[@]}" --mode recovery > "$CAMPAIGN_DIR/recovery.csv"

if [[ -s "$CAMPAIGN_DIR/loadgen.csv" ]]; then
  "$METRICS" "${LOG_ARGS[@]}" --mode e2e --loadgen-csv "$CAMPAIGN_DIR/loadgen.csv" > "$CAMPAIGN_DIR/e2e.csv"
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
  echo "[$(date -u +%FT%TZ)] uploading to s3://gsx-dag-perf-artifacts/reports/$CAMPAIGN_ID/"
  aws s3 cp --recursive "$CAMPAIGN_DIR" "s3://gsx-dag-perf-artifacts/reports/$CAMPAIGN_ID/" \
    --region us-east-1 \
    || echo "  (s3 upload failed — local artifacts still at $CAMPAIGN_DIR)"
fi

echo "[$(date -u +%FT%TZ)] campaign $CAMPAIGN_ID complete"
echo "  local:  $CAMPAIGN_DIR"
echo "  s3:     s3://gsx-dag-perf-artifacts/reports/$CAMPAIGN_ID/"
echo "  open:   $CAMPAIGN_DIR/report.html"
