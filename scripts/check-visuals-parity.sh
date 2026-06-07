#!/usr/bin/env bash
# check-visuals-parity.sh — diff the cross-repo-shared visuals between
# suwappu-dag and suwappu-lattice-protocol.
#
# Background: a small subset of docs/visuals/ is intentionally bit-identically
# mirrored between suwappu-dag and suwappu-lattice-protocol so the LTP repo can render
# the core stack diagrams without a cross-repo include. Canonical home: suwappu-dag.
#
# Each repo additionally carries its own diagrams (suwappu-dag has the consensus
# deep-dives like commit-rule.md / governance-flow.md; LTP has dkg-ceremony.md,
# corridor-quorum.md, etc.). Those are NOT checked here — only the explicit
# `SHARED_FILES` allow-list is.
#
# Exit codes:
#   0  → no drift in the shared set OR peer repo not present locally (so the
#        script is safe to run on CI where only one repo is checked out).
#   1  → drift detected in the shared set; per-file summary printed to stderr.
#   2  → script invocation error.
#
# Usage:
#   ./scripts/check-visuals-parity.sh [PEER_REPO_ROOT]
#
# If PEER_REPO_ROOT is omitted, default is `../suwappu-lattice-protocol` relative
# to the repo root (matches the user's `~/suwappu-build/` checkout layout).
#
# See docs/visuals/SOURCE-OF-TRUTH.md for the duplication policy + the
# rationale for the allow-list shape.

set -euo pipefail

# Files that MUST be bit-identical across both repos. Add to this list when a
# new diagram crosses the suwappu-dag / LTP boundary (i.e., when LTP needs to
# render it offline without resolving a relative path back to suwappu-dag).
SHARED_FILES=(
  "mermaid/suwappu-dag.md"
  "mermaid/suwappu-db.md"
  "mermaid/ltp.md"
  "suwappu-dag.html"
  "suwappu-db.html"
  "ltp.html"
  "suwappu-ecosystem-atlas.html"
  "index.html"
)

# Resolve the repo root the script lives in (so we can be invoked from any cwd).
script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd "$script_dir/.." && pwd)

our_visuals="$repo_root/docs/visuals"
peer_root="${1:-$repo_root/../suwappu-lattice-protocol}"
peer_visuals="$peer_root/docs/visuals"

if [[ ! -d "$our_visuals" ]]; then
  echo "fatal: no docs/visuals/ in $repo_root" >&2
  exit 2
fi

if [[ ! -d "$peer_visuals" ]]; then
  echo "info: peer repo not present at $peer_root — skipping parity check." >&2
  exit 0
fi

drift_found=0
drift_summary=()

for relpath in "${SHARED_FILES[@]}"; do
  our_file="$our_visuals/$relpath"
  peer_file="$peer_visuals/$relpath"

  if [[ ! -f "$our_file" ]]; then
    drift_summary+=("missing in our repo: $relpath")
    drift_found=1
    continue
  fi

  if [[ ! -f "$peer_file" ]]; then
    drift_summary+=("missing in peer: $relpath")
    drift_found=1
    continue
  fi

  if ! diff -q "$our_file" "$peer_file" > /dev/null 2>&1; then
    drift_summary+=("drift: $relpath")
    drift_found=1
  fi
done

if [[ $drift_found -eq 0 ]]; then
  echo "ok: shared docs/visuals/ files match between $repo_root and $peer_root" >&2
  exit 0
fi

echo "DRIFT detected between $repo_root/docs/visuals/ and $peer_visuals/" >&2
echo >&2
for line in "${drift_summary[@]}"; do
  echo "  $line" >&2
done
echo >&2
echo "Shared set: ${SHARED_FILES[*]}" >&2
echo "See docs/visuals/SOURCE-OF-TRUTH.md for the duplication policy." >&2
echo "Canonical home is suwappu-dag; edit there first, mirror to suwappu-lattice-protocol." >&2

exit 1
