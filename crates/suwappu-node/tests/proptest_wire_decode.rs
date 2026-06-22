//! B3 hardening — wire-decode robustness.
//!
//! Feed arbitrary byte slices through `codec::decode_frame` for the
//! `WireMessage` and `ClientMessage` shapes (the two types that go on
//! the wire). The decoder MUST return `Err`, NEVER panic, NEVER hang,
//! for any input.
//!
//! Why this matters: a peer or client controls the bytes after the
//! 4-byte length prefix. `MAX_FRAME_BYTES` caps allocation; this
//! proptest covers the decode-correctness side — that no malformed
//! input panics inside bincode's recursive descent (length-tagged
//! Vec with a "allocate 4 GiB" tag, arithmetic overflow on a
//! nested-collection length, infinite recursion on a self-referential
//! serde tag, an unknown F4 wire-frame version byte).
//!
//! F4: every wire-going frame is now `[0x01, …bincode bytes…]`. The
//! contract still holds for arbitrary input — including inputs that
//! omit the version byte or carry an unknown one.

use proptest::prelude::*;
use suwappu_node::{client::ClientMessage, codec, wire::WireMessage};

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1024,
        max_shrink_iters: 64,
        .. ProptestConfig::default()
    })]

    /// Fuzz `WireMessage` framed decode with arbitrary byte slices up
    /// to 64 KiB (the `MAX_COMPACT_MESSAGE_BYTES` cap). Assert: no
    /// panic, no hang — only `Result::Ok(_)` or `Result::Err(_)`.
    #[test]
    fn wire_message_decode_rejects_garbage_without_panic(
        bytes in prop::collection::vec(any::<u8>(), 0..64 * 1024),
    ) {
        let _ = codec::decode_frame::<WireMessage>(&bytes);
    }

    /// Fuzz `ClientMessage` framed decode with arbitrary byte slices up
    /// to 64 KiB. Same contract: decode is total over `&[u8]`.
    #[test]
    fn client_message_decode_rejects_garbage_without_panic(
        bytes in prop::collection::vec(any::<u8>(), 0..64 * 1024),
    ) {
        let _ = codec::decode_frame::<ClientMessage>(&bytes);
    }
}

/// Empty-input edge case — decode should error (FrameDecodeError::Empty),
/// not panic.
#[test]
fn empty_input_does_not_panic() {
    let _ = codec::decode_frame::<WireMessage>(&[]);
    let _ = codec::decode_frame::<ClientMessage>(&[]);
}

/// All-zero input edge case — common payload-corruption pattern.
/// First byte is 0x00 (UnknownVersion), so this should error fast
/// without ever touching bincode.
#[test]
fn all_zero_input_does_not_panic() {
    let zeros = vec![0u8; 4096];
    let _ = codec::decode_frame::<WireMessage>(&zeros);
    let _ = codec::decode_frame::<ClientMessage>(&zeros);
}

/// Length-tagged amplification — a payload with the v1 marker
/// followed by a u64 length prefix claiming a huge Vec. bincode's
/// legacy config has no inherent size limit; verify no panic still.
#[test]
fn length_amplification_does_not_panic() {
    let mut bytes = vec![codec::FRAME_VERSION_V1];
    bytes.extend_from_slice(&(1u64 << 48).to_le_bytes());
    bytes.extend_from_slice(&[0u8; 4096]);
    let _ = codec::decode_frame::<WireMessage>(&bytes);
    let _ = codec::decode_frame::<ClientMessage>(&bytes);
}

/// F4: an unknown version byte returns Err, doesn't panic, and
/// doesn't touch bincode at all.
#[test]
fn unknown_version_byte_rejected() {
    let bytes = vec![0x42u8, 0, 0, 0, 0, 0, 0, 0];
    let err = codec::decode_frame::<WireMessage>(&bytes).unwrap_err();
    match err {
        codec::FrameDecodeError::UnknownVersion(b) => assert_eq!(b, 0x42),
        other => panic!("expected UnknownVersion, got {other:?}"),
    }
}
