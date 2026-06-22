# Overview

## Why this exists

The institutional asset market for tokenized real-world assets, stablecoins,
and central-bank digital instruments needs an infrastructure that
simultaneously delivers:

1. **Programmability** of public chains,
2. **Compliance trust** of permissioned institutional networks, and
3. **Cross-chain reach** without custodial bridges.

No single architectural decision yields all three. The SUWAPPU DAG L1 design rests
on three interlocking commitments:

1. **Dual-ring validator set** (§5) — Authority Ring (PoA, 30–50 licensed
   institutional entities) + Validator Ring (PoS, 100–500 stake-weighted open
   participants). Both rings operate over a Mysticeti-style certificate DAG.
2. **Co-resident dual virtual machine** (§7) — EVM (over Reth/REVM) +
   permissioned Move VM, both over a single polymorphic balance map.
3. **Lattice Transfer Protocol** (§10) — corridor super-node attestation
   quorum sealed in NIST-standardized post-quantum cryptography.

## Four-layer stack

```text
┌─────────────────────────────────────────────────────────────┐
│ 4. Application — registered-issuer precompile, Issuer       │
│    Studio, Compliance Extension, policy-vocabulary engine   │
│    (paper §8 → suwappu-precompiles)                             │
├─────────────────────────────────────────────────────────────┤
│ 3. Execution — co-resident dual VM over polymorphic balance │
│    map (paper §7 → suwappu-execution + suwappu-db)                  │
├─────────────────────────────────────────────────────────────┤
│ 2. Consensus — Mysticeti-C certificate DAG with deterministic│
│    BFT linearization (paper §6 → suwappu-consensus)             │
├─────────────────────────────────────────────────────────────┤
│ 1. Data availability + attestation — LTP Commitment Nodes   │
│    under governed SLAs (paper §10 → suwappu-ltp)                │
└─────────────────────────────────────────────────────────────┘
```

All four layers run on a single chain.

## Crate boundaries

```text
                suwappu-node (binary)
                     │
        ┌────────────┼────────────────┬──────────┐
        ▼            ▼                ▼          ▼
   suwappu-consensus  suwappu-execution  suwappu-fastpath  suwappu-precompiles
        │            │                │          │
        │            │                │          ▼
        │            │                │      suwappu-ltp
        │            │                │          │
        └─────┬──────┴────────┬───────┴──────────┘
              ▼               ▼
        suwappu-authority + suwappu-validator      suwappu-transport
              │                                  │
              └────────────────┬─────────────────┘
                               ▼
                          suwappu-crypto
```

Each crate maps 1:1 to a paper section. Crate boundaries are also the
**review boundaries** for specialist subagents (see `CLAUDE.md`).

## What's structural, what's swappable

| Layer | Phase-1 impl | Production swap | Swap point |
|---|---|---|---|
| Consensus base | Mysticeti-C v1 | Mysticeti v2 [Sui, 2025] | `suwappu-consensus` upstream |
| Transport | tokio TCP (dev) | SCION path-authenticated | `suwappu-transport` |
| Authority signatures | ML-DSA-65 (pure) | ML-DSA-65 (pure, unchanged) | n/a |
| EVM TX signing | ECDSA secp256k1 hybrid with ML-DSA-65 | Pure ML-DSA-65 by ~2030 | Account-signing surface |
| LTP aggregate sig | BLS12-381 | Hash-based + SP1-STARK | `suwappu-ltp::aggregate_signature` |
| LTP verification | FRI default, Groth16 opt-in | FRI only by ~2030 | `suwappu-ltp::verification_mode` |
| State substrate | `suwappu-db` Phase-1 | `suwappu-db` launch-readiness (real Move + IPA Verkle + Solidity registry) | `suwappu-execution` |

The trait surfaces are the load-bearing structure. Every property test runs
against the trait, not the impl, so all sprint exit gates stay green under
any of the swaps above.

## What the chain commits to per block

After every Mysticeti round:

1. **Linearized order** of certificates produced into the DAG.
2. **Joint state commitment** `(Σ_EVM, Σ_Move)` — the BLAKE3-rooted state
   tree of `suwappu-db`, co-signed by the Authority Ring at checkpoint cadence.
3. **LTP attestation** for every registered corridor — 1,600-byte commitment
   on each downstream chain.
4. **DAG block hash** linked to the previous block via `parent_anchor`.

All four are deterministic functions of the same input (seeded state + ordered
intents). Recovery via `replay` reproduces all four bit-for-bit (inherited
from `suwappu-db` S8).
