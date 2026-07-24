# Changelog

All notable changes to **suwappu-dag** are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). The
release workflow (`.github/workflows/release.yml`) extracts each
`## <version>` section verbatim as the GitHub Release notes.

The pre-mainnet `0.x` line tracks the substrate + consensus + bridge
surface landing per [ROADMAP.md](./ROADMAP.md). The first `1.0` release
will coincide with mainnet genesis.

## [Unreleased]

### Changed

- Renamed our consensus's internal identifier from `Mysticeti-C` to
  `DagBft-C` across crates, docs, and visuals. `Mysticeti-C` is not a
  generic term — it names a specific sub-protocol in Mysten Labs'
  published Mysticeti consensus (arXiv:2310.14821), live in production
  on Sui mainnet. Reusing that exact name for our own independently
  implemented consensus read as an unintended claim of shared lineage.
  `DagBft-C` remains design-inspired by Mysticeti (see README
  Attribution section and `suwappu-consensus` module docs) but the
  identifier is now our own.

### Planned (G-track devnet hosting program → public testnet rollout)

- **G2** — public RPC endpoint with DNS + TLS + ALB + WAF.
- **G3** — `suwappu-faucet` service.
- **G5** — `OPERATIONS.md` runbook hardening (devnet + testnet sections).
- **G6** — Prometheus `/metrics` on `suwappu-node` + CloudWatch dashboard + alarms.
- **G7** — block explorer SPA.
- **G8** — status page.
- L2 sequencer + prover wire-up (Track G follow-up; see `terraform/testnet/l2.tf`
  placeholder).
- Two-phase Undelegate (`UndelegateBegin` → cooldown → `UndelegateClaim`)
  + per-slot delegator slashing on `EjectValidator`.
- `suwappu-validator-program` points-accumulator daemon.

---

## [0.3.0] — 2026-05-18

Full **stake lifecycle + epoch economic surface** on the execution
substrate. Closes the gap between the validator-set registries (v0.2)
and the validator-economic model (Tokenomics §3 / §4 / §8).

### Added

