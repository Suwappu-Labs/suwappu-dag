//! Fuzz target: bincode decode of the inter-validator wire types.
//!
//! Contract: `bincode::deserialize::<T>(&[u8])` is total for every
//! `T` we accept on the wire. No panic, no UB, no infinite recursion
//! — only `Result::Ok` or `Result::Err`.
//!
//! Covers:
//!   - `gsx_node::wire::WireMessage` (the peer-to-peer envelope)
//!   - `gsx_node::client::ClientMessage` (the client-to-validator wire)
//!
//! Same surface as the `proptest_wire_decode.rs` proptests, but
//! cargo-fuzz uses libFuzzer's coverage-guided mutation — typically
//! finds adversarial inputs the bounded-shrinker proptest misses.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    let _ = bincode::deserialize::<gsx_node::wire::WireMessage>(bytes);
    let _ = bincode::deserialize::<gsx_node::client::ClientMessage>(bytes);
});
