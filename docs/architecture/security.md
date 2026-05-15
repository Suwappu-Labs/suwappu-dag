# Security surface

Operator-facing catalog of the gsx-dag security defenses + how to
exercise / extend them. Paired with
[`cryptographic-posture.md`](cryptographic-posture.md) (which covers
the PQ cryptographic primitives) and [`safety-liveness.md`](safety-liveness.md)
(joint-quorum AND-gate proof). This doc is the ingress + tooling view.

## Ingress hardening

### Client TCP wire (`crates/gsx-node/src/client.rs`)

| Defense | Default | Source of truth |
|---|---|---|
| Global concurrent-connection cap | 256 | `NodeConfig::max_client_connections` |
| Per-source-IP concurrent-connection cap | 8 | `NodeConfig::client_per_ip_limit` |
| Idle-frame timeout | 30 s | `NodeConfig::client_idle_timeout_ms` |
| ML-DSA-65 signature gate on every intent | always on | `verify_signed_intent` (Issue #28) |
| Per-peer leaky-bucket rate limit via mempool | 100 burst / 50 tokens/s | `gsx_mempool::MempoolConfig` |

### Validator peer wire (`crates/gsx-node/src/wire.rs`)

| Defense | Default | Source |
|---|---|---|
| Frame size cap (outer length-prefix) | 1 MiB | `MAX_FRAME_BYTES` |
| Compact-variant size cap | 64 KiB | `MAX_COMPACT_MESSAGE_BYTES` (B3) |
| Per-peer inbox channel | 1024-slot tokio mpsc | `daemon::run_inbox` |
| Orphan-cert buffer cap | 4096 entries | `MAX_ORPHAN_CERTS` |
| Per-orphan exponential backoff | 500 ms → 5 s cap | `orphan_pull_backoff_ms` (DAG-S32) |

### JSON-RPC ingress (`crates/gsx-rpc/src/router.rs`)

| Defense | Default | Source |
|---|---|---|
| Request body size limit | 1 MiB | `RouterLimits::max_request_body_bytes` (B2) |
| Global in-flight concurrency cap | 64 | `RouterLimits::max_concurrent_requests` (B2) |
| Reserved `RateLimited` error code | -32099 | `RpcError::RateLimited` (B2, no middleware emits yet) |
| Per-IP rate limit | pending | follow-up: `tower-governor` evaluation OR custom layer in B2.1 |

## Fuzz targets

`fuzz/` is a cargo-fuzz workspace member. Targets:

| Target | Surface | Why |
|---|---|---|
| `wire_decode` | `bincode::deserialize::<{WireMessage,ClientMessage}>` | Bincode decode of attacker-controlled bytes — total over `&[u8]` |
| `dag_insert` | `DagStore::insert` | Total over decoded `Certificate` |
| `decide_slot` | `gsx_consensus::decide_slot` | Total over `(DagStore, Round, CommitteeSize)`; covers IQ-004 multi-anchor scan |

### Running locally

Requires nightly rustc + cargo-fuzz:

```sh
rustup toolchain install nightly
cargo install cargo-fuzz --locked

cd fuzz
cargo +nightly fuzz run wire_decode -- -max_total_time=60
cargo +nightly fuzz run dag_insert  -- -max_total_time=60
cargo +nightly fuzz run decide_slot -- -max_total_time=60
```

A crash drops the input under `fuzz/artifacts/<target>/`; re-run with
that input as a positional argument to reproduce.

### Running in CI

`.github/workflows/fuzz.yml` runs all three targets on a weekly cron
(Sunday 03:00 UTC) and on manual dispatch. Each target runs for
`FUZZ_DURATION_SECS` (default 300 s = 5 min/target). Crashes surface
as workflow failures with the crashing input attached as a
GitHub-Actions artifact for repro.

The corpus (`fuzz/corpus/<target>/`) is cached between runs so the
next run picks up where the last left off.

## Cryptographic surface

See [`cryptographic-posture.md`](cryptographic-posture.md) for the
PQ-conservative claim. Quick summary:

- **ML-DSA-65** (FIPS 204): client intent auth + Authority cert signing.
- **ML-KEM-768** (FIPS 203): LTP envelope encryption.
- **BLS12-381**: aggregate cert + LTP super-node co-signature
  (classical, with documented migration target).
- **SHA3-256**: transport hash + LTP payload root.

## Slashing surfaces

- **Fast-path equivocation**: 100% bonded stake + Authority Ring
  expulsion. See [fast-path.md](fast-path.md) and IQ-003.
- **Authority equivocation at the cert level**: detected by
  `detect_authority_equivocation` (DAG-S30.1); slashing emitted in
  `try_commit`.

## Audit posture

- Latest snapshot: [`audit/mainnet-readiness-2026-05-14.md`](../audit/mainnet-readiness-2026-05-14.md).
- B5 will refresh this snapshot after Track B closes.
- External audit recruitment is out of scope for this iteration — see
  the master plan at `~/.claude/plans/research-how-to-starry-floyd.md`
  for the path to mainnet.

## Cross-references

- [`cryptographic-posture.md`](cryptographic-posture.md) — PQ + classical primitives + exception zones.
- [`safety-liveness.md`](safety-liveness.md) — joint-quorum AND-gate (Theorem 2).
- [`../iq/IQ-004-decide-slot-orphan-window.md`](../iq/IQ-004-decide-slot-orphan-window.md) — the consensus-side fix `decide_slot` fuzz now exercises.
- Skill bank: `~/.claude/skills/dag-decide-slot-single-cert-orphan-after-parent-set-frozen` — operator playbook for the orphan-window failure mode.
