//! Garbage-collection arithmetic for the certificate DAG (IQ-008 D1).
//!
//! Two round numbers govern bounded retention, and both are pure
//! functions of the commit sequence so that every honest node derives
//! the same values from the same committed leaders:
//!
//! - [`gc_round`] — the highest round whose certificates are obsolete.
//!   Derived from the *last committed leader round*. Certificates at or
//!   below it are never inserted, never served, never committed
//!   (Sui `consensus/core` `DagState` semantics; Narwhal §"Garbage
//!   Collection": "all later messages from previous rounds can be safely
//!   ignored").
//! - [`commit_floor`] — the floor below which a given leader's causal
//!   history is cut. Derived from *that leader's* round, not from local
//!   pruning progress, so the committed sub-DAG of a leader is identical
//!   on a node that has pruned aggressively and on one that has not
//!   (Bullshark Algorithm 6 restated in rounds).
//!
//! Consistency argument (IQ-008 I-GC1). A node prunes round `x` only after
//! committing a leader at round ≥ `x + depth`. Any later leader `L` has
//! `L.round > x + depth`, hence `x ≤ commit_floor(L)`, hence the pruned
//! certificate is excluded from `L`'s sweep on every node whether or not
//! it still holds it. Pruning therefore never changes a committed set.

use crate::cert::Round;

/// Retention depth in rounds. A certificate is obsolete once the chain
/// has committed a leader this many rounds past it. 256 rounds ≈ 64 s at
/// the 250 ms testnet cadence — deliberately larger than Sui's default
/// so that it dwarfs the single-round late-arrival lag IQ-004 documents
/// (see IQ-008 Residuals §1).
pub const GC_DEPTH: Round = 256;

/// The garbage-collection round implied by `last_committed_leader_round`:
/// `Some(last − depth)` once the chain is at least `depth` rounds deep,
/// `None` while nothing is obsolete yet. `None` rather than `Some(0)` so
/// that genesis (round 0) is retained until the chain has genuinely
/// committed `depth` rounds past it.
pub fn gc_round(last_committed_leader_round: Round, depth: Round) -> Option<Round> {
    last_committed_leader_round.checked_sub(depth)
}

/// The commit floor for a leader at `leader_round`: certificates at or
/// below `Some(leader_round − depth)` are excluded from that leader's
/// committed sub-DAG. `None` means the whole causal history is swept
/// (the leader is less than `depth` rounds from genesis).
pub fn commit_floor(leader_round: Round, depth: Round) -> Option<Round> {
    leader_round.checked_sub(depth)
}

/// `true` iff a certificate at `round` is obsolete under `gc_round`.
pub fn is_obsolete(round: Round, gc_round: Option<Round>) -> bool {
    matches!(gc_round, Some(g) if round <= g)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gc_round_is_none_until_depth_committed() {
        assert_eq!(gc_round(0, 256), None);
        assert_eq!(gc_round(255, 256), None);
        assert_eq!(gc_round(256, 256), Some(0));
        assert_eq!(gc_round(300, 256), Some(44));
    }

    #[test]
    fn commit_floor_mirrors_gc_round() {
        assert_eq!(commit_floor(10, 256), None);
        assert_eq!(commit_floor(256, 256), Some(0));
        assert_eq!(commit_floor(1000, 256), Some(744));
    }

    #[test]
    fn floor_of_later_leader_dominates_gc_round_of_earlier_commit() {
        // The I-GC1 lemma in one line: gc_round(prev) <= commit_floor(L)
        // whenever prev < L.
        for prev in 0..600u64 {
            for l in (prev + 1)..(prev + 40) {
                let g = gc_round(prev, GC_DEPTH);
                let f = commit_floor(l, GC_DEPTH);
                match (g, f) {
                    (None, _) => {}
                    (Some(g), Some(f)) => assert!(g <= f, "prev={prev} l={l}"),
                    (Some(_), None) => unreachable!("floor None but gc Some"),
                }
            }
        }
    }

    #[test]
    fn obsolete_is_inclusive_at_the_gc_round() {
        assert!(!is_obsolete(5, None));
        assert!(is_obsolete(5, Some(5)));
        assert!(is_obsolete(0, Some(5)));
        assert!(!is_obsolete(6, Some(5)));
    }
}
