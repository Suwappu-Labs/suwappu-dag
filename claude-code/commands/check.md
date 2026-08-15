---
description: Run the full local verification gate (fmt, clippy, tests, crypto boundary, cargo-deny)
---

Run `./scripts/check.sh` and report the result.

This is the same set of gates CI enforces: `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace --all-targets`,
`./scripts/check-crypto-boundary.sh`, and `cargo deny check`.

Caveat from CLAUDE.md §Local development: on machines where
workspace-wide cargo commands are off-limits (the usual dev Mac), do NOT
run this locally — push to the feature branch and let the GHA CI matrix
validate instead. Use your judgment about the machine you are on; when
in doubt, push and watch CI.
