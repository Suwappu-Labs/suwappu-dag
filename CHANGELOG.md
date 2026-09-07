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

### Fixed

- CI is green again on `main`'s own code. Three findings, one cause:
  every job used `dtolnay/rust-toolchain@stable` with nothing pinning
  what "stable" resolves to. Rust 1.98.0 shipped 2026-08-18, two days
  after the last green run, and made `clippy::drain_collect` fire under
  the workspace's `-D warnings` — turning `crates/suwappu-node/src/daemon.rs`
  red with no commit touching it.
  - Rewrote both `drain(..).collect()` sites as `std::mem::take`, which
    is what clippy asked for and also avoids a needless allocation.
    Behaviour is identical: both drained the whole collection.
  - Added `rust-toolchain.toml` pinning 1.98.0 with `rustfmt` + `clippy`.
    Single source of truth — the workflows are untouched, since a
    toolchain file takes precedence over `rustup default` and duplicating
    the version across five job definitions would be worse. Raising the
    pin is now a deliberate PR that carries its own lint fixes, instead
    of new enforcement landing as unrelated red on someone else's branch.
  - RUSTSEC-2026-0258 (h2 unbounded empty DATA frames, low severity):
    bumped the 0.4 tree 0.4.15 -> 0.4.19, past the 0.4.16 fix. The 0.3
    tree cannot be fixed here — no patched 0.3.x exists (0.3.27 is the
    newest 0.3 release) and it arrives via
    `suwappudb-bridge -> reqwest 0.11 -> hyper 0.14`, the same upstream
    gate already recorded for RUSTSEC-2025-0134, so both ignores drop in
    the same future PR. cargo-deny 0.20's `[advisories].ignore` table
    accepts only `id` and `reason`, so the advisory cannot be scoped to
    one crate version; a `[[bans.deny]]` entry on `h2:>=0.4.0, <0.4.16`
    restores the coverage the bare-id ignore would otherwise lose on the
    0.4 tree — the tree our own axum / reqwest-0.12 listeners use.
    Verified the ban fires by downgrading h2 to 0.4.15 and re-running.

### Added

- **DAG-S34 (IQ-008): bounded DAG retention, durable commit log,
  checkpoint-anchored snapshot sync.** Closes `/goal` A6 and A7 at the
  code level; consensus-reviewer verdict + human sign-off gate the merge.
  Decision record: `docs/iq/IQ-008-bounded-retention-and-checkpoint-sync.md`.
  - `suwappu-consensus`: `gc.rs` (`GC_DEPTH = 256`, `gc_round`,
    `commit_floor`), a GC-aware `DagStore` (`prune_below`, tombstone
    window so a cert whose parent was pruned still inserts,
    `BelowGcRound` rejection) and `causal_history_bounded`. The committed
    sub-DAG of a leader is a function of the leader alone, so nodes with
    different pruning progress commit the same set (I-GC1; `proptest_gc.rs`).
  - `suwappu-node`: the gc round is `last_committed_leader − gc_depth`
    (Narwhal / Sui consensus-core rule); every hash- and round-keyed map
    is pruned at the same floor, ingest drops obsolete certs, `TipInfo`
    carries the peer's gc round, and `suwappu_getSyncStatus` / `/metrics`
    expose `gc_round`, `dag_certs`, `needs_snapshot`.
  - `suwappu-node::store`: blake3-chained append-only commit log with
    torn-tail truncation, atomic `StateSnapshot` files verified by
    `state_root` on load, and startup replay that re-uses the live
    `apply_commit` path (I-P1; `proptest_persistence.rs`). A restarted
    validator resumes its authored-round marker from disk and never
    re-signs a round (`restart_resumes_from_disk_without_equivocating`).
    New `NodeConfig` fields `data_dir`, `store_fsync`.
  - Checkpoint v2: `Checkpoint.registry_root` binds the committee, hash
    domain `SUWAPPU-CHECKPOINT-V2`; seated authorities co-sign every
    `checkpoint_cadence_rounds` (genesis manifest, default 32, must be ≤
    half `gc_depth_rounds`) via `CheckpointSig` frames;
    `verify_checkpoint_chain` walks a chain from the genesis committee
    (I-CK1; `proptest_checkpoint_chain.rs`). A joiner whose peers have
    pruned past its DAG requests `GetCheckpoints`, verifies the chain,
    pulls the snapshot in `SnapshotChunk`s, checks it against the
    co-signed root, installs it and resumes forward backfill
    (`joiner_bootstraps_from_cosigned_checkpoint_snapshot`).
  - Validator-Ring votes are retained until their certificate is pruned
    and relayed in `Votes` frames after every `GetCert` /
    `GetCertsByRound` reply, so a catching-up node ratifies leaders
    through the same joint-quorum AND-gate as a live node instead of
    halting at the first leader whose votes its peers had already
    dropped. Seeds push certs, blocks, votes and checkpoint signatures to
    dynamic peers; dynamic peers still cannot inject any of them.
  - Consensus-review fixes (S34.5): one commit walk / snapshot capture
    at a time (`State::commit_lock`); `Checkpoint.snapshot_root` binds
    the served snapshot's commit-derived body and joiners verify it plus
    every certificate's signature; checkpoints are crossed before the
    first leader above the boundary so every node signs the same root;
    the late-flip sweep floor is clamped explicitly and gated by
    `proptest_gc.rs::bounded_history_below_gc_is_clamped`; a validator
    that falls behind jumps its authoring round to the observed tip and
    backfills from its commit frontier rather than its DAG tip.
