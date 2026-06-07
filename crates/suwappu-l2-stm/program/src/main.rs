//! SP1 guest program for the SUWAPPU L2 STM (Track G G1 /
//! Phase 1.1 follow-up, #82).
//!
//! ## Contract
//!
//! - **Reads** a `suwappu_l2_stm::BatchInput` from SP1 stdin
//!   (`sp1_zkvm::io::read`). The host (the prover daemon,
//!   #104) supplies this via `SP1Stdin::write`.
//! - **Computes** the L2 state transition via the same
//!   `suwappu_l2_stm::execute_batch` the native host runs. Any
//!   divergence here would mean two implementations of the
//!   STM — load-bearing-bad. Sharing the lib is the single
//!   guarantee of native↔guest equivalence.
//! - **Commits** the 240-byte public-input blob via
//!   `sp1_zkvm::io::commit_slice`. The layout matches
//!   `suwappu_l2_verifier_precompile::public_inputs::*` byte-for-
//!   byte; the substrate verifier reads the proof's public
//!   inputs at the exact same offsets.
//!
//! ## Failure semantics
//!
//! If `execute_batch` returns `Err`, the guest panics. SP1
//! treats a panicked guest as "this proof cannot be produced"
//! — the prover daemon catches this and refuses to submit a
//! `CommitL2StateRoot` for the offending batch. That's the
//! desired behavior: invalid batches must never produce a
//! valid proof.
//!
//! ## Build
//!
//! From `crates/suwappu-l2-stm/program/`:
//!
//! ```sh
//! cargo prove build --output-directory ../elf
//! ```
//!
//! Produces `../elf/suwappu-l2-stm-program` (the ELF) that the
//! prover daemon loads into `sp1_sdk::ProverClient`.
//!
//! ## Equivalence harness
//!
//! A native↔guest equivalence test (running the same
//! `BatchInput` through both `execute_batch` and the guest's
//! `execute()` path under SP1 emulation, asserting identical
//! public-input output) lands in a follow-up PR alongside the
//! `sp1-sdk` dep. The shared lib makes the property hold by
//! construction; the harness catches regressions.

#![no_main]

sp1_zkvm::entrypoint!(main);

use suwappu_l2_stm::{execute_batch, to_public_inputs, BatchInput};

pub fn main() {
    // Read the BatchInput. SP1's serde + bincode path
    // round-trips the same shape that `BatchInput::Serialize`
    // produces on the host side, so the host can simply
    // `stdin.write(&batch_input)` without bespoke encoding.
    let input: BatchInput = sp1_zkvm::io::read();

    // Run the STM. Panic on Err is the desired behavior — see
    // "Failure semantics" above.
    let output = execute_batch(&input).expect("STM rejected batch");

    // Commit the 240-byte public-input blob. The substrate's
    // verifier (`suwappu-l2-verifier-precompile::verify_l2_batch`)
    // reads back at the offsets defined in
    // `suwappu_l2_verifier_precompile::public_inputs::*`.
    let public_inputs = to_public_inputs(&input, &output);
    sp1_zkvm::io::commit_slice(&public_inputs);
}
