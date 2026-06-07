//! Indexed (sorted low-leaf) nullifier-set Merkle tree.
//!
//! This is the canonical data structure the L2 STM uses to track
//! *spent* nullifiers and — critically — to prove that a nullifier
//! has **not** been spent yet (non-membership). It is the exact
//! shape the #157 spend circuit reuses: the circuit replays the
//! `verify_*` routines below as in-circuit constraints, so the leaf
//! encoding, hashing, and proof layout here are load-bearing.
//!
//! ## Why an indexed Merkle tree?
//!
//! A plain append-only Merkle tree proves *membership* cheaply but
//! cannot prove *non*-membership without a separate sorted structure
//! or an O(n) scan inside the circuit. The **indexed Merkle tree**
//! (a.k.a. "sorted low-leaf" / Aztec-style nullifier tree) solves
//! this: every leaf stores a pointer to the next-larger value in the
//! set, so the leaves form a sorted singly-linked list embedded in
//! the tree. To prove `v` is absent, exhibit the unique "low leaf"
//! `L` whose value is `< v` and whose successor is `> v` (or is the
//! list tail). Both membership and non-membership reduce to a single
//! Merkle path check plus a small range check — constant work,
//! circuit-friendly.
//!
//! ## Leaf encoding `{ value, next_index, next_value }`
//!
//! Each leaf is the triple:
//!
//! - `value: [u8; 32]` — the nullifier this leaf holds. Interpreted
//!   as a big-endian 256-bit unsigned integer for ordering, which is
//!   identical to byte-lexicographic order on the 32-byte array.
//! - `next_index: u64` — the leaf index of the next-larger value in
//!   the set (the linked-list pointer).
//! - `next_value: [u8; 32]` — a copy of that successor's value, so a
//!   non-membership verifier never has to dereference `next_index`;
//!   it checks the gap `value < v < next_value` from the leaf alone.
//!
//! ### The sentinel and the reserved zero value
//!
//! An *empty* indexed tree is **not** all-zeros. It is seeded with a
//! single sentinel leaf at index 0:
//!
//! ```text
//! leaf[0] = { value: 0, next_index: 0, next_value: 0 }
//! ```
//!
//! This sentinel is the permanent head of the linked list. A
//! `next_value` of `0` is the distinguished **+infinity** marker
//! meaning "this leaf is currently the largest value in the set".
//! Because `0` is overloaded as both the sentinel value and the
//! +infinity tail marker, **the all-zero 32-byte value is reserved
//! and is not a valid nullifier**. Real nullifiers are
//! collision-resistant hash outputs, so `0` occurs with negligible
//! probability; [`insert`](IndexedMerkleTree::insert) rejects it
//! explicitly anyway.
//!
//! ## Hashing
//!
//! All hashing is BLAKE3 under the explicit domain tag
//! [`DOMAIN_TAG`]. Leaf hashing and internal-node hashing are
//! additionally separated by a one-byte discriminator
//! ([`LEAF_PREFIX`] / [`NODE_PREFIX`]) so a leaf preimage can never
//! collide with an internal-node preimage. An *unoccupied* leaf slot
//! commits to the all-zero node (never a leaf preimage), so a claimed
//! leaf — always a non-zero BLAKE3 output — can never be mistaken for
//! an empty slot; this is what keeps non-membership sound. The empty
//! tree is a fixed, non-zero root derived from genuinely seating the
//! sentinel leaf at index 0.
//!
//! ## Ordering / determinism caveat
//!
//! An indexed Merkle tree is **insertion-order dependent**: inserting
//! `{a, b}` and `{b, a}` yields *different* roots, because leaf
//! positions and the linked-list pointers differ. "Root determinism"
//! here means *the same insertion sequence always yields the same
//! root* — not order-independence. The spend circuit commits to a
//! specific historical root, so this is exactly the property we want.

use serde::{Deserialize, Serialize};

/// Height of the tree (number of levels above the leaves). With
/// `HEIGHT = 32` the tree holds up to `2^32` leaves — far beyond any
/// realistic nullifier count, matching the Aztec-standard depth the
/// #157 circuit is sized for.
pub const HEIGHT: usize = 32;

