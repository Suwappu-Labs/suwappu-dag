---
description: Drive a sprint from spec to PR (CLAUDE.md §Workflow)
argument-hint: <sprint-id, e.g. S21>
---

Drive sprint $ARGUMENTS end to end, following the standard flow in
CLAUDE.md §Workflow:

1. Read the sprint spec: the CLAUDE.md sprint backlog row, any
   `gh issue list --label sprint-<n>` issues, and the reference design in
   `suwappu-papers/papers/dag-l1` sections the row cites.
2. Write a step-by-step plan (crates touched, invariants at risk, exit
   gate property tests) and get the human's approval before implementing.
3. Implement step by step. Record in-flight state in `.sprint-state.md`
   at the branch root as you go.
4. Verify with `/check`; for sprint exit gates run `/check-10k`
   (10k proptest cases, `--release`).
5. Invoke the specialist subagents required by CLAUDE.md §Specialist
   subagents for every sensitive surface touched.
6. Prepare the PR with `gh pr create` (ask tier — the human confirms).
   Body must include the exit gate test command and subagent verdicts.

Never weaken a load-bearing invariant (CLAUDE.md §Load-bearing
invariants). Never rebase. Branch naming: `<scope>/<short-slug>`.
