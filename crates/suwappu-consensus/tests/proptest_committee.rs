//! DAG-S36 exit-gate property tests (IQ-010 D1).
//!
//! The commit rule is defined over a [`Committee`] — the sorted set of
//! active authority ids — instead of the integer range `0..n`. These
//! properties pin the relationship between the two rules and the
//! committee rule's own invariants:
//!
//! - `contiguous_committee_matches_n_rule` — over `Committee::contiguous(n)`
//!   every slot decision and the finalize sequence equal the `n`-based
//!   rule's (the DAG-S4/S5 gates carry over unchanged).
//! - `decisions_are_invariant_under_relabelling` — relabelling the members
//!   to arbitrary ids with gaps (what admission and ejection produce)
//!   changes no decision and no finalize position (I-CT2: the rotation
//!   covers exactly the members).
//! - `non_member_certificates_never_count` — certificates by authors
//!   outside the committee that no member references change no decision
//!   and no finalize sequence (I-CT3: only members are support).
//! - `committee_finality_is_monotone` — extending the DAG with more
//!   member rounds and with non-member certificates that members do
//!   reference never turns a `Direct` slot into anything else, and never
//!   drops a finalized certificate (positions may move: the IQ-004
//!   late-flip class, identical under the n-rule).
//!
//! Run at 256 cases under CI; sprint close runs
//! `PROPTEST_CASES=10000 cargo test -p suwappu-consensus --release --test proptest_committee`.

use std::collections::BTreeMap;

use proptest::prelude::*;
use suwappu_consensus::{
    decide_slot, decide_slot_for, finalize, finalize_for, AuthorityId, CertHash, Certificate,
    Committee, DagStore, LeaderStatus, Round,
};

/// A DAG over `labels` (the committee members' ids): `n_rounds` rounds,
/// one certificate per member per round unless its omit bit is set, each
/// certificate referencing every present certificate of the previous
/// round. Returns the certificates in topological order plus the hash
/// at every `(round, member index)` slot that was not omitted.
fn build_over(
    labels: &[AuthorityId],
    n_rounds: u64,
    omit_bits: u64,
    payload_seed: u64,
) -> (Vec<Certificate>, BTreeMap<(Round, usize), CertHash>) {
    let mut certs = Vec::new();
    let mut slots: BTreeMap<(Round, usize), CertHash> = BTreeMap::new();
    let mut prev: Vec<CertHash> = Vec::new();
    for r in 0..n_rounds {
        let mut this = Vec::new();
        for (i, &author) in labels.iter().enumerate() {
            let bit = (r as usize * labels.len() + i) % 64;
            if (omit_bits >> bit) & 1 == 1 {
                continue;
            }
            let mut payload = [0u8; 32];
            payload[0] = i as u8;
            payload[1] = r as u8;
            payload[2] = (payload_seed & 0xFF) as u8;
            let cert = Certificate {
                author,
                round: r,
                parents: if r == 0 { Vec::new() } else { prev.clone() },
                payload_digest: payload,
                signature: Vec::new(),
            };
            let h = cert.hash();
            slots.insert((r, i), h);
            this.push(h);
            certs.push(cert);
        }
        prev = this;
    }
    (certs, slots)
}

/// `build_over` plus an outsider (id 1000, never a member) whose
/// certificate at round `r` — present when bit `r` of `outsider_bits` is
/// set — references every member certificate of round `r − 1` and is
/// referenced by every member certificate of round `r + 1`.
fn build_mixed(
    labels: &[AuthorityId],
    n_rounds: u64,
    omit_bits: u64,
    payload_seed: u64,
    outsider_bits: u64,
) -> Vec<Certificate> {
    let outsider: AuthorityId = 1_000;
    let mut certs = Vec::new();
    let mut prev_members: Vec<CertHash> = Vec::new();
    let mut prev_outsider: Option<CertHash> = None;
    for r in 0..n_rounds {
        let mut this = Vec::new();
        for (i, &author) in labels.iter().enumerate() {
            let bit = (r as usize * labels.len() + i) % 64;
            if (omit_bits >> bit) & 1 == 1 {
                continue;
            }
            let mut payload = [0u8; 32];
            payload[0] = i as u8;
            payload[1] = r as u8;
            payload[2] = (payload_seed & 0xFF) as u8;
            let mut parents = if r == 0 {
                Vec::new()
            } else {
                prev_members.clone()
            };
            if let Some(o) = prev_outsider {
                parents.push(o);
            }
            let cert = Certificate {
                author,
                round: r,
                parents,
                payload_digest: payload,
                signature: Vec::new(),
            };
            this.push(cert.hash());
            certs.push(cert);
        }
        prev_outsider = if r > 0 && (outsider_bits >> (r % 64)) & 1 == 1 {
            let mut payload = [0xBBu8; 32];
            payload[1] = r as u8;
            let cert = Certificate {
                author: outsider,
                round: r,
                parents: prev_members.clone(),
                payload_digest: payload,
                signature: Vec::new(),
            };
            let h = cert.hash();
            certs.push(cert);
            Some(h)
        } else {
            None
        };
        prev_members = this;
    }
    certs
}

