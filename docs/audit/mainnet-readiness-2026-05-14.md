# suwappu-dag mainnet-readiness audit (2026-05-14)

## Context

Operator request: audit GitHub branches and PRs, then the AWS infra, then
assess feature parity with the best chains and mainnet readiness.

This audit is grounded in two passes:

1. A local sweep of GitHub state, the codebase against production-L1
   baselines, and the AWS account.
2. A re-check of every code claim against current `main` (HEAD `bf1325c`,
   2026-05-14) — three code-status claims from the local sweep were
   already stale and have been corrected here — and a re-grounding of
   the L1 parity matrix in the 2025–2026 production landscape (Solana
   Alpenglow + Firedancer, Sui Mysticeti v2 + Transaction Driver,
   Aptos Raptr / Baby Raptr, Monad, MegaETH, Hyperliquid, Ethereum
   Pectra / Fusaka / Hegota, Avalanche9000 / ACP-77, Sei Giga,
   Berachain, Celestia, EigenDA; post-quantum chains: Algorand
   Falcon-1024, QRL Zond ML-DSA-87, Naoris).

**Verdict in one line.** Consensus core is sound and the *combined*
post-quantum surface is still a real differentiator (ML-DSA-65 +
ML-KEM-768 + BLS aggregate + SHA3-256 — Algorand and QRL each ship only
parts of this), but `suwappu-dag` is still a research artifact, not a chain
a user can interact with: no query RPC, no SDK, no explorer, no
mempool/fee market, no deployed bridge, deterministic placeholder
validator keys, and one real consensus bug under `#[ignore]` (Issue #18).
The 2026 mainnet bar is ≥5k real-world TPS and sub-second finality;
the 4-region perf cluster currently commits at **0.125 TPS** sustained.
Mainnet is **9–14 months** of focused work away.

---

## 1. GitHub state

| # | Type | Title | Action |
|---|---|---|---|
| PR #15 | PR | `DAG-S33: cert-finality metric + global campaign-window filter` | **Close.** Perf campaigns paused; re-open later if/when needed. |
| Issue #18 | Bug | `phase_g eject path: transitional quorum-threshold asymmetry stalls commits` | **Real consensus bug**; mainnet blocker. Three candidate fixes in the issue: (a) apply governance at epoch boundary; (b) gate `quorum_threshold` on highest committed `n`; (c) explicit barrier round. Recommend (a). Rationale at `crates/suwappu-node/src/daemon.rs:1650-1680`. |

Stale remote branches (`consensus/dag-s27-...`, `s28`, `s29`, `s30`,
`orphan-cert-pull`) are merged into `main` — delete via
`gh api -X DELETE /repos/.../git/refs/heads/<branch>`.

No tags or releases exist. Cut `v0.1.0-testnet` before any public exposure.

---

## 2. Code feature parity vs. the 2026 production landscape

### 2.1 S1–S20 spot-check (only the discrepancies from the local sweep)

| Sprint | Earlier read | Verified state (2026-05-14) |
|---|---|---|
| S4 (Mysticeti-C commit) | Direct-only; IQ-002 open | Direct + indirect both implemented at `commit.rs:126-202`. IQ-002 awaiting formal sign-off only. |
| S5 (joint quorum) | Diverges from Definition 2 | Canonical `2f+1` (`commit.rs:61-66`). IQ-001 awaiting formal sign-off only. |
| S8/S9 (fast-path + slashing) | Library only; daemon no-op | Library + daemon receiver + proposer + equivocation slashing across fast-path partials (`daemon.rs:499-820`). **K-binding vs main-lane window not wired** (`binding.rs:51-63` defined but unused outside tests). |
| S20 (full E2E) | Only main lane | Same. `four_node_main_lane_commits` exercises main lane. Fast-path lane and LTP lane not exercised end-to-end. `phase_g_admit_and_eject` is `#[ignore]` (`daemon.rs:1680`). |

Net: collapse "rework three sprints" into "ratify two IQs (docs only) +
close one wiring gap (~30 LOC + one integration test)" — roughly 1 week
of correctness work, not 4–6 weeks.

### 2.2 Live-cluster throughput vs. 2026 production baselines

| Chain | Architecture | Demonstrated TPS | Finality | Mainnet status |
|---|---|---:|---:|---|
| **suwappu-dag (this repo, 4-region t3.small)** | Mysticeti-C + dual-ring + ML-DSA | **4,300 submit / 0.125 commit** | (no finality; stalls) | Pre-testnet |
| Solana (Firedancer + Alpenglow) | Sealevel BPF + TowerBFT→Votor | 3k–5k real, 100k peak (Aug 2025) | 400–800ms today → ~150ms (Alpenglow, late 2026) | Mainnet 2020; Firedancer Dec 2025 |
| Sui (Mysticeti v2) | Move + DAG-BFT + FPC built-in | ~15k real | ~390ms (35% latency cut on Asia) | Mainnet May 2023; v2 Nov 2025 |
| Aptos (Baby Raptr → Raptr → Velociraptr) | Move + Block-STM + Quorum Store | 250k bench, ~10k real | 100–150ms faster than 2024 | Mainnet Oct 2022; Baby Raptr 2025 |
| Monad | Parallel EVM + MonadBFT + MonadDB | 10k real, target much higher | 1s | Mainnet Nov 2025 |
| MegaETH (L2; treated as L1 by builders) | Real-time EVM + sequencer | 35k pre-mainnet, target 100k | 10ms blocks / ~1ms sequencer | Mainnet Feb 2026 |
| Hyperliquid (HyperBFT) | HotStuff-pipelined + HyperCore + HyperEVM | 200k (orderbook) | 70ms | Mainnet 2023 |
| Sei Giga | EVM-only (after June 2026 cutover) + Twin-Turbo | 200k devnet | sub-400ms | Mainnet 2023; Giga rolling 2026 |
| Berachain (PoL v2) | EVM + PoL | ~5k | ~2s | Mainnet Feb 2025 |
| Ethereum (Fusaka, Dec 2025) | EVM + PBS + PeerDAS | 60M gas/block, ~50 TPS L1 | 12-min full finality | Long-established |

**Implication.** A 2026 settlement-chain mainnet floor is roughly
**≥5k real TPS with sub-second finality**. suwappu-dag's current commit
cadence of 0.125 TPS (the May-13 perf stall, caused by older code paths
since superseded by IQ-001 + IQ-002 in `main`) is three orders of magnitude
below this floor. Once IQ-001 + IQ-002 land in `main` (they have), Issue
#18 governance asymmetry is fixed, and one more perf campaign is run,
expectation is a step change. The S31 cross-region throughput work
(per-peer inboxes, lock split, parking_lot) is the right scaffolding —
it has not yet been validated against the post-IQ commit rule.

### 2.3 Production-L1 capability matrix (gaps)

| Capability | Sui | Aptos | Solana | Monad | MegaETH | Hyperliquid | **suwappu-dag** | Verdict |
|---|---|---|---|---|---|---|---|---|
| Query RPC | JSON-RPC + GraphQL + gRPC + archival store | REST + Indexer API | JSON-RPC (+Helius) | JSON-RPC | JSON-RPC + Flashblocks WSS | REST + WebSocket | **Write-only TCP bincode** (`suwappu-node/src/client.rs:38-75`) | **Critical** |
| Client SDK | TS, Rust, Python, Go, Swift, Kotlin | TS, Python, Rust | Web3.js, @solana/web3.js v2, Rust | viem/ethers + Rust | viem/ethers + WSS | Python, TS | **None** | **Critical** |
| Block explorer | Suiscan, Suivision | Aptos Explorer | Solana FM, SolanaBeach | MonadScan | MegaScan | Hyperliquid Stats | **NDJSON event log per node only** | **High** |
| Indexer | Sui indexer + archival | Aptos indexer | Helius, Triton | Custom | Self-hosted | Self-hosted | **None** | **High** |
| Mempool / fee market | Yes | Quorum store + gas | Banking-stage + priority | Custom | Native | Orderbook native | **FIFO into `pending_intents`** | **High** |
| Account abstraction | Native (sponsored) | Native | Smart wallet | ERC-4337 + 7702 | ERC-4337 + 7702 | Native | **None** (paper §3.3 mandates ML-DSA-signed intents; not enforced at wire — `client.rs:21-23`) | **High** |
| Bridges (deployed) | Sui Bridge, Wormhole, LayerZero v2, Axelar | LayerZero v2, Wormhole | Wormhole, Allbridge, deBridge, CCIP | LayerZero v2, Wormhole, Stargate | LayerZero v2, native | Native bridge to Arbitrum + USDC | **LTP framework only**, no corridor | **Critical** |
| Restaking / AVS integration | n/a | n/a | n/a (Jito) | n/a | EigenLayer-secured | n/a | **None** — option: register LTP as AVS post-mainnet | Optional |
| Public-key crypto | Ed25519 | Ed25519 | Ed25519 | secp256k1 | secp256k1 | Ed25519 | **ML-DSA-65 + ML-KEM-768 + BLS12-381 + SHA3-256** | Advantage |
| Joint-quorum BFT | No | No | No | No | No | No | **Yes — Theorem 2** | Advantage |
| Cross-chain attestation (constant-size) | No | No | No | No | No | No | **LTP §10.2 — ~1.6 kB regardless of payload** | Advantage |
| Mainnet uptime | 3 years | 3.5 years | 5 years | 6 months | 3 months | 2 years | 0 | by definition |

**Top 5 codebase blockers for mainnet (ordered by criticality):**

1. Ratify IQ-001 + IQ-002 (docs only); land IQ-003 K-binding wire-up
   (~30 LOC plus one integration test).
2. Build a real query RPC (recommend JSON-RPC over HTTP, with an
   eventual gRPC/GraphQL upgrade path matching Sui's Transaction Driver
   pattern). New crate `suwappu-rpc`. Methods: `getBlock`,
   `getTransaction`, `getBalance`, `getAuthorityRegistry`,
   `getEpoch`, `getStake`, `submitIntent` (mirror of
   `ClientMessage::Submit`), `subscribeEvents` (WS tail of NDJSON).
3. Client SDK — Rust + TypeScript. Single crate `clients/rust-sdk`
   and single npm package `@suwappu/client`. Pattern after `viem` for TS.
4. Indexer (NDJSON tail → Postgres or ClickHouse) + minimal Next.js
   explorer. New crate `suwappu-indexer`.
5. Mempool with priority queue, fee model, per-peer rate limit on
   `client.rs::run`, and intent expiry. New crate `suwappu-mempool`.

---

## 3. Competitors' active upgrades — what changes the bar by mainnet day

A practical 2026 mainnet lands into a market where:

- **Solana Alpenglow** (community-cluster May 2026; mainnet via Agave
  4.1 late 2026): TowerBFT replaced by Votor (1–2 round vote, 100ms
  fast / 150ms slow), Proof-of-History removed from the hot path.
  Finality floor on competitor chains drops to **~150ms** by year-end.
- **Solana Firedancer** (full mainnet Dec 2025; running on >20% of
  validators): 1M TPS *target*, ~600k bench-demonstrated, client
  diversity. Raises the "credible high-perf L1" bar.
- **Sui Mysticeti v2** (Nov 2025): consensus + execution fused
  ("Transaction Driver"); pre-consensus validation removed. 35%
  latency reduction. Sui ships *gRPC + GraphQL + archival store*
  alongside JSON-RPC as standard.
- **Aptos Raptr / Baby Raptr** (AIP-106, live 2025): 6→4 network hops
  via merging quorum-store DA into consensus. 100–150ms latency cut on
  mainnet. Roadmap: full Raptr → 250k TPS / 750ms.
- **Ethereum Fusaka** (Dec 2025): PeerDAS (EIP-7594), block gas 36M→60M
  (EIP-7935), blob-base-fee bounding (EIP-7918). 100k+ TPS capacity
  *for L2s anchored on L1 blobs*. **Hegota** (H2 2026) is expected to
  land EIP-8141 — Ethereum's first post-quantum EIP.
- **Avalanche Etna / Avalanche9000 / ACP-77** (Dec 2024): sovereign L1s
  pay a continuous AVAX fee (~1.3 AVAX/month) instead of staking 2,000
  AVAX; deploy cost cut 99.9%. Subnet → L1 rename.
- **MegaETH** (Feb 2026): real-time L1 narrative — 10ms blocks, ~1ms
  sequencer latency, 100k TPS target. EigenLayer-secured.
- **Hyperliquid HyperEVM**: 200k TPS, 70ms block time. >170 projects
  on HyperEVM by March 2026.
- **Sei Giga**: EVM-only cutover June 2026; 200k TPS devnet.
- **Cross-chain landscape (2026):** LayerZero ~75% volume share ($44B+
  total bridged), Wormhole $1B+ daily, CCIP rolling 2.0 with
  customizable risk tier, IBC + Eureka for trust-minimized. ERC-7683
  standardizing intent-based cross-chain.
- **Post-quantum landscape (2026):** Algorand shipped Falcon-1024 on
  mainnet (Nov 2025) — *first* major L1 with deployed PQ. QRL Zond
  shipping ML-DSA-87 EVM-compatible L1 in 2026. Naoris went live April
  2026. Ethereum Foundation formed a PQ team Jan 2026; full PQ infra
  targeted ~2029. **suwappu-dag is no longer alone in the PQ space**; the
  differentiator is the *combined* surface (ML-DSA-65 + ML-KEM-768 +
  BLS12-381 aggregate + SHA3-256) and the *constant-size ≈1.6 kB LTP
  attestation* (paper §10.2) — neither Algorand nor QRL ships both.
- **Account abstraction (2026):** ERC-4337 + EIP-7702 is the dominant
  UX pattern, 40M+ smart accounts. suwappu-dag's paper §3.3 mandates
  ML-DSA-signed intents but the wire (`client.rs:21-23`) accepts
  unsigned intents — must close before mainnet.
- **Data availability (2026):** Celestia 21 MB/s sustained (mamo-1),
  EigenDA 100 MB/s. Modular L2 ecosystem normalizes external DA. If
  suwappu-dag ever needs DA scaling, EigenDA is the path of least
  resistance (Ethereum-aligned, restaking-backed).
- **Shared security:** EigenLayer $18B restaked across 1,900
  operators. Symbiotic + Karak round out the multi-protocol landscape.
  **Optionality for suwappu-dag:** post-mainnet, register the LTP
  attestation service as an AVS — restaking-backed economic security
  for the cross-chain corridor without bootstrapping a separate set of
  validators.

---

## 4. AWS infra state

In the `gsn` account (492042618949, us-east-1):

| Resource | Source | State |
|---|---|---|
| 4 perf t3.small validators (4 regions) | `terraform/perf/` | All stopped. |
| S3 `suwappu-dag-tf-state` + DDB `suwappu-dag-tf-locks` | `terraform/bootstrap/` | Created today. |
| S3 `suwappu-dag-perf-artifacts` | `terraform/perf/main.tf` | ~2.9 GB, versioned, 5 lifecycle rules. |
| CodeBuild `suwappu-perf-musl-build` | `terraform/perf/codebuild.tf` | Active. |
| `terraform plan` | — | Clean. |

Validator keys are deterministic placeholders generated by
`scripts/perf/gen-genesis.py:17`, pulled from S3 via cloud-init
(`terraform/perf/modules/region/cloud-init.yaml:64-66`). **Mainnet
blocker:** Secrets Manager + KMS, or HSM.

Top 7 AWS gaps (ranked):

1. Validator key management
2. Public DNS + TLS termination
3. CloudFront + WAF (+ Shield Advanced)
4. CloudWatch dashboards / alarms / SNS
5. CloudTrail multi-region + S3 data-plane logging + log-file integrity
6. MFA enforcement + tighten over-broad IAM
7. Cross-region snapshot DR + RTO/RPO doc

Baseline AWS hardening: **12–18 engineer-days.**

---

## 5. Mainnet-readiness verdict

Three categories:

- **A. Specification correctness.** ~1 week. IQ-001 + IQ-002 are
  ratification-only; IQ-003 K-binding is ~30 LOC + one integration
  test; Issue #18 is the epoch-boundary governance apply (the right
  fix per `daemon.rs:1674-1677`); remove `#[ignore]` on
  `phase_g_admit_and_eject`; re-run perf campaign to establish a
  post-IQ baseline. *Floor:* the 4-region cluster must commit at
  ≥1k real TPS sustained before any wider exposure.
