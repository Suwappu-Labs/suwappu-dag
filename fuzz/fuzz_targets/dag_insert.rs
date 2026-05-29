//! Fuzz target: `DagStore::insert` against bincode-decoded
//! `Certificate` instances.
//!
//! Contract: `DagStore::insert(cert)` is total for every well-typed
//! `Certificate`. Errors are returned as `ConsensusError` variants
//! (UnknownParent, RoundMonotonicityViolation, …); panics are bugs.
//!
//! Drive sequence:
//!   1. Treat the fuzz input as a stream of bincode-encoded
//!      `Certificate`s (length-prefixed, u16 prefix to bound a single
//!      shape ≤ 64 KiB).
//!   2. Decode each in turn; bincode rejects malformed prefixes via
//!      `Err`. Successful decodes go into a fresh `DagStore` via
//!      `insert`.
//!   3. After the stream is exhausted, all inserts are dropped along
//!      with the DAG — no state persists across iterations.
//!
//! Why bincode here (not `arbitrary`-derived structures): bincode is
//! the exact decode surface a wire peer attacks. Generating arbitrary
//! `Certificate` values directly would skip the codec boundary.

#![no_main]

use libfuzzer_sys::fuzz_target;

use gsx_consensus::{Certificate, DagStore};

fuzz_target!(|bytes: &[u8]| {
    let mut dag = DagStore::new();
    let mut cursor = 0usize;
    while cursor + 2 <= bytes.len() {
        let len = u16::from_le_bytes([bytes[cursor], bytes[cursor + 1]]) as usize;
        cursor += 2;
        if cursor + len > bytes.len() {
            break;
        }
        let chunk = &bytes[cursor..cursor + len];
        cursor += len;
        if let Ok((cert, _)) =
            bincode::serde::decode_from_slice::<Certificate, _>(chunk, bincode::config::legacy())
        {
            let _ = dag.insert(cert, "test");
        }
    }
});