/// Maximum number of leaves the tree can hold (`2^HEIGHT`).
pub const MAX_LEAVES: u64 = 1 << HEIGHT;

/// Base domain-separation tag for the nullifier tree, mixed into
/// every BLAKE3 invocation. Distinct (and uppercase) from the SHA3
/// commitment/nullifier tags in [`crate`] so the two surfaces can
/// never alias.
pub const DOMAIN_TAG: &[u8] = b"SUWAPPU-L2-NULLIFIER-V1";

/// Discriminator byte prepended (after [`DOMAIN_TAG`]) when hashing a
/// leaf preimage. Keeps leaf hashes disjoint from node hashes.
pub const LEAF_PREFIX: u8 = 0x00;

/// Discriminator byte prepended (after [`DOMAIN_TAG`]) when hashing an
/// internal node preimage.
pub const NODE_PREFIX: u8 = 0x01;

/// A leaf in the indexed Merkle tree: a nullifier value plus the
/// linked-list pointer to the next-larger value in the set.
///
/// See the [module docs](self) for the full encoding rationale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Leaf {
    /// The nullifier held by this leaf, ordered as a big-endian
    /// 256-bit unsigned integer. The all-zero value is reserved for
    /// the sentinel / +infinity marker and is not a valid nullifier.
    pub value: [u8; 32],
    /// Leaf index of the next-larger value in the set (linked-list
    /// pointer). `0` together with a zero `next_value` marks the list
    /// tail (this leaf is the current maximum).
    pub next_index: u64,
    /// Copy of the successor leaf's value, or all-zero (+infinity) if
    /// this leaf is the current maximum.
    pub next_value: [u8; 32],
}

impl Leaf {
    /// The seeded sentinel leaf `{ value: 0, next_index: 0,
    /// next_value: 0 }` that heads the linked list of an empty tree.
    pub const SENTINEL: Leaf = Leaf {
        value: [0u8; 32],
        next_index: 0,
        next_value: [0u8; 32],
    };

    /// Hash this leaf to its 32-byte node digest under
    /// `BLAKE3(DOMAIN_TAG || LEAF_PREFIX || value || next_index_be ||
    /// next_value)`.
    fn hash(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(DOMAIN_TAG);
        hasher.update(&[LEAF_PREFIX]);
        hasher.update(&self.value);
        hasher.update(&self.next_index.to_be_bytes());
        hasher.update(&self.next_value);
        *hasher.finalize().as_bytes()
    }
}

/// Hash two child digests into their parent under
/// `BLAKE3(DOMAIN_TAG || NODE_PREFIX || left || right)`.
fn hash_node(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(DOMAIN_TAG);
    hasher.update(&[NODE_PREFIX]);
    hasher.update(left);
    hasher.update(right);
    *hasher.finalize().as_bytes()
}

/// Compare two 32-byte values as big-endian unsigned integers.
/// Identical to byte-lexicographic comparison of the arrays.
fn cmp_value(a: &[u8; 32], b: &[u8; 32]) -> core::cmp::Ordering {
    a.cmp(b)
}

/// `true` iff `v` is the reserved all-zero value.
fn is_zero(v: &[u8; 32]) -> bool {
    v == &[0u8; 32]
}

/// Errors emitted by [`IndexedMerkleTree`] operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TreeError {
    /// The nullifier is already present in the set (double-insert).
    #[error("nullifier already present in the set")]
    Duplicate,
    /// The all-zero value is reserved for the sentinel / +infinity
    /// marker and cannot be inserted as a nullifier.
    #[error("the all-zero value is reserved and cannot be inserted")]
    ReservedZero,
    /// The tree is full (`2^HEIGHT` leaves already occupied).
    #[error("tree is full ({0} leaves)")]
    Full(u64),
}