- **B. User-facing surface.** 8–12 weeks parallelizable. JSON-RPC →
  Rust+TS SDK → indexer → explorer → mempool. Plus account-abstraction
  enforcement (ML-DSA intent signatures at `client.rs`).
- **C. Operational + security hardening.** 12–18 engineer-days
  baseline; security audit (Trail of Bits / Certora) is the long pole
  at 8–12 weeks lead time.

---

## 6. Recommended sequence

**Phase 1 — Consensus correctness + throughput baseline (≈ 4 weeks).**

1. Ratify IQ-001 + IQ-002 (sign-off + `suwappu-papers` PR).
2. Land IQ-003 K-binding cross-check in
   `daemon.rs::handle_fastpath_cert` (call
   `suwappu_fastpath::binding::is_main_lane_consistent` against a snapshot
   of the last 4 committed rounds; emit `slashed` on inconsistency).
   New 4-node integration test
   `fastpath_main_lane_equivocation_slashes_within_1_epoch`.
3. Fix Issue #18 (epoch-boundary governance apply).
4. Remove `#[ignore]` on `phase_g_admit_and_eject`.
5. Run one 7-region perf campaign; target ≥1k TPS sustained commit.

**Phase 2 — User-facing surface (≈ 8–12 weeks, parallel).**

