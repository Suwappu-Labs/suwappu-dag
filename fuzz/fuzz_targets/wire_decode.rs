//! Fuzz target: framed decode of the inter-validator wire types.
//!
//! Contract: `suwappu_node::codec::decode_frame::<T>(&[u8])` is total for
//! every `T` we accept on the wire. No panic, no UB, no infinite
//! recursion — only `Result::Ok` or `Result::Err`.
//!
//! Covers:
//!   - `suwappu_node::wire::WireMessage` (the peer-to-peer envelope)
//!   - `suwappu_node::client::ClientMessage` (the client-to-validator wire)
//!
//! Same surface as the `proptest_wire_decode.rs` proptests, but
//! cargo-fuzz uses libFuzzer's coverage-guided mutation — typically
//! finds adversarial inputs the bounded-shrinker proptest misses.
//!
//! F4: every wire frame is now `[0x01, …bincode bytes…]`. Inputs
//! omitting or mismatching the version byte fail-fast in
//! `decode_frame` and never reach bincode — the fuzzer still has to
//! verify both paths are panic-free.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    let _ = suwappu_node::codec::decode_frame::<suwappu_node::wire::WireMessage>(bytes);
    let _ = suwappu_node::codec::decode_frame::<suwappu_node::client::ClientMessage>(bytes);
});
