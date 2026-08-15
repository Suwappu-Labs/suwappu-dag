#!/usr/bin/env bash
# PostToolUse hook (Edit|Write|MultiEdit): rustfmt drift hint.
#
# Documented in CLAUDE.md §Hooks. Never blocks — it prints a hint so the
# agent runs `cargo fmt -p <crate>` before pushing (rustfmt CI failures
# are the most common trivial red). Uses rustfmt directly on the edited
# file, honoring rustfmt.toml at the repo root, so it stays cheap even
# on machines where workspace-wide cargo commands are off-limits.

set -u

payload="$(cat)"

file_path="$(printf '%s' "$payload" | python3 -c '
import json, sys
try:
    data = json.load(sys.stdin)
    print(data.get("tool_input", {}).get("file_path", ""))
except Exception:
    pass
')"

case "$file_path" in
    *.rs) ;;
    *) exit 0 ;;
esac

[ -f "$file_path" ] || exit 0
command -v rustfmt >/dev/null 2>&1 || exit 0

if ! rustfmt --check --edition 2021 "$file_path" >/dev/null 2>&1; then
    echo "rustfmt drift in ${file_path} — run \`cargo fmt -p <crate>\` before pushing (CI enforces cargo fmt --all --check)."
fi

exit 0
