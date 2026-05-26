# gsx-l2-stm-program

SP1 guest program for the GSX L2 state-transition function.
Track G G1 / Phase 1.1 follow-up (#82).

This is a **standalone Cargo project** that targets the
`riscv32im-succinct-zkvm-elf` triple. It is **excluded from
the gsx-dag workspace** so the host build doesn't need the
zkVM toolchain.

## What it does

The guest reads a `gsx_l2_stm::BatchInput` from SP1's stdin,
runs `execute_batch`, and commits the 240-byte public-input
blob via `to_public_inputs`. Same Rust as the native host —
the shared lib is the equivalence guarantee.

The substrate-side verifier (`gsx-l2-verifier-precompile`)
consumes the Groth16 BN254 wrap of the resulting proof at the
matching public-input offsets.

## Build

Requires the [SP1 toolchain](https://docs.succinct.xyz/getting-started/install.html):

```sh
curl -L https://sp1up.succinct.xyz | bash
sp1up
```

Then, from this directory:

```sh
cargo prove build --output-directory ../elf
```

The resulting `../elf/gsx-l2-stm-program` ELF is what the
prover daemon (Phase 2.1, #104) loads into `sp1_sdk::ProverClient`.

## Run (native emulation)

For development / debugging without proof generation:

```sh
# From the gsx-dag repo root
cargo prove run --release --release-elf ../elf/gsx-l2-stm-program
```

## Native ↔ guest equivalence

The shared `gsx-l2-stm` lib (`../`) guarantees that the
guest's `execute_batch` is byte-identical to the host's. The
equivalence harness — which runs the same `BatchInput` through
both paths and asserts `to_public_inputs(input, output)`
matches — lands in a separate PR alongside the `sp1-sdk` dep
(needs the SP1 toolchain to be CI-installable, which we'll
gate on the prover daemon's #104 PR).

## Where this fits in the L2 stack

```
        L2 user                          gsx-l2-bridge (relayer)
            │                                       ▲
            ▼                                       │
   gsx-l2-sequencer (Phase 2.2)                     │
            │  - mempool, batch build               │
            │  - force_include::evaluate            │
            ▼                                       │
        BatchInput                                  │
            │                                       │
            ▼                                       │
  ┌────────────────────────┐                        │
  │  gsx-l2-stm (host lib) │  ←──  shared lib       │
  └────────────────────────┘                        │
            │                                       │
            ▼                                       │
   gsx-l2-stm-program (THIS)  →  ELF  →  prover daemon (Phase 2.1)
            │                              │
            ▼                              ▼
   240 B public inputs              Groth16 BN254 proof (260 B)
                                           │
                                           ▼
                          gsx-l2-verifier-precompile (#97)
                                           │
                                           ▼
                          substrate apply_intent::CommitL2StateRoot
```

## See also

- `../src/lib.rs` — the shared STM lib.
- `../tests/proptest_stm.rs` — host-side property tests.
- `crates/gsx-l2-verifier-precompile/src/lib.rs` — the on-chain
  verifier this guest's proofs are consumed by.
- `docs/architecture/l2-proof-system-selection.md` — why SP1.