1. `suwappu-rpc` crate — JSON-RPC over HTTP, mirrored on Sui's RPC schema
   for compatibility. Methods: `getBlock`, `getTransaction`,
   `getBalance`, `getAuthorityRegistry`, `getEpoch`, `getStake`,
   `submitIntent`, `subscribeEvents`. Bind from `Daemon::start`.
2. `clients/rust-sdk` + `clients/ts-sdk` (npm `@suwappu/client`,
   viem-style API).
3. `suwappu-indexer` — NDJSON event-log tail → Postgres or ClickHouse;
   GraphQL query API to follow Sui's pattern (gRPC is a P2 upgrade).
4. Next.js explorer hitting the indexer.
5. `suwappu-mempool` — priority queue, fee model, per-peer rate limits on
   `client.rs::run`, intent expiry. Wire into `Daemon` ingress between
   `client.rs` and the round driver's `pending_intents` drain.
6. Account abstraction: enforce ML-DSA signature verification on every
   `Intent::Transfer` in `client.rs::handle_connection` against the
   sender's registered key in `suwappu-authority`. Currently `client.rs:21-23`
   documents this as a mainnet gate but does not enforce it.

**Phase 3 — Bridge + governance (≈ 4–8 weeks, can overlap Phase 2).**

1. Deploy one ETH ↔ suwappu-dag LTP corridor (broadest ecosystem reach;
   ETH/Eureka + LayerZero v2 DVN compatibility is the easiest
   integration story).