/// A membership proof: leaf `leaf` sits at `leaf_index` and the
/// `siblings` recompute to a committed root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipProof {
    /// The leaf being proven present.
    pub leaf: Leaf,
    /// The leaf's index in the dense `[0, count)` prefix.
    pub leaf_index: u64,
    /// Sibling digests from leaf level up to (not including) the root,
    /// one per level: `siblings[0]` is the leaf's sibling.
    pub siblings: Vec<[u8; 32]>,
}

/// A non-membership proof for a `query` value: the membership proof
/// of the unique *low leaf* that brackets `query`.
///
/// Verification (see [`IndexedMerkleTree::verify_non_membership`])
/// checks three things: the low leaf is in the tree, `low.value <
/// query`, and `query` falls in the gap before the low leaf's
/// successor (`query < low.next_value`, or the low leaf is the list
/// tail).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NonMembershipProof {
    /// The value being proven absent.
    pub query: [u8; 32],
    /// Membership proof of the bracketing low leaf.
    pub low_leaf_proof: MembershipProof,
}

/// An indexed (sorted low-leaf) Merkle tree over the nullifier set.
///
/// Occupied leaves form a dense append-only prefix `[0, count)`.
/// Internal node digests are cached sparsely in `nodes`; any node not
/// present is the all-empty subtree digest from `empty_hash`. This
/// avoids materializing `2^32` leaves — only the populated paths exist
/// in memory.
#[derive(Debug, Clone)]
pub struct IndexedMerkleTree {
    /// Dense list of occupied leaves, index `i` == `leaves[i]`.
    leaves: Vec<Leaf>,
    /// Sparse cache of computed node digests, keyed by
    /// `(level, index_within_level)`. Level 0 is the leaf level.
    nodes: std::collections::HashMap<(usize, u64), [u8; 32]>,
    /// `empty_hash[level]` is the digest of a fully-empty subtree
    /// rooted at `level`. `empty_hash[0]` is the empty-leaf digest.
    empty_hash: [[u8; 32]; HEIGHT + 1],
}

impl Default for IndexedMerkleTree {
    fn default() -> Self {
        Self::new()
    }
}

impl IndexedMerkleTree {
    /// Build an empty tree: one seeded sentinel leaf at index 0.
    ///
    /// The resulting root is fixed and non-zero; it is the root every
    /// fresh L2 nullifier accumulator starts from.
    pub fn new() -> Self {
        // Precompute empty-subtree digests bottom-up. An unoccupied
        // leaf slot commits to the all-zero node, NOT to any leaf
        // preimage. This is load-bearing for soundness: a `Leaf::hash`
        // is a BLAKE3 output and can never be all-zero, so a claimed
        // leaf can never be mistaken for an empty slot. If empty slots
        // hashed like a real `{0,0,0}` sentinel leaf, an attacker could
        // exhibit any empty slot as a `{0,0,0}` low leaf and forge a
        // non-membership proof for a value that is actually present
        // (a double-spend hole). Everything above level 0 is the hash
        // of two empty children. This precompute is mandatory at
        // HEIGHT = 32 — the tree can never be materialized densely.
        let mut empty_hash = [[0u8; 32]; HEIGHT + 1];
        empty_hash[0] = [0u8; 32];
        for level in 1..=HEIGHT {
            empty_hash[level] = hash_node(&empty_hash[level - 1], &empty_hash[level - 1]);
        }

        let mut tree = IndexedMerkleTree {
            leaves: Vec::new(),
            nodes: std::collections::HashMap::new(),
            empty_hash,
        };
        // Seat the sentinel at index 0 and compute its path.
        tree.leaves.push(Leaf::SENTINEL);
        tree.recompute_path(0);
        tree
    }

    /// Number of occupied leaves (including the sentinel at index 0).
    pub fn len(&self) -> u64 {
        self.leaves.len() as u64
    }

    /// `true` iff the tree holds only the sentinel leaf.
    pub fn is_empty(&self) -> bool {
        self.leaves.len() <= 1
    }

    /// The current Merkle root (32 bytes). Deterministic for a given
    /// insertion sequence.
    pub fn root(&self) -> [u8; 32] {
        self.node(HEIGHT, 0)
    }