fn store_from(certs: &[Certificate]) -> DagStore {
    let mut s = DagStore::new();
    for c in certs {
        s.insert(c.clone())
            .expect("topo-ordered insert must succeed");
    }
    s
}

/// `n` distinct ids drawn from the low bits of `mask`, padded upward if
/// the mask is too sparse — a committee with arbitrary gaps.
fn labels_from_mask(n: usize, mask: u32) -> Vec<AuthorityId> {
    let mut out: Vec<AuthorityId> = (0..32u32).filter(|b| (mask >> b) & 1 == 1).collect();
    let mut next = 32u32;
    while out.len() < n {
        out.push(next);
        next += 1;
    }
    out.truncate(n);
    out.sort_unstable();
    out
}

/// Map a `LeaderStatus` through a hash relabelling.
fn map_status(s: LeaderStatus, map: &BTreeMap<CertHash, CertHash>) -> LeaderStatus {
    match s {
        LeaderStatus::Direct(h) => LeaderStatus::Direct(*map.get(&h).expect("known hash")),
        other => other,
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(256),
        max_shrink_iters: 32,
        .. ProptestConfig::default()
    })]

    #[test]
    fn contiguous_committee_matches_n_rule(
        n in 1u32..=7,
        n_rounds in 1u64..=6,
        omit_bits in any::<u64>(),
        payload_seed in any::<u64>(),
    ) {
        let labels: Vec<AuthorityId> = (0..n).collect();
        let (certs, _) = build_over(&labels, n_rounds, omit_bits, payload_seed);
        let dag = store_from(&certs);
        let committee = Committee::contiguous(n);
        prop_assert!(committee.is_contiguous());
        for r in 0..n_rounds {
            prop_assert_eq!(decide_slot(&dag, r, n), decide_slot_for(&dag, r, &committee), "round {}", r);
        }
        prop_assert_eq!(finalize(&dag, n), finalize_for(&dag, &committee));
    }

    #[test]
    fn decisions_are_invariant_under_relabelling(
        n in 1usize..=6,
        mask in any::<u32>(),
        n_rounds in 1u64..=6,
        omit_bits in any::<u64>(),
        payload_seed in any::<u64>(),
    ) {
        let base_labels: Vec<AuthorityId> = (0..n as u32).collect();
        let labels = labels_from_mask(n, mask);
        let (base_certs, base_slots) = build_over(&base_labels, n_rounds, omit_bits, payload_seed);
        let (certs, slots) = build_over(&labels, n_rounds, omit_bits, payload_seed);
        let base = store_from(&base_certs);
        let dag = store_from(&certs);
        let map: BTreeMap<CertHash, CertHash> = base_slots
            .iter()
            .map(|(k, h)| (*h, slots[k]))
            .collect();
        let committee = Committee::new(labels.iter().copied());
        prop_assert_eq!(committee.size(), n as u32);
        for r in 0..n_rounds {
            // The rotation names the i-th member where the n-rule names i.
            prop_assert_eq!(committee.leader(r), Some(labels[(r % n as u64) as usize]));
            prop_assert_eq!(
                map_status(decide_slot(&base, r, n as u32), &map),
                decide_slot_for(&dag, r, &committee),
                "round {} labels {:?}", r, labels
            );
        }
        let mapped: Vec<CertHash> = finalize(&base, n as u32).into_iter().map(|h| map[&h]).collect();
        prop_assert_eq!(mapped, finalize_for(&dag, &committee));
    }

    #[test]
    fn non_member_certificates_never_count(
        n in 1usize..=6,
        mask in any::<u32>(),
        n_rounds in 1u64..=6,
        omit_bits in any::<u64>(),
        payload_seed in any::<u64>(),
        outsiders in 1u32..=3,
        outsider_bits in any::<u64>(),
    ) {
        let labels = labels_from_mask(n, mask);
        let committee = Committee::new(labels.iter().copied());
        let (certs, _) = build_over(&labels, n_rounds, omit_bits, payload_seed);
        let base = store_from(&certs);
        // Outsiders reference every member certificate of the previous
        // round (a valid parent set) but no member references them.
        let mut ext = store_from(&certs);
        let mut by_round: BTreeMap<Round, Vec<CertHash>> = BTreeMap::new();
        for c in &certs {
            by_round.entry(c.round).or_default().push(c.hash());
        }
        for k in 0..outsiders {
            let author = 1_000 + k;
            prop_assert!(!committee.contains(author));
            for r in 0..n_rounds {
                if (outsider_bits >> ((r as u32 * 3 + k) % 64)) & 1 == 0 {
                    continue;
                }
                let parents = if r == 0 { Vec::new() } else { by_round.get(&(r - 1)).cloned().unwrap_or_default() };
                let mut payload = [0xAAu8; 32];
                payload[0] = k as u8;
                payload[1] = r as u8;
                ext.insert(Certificate { author, round: r, parents, payload_digest: payload, signature: Vec::new() }).unwrap();
            }
        }
        for r in 0..n_rounds {
            prop_assert_eq!(decide_slot_for(&base, r, &committee), decide_slot_for(&ext, r, &committee), "round {}", r);
        }
        prop_assert_eq!(finalize_for(&base, &committee), finalize_for(&ext, &committee));
    }

    #[test]
    fn committee_finality_is_monotone(
        n in 1usize..=6,
        mask in any::<u32>(),
        n_rounds in 2u64..=5,
        extra_rounds in 0u64..=3,
        omit_bits in any::<u64>(),
        payload_seed in any::<u64>(),
        outsider_bits in any::<u64>(),
    ) {
        let labels = labels_from_mask(n, mask);
        let committee = Committee::new(labels.iter().copied());

        // Pure later-round extension: same members, more rounds.
        let (certs, _) = build_over(&labels, n_rounds, omit_bits, payload_seed);
        let (longer, _) = build_over(&labels, n_rounds + extra_rounds, omit_bits, payload_seed);
        let base = store_from(&certs);
        let ext = store_from(&longer);
        for r in 0..n_rounds {
            if let LeaderStatus::Direct(h) = decide_slot_for(&base, r, &committee) {
                prop_assert_eq!(decide_slot_for(&ext, r, &committee), LeaderStatus::Direct(h), "round {}", r);
            }
        }
        // Everything finalized in the base stays finalized. The
        // *position* may change: with omissions a later anchor can
        // decide an earlier slot indirectly and insert its history ahead
        // of already-finalized certificates — the IQ-004 late-flip class,
        // identical under the n-rule (`finalize_is_append_only` asserts
        // the prefix only for dense DAGs, where every slot decides
        // directly).
        let base_final = finalize_for(&base, &committee);
        let ext_final = finalize_for(&ext, &committee);
        for h in &base_final {
            prop_assert!(ext_final.contains(h), "finalized certificate dropped by extension");
        }

        // Mixed extension: an outsider's certificates are inserted and
        // referenced by every member certificate of the next round (a
        // registered-but-inactive author whose certificates honest
        // proposers select as parents). Decisions stay Direct under the
        // mixed DAG's own extension by more rounds.
        let mixed = store_from(&build_mixed(&labels, n_rounds, omit_bits, payload_seed, outsider_bits));
        let mixed_ext = store_from(&build_mixed(&labels, n_rounds + extra_rounds, omit_bits, payload_seed, outsider_bits));
        for r in 0..n_rounds {
            if let LeaderStatus::Direct(h) = decide_slot_for(&mixed, r, &committee) {
                prop_assert_eq!(decide_slot_for(&mixed_ext, r, &committee), LeaderStatus::Direct(h), "mixed round {}", r);
            }
        }
        let mixed_final = finalize_for(&mixed, &committee);
        let mixed_ext_final = finalize_for(&mixed_ext, &committee);
        for h in &mixed_final {
            prop_assert!(mixed_ext_final.contains(h), "finalized certificate dropped by mixed extension");
        }
        // Round-0 certificates are hash-identical in the pure and mixed
        // DAGs (no parents), so the slot-0 decision agrees exactly.
        if let LeaderStatus::Direct(h) = decide_slot_for(&base, 0, &committee) {
            prop_assert_eq!(decide_slot_for(&mixed, 0, &committee), LeaderStatus::Direct(h));
        }
    }
}