2. Phase G extension: parameter-change governance + on-chain voting.
3. Staking reward distribution at epoch boundaries.

**Phase 4 — Mainnet ops (≈ 4–6 weeks).**

1. Real ML-DSA + BLS keygen in Secrets Manager (or YubiHSM2).
2. Public DNS `*.mainnet.suwappu.network` + ACM wildcard.
3. CloudFront + WAF + Shield Advanced.
4. CloudWatch dashboards, SNS → PagerDuty, GuardDuty, MFA, IAM trimming.
5. Cross-region EBS snapshot replication; runbooks (validator
   onboarding, key rotation, emergency stop).
6. **Independent security audit** of `suwappu-consensus`, `suwappu-crypto`,
   `suwappu-fastpath`, `suwappu-ltp`. Vendor shortlist: Trail of Bits
   (consensus + crypto track record), Certora (formal verification
   for joint-quorum invariants), Zellic (PQ crypto track record). Bug
   bounty (Immunefi, $250k–$1M cap).

**Phase 5 — Launch (≈ 12 weeks minimum).**

1. Public testnet for ≥12 weeks (real validators, faucet, real users,
   no real value). Reference: Monad ran a year-plus of public
   testnet; Sui ran 6+ months; Aptos ran 8+ months. **Mainnet on
   <4 weeks of testnet is irresponsible.**