    /// Fetch a cached node digest, falling back to the empty-subtree
    /// digest at `level` when the node has never been touched.
    fn node(&self, level: usize, index: u64) -> [u8; 32] {
        self.nodes
            .get(&(level, index))
            .copied()
            .unwrap_or(self.empty_hash[level])
    }

    /// Recompute and cache the digests on the path from leaf
    /// `leaf_index` up to the root, given the current `leaves`.
    fn recompute_path(&mut self, leaf_index: u64) {
        // Level 0: the leaf hash itself.
        let leaf_hash = self.leaves[leaf_index as usize].hash();
        self.nodes.insert((0, leaf_index), leaf_hash);

        let mut index = leaf_index;
        for level in 0..HEIGHT {
            let sibling_index = index ^ 1;
            let (left, right) = if index & 1 == 0 {
                (self.node(level, index), self.node(level, sibling_index))
            } else {
                (self.node(level, sibling_index), self.node(level, index))
            };
            let parent = hash_node(&left, &right);
            let parent_index = index >> 1;
            self.nodes.insert((level + 1, parent_index), parent);
            index = parent_index;
        }
    }

    /// Find the index of the leaf holding exactly `value`, if any.
    /// Linear scan over occupied leaves — O(n) host-side, which is
    /// acceptable because the #157 circuit only reuses the proof /
    /// verify shapes, not this lookup.
    fn find_value(&self, value: &[u8; 32]) -> Option<u64> {
        self.leaves
            .iter()
            .position(|l| &l.value == value)
            .map(|p| p as u64)
    }

    /// Find the *low leaf* for `value`: the unique occupied leaf `L`
    /// with `L.value < value` and (`value < L.next_value` or `L` is
    /// the list tail, i.e. `L.next_value == 0`). Returns its index.
    ///
    /// Assumes `value` is not already present (callers check first)
    /// and `value != 0`. The sentinel guarantees a low leaf always
    /// exists for any `value > 0`.
    fn find_low_leaf(&self, value: &[u8; 32]) -> u64 {
        let mut found = 0u64;
        for (i, leaf) in self.leaves.iter().enumerate() {
            let below = cmp_value(&leaf.value, value) == core::cmp::Ordering::Less;
            let tail = is_zero(&leaf.next_value);
            let before_next = cmp_value(value, &leaf.next_value) == core::cmp::Ordering::Less;
            if below && (tail || before_next) {
                found = i as u64;
                break;
            }
        }
        found
    }

    /// Insert `nullifier` into the set.
    ///
    /// Performs the indexed-tree two-step: (1) splice a fresh leaf
    /// `N = { value, low.next_index, low.next_value }` at the next
    /// append slot, then (2) rewire the low leaf to point at `N`
    /// (`low.next_index = new_index`, `low.next_value = value`). Both
    /// affected paths are recomputed, advancing the root.
    ///
    /// # Errors
    /// - [`TreeError::Duplicate`] if `nullifier` is already present.
    /// - [`TreeError::ReservedZero`] if `nullifier` is all-zero.
    /// - [`TreeError::Full`] if the tree already holds `2^HEIGHT`
    ///   leaves.
    pub fn insert(&mut self, nullifier: [u8; 32]) -> Result<(), TreeError> {
        if is_zero(&nullifier) {
            return Err(TreeError::ReservedZero);
        }
        if self.find_value(&nullifier).is_some() {
            return Err(TreeError::Duplicate);
        }
        if self.len() >= MAX_LEAVES {
            return Err(TreeError::Full(self.len()));
        }

        let low_index = self.find_low_leaf(&nullifier);
        let low = self.leaves[low_index as usize];
        let new_index = self.len();

        // (1) New leaf inherits the low leaf's old successor pointer.
        let new_leaf = Leaf {
            value: nullifier,
            next_index: low.next_index,
            next_value: low.next_value,
        };
        self.leaves.push(new_leaf);
        self.recompute_path(new_index);

        // (2) Low leaf now points at the new leaf.
        let low_mut = &mut self.leaves[low_index as usize];
        low_mut.next_index = new_index;
        low_mut.next_value = nullifier;
        self.recompute_path(low_index);

        Ok(())
    }

