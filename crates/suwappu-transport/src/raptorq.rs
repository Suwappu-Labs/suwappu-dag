//! RaptorQ (RFC 6330) shred and reconstruct for inter-validator block
//! propagation.
//!
//! The SUWAPPU DAG L1 paper (§6.3) specifies RaptorQ erasure coding for block
//! propagation between validators. This module wraps the upstream `raptorq`
//! crate behind a minimal SUWAPPU-shaped API:
//!
//! ```text
//!     shred(payload, packet_size, repair_packets) -> ShredSet
//!     reconstruct(payload_len, packets)           -> Result<Vec<u8>>
//! ```
//!
//! `ShredSet` is the on-wire representation: a vector of `Shred`s plus the
//! `ObjectTransmissionInformation` (OTI) required by every receiver to
//! configure the decoder. In production each `Shred` carries the OTI in its
//! header; here we hand the receiver the OTI directly via `ShredSet::oti`.
//!
//! The DAG-S2 exit gate is `raptorq_reconstructs_under_loss`: for any
//! payload `P`, any encoding configuration `(packet_size, repair_count)`,
//! and any subset of the produced packets of size `≥ source_packets`, the
//! receiver reconstructs `P` bit-for-bit. Verified at 10,000 cases.

use raptorq::{Decoder, Encoder, EncodingPacket, ObjectTransmissionInformation};
use thiserror::Error;

/// Errors produced by the transport layer.
#[derive(Debug, Error)]
pub enum TransportError {
    /// The decoder could not reconstruct the payload from the supplied
    /// packets. Typical cause: too few packets relative to the source
    /// packet count, or all supplied packets were duplicates of a strict
    /// subset of the source set.
    #[error("raptorq decode failed: insufficient or duplicate packets")]
    DecodeFailed,

    /// A wire-format error decoding an `EncodingPacket` from bytes.
    #[error("raptorq packet decode failed")]
    MalformedPacket,
}

/// Size of the RFC 6330 §4.4.2 FEC Payload ID that prefixes every encoded
/// packet. Anything shorter than this cannot be a packet.
const FEC_PAYLOAD_ID_BYTES: usize = 4;

/// A single RaptorQ encoded packet, serialized on the wire.
///
/// Wraps `raptorq::EncodingPacket` and exposes only the byte interface so
/// the rest of the workspace can treat it as opaque.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shred(Vec<u8>);

impl Shred {
    /// Borrow the serialized packet bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Construct a `Shred` from on-wire bytes.
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}

/// A complete encoded set produced by [`shred`].
///
/// In production the OTI travels in-band on each packet header; for the
/// in-memory phase-1 transport (paper §6.3 phase) we attach it once at
/// the set level so receivers can configure the decoder.
#[derive(Debug, Clone)]
pub struct ShredSet {
    /// RaptorQ object transmission information (RFC 6330 §4.4.2).
    pub oti: ObjectTransmissionInformation,
    /// Encoded packets. Length is `source_packets + repair_packets`.
    pub packets: Vec<Shred>,
}

/// Shred `payload` into a `ShredSet` of RaptorQ encoded packets.
///
/// * `packet_size_bytes` controls the symbol size of the encoding. Smaller
///   packet sizes yield more packets and finer-grained recovery; larger
///   sizes amortize per-packet overhead at the cost of recovery granularity.
///   The default in [`crate::DEFAULT_RAPTORQ_BLOCK_BYTES`] (64 KiB) is the
///   tuning anchor; sprint exit-gate properties cover the broader range.
/// * `repair_packets` is the additional redundancy beyond the source set.
///   Any `source_packets + repair_packets` total can be sent; receivers
///   reconstruct after collecting any `source_packets` of them.
pub fn shred(payload: &[u8], packet_size_bytes: u16, repair_packets: u32) -> ShredSet {
    let encoder = Encoder::with_defaults(payload, packet_size_bytes);
    let oti = encoder.get_config();
    let packets: Vec<Shred> = encoder
        .get_encoded_packets(repair_packets)
        .into_iter()
        .map(|p| Shred(p.serialize()))
        .collect();
    ShredSet { oti, packets }
}

