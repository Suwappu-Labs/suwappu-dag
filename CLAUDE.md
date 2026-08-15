# suwappu-dag — Claude Code project context

This file is loaded automatically at the start of every Claude Code session in
this repo. It is the entry point for orienting Claude Code on conventions,
current sprint state, load-bearing invariants, and how to collaborate.

## Project

`suwappu-dag` is the implementation of the **SUWAPPU DAG Layer 1**: a
`DagBft-C` certificate-DAG settlement chain (design-inspired by Mysticeti,
arXiv:2310.14821 — an independent implementation, not a fork of Sui's
`MystenLabs/sui` consensus) with a dual-ring validator set, co-resident
dual VM, and post-quantum cross-chain attestation under the Lattice Transfer
Protocol. The reference design is `suwappu-papers/papers/dag-l1` (formerly
`suwappu_dag_l1_academic_v7.pdf`).

The execution substrate (polymorphic balance map, OCC scheduler, state tree,
anchor pipeline, recovery replay) is implemented in
[`Suwappu-Labs/suwappu-db`](https://github.com/Suwappu-Labs/suwappu-db)
and consumed here as a workspace dependency from DAG-S10 onward.

## Load-bearing invariants

These are non-negotiable. Code that weakens them does not ship.

1. **Joint-quorum AND-gate safety** (Paper Theorem 2) — a safety violation
   requires Byzantine corruption of *both* the Authority Ring and the
   Validator Ring simultaneously. Quorum logic that collapses either ring
   into the other is rejected.

2. **PQ-conservative crypto surface** (Paper §3.3, §12) — every long-lived
   confidentiality and integrity surface uses NIST-standardized post-quantum
   primitives (ML-DSA-65 / FIPS 204, ML-KEM-768 / FIPS 203). Classical primitives
   (ECDSA secp256k1, BLS12-381, Groth16/BN254) are retained only on the documented
   exception zones with migration targets.

3. **Constant-size LTP commitment** (Paper §10.2) — every LTP attestation commits
   ≈1,600 B regardless of payload: ML-KEM-768 ciphertext (≈1,568 B) +
   BLS12-381 aggregate signature (≈96 B) + SHA3-256 payload root (32 B). Changes
   that add per-payload bytes to the on-chain commitment surface are rejected.

4. **Substrate invariants inherited from suwappu-db** — lane separation, dual-VM
   projection equality, schedule determinism, bundle atomicity, tree determinism,
   cross-chain parity, replay equivalence. The DAG executor wires these through;
   it cannot weaken them.

5. **Fast-path equivocation = 100% slashing** (Paper §6.4) — an Authority Node
   that signs a fast-path certificate for a transaction whose main-lane
   confirmation observes a conflicting ordering forfeits 100% of bonded stake
   plus expulsion.

6. **No git rebase, ever** — repo convention. Use `git merge` or
   `git pull --no-rebase`.

7. **No "Co-Authored-By" lines in commit messages** — repo convention.

## Workflow

You and the human collaborate sprint-by-sprint. The standard flow:

1. Human types `/sprint S<n>` to start a sprint
2. You read the spec, plan, get approval, implement step-by-step
3. You run `/check` for verification + invoke specialist subagents on sensitive surfaces
4. You prepare a PR with `gh pr create` (in `ask` permission tier — human confirms)
5. Human reviews and merges

Between sessions, you resume via:

- This `CLAUDE.md` (sprint backlog section below)
- `gh issue list --label sprint-<n>` for tracked work
- Branch name (`<scope>/<short-slug>`) for context
- `.sprint-state.md` at the branch root for in-flight state

## Sprint backlog

| Sprint | Scope                                                     | Status      | Exit gate |
|--------|-----------------------------------------------------------|-------------|-----------|
| DAG-S1 | `suwappu-crypto` — ML-DSA-65, ML-KEM-768, BLS12-381, SHA3-256 | ✅ Closed    | 7 properties × 10k cases (`tests/proptest_roundtrips.rs`) |
| DAG-S2 | `suwappu-transport` — RaptorQ shred/reconstruct (in-mem)      | ✅ Closed    | 4 properties × 10k cases (`tests/proptest_reconstruction.rs`) |
| DAG-S3 | `suwappu-consensus` — DAG store, certificate types, voting   | ✅ Closed    | 4 properties × 10k cases (`tests/proptest_dag_order.rs`) |
| DAG-S4 | `suwappu-consensus` — DagBft-C commit rule                | ✅ Closed    | 4 properties × 10k cases (`tests/proptest_dagbft_commit.rs`) |
| DAG-S5 | `suwappu-consensus` — joint-quorum AND-gate (Theorem 2)     | ✅ Closed    | 4 properties × 10k cases (`tests/proptest_joint_quorum.rs`) |
| DAG-S6 | `suwappu-authority` + `suwappu-validator` — registries & quorum  | ✅ Closed    | 4 properties × 10k cases (`tests/proptest_quorum.rs`) |
| DAG-S7 | Equivocation detection + slashing                         | ✅ Closed    | 4 properties × 10k cases (`tests/proptest_slashing.rs`) |
| DAG-S8 | `suwappu-fastpath` — single-owner lane + K=4 binding         | ✅ Closed    | 4 properties × 10k cases (`tests/proptest_fast_path.rs`) |
| DAG-S9 | Fast-path equivocation slashing                          | ✅ Closed    | 4 properties × 10k cases (`tests/proptest_fp_slashing.rs`) |
| DAG-S10 | `suwappu-execution` — block executor adapter (Substrate trait) | ✅ Closed    | 4 properties × 10k cases (`tests/proptest_block_execution.rs`) |
| DAG-S11 | Checkpoint cadence + Authority joint co-signature       | ✅ Closed    | 4 properties × 10k cases (`tests/proptest_checkpoint.rs`) |
| DAG-S12 | `suwappu-precompiles` — DID resolver                         | ✅ Closed    | 4 properties × 10k cases (`tests/proptest_did.rs`) |
| DAG-S13 | Registered-issuer precompile (mint/burn)                | ✅ Closed    | 4 properties × 10k cases (`tests/proptest_issuer.rs`) |
| DAG-S14 | Reserve-coverage circuit breaker (predicate)             | ✅ Closed    | 4 properties × 10k cases (`tests/proptest_reserve.rs`) |
| DAG-S15 | `suwappu-ltp` — super-node 7-of-9 attestation                | ✅ Closed    | 4 properties × 10k cases (`tests/proptest_attestation.rs`) |
| DAG-S16 | LTP Commitment Node DA SLA                                | ✅ Closed    | 4 properties × 10k cases (`tests/proptest_da.rs`) |
| DAG-S17 | Cross-chain DID STARK pipeline (SP1/Plonky3)             | ✅ Closed    | 4 properties × 10k cases (`tests/proptest_did_stark.rs`) |
| DAG-S18 | `suwappu-transport` — SCION path-authenticated routing       | ✅ Closed    | 4 properties × 10k cases (`tests/proptest_scion.rs`) |
| DAG-S19 | SCION-IP-Gateway fallback                                | ✅ Closed    | 4 properties × 10k cases (`tests/proptest_gateway.rs`) |
| DAG-S20 | `suwappu-node` — full validator composition (E2E)            | ✅ Closed    | 3 properties × 10k cases (`tests/proptest_genesis_flow.rs`) |

Update this table when a sprint closes.

## Conventions

### Build tools

- **Rust:** `cargo` for everything. Workspace at repo root.
- **Solidity:** none in this repo (LTPAnchorRegistry lives in `suwappu-db/contracts`).
- **Infra:** `terraform`. Lives in `terraform/`. AWS profile `gsn`
  (account 492042618949, us-east-1). Apply via `scripts/deploy-aws.sh`,
  never raw `terraform apply` (that's denied).

### Local development

- **Never run `cargo test --workspace`, `cargo build --workspace`, or
  `cargo clippy --workspace` locally.** This Mac isn't powerful enough —
  workspace-wide commands hang, thrash, or saturate CPU and starve other
  work. Push to a feature branch and let GHA's CI matrix (rustfmt /
  clippy / test / cargo-deny) validate. Iteration loop = CI cycle, not
  local cargo. See `~/.claude/projects/-Users-mongolraider/memory/2-patterns/suwappu-dag-no-local-cargo-test.md`.
- Single-binary local builds are fine: `cargo build --release -p suwappu-node --bin suwappu-metrics`.
- `cargo fmt -p <crate>` is cheap; run it locally before pushing to avoid
  trivial rustfmt CI failures.
- When changing a public enum's exhaustiveness (`#[non_exhaustive]`,
  adding/removing a variant), per-crate `cargo check -p <defining>`
  misses matches in downstream consumer crates — `#[non_exhaustive]`
  doesn't apply within the defining crate but DOES across the workspace.
  Explicitly include every consumer in the `-p` set, e.g.
  `cargo check -p suwappu-execution -p suwappu-node -p suwappu-rpc -p suwappu-fastpath
  -p suwappu-mempool`. CI catches this; local per-crate check does not.