- `suwappu_getSyncStatus` JSON-RPC method: the one-call answer to "is
  this node caught up?" for operators, the status page (G8) and the
  explorer (G7). Returns `local_dag_round`, `latest_committed_round`,
  `peer_tip_round` (highest tip reported by a configured peer),
  `rounds_behind`, `synced` (within the forward-backfill lag threshold,
  now a shared constant so the RPC answer and the backfill loop cannot
  drift), `seated` (own authority id present in the held Authority
  Ring; `false` while a post-genesis joiner waits on its admit intent),
  and the orphan / inflight-fetch / needed-block counts that explain a
  stall. Exposed as `Client::get_sync_status` (Rust SDK) and
  `client.getSyncStatus()` (TS SDK). Previously an external validator
  admitted per `docs/testnet/VALIDATOR-OPERATORS.md` had no way to
  observe its own catch-up. Same numbers on `/metrics` as
  `suwappu_local_dag_round`, `suwappu_peer_tip_round`,
  `suwappu_rounds_behind`, `suwappu_synced`, `suwappu_seated`,
  `suwappu_orphan_certs` for the G6 dashboard + catch-up alarm.
- Research note `docs/research/kasperchain-kasper.md`: evaluated
  `kasperchain/kasper` for reusable material (verdict: a renamed copy of
  the deprecated, unlicensed KDX desktop app with no implementation of
  its README's PoW chain; nothing to adopt as code). The sync-status
  method above is the one idea from that review worth carrying.
- Portal live points lookup: the provider portal reads an operator's real
  points + TGE share estimate (2%-of-allocation cap honored) from the
  leaderboard API. Enabled by `Access-Control-Allow-Origin: *` on the
  validator-program public read routes only (admin stays un-layered;
  `leaderboard::add_public_cors`, with tests) and a TLS front for the
  HTTP-only program origin (`terraform/testnet/leaderboard-cdn.tf`,
  `leaderboard.testnet.suwappu.bot` -> program EC2 :8090, caching
  disabled, `program.` A record untouched)
- Compute-provider portal deploy surface: `terraform/testnet/compute-portal.tf`
  (S3 + CloudFront + Route53 for `compute.testnet.suwappu.bot`, same shape
  as the status page) and `.github/workflows/compute-portal-testnet.yml`
  (sed-rewrite devnet→testnet hostnames, sync, invalidate; skips cleanly
  until the `COMPUTE_TESTNET_DEPLOY_ROLE` secret is set)
- Validator compute-incentive settlement (`suwappu-precompiles::rewards`):
  per-epoch stablecoin rewards for proven validator work
  (`ComputeReceipt`: certificates signed, uptime samples, corridor
  attestations, DA bytes served), priced under `RewardParams`, clamped
  pro-rata to a hard epoch budget, and minted only through the new
  `reserve::mint_with_coverage` — the §8.3 reserve-coverage breaker
  bound into the §8.2 issuer mint surface (the follow-up DAG-S14 left
  open), evaluated at the projected post-mint outstanding supply and
  failing closed. Uptime gating mirrors `docs/testnet/POINTS.md`
  (≥99% full rate, ≥95% half, below zero); output recipient lists are
  shaped for the substrate's existing `Intent::DistributeRewards`.
  Exit gate: `tests/proptest_compute_rewards.rs`, 4 properties
  (conservation + reserve backing, coverage fail-closed, work
  monotonicity, epoch replay), green at `PROPTEST_CASES=10000
  --release`. Design doc:
  `suwappu-lattice-protocol/docs/economics/VALIDATOR_COMPUTE_INCENTIVES.md`
- Quality-parity pass (cross-repo bar set by suwappubot): shipped the
  agent infrastructure `CLAUDE.md` documents but the tree lacked —
  `claude-code/settings.json` (three permission tiers + hook wiring),
  `claude-code/hooks/` (rustfmt drift hint, destructive-command
  blocker), `claude-code/commands/` (all six slash commands) and a
  wiring README; `scripts/check-crypto-boundary.sh` lane-separation
  gate (referenced by `deny.toml` but previously missing) plus a
  `crypto-boundary` CI job; CodeQL analysis workflow
  (javascript-typescript + actions, SHA-pinned); `.github/dependabot.yml`
  (cargo, npm, actions, docker); tracked `clients/ts-sdk`
  `package-lock.json` with `npm ci` + npm cache in the ts-sdk workflow;
  root `CODE_OF_CONDUCT.md` and `SUPPORT.md`; `scripts/check.sh` now
  runs the crypto-boundary check and `cargo deny check` (matching the
  documented `/check` gate set)

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
