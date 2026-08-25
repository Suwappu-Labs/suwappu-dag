//! Randomness bridge.
//!
//! The workspace is on `rand` 0.8 (which uses `rand_core` 0.6), while the
//! RustCrypto PQ crates take an RNG through `rand_core` 0.10. Rather than
//! introducing a second entropy source, this adapts the one the rest of the
//! crate already uses, so there is a single place where randomness enters.

use rand::RngCore as _;
use rand_core_0_10::{TryCryptoRng, TryRng};

/// Fill `dst` with bytes from the operating system CSPRNG.
pub(crate) fn random_bytes(dst: &mut [u8]) {
    rand::rngs::OsRng.fill_bytes(dst);
}

/// `rand_core` 0.10 view of the OS CSPRNG, for the RustCrypto PQ crates.
pub(crate) struct OsRng;

impl TryRng for OsRng {
    // `rand` 0.8's `OsRng` panics rather than reporting failure, so no error is
    // observable through this interface.
    type Error = core::convert::Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        Ok(rand::rngs::OsRng.next_u32())
    }

    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        Ok(rand::rngs::OsRng.next_u64())
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
        random_bytes(dst);
        Ok(())
    }
}

impl TryCryptoRng for OsRng {}
