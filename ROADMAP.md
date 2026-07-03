# Roadmap

The pre-mainnet roadmap for **suwappu-dag**. This document tracks the
public-facing milestones; `docs/architecture/sprint-map.md` is the
internal dependency graph + per-sprint exit gates.

For day-to-day work see [`CLAUDE.md`](./CLAUDE.md) (sprint backlog table)
and [`CHANGELOG.md`](./CHANGELOG.md) (released versions).

---

## TL;DR

| Phase | Window | Headline | Status |
|---|---|---|---|
| **Phase 0 — Spec** | Q1 2026 | v8 academic paper ratified, repo bootstrapped | ✅ Closed |
| **Phase 1 — Consensus + crypto** | Q1–Q2 2026 | DAG-S1…S20 close (Mysticeti-C + PQ crypto + LTP + SCION + fast-path) | ✅ Closed (v0.1.0) |
| **Phase 2 — State surface** | May 2026 | Force-include, slashing waterfall, registries, dual-bond model | ✅ Closed (v0.2.0) |
| **Phase 3 — Economic surface** | May 2026 | Per-slot stake, withdraw, eject-slash, cooldown, genesis, inflation, distribute, delegate, atomicity | ✅ Closed (v0.3.0) |
| **Phase 4 — Public devnet** | Q2 2026 | 4-region public devnet, RPC + faucet + explorer + status, operator-program v0 | 🟡 In flight |
| **Phase 5 — Incentivized testnet** | Q3 2026 | 7-region testnet, external operator points program, audit prep | ⏳ Next |
| **Phase 6 — Mainnet candidate** | Q4 2026 | All audits closed, fork-test passing, genesis ceremony scripted | ⏳ |
| **Mainnet GA (v1.0)** | M18 — M24 | Genesis ceremony, validator set onboarding, public mainnet | ⏳ |

---

## Phase 0 — Specification (closed)

- v8 academic paper ratified in
  `Suwappu-Labs/suwappu-papers/papers/dag-l1` (`suwappu_dag_l1_academic_v7.pdf`).
- Companion LTP paper in `suwappu_ltp_academic_v7.pdf`.
- Repo bootstrap: workspace layout, crate skeletons, CI matrix
  (rustfmt / clippy / test / cargo-deny), collaboration contract in
  `CLAUDE.md`.

## Phase 1 — Consensus + crypto (v0.1.0)

Sprints **DAG-S1 → DAG-S20** plus F1–F4 + C1–C4 hardening tracks. Every
sprint shipped a 10,000-case property-test exit gate. See `CHANGELOG.md`
for the per-sprint scope and `docs/architecture/sprint-map.md` for the
dependency graph.

Highlights:

- Post-quantum crypto surface (ML-DSA-65, ML-KEM-768, BLS12-381, SHA3-256).
- Mysticeti-C certificate DAG + joint-quorum AND-gate safety (Theorem 2).
- Fast-path lane with K=4 equivocation binding (100% slashing).
- Constant-size LTP attestation (≈1,600 B regardless of payload).
- JSON-RPC + WebSocket API + Rust + TypeScript SDKs.
- Streaming indexer + per-IP rate limit + bincode 2.x wire frames.

## Phase 2 — State surface (v0.2.0)

Closes Track G (force-include + bridge hardening) and Tokenomics §8.3
(slashing distribution) at the substrate layer:

- Force-include obligation registry + full lifecycle.
- Sequencer dual-bond (liveness + safety).
- Bridge asset whitelist + L2 burn-nullifier set.
- Multi-chain VK registry (multiple L2s on one L1).
- Equivocation replay defense.
- Insurance / treasury / snitch-bounty payout Intents.
- Authority + Validator Ring registry modules.

## Phase 3 — Economic surface (v0.3.0, shipped 2026-05-18)