### Branch naming

`<scope>/<short-slug>` — e.g., `crypto/mldsa-acvp`, `consensus/dagbft-c`,
`fastpath/equivocation-proof`.

### Commits

- Focused, single-purpose
- Imperative mood ("Add X", not "Added X")
- No "Co-Authored-By"
- Reference issues with `Closes #N` or `Refs #N`

### Pull requests

- Title: matches the sprint or IQ context
- Body: must include exit gate test command + subagent verdicts
- Do not auto-merge; the human approves and merges

### Tests

- Unit tests inline (`#[cfg(test)] mod tests`)
- Integration tests in `tests/`
- Property tests use `proptest` — minimum 10k iterations for sprint exit gates
- NIST ACVP vectors integrated for ML-DSA-65 and ML-KEM-768 (DAG-S1.1,
  pending — see `suwappu-crypto/tests/acvp_vectors/`)

## Specialist subagents

Invoke these proactively per the rules below.

| Trigger | Subagent | Why |
|---|---|---|
| Changes to `suwappu-crypto` | `crypto-reviewer` | PQ correctness + side-channels |
| Changes to `suwappu-consensus` | `consensus-reviewer` | DAG topology + DagBft-C commit rule |
| Changes to `suwappu-fastpath` | `fastpath-auditor` | Equivocation proof completeness |
| Changes to `suwappu-transport` | `transport-auditor` | SCION path auth + RaptorQ |
| Changes touching joint-quorum logic | `consensus-reviewer` + `crypto-reviewer` | Theorem 2 dependence |

