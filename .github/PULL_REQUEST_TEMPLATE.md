<!--
Thanks for contributing to gsx-dag. Fill in the sections below — none of
them are throw-away, every line shapes how this PR gets reviewed.

If this is a draft / WIP, mark it as a draft. Otherwise reviewers assume
you've covered the test plan and you're asking for merge.
-->

## Summary

<!-- One paragraph: what does this change do, and why does it need to
exist? Reference the sprint (DAG-S*, G*, Track B/G/I) or the issue # if
applicable. -->

## What changed

<!-- 2–6 bullets describing the surface changes. Be specific:
"add Intent::Foo + apply_intent arm + bytes_state registry" beats
"refactor execution". -->

-
-
-

## Test plan

<!-- Mandatory. Reviewers will not merge without this. Pick the form
that matches the change:

- Unit / integration: name the new tests + how to run them.
- Property tests: how many cases (default ≥ 10k for exit-gate sprints).
- Manual: exact curl / cargo command + expected output.
- Behavior preservation: name the existing tests that exercise the
  refactored code path. -->

- [ ]
- [ ]

## Invariants touched

<!-- Optional, but required if this PR could affect:
  - Theorem 2 safety (joint-quorum AND-gate)
  - PQ crypto surface (ML-DSA-65 / ML-KEM-768)
  - LTP constant-size commitment (≈1,600 B)
  - Substrate invariants inherited from gsx-db
  - Fast-path equivocation slashing (100% bond forfeiture)
  - Reserved-address invariant (only protocol-owned arms mutate them)
  - State-root atomicity (all-or-nothing on failure)

Say which invariant + how this PR preserves it (or — if relaxing —
explain why and which test pins the new shape). -->

## Subagent verdicts

<!-- For PRs touching gsx-crypto / gsx-consensus / gsx-fastpath /
gsx-transport / joint-quorum logic, paste the specialist-subagent
verdicts from CLAUDE.md§Specialist-subagents. Skip otherwise. -->

## Closes

<!-- `Closes #N` to auto-close an issue, or `Refs #N` if related. -->