/// Reconstruct the payload from `packets`, using `oti` to configure the
/// decoder.
///
/// Returns the recovered payload on success, or [`TransportError::DecodeFailed`]
/// if the decoder cannot complete (insufficient or duplicate packets).
pub fn reconstruct(
    oti: ObjectTransmissionInformation,
    packets: &[Shred],
) -> Result<Vec<u8>, TransportError> {
    let mut decoder = Decoder::new(oti);
    for shred in packets {
        let bytes = shred.as_bytes();
        // `EncodingPacket::deserialize` indexes the first four bytes without
        // checking the length, so a truncated shred would panic here rather
        // than fail to decode. `Shred::from_bytes` takes whatever arrives on
        // the wire, so that input is not necessarily well formed. Drop short
        // shreds and keep going: one malformed packet from one peer must not
        // take down reconstruction of an otherwise recoverable object.
        //
        // Upstream declined to add a fallible variant (cberner/raptorq#230),
        // and the reasoning applies here: RaptorQ corrects erasures, not
        // errors, so it cannot detect a shred whose contents were altered.
        // A length check is not an integrity check. Corrupt shreds that are
        // long enough still decode to incorrect data silently, so the real
        // guarantee has to come from authenticating shreds at the network
        // boundary before they reach this function. This guard only keeps a
        // truncated shred from panicking; see cberner/raptorq#231.
        if bytes.len() < FEC_PAYLOAD_ID_BYTES {
            continue;
        }
        let pkt = EncodingPacket::deserialize(bytes);
        if let Some(out) = decoder.decode(pkt) {
            return Ok(out);
        }
    }
    Err(TransportError::DecodeFailed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncated_shreds_are_skipped_not_fatal() {
        // A peer sending a short datagram must not be able to panic the
        // receiver, and must not prevent an otherwise decodable object from
        // being reconstructed.
        let payload = b"suwappu-transport truncated shred resilience";
        let set = shred(payload, 64, 8);

        let mut packets = Vec::new();
        for len in 0..FEC_PAYLOAD_ID_BYTES {
            packets.push(Shred::from_bytes(vec![0u8; len]));
        }
        packets.extend(set.packets.iter().cloned());

        let out = reconstruct(set.oti, &packets).expect("decode despite truncated shreds");
        assert_eq!(out.as_slice(), payload.as_slice());
    }

    #[test]
    fn only_truncated_shreds_fails_cleanly() {
        // With nothing decodable at all we want an error, not a panic.
        let set = shred(b"payload", 64, 2);
        let packets: Vec<Shred> = (0..FEC_PAYLOAD_ID_BYTES)
            .map(|len| Shred::from_bytes(vec![0u8; len]))
            .collect();
        assert!(matches!(
            reconstruct(set.oti, &packets),
            Err(TransportError::DecodeFailed)
        ));
    }

    #[test]
    fn roundtrip_small_payload() {
        let payload = b"suwappu-transport raptorq smoke test";
        let set = shred(payload, 64, 4);
        let out = reconstruct(set.oti, &set.packets).unwrap();
        assert_eq!(out.as_slice(), payload.as_slice());
    }

    #[test]
    fn roundtrip_with_repair_packets_only() {
        // RaptorQ should decode even when many source packets are missing,
        // as long as the total kept packets meet or exceed the source
        // packet count. For 8 KiB at 256 B packet_size, source_packets = 32;
        // we produce 32 repair packets (total 64) and drop the first 16,
        // leaving 48 packets — comfortably above the 32 source threshold.
        let payload: Vec<u8> = (0u8..=255).cycle().take(8 * 1024).collect();
        let set = shred(&payload, 256, 32);

        let dropped = 16;
        let kept: Vec<Shred> = set.packets[dropped..].to_vec();
        assert!(kept.len() >= 32);

        let out = reconstruct(set.oti, &kept).expect("decode under loss");
        assert_eq!(out, payload);
    }

    #[test]
    fn insufficient_packets_fail() {
        let payload = vec![0xABu8; 2048];
        let set = shred(&payload, 64, 0); // no repair packets

        // Drop most packets — the decoder must report failure.
        let kept: Vec<Shred> = set.packets.iter().take(1).cloned().collect();
        assert!(reconstruct(set.oti, &kept).is_err());
    }
}
