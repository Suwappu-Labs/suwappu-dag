//! Fuzz target: `decide_slot` against arbitrary DAG topologies built
//! from bincode-decoded `Certificate` streams.
//!
//! Contract: `decide_slot(&dag, round, n)` is total for any
//! `(DagStore, Round, CommitteeSize)`. No panic, no infinite loop.
//! Output is `LeaderStatus::Direct(_) | Skip | Undecided`.
//!
//! Drive sequence:
//!   1. Reuse the dag-insert decode loop to build a `DagStore` from
//!      the fuzz input.
//!   2. Pull two u8s from the tail as `(n, round_lo)`. `n` is clamped
//!      to `[1, 32]` so the round-robin leader function doesn't panic
//!      on `n == 0`. `round_lo` selects which round in the DAG to
//!      probe.
//!   3. Call `decide_slot(&dag, round, n)` — must return any of the
//!      three `LeaderStatus` variants without panicking.
//!
//! Exercises the IQ-004 multi-anchor scan in `decide_slot`: the
//! corpus will explore DAG topologies where the first directly-decided
//! anchor's `causal_history` excludes the target leader but a later
//! anchor's includes it (and vice versa).

#![no_main]

use libfuzzer_sys::fuzz_target;

use suwappu_consensus::{decide_slot, Certificate, DagStore};

fuzz_target!(|bytes: &[u8]| {
    if bytes.len() < 2 {
        return;
    }
    let n_raw = bytes[0];
    let round_lo = bytes[1];
    let body = &bytes[2..];

    let mut dag = DagStore::new();
    let mut cursor = 0usize;
    while cursor + 2 <= body.len() {
        let len = u16::from_le_bytes([body[cursor], body[cursor + 1]]) as usize;
        cursor += 2;
        if cursor + len > body.len() {
            break;
        }
        let chunk = &body[cursor..cursor + len];
        cursor += len;
        if let Ok((cert, _)) =
            bincode::serde::decode_from_slice::<Certificate, _>(chunk, bincode::config::legacy())
        {
            let _ = dag.insert(cert, "test");
        }
    }

    // Clamp `n` to [1, 32] to avoid panicking the round-robin
    // `leader(round, n)` function on n == 0. Real committees are
    // typically 4..=50; 32 is generous headroom.
    let n = (n_raw % 32).max(1) as u32;
    let round = round_lo as u64;
    let _ = decide_slot(&dag, round, n);
});
