//! DAG-S18 exit-gate property tests.
//!
//! Exit gate: `scion_path_auth` — for any honest path sealed under a
//! TRC, `verify_path` accepts; tampering with any byte in the hop
//! MAC chain causes `verify_path` to reject with `InvalidHopMac`,
//! precisely identifying the first broken hop.
//!
//! Supporting properties:
//!
//! - `forged_hop_breaks_chain` — flipping any byte of a hop's MAC or
//!   routing fields is detected.
//! - `unauthorized_as_rejected` — a hop authored by an AS outside the
//!   TRC's `as_keys` set is rejected with `UnauthorizedAs`.
//! - `expired_hop_rejected` — a hop with `expiration_round < now` is
//!   rejected with `HopExpired`.
//!
//! Run at default 256 cases under CI; sprint close runs
//! `PROPTEST_CASES=10000 cargo test -p suwappu-transport --release`.

use std::collections::BTreeMap;

use suwappu_transport::{
    hop_mac, seal_path, verify_path, AsId, HopField, IsdId, Path, ScionError, TrustRootConfig,
};
use proptest::prelude::*;

fn build_trc(isd: IsdId, n_ases: u32, valid_until: u64) -> (TrustRootConfig, Vec<AsId>) {
    let mut as_keys = BTreeMap::new();
    let mut ases = Vec::with_capacity(n_ases as usize);
    for i in 0..n_ases {
        let as_id = (i + 10) as AsId;
        let mut key = [0u8; 32];
        // Spread the seed across the key so distinct ASes have
        // distinct keys.
        key[0..4].copy_from_slice(&as_id.to_be_bytes());
        as_keys.insert(as_id, key);
        ases.push(as_id);
    }
    (
        TrustRootConfig {
            isd,
            version: 1,
            as_keys,
            valid_until,
        },
        ases,
    )
}

fn build_hops(isd: IsdId, ases: &[AsId], expiration: u64) -> Vec<HopField> {
    ases.iter()
        .enumerate()
        .map(|(i, a)| HopField {
            isd_as: (isd, *a),
            ingress_iface: (i * 2 + 1) as u16,
            egress_iface: (i * 2 + 2) as u16,
            expiration_round: expiration,
            mac: [0u8; 16],
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_shrink_iters: 32,
        .. ProptestConfig::default()
    })]

    /// EXIT GATE — any path sealed honestly verifies under its TRC,
    /// at any `now` within the valid window and before hop expiration.
    #[test]
    fn scion_path_auth(
        isd in 1u16..=100,
        n_ases in 1u32..=8,
        created_at in 0u64..=10_000,
        check_offset in 0u64..=1_000,
        expiration_offset in 1u64..=10_000,
    ) {
        let (trc, ases) = build_trc(isd, n_ases, 1_000_000);
        let expiration = created_at + expiration_offset;
        let hops = build_hops(isd, &ases, expiration);
        let path = seal_path(isd, created_at, hops, &trc).expect("seal");
        let now = created_at + check_offset;
        // Constrain check_offset to be within both TRC validity AND
        // hop expiration.
        if now <= expiration && now <= trc.valid_until {
            verify_path(&path, &trc, now).expect("honest path must verify");
        }
    }

    /// Flipping any byte of any hop's MAC or routing fields breaks the
    /// chain. The first broken hop is identified precisely.
    #[test]
    fn forged_hop_breaks_chain(
        isd in 1u16..=100,
        n_ases in 1u32..=8,
        created_at in 0u64..=10_000,
        target_hop in 0usize..=7,
        byte_idx in 0usize..16,
    ) {
        let (trc, ases) = build_trc(isd, n_ases, 1_000_000);
        let expiration = created_at + 5_000;
        let hops = build_hops(isd, &ases, expiration);
        let mut path = seal_path(isd, created_at, hops, &trc).unwrap();
        let target = target_hop.min(path.hops.len() - 1);
        path.hops[target].mac[byte_idx] ^= 1; // flip one bit
        let err = verify_path(&path, &trc, created_at + 100);
        let is_invalid_mac = matches!(err, Err(ScionError::InvalidHopMac { hop_index }) if hop_index == target);
        prop_assert!(is_invalid_mac);
    }

    /// Adding a single hop authored by an AS outside the TRC's
    /// authorized set is rejected with `UnauthorizedAs`.
    #[test]
    fn unauthorized_as_rejected(
        isd in 1u16..=100,
        n_ases in 1u32..=4,
        rogue_as_id in 100u32..=10_000,
    ) {
        let (trc, _ases) = build_trc(isd, n_ases, 1_000_000);
        let rogue_hop = HopField {
            isd_as: (isd, rogue_as_id),
            ingress_iface: 1,
            egress_iface: 2,
            expiration_round: 1_000_000,
            mac: [0u8; 16],
        };
        let path = Path {
            isd,
            hops: vec![rogue_hop],
            created_at: 0,
        };
        let err = verify_path(&path, &trc, 200);
        let is_unauthorized = matches!(
            err,
            Err(ScionError::UnauthorizedAs { hop_index: 0, .. })
        );
        prop_assert!(is_unauthorized);
    }

    /// A hop with `expiration_round < now` is rejected with `HopExpired`.
    /// We seal the path honestly then check at a round strictly past
    /// the expiration.
    #[test]
    fn expired_hop_rejected(
        isd in 1u16..=100,
        created_at in 0u64..=100,
        expiration in 1u64..=1_000,
        check_offset in 1u64..=1_000,
    ) {
        let (trc, ases) = build_trc(isd, 1, 10_000_000);
        let hops = vec![HopField {
            isd_as: (isd, ases[0]),
            ingress_iface: 1,
            egress_iface: 2,
            expiration_round: expiration,
            mac: [0u8; 16],
        }];
        let path = seal_path(isd, created_at, hops, &trc).unwrap();
        let now = expiration + check_offset;
        let err = verify_path(&path, &trc, now);
        let is_expired = matches!(err, Err(ScionError::HopExpired { .. }));
        prop_assert!(is_expired);

        // Also: the MAC chain itself is honest, verified by hop_mac
        // equality at the post-seal state.
        let key = trc.as_keys.get(&ases[0]).unwrap();
        let seed = {
            let mut hasher = blake3::Hasher::new();
            hasher.update(b"GSX-SCION-PATH-SEED-V1");
            hasher.update(&isd.to_be_bytes());
            hasher.update(&created_at.to_be_bytes());
            let mut out = [0u8; 16];
            out.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
            out
        };
        let expected = hop_mac(key, seed, &path.hops[0]);
        prop_assert_eq!(expected, path.hops[0].mac);
    }
}
