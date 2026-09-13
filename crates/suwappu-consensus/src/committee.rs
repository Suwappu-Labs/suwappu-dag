//! The active committee the commit rule runs over (IQ-010 D1).
//!
//! DagBft-C's leader schedule, quorum threshold and support counting are
//! defined over the set of *active* authorities, not over the integer
//! range `0..n`. Authority ids are caller-chosen at admission and are
//! never renumbered on ejection, so after any membership change the two
//! differ: with ids `{0, 2, 3}` the `0..3` rule gives id 1 a leader slot
//! it can never fill and never selects id 3's certificates as parents or
//! counts them as support — a permanent halt at threshold 3. A
//! [`Committee`] is the sorted member set; the leader of round `r` is its
//! `r mod |C|`-th member, and only members' certificates count.
//!
//! The committee is a deterministic function of the committed sequence
//! (IQ-010 D2–D4): every node with the same committed prefix holds the
//! same committee, so every node decides the same leader for the same
//! slot. This module is pure; when and how it changes is the node's
//! business (`suwappu-node`, epoch-boundary governance drain).

use serde::{Deserialize, Serialize};

use crate::{
    cert::{AuthorityId, Round},
    commit::{quorum_threshold, CommitteeSize},
};

/// Sorted, de-duplicated set of active authority ids.
///
/// Serialises as the plain sorted vector, so it can sit inside a
/// checkpoint's registry commitment without an encoding of its own.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Committee {
    members: Vec<AuthorityId>,
}

impl Committee {
    /// Build a committee from any iterator of ids; order and duplicates
    /// are normalised away.
    pub fn new<I: IntoIterator<Item = AuthorityId>>(ids: I) -> Self {
        let mut members: Vec<AuthorityId> = ids.into_iter().collect();
        members.sort_unstable();
        members.dedup();
        Self { members }
    }

    /// The committee `{0, 1, …, n − 1}`: what the pre-IQ-010 `n`-based
    /// rule implicitly assumed. The `n`-based functions are wrappers over
    /// this constructor.
    pub fn contiguous(n: CommitteeSize) -> Self {
        Self {
            members: (0..n).collect(),
        }
    }

    /// Number of members.
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// `true` iff the committee has no members.
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// Number of members as the commit rule's `n`.
    pub fn size(&self) -> CommitteeSize {
        self.members.len() as CommitteeSize
    }

    /// `true` iff `id` is a member.
    pub fn contains(&self, id: AuthorityId) -> bool {
        self.members.binary_search(&id).is_ok()
    }

    /// Position of `id` in the sorted member list.
    pub fn index_of(&self, id: AuthorityId) -> Option<usize> {
        self.members.binary_search(&id).ok()
    }

    /// Members in ascending id order.
    pub fn members(&self) -> &[AuthorityId] {
        &self.members
    }

    /// BFT supermajority threshold over the committee size.
    pub fn quorum_threshold(&self) -> u32 {
        quorum_threshold(self.size())
    }

    /// Round-robin leader of `round`: the `round mod |C|`-th member.
    /// `None` for an empty committee (nothing can be decided).
    pub fn leader(&self, round: Round) -> Option<AuthorityId> {
        if self.members.is_empty() {
            return None;
        }
        Some(self.members[(round % self.members.len() as u64) as usize])
    }

    /// `true` iff the members are exactly `0..len`, i.e. the `n`-based
    /// rule and the committee rule coincide.
    pub fn is_contiguous(&self) -> bool {
        self.members
            .iter()
            .enumerate()
            .all(|(i, id)| *id as usize == i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_order_and_duplicates() {
        let c = Committee::new([3, 1, 3, 0]);
        assert_eq!(c.members(), &[0, 1, 3]);
        assert_eq!(c.size(), 3);
        assert!(c.contains(3));
        assert!(!c.contains(2));
        assert_eq!(c.index_of(3), Some(2));
        assert!(!c.is_contiguous());
        assert!(Committee::contiguous(4).is_contiguous());
    }

    #[test]
    fn leader_rotates_over_members_only() {
        let c = Committee::new([0, 2, 3]);
        let slots: Vec<AuthorityId> = (0..6).map(|r| c.leader(r).unwrap()).collect();
        assert_eq!(slots, vec![0, 2, 3, 0, 2, 3]);
        assert_eq!(Committee::default().leader(7), None);
    }

    #[test]
    fn contiguous_leader_matches_modulo() {
        let c = Committee::contiguous(4);
        for r in 0..20u64 {
            assert_eq!(c.leader(r), Some((r % 4) as AuthorityId));
        }
    }
}
