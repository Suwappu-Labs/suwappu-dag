//! DAG-S2 exit-gate property tests.
//!
//! Exit gate (paper §6.3, RFC 6330 [RaptorQ]):
//!
//! For any payload `P`, any encoding configuration
//! `(packet_size, repair_packets)`, and any selection of received packets
//! whose count meets or exceeds the receiver's recovery threshold, the
//! receiver reconstructs `P` bit-for-bit.
//!
//! Property tests:
//!
//! 1. `raptorq_reconstructs_full_set` — delivering every encoded packet
//!    reconstructs the original payload bit-for-bit.
//! 2. `raptorq_reconstructs_under_loss` — dropping packets within the
//!    repair budget (defined by the encoder, not a hard-coded formula)
//!    still reconstructs.
//! 3. `raptorq_reconstructs_with_shuffled_packets` — packet order does
//!    not matter.
//! 4. `raptorq_fails_with_no_packets` — supplying zero packets returns
//!    a `DecodeFailed` error rather than a silent default.
//!
//! Run at default 256 cases under CI; sprint close runs at
//! `PROPTEST_CASES=10000 cargo test -p gsx-transport --release`.

use gsx_transport::{reconstruct, shred, Shred};
use proptest::prelude::*;
use rand::{rngs::StdRng, seq::SliceRandom, SeedableRng};

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_shrink_iters: 32,
        .. ProptestConfig::default()
    })]

    /// The full set of encoded packets always reconstructs the original
    /// payload.
    #[test]
    fn raptorq_reconstructs_full_set(
        payload in prop::collection::vec(any::<u8>(), 64..4096),
        packet_size in 64u16..=512,
        repair_packets in 0u32..=32,
    ) {
        let set = shred(&payload, packet_size, repair_packets);
        let recovered = reconstruct(set.oti, &set.packets)
            .expect("full packet set must decode");
        prop_assert_eq!(recovered, payload);
    }

    /// Dropping packets while keeping at least the encoder's repair budget
    /// available still recovers the payload.
    ///
    /// RaptorQ is a near-MDS code with a small probabilistic overhead:
    /// typical recovery succeeds at exactly `source_packets` received
    /// packets, with 0–2 additional packets occasionally required (see
    /// RFC 6330 §1.2 on decoding inefficiency). We therefore drop at most
    /// `repair_packets - 2` packets, leaving a 2-packet safety margin
    /// inside the repair budget — this is the worst-case-safe bound that
    /// holds at 10,000 cases across the parameter space.
    #[test]
    fn raptorq_reconstructs_under_loss(
        payload in prop::collection::vec(any::<u8>(), 64..4096),
        packet_size in 64u16..=512,
        repair_packets in 8u32..=64,
        drop_seed in any::<u64>(),
    ) {
        let set = shred(&payload, packet_size, repair_packets);
        let total = set.packets.len();

        // Drop up to `repair_packets - 2` packets randomly. The 2-packet
        // margin absorbs RaptorQ's typical 0–2 decoding overhead.
        let drops = (repair_packets.saturating_sub(2) as usize)
            .min(total.saturating_sub(1));
        let mut rng = StdRng::seed_from_u64(drop_seed);
        let mut shuffled = set.packets.clone();
        shuffled.shuffle(&mut rng);
        let kept: Vec<Shred> = shuffled.into_iter().skip(drops).collect();

        let recovered = reconstruct(set.oti, &kept)
            .expect("decoder must recover with repair_packets-2 drops");
        prop_assert_eq!(recovered, payload);
    }

    /// Packet order on the wire is not load-bearing.
    #[test]
    fn raptorq_reconstructs_with_shuffled_packets(
        payload in prop::collection::vec(any::<u8>(), 64..2048),
        packet_size in 64u16..=256,
        repair_packets in 16u32..=48,
        permute_seed in any::<u64>(),
    ) {
        let set = shred(&payload, packet_size, repair_packets);
        let mut rng = StdRng::seed_from_u64(permute_seed);
        let mut shuffled = set.packets.clone();
        shuffled.shuffle(&mut rng);

        let recovered = reconstruct(set.oti, &shuffled)
            .expect("full packet set in any order must decode");
        prop_assert_eq!(recovered, payload);
    }

    /// Supplying zero packets surfaces as an error, not a silent partial
    /// reconstruction.
    #[test]
    fn raptorq_fails_with_no_packets(
        payload in prop::collection::vec(any::<u8>(), 64..1024),
        packet_size in 64u16..=256,
        repair_packets in 0u32..=8,
    ) {
        let set = shred(&payload, packet_size, repair_packets);
        let empty: Vec<Shred> = Vec::new();
        prop_assert!(reconstruct(set.oti, &empty).is_err());
    }
}
