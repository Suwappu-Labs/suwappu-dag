# Table of contents

## Getting Started

* [Overview](README.md)
* [Chain README](../README.md)
* [Visuals](visuals/README.md)
* [Contributing](../CLAUDE.md)

## Architecture

* [Section index](architecture/README.md)
* [Overview — four-layer stack (§4)](architecture/overview.md)
* [Validator rings (§5)](architecture/validator-rings.md)
* [Consensus — Mysticeti-C DAG (§6)](architecture/consensus.md)
* [Transport — SCION + RaptorQ (§6.3)](architecture/transport.md)
* [Fast path — single-owner + K-binding (§6.4)](architecture/fast-path.md)
* [Execution — dual VM (§7)](architecture/execution.md)
* [suwappu-db bridge — substrate dep boundary (§7)](architecture/suwappu-db-bridge.md)
* [Application — precompiles (§8)](architecture/application.md)
* [Super-node — 7-of-9 (§9)](architecture/super-node.md)
* [LTP integration — Commit/Lattice/Materialize (§10)](architecture/ltp-integration.md)
* [Safety + liveness — joint-quorum AND-gate (§11)](architecture/safety-liveness.md)
* [Cryptographic posture (§12)](architecture/cryptographic-posture.md)
* [Governance phasing — G2 → G3 → G4 (§14)](architecture/governance-phasing.md)
* [Sprint map](architecture/sprint-map.md)

## Investigation Questions

* [IQ index](iq/README.md)
* [IQ-001 — Quorum formula](iq/IQ-001-quorum-formula.md)
* [IQ-002 — Indirect commit](iq/IQ-002-indirect-commit.md)
* [IQ-003 — Fast-path architecture](iq/IQ-003-fast-path-architecture.md)
* [IQ-004 — decide_slot orphan window](iq/IQ-004-decide-slot-orphan-window.md)

## Research

* [Competitive gap analysis — payments & settlement chains (2026-07)](research/competitive-gap-analysis.md)
* [Brief — Tempo (Stripe/Paradigm)](research/briefs/tempo.md)
* [Brief — Arc (Circle)](research/briefs/arc.md)
* [Brief — Robinhood Chain](research/briefs/robinhood-chain.md)
* [Brief — Payments-chain landscape](research/briefs/landscape.md)

## Audit & Perf

* [Mainnet readiness 2026-05-14](audit/mainnet-readiness-2026-05-14.md)
* [Perf run 2026-05-12 (6-region)](perf-run-2026-05-12/README.md)
* [Perf run 2026-05-13 (extended)](perf-run-2026-05-13/README.md)