2. Mainnet cut as a tagged release with reproducible genesis.
   Multi-client preferred but not gating (only Solana has meaningful
   client diversity today — Agave + Firedancer).

**Total: 26–38 weeks. Realistic estimate: 9–14 months.**

---

## 7. Verification commands

```bash
# GitHub
unset GH_TOKEN GITHUB_TOKEN
gh pr list --state open
gh issue list --state open
ls docs/iq/

# Spot-check the three corrected claims
sed -n '61,66p' crates/suwappu-consensus/src/commit.rs        # canonical 2f+1
sed -n '126,202p' crates/suwappu-consensus/src/commit.rs       # direct+indirect
sed -n '499,505p' crates/suwappu-node/src/daemon.rs            # FastPath wired
grep -n is_main_lane_consistent crates/suwappu-node/src/*.rs   # expect: 0 hits
grep -n 'mldsa\|ML-DSA\|signature' crates/suwappu-node/src/client.rs
                                                            # expect: 0 enforcement

# AWS drift
cd terraform/perf
AWS_PROFILE=gsn terraform plan \
  -var 'operator_ip_cidrs=["100.15.218.188/32","172.56.220.227/32"]' \
  -var "ssh_public_key=$(cat ~/.ssh/id_ed25519.pub)"

# Key storage today
AWS_PROFILE=gsn aws s3 ls s3://suwappu-dag-perf-artifacts/keys/ --recursive
AWS_PROFILE=gsn aws secretsmanager list-secrets \
  --query 'SecretList[?contains(Name, `suwappu`)]'
```

