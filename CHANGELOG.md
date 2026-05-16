# Changelog

All notable changes to this project. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). The
release workflow (`.github/workflows/release.yml`) extracts each
`## <version>` section verbatim as the GitHub Release notes.

## Unreleased

### Added

- Devnet hosting infrastructure (`terraform/devnet/`, G1) — 4-region
  always-on stack with persistent EBS and public RPC. Apply-ready;
  not yet deployed.
- Release-binary workflow (`.github/workflows/release.yml`, G4) —
  publishes tarballed `gsx-node` + `gsx-loadgen` + `gsx-indexer`
  + `gsx-faucet` (when present) on `gsx-dag-v*` tags for
  x86_64-linux-musl, x86_64-darwin, aarch64-darwin.

### Pending (post-F1–F4 program → devnet hosting program)

- G2: public RPC endpoint with DNS + TLS + ALB + WAF.
- G3: `gsx-faucet` service.
- G5: `OPERATIONS.md` runbook.
- G6: Prometheus `/metrics` on `gsx-node` + CloudWatch dashboard + alarms.
- G7: block explorer SPA.
- G8: status page.

## 0.1.0 — Initial milestone (not yet tagged)

Captures the state of the codebase after PRs #44 → #73:

### Added

- Mysticeti-C DAG consensus (DAG-S1 → S20).
- ML-DSA-65 + ML-KEM-768 + BLS12-381 + SHA3-256 crypto surface.
- Joint-quorum AND-gate safety (Theorem 2).
- Fast-path lane with K=4 equivocation binding (100% slashing).
- Constant-size LTP attestation (≈1,600 B regardless of payload).
- JSON-RPC + WebSocket API (`crates/gsx-rpc/`) — 8 read methods +
  `submit_intent` + `subscribe_events`.
- Rust SDK (`clients/rust-sdk/`) + TypeScript SDK
  (`clients/ts-sdk/`).
- Streaming indexer (`crates/gsx-indexer/`) with Postgres backend
  + F2 startup catch-up backfill.
- Per-IP rate limit (F1, `crates/gsx-rpc/src/per_ip.rs`).
- bincode 2.x + 1-byte wire-frame version marker (F4,
  `crates/gsx-node/src/codec.rs`).
- `#[non_exhaustive]` on `Intent` and `RpcError` (C4).
- `DEVNET.md` local 4-node docker-compose devnet (C1).
- 4 Rust + 3 TS starter examples (C2).
- `CONTRIBUTING.md` + `SECURITY.md` (C3 partial).
- rustdoc + TypeDoc publishing workflow (C4).
- cargo-fuzz workspace with `wire_decode`, `dag_insert`,
  `decide_slot` targets (B4).

See `docs/iq/` for the ratified investigation questions (IQ-001
through IQ-005) and `docs/audit/mainnet-readiness-2026-05-15.md`
for the current security + ops posture.
