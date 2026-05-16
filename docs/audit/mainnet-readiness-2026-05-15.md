# gsx-dag mainnet-readiness audit (2026-05-15)

## Context

Refresh of the 2026-05-14 audit snapshot after one day of focused
Track A (build) + Track B (security harden) work landed via the
14-PR program at `~/.claude/plans/research-how-to-starry-floyd.md`.
Supersedes the May-14 doc on the following points where state has
materially changed:

1. Issue #18 was claimed open; it's been fixed (#32 deferred-activation
   + #18 epoch-boundary; see IQ-002 ratification).
2. K-binding cross-check was claimed unwired; it ships at
   `crates/gsx-node/src/daemon.rs:837-872` with integration test
   `K_binding_violator_is_slashed` at line 2556+ (PR A2 documents).
3. Account-abstraction enforcement was claimed unwired; the ML-DSA-65
   client-wire gate landed in #28 (`verify_signed_intent`).
4. Mempool was claimed not integrated into the round driver; PR A3
   (#55) wires it via `state.mempool.drain_for_block`.
5. SDKs were claimed at 40% complete; both Rust and TS now expose all
   8 JSON-RPC methods. TS adds `subscribeEvents` over WebSocket
   (PR A4 / #56).
6. Indexer was in-memory only; PR A5 (#57) adds the Postgres backend
   behind the `postgres` feature flag.
7. Client TCP listener lacked slow-loris / connection-cap; PR B1
   (#58) adds three defenses with `NodeConfig` knobs.
8. JSON-RPC ingress lacked body-size + concurrency caps; PR B2 (#59)
   adds them via tower middleware.
9. Wire per-cert size cap was unbounded; PR B3 (#60) adds
   `MAX_COMPACT_MESSAGE_BYTES = 64 KiB`.
10. No fuzzing infrastructure; PR B4 (#61) adds cargo-fuzz with three
    targets + weekly CI run.

## 1. Code feature parity vs. the 2026 production landscape — DELTA

Diff against the May-14 capability matrix:

| Capability | May-14 verdict | May-15 verdict |
|---|---|---|
| Query RPC | Write-only TCP bincode (`Critical` blocker) | **JSON-RPC MVP shipped**, 8 methods + WS subscribe live |
| Client SDK | None (`Critical`) | **Rust + TS at full parity**, 8 methods + WS in both |
| Block explorer | None (`High`) | Still missing — out of scope until indexer matures |
| Indexer | None (`High`) | **Scaffold + Postgres backend shipped**; backfill via `gsx_getBlock` still missing |
| Mempool | FIFO into `pending_intents` (`High`) | **gsx-mempool wired**: per-peer rate limit + priority + dedup + TTL |
| Account abstraction | Not enforced at wire (`High`) | **Enforced**: `verify_signed_intent` on client wire + JSON-RPC ingress |
| Deployed bridges | LTP framework only (`Critical`) | Unchanged — out of scope until corridor super-node infra lands |
| Restaking / AVS integration | n/a | n/a |
| Public-key crypto | ML-DSA-65 + ML-KEM-768 + BLS12-381 + SHA3-256 (advantage) | Unchanged |
| Joint-quorum BFT | Theorem 2 (advantage) | Unchanged + **IQ-004 multi-anchor scan** closes the orphan-window liveness gap (PR A1) |
| Cross-chain constant-size attestation | LTP §10.2 (advantage) | Unchanged |

## 2. Ingress hardening posture — NEW (post-B1..B4)

The May-14 audit explicitly flagged the client listener as a
slow-loris attack surface; this is now hardened.

### Client TCP wire (`crates/gsx-node/src/client.rs`)

| Defense | Default | NodeConfig knob |
|---|---|---|
| Global concurrent-connection cap | 256 | `max_client_connections` |
| Per-source-IP concurrent-connection cap | 8 | `client_per_ip_limit` |
| Idle-frame timeout | 30 s | `client_idle_timeout_ms` |
| ML-DSA-65 signature gate on every intent | always | `verify_signed_intent` (Issue #28) |
| Per-peer leaky bucket (via mempool admission) | 100 burst / 50 tok-s | `gsx_mempool::MempoolConfig` |

### Validator peer wire (`crates/gsx-node/src/wire.rs`)

| Defense | Default | Source |
|---|---|---|
| Frame size cap (outer prefix) | 1 MiB | `MAX_FRAME_BYTES` |
| Compact-variant size cap | 64 KiB | `MAX_COMPACT_MESSAGE_BYTES` (B3) |
| Per-peer inbox channel | 1024 slots | `daemon::run_inbox` |
| Orphan-cert buffer cap | 4096 entries | `MAX_ORPHAN_CERTS` |
| Per-orphan exponential backoff | 500 ms → 5 s cap | `orphan_pull_backoff_ms` (DAG-S32) |

### JSON-RPC ingress (`crates/gsx-rpc/src/router.rs`)

| Defense | Default | Source |
|---|---|---|
| Request body size limit | 1 MiB | `RouterLimits::max_request_body_bytes` (B2) |
| Global in-flight concurrency cap | 64 | `RouterLimits::max_concurrent_requests` (B2) |
| Reserved `RateLimited` error code | -32099 | `RpcError::RateLimited` (B2) |
| Per-IP rate limit | TODO | follow-up B2.1 |

## 3. Cryptographic posture — UNCHANGED

ML-DSA-65 / ML-KEM-768 / BLS12-381 / SHA3-256 surface unchanged from
May-14. Documented in
[`docs/architecture/cryptographic-posture.md`](../architecture/cryptographic-posture.md).
Three accepted cargo-deny exceptions all carry `Re-check:` dates as
of B5 (this PR).

## 4. Fuzz coverage — NEW

`fuzz/` ships as a separate cargo-fuzz project (`exclude = ["fuzz"]`
in workspace root). Three targets, weekly scheduled CI:

| Target | Surface |
|---|---|
| `wire_decode` | `gsx_node::codec::decode_frame::<{WireMessage, ClientMessage}>` (F4) |
| `dag_insert` | `DagStore::insert` against bincode-decoded `Certificate` streams |
| `decide_slot` | `gsx_consensus::decide_slot` over arbitrary DAG topologies (exercises IQ-004 multi-anchor scan) |

See [`docs/architecture/security.md`](../architecture/security.md) for
the operator catalog.

## 5. IQ ratification posture — UPDATED

| IQ | Topic | May-14 | May-15 |
|---|---|---|---|
| IQ-001 | Quorum formula | Ratified via gsx-papers#1 | Unchanged |
| IQ-002 | Indirect commit | Ratified via gsx-papers#1 | Unchanged |
| IQ-003 | Fast-path lane | Pending sign-off | **Ratified** (PR A2 confirms wired) |
| IQ-004 | `decide_slot` orphan window | (didn't exist on May-14) | **Ratified** — Option A multi-anchor scan shipped (PR A1) |

## 6. Mainnet-readiness verdict — UPDATED

The May-14 audit's "9–14 months from mainnet" estimate stands, but
the consensus-correctness phase that opened it is now **closed**:

- All four open IQs ratified.
- Account-abstraction gate live on both wires.
- Ingress hardening live on all three boundaries (TCP / wire / RPC).
- Fuzz infrastructure ready for sustained adversarial coverage.

Remaining time-to-mainnet is concentrated in:

1. **User-facing surfaces** (~8-12 weeks parallelizable): block
   explorer UI, indexer backfill, examples/SDK docs (Track C of the
   active plan addresses the SDK + devnet onramp; explorer + bridge
   are out of scope).
2. **Ops hardening** (~12-18 days): deployment automation, key
   ceremony procedures, monitoring/alerting baseline.
3. **External security audit** (8-12 weeks lead time): can be
   initiated now that the post-Track-B surface is stable.
4. **Public testnet** (12+ weeks minimum): genesis ceremony, operator
   recruitment, sustained 4-region 5k-TPS perf campaign.
5. **bincode 2.x migration** (~~pre-mainnet blocker~~ — **shipped F4
   on 2026-05-16**, see [IQ-005](../iq/IQ-005-bincode-2x-migration.md)).
   Workspace flipped to bincode 2.x with `config::legacy()` for byte
   parity; 1-byte wire-frame version marker (`FRAME_VERSION_V1`)
   added so future codec flips are detectable. `RUSTSEC-2025-0141`
   ignore removed from `deny.toml`.

## 7. References

**External:** unchanged from May-14 (Solana Alpenglow, Sui Mysticeti
v2, Aptos Baby Raptr, Monad, MegaETH, Hyperliquid, Ethereum Fusaka,
Avalanche9000, Sei Giga, Celestia, EigenDA, Algorand Falcon-1024,
QRL Zond, Naoris).

**Internal:**

- `CLAUDE.md` — sprint backlog table, load-bearing invariants.
- `docs/architecture/sprint-map.md` — sprint dependency DAG.
- `docs/architecture/security.md` (NEW) — ingress + fuzz catalog.
- `docs/iq/IQ-001-quorum-formula.md` — ratified 2026-05-14.
- `docs/iq/IQ-002-indirect-commit.md` — ratified 2026-05-14.
- `docs/iq/IQ-003-fast-path-architecture.md` — ratified 2026-05-15.
- `docs/iq/IQ-004-decide-slot-orphan-window.md` — ratified 2026-05-15.

**Perf history:**

- `docs/perf-run-2026-05-12/README.md` — 6-region snapshot, pre-S29.
- `docs/perf-run-2026-05-13/README.md` — extended campaign with S29
  batch submit + S30 round-driver lock split. **Stale post-Track-A** —
  next campaign needs to run with mempool-integrated round driver
  + IQ-004 multi-anchor scan + hardened ingress to validate the
  full post-Track-A path. Tracked as a follow-up perf sprint.

**Tracked code paths (post-Track-A + post-B1..B4):**

- `crates/gsx-consensus/src/commit.rs::decide_slot` — IQ-004
  multi-anchor scan (PR A1).
- `crates/gsx-node/src/daemon.rs::handle_fastpath_cert:837-872` —
  K-binding cross-check (PR A2 ratifies).
- `crates/gsx-node/src/daemon.rs::run_round_driver:1551` — mempool
  drain (PR A3).
- `crates/gsx-node/src/client.rs::run` — semaphore + per-IP map
  + idle timeout (PR B1).
- `crates/gsx-rpc/src/router.rs::router_with_limits` — tower
  middleware (PR B2).
- `crates/gsx-node/src/wire.rs::enforce_compact_variant_cap` —
  per-variant size cap (PR B3).
- `fuzz/` — cargo-fuzz workspace (PR B4).

## 8. Open items / known unknowns

- ~~**Per-IP JSON-RPC rate limit**~~ — **shipped F1 on 2026-05-15**
  ([PR #67](https://github.com/GlobalSettlementNetwork/gsx-dag/pull/67)):
  `PerIpRateLimiter` reuses `gsx_mempool::LeakyBucket`; defaults
  60 burst / 10 req/s per IP; emits `RpcError::RateLimited`
  (`-32099`) inside a JSON-RPC envelope.
- ~~**Indexer backfill**~~ — **shipped F2 on 2026-05-15**
  ([PR #68](https://github.com/GlobalSettlementNetwork/gsx-dag/pull/68)):
  `gsx_indexer::backfill::catch_up` walks `gsx_getBlock` from
  `Store::latest_round()` to the new `EpochView.latest_committed_round`
  chain-head field; idempotent against existing rows.
- ~~**bincode 2.x migration**~~ — **shipped F4 on 2026-05-16**
  (this audit's IQ-005). Workspace flipped to bincode 2.x with
  `config::legacy()` for byte parity; 1-byte
  `FRAME_VERSION_V1 = 0x01` marker prepended to wire-going frames
  so future codec flips fail-fast on
  `FrameDecodeError::UnknownVersion`.
- **`phase_g_admit_and_eject` flake** — F3 (shipped 2026-05-15,
  [PR #69](https://github.com/GlobalSettlementNetwork/gsx-dag/pull/69))
  added a 30-second pre-convergence propagation probe that
  bisects the flake into propagation-class vs boundary-drain
  class. The structural fix (cert-broadcast retry on orphan-pull
  path, or a CI-only round-barrier) is gated on CI data once
  the new diagnostic ships.
- **External security audit** — not initiated. Recommend kickoff
  once Track C ships (devnet + examples + governance docs) so an
  auditor has the full public-facing surface to review.
