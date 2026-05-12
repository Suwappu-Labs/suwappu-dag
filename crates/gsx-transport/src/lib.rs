//! gsx-transport — Inter-validator transport.
//!
//! Implements §6.3 of the *GSX DAG Layer 1* paper:
//!
//! - Inter-validator transport runs on SCION [SCION Book, 2017]
//! - SCION-IP-Gateway fallback for external clients
//! - Path-authenticated routing — eliminates the BGP-class attack vector
//!   responsible for multiple production blockchain incidents on flat IP
//!   infrastructure [Birgi et al., 2022]
//! - Trust Root Configuration governance over the validator mesh's Isolation
//!   Domain provides cryptographically anchored route-authority rotation
//! - Block propagation uses RaptorQ erasure coding (RFC 6330, 2011)
//!
//! Sprint scope:
//!
//! - DAG-S2: RaptorQ block-shred + reconstruction (in-memory)
//! - DAG-S18: SCION path-authenticated routing integration
//! - DAG-S19: SCION-IP-Gateway external-client fallback

#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Default RaptorQ block size for inter-validator gossip (tunable).
pub const DEFAULT_RAPTORQ_BLOCK_BYTES: usize = 64 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raptorq_block_size_is_64kib() {
        assert_eq!(DEFAULT_RAPTORQ_BLOCK_BYTES, 64 * 1024);
    }
}
