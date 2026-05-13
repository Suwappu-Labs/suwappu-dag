#!/usr/bin/env bash
# Destroy the perf testnet — stops the EC2 meter.
#
# Thin wrapper around scripts/deploy-aws.sh destroy perf.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
"$ROOT/scripts/deploy-aws.sh" destroy perf