- **Per-slot bonded stake tracking** on Authority + Validator Ring
  records (`deposited_stake: u64`) — registry encoding bumped v1→v2
  with full back-compat decoding. (#209)
- **Per-slot ejection slashing**: `EjectAuthority` / `EjectValidator`
  drain the ejected slot's `deposited_stake` through the Tokenomics
  §8.3 waterfall (70% insurance, 30% treasury) instead of being
  status-only. (#210)
- **Graceful-path withdraw**: `Intent::WithdrawAuthorityStake` /
  `Intent::WithdrawValidatorStake` reverse a prior deposit when the
  slot is in `Exiting` state. (#211)
- **Exit cooldown** (`EXIT_COOLDOWN_BLOCKS = 2_419_200`, ≈14 days at
  500 ms/round) — anchored in a new `exit_block_height` field on
  Authority / Validator records (encoding v2→v3). `Withdraw*` rejects
  inside the window. Also introduces `current_block_height()` on the
  `Substrate` trait, plumbed through `execute_block`. (#212)
- **Genesis allocation**: `Intent::GenesisAllocation` for on-chain
  TGE seeding, gated to block 0 only; permits crediting reserved
  protocol-owned addresses. (#213)
- **Inflation minting**: `Intent::MintInflation` credits the Authority
  Ring rewards pool, Validator Ring rewards pool, and treasury at
  epoch boundaries; replay-defended via monotone-increasing epoch
  counter. (#214)
- **Reward distribution**: `Intent::DistributeRewards { epoch, ring,
  recipients }` drains either rewards pool to its active set's payout
  addresses; per-ring epoch replay defense. (#215)
- **Delegation primitive**: `Intent::Delegate` routes user stake into
  a Validator Ring slot's pool with per-(validator, delegator)
  tracking in a new `delegation_registry` module. (#216)
- **Atomicity hardening**: every multi-credit + debit-then-credit arm
  in the substrate is now all-or-nothing on overflow, via three new
  helpers (`transfer_internal`, `credit_many_atomic`,
  `drain_and_credit_atomic`). (#217)
- **20 reserved registry addresses** in `crates/suwappu-execution/src/reserved.rs`,
  covering the full lifecycle of stake / rewards / inflation /
  delegation pools and their replay-defense registries.

### Fixed

- Terraform output `description` fields in `terraform/testnet/dns.tf`
  and `terraform/devnet/dns.tf` no longer interpolate variables
  (rejected by terraform 1.x schema). Unblocks `terraform plan` on
  both stacks. (#218)

### Tests

- Each new Intent ships with happy-path + every rejection-path test,
  plus state-root atomicity assertions for the rollback paths in
  #217. ~80 new tests added on top of the existing ~265 in
  `suwappu-execution`.

---

## [0.2.0] — 2026-05-17

Substrate state-surface for **force-include lifecycle + slashing
waterfall + bridge security**. Closes Track G (force-include +
bridge hardening) and Tokenomics §8.3 (slashing distribution) at the
substrate layer. Companion to the v0.1.0 consensus surface.

### Added

- Force-include obligation registry + Pending → Honored / Slashed →
  Ejected lifecycle (`MarkForceIncludeHonored`, `SlashSequencer`,
  `EjectSequencer`).
- Sequencer dual-bond model: `sequencer_bond_address` (liveness,
  refundable, 5%-drain-per-slash) + `safety_bond_address`
  (equivocation, 100% forfeit).
- Bridge asset whitelist + `AssetStatus` lifecycle.
- L2 burn-nullifier set (G3.2): double-spend defense on
  `L2BurnProven`.
- Multi-chain VK registry: per-chain `aggregation_vk_hash` pinning so
  multiple L2s can coexist on one L1.
- Equivocation replay defense: `intent_hash` set keyed per
  `OffenseKind` so re-slashing after a safety-bond refill rejects.
- Insurance / treasury disbursement Intents (`DisburseTreasury`,
  `ClaimInsurance`).
- Snitch bounty (10% of slash, capped 1M SUWAPPU) paid from treasury on
  successful `SlashSequencer`.
- Authority Ring + Validator Ring registry modules
  (`authority_registry`, `validator_registry`).
- Real economic stake bonding (`DepositSequencerBond`,
  `DepositSafetyBond`, `DepositAuthorityStake`,
  `DepositValidatorStake`).

### Pending (rolling forward into 0.3.x)

- DA anchor registry (PostL2DA no-op gap closure) — in flight on
  branch `execution/da-anchor-registry`, PR #208.

---

## [0.1.0] — 2026-04

**Mainnet-track consensus + crypto + transport stack.** Sprints
DAG-S1 through DAG-S20 plus the F (F1–F4) and C (C1–C4) hardening
tracks. Every sprint shipped its 4 properties × 10k proptest cases
exit gate; see [CLAUDE.md](./CLAUDE.md) sprint backlog table for
per-sprint scope.

### Added

- **DAG-S1** `suwappu-crypto`: ML-DSA-65 (FIPS 204), ML-KEM-768
  (FIPS 203), BLS12-381, SHA3-256.
- **DAG-S2** `suwappu-transport`: RaptorQ shred / reconstruct (in-mem).
- **DAG-S3** `suwappu-consensus`: DAG store, certificate types, vote
  aggregation.
- **DAG-S4** DagBft-C commit rule.
- **DAG-S5** Joint-quorum AND-gate (paper Theorem 2 — Authority Ring
  AND Validator Ring must both ratify).
- **DAG-S6** Authority + Validator registry types + quorum threshold.
- **DAG-S7** Equivocation detection + slashing surface.
- **DAG-S8** `suwappu-fastpath`: single-owner lane + K=4 binding.
- **DAG-S9** Fast-path equivocation slashing (paper §6.4 — 100%
  bond forfeiture).
- **DAG-S10** `suwappu-execution`: block executor adapter + `Substrate`
  trait.
- **DAG-S11** Checkpoint cadence + Authority joint co-signature.
- **DAG-S12** `suwappu-precompiles`: DID resolver.
- **DAG-S13** Registered-issuer precompile (mint / burn).
- **DAG-S14** Reserve-coverage circuit-breaker predicate.
- **DAG-S15** `suwappu-ltp`: super-node 7-of-9 attestation.
- **DAG-S16** LTP Commitment Node DA SLA.
- **DAG-S17** Cross-chain DID STARK pipeline (SP1 / Plonky3).
- **DAG-S18** SCION path-authenticated routing.
- **DAG-S19** SCION-IP-Gateway fallback.
- **DAG-S20** `suwappu-node`: full validator composition (E2E).
- **F1** Per-IP rate-limit (`crates/suwappu-rpc/src/per_ip.rs`).
- **F2** Streaming indexer (`crates/suwappu-indexer/`) with Postgres
  backend + startup catch-up backfill.
- **F3** JSON-RPC + WebSocket API (`crates/suwappu-rpc/`) — 8 read
  methods + `submit_intent` + `subscribe_events`.
- **F4** bincode 2.x + 1-byte wire-frame version marker
  (`crates/suwappu-node/src/codec.rs`).
- **C1** Local 4-node docker-compose devnet (`DEVNET.md`).
- **C2** 4 Rust + 3 TS starter examples.
- **C3** `CONTRIBUTING.md` + initial `SECURITY.md` (partial).
- **C4** `#[non_exhaustive]` on `Intent` and `RpcError`; rustdoc +
  TypeDoc publishing workflow.
- **B4** cargo-fuzz workspace with `wire_decode`, `dag_insert`,
  `decide_slot` targets.
- Constant-size LTP attestation (≈1,600 B regardless of payload,
  paper §10.2).
- Rust + TypeScript SDKs (`clients/rust-sdk/`, `clients/ts-sdk/`).
- Devnet hosting infra (`terraform/devnet/`, G1) — 4-region
  always-on stack with persistent EBS and public RPC.
- Release-binary workflow (`.github/workflows/release.yml`, G4).

See [`docs/iq/`](./docs/iq) for the ratified investigation questions
(IQ-001 through IQ-005) and
[`docs/audit/mainnet-readiness-2026-05-15.md`](./docs/audit/mainnet-readiness-2026-05-15.md)
for the security + ops posture at this milestone.

---

[Unreleased]: https://github.com/Suwappu-Labs/suwappu-dag/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/Suwappu-Labs/suwappu-dag/releases/tag/v0.3.0
[0.2.0]: https://github.com/Suwappu-Labs/suwappu-dag/releases/tag/v0.2.0
[0.1.0]: https://github.com/Suwappu-Labs/suwappu-dag/releases/tag/v0.1.0
