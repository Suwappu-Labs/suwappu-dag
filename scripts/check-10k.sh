#!/usr/bin/env bash
# Sprint exit-gate run — proptests at 10,000 cases each, release mode.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

echo "==> 10,000-case proptest sweep (release mode)"
PROPTEST_CASES=10000 cargo test --workspace --release

echo "==> exit gates green"