Subagent definitions live in `claude-code/`.

## Slash commands

| Command | What it does |
|---|---|
| `/sprint <id>` | Drive a sprint from start to PR |
| `/check` | Run `cargo fmt`, `cargo clippy`, `cargo test --workspace`, `check-crypto-boundary.sh`, `cargo deny check` |
| `/check-10k` | Run all proptests at `PROPTEST_CASES=10000 --release` |
| `/release <version>` | Tag and ship a release |
| `/aws-status` | Snapshot AWS infra health (read-only) |
| `/iq-decision <topic>` | Record a new IQ (Investigation Question) |

## Permissions

`claude-code/settings.json` defines three tiers (mirrors `suwappu-db`):

- **Allowed silently** — read-only ops, local builds/tests, file ops
- **Asked** — anything that mutates remote state (push, tag, PR creation, AWS deploys)
- **Denied** — destructive ops (`rm -rf /`, force push, `terraform destroy`,
  `aws ec2 terminate`)

The denylist is the security floor. Add to it; do not remove without explicit
security review.

## Hooks

`settings.json` configures:

1. **PostToolUse on Edit/Write/MultiEdit** — runs `cargo fmt --check`; hints if drift.
2. **PreToolUse on Bash** — pattern-blocks `rm -rf /`, `git push --force`,
   `terraform destroy` even if they sneak past the denylist.

## Resuming work

When a session opens cold:

1. Read this `CLAUDE.md` (already loaded).
2. Run `git status` and `git rev-parse --abbrev-ref HEAD`.
3. If on a sprint branch, read `.sprint-state.md` at root.
4. Run `gh pr list --state open` to see in-flight PRs.

That's enough state to pick up cleanly without asking the human to re-orient.

## Updating this file

Update `CLAUDE.md` when:

- A sprint closes (mark it ✅ in the backlog table)
- A new load-bearing invariant is added (rare; needs an IQ first)
- A new slash command or subagent is canonicalized
- A repo convention shifts

Treat changes to `CLAUDE.md` like any other PR — review and merge.