    /// `true` iff `nullifier` is present in the set.
    pub fn contains(&self, nullifier: &[u8; 32]) -> bool {
        self.find_value(nullifier).is_some()
    }

    /// Build the Merkle authentication path (sibling digests) for the
    /// leaf at `leaf_index`, leaf level first.
    fn siblings(&self, leaf_index: u64) -> Vec<[u8; 32]> {
        let mut siblings = Vec::with_capacity(HEIGHT);
        let mut index = leaf_index;
        for level in 0..HEIGHT {
            siblings.push(self.node(level, index ^ 1));
            index >>= 1;
        }
        siblings
    }

    /// Produce a membership proof for `nullifier`, or `None` if it is
    /// absent.
    pub fn prove_membership(&self, nullifier: &[u8; 32]) -> Option<MembershipProof> {
        let leaf_index = self.find_value(nullifier)?;
        Some(MembershipProof {
            leaf: self.leaves[leaf_index as usize],
            leaf_index,
            siblings: self.siblings(leaf_index),
        })
    }

    /// Produce a non-membership proof for `query`, or `None` if
    /// `query` is actually present (or is the reserved zero value, for
    /// which absence is not meaningful).
    pub fn prove_non_membership(&self, query: &[u8; 32]) -> Option<NonMembershipProof> {
        if is_zero(query) || self.find_value(query).is_some() {
            return None;
        }
        let low_index = self.find_low_leaf(query);
        let low_leaf_proof = MembershipProof {
            leaf: self.leaves[low_index as usize],
            leaf_index: low_index,
            siblings: self.siblings(low_index),
        };
        Some(NonMembershipProof {
            query: *query,
            low_leaf_proof,
        })
    }

    /// Recompute the root implied by a membership proof. Pure function
    /// of the proof — the verifier shape the #157 circuit replays.
    fn root_from_membership(proof: &MembershipProof) -> [u8; 32] {
        let mut acc = proof.leaf.hash();
        let mut index = proof.leaf_index;
        for sibling in &proof.siblings {
            acc = if index & 1 == 0 {
                hash_node(&acc, sibling)
            } else {
                hash_node(sibling, &acc)
            };
            index >>= 1;
        }
        acc
    }

    /// Verify a membership proof against `root`: the leaf's
    /// authentication path must recompute to `root`, and the path must
    /// have exactly [`HEIGHT`] siblings.
    pub fn verify_membership(root: &[u8; 32], proof: &MembershipProof) -> bool {
        if proof.siblings.len() != HEIGHT {
            return false;
        }
        &Self::root_from_membership(proof) == root
    }

