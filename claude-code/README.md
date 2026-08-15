# claude-code/ — agent infrastructure

Tracked home for the Claude Code configuration documented in
`CLAUDE.md` (§Specialist subagents, §Slash commands, §Permissions,
§Hooks). The `.claude/` directory is gitignored, so the tracked source
of truth lives here and gets wired into a session by symlink.

## Layout

| Path | What it is |
|---|---|
| `agents/` | Specialist subagent definitions (crypto/consensus/fastpath/transport/lane reviewers) |
| `commands/` | Slash commands: `/sprint`, `/check`, `/check-10k`, `/release`, `/aws-status`, `/iq-decision` |
| `hooks/` | `fmt-check.sh` (PostToolUse rustfmt hint), `block-destructive.sh` (PreToolUse security floor) |
| `settings.json` | Three permission tiers (allow / ask / deny) + hook wiring |

## Setup (once per clone)

```bash
mkdir -p .claude
ln -sf ../claude-code/settings.json .claude/settings.json
ln -sf ../claude-code/agents .claude/agents
ln -sf ../claude-code/commands .claude/commands
```

Hook scripts are referenced by repo-relative path from `settings.json`,
so no extra wiring is needed for them.

## Rules

- The `deny` tier in `settings.json` is the security floor: add to it,
  never remove entries without explicit security review.
- Keep `CLAUDE.md` and this directory in sync — a command or hook that
  exists in only one of the two places is a bug (this directory is the
  executable half, `CLAUDE.md` is the documentation half).