---

## 8. References

**External (2026 production landscape):**

- Solana Alpenglow + Votor — Anza blog, May 2026 community-cluster.
- Solana Firedancer — Jump Crypto; mainnet Dec 2025.
- Sui Mysticeti v2 — Mysten Labs, Nov 2025.
- Aptos Baby Raptr — AIP-106; Raptr paper arXiv:2504.18649.
- Monad — mainnet Nov 2025, parallel EVM + MonadBFT.
- MegaETH — mainnet Feb 2026; real-time L1.
- Hyperliquid HyperBFT / HyperCore / HyperEVM.
- Ethereum Pectra (May 2025), Fusaka (Dec 2025), Glamsterdam (2026),
  Hegota (H2 2026).
- Avalanche Etna / Avalanche9000 / ACP-77 (Dec 2024).
- Sei Giga (rolling 2026).
- LayerZero v2, Wormhole, Chainlink CCIP 2.0, Axelar, IBC Eureka.
- ERC-4337, EIP-7702.
- Algorand Falcon-1024 (Nov 2025) — first PQ L1.
- QRL Zond ML-DSA-87 (2026).
- Naoris Protocol PQ chain (April 2026).
- EigenLayer ($18B AVS), Symbiotic, Karak — shared security.
- Celestia (21 MB/s sustained), EigenDA (100 MB/s).

**Internal:**

- `SUWAPPUHELPER.md` — sprint backlog table, load-bearing invariants.
- `docs/architecture/sprint-map.md`.
- `docs/iq/IQ-001-quorum-formula.md` — ratified 2026-05-14 ([suwappu-papers#1](https://github.com/suwappu/suwappu-papers/pull/1)).
- `docs/iq/IQ-002-indirect-commit.md` — ratified 2026-05-14 ([suwappu-papers#1](https://github.com/suwappu/suwappu-papers/pull/1)).
- `docs/iq/IQ-003-fast-path-architecture.md` — handler wired; K-binding gap.
- `docs/iq/IQ-004-decide-slot-orphan-window.md` — pending sign-off; tracking [#45](https://github.com/suwappu/suwappu-dag/issues/45).

**Perf history (this repo):**

- `docs/perf-run-2026-05-12/README.md` — 6-region perf-testnet snapshot, pre-S29 RPC batch.
- `docs/perf-run-2026-05-13/README.md` — extended campaign with S29 batch submit + S30 round-driver lock split; the throughput numbers cited in §2.2 of this audit derive from this run.
- `crates/suwappu-consensus/src/commit.rs:61-66` — canonical `2f+1`.
- `crates/suwappu-consensus/src/commit.rs:126-202` — `try_direct_decide`,
  `try_indirect_decide`, `decide_slot`, `finalize`.
- `crates/suwappu-fastpath/src/quorum.rs:38-43` — fast-path quorum.
- `crates/suwappu-fastpath/src/binding.rs:51-63` — `is_main_lane_consistent`
  (defined but unused outside tests).
- `crates/suwappu-node/src/daemon.rs:499-820` — fast-path lane handler +
  proposer.
- `crates/suwappu-node/src/daemon.rs:1650-1680` — `phase_g_admit_and_eject`
  `#[ignore]` rationale.
- `crates/suwappu-node/src/client.rs:21-23,38-75` — write-only client wire
  protocol; auth-deferred-to-mainnet comment.
- `terraform/perf/main.tf` + `modules/region/cloud-init.yaml:64-66`.
- `scripts/perf/gen-genesis.py:17` — placeholder-key disclaimer.
