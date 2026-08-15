---
description: Run all property tests at 10,000 cases in release mode (sprint exit gate)
---

Run `./scripts/check-10k.sh` (equivalent to
`PROPTEST_CASES=10000 cargo test --workspace --release`).

This is the sprint exit-gate bar: every sprint in the CLAUDE.md backlog
closes with its properties at 10k cases. Expect a long run; do not
cancel it for slowness. The same local-machine caveat as `/check`
applies — on constrained machines, push and let CI + a manual dispatch
handle it.
