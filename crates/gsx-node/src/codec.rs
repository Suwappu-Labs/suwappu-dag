//! F4: bincode 2.x codec shim + 1-byte wire-frame version marker.
//!
//! ## Why this lives here
//!
//! The workspace migrated from bincode 1.x to 2.x in one PR. To
//! preserve every existing on-wire byte layout (so that pre-flip
//! intent hashes — `blake3(bincode(intent))` — stay canonical, and
//! so that any in-flight wire frame survives the cutover) we pin
//! `bincode::config::legacy()` everywhere bytes can leak outside a
//! single process. This module wraps the 2.x API into the two thin
//! helpers the rest of the crate uses.
//!
//! ## Wire-frame version byte
//!
//! Anything that crosses a TCP socket between two `gsx-node`
//! instances goes through [`encode_frame`] / [`decode_frame`]. They
//! prepend a 1-byte version marker ([`FRAME_VERSION_V1`] = `0x01`)
//! to every framed payload so a future format flip (bincode-3,
//! protobuf, postcard, or a config change) is detectable instead
//! of silently corrupting peers.
//!
//! The version byte sits INSIDE the existing 4-byte length-prefix
//! envelope. Read order on the wire:
//!
//! ```text
//!   [4 B big-endian length] [1 B version=0x01] [payload bytes]
//! ```
//!
//! Inner uses (intent → hash, internal cache encoding) DO NOT use
//! the framed helpers — the hash recipe must consume the pure
//! bincode bytes so the canonical hash stays stable independent
//! of any future frame-version bump.

use std::{error::Error as StdError, fmt};

use bincode::config::Configuration;
use serde::{de::DeserializeOwned, Serialize};

/// Wire-format version marker. v1 is the format shipped at the
/// bincode-2.x flip — byte-for-byte identical to bincode 1.x's
/// default (`config::legacy()`). A future cutover (e.g. moving to
/// varint encoding for smaller frames, or to a non-bincode codec
/// entirely) increments this byte.
pub const FRAME_VERSION_V1: u8 = 0x01;

/// bincode 2.x configuration that produces byte-for-byte identical
/// output to bincode 1.x's default: fixed-int encoding,
/// little-endian, no size limit, trailing bytes allowed on decode.
/// Used at every encode/decode site so the 1.x → 2.x flip is wire-
/// transparent.
///
/// `const fn` because `Configuration::default()` is `const`.
#[inline]
pub fn config() -> impl bincode::config::Config {
    Configuration::<bincode::config::LittleEndian, bincode::config::Fixint, bincode::config::NoLimit>::default()
}

/// Encode a serde value to bytes using the legacy/fixint config.
///
/// Errors only on a type that fails to serialize (unsupported
/// shape, e.g. an internally-tagged enum with bincode); never on
/// allocation since we use the default in-memory writer.
pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, EncodeError> {
    bincode::serde::encode_to_vec(value, config()).map_err(EncodeError)
}

/// Decode a serde value from bytes using the legacy/fixint config.
///
/// Returns the decoded value and discards the (unused) number of
/// bytes consumed — the legacy config allows trailing bytes by
/// default, so callers that need strict-end-of-input parsing must
/// check the slice length themselves.
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, DecodeError> {
    let (value, _consumed) =
        bincode::serde::decode_from_slice(bytes, config()).map_err(DecodeError)?;
    Ok(value)
}

/// Encode a serde value as a framed payload: `[FRAME_VERSION_V1,
/// payload bytes...]`. The 4-byte length prefix wrapping this
/// payload is the responsibility of the transport layer (see
/// `wire::write_frame`).
pub fn encode_frame<T: Serialize>(value: &T) -> Result<Vec<u8>, EncodeError> {
    let mut out = Vec::with_capacity(64);
    out.push(FRAME_VERSION_V1);
    let body = encode(value)?;
    out.extend_from_slice(&body);
    Ok(out)
}

/// Decode a framed payload. Errors if the frame is empty or carries
/// a version byte this build doesn't understand.
pub fn decode_frame<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, FrameDecodeError> {
    let (version, body) = bytes.split_first().ok_or(FrameDecodeError::Empty)?;
    if *version != FRAME_VERSION_V1 {
        return Err(FrameDecodeError::UnknownVersion(*version));
    }
    decode(body).map_err(FrameDecodeError::Decode)
}

/// Encode error wrapper. bincode 2's `EncodeError` is split from
/// `DecodeError`; this newtype keeps callers from depending on
/// the 2.x crate path directly.
#[derive(Debug)]
pub struct EncodeError(pub bincode::error::EncodeError);

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bincode encode: {}", self.0)
    }
}

impl StdError for EncodeError {}

/// Decode error wrapper.
#[derive(Debug)]
pub struct DecodeError(pub bincode::error::DecodeError);

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bincode decode: {}", self.0)
    }
}

impl StdError for DecodeError {}

/// Frame-decode failure: empty payload, unknown version, or inner
/// bincode decode failure.
#[derive(Debug)]
pub enum FrameDecodeError {
    /// Frame payload was zero bytes — the version marker is missing.
    Empty,
    /// First byte was not [`FRAME_VERSION_V1`]. Carries the observed
    /// byte for diagnostics.
    UnknownVersion(u8),
    /// Inner bincode-deserialize failure on the post-version body.
    Decode(DecodeError),
}

impl fmt::Display for FrameDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "wire frame is empty"),
            Self::UnknownVersion(b) => write!(f, "unknown wire frame version: 0x{b:02x}"),
            Self::Decode(e) => write!(f, "{e}"),
        }
    }
}

impl StdError for FrameDecodeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_inner() {
        let v: (u32, String) = (42, "hi".into());
        let bytes = encode(&v).unwrap();
        let back: (u32, String) = decode(&bytes).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn round_trip_frame() {
        let v: Vec<u8> = vec![1, 2, 3, 4];
        let frame = encode_frame(&v).unwrap();
        assert_eq!(frame[0], FRAME_VERSION_V1, "version byte first");
        let back: Vec<u8> = decode_frame(&frame).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn empty_frame_rejected() {
        let err = decode_frame::<()>(&[]).unwrap_err();
        assert!(matches!(err, FrameDecodeError::Empty));
    }

    #[test]
    fn unknown_version_rejected() {
        // 0x02 isn't a version we know about.
        let frame = [0x02u8, 0, 0, 0, 0];
        let err = decode_frame::<u32>(&frame).unwrap_err();
        match err {
            FrameDecodeError::UnknownVersion(b) => assert_eq!(b, 0x02),
            other => panic!("expected UnknownVersion, got {other:?}"),
        }
    }

    #[test]
    fn legacy_config_matches_1x_byte_layout() {
        // Fixed-int u32: bincode 1.x default writes 4 little-endian bytes.
        let v: u32 = 0xDEAD_BEEF;
        let bytes = encode(&v).unwrap();
        assert_eq!(bytes, vec![0xEF, 0xBE, 0xAD, 0xDE]);
    }
}
