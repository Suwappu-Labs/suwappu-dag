#!/usr/bin/env bash
# PreToolUse hook (Bash): pattern-block destructive commands.
#
# Documented in CLAUDE.md §Hooks. Second line of defense behind the
# settings.json deny list — catches destructive commands even when they
# arrive embedded in compound invocations that pattern-match past the
# permission rules. Exit 2 blocks the tool call and feeds stderr back to
# the agent.

set -u

payload="$(cat)"

command_text="$(printf '%s' "$payload" | python3 -c '
import json, sys
try:
    data = json.load(sys.stdin)
    print(data.get("tool_input", {}).get("command", ""))
except Exception:
    pass
')"

[ -n "$command_text" ] || exit 0

block() {
    echo "BLOCKED by claude-code/hooks/block-destructive.sh: $1" >&2
    echo "This is the repo security floor (CLAUDE.md §Permissions/§Hooks). If genuinely needed, a human runs it by hand." >&2
    exit 2
}

case "$command_text" in
    *"rm -rf /"*|*"rm -fr /"*)          block "recursive delete from filesystem root" ;;
esac

printf '%s' "$command_text" | grep -qE 'git push[^|;&]*(--force|-f)([^-a-z]|$)' \
    && block "git push --force (force pushes are denied; history is commit-topology sensitive)"

printf '%s' "$command_text" | grep -qE 'git rebase' \
    && block "git rebase (repo invariant 6: never rebase — use git merge or git pull --no-rebase)"

printf '%s' "$command_text" | grep -qE 'terraform +(destroy|apply)' \
    && block "raw terraform destroy/apply (use ./scripts/deploy-aws.sh; destroy needs a human)"

printf '%s' "$command_text" | grep -qE 'aws +ec2 +terminate-instances' \
    && block "aws ec2 terminate-instances"

exit 0