Closes Tokenomics §3 (inflation) / §4 (delegated PoS) / §8 (slashing
waterfall) at the substrate layer. See
[CHANGELOG.md#030--2026-05-18](./CHANGELOG.md) for full detail.

- Per-slot bonded `deposited_stake` tracking on Authority + Validator Ring.
- Per-slot ejection slashing (drain through §8.3 waterfall).
- Graceful Withdraw + cooldown (`EXIT_COOLDOWN_BLOCKS ≈ 14 days`).
- `Intent::GenesisAllocation` for TGE seeding.
- `Intent::MintInflation` + `Intent::DistributeRewards` (per-ring
  monotone-epoch replay defense).
- `Intent::Delegate` + delegation registry.
- All-or-nothing atomicity sweep across stake / inflation / distribute /
  eject arms.

## Phase 4 — Public devnet (in flight)

Hosting-program letter tracks from the existing devnet README:

| Track | Scope | Status |
|---|---|---|
| **G1** | Devnet hosting infrastructure (4-region terraform, persistent EBS, public RPC) | ✅ Apply-ready |
| **G2** | Public RPC endpoint (DNS + TLS + ALB + WAF) | ✅ Apply-ready (`terraform/devnet/{dns,acm,alb,waf}.tf`) |
| **G3** | `suwappu-faucet` service | ✅ Built (`crates/suwappu-faucet`, `terraform/devnet/faucet.tf`, CI) |
| **G4** | Release-binary workflow (multi-target `suwappu-node` / `suwappu-loadgen` / `suwappu-indexer` / `suwappu-faucet` tarballs on tag) | ✅ Live |
| **G5** | `OPERATIONS.md` runbook (devnet + testnet sections) | 🟡 Partial |
| **G6** | Prometheus `/metrics` on `suwappu-node` + CloudWatch dashboard + alarms | ✅ Built (`crates/suwappu-node/src/metrics_http.rs`, `terraform/devnet/cloudwatch.tf`) |
| **G7** | Block explorer SPA | ✅ Built (`clients/explorer`, `terraform/devnet/explorer.tf`, `explorer.yml` CI) |
| **G8** | Status page | ✅ Built (`clients/status-page`, `terraform/devnet/status.tf`, `status.yml` CI) |

The software and infrastructure-as-code for every G-track above are in
the repo (apply-ready). What remains for each is the operator-run
`scripts/deploy-aws.sh` apply against the `gsn` AWS account (deployment
is gated on AWS credentials, not on further engineering). See
`terraform/devnet/README.md` and the **Unreleased** section of
`CHANGELOG.md`.

## Phase 5 — Incentivized testnet

7-region foundation-operated seed cluster + external operator
points program (Track B of the M18–M24 mainnet plan). Plan in
[`terraform/testnet/README.md`](./terraform/testnet/README.md).

Headline scope:

- 7 seed validators (us-east-1, us-west-2, eu-west-1, eu-central-1,
  ap-southeast-1, ap-northeast-1, sa-east-1).
- External operator onboarding (`scripts/testnet/onboard-operator.sh`).
- Points-accumulator daemon (`crates/suwappu-validator-program/`, TBD).
- Points-to-mainnet-token conversion published in `docs/testnet/POINTS.md`.
- `terraform/testnet/l2.tf` follow-up: L2 sequencer + zk-prover on top of
  the L1 (Track G).

## Phase 6 — Mainnet candidate

Pre-genesis gates:

- External security audits (consensus, crypto, bridge, substrate).
- Fork-test against the testnet's accumulated state (~weeks of chain
  history + points data).
- Genesis ceremony scripted (`scripts/mainnet/gen-genesis.py` follow-up).
- Validator set onboarding complete (40 Authority Ring + 200 Validator
  Ring seats per Tokenomics §4).
- Treasury + insurance + foundation allocations sealed at TGE.
- LTP corridor live (7-of-9 attestation across foundational chains).

## Mainnet GA (v1.0)

- Genesis ceremony executes.
- `suwappu-mainnet-v1` chain-id reserved.
- Validator set transitions from testnet to mainnet via the dual-ring
  joint-quorum hand-off documented in
  `docs/architecture/governance-phasing.md`.
- Public mainnet RPC + faucet (initial liquidity) + explorer.

---

## How to contribute to the roadmap

- File a [discussion](https://github.com/Suwappu-Labs/suwappu-dag/discussions)
  for any phase-level proposal.
- File an [issue](https://github.com/Suwappu-Labs/suwappu-dag/issues)
  for a concrete sub-sprint that fits an existing phase.
- See [`CONTRIBUTING.md`](./CONTRIBUTING.md) for the development
  workflow + sprint cadence.
- See [`SECURITY.md`](./SECURITY.md) for the coordinated-disclosure
  process if you found a vulnerability rather than a feature gap.
