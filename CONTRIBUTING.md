# Contributing to gsx-dag

Thanks for considering a contribution. This guide covers what we
need from external contributors before reviewing a PR.

## Before you start

- **Familiarize yourself with the codebase.**
  - [`README.md`](README.md) is the 5-minute tour.
  - [`DEVNET.md`](DEVNET.md) brings up a 4-node local cluster you can
    submit transactions to.
  - [`docs/README.md`](docs/README.md) is the full documentation index.
- **Check existing issues + PRs.** Search [GitHub issues](https://github.com/GlobalSettlementNetwork/gsx-dag/issues)
  to make sure the work isn't already in flight.
- **Open an issue first** for non-trivial changes. The
  [`bug_report`](.github/ISSUE_TEMPLATE/bug_report.md) and
  [`feature_request`](.github/ISSUE_TEMPLATE/feature_request.md)
  templates explain the shape we expect.

## License + sign-off

gsx-dag is licensed under **Apache 2.0**
([`LICENSE`](LICENSE)). Every commit must be signed off under the
[Developer Certificate of Origin](https://developercertificate.org/):

```
git commit -s -m "your message"
```

This adds a `Signed-off-by:` trailer that certifies you have the
right to submit the change under Apache 2.0. CI checks this on every
PR. We do **not** require a CLA.

## Workflow

1. **Fork** the repo and create a topic branch off `main`.
   - Branch naming: `<scope>/<short-slug>`. Examples:
     `consensus/iq-005-something`, `harden/scion-tls`,
     `extern/rust-sdk-docs`. See [`GSXHELPER.md`](GSXHELPER.md) for the
     full convention.
2. **Make focused commits.** One logical change per commit; imperative
   mood ("Add X" not "Added X"); reference issues via `Refs #N` or
   `Closes #N` in the body.
3. **Write tests.** New code paths need coverage. The exit-gate
   property tests under `crates/*/tests/proptest_*.rs` are the
   model; aim for the same density.
4. **Run the local checks** before pushing:
   ```sh
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings    # SEE BELOW
   cargo test  --workspace --all-targets                    # SEE BELOW
   cargo deny check
   ```
   On a low-RAM machine the workspace-wide commands can saturate;
   it's fine to run only the crates you touched. CI runs the full
   matrix.
5. **Push + open a PR.** Use the PR template (it auto-populates).
   Link the issue you're closing; describe the test strategy; flag
   anything load-bearing (consensus rule change, cryptographic
   surface change, ingress hardening change).
6. **Address review.** A specialist reviewer is assigned automatically
   based on which crate you touched — see [Specialist reviewers](#specialist-reviewers)
   below.
7. **Squash + merge** once approved. The reviewer will handle the
   merge; you don't need access to do it.

## What CI runs on your PR

- `rustfmt --check` — formatting.
- `cargo clippy --workspace --all-targets -- -D warnings` — lints.
- `cargo test --workspace --all-targets` — full test suite including
  proptests (default 256 cases; sprint exit gates run 10k).
- `cargo deny check` — license + advisory + source allowlist.
- `visuals-parity` (non-blocking) — verifies `docs/visuals/`
  bit-identity against gsx-lattice-protocol's mirror.
- Path-targeted: `ts-sdk` (npm typecheck + build + test) if you
  touched `clients/ts-sdk/`.
- Path-targeted (scheduled, not per-PR): `fuzz` runs three cargo-fuzz
  targets weekly on Sunday 03:00 UTC.

## Specialist reviewers

Some surfaces need extra scrutiny. The bot assigns reviewers
based on the files you touched:

| You touched | Reviewer |
|---|---|
| `crates/gsx-crypto/**` | `crypto-reviewer` |
| `crates/gsx-consensus/**` | `consensus-reviewer` |
| `crates/gsx-fastpath/**` | `fastpath-auditor` |
| `crates/gsx-transport/**` | `transport-auditor` |
| Joint-quorum logic | `consensus-reviewer` + `crypto-reviewer` |
| Anything else | maintainer rotation |

Reviewers may pull in an
[Investigation Question (IQ)](docs/iq/README.md) — a written
ratification document — before approving load-bearing changes. The
IQ process exists for changes that affect Theorem 2's safety
proof, fast-path equivocation semantics, the LTP attestation
surface, or the PQ cryptographic posture. Don't be surprised; it's
how we keep paper-vs-implementation in sync.

## Coding conventions

- **Rust:** stable channel pinned at `rust-version` in `Cargo.toml`
  (currently 1.78). No `#![allow(unsafe_code)]` without a written
  justification.
- **Default to no comments.** Well-named identifiers and short
  functions beat narration. Write a comment only when the WHY is
  non-obvious — a hidden constraint, a subtle invariant, or a
  workaround for a specific bug.
- **Proptests.** Sprint exit gates use 10,000 cases under
  `PROPTEST_CASES=10000 cargo test --release`. CI runs the default
  256 cases per PR. New invariants need both bounds documented.
- **Error handling.** Use `thiserror` at module boundaries;
  `anyhow::Result` is fine inside binaries. Don't `unwrap()` in
  production code paths.

## What's out of scope for external contributions (today)

- **Mainnet validator keys / genesis material.** The validator-set
  registry is governed by the IQ process + the existing operator
  team; external PRs that touch `terraform/perf/` configs or the
  genesis manifest will be politely closed.
- **Cryptographic primitive substitutions.** Replacing ML-DSA-65,
  ML-KEM-768, or BLS12-381 with another primitive needs a paper-side
  IQ first. Open an issue describing the case for the substitution
  before writing code.
- **Breaking wire-format changes.** The bincode/wire shape is
  load-bearing across many in-flight perf campaigns. Wire changes
  need an IQ + a coordinated rollout plan.

## Reporting a security issue

**Do not** open a public issue for a security vulnerability.
See [`SECURITY.md`](SECURITY.md) for the disclosure path.

## Community + governance

- **Code of conduct:** [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md)
  (Contributor Covenant 2.1). Treat each other well; harassment in
  any form gets you banned.
- **Roadmap:** the active program is described in commits + Linear
  GLO issues. The 14-PR consolidation tracked at
  `~/.claude/plans/research-how-to-starry-floyd.md` (operator-local,
  not in the repo) drove the May-15 mainnet-readiness refresh.
- **Roadmap visibility for external contributors:**
  [`docs/audit/mainnet-readiness-2026-05-15.md`](docs/audit/mainnet-readiness-2026-05-15.md)
  is the public-facing snapshot.

Thanks for contributing.