    /// Verify a non-membership proof against `root`.
    ///
    /// All three conditions must hold:
    /// 1. the low leaf is genuinely in the tree (its path recomputes
    ///    to `root`);
    /// 2. `low.value < query` (the query is strictly above the low
    ///    leaf);
    /// 3. `query` lies in the gap before the low leaf's successor:
    ///    either the low leaf is the list tail (`next_value == 0`,
    ///    i.e. +infinity) or `query < low.next_value`.
    ///
    /// The reserved zero `query` is always rejected — absence of the
    /// sentinel value is not a meaningful statement.
    pub fn verify_non_membership(root: &[u8; 32], proof: &NonMembershipProof) -> bool {
        if is_zero(&proof.query) {
            return false;
        }
        let low = &proof.low_leaf_proof.leaf;
        // (1) The low leaf must be in the tree.
        if !Self::verify_membership(root, &proof.low_leaf_proof) {
            return false;
        }
        // (2) low.value < query.
        if cmp_value(&low.value, &proof.query) != core::cmp::Ordering::Less {
            return false;
        }
        // (3) query < low.next_value, or low is the list tail.
        let tail = is_zero(&low.next_value);
        let before_next = cmp_value(&proof.query, &low.next_value) == core::cmp::Ordering::Less;
        tail || before_next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 32-byte big-endian value from a small integer, for legible
    /// ordering in tests.
    fn val(n: u64) -> [u8; 32] {
        let mut v = [0u8; 32];
        v[24..].copy_from_slice(&n.to_be_bytes());
        v
    }

    // ----- empty tree -----

    #[test]
    fn empty_tree_has_only_sentinel() {
        let tree = IndexedMerkleTree::new();
        assert_eq!(tree.len(), 1);
        assert!(tree.is_empty());
    }

    #[test]
    fn empty_tree_root_is_fixed_and_nonzero() {
        let a = IndexedMerkleTree::new();
        let b = IndexedMerkleTree::new();
        // Deterministic across instances.
        assert_eq!(a.root(), b.root());
        // Not the all-zero root — the sentinel seeds a real digest.
        assert_ne!(a.root(), [0u8; 32]);
    }

    #[test]
    fn empty_tree_proves_non_membership_via_sentinel() {
        // With only the sentinel present, any v > 0 is absent and its
        // low leaf is the sentinel (value 0, next_value 0 = +inf).
        let tree = IndexedMerkleTree::new();
        let q = val(42);
        let proof = tree.prove_non_membership(&q).expect("absent");
        assert_eq!(proof.low_leaf_proof.leaf, Leaf::SENTINEL);
        assert!(IndexedMerkleTree::verify_non_membership(
            &tree.root(),
            &proof
        ));
    }

    // ----- single insert -----

    #[test]
    fn single_insert_changes_root_and_count() {
        let mut tree = IndexedMerkleTree::new();
        let before = tree.root();
        tree.insert(val(10)).unwrap();
        assert_eq!(tree.len(), 2);
        assert_ne!(tree.root(), before);
        assert!(tree.contains(&val(10)));
    }

    #[test]
    fn single_insert_rewires_sentinel_pointer() {
        let mut tree = IndexedMerkleTree::new();
        tree.insert(val(10)).unwrap();
        // Sentinel now points at the new leaf at index 1.
        let sentinel = tree.leaves[0];
        assert_eq!(sentinel.next_index, 1);
        assert_eq!(sentinel.next_value, val(10));
        // New leaf is the tail (+inf successor).
        let new_leaf = tree.leaves[1];
        assert_eq!(new_leaf.value, val(10));
        assert_eq!(new_leaf.next_value, [0u8; 32]);
    }

    // ----- membership -----

    #[test]
    fn membership_proof_verifies_after_insert() {
        let mut tree = IndexedMerkleTree::new();
        tree.insert(val(7)).unwrap();
        tree.insert(val(99)).unwrap();
        tree.insert(val(3)).unwrap();
        for v in [val(7), val(99), val(3)] {
            let proof = tree.prove_membership(&v).expect("present");
            assert!(IndexedMerkleTree::verify_membership(&tree.root(), &proof));
        }
    }

    #[test]
    fn membership_proof_absent_value_is_none() {
        let mut tree = IndexedMerkleTree::new();
        tree.insert(val(7)).unwrap();
        assert!(tree.prove_membership(&val(8)).is_none());
    }

    #[test]
    fn membership_proof_fails_against_wrong_root() {
        let mut tree = IndexedMerkleTree::new();
        tree.insert(val(7)).unwrap();
        let proof = tree.prove_membership(&val(7)).unwrap();
        let stale = tree.root();
        tree.insert(val(8)).unwrap();
        // Proof built against the stale root must not verify against
        // the new root.
        assert!(!IndexedMerkleTree::verify_membership(&tree.root(), &proof));
        // ...but still verifies against the root it was built for.
        assert!(IndexedMerkleTree::verify_membership(&stale, &proof));
    }

    #[test]
    fn membership_proof_wrong_length_rejected() {
        let mut tree = IndexedMerkleTree::new();
        tree.insert(val(7)).unwrap();
        let mut proof = tree.prove_membership(&val(7)).unwrap();
        proof.siblings.pop();
        assert!(!IndexedMerkleTree::verify_membership(&tree.root(), &proof));
    }

    #[test]
    fn membership_proof_tampered_leaf_rejected() {
        let mut tree = IndexedMerkleTree::new();
        tree.insert(val(7)).unwrap();
        let mut proof = tree.prove_membership(&val(7)).unwrap();
        proof.leaf.value = val(8);
        assert!(!IndexedMerkleTree::verify_membership(&tree.root(), &proof));
    }

    // ----- non-membership before + after insert -----

    #[test]
    fn non_membership_before_insert_then_membership_after() {
        let mut tree = IndexedMerkleTree::new();
        tree.insert(val(10)).unwrap();
        tree.insert(val(30)).unwrap();

        // 20 sits in the gap (10, 30): absent, provably so.
        let nm = tree.prove_non_membership(&val(20)).expect("absent");
        assert_eq!(nm.low_leaf_proof.leaf.value, val(10));
        assert_eq!(nm.low_leaf_proof.leaf.next_value, val(30));
        assert!(IndexedMerkleTree::verify_non_membership(&tree.root(), &nm));

        // Insert 20 — now it's a member, and the stale non-membership
        // proof no longer verifies against the new root.
        tree.insert(val(20)).unwrap();
        assert!(tree.contains(&val(20)));
        assert!(tree.prove_non_membership(&val(20)).is_none());
        assert!(!IndexedMerkleTree::verify_non_membership(&tree.root(), &nm));

        let m = tree.prove_membership(&val(20)).expect("present");
        assert!(IndexedMerkleTree::verify_membership(&tree.root(), &m));
    }

    #[test]
    fn non_membership_above_max_uses_tail_leaf() {
        let mut tree = IndexedMerkleTree::new();
        tree.insert(val(10)).unwrap();
        tree.insert(val(30)).unwrap();
        // 100 is above the current max; low leaf is the tail (30) with
        // +inf successor.
        let nm = tree.prove_non_membership(&val(100)).expect("absent");
        assert_eq!(nm.low_leaf_proof.leaf.value, val(30));
        assert_eq!(nm.low_leaf_proof.leaf.next_value, [0u8; 32]);
        assert!(IndexedMerkleTree::verify_non_membership(&tree.root(), &nm));
    }

    #[test]
    fn non_membership_present_value_is_none() {
        let mut tree = IndexedMerkleTree::new();
        tree.insert(val(10)).unwrap();
        assert!(tree.prove_non_membership(&val(10)).is_none());
    }

    #[test]
    fn forged_non_membership_for_present_value_is_rejected() {
        // An adversary cannot use the sentinel as a low leaf to claim
        // a value that is actually present is absent: condition (3)
        // fails because the query exceeds the sentinel's next_value.
        let mut tree = IndexedMerkleTree::new();
        tree.insert(val(10)).unwrap();
        tree.insert(val(30)).unwrap();
        // Hand-build a forged proof: sentinel (value 0, next_value 10)
        // as low leaf, query = 30. 30 IS present, and 30 > 10 =
        // sentinel.next_value, so the gap check must reject it.
        let sentinel_proof = MembershipProof {
            leaf: tree.leaves[0],
            leaf_index: 0,
            siblings: tree.siblings(0),
        };
        assert_eq!(sentinel_proof.leaf.next_value, val(10));
        let forged = NonMembershipProof {
            query: val(30),
            low_leaf_proof: sentinel_proof,
        };
        assert!(!IndexedMerkleTree::verify_non_membership(
            &tree.root(),
            &forged
        ));
    }

    #[test]
    fn forged_non_membership_from_empty_slot_is_rejected() {
        // Soundness: an attacker must not be able to pass off an
        // UNOCCUPIED leaf slot as a `{0,0,0}` low leaf. If empty slots
        // committed to the same digest as a real sentinel leaf, the
        // forged path below would recompute to the genuine root and
        // verify_non_membership would falsely accept absence of a
        // value that is actually present — a double-spend hole. The
        // empty-slot digest is the all-zero node, which no BLAKE3 leaf
        // hash can equal, so the forged path diverges at level 0.
        let mut tree = IndexedMerkleTree::new();
        tree.insert(val(10)).unwrap();
        tree.insert(val(30)).unwrap();
        let root = tree.root();
        let empty_index = tree.len(); // first unoccupied slot
        let forged = NonMembershipProof {
            query: val(10), // val(10) IS present
            low_leaf_proof: MembershipProof {
                // {0,0,0}: value 0 < 10, next_value 0 = +inf, so the
                // range checks (2) and (3) would pass — only the path
                // recomputation must reject this.
                leaf: Leaf::SENTINEL,
                leaf_index: empty_index,
                siblings: tree.siblings(empty_index),
            },
        };
        assert!(!IndexedMerkleTree::verify_non_membership(&root, &forged));
    }

    #[test]
    fn non_membership_rejects_reserved_zero_query() {
        let tree = IndexedMerkleTree::new();
        assert!(tree.prove_non_membership(&[0u8; 32]).is_none());
        // Even a hand-built proof with query 0 is rejected by verify.
        let forged = NonMembershipProof {
            query: [0u8; 32],
            low_leaf_proof: MembershipProof {
                leaf: tree.leaves[0],
                leaf_index: 0,
                siblings: tree.siblings(0),
            },
        };
        assert!(!IndexedMerkleTree::verify_non_membership(
            &tree.root(),
            &forged
        ));
    }

    // ----- duplicate-insert rejection -----

    #[test]
    fn duplicate_insert_is_rejected() {
        let mut tree = IndexedMerkleTree::new();
        tree.insert(val(10)).unwrap();
        let root_after_first = tree.root();
        assert_eq!(tree.insert(val(10)), Err(TreeError::Duplicate));
        // Rejected insert leaves the tree (and root) untouched.
        assert_eq!(tree.root(), root_after_first);
        assert_eq!(tree.len(), 2);
    }

    #[test]
    fn reserved_zero_insert_is_rejected() {
        let mut tree = IndexedMerkleTree::new();
        assert_eq!(tree.insert([0u8; 32]), Err(TreeError::ReservedZero));
        assert_eq!(tree.len(), 1);
    }

    // ----- root determinism -----

    #[test]
    fn root_is_deterministic_for_same_sequence() {
        let mut a = IndexedMerkleTree::new();
        let mut b = IndexedMerkleTree::new();
        for v in [val(5), val(17), val(2), val(900)] {
            a.insert(v).unwrap();
            b.insert(v).unwrap();
        }
        assert_eq!(a.root(), b.root());
    }

    #[test]
    fn root_is_order_dependent() {
        // Indexed trees are NOT order-independent: different insertion
        // orders place leaves at different indices with different
        // pointers, so the roots differ.
        let mut a = IndexedMerkleTree::new();
        a.insert(val(5)).unwrap();
        a.insert(val(9)).unwrap();

        let mut b = IndexedMerkleTree::new();
        b.insert(val(9)).unwrap();
        b.insert(val(5)).unwrap();

        assert_ne!(a.root(), b.root());
    }

    #[test]
    fn many_inserts_all_provable() {
        // Stress: insert a spread of values, then prove membership of
        // each and non-membership of values in between.
        let mut tree = IndexedMerkleTree::new();
        let inserted: Vec<[u8; 32]> = (1..=50u64).map(|n| val(n * 10)).collect();
        for v in &inserted {
            tree.insert(*v).unwrap();
        }
        let root = tree.root();
        for v in &inserted {
            let m = tree.prove_membership(v).expect("present");
            assert!(IndexedMerkleTree::verify_membership(&root, &m));
        }
        // Values ending in 5 fall in the gaps — all absent.
        for n in 1..=50u64 {
            let q = val(n * 10 + 5);
            let nm = tree.prove_non_membership(&q).expect("absent");
            assert!(IndexedMerkleTree::verify_non_membership(&root, &nm));
        }
    }

    #[test]
    fn leaf_and_node_hash_domains_are_separated() {
        // A leaf preimage and a node preimage of the same byte shape
        // must not collide. Sentinel leaf hash != node(0,0)-ish hash.
        let leaf = Leaf::SENTINEL.hash();
        let node = hash_node(&[0u8; 32], &[0u8; 32]);
        assert_ne!(leaf, node);
    }
}
