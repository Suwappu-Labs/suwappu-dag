# gsx-dag — Claude Code project context

This file is loaded automatically at the start of every Claude Code session in
this repo. It is the entry point for orienting Claude Code on conventions,
current sprint state, load-bearing invariants, and how to collaborate.

## Project

`gsx-dag` is the implementation of the **GSX DAG Layer 1**: a Mysticeti-style
certificate-DAG settlement chain with a dual-ring validator set, co-resident
dual VM, and post-quantum cross-chain attestation under the Lattice Transfer
Protocol. The reference design is `gsx-papers/papers/dag-l1` (formerly
`gsx_dag_l1_academic_v7.pdf`).

The execution substrate (polymorphic balance map, OCC scheduler, state tree,
anchor pipeline, recovery replay) is implemented in
[`GlobalSettlementNetwork/gsx-db`](https://github.com/GlobalSettlementNetwork/gsx-db)
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

4. **Substrate invariants inherited from gsx-db** — lane separation, dual-VM
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
| DAG-S1 | `gsx-crypto` — ML-DSA-65, ML-KEM-768, BLS12-381, SHA3-256 | ✅ Closed    | 7 properties × 10k cases (`tests/proptest_roundtrips.rs`) |
| DAG-S2 | `gsx-transport` — RaptorQ shred/reconstruct (in-mem)      | ✅ Closed    | 4 properties × 10k cases (`tests/proptest_reconstruction.rs`) |
| DAG-S3 | `gsx-consensus` — DAG store, certificate types, voting   | ✅ Closed    | 4 properties × 10k cases (`tests/proptest_dag_order.rs`) |
| DAG-S4 | `gsx-consensus` — Mysticeti-C commit rule                | ⏳ Queued    | `mysticeti_c_finality` @ 10k |
| DAG-S5 | `gsx-consensus` — joint-quorum AND-gate                  | ⏳ Queued    | `joint_quorum_safety` @ 10k |
| DAG-S6 | `gsx-authority` + `gsx-validator` — registries & quorum  | ⏳ Queued    | `quorum_math_matches_paper` @ 10k |
| DAG-S7 | Equivocation detection + slashing                         | ⏳ Queued    | `equivocation_proof_slashes` @ 10k |
| DAG-S8 | `gsx-fastpath` — single-owner lane + K=4 binding         | ⏳ Queued    | `fast_path_main_lane_consistency` @ 10k |
| DAG-S9 | Fast-path equivocation slashing                          | ⏳ Queued    | `fast_path_equivocation_full_slash` @ 10k |
| DAG-S10 | `gsx-execution` — wire gsx-db; block executor adapter   | ⏳ Queued    | `block_execution_matches_substrate` @ 10k |
| DAG-S11 | Checkpoint cadence + Authority joint co-signature       | ⏳ Queued    | `joint_state_commitment_signed` @ 10k |
| DAG-S12 | `gsx-precompiles` — DID resolver                         | ⏳ Queued    | `did_document_validates` @ 10k |
| DAG-S13 | Registered-issuer precompile (mint/burn)                | ⏳ Queued    | `issuer_mint_burn_atomic` @ 10k |
| DAG-S14 | Reserve-coverage PlonK circuit                           | ⏳ Queued    | `reserve_coverage_predicate` @ 10k |
| DAG-S15 | `gsx-ltp` — super-node 7-of-9 attestation                | ⏳ Queued    | `seven_of_nine_attestation` @ 10k |
| DAG-S16 | LTP Commitment Node DA SLA                                | ⏳ Queued    | `da_sla_enforced` @ 10k |
| DAG-S17 | Cross-chain DID STARK pipeline (SP1/Plonky3)             | ⏳ Queued    | `did_stark_round_trip` @ 10k |
| DAG-S18 | `gsx-transport` — SCION routing                          | ⏳ Queued    | `scion_path_auth` @ 10k |
| DAG-S19 | SCION-IP-Gateway fallback                                | ⏳ Queued    | `gateway_fallback_correctness` @ 10k |
| DAG-S20 | `gsx-node` — full validator composition                  | ⏳ Queued    | `node_runs_genesis_block` (E2E) |

Update this table when a sprint closes.

## Conventions

### Build tools

- **Rust:** `cargo` for everything. Workspace at repo root.
- **Solidity:** none in this repo (LTPAnchorRegistry lives in `gsx-db/contracts`).
- **Infra:** `terraform`. Lives in `terraform/`. AWS profile `gsn`
  (account 492042618949, us-east-1). Apply via `scripts/deploy-aws.sh`,
  never raw `terraform apply` (that's denied).

### Branch naming

`<scope>/<short-slug>` — e.g., `crypto/mldsa-acvp`, `consensus/mysticeti-c`,
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
  pending — see `gsx-crypto/tests/acvp_vectors/`)

## Specialist subagents

Invoke these proactively per the rules below.

| Trigger | Subagent | Why |
|---|---|---|
| Changes to `gsx-crypto` | `crypto-reviewer` | PQ correctness + side-channels |
| Changes to `gsx-consensus` | `consensus-reviewer` | DAG topology + Mysticeti commit rule |
| Changes to `gsx-fastpath` | `fastpath-auditor` | Equivocation proof completeness |
| Changes to `gsx-transport` | `transport-auditor` | SCION path auth + RaptorQ |
| Changes touching joint-quorum logic | `consensus-reviewer` + `crypto-reviewer` | Theorem 2 dependence |

Subagent definitions live in `claude-code/`.

## Slash commands

| Command | What it does |
|---|---|
| `/sprint <id>` | Drive a sprint from start to PR |
| `/check` | Run `cargo fmt`, `cargo clippy`, `cargo test --workspace`, `cargo deny check` |
| `/check-10k` | Run all proptests at `PROPTEST_CASES=10000 --release` |
| `/release <version>` | Tag and ship a release |
| `/aws-status` | Snapshot AWS infra health (read-only) |
| `/iq-decision <topic>` | Record a new IQ (Investigation Question) |

## Permissions

`claude-code/settings.json` defines three tiers (mirrors `gsx-db`):

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
