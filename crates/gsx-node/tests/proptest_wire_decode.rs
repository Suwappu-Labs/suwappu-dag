//! B3 hardening — wire-decode robustness.
//!
//! Feed arbitrary byte slices through bincode-deserialize for the
//! `WireMessage` and client `ClientMessage` shapes. The decoder MUST
//! return `Err`, NEVER panic, NEVER hang, for any input.
//!
//! Why this matters: a peer or client controls the bytes after the
//! 4-byte length prefix. `MAX_FRAME_BYTES` caps allocation; this
//! proptest covers the decode-correctness side — that no malformed
//! input panics inside bincode's recursive descent (e.g., a
//! length-tagged Vec with a tag that says "allocate 4 GiB",
//! arithmetic overflow on a nested-collection length, infinite
//! recursion on a self-referential serde tag).
//!
//! Bincode rejects invalid inputs via `Result::Err`; the value
//! here is documenting the contract + catching any future panic
//! regression.

use gsx_node::{client::ClientMessage, wire::WireMessage};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1024,
        max_shrink_iters: 64,
        .. ProptestConfig::default()
    })]

    /// Fuzz `WireMessage` decode with arbitrary byte slices up to
    /// 64 KiB (the `MAX_COMPACT_MESSAGE_BYTES` cap). Assert: no
    /// panic, no hang — only `Result::Ok(_)` or `Result::Err(_)`.
    #[test]
    fn wire_message_decode_rejects_garbage_without_panic(
        bytes in prop::collection::vec(any::<u8>(), 0..64 * 1024),
    ) {
        let _ = bincode::deserialize::<WireMessage>(&bytes);
    }

    /// Fuzz `ClientMessage` decode with arbitrary byte slices up
    /// to 64 KiB. Same contract: decode is total over `&[u8]`.
    #[test]
    fn client_message_decode_rejects_garbage_without_panic(
        bytes in prop::collection::vec(any::<u8>(), 0..64 * 1024),
    ) {
        let _ = bincode::deserialize::<ClientMessage>(&bytes);
    }
}

/// Empty-input edge case — bincode should error, not panic.
#[test]
fn empty_input_does_not_panic() {
    let _ = bincode::deserialize::<WireMessage>(&[]);
    let _ = bincode::deserialize::<ClientMessage>(&[]);
}

/// All-zero input edge case — common payload-corruption pattern.
#[test]
fn all_zero_input_does_not_panic() {
    let zeros = vec![0u8; 4096];
    let _ = bincode::deserialize::<WireMessage>(&zeros);
    let _ = bincode::deserialize::<ClientMessage>(&zeros);
}

/// Length-tagged amplification — a u64 length prefix claiming a
/// huge Vec. bincode's default config refuses lengths it can't
/// fulfill; verify no panic.
#[test]
fn length_amplification_does_not_panic() {
    // Construct a payload where the first 8 bytes look like a
    // bincode u64 length tag of 1<<48 (Vec claims 281 TB).
    let mut bytes = (1u64 << 48).to_le_bytes().to_vec();
    bytes.extend_from_slice(&[0u8; 4096]);
    let _ = bincode::deserialize::<WireMessage>(&bytes);
    let _ = bincode::deserialize::<ClientMessage>(&bytes);
}
